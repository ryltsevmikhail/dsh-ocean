use std::time::Duration;

use anyhow::{Context, Result};
use tauri::{AppHandle, Manager};
use tokio::time::Instant;
use url::Url;

use crate::bootstrap::{self, Runtime};
use crate::runtime_lifecycle::{HealthFailures, Operation};
use crate::state::{AppState, RuntimePermit};
use crate::{auth, config, http, systemd, windows};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerMode {
    Attached,
    Systemd,
    Child,
}

#[derive(Clone, Debug)]
pub struct Server {
    pub base_url: Url,
    pub token: String,
    pub mode: ServerMode,
    pub unit: Option<String>,
    /// Actual argv recipe; never reconstructed from a later preference.
    pub locale_patch: Option<std::path::PathBuf>,
}

impl Server {
    pub fn with_token(&self, token: String) -> Self {
        let mut server = self.clone();
        server.token = token;
        server
    }
    fn new(base_url: Url, token: String, mode: ServerMode) -> Self {
        let unit = match mode {
            ServerMode::Systemd => Some(config::UNIT_NAME.to_string()),
            _ => None,
        };
        Self {
            base_url,
            token,
            mode,
            unit,
            locale_patch: None,
        }
    }
}

pub async fn prepare_and_open(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let Some(permit) = state.begin_prepare()? else {
        return Ok(());
    };
    // A cancelled preparation must not leave Phase::Preparing latched forever.
    struct Attempt<'a> {
        state: &'a AppState,
        finished: bool,
    }
    impl Drop for Attempt<'_> {
        fn drop(&mut self) {
            if !self.finished {
                self.state.reset_idle();
            }
        }
    }
    let mut attempt = Attempt {
        state: &state,
        finished: false,
    };
    let result = async {
        let server = prepare(app, &permit).await?;
        if server.mode != ServerMode::Child {
            permit.publish_server(server.clone())?;
        }
        auth::cache_token(&server.token);
        state.log(
            crate::i18n::render(
                crate::i18n::active_language(),
                "startup.ready",
                &[("url", server.base_url.as_str())],
            )
            .expect("validated startup log arguments"),
        );
        reload_window(app, &permit, server, true).await?;
        Ok(())
    }
    .await;
    if let Err(error) = &result {
        state.fail(format!("{error:#}"));
    }
    attempt.finished = true;
    result
}

/// Keep admission through dispatch acknowledgement, without holding a metadata
/// lock or blocking GTK. A cancelled/timed-out older callback uses its stamp to
/// skip navigation after a newer operation or shutdown.
pub(crate) async fn reload_window(
    app: &AppHandle,
    permit: &RuntimePermit,
    server: Server,
    open: bool,
) -> Result<()> {
    let stamp = permit.stamp();
    let app_for_main = app.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = (|| -> Result<()> {
            anyhow::ensure!(stamp.is_current(), "the DSH window reload was superseded");
            match app_for_main.get_webview_window("main") {
                Some(window) if !open => windows::navigate_to_server(&window, &server)?,
                _ => windows::open_main(&app_for_main, &server)?,
            }
            if let Some(setup) = app_for_main.get_webview_window("setup") {
                let _ = setup.hide();
            }
            app_for_main.state::<AppState>().ready();
            Ok(())
        })();
        let _ = tx.send(result.map_err(|error| format!("{error:#}")));
    })
    .context("schedule DSH window reload")?;
    tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .context("timed out waiting for DSH window reload dispatch")?
        .context("DSH window reload dispatch was dropped")?
        .map_err(anyhow::Error::msg)
}

pub(crate) fn permits_local_start(base: &Url) -> bool {
    if base.scheme() != "http" {
        return false;
    }
    match base.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain("localhost")) => true,
        _ => false,
    }
}

/// Attach without detecting/installing a local runtime or changing a service.
/// Only a 401 origin plus a validated launch token authorizes attachment.
pub fn attach_existing(base: &Url) -> Result<Option<Server>> {
    if http::status(base) != Some(401) {
        return Ok(None);
    }
    let token = auth::resolve_token(base, Some(config::UNIT_NAME))
        .or_else(|_| auth::resolve_token(base, None))?;
    Ok(Some(Server::new(base.clone(), token, ServerMode::Attached)))
}

async fn prepare(app: &AppHandle, permit: &RuntimePermit) -> Result<Server> {
    let state = app.state::<AppState>();
    let base = config::checked_server_url()?;
    if permits_local_start(&base) {
        crate::runtime_switch::ensure_no_pending(&config::data_dir())?;
    }
    // Retrying window dispatch must not provision a second child on the fixed
    // configured origin: an owned child listens on its own ephemeral port.
    if let Some(server) = permit.owned_server()? {
        anyhow::ensure!(
            server.mode == ServerMode::Child,
            "managed child ownership has inconsistent server metadata"
        );
        anyhow::ensure!(
            auth::validate(&server.base_url, &server.token),
            "the owned managed DSH is not ready; use Restart DSH server before retrying"
        );
        state.log(crate::i18n::text("startup.reopening"));
        return Ok(server);
    }
    // Probe before runtime detection: an existing server needs no local Node or
    // Harness installation, and attachment is not permission to rewrite a unit.
    if let Some(server) = attach_existing(&base)? {
        state.log(
            crate::i18n::render(
                crate::i18n::active_language(),
                "startup.attached",
                &[("url", base.as_str())],
            )
            .expect("validated startup log arguments"),
        );
        return Ok(server);
    }

    anyhow::ensure!(
        permits_local_start(&base),
        "the configured remote server is unavailable; refusing to start a local service instead"
    );
    let runtime = bootstrap::detect()
        .context("The DeepSeek Harness runtime was not found. Install it to continue.")?;
    state.log(format!(
        "DSH {} · node {}",
        runtime.version,
        runtime.node.display()
    ));
    if systemd::available() {
        let scope = crate::runtime_controller::systemd_launch_scope()?;
        let previous = config::data_dir()
            .join("builtin-locales")
            .join(crate::builtin_locales::bundle_id())
            .join("overlay.yml");
        // Prove the actual manager slot before any permission UI or cache work.
        systemd::prove_cold_launch(&runtime, &base, Some(&previous))?;
        let patch =
            match crate::builtin_locale_planner::prepare(app, &runtime, permit, &scope).await {
                Ok(patch) => patch,
                Err(error) => {
                    state.log(
                        crate::i18n::render(
                            crate::i18n::active_language(),
                            "startup.languages_failed",
                            &[("error", &format!("{error:#}"))],
                        )
                        .expect("validated startup language diagnostic"),
                    );
                    None
                }
            };
        crate::runtime_controller::validate_systemd_scope(&scope.dsh_home)?;
        state.log(crate::i18n::text("startup.start_service"));
        // A failed ownership/scope/start proof is never authority for another
        // backend against a potentially shared profile.
        return start_with_systemd(
            app,
            &runtime,
            &base,
            permit,
            patch.as_deref(),
            &previous,
            &scope,
        )
        .await;
    }

    // Reached only when the user manager was genuinely unavailable; no failed
    // systemd proof is converted into managed-child launch authority.
    let patch = match crate::builtin_locale_planner::LaunchScope::child() {
        Ok(scope) => {
            match crate::builtin_locale_planner::prepare(app, &runtime, permit, &scope).await {
                Ok(patch) => patch,
                Err(error) => {
                    state.log(
                        crate::i18n::render(
                            crate::i18n::active_language(),
                            "startup.languages_failed",
                            &[("error", &format!("{error:#}"))],
                        )
                        .expect("validated startup language diagnostic"),
                    );
                    None
                }
            }
        }
        Err(error) => {
            state.log(
                crate::i18n::render(
                    crate::i18n::active_language(),
                    "startup.scope_failed",
                    &[("error", &format!("{error:#}"))],
                )
                .expect("validated startup language diagnostic"),
            );
            None
        }
    };
    permit.ensure_open()?;
    crate::runtime_switch::ensure_no_pending(&config::data_dir())?;
    spawn_managed(app, &runtime, permit, patch.as_deref()).await
}

async fn start_with_systemd(
    app: &AppHandle,
    runtime: &Runtime,
    base: &Url,
    permit: &RuntimePermit,
    patch: Option<&std::path::Path>,
    previous: &std::path::Path,
    scope: &crate::builtin_locale_planner::LaunchScope,
) -> Result<Server> {
    let state = app.state::<AppState>();
    permit.ensure_open()?;
    scope.verify()?;
    crate::runtime_controller::validate_systemd_scope(&scope.dsh_home)?;
    systemd::ensure_unit_for_launch(runtime, base, patch, Some(previous))?;
    permit.ensure_open()?;
    scope.verify()?;
    crate::runtime_controller::validate_systemd_scope(&scope.dsh_home)?;
    crate::runtime_switch::ensure_no_pending(&config::data_dir())?;
    systemd::start()?;

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_status = None;
    loop {
        permit.ensure_open()?;
        if let Some(status) = http::status(base) {
            last_status = Some(status);
            if let Ok(token) = auth::resolve_token(base, Some(config::UNIT_NAME)) {
                state.log(
                    crate::i18n::render(
                        crate::i18n::active_language(),
                        "startup.service_ready",
                        &[("url", base.as_str())],
                    )
                    .expect("validated startup log arguments"),
                );
                let mut server = Server::new(base.clone(), token, ServerMode::Systemd);
                server.locale_patch = patch.map(std::path::Path::to_path_buf);
                return Ok(server);
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "{} did not become ready within 30s (last HTTP status: {last_status:?})",
                config::UNIT_NAME
            );
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

async fn spawn_managed(
    app: &AppHandle,
    runtime: &Runtime,
    permit: &RuntimePermit,
    patch: Option<&std::path::Path>,
) -> Result<Server> {
    permit.ensure_open()?;
    let state = app.state::<AppState>();
    state.log(crate::i18n::text("startup.start_child"));
    let log_app = app.clone();
    let log: crate::managed_child::Log = std::sync::Arc::new(move |line: String| {
        log_app.state::<AppState>().log(line);
    });
    let server = spawn_managed_logged_with_patch(runtime, permit, log, patch).await?;
    state.log(
        crate::i18n::render(
            crate::i18n::active_language(),
            "startup.child_ready",
            &[("url", server.base_url.as_str())],
        )
        .expect("validated startup log arguments"),
    );
    Ok(server)
}

/// Spawn and publish a managed child from an explicit runtime without touching
/// the app or the log sink. The owned switch engine uses this to start a freshly
/// selected generation without `bootstrap::detect()`.
pub fn managed_arguments(patch: Option<&std::path::Path>) -> Result<Vec<String>> {
    let mut args = [
        "--profile",
        "web",
        "--host",
        "127.0.0.1",
        "--port",
        "0",
        "--no-open",
    ]
    .map(str::to_owned)
    .to_vec();
    if let Some(patch) = patch {
        anyhow::ensure!(
            patch.is_absolute()
                && !patch
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir)),
            "absolute locale patch path required"
        );
        // Commander forwards everything after the first web-app option.
        args.splice(
            2..2,
            [
                "--patch".to_owned(),
                patch
                    .to_str()
                    .context("locale patch path must be UTF-8")?
                    .to_owned(),
            ],
        );
    }
    Ok(args)
}

pub async fn spawn_managed_logged_with_patch(
    runtime: &Runtime,
    permit: &RuntimePermit,
    log: crate::managed_child::Log,
    patch: Option<&std::path::Path>,
) -> Result<Server> {
    permit.ensure_open()?;
    let args = managed_arguments(patch)?;
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let started = crate::managed_child::start(
        &runtime.node,
        &runtime.dsh,
        &args,
        &bootstrap::path_env(Some(&runtime.node)),
        Duration::from_secs(45),
        log,
    )
    .await?;
    let crate::managed_child::ManagedChild {
        child,
        base_url,
        token,
    } = started;
    let mut server = Server::new(base_url, token, ServerMode::Child);
    server.locale_patch = patch.map(std::path::Path::to_path_buf);
    if let Err((error, mut child)) = permit.publish_child(server.clone(), child) {
        crate::managed_child::stop_child(&mut child)
            .await
            .context("could not clean up rejected managed DSH")?;
        return Err(error.into());
    }
    auth::cache_token(&server.token);
    Ok(server)
}

/// What recovery does for a wedged server in systemd mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemdRecovery {
    Start,
    Restart,
}

/// A unit can be active while the server inside it is wedged, so an inactive-check
/// alone would log recovery forever without acting.
pub fn systemd_recovery(active: bool) -> SystemdRecovery {
    if active {
        SystemdRecovery::Restart
    } else {
        SystemdRecovery::Start
    }
}

/// Watch the attached server and recover a managed child or an inactive
/// systemd unit. Attached third-party servers are left untouched.
pub fn start_health_monitor(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut failures = HealthFailures::default();
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let state = app.state::<AppState>();
            let Some((revision, server)) = state.runtime.observation() else {
                failures.reset();
                continue;
            };
            if server.mode == ServerMode::Attached
                || crate::runtime_switch::ensure_no_pending(&config::data_dir()).is_err()
            {
                failures.reset();
                continue;
            }
            let reachable = http::status(&server.base_url).is_some();
            if reachable || !state.runtime.is_idle_revision(revision) {
                failures.reset();
                continue;
            }
            if !failures.failed(revision) {
                continue;
            }
            let Ok(permit) = state.runtime.try_recover(revision) else {
                continue;
            };
            // The decision and fresh server lookup now belong to this operation.
            // A transiently recovered server must not be restarted from old probes.
            let result = recover_under_permit(&app, &permit).await;
            if let Err(error) = result {
                state.log(
                    crate::i18n::render(
                        crate::i18n::active_language(),
                        "startup.recovery_failed",
                        &[("error", &format!("{error:#}"))],
                    )
                    .expect("validated startup log arguments"),
                );
            }
        }
    });
}

async fn recover_under_permit(app: &AppHandle, permit: &RuntimePermit) -> Result<()> {
    crate::runtime_switch::ensure_no_pending(&config::data_dir())?;
    let Some(server) = permit.server()? else {
        return Ok(());
    };
    if server.mode == ServerMode::Attached || http::status(&server.base_url).is_some() {
        return Ok(());
    }
    app.state::<AppState>()
        .log(crate::i18n::text("startup.recovery_attempt"));
    match server.mode {
        ServerMode::Child => restart_under_permit(app, permit).await?,
        ServerMode::Systemd => {
            permit.ensure_open()?;
            match systemd_recovery(systemd::is_active()) {
                SystemdRecovery::Start => systemd::start()?,
                SystemdRecovery::Restart => systemd::restart()?,
            }
            reload_systemd_server(app, &server.base_url, permit).await?;
        }
        ServerMode::Attached => {}
    }
    Ok(())
}

pub async fn restart_and_reload(app: &AppHandle) -> Result<()> {
    let permit = app
        .state::<AppState>()
        .runtime
        .try_begin(Operation::Restart)?;
    restart_under_permit(app, &permit).await
}

fn ensure_owned_restart(server: &Server) -> Result<()> {
    anyhow::ensure!(
        server.mode != ServerMode::Attached,
        crate::i18n::text("startup.attached_restart_refused")
    );
    Ok(())
}

async fn restart_under_permit(app: &AppHandle, permit: &RuntimePermit) -> Result<()> {
    crate::runtime_switch::ensure_no_pending(&config::data_dir())?;
    let state = app.state::<AppState>();
    let server = permit
        .server()?
        .context("there is no running DSH server to restart")?;

    // A loopback endpoint or active unit does not prove ownership of an attachment.
    ensure_owned_restart(&server)?;
    state.log(crate::i18n::text("startup.restarting"));
    match server.mode {
        ServerMode::Child => {
            let mut lease = permit.lease_child()?;
            if let Some(child) = lease.child_mut() {
                crate::managed_child::stop_child(child)
                    .await
                    .context("could not stop managed DSH; refusing replacement")?;
            }
            // Errors/cancellation before here restore the old handle via the lease.
            lease.discard_stopped();
            permit.ensure_open()?;
            let runtime = bootstrap::detect().context("DSH runtime disappeared")?;
            let new_server =
                spawn_managed(app, &runtime, permit, server.locale_patch.as_deref()).await?;
            reload_window(app, permit, new_server, false).await?;
        }
        ServerMode::Systemd | ServerMode::Attached => {
            anyhow::ensure!(
                permits_local_start(&server.base_url),
                "a remote server cannot be restarted through the local user service"
            );
            if !systemd::is_active() {
                anyhow::bail!(
                    "this DSH server is not managed by dsh-web.service; restart it where it was started"
                );
            }
            permit.ensure_open()?;
            systemd::restart()?;
            reload_systemd_server(app, &server.base_url, permit).await?;
        }
    }
    state.log(crate::i18n::text("startup.restarted"));
    Ok(())
}

/// Reattach after an independent update worker already restarted (or rolled
/// back) the user unit. Never restart it a second time from the GUI.
pub async fn refresh_after_update(app: &AppHandle, permit: &RuntimePermit) -> Result<()> {
    anyhow::ensure!(
        permit.operation() == Operation::Update,
        "update ownership is required for refresh"
    );
    let base = permit.server()?.context("no server to refresh")?.base_url;
    anyhow::ensure!(systemd::is_active(), "updated service is not active");
    reload_systemd_server(app, &base, permit).await
}

fn refreshed_service_server(server: Option<Server>, base: &Url, token: String) -> Result<Server> {
    let server = server.context("service refresh requires captured server metadata")?;
    anyhow::ensure!(
        server.base_url == *base
            && matches!(server.mode, ServerMode::Systemd | ServerMode::Attached),
        "service refresh cannot replace an unrelated server or child recipe"
    );
    // An explicit restart authorizes that action, not ownership of an attached
    // runtime. Only an already proven owned server carries an owned recipe.
    Ok(server.with_token(token))
}

pub(crate) async fn reload_systemd_server(
    app: &AppHandle,
    base: &Url,
    permit: &RuntimePermit,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        permit.ensure_open()?;
        if let Ok(token) = auth::resolve_token(base, Some(config::UNIT_NAME)) {
            let new_server = refreshed_service_server(permit.server()?, base, token)?;
            auth::cache_token(&new_server.token);
            permit.publish_server(new_server.clone())?;
            reload_window(app, permit, new_server, false).await?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("the restarted DSH service did not become ready");
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn token_refresh_never_promotes_attachment_or_invents_a_plain_owned_recipe() {
        let base = Url::parse("http://127.0.0.1:3080/").unwrap();
        let attached = Server::new(base.clone(), "old".into(), ServerMode::Attached);
        let refreshed = refreshed_service_server(Some(attached), &base, "new".into()).unwrap();
        assert_eq!(refreshed.mode, ServerMode::Attached);
        assert_eq!(refreshed.unit, None);
        assert_eq!(refreshed.token, "new");
        let mut owned = Server::new(base.clone(), "old".into(), ServerMode::Systemd);
        owned.locale_patch = Some(std::path::PathBuf::from("/private/old/overlay.yml"));
        let refreshed = refreshed_service_server(Some(owned.clone()), &base, "new".into()).unwrap();
        assert_eq!(refreshed.locale_patch, owned.locale_patch);
        assert_eq!(refreshed.mode, ServerMode::Systemd);
        assert!(refreshed_service_server(None, &base, "new".into()).is_err());
        assert!(refreshed_service_server(
            Some(owned),
            &Url::parse("http://127.0.0.1:3081/").unwrap(),
            "new".into()
        )
        .is_err());
        assert!(refreshed_service_server(
            Some(Server::new(base.clone(), "old".into(), ServerMode::Child)),
            &base,
            "new".into()
        )
        .is_err());
    }
    #[test]
    fn remote_origins_never_authorize_local_service_fallback() {
        for origin in [
            "http://192.168.1.50:3080/",
            "http://example.test/",
            "http://0.0.0.0:3080/",
            "https://127.0.0.1/",
        ] {
            assert!(
                !permits_local_start(&Url::parse(origin).unwrap()),
                "{origin}"
            );
        }
        for origin in [
            "http://127.0.0.1:3080/",
            "http://localhost:3080/",
            "http://[::1]:3080/",
        ] {
            assert!(
                permits_local_start(&Url::parse(origin).unwrap()),
                "{origin}"
            );
        }
    }

    #[test]
    fn attached_metadata_never_authorizes_a_local_restart() {
        let base = Url::parse("http://127.0.0.1:9/").unwrap();
        assert!(ensure_owned_restart(&Server::new(
            base.clone(),
            "synthetic".into(),
            ServerMode::Attached
        ))
        .is_err());
        for mode in [ServerMode::Systemd, ServerMode::Child] {
            assert!(
                ensure_owned_restart(&Server::new(base.clone(), "synthetic".into(), mode)).is_ok()
            );
        }
    }
    #[test]
    fn a_wedged_but_active_unit_is_restarted_not_ignored() {
        assert_eq!(systemd_recovery(true), SystemdRecovery::Restart);
        assert_eq!(systemd_recovery(false), SystemdRecovery::Start);
    }
}

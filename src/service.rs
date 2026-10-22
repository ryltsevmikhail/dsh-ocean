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
}

impl Server {
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
        state.log(format!("DSH is ready at {}", server.base_url));
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
        state.log("Reopening the existing managed DSH window");
        return Ok(server);
    }
    // Probe before runtime detection: an existing server needs no local Node or
    // Harness installation, and attachment is not permission to rewrite a unit.
    if let Some(server) = attach_existing(&base)? {
        state.log(format!("Found an existing DSH server at {base}; attaching"));
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
        state.log("Starting the background DSH service…");
        match start_with_systemd(app, &runtime, &base, permit).await {
            Ok(server) => return Ok(server),
            Err(error) => state.log(format!(
                "systemd service did not start: {error:#}; falling back to a managed process"
            )),
        }
    }

    spawn_managed(app, &runtime, permit).await
}

async fn start_with_systemd(
    app: &AppHandle,
    runtime: &Runtime,
    base: &Url,
    permit: &RuntimePermit,
) -> Result<Server> {
    let state = app.state::<AppState>();
    permit.ensure_open()?;
    systemd::ensure_unit(runtime, base)?;
    systemd::start()?;

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_status = None;
    loop {
        permit.ensure_open()?;
        if let Some(status) = http::status(base) {
            last_status = Some(status);
            if let Ok(token) = auth::resolve_token(base, Some(config::UNIT_NAME)) {
                state.log(format!("Background service is ready at {base}"));
                return Ok(Server::new(base.clone(), token, ServerMode::Systemd));
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
) -> Result<Server> {
    permit.ensure_open()?;
    let state = app.state::<AppState>();
    state.log("Starting a managed DSH web process…");
    let log_app = app.clone();
    let log: crate::managed_child::Log = std::sync::Arc::new(move |line: String| {
        log_app.state::<AppState>().log(line);
    });
    let server = spawn_managed_logged(runtime, permit, log).await?;
    state.log(format!("Managed DSH is ready at {}", server.base_url));
    Ok(server)
}

/// Spawn and publish a managed child from an explicit runtime without touching
/// the app or the log sink. The owned switch engine uses this to start a freshly
/// selected generation without `bootstrap::detect()`.
pub(crate) async fn spawn_managed_logged(
    runtime: &Runtime,
    permit: &RuntimePermit,
    log: crate::managed_child::Log,
) -> Result<Server> {
    permit.ensure_open()?;
    let started = crate::managed_child::start(
        &runtime.node,
        &runtime.dsh,
        &[
            "--profile",
            "web",
            "--host",
            "127.0.0.1",
            "--port",
            "0",
            "--no-open",
        ],
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
    let server = Server::new(base_url, token, ServerMode::Child);
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
                state.log(format!("DSH recovery failed: {error:#}"));
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
        .log("DSH server stopped responding; attempting recovery");
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

async fn restart_under_permit(app: &AppHandle, permit: &RuntimePermit) -> Result<()> {
    crate::runtime_switch::ensure_no_pending(&config::data_dir())?;
    let state = app.state::<AppState>();
    let server = permit
        .server()?
        .context("there is no running DSH server to restart")?;

    state.log("Restarting DSH…");
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
            let new_server = spawn_managed(app, &runtime, permit).await?;
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
    state.log("DSH restarted");
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

pub(crate) async fn reload_systemd_server(
    app: &AppHandle,
    base: &Url,
    permit: &RuntimePermit,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        permit.ensure_open()?;
        if let Ok(token) = auth::resolve_token(base, Some(config::UNIT_NAME)) {
            auth::cache_token(&token);
            let new_server = Server::new(base.clone(), token, ServerMode::Systemd);
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
    fn a_wedged_but_active_unit_is_restarted_not_ignored() {
        assert_eq!(systemd_recovery(true), SystemdRecovery::Restart);
        assert_eq!(systemd_recovery(false), SystemdRecovery::Start);
    }
}

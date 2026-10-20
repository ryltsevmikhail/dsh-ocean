use std::time::Duration;

use anyhow::{Context, Result};
use tauri::{AppHandle, Manager};
use tokio::time::Instant;
use url::Url;

use crate::bootstrap::{self, Runtime};
use crate::state::AppState;
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
    {
        let state = app.state::<AppState>();
        if !state.begin_prepare() {
            return Ok(());
        }
    }

    let state = app.state::<AppState>();
    match prepare(app).await {
        Ok(server) => {
            auth::cache_token(&server.token);
            state.log(format!("DSH is ready at {}", server.base_url));
            state.set_server(server.clone());

            let app_for_main = app.clone();
            let server_for_main = server.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                match windows::open_main(&app_for_main, &server_for_main) {
                    Ok(()) => {
                        if let Some(setup) = app_for_main.get_webview_window("setup") {
                            let _ = setup.hide();
                        }
                        app_for_main.state::<AppState>().ready();
                    }
                    Err(error) => app_for_main
                        .state::<AppState>()
                        .fail(format!("Could not open the DSH window: {error}")),
                }
            }) {
                state.fail(format!("Could not schedule the DSH window: {error}"));
                anyhow::bail!("could not schedule window creation: {error}");
            }
            Ok(())
        }
        Err(error) => {
            state.fail(format!("{error:#}"));
            Err(error)
        }
    }
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

async fn prepare(app: &AppHandle) -> Result<Server> {
    let state = app.state::<AppState>();
    let base = config::checked_server_url()?;
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
        match start_with_systemd(app, &runtime, &base).await {
            Ok(server) => return Ok(server),
            Err(error) => state.log(format!(
                "systemd service did not start: {error:#}; falling back to a managed process"
            )),
        }
    }

    spawn_managed(app, &runtime).await
}

async fn start_with_systemd(app: &AppHandle, runtime: &Runtime, base: &Url) -> Result<Server> {
    let state = app.state::<AppState>();
    systemd::ensure_unit(runtime, base)?;
    systemd::start()?;

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_status = None;
    loop {
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

async fn spawn_managed(app: &AppHandle, runtime: &Runtime) -> Result<Server> {
    let state = app.state::<AppState>();
    state.log("Starting a managed DSH web process…");
    let log_app = app.clone();
    let log: crate::managed_child::Log = std::sync::Arc::new(move |line: String| {
        log_app.state::<AppState>().log(line);
    });
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
    state.set_child(child);
    auth::cache_token(&token);
    state.log(format!("Managed DSH is ready at {base_url}"));
    Ok(Server::new(base_url, token, ServerMode::Child))
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
        let mut failures = 0u8;
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let state = app.state::<AppState>();
            if state.is_updating() {
                continue;
            }
            let Some(server) = state.server() else {
                continue;
            };
            if http::status(&server.base_url).is_some() {
                failures = 0;
                continue;
            }
            failures += 1;
            if failures < 2 {
                continue;
            }
            failures = 0;
            state.log("DSH server stopped responding; attempting recovery");
            match server.mode {
                ServerMode::Child => {
                    if let Err(error) = restart_and_reload(&app).await {
                        state.log(format!("managed DSH recovery failed: {error:#}"));
                    }
                }
                ServerMode::Systemd => {
                    let started = match systemd_recovery(systemd::is_active()) {
                        SystemdRecovery::Restart => {
                            let result = systemd::restart();
                            if result.is_ok() {
                                state.log("restarted the unresponsive dsh-web.service");
                            }
                            result
                        }
                        SystemdRecovery::Start => {
                            let result = systemd::start();
                            if result.is_ok() {
                                state.log("started the stopped dsh-web.service");
                            }
                            result
                        }
                    };
                    if let Err(error) = started {
                        state.log(format!("could not recover dsh-web.service: {error:#}"));
                    }
                }
                ServerMode::Attached => {}
            }
        }
    });
}

pub async fn restart_and_reload(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let server = state
        .server()
        .context("there is no running DSH server to restart")?;

    state.log("Restarting DSH…");
    match server.mode {
        ServerMode::Child => {
            if let Some(mut child) = state.take_child() {
                if let Err(error) = crate::managed_child::stop_child(&mut child).await {
                    state.set_child(child);
                    return Err(error).context("could not stop managed DSH; refusing replacement");
                }
            }
            let runtime = bootstrap::detect().context("DSH runtime disappeared")?;
            let new_server = spawn_managed(app, &runtime).await?;
            state.set_server(new_server.clone());
            let app_for_main = app.clone();
            let app_for_window = app_for_main.clone();
            app_for_main.run_on_main_thread(move || {
                if let Some(window) = app_for_window.get_webview_window("main") {
                    if let Err(error) = windows::navigate_to_server(&window, &new_server) {
                        app_for_window
                            .state::<AppState>()
                            .log(format!("could not reload DSH window: {error}"));
                    }
                }
            })?;
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
            systemd::restart()?;
            reload_systemd_server(app, &server.base_url).await?;
        }
    }
    state.log("DSH restarted");
    Ok(())
}

/// Reattach after an independent update worker already restarted (or rolled
/// back) the user unit. Never restart it a second time from the GUI.
pub async fn refresh_after_update(app: &AppHandle) -> Result<()> {
    let base = app
        .state::<AppState>()
        .server()
        .context("no server to refresh")?
        .base_url;
    anyhow::ensure!(systemd::is_active(), "updated service is not active");
    reload_systemd_server(app, &base).await
}

async fn reload_systemd_server(app: &AppHandle, base: &Url) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(token) = auth::resolve_token(base, Some(config::UNIT_NAME)) {
            auth::cache_token(&token);
            let new_server = Server::new(base.clone(), token, ServerMode::Systemd);
            app.state::<AppState>().set_server(new_server.clone());
            let app_for_main = app.clone();
            let app_for_window = app.clone();
            app_for_main.run_on_main_thread(move || {
                if let Some(window) = app_for_window.get_webview_window("main") {
                    if let Err(error) = windows::navigate_to_server(&window, &new_server) {
                        app_for_window
                            .state::<AppState>()
                            .log(format!("could not reload DSH window: {error}"));
                    }
                }
            })?;
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

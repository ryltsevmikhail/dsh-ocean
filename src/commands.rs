use std::process::Stdio;

use anyhow::{bail, Context, Result};
use tauri::{AppHandle, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};

use crate::bootstrap_supervisor::{self, Outcome};
use crate::state::{AppState, SetupState};
use crate::{bootstrap, diagnostics, service};

/// Only the pages that actually own a control may invoke it.
///
/// The `main` window loads the harness origin, so any script running there (a
/// third-party profile bundle, a session artifact) shares `__TAURI_INTERNALS__`
/// with the app. Tauri re-checks a plugin's ACL but not a non-plugin command's
/// origin, so each mutating command names the window allowed to call it.
fn require_window(window: &tauri::WebviewWindow, allowed: &[&str]) -> Result<(), String> {
    let label = window.label();
    if allowed.contains(&label) {
        Ok(())
    } else {
        Err(format!(
            "this command is not available to the {label} window"
        ))
    }
}

/// Pure boundary used by the actual IPC command; labels come from Tauri, never arguments.
pub fn native_locale_snapshot(
    label: &str,
    language: crate::i18n::Language,
) -> Result<serde_json::Value, String> {
    if !["setup", "remote"].contains(&label) {
        return Err("native catalogs are only available to owned pages".into());
    }
    Ok(serde_json::json!({"language":language.code(),"messages":crate::i18n::messages(language)}))
}
pub fn language_settings_allowed(label: &str) -> bool {
    label == "setup"
}

#[tauri::command]
pub fn get_native_locale(window: tauri::WebviewWindow) -> Result<serde_json::Value, String> {
    native_locale_snapshot(window.label(), crate::i18n::active_language())
}
#[tauri::command]
pub fn open_language_settings(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    if !language_settings_allowed(window.label()) {
        return Err("language settings are only available to setup".into());
    }
    crate::localization_ui::show(&app);
    Ok(())
}

#[tauri::command(async)]
pub fn get_setup_state(window: tauri::WebviewWindow, state: State<'_, AppState>) -> SetupState {
    // Errors are ignored on purpose: a foreign caller gets the plain snapshot,
    // which carries no control, instead of an error string it did not expect.
    let _ = require_window(&window, &["setup"]);
    // Runs on the async runtime, not the GTK main thread: detection spawns a
    // subprocess, and the setup window polls this every 700 ms.
    state.snapshot()
}

#[tauri::command]
pub async fn prepare_service(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<SetupState, String> {
    require_window(&window, &["setup"])?;
    service::prepare_and_open(&app)
        .await
        .map_err(|error| format!("{error:#}"))?;
    Ok(app.state::<AppState>().snapshot())
}

/// Only the bundled setup page may request the existing native recovery prompt.
/// The renderer supplies no paths, tokens or journal contents as authority.
#[tauri::command]
pub fn recover_runtime(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_window(&window, &["setup"])?;
    crate::update::recover_and_show(&app);
    Ok(())
}

/// Clears the installer lifecycle state whenever the bootstrap scope ends.
struct BootstrapLifecycle<'a>(&'a AppState);

impl<'a> BootstrapLifecycle<'a> {
    fn new(state: &'a AppState) -> Self {
        Self(state)
    }
}

impl Drop for BootstrapLifecycle<'_> {
    fn drop(&mut self) {
        self.0.installer_finished();
        self.0.set_bootstrap_pgid(None);
    }
}

#[tauri::command]
pub async fn start_bootstrap(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<SetupState, String> {
    require_window(&window, &["setup"])?;
    // One-shot ownership: a second click/command cannot start a competing
    // download. The gate covers only the installer child; service preparation
    // below is a separate step, so cancellation cannot claim to stop it.
    let Some(_gate) = app.state::<AppState>().begin_bootstrap() else {
        return Ok(app.state::<AppState>().snapshot());
    };
    let outcome = run_bootstrap(&app).await;
    match outcome {
        Ok(Outcome::Committed) => {
            if let Err(error) = service::prepare_and_open(&app).await {
                app.state::<AppState>().fail(format!("{error:#}"));
            }
        }
        Ok(Outcome::Cancelled) => {
            let state = app.state::<AppState>();
            state.reset_idle();
            if bootstrap::detect().is_some() {
                // The script can commit before a late signal arrives; report what
                // is actually on disk instead of claiming a rollback.
                state.log(crate::i18n::text("startup.install_interrupted_selected"));
            } else {
                state.log(crate::i18n::text("startup.install_cancelled_unselected"));
            }
        }
        Ok(Outcome::Failed) => {
            app.state::<AppState>()
                .fail(crate::i18n::text("startup.bootstrap_failed"));
        }
        Err(error) => app.state::<AppState>().fail(format!("{error:#}")),
    }
    Ok(app.state::<AppState>().snapshot())
}

/// Requests cooperative cancellation of a running runtime installation.
/// Returns false when nothing is running, because a finished installer's
/// outcome must not be rewritten by a late click.
#[tauri::command]
pub fn cancel_bootstrap(window: tauri::WebviewWindow, state: State<'_, AppState>) -> bool {
    if require_window(&window, &["setup"]).is_err() {
        return false;
    }
    if !state.request_bootstrap_cancel() {
        return false;
    }
    state.log(crate::i18n::text("startup.cancel_requested"));
    true
}

#[tauri::command]
pub fn save_diagnostics(window: tauri::WebviewWindow, app: AppHandle) {
    if require_window(&window, &["setup"]).is_err() {
        return;
    }
    diagnostics::save_and_show(&app);
}

#[tauri::command]
pub fn open_diagnostics_folder(window: tauri::WebviewWindow) {
    if require_window(&window, &["setup"]).is_err() {
        return;
    }
    diagnostics::open_folder();
}

#[tauri::command]
pub fn close_pairing(window: tauri::WebviewWindow, app: AppHandle) {
    if require_window(&window, &["remote"]).is_err() {
        return;
    }
    if let Some(window) = app.get_webview_window("remote") {
        let _ = window.hide();
    }
}

#[tauri::command]
pub async fn restart_service(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_window(&window, &["setup"])?;
    service::restart_and_reload(&app)
        .await
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
pub fn quit_app(window: tauri::WebviewWindow, app: AppHandle) {
    if require_window(&window, &["setup"]).is_err() {
        return;
    }
    app.state::<AppState>()
        .quitting
        .store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(0);
}

async fn run_bootstrap(app: &AppHandle) -> Result<Outcome> {
    let state = app.state::<AppState>();
    state.log(crate::i18n::text("startup.installing"));
    state.log(crate::i18n::text("startup.one_time"));

    let manifest = bootstrap::manifest().context("read runtime manifest")?;
    let arch = match std::env::consts::ARCH {
        "x86_64" => "linux-x64",
        "aarch64" => "linux-arm64",
        other => anyhow::bail!("unsupported bootstrap architecture: {other}"),
    };
    let expected_node_sha = bootstrap::node_archive_sha256(arch)
        .context("runtime manifest has no checksum for this architecture")?;

    let script = bootstrap::install_script();
    let mut child = tokio::process::Command::new("bash")
        .arg("-s")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("DSH_NODE_VERSION", &manifest.node.version)
        .env("DSH_NODE_SHA256", expected_node_sha)
        .env("DSH_VERSION", &manifest.dsh.version)
        .env("DSH_PNPM_VERSION", &manifest.pnpm.version)
        .env("DSH_OCEAN_HEALTH_JS", bootstrap::health_script())
        .kill_on_drop(true)
        .spawn()
        .context("spawn bootstrap script")?;
    let state = app.state::<AppState>().inner();
    state.installer_started();
    state.set_bootstrap_pgid(child.id().map(|pid| pid as i32));
    // Everything below can fail (a broken stdin pipe, a vanished stream, a wait
    // error). The guard clears the process-group id and the installer flag on every
    // path, so a stale pgid is never signalled at shutdown and the UI cannot stay
    // stuck on "installing".
    let _lifecycle = BootstrapLifecycle::new(state);

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .await
            .context("feed bootstrap script")?;
        stdin.shutdown().await.context("close bootstrap stdin")?;
    }

    let stdout = child.stdout.take().context("capture bootstrap stdout")?;
    let stderr = child.stderr.take().context("capture bootstrap stderr")?;
    let stdout_task = tauri::async_runtime::spawn(read_stream(stdout, app.clone()));
    let stderr_task = tauri::async_runtime::spawn(read_stream(stderr, app.clone()));

    let supervised = bootstrap_supervisor::supervise(
        child,
        app.state::<AppState>().bootstrap_cancel_flag(),
        bootstrap_supervisor::DEFAULT_GRACE,
    )
    .await
    .context("supervise bootstrap")?;
    // A surviving grandchild can hold the output pipes forever; never block the
    // command (and the one-shot gate) on them.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), stdout_task).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), stderr_task).await;

    match supervised.outcome() {
        Outcome::Cancelled => {
            if supervised.escalated {
                state.log(crate::i18n::text("startup.forcibly_terminated"));
            } else {
                state.log(crate::i18n::text("startup.cooperatively_stopped"));
            }
            state.log(crate::i18n::text("startup.cancelled"));
            Ok(Outcome::Cancelled)
        }
        Outcome::Committed => {
            if bootstrap::detect().is_none() {
                bail!("runtime bootstrap finished but DSH could not be detected");
            }
            state.log(crate::i18n::text("startup.installed"));
            Ok(Outcome::Committed)
        }
        Outcome::Failed => {
            bail!("runtime bootstrap exited with {}", supervised.status)
        }
    }
}

async fn read_stream<R>(reader: R, app: AppHandle)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        app.state::<AppState>().log(format!("[bootstrap] {line}"));
    }
}

/// Starts a short-lived LAN forwarder for the loopback server.
///
/// Only explicit user action reaches this; the TTL is bounded and the proxy stops
/// itself. The launch token stays the only access control.
#[tauri::command]
pub fn start_pairing_proxy(
    window: tauri::WebviewWindow,
    app: AppHandle,
    ttl_seconds: u64,
    port: Option<u16>,
) -> Result<crate::pairing_proxy::Status, String> {
    require_window(&window, &["remote"])?;
    let ttl = std::time::Duration::from_secs(ttl_seconds.clamp(60, 3600));
    let port = port.unwrap_or(3091).max(1024);
    let bind: std::net::SocketAddr = format!("0.0.0.0:{port}")
        .parse()
        .map_err(|_| "invalid proxy port".to_string())?;
    app.state::<crate::state::AppState>()
        .start_pairing(ttl, bind)
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
pub fn stop_pairing_proxy(window: tauri::WebviewWindow, app: AppHandle) -> bool {
    if require_window(&window, &["remote"]).is_err() {
        return false;
    }
    app.state::<crate::state::AppState>().stop_pairing()
}

#[tauri::command]
pub fn pairing_proxy_status(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Option<crate::pairing_proxy::Status> {
    if require_window(&window, &["remote"]).is_err() {
        return None;
    }
    app.state::<crate::state::AppState>().pairing_status()
}

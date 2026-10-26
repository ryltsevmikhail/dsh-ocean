use anyhow::{Context, Result};
use tauri::{AppHandle, Manager, State};

use crate::bootstrap_supervisor::Outcome;
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
struct BootstrapLifecycle(AppHandle);

impl Drop for BootstrapLifecycle {
    fn drop(&mut self) {
        let state = self.0.state::<AppState>();
        state.invalidate_runtime_probe();
        state.installer_finished();
        state.set_bootstrap_pgid(None);
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
    let Some(gate) = app.state::<AppState>().begin_bootstrap() else {
        return Ok(app.state::<AppState>().snapshot());
    };
    // The task owns the entire operation, including outcome and preparation.
    // Reloading the renderer may drop its IPC observer, never this task.
    owned_bootstrap_task(gate, async move {
        finish_bootstrap(&app).await;
        app.state::<AppState>().snapshot()
    })
    .await
    .map_err(|error| format!("join bootstrap completion: {error}"))
}

fn owned_bootstrap_task<T: Send + 'static>(
    gate: crate::state::BootstrapGuard,
    completion: impl std::future::Future<Output = T> + Send + 'static,
) -> tauri::async_runtime::JoinHandle<T> {
    tauri::async_runtime::spawn(async move {
        let _gate = gate;
        completion.await
    })
}

async fn finish_bootstrap(app: &AppHandle) {
    let outcome = run_bootstrap(app).await;
    match outcome {
        Ok(Outcome::Committed) => {
            if app
                .state::<AppState>()
                .quitting
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return;
            }
            if let Err(error) = service::prepare_and_open(app).await {
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
    let permit = state.begin_update()?;
    anyhow::ensure!(
        permit.server()?.is_none(),
        "DSH is already running; first installation refused"
    );
    state.log(crate::i18n::text("startup.installing"));
    state.log(crate::i18n::text("startup.one_time"));
    let operation = state.begin_native_installer()?;
    let control = operation.control.clone();
    let worker_app = app.clone();
    let worker_control = control.clone();
    // The worker owns the permit and all payload processes. Shutdown cancels
    // its Control; dropping an IPC future cannot detach unowned child processes.
    let result = tauri::async_runtime::spawn_blocking(move || {
        // Drop in reverse order: release runtime permit, finish installer/cache
        // cleanup, then mark the exit-deferral operation finished LAST.
        let _operation = operation;
        let _lifecycle = BootstrapLifecycle(worker_app);
        let _permit = permit;
        crate::runtime_install::install(worker_control)
    })
    .await
    .context("join signed runtime installation")?;
    state.invalidate_runtime_probe();
    match result {
        Ok(()) => {
            state.log(crate::i18n::text("startup.installed"));
            Ok(Outcome::Committed)
        }
        Err(_) if control.snapshot().cancel_requested => {
            state.log(crate::i18n::text("startup.cancelled"));
            Ok(Outcome::Cancelled)
        }
        Err(error) => Err(error),
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

#[cfg(test)]
mod bootstrap_task_tests {
    use super::*;

    #[tokio::test]
    async fn dropped_renderer_observer_keeps_ownership_and_completion() {
        let state = std::sync::Arc::new(AppState::default());
        let gate = state.begin_bootstrap().unwrap();
        let (resume, pending) = tokio::sync::oneshot::channel();
        let (finished, completion) = tokio::sync::oneshot::channel();
        let worker_state = state.clone();
        let observer = owned_bootstrap_task(gate, async move {
            pending.await.unwrap();
            worker_state.fail("native outcome survived renderer reload");
            finished.send(()).unwrap();
        });
        drop(observer);
        assert!(state.begin_bootstrap().is_none());
        resume.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), completion)
            .await
            .unwrap()
            .unwrap();
        // The callback sends before its gate is dropped; wait for the actual end.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while state.bootstrap_running() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            state.snapshot().message.as_deref(),
            Some("native outcome survived renderer reload")
        );
        assert!(state.begin_bootstrap().is_some());
    }
}

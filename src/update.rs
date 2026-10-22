use semver::Version;
use tauri::{AppHandle, Manager};

use crate::bootstrap::{self, Runtime};
use crate::managed_runtime::{self, ApplyOutcome, Check, RecoverOutcome};
use crate::runtime_feed::Candidate;
use crate::state::AppState;
use crate::{auth, ui};

/// Native signed installed-runtime update entry point. This is the reachable
/// production path (tray → Updates): it checks the independent configured
/// runtime Source (settings/key/channel) and never queries `npm view`. A pending
/// transaction routes to explicit recovery instead.
pub fn check_and_show(app: &AppHandle) {
    check_and_show_mode(app, false);
}

/// Setup recovery never falls through to release checks when its state is stale.
/// Both entry points reuse the same fresh inspection and explicit confirmation.
pub fn recover_and_show(app: &AppHandle) {
    check_and_show_mode(app, true);
}

fn check_and_show_mode(app: &AppHandle, recovery_only: bool) {
    let app = app.clone();
    // Detection, feed checks and downloads must not block GTK's event loop.
    tauri::async_runtime::spawn_blocking(move || {
        match managed_runtime::pending_status() {
            Ok(Some(info)) => {
                let app_for_main = app.clone();
                let _ = app.run_on_main_thread(move || present_recovery(&app_for_main, &info));
                return;
            }
            Ok(None) if recovery_only => {
                show_message(
                    &app,
                    "Runtime recovery",
                    "No runtime recovery is pending. Retry the normal startup if needed.",
                    gtk::MessageType::Info,
                );
                return;
            }
            Ok(None) => {}
            Err(error) => {
                show_message(
                    &app,
                    "Runtime recovery inspection failed",
                    &format!("{error:#}"),
                    gtk::MessageType::Error,
                );
                return;
            }
        }
        let Some(runtime) = bootstrap::detect() else {
            show_message(
                &app,
                "DSH update check",
                "DeepSeek Harness was not found; install it in the setup window first.",
                gtk::MessageType::Error,
            );
            return;
        };
        let current = match Version::parse(runtime.version.trim_start_matches('v')) {
            Ok(version) => version,
            Err(error) => {
                show_message(
                    &app,
                    "DSH update check",
                    &format!("invalid installed DSH version: {error}"),
                    gtk::MessageType::Error,
                );
                return;
            }
        };
        // Register the metadata check with the native update registry so quit
        // cancels the metadata curl and a concurrent apply/check is refused.
        let operation = match app.state::<AppState>().updates.begin() {
            Ok(operation) => operation,
            Err(error) => {
                show_message(
                    &app,
                    "Update busy",
                    &error.to_string(),
                    gtk::MessageType::Warning,
                );
                return;
            }
        };
        let control = operation.control.clone();
        match managed_runtime::check(&current, control.clone()) {
            Ok(check) => {
                let cancelled = !control.complete();
                drop(operation);
                if cancelled {
                    return;
                }
                let app_for_main = app.clone();
                let _ =
                    app.run_on_main_thread(move || present_update(&app_for_main, runtime, check));
            }
            Err(error) => {
                let _ = control.complete();
                drop(operation);
                show_message(
                    &app,
                    "DSH update check failed",
                    &auth::redact(&format!("{error:#}")),
                    gtk::MessageType::Error,
                );
            }
        }
    });
}

fn describe_check(runtime: &Runtime, check: &Check) -> String {
    let summary = &check.summary;
    let release = summary
        .get("release")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let sequence = summary
        .get("sequence")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let fingerprint = summary
        .get("keyFingerprint")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    format!(
        "Installed:  {}\nChannel:    {}\nUpdate:     {} (sequence {})\nPublic key: {}\nSource:     {}\n\nThis downloads the signed runtime bundle, verifies it, stops the local server, takes a full private (unencrypted) backup of your DSH home — credentials, configuration, plugins and sessions — and activates the new runtime. The server and its WebSocket clients will restart. Close all other DSH CLI/TUI writers first.",
        runtime.version,
        check.source.channel,
        release,
        sequence,
        fingerprint,
        check.source.manifest_url,
    )
}

fn present_update(app: &AppHandle, runtime: Runtime, check: Check) {
    // Offer policy: only an owned (systemd or managed-child), generation-managed,
    // loopback install may be offered an apply. External/attached/remote installs
    // get check-only guidance before any download, never a doomed apply.
    let server = app.state::<AppState>().server();
    if let Some(block) = managed_runtime::apply_block(&runtime, server.as_ref()) {
        let mut body = describe_check(&runtime, &check);
        body.push_str("\n\n");
        body.push_str(block.guidance());
        ui::message(
            app,
            "DSH update available",
            &body,
            gtk::MessageType::Warning,
        );
        return;
    }
    match check.status {
        managed_runtime::Status::Current => {
            let body = format!(
                "Installed:  {}\nChannel:    {}\n\nYou are on the latest DSH for this channel.",
                runtime.version, check.source.channel
            );
            ui::message(app, "DSH updates", &body, gtk::MessageType::Info);
        }
        managed_runtime::Status::Available => {
            let release = check
                .summary
                .get("release")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            let mut body = describe_check(&runtime, &check);
            body.push_str(&format!("\n\nUpdate to {release} now?"));
            let app_for_update = app.clone();
            let candidate = check.candidate;
            ui::confirm(app, "DSH update available", &body, move || {
                perform_apply(app_for_update, runtime, candidate);
            });
        }
    }
}

fn perform_apply(app: AppHandle, runtime: Runtime, candidate: Candidate) {
    let state = app.state::<AppState>();
    let permit = match state.begin_update() {
        Ok(permit) => permit,
        Err(error) => {
            show_message(
                &app,
                "Runtime busy",
                &error.to_string(),
                gtk::MessageType::Warning,
            );
            return;
        }
    };
    let operation = match state.updates.begin() {
        Ok(operation) => operation,
        Err(error) => {
            show_message(
                &app,
                "Update busy",
                &error.to_string(),
                gtk::MessageType::Warning,
            );
            return;
        }
    };
    let control = operation.control.clone();
    let worker_app = app.clone();
    std::thread::spawn(move || {
        let result = managed_runtime::apply_native(
            Some(worker_app.clone()),
            permit,
            runtime,
            candidate,
            control.clone(),
        );
        let cancelled = !control.complete();
        drop(operation); // release the global gate before exposing completion
        let for_main = worker_app.clone();
        let _ = worker_app
            .run_on_main_thread(move || present_apply_outcome(&for_main, cancelled, result));
    });
}

fn present_apply_outcome(app: &AppHandle, cancelled: bool, result: anyhow::Result<ApplyOutcome>) {
    if cancelled {
        show_message(
            app,
            "Update cancelled",
            "No runtime was activated.",
            gtk::MessageType::Info,
        );
        return;
    }
    match result {
        Ok(ApplyOutcome::Applied { release }) => show_message(
            app,
            "DSH updated",
            &format!("DeepSeek Harness {release} is now active."),
            gtk::MessageType::Info,
        ),
        Ok(ApplyOutcome::RolledBack { error }) => show_message(
            app,
            "DSH update not committed",
            &format!(
                "The previous runtime was restored.\n\n{}",
                auth::redact(&error)
            ),
            gtk::MessageType::Error,
        ),
        Ok(ApplyOutcome::PendingRecovery { error }) => present_pending(app, &auth::redact(&error)),
        Err(error) => show_message(
            app,
            "DSH update failed",
            &auth::redact(&format!("{error:#}")),
            gtk::MessageType::Error,
        ),
    }
}

fn present_recovery(app: &AppHandle, info: &crate::runtime_switch::PendingInfo) {
    let snapshot = if info.has_snapshot {
        "A full private home backup snapshot is available for rollback."
    } else {
        "No snapshot was captured before activation."
    };
    let body = format!(
        "A runtime switch is pending and ordinary start/restart stays blocked until recovery succeeds.\n\nOld generation: {}\nNew generation: {}\n{snapshot}\n\nRecover the previous runtime now?",
        info.old.display(),
        info.new.display()
    );
    let app_for_recover = app.clone();
    ui::confirm(app, "Runtime recovery required", &body, move || {
        perform_recover(app_for_recover);
    });
}

fn present_pending(app: &AppHandle, error: &str) {
    let body = format!(
        "The runtime switch did not complete and the previous runtime could not be restored automatically. Ordinary start/restart stays blocked until recovery succeeds.\n\n{error}\n\nRecover the previous runtime now?"
    );
    let app_for_recover = app.clone();
    ui::confirm(app, "Runtime recovery required", &body, move || {
        perform_recover(app_for_recover);
    });
}

fn perform_recover(app: AppHandle) {
    let state = app.state::<AppState>();
    let permit = match state.begin_update() {
        Ok(permit) => permit,
        Err(error) => {
            show_message(
                &app,
                "Runtime busy",
                &error.to_string(),
                gtk::MessageType::Warning,
            );
            return;
        }
    };
    let operation = match state.updates.begin() {
        Ok(operation) => operation,
        Err(error) => {
            show_message(
                &app,
                "Recovery busy",
                &error.to_string(),
                gtk::MessageType::Warning,
            );
            return;
        }
    };
    let control = operation.control.clone();
    let worker_app = app.clone();
    std::thread::spawn(move || {
        let result =
            managed_runtime::recover_native(Some(worker_app.clone()), permit, control.clone());
        let cancelled = !control.complete();
        drop(operation);
        let for_main = worker_app.clone();
        let _ = worker_app
            .run_on_main_thread(move || present_recover_outcome(&for_main, cancelled, result));
    });
}

fn present_recover_outcome(
    app: &AppHandle,
    cancelled: bool,
    result: anyhow::Result<RecoverOutcome>,
) {
    let (title, body, kind) = recover_outcome_message(cancelled, &result);
    show_message(app, title, &body, kind);
}

/// Message metadata for a recovery outcome, separated from the GTK dialog so the
/// outcome-truth mapping is unit-testable without a window. The rule: only an
/// `Err` (journal still pending) may say "pending retained / start blocked"; a
/// completed recovery with a failed window refresh is a warning that still
/// reports recovery, never a claim that the transaction is still pending.
fn recover_outcome_message(
    cancelled: bool,
    result: &anyhow::Result<RecoverOutcome>,
) -> (&'static str, String, gtk::MessageType) {
    if cancelled {
        return (
            "Recovery cancelled",
            "The pending transaction was retained.".to_owned(),
            gtk::MessageType::Warning,
        );
    }
    match result {
        Ok(RecoverOutcome::Recovered {
            refresh_error: None,
        }) => (
            "Runtime recovered",
            "The previous runtime was restored and is serving again.".to_owned(),
            gtk::MessageType::Info,
        ),
        Ok(RecoverOutcome::Recovered {
            refresh_error: Some(error),
        }) => (
            "Runtime recovered",
            format!(
                "The previous runtime was restored and is serving again.\n\n\
                 The window refresh to the restored runtime failed: {}\n\
                 Recovery is complete and the pending transaction was cleared; \
                 start or restart DSH normally.",
                auth::redact(error)
            ),
            gtk::MessageType::Warning,
        ),
        Err(error) => (
            "Recovery incomplete",
            format!(
                "The pending transaction was retained; ordinary start/restart stays blocked.\n\n{}",
                auth::redact(&format!("{error:#}"))
            ),
            gtk::MessageType::Error,
        ),
    }
}

fn show_message(app: &AppHandle, title: &'static str, body: &str, kind: gtk::MessageType) {
    let app_for_main = app.clone();
    let body = auth::redact(body);
    let _ = app.run_on_main_thread(move || ui::message(&app_for_main, title, &body, kind));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recover_outcome_message_never_claims_pending_after_recovery() {
        // A completed recovery with a failed window refresh is a warning that
        // still reports recovery — never "pending retained / start blocked".
        let (title, body, kind) = recover_outcome_message(
            false,
            &Ok(RecoverOutcome::Recovered {
                refresh_error: Some("window refresh failed".to_owned()),
            }),
        );
        assert_eq!(title, "Runtime recovered");
        assert!(body.contains("restored"), "{body}");
        assert!(!body.contains("retained"), "{body}");
        assert!(!body.contains("blocked"), "{body}");
        assert!(matches!(kind, gtk::MessageType::Warning));

        // A clean recovery stays informational.
        let (title, body, kind) = recover_outcome_message(
            false,
            &Ok(RecoverOutcome::Recovered {
                refresh_error: None,
            }),
        );
        assert_eq!(title, "Runtime recovered");
        assert!(body.contains("restored"), "{body}");
        assert!(matches!(kind, gtk::MessageType::Info));

        // An incomplete recovery (journal still pending) keeps the blocked
        // guidance and is an error, not a warning.
        let (title, body, kind) =
            recover_outcome_message(false, &Err(anyhow::anyhow!("old root failed to restart")));
        assert_eq!(title, "Recovery incomplete");
        assert!(body.contains("blocked"), "{body}");
        assert!(body.contains("pending"), "{body}");
        assert!(matches!(kind, gtk::MessageType::Error));
    }

    #[test]
    fn recover_outcome_message_redacts_tokens() {
        let (_, body, _) = recover_outcome_message(
            false,
            &Err(anyhow::anyhow!(
                "dsh web: http://127.0.0.1:3080/?token=secret"
            )),
        );
        assert!(!body.contains("token=secret"), "{body}");
        assert!(body.contains("token=***"), "{body}");
    }
}

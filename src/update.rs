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
                    crate::i18n::text("recovery.title"),
                    crate::i18n::text("recovery.no_pending"),
                    gtk::MessageType::Info,
                );
                return;
            }
            Ok(None) => {}
            Err(error) => {
                show_message(
                    &app,
                    crate::i18n::text("recovery.inspection_failed_title"),
                    &format!("{error:#}"),
                    gtk::MessageType::Error,
                );
                return;
            }
        }
        let Some(runtime) = bootstrap::detect() else {
            show_message(
                &app,
                crate::i18n::text("runtime_ui.check_title"),
                crate::i18n::text("runtime_ui.not_found"),
                gtk::MessageType::Error,
            );
            return;
        };
        let current = match Version::parse(runtime.version.trim_start_matches('v')) {
            Ok(version) => version,
            Err(error) => {
                show_message(
                    &app,
                    crate::i18n::text("runtime_ui.check_title"),
                    &crate::i18n::render(
                        crate::i18n::active_language(),
                        "runtime_ui.invalid_version",
                        &[("error", &error.to_string())],
                    )
                    .expect("validated invalid runtime version diagnostic"),
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
                    crate::i18n::text("runtime_ui.busy_title"),
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
                    crate::i18n::text("runtime_ui.check_failed_title"),
                    &auth::redact(&format!("{error:#}")),
                    gtk::MessageType::Error,
                );
            }
        }
    });
}

fn describe_check(runtime: &Runtime, check: &Check) -> String {
    describe_check_in(
        &runtime.version,
        &check.summary,
        &check.source,
        crate::i18n::active_language(),
    )
}
fn describe_check_in(
    installed: &str,
    summary: &serde_json::Value,
    source: &managed_runtime::SourceSummary,
    language: crate::i18n::Language,
) -> String {
    let unknown = crate::i18n::message(language, "runtime_ui.unknown")
        .expect("embedded unknown runtime metadata label");
    let release = summary
        .get("release")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(unknown);
    let sequence = summary
        .get("sequence")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let fingerprint = summary
        .get("keyFingerprint")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(unknown);
    crate::i18n::render(
        language,
        "runtime_ui.description",
        &[
            ("installed", installed),
            ("channel", &source.channel),
            ("release", release),
            ("sequence", &sequence.to_string()),
            ("fingerprint", fingerprint),
            ("source", &source.manifest_url),
        ],
    )
    .expect("validated signed runtime offer metadata")
}
fn apply_block_guidance_in(
    block: managed_runtime::ApplyBlock,
    language: crate::i18n::Language,
) -> &'static str {
    let key = match block {
        managed_runtime::ApplyBlock::External => "external",
        managed_runtime::ApplyBlock::Attached => "attached",
        managed_runtime::ApplyBlock::Remote => "remote",
        managed_runtime::ApplyBlock::NoServer => "no_server",
    };
    crate::i18n::message(language, &format!("runtime_ui.block.{key}"))
        .expect("embedded check-only ownership guidance")
}

fn present_update(app: &AppHandle, runtime: Runtime, check: Check) {
    // Offer policy: only an owned (systemd or managed-child), generation-managed,
    // loopback install may be offered an apply. External/attached/remote installs
    // get check-only guidance before any download, never a doomed apply.
    let server = app.state::<AppState>().server();
    if let Some(block) = managed_runtime::apply_block(&runtime, server.as_ref()) {
        let mut body = describe_check(&runtime, &check);
        body.push_str("\n\n");
        body.push_str(apply_block_guidance_in(
            block,
            crate::i18n::active_language(),
        ));
        ui::message(
            app,
            crate::i18n::text("runtime_ui.available_title"),
            &body,
            gtk::MessageType::Warning,
        );
        return;
    }
    match check.status {
        managed_runtime::Status::Current => {
            let body = crate::i18n::render(
                crate::i18n::active_language(),
                "runtime_ui.current",
                &[
                    ("installed", &runtime.version),
                    ("channel", &check.source.channel),
                ],
            )
            .expect("validated current runtime metadata");
            ui::message(
                app,
                crate::i18n::text("runtime_ui.updates_title"),
                &body,
                gtk::MessageType::Info,
            );
        }
        managed_runtime::Status::Available => {
            let release = check
                .summary
                .get("release")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(crate::i18n::text("runtime_ui.unknown"))
                .to_owned();
            let mut body = describe_check(&runtime, &check);
            body.push_str(
                &crate::i18n::render(
                    crate::i18n::active_language(),
                    "runtime_ui.question",
                    &[("release", &release)],
                )
                .expect("validated explicit runtime apply question"),
            );
            let app_for_update = app.clone();
            let candidate = check.candidate;
            ui::confirm(
                app,
                crate::i18n::text("runtime_ui.available_title"),
                &body,
                move || {
                    perform_apply(app_for_update, runtime, candidate);
                },
            );
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
                crate::i18n::text("recovery.runtime_busy_title"),
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
                crate::i18n::text("runtime_ui.busy_title"),
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
            crate::i18n::text("runtime_ui.cancelled_title"),
            crate::i18n::text("runtime_ui.cancelled"),
            gtk::MessageType::Info,
        );
        return;
    }
    match result {
        Ok(ApplyOutcome::Applied { release }) => show_message(
            app,
            crate::i18n::text("runtime_ui.applied_title"),
            &crate::i18n::render(
                crate::i18n::active_language(),
                "runtime_ui.applied",
                &[("release", &release)],
            )
            .expect("validated applied runtime release"),
            gtk::MessageType::Info,
        ),
        Ok(ApplyOutcome::RolledBack { error }) => show_message(
            app,
            crate::i18n::text("runtime_ui.rollback_title"),
            &crate::i18n::render(
                crate::i18n::active_language(),
                "runtime_ui.rollback",
                &[("error", &auth::redact(&error))],
            )
            .expect("validated rollback diagnostic"),
            gtk::MessageType::Error,
        ),
        Ok(ApplyOutcome::PendingRecovery { error }) => present_pending(app, &auth::redact(&error)),
        Err(error) => show_message(
            app,
            crate::i18n::text("runtime_ui.failed_title"),
            &auth::redact(&format!("{error:#}")),
            gtk::MessageType::Error,
        ),
    }
}

fn present_recovery(app: &AppHandle, info: &crate::runtime_switch::PendingInfo) {
    let snapshot = crate::i18n::text(if info.has_snapshot {
        "recovery.snapshot_available"
    } else {
        "recovery.snapshot_missing"
    });
    let body = crate::i18n::render(
        crate::i18n::active_language(),
        "recovery.confirmation",
        &[
            ("old", &info.old.display().to_string()),
            ("new", &info.new.display().to_string()),
            ("snapshot", snapshot),
        ],
    )
    .expect("validated explicit recovery confirmation");
    let app_for_recover = app.clone();
    ui::confirm(
        app,
        crate::i18n::text("recovery.required_title"),
        &body,
        move || {
            perform_recover(app_for_recover);
        },
    );
}

fn present_pending(app: &AppHandle, error: &str) {
    let body = crate::i18n::render(
        crate::i18n::active_language(),
        "recovery.pending_confirmation",
        &[("error", error)],
    )
    .expect("validated blocked recovery confirmation");
    let app_for_recover = app.clone();
    ui::confirm(
        app,
        crate::i18n::text("recovery.required_title"),
        &body,
        move || {
            perform_recover(app_for_recover);
        },
    );
}

fn perform_recover(app: AppHandle) {
    let state = app.state::<AppState>();
    let permit = match state.begin_update() {
        Ok(permit) => permit,
        Err(error) => {
            show_message(
                &app,
                crate::i18n::text("recovery.runtime_busy_title"),
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
                crate::i18n::text("recovery.busy_title"),
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
    recover_outcome_message_in(cancelled, result, crate::i18n::active_language())
}
fn recover_outcome_message_in(
    cancelled: bool,
    result: &anyhow::Result<RecoverOutcome>,
    language: crate::i18n::Language,
) -> (&'static str, String, gtk::MessageType) {
    let message = |key| crate::i18n::message(language, key).expect("embedded recovery message");
    if cancelled {
        return (
            message("recovery.cancelled_title"),
            message("recovery.cancelled").into(),
            gtk::MessageType::Warning,
        );
    }
    match result {
        Ok(RecoverOutcome::Recovered {
            refresh_error: None,
        }) => (
            message("recovery.recovered_title"),
            message("recovery.recovered").into(),
            gtk::MessageType::Info,
        ),
        Ok(RecoverOutcome::Recovered {
            refresh_error: Some(error),
        }) => (
            message("recovery.recovered_title"),
            crate::i18n::render(
                language,
                "recovery.refresh_warning",
                &[("error", &auth::redact(error))],
            )
            .expect("validated recovered window warning"),
            gtk::MessageType::Warning,
        ),
        Err(error) => (
            message("recovery.incomplete_title"),
            crate::i18n::render(
                language,
                "recovery.incomplete",
                &[("error", &auth::redact(&format!("{error:#}")))],
            )
            .expect("validated pending recovery diagnostic"),
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
    fn runtime_offer_is_localized_without_reparsing_summary_or_changing_apply_blocks() {
        let source = managed_runtime::SourceSummary {
            channel: "next".into(),
            manifest_url: "https://example.invalid/{release}".into(),
            signature_url: "https://example.invalid/signature".into(),
        };
        let summary = serde_json::json!({"release":"<b>{installed}</b>","sequence":31,"keyFingerprint":"synthetic-fingerprint"});
        for (language, _) in crate::i18n::LANGUAGES {
            let body = describe_check_in("0.2.0", &summary, &source, language);
            assert!(
                body.contains("<b>{installed}</b>")
                    && body.contains("https://example.invalid/{release}")
                    && body.contains("31")
                    && body.contains("synthetic-fingerprint")
            );
            for (block, key) in [
                (managed_runtime::ApplyBlock::External, "external"),
                (managed_runtime::ApplyBlock::Attached, "attached"),
                (managed_runtime::ApplyBlock::Remote, "remote"),
                (managed_runtime::ApplyBlock::NoServer, "no_server"),
            ] {
                assert_eq!(
                    apply_block_guidance_in(block, language),
                    crate::i18n::message(language, &format!("runtime_ui.block.{key}")).unwrap()
                );
            }
        }
    }
    #[test]
    fn recovery_truth_and_redaction_are_preserved_in_every_language() {
        for (language, _) in crate::i18n::LANGUAGES {
            let result = Ok(RecoverOutcome::Recovered {
                refresh_error: Some("http://127.0.0.1/?token=synthetic-private".into()),
            });
            let (title, body, kind) = recover_outcome_message_in(false, &result, language);
            assert_eq!(
                title,
                crate::i18n::message(language, "recovery.recovered_title").unwrap()
            );
            assert!(
                matches!(kind, gtk::MessageType::Warning) && !body.contains("synthetic-private")
            );
            let (_, body, kind) =
                recover_outcome_message_in(false, &Err(anyhow::anyhow!("failure")), language);
            assert!(matches!(kind, gtk::MessageType::Error));
            assert!(body.contains(
                &crate::i18n::render(language, "recovery.incomplete", &[("error", "failure")])
                    .unwrap()
            ));
            let (_, body, kind) = recover_outcome_message_in(true, &result, language);
            assert!(matches!(kind, gtk::MessageType::Warning));
            assert_eq!(
                body,
                crate::i18n::message(language, "recovery.cancelled").unwrap()
            );
        }
    }
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

//! Tray flow for creating a whole-data snapshot (sessions + shared stores).
//!
//! Snapshotting is read-only with respect to the source tree but can copy
//! gigabytes, so it asks first, shows what will be copied (counts/bytes only),
//! refuses a second concurrent run, and reports the result without exposing file
//! names or contents. Restoring stays an explicit offline CLI operation.
use std::os::unix::fs::DirBuilderExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gtk::glib;
use gtk::prelude::*;
use tauri::{AppHandle, Manager};

use crate::session_inventory::{self, Summary};

pub const TITLE: &str = "DSH Ocean data snapshot";

/// Confirmation body: counts and bytes only, never a file name.
pub fn confirmation_text(summary: &Summary) -> String {
    confirmation_text_in(summary, crate::i18n::active_language())
}
pub fn confirmation_text_in(summary: &Summary, language: crate::i18n::Language) -> String {
    let total = summary.session_bytes + summary.storage_bytes;
    crate::i18n::render(
        language,
        "data_snapshot.confirm",
        &[
            ("session_files", &summary.session_files.to_string()),
            ("session_bytes", &summary.session_bytes.to_string()),
            ("storage_files", &summary.storage_files.to_string()),
            ("storage_bytes", &summary.storage_bytes.to_string()),
            ("total", &total.to_string()),
        ],
    )
    .expect("validated snapshot confirmation counters")
}

pub fn result_text(files: usize, bytes: u64) -> String {
    result_text_in(files, bytes, crate::i18n::active_language())
}
pub fn result_text_in(files: usize, bytes: u64, language: crate::i18n::Language) -> String {
    crate::i18n::render(
        language,
        "data_snapshot.result",
        &[("files", &files.to_string()), ("bytes", &bytes.to_string())],
    )
    .expect("validated snapshot result counters")
}

/// Shown in addition to [`result_text`] when the source changed during the copy.
pub const INCONSISTENT_TEXT: &str =
    "Note: sessions or stores changed while the copy ran, so this is a best-effort point-in-time copy. Stop DSH writers for a frozen snapshot.";

/// Terminal outcome computed by the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Created {
        files: usize,
        bytes: u64,
        consistent: bool,
    },
    Cancelled,
    Failed,
}

/// Mutable progress shared between the worker and the GTK timer.
///
/// The worker never touches a GTK object: it only records counters, the outcome
/// and whether cancellation was requested. The timer owns all widget updates.
#[derive(Default)]
pub struct ProgressState {
    pub copied_files: usize,
    pub copied_bytes: u64,
    pub total_bytes: u64,
    pub cancel_requested: bool,
    pub outcome: Option<Outcome>,
}

impl ProgressState {
    fn finish(&mut self, outcome: Option<Outcome>) {
        // The request is cooperative; the worker result determines whether
        // cancellation actually happened before publication.
        self.outcome = outcome;
    }
}

pub const FAILURE_TEXT: &str =
    "The snapshot could not be created. Existing snapshots and live data were not changed.";
pub const CANCELLED_TEXT: &str = "The snapshot was cancelled. Live data was not changed; an incomplete staging copy was removed.";

struct Gate(Arc<AtomicBool>);
impl Drop for Gate {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn begin(flag: &Arc<AtomicBool>) -> Option<Gate> {
    flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| Gate(flag.clone()))
}

pub struct Window {
    pub dialog: gtk::Dialog,
    pub label: gtk::Label,
    pub accept: gtk::Button,
}

pub struct ProgressWindow {
    pub dialog: gtk::Dialog,
    pub label: gtk::Label,
    pub cancel: gtk::Button,
    pub state: Arc<Mutex<ProgressState>>,
}

/// Progress dialog with an explicit cancel action; counts only.
pub fn build_progress(parent: Option<&gtk::Window>) -> ProgressWindow {
    let dialog = gtk::Dialog::with_buttons(
        Some(crate::i18n::text("data_snapshot.title")),
        parent,
        gtk::DialogFlags::MODAL,
        &[(
            crate::i18n::text("data_snapshot.cancel"),
            gtk::ResponseType::Cancel,
        )],
    );
    dialog.set_default_size(520, 180);
    let label = gtk::Label::new(Some(&data_backup_progress_text(&ProgressState::default())));
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_line_wrap(true);
    label.set_xalign(0.0);
    label.set_margin_start(18);
    label.set_margin_end(18);
    label.set_margin_top(18);
    dialog.content_area().pack_start(&label, false, false, 0);
    let cancel = dialog
        .widget_for_response(gtk::ResponseType::Cancel)
        .expect("cancel button")
        .downcast::<gtk::Button>()
        .expect("cancel button is a gtk button");
    let state = Arc::new(Mutex::new(ProgressState::default()));
    let cancel_state = state.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Cancel {
            let mut state = cancel_state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.outcome.is_none() {
                state.cancel_requested = true;
            }
            if let Some(button) = dialog.widget_for_response(gtk::ResponseType::Cancel) {
                button.set_sensitive(false);
            }
        }
    });
    let cancel_state = state.clone();
    dialog.connect_delete_event(move |_, _| {
        let mut state = cancel_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.outcome.is_none() {
            // Keep the running copy visible until cooperative cancellation settles.
            state.cancel_requested = true;
            gtk::glib::Propagation::Stop
        } else {
            // GTK close emits delete-event too; do not veto the terminal timer.
            gtk::glib::Propagation::Proceed
        }
    });
    ProgressWindow {
        dialog,
        label,
        cancel,
        state,
    }
}

fn data_backup_progress_text(state: &ProgressState) -> String {
    data_backup_progress_text_in(state, crate::i18n::active_language())
}
fn data_backup_progress_text_in(state: &ProgressState, language: crate::i18n::Language) -> String {
    let mut text = if state.total_bytes == 0 {
        crate::i18n::render(
            language,
            "data_snapshot.progress_unknown",
            &[("files", &state.copied_files.to_string())],
        )
    } else {
        let percent = (state.copied_bytes.min(state.total_bytes) * 100)
            .checked_div(state.total_bytes)
            .expect("nonzero snapshot total");
        crate::i18n::render(
            language,
            "data_snapshot.progress_known",
            &[
                ("copied", &state.copied_bytes.to_string()),
                ("total", &state.total_bytes.to_string()),
                ("percent", &percent.to_string()),
                ("files", &state.copied_files.to_string()),
            ],
        )
    }
    .expect("validated snapshot progress counters");
    if state.cancel_requested {
        text.push_str(
            crate::i18n::message(language, "data_snapshot.cancelling")
                .expect("embedded snapshot cancellation notice"),
        );
    }
    text
}

/// Confirmation dialog with an explicit Accept button (no default action).
pub fn build_confirm(parent: Option<&gtk::Window>, body: &str) -> Window {
    let dialog = gtk::Dialog::with_buttons(
        Some(crate::i18n::text("data_snapshot.title")),
        parent,
        gtk::DialogFlags::MODAL,
        &[
            (
                crate::i18n::text("common.cancel"),
                gtk::ResponseType::Cancel,
            ),
            (
                crate::i18n::text("data_snapshot.create"),
                gtk::ResponseType::Accept,
            ),
        ],
    );
    dialog.set_default_size(620, 380);
    let label = gtk::Label::new(Some(body));
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_line_wrap(true);
    label.set_max_width_chars(78);
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    label.set_margin_start(18);
    label.set_margin_end(18);
    label.set_margin_top(18);
    label.set_margin_bottom(18);
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.add(&label);
    dialog.content_area().pack_start(&scroll, true, true, 0);
    let accept = dialog
        .widget_for_response(gtk::ResponseType::Accept)
        .expect("accept button")
        .downcast::<gtk::Button>()
        .expect("accept button is a gtk button");
    Window {
        dialog,
        label,
        accept,
    }
}

pub fn show(app: &AppHandle) {
    let parent = app
        .get_webview_window("main")
        .and_then(|window| window.gtk_window().ok());
    let summary = session_inventory::inventory(&crate::config::dsh_home());
    let window = build_confirm(
        parent.as_ref().map(|parent| parent.upcast_ref()),
        &confirmation_text(&summary),
    );
    let handle = app.clone();
    let accept = window.accept.clone();
    window.dialog.connect_response(move |dialog, response| {
        if response != gtk::ResponseType::Accept {
            dialog.close();
            return;
        }
        accept.set_sensitive(false);
        let Some(gate) = begin(&handle.state::<crate::state::AppState>().data_snapshot) else {
            crate::ui::message(
                &handle,
                crate::i18n::text("data_snapshot.busy_title"),
                crate::i18n::text("data_snapshot.busy"),
                gtk::MessageType::Info,
            );
            dialog.close();
            return;
        };
        dialog.close();
        // The worker only touches this shared state; the timer below owns every
        // widget, so no GTK object ever crosses a thread boundary.
        let progress_window = build_progress(parent.as_ref().map(|parent| parent.upcast_ref()));
        let shared = progress_window.state.clone();
        progress_window.dialog.show_all();
        let weak_dialog = progress_window.dialog.downgrade();
        let weak_label = progress_window.label.downgrade();
        let timer_state = shared.clone();
        let timer_app = handle.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            let (Some(dialog), Some(label)) = (weak_dialog.upgrade(), weak_label.upgrade()) else {
                return glib::ControlFlow::Break;
            };
            let outcome = {
                let state = timer_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                label.set_text(&data_backup_progress_text(&state));
                state.outcome
            };
            let Some(outcome) = outcome else {
                return glib::ControlFlow::Continue;
            };
            let timer_app = timer_app.clone();
            crate::ui::close_progress_then(&dialog, move || match outcome {
                Outcome::Created {
                    files,
                    bytes,
                    consistent,
                } => {
                    let mut body = result_text(files, bytes);
                    if !consistent {
                        body.push_str("\n\n");
                        body.push_str(crate::i18n::text("data_snapshot.inconsistent"));
                    }
                    crate::ui::message(
                        &timer_app,
                        crate::i18n::text("data_snapshot.created_title"),
                        &body,
                        if consistent {
                            gtk::MessageType::Info
                        } else {
                            gtk::MessageType::Warning
                        },
                    )
                }
                Outcome::Cancelled => crate::ui::message(
                    &timer_app,
                    crate::i18n::text("data_snapshot.cancelled_title"),
                    crate::i18n::text("data_snapshot.cancelled"),
                    gtk::MessageType::Info,
                ),
                Outcome::Failed => crate::ui::message(
                    &timer_app,
                    crate::i18n::text("data_snapshot.failed_title"),
                    crate::i18n::text("data_snapshot.failure"),
                    gtk::MessageType::Warning,
                ),
            });
            glib::ControlFlow::Break
        });

        let worker_state = shared.clone();
        // Copying can take a long time; never do it on the GTK thread.
        tauri::async_runtime::spawn_blocking(move || {
            let _gate = gate;
            let backups = crate::config::profile_backup_dir();
            let result = std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&backups)
                .map_err(anyhow::Error::from)
                .and_then(|()| {
                    crate::data_backup::snapshot_with_progress(
                        &crate::config::dsh_home(),
                        &backups,
                        crate::bootstrap::detect().map(|runtime| runtime.version),
                        &mut |update| {
                            let mut state = worker_state
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            state.copied_files = update.copied_files;
                            state.copied_bytes = update.copied_bytes;
                            state.total_bytes = update.total_bytes;
                            anyhow::ensure!(!state.cancel_requested, "snapshot cancelled");
                            Ok(())
                        },
                    )
                })
                .and_then(|path| {
                    crate::data_backup::manifest(&path).map(|manifest| {
                        (
                            manifest.files.len(),
                            manifest.total_bytes(),
                            manifest.consistent,
                        )
                    })
                });
            let outcome = match result {
                Ok((files, bytes, consistent)) => Some(Outcome::Created {
                    files,
                    bytes,
                    consistent,
                }),
                Err(error) => {
                    let cancelled = format!("{error:#}").contains("cancelled");
                    Some(if cancelled {
                        Outcome::Cancelled
                    } else {
                        Outcome::Failed
                    })
                }
            };
            let mut state = worker_state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.finish(outcome);
        });
    });
    window.dialog.show_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_cancel_does_not_rewrite_worker_result() {
        for outcome in [
            Outcome::Created {
                files: 1,
                bytes: 2,
                consistent: true,
            },
            Outcome::Failed,
            Outcome::Cancelled,
        ] {
            let mut state = ProgressState {
                cancel_requested: true,
                ..Default::default()
            };
            state.finish(Some(outcome));
            assert_eq!(state.outcome, Some(outcome));
            assert!(state.cancel_requested);
        }
    }

    #[test]
    fn late_cancel_keeps_a_real_published_snapshot_successful() {
        let root = std::env::temp_dir().join(format!(
            "ocean-o2-snapshot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let home = root.join("data");
        let backups = root.join("backups");
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        std::fs::create_dir_all(home.join("storages")).unwrap();
        std::fs::create_dir(&backups).unwrap();
        std::fs::write(home.join("sessions/synthetic.jsonl"), b"synthetic").unwrap();
        let path =
            crate::data_backup::snapshot_with_checkpoint(&home, &backups, None, &mut || Ok(()))
                .unwrap();
        let manifest = crate::data_backup::manifest(&path).unwrap();
        let mut state = ProgressState {
            cancel_requested: true,
            ..Default::default()
        };
        state.finish(Some(Outcome::Created {
            files: manifest.files.len(),
            bytes: manifest.total_bytes(),
            consistent: manifest.consistent,
        }));
        assert_eq!(
            state.outcome,
            Some(Outcome::Created {
                files: 1,
                bytes: 9,
                consistent: true
            })
        );
        assert_eq!(
            std::fs::read(path.join("data/sessions/synthetic.jsonl")).unwrap(),
            b"synthetic"
        );
        assert!(crate::data_backup::manifest(&path).unwrap().consistent);
        assert!(root
            .canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap()));
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ocean-o2-snapshot-"));
        std::fs::remove_dir_all(root).unwrap();
    }

    fn summary(
        session_files: usize,
        session_bytes: u64,
        storage_files: usize,
        storage_bytes: u64,
    ) -> Summary {
        Summary {
            session_files,
            session_bytes,
            storage_files,
            storage_bytes,
            skipped: 2,
            truncated: false,
            error: None,
        }
    }

    #[test]
    fn snapshot_dialog_text_uses_selected_locale_and_only_aggregate_counters() {
        for (language, _) in crate::i18n::LANGUAGES {
            let confirmation = confirmation_text_in(&summary(3, 1000, 2, 500), language);
            assert!(
                confirmation.contains("1000")
                    && confirmation.contains("500")
                    && confirmation.contains("1500")
            );
            assert!(!confirmation.contains("/home") && !confirmation.contains("session.jsonl"));
            assert!(result_text_in(4, 2048, language).contains("--verify-data-backup"));
            let state = ProgressState {
                copied_files: 3,
                copied_bytes: 500,
                total_bytes: 1000,
                cancel_requested: true,
                outcome: None,
            };
            let progress = data_backup_progress_text_in(&state, language);
            assert!(
                progress.contains("50")
                    && progress.contains("500")
                    && progress.contains(
                        crate::i18n::message(language, "data_snapshot.cancelling").unwrap()
                    )
            );
        }
    }
    #[test]
    fn confirmation_is_count_only_and_warns_about_space_and_freeze() {
        let text = confirmation_text(&summary(3, 1_000, 2, 500));
        assert!(text.contains("Sessions: 3 file(s), 1000 bytes"), "{text}");
        assert!(
            text.contains("Shared stores: 2 file(s), 500 bytes"),
            "{text}"
        );
        assert!(text.contains("Total to copy: 1500 bytes"), "{text}");
        assert!(text.contains("several gigabytes"), "{text}");
        assert!(text.contains("point-in-time best-effort"), "{text}");
        assert!(text.contains("only read, never moved"), "{text}");
        assert!(text.contains("not offered here"), "{text}");
        // No file names, paths or delete affordances.
        assert!(!text.contains("/home") && !text.contains("session.jsonl"));
        assert!(!text.contains("Delete"));
    }

    #[test]
    fn progress_state_renders_counts_and_cancellation_honestly() {
        let mut state = ProgressState::default();
        assert!(data_backup_progress_text(&state).contains("Copying"));
        state.total_bytes = 1_000;
        state.copied_bytes = 500;
        state.copied_files = 3;
        let text = data_backup_progress_text(&state);
        assert!(text.contains("500 of 1000 bytes (50%)"), "{text}");
        assert!(text.contains("3 file(s)"), "{text}");
        state.cancel_requested = true;
        let text = data_backup_progress_text(&state);
        assert!(text.contains("Cancellation requested"), "{text}");
        assert!(text.contains("staging copy is removed"), "{text}");
        // No GTK object is needed to render progress, and no path leaks.
        assert!(!text.contains('/'));
    }

    #[test]
    fn result_and_failure_texts_never_forward_paths_or_backend_errors() {
        let text = result_text(4, 2_048);
        assert!(text.contains("4 file(s), 2048 bytes"), "{text}");
        assert!(!text.contains("/home") && !text.contains("dsh/sessions"));
        assert!(FAILURE_TEXT.contains("not changed"));
        assert!(CANCELLED_TEXT.contains("removed"));
        assert!(!FAILURE_TEXT.contains('/') && !CANCELLED_TEXT.contains('/'));
    }
}

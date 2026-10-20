//! Tray flow for exporting a snapshot into one archive.
//!
//! Two choosers (snapshot folder, destination file) followed by a confirmation that
//! shows the manifest-only file count and byte total. The tar is written on a
//! blocking worker with count-only progress; the client never overwrites an
//! existing file and never claims the archive is signed.
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gtk::glib;
use gtk::prelude::*;
use tauri::{AppHandle, Manager};

use crate::backup_export::{self, Description};

pub const TITLE: &str = "DSH Ocean export backup";
pub const FAILURE_TEXT: &str =
    "The export did not finish. No snapshot was modified; a partial archive was removed.";
pub const CANCELLED_TEXT: &str =
    "Export cancelled. The partial archive was removed and the snapshot is unchanged.";

pub fn confirmation_text(description: &Description) -> String {
    let scope = match description.scope.as_str() {
        "profile-config" => "configuration",
        "dsh-data" => "sessions and shared stores",
        _ => "snapshot",
    };
    format!(
        "Export this {scope} snapshot into one archive?\n\n\
         Files: {}\n\
         Bytes: {}\n\n\
         The snapshot is verified before exporting, the archive is written once and \
         never overwrites an existing file, and the snapshot itself is not modified. \
         The export is reproducible: the same snapshot always produces the same bytes.\n\n\
         The archive is NOT signed. It proves integrity against its own manifest, not \
         who created it, and it can contain conversation content or credentials, so \
         store it as privately as the backup folder.",
        description.files, description.bytes
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Exported { files: usize, bytes: u64 },
    Cancelled,
    Failed,
}

#[derive(Default)]
pub struct ProgressState {
    pub outcome: Option<Outcome>,
    pub cancel_requested: bool,
}

pub struct Window {
    pub dialog: gtk::Dialog,
    pub label: gtk::Label,
    pub accept: gtk::Button,
}

pub fn build_confirm(parent: Option<&gtk::Window>, body: &str) -> Window {
    let dialog = gtk::Dialog::with_buttons(
        Some(TITLE),
        parent,
        gtk::DialogFlags::MODAL,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Export snapshot", gtk::ResponseType::Accept),
        ],
    );
    dialog.set_default_size(640, 380);
    let label = gtk::Label::new(Some(body));
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_line_wrap(true);
    label.set_max_width_chars(80);
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

/// Folder chooser for the snapshot to export.
pub fn build_snapshot_picker(
    parent: Option<&gtk::Window>,
    folder: Option<&Path>,
) -> gtk::FileChooserDialog {
    let dialog = gtk::FileChooserDialog::with_buttons(
        Some("Choose a snapshot to export"),
        parent,
        gtk::FileChooserAction::SelectFolder,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Choose snapshot", gtk::ResponseType::Accept),
        ],
    );
    dialog.set_modal(true);
    dialog.set_local_only(true);
    dialog.set_create_folders(false);
    dialog.set_default_response(gtk::ResponseType::Accept);
    if let Some(folder) = folder.filter(|path| path.is_dir()) {
        dialog.set_current_folder(folder);
    }
    dialog
}

/// Save chooser for the destination archive.
pub fn build_destination_picker(
    parent: Option<&gtk::Window>,
    folder: Option<&Path>,
) -> gtk::FileChooserDialog {
    let dialog = gtk::FileChooserDialog::with_buttons(
        Some("Save the backup archive"),
        parent,
        gtk::FileChooserAction::Save,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Export here", gtk::ResponseType::Accept),
        ],
    );
    dialog.set_modal(true);
    dialog.set_local_only(true);
    dialog.set_do_overwrite_confirmation(false);
    dialog.set_current_name("dsh-ocean-backup.tar");
    if let Some(folder) = folder.filter(|path| path.is_dir()) {
        dialog.set_current_folder(folder);
    }
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Tar archives"));
    filter.add_pattern("*.tar");
    dialog.add_filter(filter);
    dialog
}

pub fn choose(dialog: &gtk::FileChooserDialog) -> Option<PathBuf> {
    let path = if dialog.run() == gtk::ResponseType::Accept {
        dialog.filename()
    } else {
        None
    };
    dialog.close();
    path
}

pub struct ProgressWindow {
    pub dialog: gtk::Dialog,
    pub label: gtk::Label,
}

pub fn build_progress(parent: Option<&gtk::Window>) -> ProgressWindow {
    let dialog = gtk::Dialog::with_buttons(
        Some(TITLE),
        parent,
        gtk::DialogFlags::MODAL,
        &[("Cancel export", gtk::ResponseType::Cancel)],
    );
    dialog.set_default_size(520, 160);
    let label = gtk::Label::new(Some("Verifying the snapshot before exporting…"));
    label.set_use_markup(false);
    label.set_line_wrap(true);
    label.set_xalign(0.0);
    label.set_margin_start(18);
    label.set_margin_end(18);
    label.set_margin_top(18);
    dialog.content_area().pack_start(&label, false, false, 0);
    // No blanket close here: a caller attaches a handler that keeps the window open
    // to render the terminal outcome, and a blanket close would win the race.

    ProgressWindow { dialog, label }
}

pub fn outcome_text(outcome: Outcome) -> (&'static str, String) {
    match outcome {
        Outcome::Exported { files, bytes } => (
            "Backup exported",
            format!(
                "{files} file(s), {bytes} bytes written to the archive you chose.\n\n\
                 The archive is unsigned: it proves integrity against its own manifest, \
                 not who created it. Verify it with `dsh-ocean --verify-export` before \
                 relying on a copy you did not create."
            ),
        ),
        Outcome::Cancelled => ("Export cancelled", CANCELLED_TEXT.to_owned()),
        Outcome::Failed => ("Export failed", FAILURE_TEXT.to_owned()),
    }
}

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

pub fn show(app: &AppHandle) {
    let parent = app
        .get_webview_window("main")
        .and_then(|window| window.gtk_window().ok());
    let parent_owned = parent.clone();
    let parent_ref = parent.as_ref().map(|window| window.upcast_ref());
    let picker = build_snapshot_picker(parent_ref, Some(&crate::config::profile_backup_dir()));
    let Some(snapshot) = choose(&picker) else {
        return;
    };
    let description = match backup_export::describe(&snapshot) {
        Ok(description) => description,
        Err(error) => {
            crate::ui::message(
                app,
                "Not a supported snapshot",
                &format!(
                    "The selected folder is not a complete Ocean snapshot.\n\n{}",
                    crate::auth::redact(&format!("{error:#}"))
                ),
                gtk::MessageType::Warning,
            );
            return;
        }
    };
    let saver = build_destination_picker(parent_ref, Some(&crate::config::profile_backup_dir()));
    let Some(destination) = choose(&saver) else {
        return;
    };
    let window = build_confirm(parent_ref, &confirmation_text(&description));
    let handle = app.clone();
    let destination_for_work = destination.clone();
    window.dialog.connect_response(move |dialog, response| {
        if response != gtk::ResponseType::Accept {
            dialog.close();
            return;
        }
        let Some(gate) = begin(&handle.state::<crate::state::AppState>().backup_export) else {
            crate::ui::message(
                &handle,
                "Export already running",
                "Another backup export is already running in this session.",
                gtk::MessageType::Info,
            );
            dialog.close();
            return;
        };
        dialog.close();
        let shared = Arc::new(Mutex::new(ProgressState::default()));
        let parent_for_progress = parent_owned.as_ref().map(|window| window.upcast_ref());
        let progress_window = build_progress(parent_for_progress);
        progress_window.dialog.show_all();
        let cancel_state = shared.clone();
        // Cancelling requests a stop and leaves the window to report the terminal
        // outcome (the worker removes a partial archive before finishing).
        progress_window
            .dialog
            .connect_response(move |dialog, response| {
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
        {
            let cancel_state = shared.clone();
            progress_window.dialog.connect_delete_event(move |_, _| {
                let mut state = cancel_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if state.outcome.is_none() {
                    state.cancel_requested = true;
                }
                gtk::glib::Propagation::Stop
            });
        }
        let weak_dialog = progress_window.dialog.downgrade();
        let weak_label = progress_window.label.downgrade();
        let timer_state = shared.clone();
        let timer_app = handle.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            let (Some(dialog), Some(label)) = (weak_dialog.upgrade(), weak_label.upgrade()) else {
                return glib::ControlFlow::Break;
            };
            let (outcome, cancel_requested) = {
                let state = timer_state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                (state.outcome, state.cancel_requested)
            };
            if outcome.is_none() {
                label.set_text(if cancel_requested {
                    "Cancelling after the current file…"
                } else {
                    "Verifying and writing the archive…"
                });
                return glib::ControlFlow::Continue;
            }
            dialog.close();
            let outcome = outcome.expect("checked above");
            let (title, body) = outcome_text(outcome);
            let kind = match outcome {
                Outcome::Exported { .. } => gtk::MessageType::Info,
                Outcome::Cancelled | Outcome::Failed => gtk::MessageType::Warning,
            };
            crate::ui::message(&timer_app, title, &body, kind);
            glib::ControlFlow::Break
        });
        let worker_state = shared.clone();
        let snapshot = snapshot.clone();
        let destination_for_work = destination_for_work.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _gate = gate;
            let outcome = if worker_state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .cancel_requested
            {
                Outcome::Cancelled
            } else {
                let cancel_state = worker_state.clone();
                match backup_export::export_with_checkpoint(
                    &snapshot,
                    &destination_for_work,
                    &mut || {
                        let state = cancel_state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        anyhow::ensure!(!state.cancel_requested, "export cancelled");
                        Ok(())
                    },
                ) {
                    Ok(receipt) => Outcome::Exported {
                        files: receipt.files,
                        bytes: receipt.bytes,
                    },
                    Err(error) => {
                        let cancelled = format!("{error:#}").contains("cancel")
                            || format!("{error:#}").contains("overwrite");
                        if cancelled {
                            Outcome::Cancelled
                        } else {
                            Outcome::Failed
                        }
                    }
                }
            };
            let mut state = worker_state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.outcome = Some(if state.cancel_requested {
                Outcome::Cancelled
            } else {
                outcome
            });
        });
    });
    window.dialog.show_all();
}

/// Creates the private destination parent when missing (used before the worker).
pub fn ensure_parent(destination: &Path) -> std::io::Result<()> {
    match destination.parent() {
        Some(parent) if !parent.as_os_str().is_empty() && !parent.exists() => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn description(scope: &str, files: usize, bytes: u64) -> Description {
        Description {
            scope: scope.to_owned(),
            files,
            bytes,
        }
    }

    #[test]
    fn confirmation_names_the_scope_and_states_limits() {
        let text = confirmation_text(&description("dsh-data", 7, 4096));
        assert!(text.contains("sessions and shared stores"), "{text}");
        assert!(text.contains("Files: 7"), "{text}");
        assert!(text.contains("Bytes: 4096"), "{text}");
        assert!(text.contains("NOT signed"), "{text}");
        assert!(text.contains("never overwrites"), "{text}");
        let profile = confirmation_text(&description("profile-config", 2, 10));
        assert!(profile.contains("configuration snapshot"), "{profile}");
        // No paths, no file names and no destructive verbs.
        assert!(!text.contains("/home") && !text.contains("Delete"));
    }

    #[test]
    fn outcomes_have_fixed_honest_text() {
        let (title, body) = outcome_text(Outcome::Exported {
            files: 3,
            bytes: 100,
        });
        assert_eq!(title, "Backup exported");
        assert!(body.contains("3 file(s), 100 bytes"), "{body}");
        assert!(body.contains("unsigned") && body.contains("--verify-export"));
        let (title, body) = outcome_text(Outcome::Cancelled);
        assert_eq!(title, "Export cancelled");
        assert!(body.contains("partial archive was removed"), "{body}");
        let (title, body) = outcome_text(Outcome::Failed);
        assert_eq!(title, "Export failed");
        assert!(body.contains("No snapshot was modified"), "{body}");
        assert!(!body.contains("/home"));
    }
}

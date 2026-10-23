//! Native read-only snapshot integrity inspection; never restore or service IO.
use gtk::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{AppHandle, Manager};

pub struct Summary {
    profile: String,
    created: Option<String>,
    files: usize,
    directories: usize,
    bytes: u64,
}
impl Summary {
    pub fn text(&self) -> String {
        self.text_in(crate::i18n::active_language())
    }
    pub fn text_in(&self, language: crate::i18n::Language) -> String {
        let created = self.created.as_deref().unwrap_or_else(|| {
            crate::i18n::message(language, "verification.timestamp_unavailable")
                .expect("embedded timestamp message")
        });
        crate::i18n::render(
            language,
            "verification.summary",
            &[
                ("profile", &self.profile),
                ("created", created),
                ("files", &self.files.to_string()),
                ("directories", &self.directories.to_string()),
                ("bytes", &self.bytes.to_string()),
            ],
        )
        .expect("validated verification summary placeholders")
    }
}
pub fn verify_snapshot(path: &Path) -> anyhow::Result<Summary> {
    summary(crate::profile_backup::verify(path)?)
}
fn summary(manifest: crate::profile_backup::Manifest) -> anyhow::Result<Summary> {
    // Do not pass contents, per-file paths, raw JSON or user-supplied strings
    // through to the GTK result; even timestamp metadata is parsed, not echoed.
    let created = if manifest.created_at.len() <= 128 {
        chrono::DateTime::parse_from_rfc3339(&manifest.created_at)
            .ok()
            .map(|time| time.with_timezone(&chrono::Utc).to_rfc3339())
    } else {
        None
    };
    let bytes = manifest
        .files
        .iter()
        .try_fold(0u64, |total, file| total.checked_add(file.size))
        .ok_or_else(|| anyhow::anyhow!("snapshot size overflow"))?;
    Ok(Summary {
        profile: manifest.profile,
        created,
        files: manifest.files.len(),
        directories: manifest.directories.len(),
        bytes,
    })
}
pub const FAILURE_TEXT:&str="Could not verify the selected configuration snapshot. It may be incomplete, changed, unsupported or unreadable. No snapshot/profile files, services or packages were changed. Raw paths and file contents are not displayed here. The explicit --verify-profile-backup CLI offers local operator diagnostics; do not share them without checking for private metadata.";
pub fn build_picker(parent: Option<&gtk::Window>, folder: Option<&Path>) -> gtk::FileChooserDialog {
    let dialog = gtk::FileChooserDialog::with_buttons(
        Some(crate::i18n::text("verification.picker_title")),
        parent,
        gtk::FileChooserAction::SelectFolder,
        &[
            (
                crate::i18n::text("common.cancel"),
                gtk::ResponseType::Cancel,
            ),
            (
                crate::i18n::text("verification.select"),
                gtk::ResponseType::Accept,
            ),
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
pub fn choose(dialog: &gtk::FileChooserDialog) -> Option<PathBuf> {
    let path = if dialog.run() == gtk::ResponseType::Accept {
        dialog.filename()
    } else {
        None
    };
    dialog.close();
    path
}
pub struct Window {
    pub dialog: gtk::Dialog,
    pub label: gtk::Label,
}
pub fn build_summary(parent: Option<&gtk::Window>, summary: &Summary) -> Window {
    let dialog = gtk::Dialog::with_buttons(
        Some(crate::i18n::text("verification.summary_title")),
        parent,
        gtk::DialogFlags::MODAL,
        &[(crate::i18n::text("common.close"), gtk::ResponseType::Close)],
    );
    dialog.set_default_size(680, 420);
    let label = gtk::Label::new(Some(&summary.text()));
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_line_wrap(true);
    label.set_max_width_chars(85);
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
    dialog.connect_response(|dialog, _| dialog.close());
    Window { dialog, label }
}
pub enum Outcome {
    Passed(Summary),
    Failed,
    Cancelled,
}
#[derive(Default)]
struct WorkState {
    started: bool,
    finished: bool,
    cancel_requested: bool,
    progress: crate::profile_backup::VerificationProgress,
    result: Option<Outcome>,
}
#[derive(Default)]
pub struct Work(std::sync::Mutex<WorkState>);
impl Work {
    /// True means cancellation won before result commitment, not that I/O stopped.
    pub fn request_cancel(&self) -> bool {
        let mut state = self.0.lock().expect("verification mutex");
        if state.finished {
            return false;
        }
        state.cancel_requested = true;
        true
    }
    pub fn finished(&self) -> bool {
        self.0.lock().expect("verification mutex").finished
    }
    pub fn progress_text(&self) -> String {
        self.progress_text_in(crate::i18n::active_language())
    }
    pub fn progress_text_in(&self, language: crate::i18n::Language) -> String {
        let state = self.0.lock().expect("verification mutex");
        let stage = match state.progress.stage {
            crate::profile_backup::VerificationStage::Opening => "verification.stage.opening",
            crate::profile_backup::VerificationStage::Inventory => "verification.stage.inventory",
            crate::profile_backup::VerificationStage::Hashing => "verification.stage.hashing",
            crate::profile_backup::VerificationStage::Rechecking => "verification.stage.rechecking",
        };
        let notice = if state.cancel_requested {
            "verification.cancelling"
        } else {
            "verification.working"
        };
        crate::i18n::render(
            language,
            "verification.progress",
            &[
                (
                    "stage",
                    crate::i18n::message(language, stage).expect("embedded verification stage"),
                ),
                ("bytes", &state.progress.hashed_bytes.to_string()),
                ("files", &state.progress.checked_files.to_string()),
                (
                    "total",
                    &state
                        .progress
                        .total_files
                        .map(|total| format!(" / {total}"))
                        .unwrap_or_default(),
                ),
                (
                    "notice",
                    crate::i18n::message(language, notice)
                        .expect("embedded verification progress notice"),
                ),
            ],
        )
        .expect("validated verification progress placeholders")
    }
    pub fn take_result(&self) -> Option<Outcome> {
        self.0.lock().expect("verification mutex").result.take()
    }
    fn finish(&self, result: anyhow::Result<Summary>) {
        let mut state = self.0.lock().expect("verification mutex");
        if state.finished {
            return;
        }
        state.result = Some(if state.cancel_requested {
            Outcome::Cancelled
        } else {
            match result {
                Ok(summary) => Outcome::Passed(summary),
                Err(_) => Outcome::Failed,
            }
        });
        state.finished = true;
    }
    pub fn run(&self, path: &Path) {
        {
            let mut state = self.0.lock().expect("verification mutex");
            if state.started || state.finished {
                return;
            }
            state.started = true;
        }
        let result = crate::profile_backup::verify_with_progress(path, |progress| {
            let mut state = self.0.lock().expect("verification mutex");
            anyhow::ensure!(!state.cancel_requested, "verification cancelled");
            state.progress = progress;
            Ok(())
        })
        .and_then(summary);
        self.finish(result);
    }
}
pub fn build_progress(parent: Option<&gtk::Window>, work: Arc<Work>) -> Window {
    let dialog = gtk::Dialog::with_buttons(
        Some(crate::i18n::text("verification.progress_title")),
        parent,
        gtk::DialogFlags::MODAL,
        &[(
            crate::i18n::text("verification.cancel"),
            gtk::ResponseType::Cancel,
        )],
    );
    dialog.set_default_size(600, 280);
    let label = gtk::Label::new(Some(&work.progress_text()));
    label.set_use_markup(false);
    label.set_selectable(true);
    label.set_line_wrap(true);
    label.set_max_width_chars(75);
    label.set_margin_start(18);
    label.set_margin_end(18);
    label.set_margin_top(18);
    label.set_margin_bottom(18);
    dialog.content_area().pack_start(&label, true, true, 0);
    let controller = work.clone();
    let text = label.downgrade();
    dialog.connect_response(move |dialog, _| {
        if controller.request_cancel() {
            if let Some(label) = text.upgrade() {
                label.set_text(&controller.progress_text());
            }
            if let Some(button) = dialog.widget_for_response(gtk::ResponseType::Cancel) {
                button.set_sensitive(false);
            }
        }
    });
    dialog.connect_delete_event(move |_, _| {
        if work.finished() {
            gtk::glib::Propagation::Proceed
        } else {
            work.request_cancel();
            gtk::glib::Propagation::Stop
        }
    });
    Window { dialog, label }
}
struct Guard(Arc<AtomicBool>);
impl Guard {
    fn acquire(flag: Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(flag))
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
pub fn show(app: &AppHandle) {
    let flag = app
        .state::<crate::state::AppState>()
        .profile_verifying
        .clone();
    let Some(guard) = Guard::acquire(flag) else {
        crate::ui::message(
            app,
            crate::i18n::text("verification.busy_title"),
            crate::i18n::text("verification.busy"),
            gtk::MessageType::Info,
        );
        return;
    };
    let parent = app
        .get_webview_window("main")
        .and_then(|window| window.gtk_window().ok());
    let dialog = build_picker(
        parent.as_ref().map(|window| window.upcast_ref()),
        Some(&crate::config::profile_backup_dir()),
    );
    let Some(path) = choose(&dialog) else {
        return;
    };
    let work = Arc::new(Work::default());
    let window = build_progress(
        parent.as_ref().map(|window| window.upcast_ref()),
        work.clone(),
    );
    window.dialog.show_all();
    let weak_dialog = window.dialog.downgrade();
    let weak_label = window.label.downgrade();
    let owner = app.clone();
    let observed = work.clone();
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        let Some(dialog) = weak_dialog.upgrade() else {
            observed.request_cancel();
            return gtk::glib::ControlFlow::Break;
        };
        if let Some(label) = weak_label.upgrade() {
            label.set_text(&observed.progress_text());
        }
        let Some(outcome) = observed.take_result() else {
            return gtk::glib::ControlFlow::Continue;
        };
        dialog.close();
        match outcome {
            Outcome::Passed(summary) => {
                let parent = owner
                    .get_webview_window("main")
                    .and_then(|window| window.gtk_window().ok());
                let window =
                    build_summary(parent.as_ref().map(|window| window.upcast_ref()), &summary);
                window.dialog.show_all();
            }
            Outcome::Failed => crate::ui::message(
                &owner,
                crate::i18n::text("verification.failed_title"),
                crate::i18n::text("verification.failure"),
                gtk::MessageType::Error,
            ),
            Outcome::Cancelled => crate::ui::message(
                &owner,
                crate::i18n::text("verification.cancelled_title"),
                crate::i18n::text("verification.cancelled"),
                gtk::MessageType::Info,
            ),
        }
        gtk::glib::ControlFlow::Break
    });
    let failed = work.clone();
    let task = tauri::async_runtime::spawn_blocking(move || {
        work.run(&path);
        drop(guard);
    });
    tauri::async_runtime::spawn(async move {
        if task.await.is_err() {
            failed.finish(Err(anyhow::anyhow!("verification worker failed")));
        }
    });
}
#[cfg(test)]
#[test]
fn verification_summary_and_progress_are_localized_without_claiming_restore_or_authenticity() {
    for (language, _) in crate::i18n::LANGUAGES {
        let summary = Summary {
            profile: "web".into(),
            created: None,
            files: 4,
            directories: 2,
            bytes: 2048,
        };
        let body = summary.text_in(language);
        assert!(body.contains("2048") && body.contains("web"));
        assert!(body.contains(
            crate::i18n::message(language, "verification.timestamp_unavailable").unwrap()
        ));
        assert!(!body.contains("/home/") && !body.contains("{profile}"));
        let work = Work::default();
        for (stage, key) in [
            (crate::profile_backup::VerificationStage::Opening, "opening"),
            (
                crate::profile_backup::VerificationStage::Inventory,
                "inventory",
            ),
            (crate::profile_backup::VerificationStage::Hashing, "hashing"),
            (
                crate::profile_backup::VerificationStage::Rechecking,
                "rechecking",
            ),
        ] {
            work.0.lock().unwrap().progress.stage = stage;
            let progress = work.progress_text_in(language);
            assert!(progress.contains(
                crate::i18n::message(language, &format!("verification.stage.{key}")).unwrap()
            ));
        }
        assert!(work.request_cancel());
        assert!(work
            .progress_text_in(language)
            .contains(crate::i18n::message(language, "verification.cancelling").unwrap()));
    }
}
#[cfg(test)]
#[test]
fn cancellation_and_result_commit_are_linearized_without_late_success_or_rewrite() {
    let result = || {
        Ok(Summary {
            profile: "web".into(),
            created: Some("test metadata".into()),
            files: 1,
            directories: 0,
            bytes: 1,
        })
    };
    let cancelled = Work::default();
    assert!(cancelled.request_cancel());
    cancelled.finish(result());
    assert!(matches!(cancelled.take_result(), Some(Outcome::Cancelled)));
    cancelled.finish(result());
    assert!(cancelled.take_result().is_none());
    let completed = Work::default();
    completed.finish(result());
    assert!(!completed.request_cancel());
    assert!(matches!(completed.take_result(), Some(Outcome::Passed(_))));
    completed.finish(Err(anyhow::anyhow!("late failure")));
    assert!(completed.take_result().is_none());
    for _ in 0..32 {
        let work = Arc::new(Work::default());
        let start = Arc::new(std::sync::Barrier::new(3));
        let request = work.clone();
        let ready = start.clone();
        let cancel = std::thread::spawn(move || {
            ready.wait();
            request.request_cancel()
        });
        let complete = work.clone();
        let ready = start.clone();
        let commit = std::thread::spawn(move || {
            ready.wait();
            complete.finish(Ok(Summary {
                profile: "web".into(),
                created: Some("test".into()),
                files: 1,
                directories: 0,
                bytes: 1,
            }));
        });
        start.wait();
        let won = cancel.join().unwrap();
        commit.join().unwrap();
        assert!(matches!(
            (won, work.take_result()),
            (true, Some(Outcome::Cancelled)) | (false, Some(Outcome::Passed(_)))
        ));
        assert!(work.finished() && !work.request_cancel());
    }
}
#[cfg(test)]
#[test]
fn verification_gate_is_exclusive_and_released_on_cancel_or_completion() {
    let flag = Arc::new(AtomicBool::new(false));
    let first = Guard::acquire(flag.clone()).unwrap();
    assert!(Guard::acquire(flag.clone()).is_none());
    drop(first);
    assert!(Guard::acquire(flag.clone()).is_some());
    let start = Arc::new(std::sync::Barrier::new(8));
    let attempted = Arc::new(std::sync::Barrier::new(8));
    let winners = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let flag = flag.clone();
        let start = start.clone();
        let attempted = attempted.clone();
        let winners = winners.clone();
        workers.push(std::thread::spawn(move || {
            start.wait();
            let guard = Guard::acquire(flag);
            if guard.is_some() {
                winners.fetch_add(1, Ordering::Relaxed);
            }
            attempted.wait();
            drop(guard);
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(winners.load(Ordering::Relaxed), 1);
    assert!(!flag.load(Ordering::Acquire));
}

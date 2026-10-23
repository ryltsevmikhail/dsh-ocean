//! Synthetic snapshots only: no actual DSH profile, credentials or services.
use dsh_ocean::{profile_backup::ProfileStore, profile_verify_ui as ui};
use gtk::{glib, prelude::*};
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    profile: PathBuf,
    snapshot: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-verify-ui-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let home = root.join("synthetic-home");
        let profile = home.join("profiles/web");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            profile.join("package.json"),
            "{\"dsh\":{\"profile\":{\"bundles\":[]}}}\n",
        )
        .unwrap();
        fs::write(
            profile.join("private-filename-should-not-appear.yaml"),
            "synthetic-secret-profile-value\n",
        )
        .unwrap();
        fs::write(
            home.join(".credentials.yaml"),
            "synthetic-global-secret-marker\n",
        )
        .unwrap();
        let snapshot = ProfileStore::new(&home, &root.join("backups"))
            .unwrap()
            .create("web")
            .unwrap();
        Self {
            root,
            profile,
            snapshot,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.root.is_absolute()
                && self
                    .root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("dsh-ocean-verify-ui-")
        );
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn verification_summary_is_content_free_and_does_not_modify_snapshot_or_profile() {
    let f = Fixture::new();
    let manifest = f.snapshot.join("manifest.json");
    let file = f
        .snapshot
        .join("data/private-filename-should-not-appear.yaml");
    let before = fs::read(&manifest).unwrap();
    let anchor = UNIX_EPOCH + Duration::from_secs(123);
    fs::File::open(&manifest)
        .unwrap()
        .set_modified(anchor)
        .unwrap();
    fs::File::open(&file).unwrap().set_modified(anchor).unwrap();
    let summary = ui::verify_snapshot(&f.snapshot).unwrap().text();
    assert!(
        summary.contains("Profile: web")
            && summary.contains("Files: 2")
            && summary.contains("Checksums are not signatures")
            && summary.contains("best-effort")
    );
    assert!(
        !summary.contains("private-filename")
            && !summary.contains("synthetic-secret")
            && !summary.contains("synthetic-global")
            && !summary.contains(&f.snapshot.to_string_lossy().to_string())
    );
    assert_eq!(fs::read(&manifest).unwrap(), before);
    assert_eq!(fs::metadata(&manifest).unwrap().modified().unwrap(), anchor);
    assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), anchor);
    assert_eq!(
        fs::read_to_string(f.profile.join("private-filename-should-not-appear.yaml")).unwrap(),
        "synthetic-secret-profile-value\n"
    );
    assert_eq!(fs::read_dir(&f.snapshot).unwrap().count(), 2);
}
#[test]
fn malformed_time_is_not_echoed_and_corrupted_snapshot_never_produces_success_summary() {
    let f = Fixture::new();
    let path = f.snapshot.join("manifest.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    metadata["createdAt"] = "synthetic-private-invalid-timestamp-token".into();
    fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let verified = ui::verify_snapshot(&f.snapshot).unwrap();
    for (language, _) in dsh_ocean::i18n::LANGUAGES {
        let localized = verified.text_in(language);
        assert!(localized.contains(
            dsh_ocean::i18n::message(language, "verification.timestamp_unavailable").unwrap()
        ));
        assert!(!localized.contains("synthetic-private"));
    }
    let text = verified.text();
    assert!(text.contains("unavailable (invalid metadata timestamp)"));
    assert!(!text.contains("synthetic-private"));
    let data = f
        .snapshot
        .join("data/private-filename-should-not-appear.yaml");
    fs::write(&data, "synthetic-tampered-value").unwrap();
    assert!(ui::verify_snapshot(&f.snapshot).is_err());
    assert_eq!(
        fs::read_to_string(&data).unwrap(),
        "synthetic-tampered-value"
    );
    assert!(
        !ui::FAILURE_TEXT.contains("private-filename")
            && !ui::FAILURE_TEXT.contains("synthetic-tampered")
    );
    assert!(
        ui::FAILURE_TEXT.contains("No snapshot/profile files, services or packages were changed")
    );
}
#[test]
fn cooperative_progress_is_aggregate_monotonic_and_can_abort_between_chunks() {
    let f = Fixture::new();
    fs::write(
        f.profile.join("private-filename-should-not-appear.yaml"),
        vec![b'x'; 2 * 1024 * 1024],
    )
    .unwrap();
    let snapshot = ProfileStore::new(&f.root.join("synthetic-home"), &f.root.join("backups"))
        .unwrap()
        .create("web")
        .unwrap();
    let data = snapshot.join("data/private-filename-should-not-appear.yaml");
    let before = fs::read(&data).unwrap();
    let mut previous = dsh_ocean::profile_backup::VerificationProgress::default();
    let stopped = dsh_ocean::profile_backup::verify_with_progress(&snapshot, |progress| {
        assert!(
            progress.hashed_bytes >= previous.hashed_bytes
                && progress.checked_files >= previous.checked_files
        );
        previous = progress;
        anyhow::ensure!(progress.hashed_bytes < 65536, "synthetic-stop-at-chunk");
        Ok(())
    });
    assert!(stopped.is_err());
    assert!(
        previous.hashed_bytes >= 65536 && previous.hashed_bytes < previous.total_bytes.unwrap()
    );
    assert!(previous.checked_files < previous.total_files.unwrap());
    assert_eq!(fs::read(&data).unwrap(), before);
    let work = ui::Work::default();
    work.run(&snapshot);
    assert!(matches!(work.take_result(), Some(ui::Outcome::Passed(_))));
    assert!(work.finished() && !work.request_cancel());
    assert!(work.take_result().is_none());
}
#[test]
fn cancellation_before_io_and_after_last_hash_cannot_report_success() {
    let f = Fixture::new();
    let missing = f.root.join("missing-do-not-create");
    let work = ui::Work::default();
    assert!(work.request_cancel());
    assert!(!work.finished());
    assert!(work.progress_text().contains("Cancellation requested"));
    work.run(&missing);
    assert!(matches!(work.take_result(), Some(ui::Outcome::Cancelled)));
    assert!(work.finished() && !missing.exists());
    let failed = ui::Work::default();
    failed.run(&missing);
    assert!(matches!(failed.take_result(), Some(ui::Outcome::Failed)));
    let final_stop = dsh_ocean::profile_backup::verify_with_progress(&f.snapshot, |progress| {
        anyhow::ensure!(
            progress.total_files != Some(progress.checked_files),
            "synthetic-stop-after-last-hash"
        );
        Ok(())
    });
    assert!(final_stop.is_err());
    assert!(ui::verify_snapshot(&f.snapshot).is_ok());
}
#[test]
fn cancellation_interrupts_metadata_enumeration_and_final_inventory_without_hash_success() {
    use dsh_ocean::profile_backup::{verify_with_progress, VerificationStage};
    let f = Fixture::new();
    for n in 0..40 {
        fs::create_dir(f.profile.join(format!("empty-{n:02}"))).unwrap();
    }
    let snapshot = ProfileStore::new(&f.root.join("synthetic-home"), &f.root.join("backups"))
        .unwrap()
        .create("web")
        .unwrap();
    let mut metadata_calls = 0;
    let scan = verify_with_progress(&snapshot, |progress| {
        if progress.stage == VerificationStage::Inventory {
            metadata_calls += 1;
            assert_eq!(progress.hashed_bytes, 0);
            assert_eq!(progress.checked_files, 0);
            anyhow::ensure!(metadata_calls < 10, "synthetic-mid-enumeration-cancel");
        }
        Ok(())
    });
    assert!(scan.is_err() && metadata_calls == 10);
    let mut recheck_calls = 0;
    let final_scan = verify_with_progress(&snapshot, |progress| {
        if progress.stage == VerificationStage::Rechecking {
            recheck_calls += 1;
            assert_eq!(progress.total_files, Some(progress.checked_files));
            assert_eq!(progress.total_bytes, Some(progress.hashed_bytes));
            anyhow::ensure!(recheck_calls < 10, "synthetic-final-inventory-cancel");
        }
        Ok(())
    });
    assert!(final_scan.is_err() && recheck_calls == 10);
    assert!(ui::verify_snapshot(&snapshot).is_ok());
    let mut opening = 0;
    verify_with_progress(&snapshot, |progress| {
        if progress.stage == VerificationStage::Opening {
            opening += 1;
        }
        Ok(())
    })
    .unwrap();
    let manifest = snapshot.join("manifest.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    metadata["createdAt"] = "x".repeat(200000).into();
    fs::write(&manifest, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let mut calls = 0;
    let metadata_cancel = verify_with_progress(&snapshot, |progress| {
        if progress.stage == VerificationStage::Opening {
            calls += 1;
            assert_eq!(progress.hashed_bytes, 0);
            anyhow::ensure!(calls <= opening, "synthetic-manifest-chunk-cancel");
        }
        Ok(())
    });
    assert!(metadata_cancel.is_err() && calls == opening + 1);
    assert!(ui::verify_snapshot(&snapshot).is_ok());
}
#[test]
fn post_hash_changes_and_identity_replacements_fail_without_cleanup_or_restore() {
    use dsh_ocean::profile_backup::{verify_with_progress, VerificationStage};
    for case in [
        "file",
        "extra-data",
        "extra-root",
        "manifest",
        "manifest-replaced",
        "data-replaced",
        "snapshot-replaced",
    ] {
        let f = Fixture::new();
        let original = fs::read(f.profile.join("package.json")).unwrap();
        let mut changed = false;
        let verified = verify_with_progress(&f.snapshot, |progress| {
            if progress.stage == VerificationStage::Rechecking && !changed {
                changed = true;
                assert_eq!(progress.total_bytes, Some(progress.hashed_bytes));
                match case {
                    "file" => fs::write(
                        f.snapshot.join("data/package.json"),
                        "synthetic-post-hash-edit",
                    )
                    .unwrap(),
                    "extra-data" => {
                        fs::write(f.snapshot.join("data/unexpected"), "synthetic-extra").unwrap()
                    }
                    "extra-root" => {
                        fs::write(f.snapshot.join("unexpected"), "synthetic-extra").unwrap()
                    }
                    "manifest" => {
                        let file = f.snapshot.join("manifest.json");
                        let mut metadata: serde_json::Value =
                            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
                        metadata["createdAt"] = "changed-after-parse".into();
                        fs::write(file, serde_json::to_vec(&metadata).unwrap()).unwrap();
                    }
                    "manifest-replaced" => {
                        let old = f.snapshot.join("manifest.json");
                        let new = f.snapshot.join("replacement.json");
                        fs::write(&new, fs::read(&old).unwrap()).unwrap();
                        fs::set_permissions(&new, fs::Permissions::from_mode(0o600)).unwrap();
                        assert!(
                            new.is_absolute()
                                && old.is_absolute()
                                && new.starts_with(&f.root)
                                && old.starts_with(&f.root)
                        );
                        fs::rename(new, old).unwrap();
                    }
                    "data-replaced" => {
                        let source = f.snapshot.join("data");
                        let retained = f.root.join("retained-data");
                        assert!(
                            source.is_absolute()
                                && retained.is_absolute()
                                && source.starts_with(&f.root)
                                && retained.starts_with(&f.root)
                        );
                        fs::rename(&source, retained).unwrap();
                        fs::create_dir(&source).unwrap();
                        fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
                    }
                    "snapshot-replaced" => {
                        let retained = f.root.join("retained-snapshot");
                        assert!(
                            f.snapshot.is_absolute()
                                && retained.is_absolute()
                                && f.snapshot.starts_with(&f.root)
                                && retained.starts_with(&f.root)
                        );
                        fs::rename(&f.snapshot, retained).unwrap();
                        fs::create_dir(&f.snapshot).unwrap();
                        fs::set_permissions(&f.snapshot, fs::Permissions::from_mode(0o700))
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
            }
            Ok(())
        });
        assert!(
            changed && verified.is_err(),
            "{case} must not yield a verified summary"
        );
        assert_eq!(fs::read(f.profile.join("package.json")).unwrap(), original);
        match case {
            "file" => assert_eq!(
                fs::read_to_string(f.snapshot.join("data/package.json")).unwrap(),
                "synthetic-post-hash-edit"
            ),
            "extra-data" => assert!(f.snapshot.join("data/unexpected").exists()),
            "extra-root" => assert!(f.snapshot.join("unexpected").exists()),
            "data-replaced" => assert!(f.root.join("retained-data/package.json").exists()),
            "snapshot-replaced" => assert!(f.root.join("retained-snapshot/manifest.json").exists()),
            _ => {}
        }
    }
}
fn tick() {
    for _ in 0..32 {
        if !glib::MainContext::default().pending() {
            break;
        }
        glib::MainContext::default().iteration(false);
    }
}
fn select(dialog: &gtk::FileChooserDialog, snapshot: Option<PathBuf>) -> Option<PathBuf> {
    eprintln!("[verify-ui] picker start: accept={}", snapshot.is_some());
    let popup = dialog.clone();
    let configured = Rc::new(Cell::new(false));
    let ready = configured.clone();
    let expected = snapshot.clone();
    let driver = glib::timeout_add_local(Duration::from_millis(20), move || {
        if !popup.is_mapped() {
            return glib::ControlFlow::Continue;
        }
        if let Some(path) = &snapshot {
            // SelectFolder can choose the current directory. Navigate into it
            // instead of treating async row selection's accepted request as
            // readiness while the initial folder model may still be loading.
            if !ready.get() && popup.set_current_folder(path) {
                ready.set(true);
            }
            if popup.current_folder().as_ref() != Some(path)
                || popup.filename().as_ref() != Some(path)
            {
                return glib::ControlFlow::Continue;
            }
        }
        let response = if snapshot.is_some() {
            gtk::ResponseType::Accept
        } else {
            gtk::ResponseType::Cancel
        };
        if let Some(button) = popup
            .widget_for_response(response)
            .and_then(|widget| widget.downcast::<gtk::Button>().ok())
        {
            if button.is_sensitive() {
                button.emit_clicked();
            }
        }
        glib::ControlFlow::Continue
    });
    let timeout = Rc::new(Cell::new(false));
    let flag = timeout.clone();
    let popup = dialog.clone();
    let target = expected.clone();
    let configured = configured.clone();
    let watchdog = glib::timeout_add_local(Duration::from_secs(8), move || {
        eprintln!("[verify-ui] picker watchdog: accept={}, mapped={}, configured={}, folder_matches={}, filename_matches={}",target.is_some(),popup.is_mapped(),configured.get(),popup.current_folder().as_ref()==target.as_ref(),popup.filename().as_ref()==target.as_ref());
        flag.set(true);
        popup.response(gtk::ResponseType::Cancel);
        glib::ControlFlow::Continue
    });
    let selected = ui::choose(dialog);
    driver.remove();
    watchdog.remove();
    assert!(!timeout.get(), "profile snapshot picker timed out");
    assert_eq!(selected, expected);
    eprintln!("[verify-ui] picker finished: accept={}", expected.is_some());
    selected
}
#[test]
#[ignore = "requires explicitly enabled isolated GTK dialog display"]
fn actual_snapshot_picker_cancel_verify_summary_and_close_never_offer_restore() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let f = Fixture::new();
    let before = fs::read(f.snapshot.join("manifest.json")).unwrap();
    let parent = gtk::Window::new(gtk::WindowType::Toplevel);
    parent.show_all();
    let work = std::sync::Arc::new(ui::Work::default());
    let progress = ui::build_progress(Some(&parent), work.clone());
    progress.dialog.show_all();
    tick();
    assert!(progress.label.is_selectable() && !progress.label.uses_markup());
    assert!(progress
        .label
        .text()
        .contains("Stage: Opening snapshot metadata"));
    let cancel_button = progress
        .dialog
        .widget_for_response(gtk::ResponseType::Cancel)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    cancel_button.emit_clicked();
    tick();
    assert!(progress.dialog.is_visible() && !work.finished());
    assert!(progress.label.text().contains("Cancellation requested"));
    assert!(!cancel_button.is_sensitive());
    progress.dialog.close();
    tick();
    assert!(
        progress.dialog.is_visible(),
        "pending I/O cancellation must not pretend the worker finished"
    );
    work.run(&f.snapshot);
    assert!(matches!(work.take_result(), Some(ui::Outcome::Cancelled)));
    progress.dialog.close();
    tick();
    assert!(!progress.dialog.is_visible());
    let cancel = ui::build_picker(Some(&parent), Some(&f.root));
    assert_eq!(cancel.action(), gtk::FileChooserAction::SelectFolder);
    assert!(cancel.is_local_only() && !cancel.property::<bool>("create-folders"));
    assert_eq!(cancel.transient_for(), Some(parent.clone()));
    assert!(select(&cancel, None).is_none());
    let picker = ui::build_picker(Some(&parent), Some(&f.root));
    let chosen = select(&picker, Some(f.snapshot.clone())).unwrap();
    let summary = ui::verify_snapshot(&chosen).unwrap();
    let window = ui::build_summary(Some(&parent), &summary);
    window.dialog.show_all();
    tick();
    assert!(window.label.is_selectable() && !window.label.uses_markup());
    assert!(window.dialog.is_modal());
    assert_eq!(window.dialog.transient_for(), Some(parent.clone()));
    assert!(window.label.text().contains("Checksums are not signatures"));
    assert!(window
        .label
        .text()
        .contains("Restore remains an explicit offline CLI"));
    assert!(!window.label.text().contains("synthetic-secret"));
    assert!(
        window
            .dialog
            .widget_for_response(gtk::ResponseType::Accept)
            .is_none()
            && window
                .dialog
                .widget_for_response(gtk::ResponseType::Apply)
                .is_none()
    );
    window
        .dialog
        .widget_for_response(gtk::ResponseType::Close)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap()
        .emit_clicked();
    let end = Instant::now() + Duration::from_secs(5);
    while window.dialog.is_visible() {
        tick();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fs::read(f.snapshot.join("manifest.json")).unwrap(), before);
    assert_eq!(fs::read_dir(&f.snapshot).unwrap().count(), 2);
    parent.close();
}

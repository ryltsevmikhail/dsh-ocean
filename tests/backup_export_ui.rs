//! GTK acceptance for the export dialogs.
//!
//! One GTK test per binary (gtk-rs allows a single initializing thread). Synthetic
//! snapshots only; nothing is exported by opening a dialog.
#![cfg(target_os = "linux")]
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use dsh_ocean::backup_export_ui as ui;
use gtk::prelude::*;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let serial = NEXT.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-export-ui-{}-{serial}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self
            .root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("dsh-ocean-export-ui-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn tick() {
    for _ in 0..32 {
        if !gtk::glib::MainContext::default().pending() {
            break;
        }
        gtk::glib::MainContext::default().iteration(false);
    }
}

#[test]
#[ignore = "requires explicitly enabled isolated GTK dialog display"]
fn export_dialogs_are_choosers_and_close_only_surfaces() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let fixture = Fixture::new();

    // Both choosers exist, are modal, and never run a transfer by themselves.
    let snapshot_picker = ui::build_snapshot_picker(None, Some(&fixture.root));
    snapshot_picker.show_all();
    tick();
    assert!(snapshot_picker.is_modal());
    assert_eq!(
        snapshot_picker.action(),
        gtk::FileChooserAction::SelectFolder
    );
    assert!(snapshot_picker.is_local_only());
    assert!(!snapshot_picker.title().unwrap_or_default().is_empty());
    snapshot_picker.close();

    let destination_picker = ui::build_destination_picker(None, Some(&fixture.root));
    destination_picker.show_all();
    tick();
    assert_eq!(destination_picker.action(), gtk::FileChooserAction::Save);
    assert_eq!(
        destination_picker.current_name().unwrap_or_default(),
        "dsh-ocean-backup.tar"
    );
    destination_picker.close();

    // The confirmation shows counts and warns that nothing is signed.
    let description = dsh_ocean::backup_export::Description {
        scope: "dsh-data".to_owned(),
        files: 4,
        bytes: 2048,
    };
    let body = ui::confirmation_text(&description);
    let window = ui::build_confirm(None, &body);
    window.dialog.show_all();
    tick();
    assert_eq!(window.dialog.title().as_deref(), Some(ui::TITLE));
    assert!(window.accept.is_sensitive());
    assert!(window.label.is_selectable() && !window.label.uses_markup());
    for response in [
        gtk::ResponseType::Ok,
        gtk::ResponseType::DeleteEvent,
        gtk::ResponseType::Apply,
    ] {
        assert!(
            window.dialog.widget_for_response(response).is_none(),
            "export confirmation must not offer {response:?}"
        );
    }
    let cancel = window
        .dialog
        .widget_for_response(gtk::ResponseType::Cancel)
        .expect("cancel button")
        .downcast::<gtk::Button>()
        .expect("cancel button is a gtk button");
    cancel.emit_clicked();
    tick();
    // Cancelling leaves no archive behind.
    assert!(fs::read_dir(&fixture.root).unwrap().next().is_none());

    let progress = ui::build_progress(None);
    progress.dialog.show_all();
    tick();
    assert!(progress.label.text().contains("Verifying"));
    let cancel = progress
        .dialog
        .widget_for_response(gtk::ResponseType::Cancel)
        .expect("cancel button")
        .downcast::<gtk::Button>()
        .expect("cancel button is a gtk button");
    // The progress window itself never closes on cancel: the caller keeps it to
    // report the terminal outcome ("partial archive removed" must be visible).
    cancel.emit_clicked();
    tick();
    assert!(
        progress.dialog.is_visible(),
        "cancelling must leave the window that reports the outcome"
    );
}

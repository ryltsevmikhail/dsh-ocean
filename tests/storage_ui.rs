//! GTK acceptance for the read-only storage-usage window.
//!
//! One GTK test per test binary: gtk-rs allows a single initializing thread.
//! Uses a synthetic DSH home; no real session or storage data is read.
#![cfg(target_os = "linux")]
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use dsh_ocean::storage_ui as ui;
use gtk::prelude::*;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let serial = NEXT.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-storage-ui-{}-{serial}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sessions = root.join("dsh/sessions/a");
        let storages = root.join("dsh/storages/s");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(&storages).unwrap();
        fs::write(sessions.join("session.jsonl"), b"synthetic-session-secret").unwrap();
        fs::write(storages.join("store.bin"), b"store").unwrap();
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self
            .root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("dsh-ocean-storage-ui-"))
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
fn storage_window_reports_counts_read_only_and_closes() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let fixture = Fixture::new();
    // Point the read-only inventory at this synthetic home only.
    std::env::set_var("DSH_HOME", fixture.root.join("dsh"));

    let summary = ui::inventory();
    assert_eq!(summary.session_files, 1);
    assert_eq!(summary.session_bytes, 24);
    assert_eq!(summary.storage_files, 1);
    assert_eq!(summary.storage_bytes, 5);
    let body = ui::text(&summary);
    assert!(body.contains("Sessions: 1 file(s), 24 bytes"), "{body}");
    assert!(!body.contains("synthetic-session-secret"));
    assert!(!body.contains("session.jsonl"));

    let window = ui::build(None, &body);
    window.dialog.show_all();
    tick();
    assert_eq!(
        window.dialog.title().as_deref(),
        Some("DSH Ocean storage usage")
    );
    assert!(window.dialog.is_modal());
    assert!(window.label.is_selectable() && !window.label.uses_markup());
    for response in [
        gtk::ResponseType::Accept,
        gtk::ResponseType::Apply,
        gtk::ResponseType::Ok,
    ] {
        assert!(
            window.dialog.widget_for_response(response).is_none(),
            "storage window must not offer {response:?}"
        );
    }
    let close = window
        .dialog
        .widget_for_response(gtk::ResponseType::Close)
        .expect("close button")
        .downcast::<gtk::Button>()
        .unwrap();
    close.emit_clicked();
    tick();
    assert!(!window.dialog.is_visible());
    // Nothing was deleted by looking at the usage.
    assert!(fixture.root.join("dsh/sessions/a/session.jsonl").is_file());
    assert!(fixture.root.join("dsh/storages/s/store.bin").is_file());
}

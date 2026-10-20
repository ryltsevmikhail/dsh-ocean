//! Synthetic-folder GTK acceptance for the read-only backup history window.
//!
//! Never touches a real profile, backup root, service or desktop dialog.
#![cfg(target_os = "linux")]
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use dsh_ocean::{profile_backup::ProfileStore, profile_history_ui as ui};
use gtk::prelude::*;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let serial = NEXT.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-history-ui-{}-{serial}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("dsh");
        let profile = home.join("profiles/web");
        fs::create_dir_all(profile.join("resources")).unwrap();
        fs::write(
            profile.join("package.json"),
            "{\"dsh\":{\"profile\":{\"bundles\":[]}}}\n",
        )
        .unwrap();
        fs::write(
            profile.join("cordis.patch.yml"),
            "value: synthetic-secret\n",
        )
        .unwrap();
        fs::write(profile.join("resources/run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(
            profile.join("resources/run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        Self { root, home }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self
            .root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("dsh-ocean-history-ui-"))
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
fn history_window_lists_snapshots_read_only_and_closes() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let fixture = Fixture::new();
    // Point the read-only inventory at this private fixture only, then let the
    // real store create real snapshot directories where the window looks.
    std::env::set_var("DSH_HOME", &fixture.home);
    std::env::set_var("XDG_STATE_HOME", fixture.root.join("state"));
    let backups = dsh_ocean::config::profile_backup_dir();
    let store = ProfileStore::new(&fixture.home, &backups).unwrap();
    let mut snapshots = Vec::new();
    for _ in 0..3 {
        snapshots.push(store.create("web").unwrap());
        std::thread::sleep(std::time::Duration::from_millis(3));
    }

    let summaries = ui::inventory().unwrap();
    assert_eq!(summaries.len(), 3);
    let body = ui::text(&summaries);
    assert!(body.contains("3 configuration backup(s)"), "{body}");
    assert!(!body.contains("synthetic-secret"));

    let window = ui::build(None, &body);
    window.dialog.show_all();
    tick();
    assert!(window.label.is_selectable() && !window.label.uses_markup());
    assert!(window.dialog.is_modal());
    assert_eq!(window.dialog.title().as_deref(), Some(ui::TITLE));
    for response in [
        gtk::ResponseType::Accept,
        gtk::ResponseType::Apply,
        gtk::ResponseType::Ok,
    ] {
        assert!(
            window.dialog.widget_for_response(response).is_none(),
            "history window must not offer {response:?}"
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
    // Nothing was deleted, moved or modified by opening and closing the window.
    for snapshot in &snapshots {
        assert!(snapshot.is_dir());
    }
}

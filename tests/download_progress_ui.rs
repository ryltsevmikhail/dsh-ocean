//! GTK acceptance for aggregate download progress.
//!
//! One GTK test per binary: gtk-rs allows a single initializing thread, and the
//! other download-window acceptance test lives in `download_history.rs`.
use dsh_ocean::{
    download_history,
    downloads::{Entry, History, Status},
};
use gtk::{glib, prelude::*};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-progress-ui-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-progress-ui-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn entry(name: &str, status: Status) -> Entry {
    Entry::new(
        Some("http://user:synthetic-private@127.0.0.1:49001/path?token=synthetic-private".into()),
        name.into(),
        status,
    )
}
fn tick() {
    for _ in 0..32 {
        if !glib::MainContext::default().pending() {
            break;
        }
        glib::MainContext::default().iteration(false);
    }
}
fn wait(test: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !test() {
        tick();
        assert!(Instant::now() < end, "progress bar did not refresh");
        std::thread::sleep(Duration::from_millis(2));
    }
    tick();
}
fn click(dialog: &gtk::Dialog, response: gtk::ResponseType) {
    dialog
        .widget_for_response(response)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap()
        .emit_clicked();
    tick();
}

#[test]
#[ignore = "requires explicitly enabled isolated GTK dialog display"]
fn aggregate_progress_bar_is_determinate_only_with_complete_totals() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let f = Fixture::new();
    let path = f.0.join("progress.json");
    let mut measured = entry("measured-row", Status::Receiving);
    measured.received_bytes = 2_048;
    measured.reported_total = Some(8_192);
    let mut unmeasured = entry("unmeasured-row", Status::Receiving);
    unmeasured.received_bytes = 512;
    let records = Arc::new(Mutex::new(History::open(&path).unwrap()));
    {
        let mut history = records.lock().unwrap();
        history.put(measured.clone());
        history.put(unmeasured.clone());
    }
    let window = download_history::build_with_cancel(None, records.clone(), None);
    window.dialog.show_all();
    // One active row has no reported total, so the aggregate must stay a pulse.
    wait(|| window.progress.is_visible() && window.progress_text.text().contains("total unknown"));
    assert!(!window.progress_text.text().contains("synthetic-private"));
    {
        let mut history = records.lock().unwrap();
        let mut done = unmeasured.clone();
        done.status = Status::Saved;
        history.put(done);
    }
    wait(|| window.progress.fraction() > 0.24);
    assert!((window.progress.fraction() - 0.25).abs() < 0.001);
    assert!(window.progress_text.text().contains("2048 of 8192 bytes"));
    assert!(window.progress_text.text().contains("25%"));
    assert!(records.lock().unwrap().update_progress(&measured.id, 6_144));
    wait(|| window.progress.fraction() > 0.74);
    assert!(window.progress_text.text().contains("75%"));
    // Finishing the last transfer hides the live bar entirely.
    {
        let mut history = records.lock().unwrap();
        let mut done = measured.clone();
        done.status = Status::Saved;
        history.put(done);
    }
    wait(|| !window.progress.is_visible());
    click(&window.dialog, gtk::ResponseType::Close);
}

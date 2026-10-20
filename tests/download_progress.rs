//! Volatile counters only; no live network, user history or display.
use dsh_ocean::{
    download_history,
    downloads::{Entry, History, Status, MAX_BYTES},
};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-progress-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
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
        assert!(
            self.0.is_absolute()
                && self
                    .0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("dsh-ocean-progress-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn live_counters_are_monotonic_bounded_and_never_extend_disk_schema() {
    let f = Fixture::new();
    let path = f.0.join("history.json");
    let mut history = History::open(&path).unwrap();
    let mut entry = Entry::new(None, "fixture".into(), Status::Receiving);
    entry.reported_total = Some(128);
    let id = entry.id.clone();
    history.put(entry);
    history.persist().unwrap();
    let original = fs::read(&path).unwrap();
    let anchor = UNIX_EPOCH + std::time::Duration::from_secs(123);
    fs::File::open(&path).unwrap().set_modified(anchor).unwrap();
    assert!(history.update_progress(&id, 300));
    assert!(history.update_progress(&id, 2));
    assert_eq!(history.entries()[0].received_bytes, 300);
    assert!(history.update_progress(&id, u64::MAX));
    assert_eq!(history.entries()[0].received_bytes, MAX_BYTES + 1);
    assert_eq!(
        fs::read(&path).unwrap(),
        original,
        "progress never writes history"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().modified().unwrap(),
        anchor,
        "progress must not rewrite the history file"
    );
    history.persist().unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        original,
        "explicit persistence still excludes volatile fields"
    );
    let raw = String::from_utf8(original).unwrap();
    assert!(!raw.contains("receivedBytes") && !raw.contains("reportedTotal"));
    let reopened = History::open(&path).unwrap();
    assert_eq!(reopened.entries()[0].status, Status::Interrupted);
    assert_eq!(reopened.entries()[0].received_bytes, 0);
    assert_eq!(reopened.entries()[0].reported_total, None);
}
#[test]
fn late_progress_cannot_create_or_revive_a_terminal_row() {
    for status in [
        Status::Saved,
        Status::Cancelled,
        Status::Blocked,
        Status::Failed,
        Status::Interrupted,
    ] {
        let mut history = History::default();
        let mut entry = Entry::new(None, "finished".into(), status);
        entry.received_bytes = 8;
        let id = entry.id.clone();
        history.put(entry);
        assert!(!history.update_progress(&id, 999));
        assert!(!history.update_progress("unknown", 999));
        assert_eq!(history.entries().len(), 1);
        assert_eq!(history.entries()[0].status, status);
        assert_eq!(history.entries()[0].received_bytes, 8);
        history.clear_finished();
        assert!(!history.update_progress(&id, 999));
        assert!(history.entries().is_empty());
    }
}
#[test]
fn rendering_distinguishes_received_bytes_reported_total_and_saved_status() {
    let mut entry = Entry::new(None, "receiving".into(), Status::Receiving);
    entry.received_bytes = 150;
    entry.reported_total = Some(100);
    let text = download_history::render(&[entry.clone()]);
    assert!(
        text.contains("Received: 150 bytes")
            && text.contains("server reports 100 bytes total")
            && text.contains("not saved until Saved")
    );
    assert!(!text.contains('%') && !text.contains("Saved —"));
    entry.received_bytes = 0;
    entry.reported_total = None;
    let text = download_history::render(&[entry.clone()]);
    assert!(text.contains("Received: 0 bytes") && !text.contains("server reports"));
    entry.received_bytes = u64::MAX;
    entry.reported_total = Some(u64::MAX);
    let text = download_history::render(&[entry]);
    assert!(text.contains(&(MAX_BYTES + 1).to_string()) && !text.contains(&u64::MAX.to_string()));
}

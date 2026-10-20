//! Round-trip, trimming and privacy of the persisted download history.
#![cfg(target_os = "linux")]
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use dsh_ocean::downloads::{Entry, History, Status};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-history-file-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn path(&self) -> PathBuf {
        self.0.join("state/downloads.json")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with("dsh-ocean-history-file-")
        }) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn entry(name: &str, status: Status) -> Entry {
    Entry::new(
        Some("http://127.0.0.1:49999/file?token=synthetic-private".into()),
        name.into(),
        status,
    )
}

#[test]
fn entries_survive_a_round_trip_without_the_source_secrets() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let mut history = History::open(&path).expect("open a missing history");
    history.put(entry("first", Status::Saved));
    history.put(entry("second", Status::Failed));
    history.persist().expect("persist");

    let raw = fs::read_to_string(&path).unwrap();
    assert!(
        !raw.contains("synthetic-private"),
        "the query token must not be persisted: {raw}"
    );
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let directory = fs::metadata(path.parent().unwrap()).unwrap();
    assert_eq!(directory.permissions().mode() & 0o777, 0o700);

    let reopened = History::open(&path).expect("reopen");
    let names: Vec<String> = reopened
        .entries()
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"first".to_owned()) && names.contains(&"second".to_owned()));

    // A second process-style writer must not truncate the file or duplicate rows.
    let mut again = History::open(&path).unwrap();
    again.put(entry("third", Status::Saved));
    again.persist().unwrap();
    let count = History::open(&path).unwrap().entries().len();
    assert_eq!(count, 3, "the history grew by exactly one record");
}

#[test]
fn the_file_is_trimmed_to_the_record_cap_and_drops_finished_rows_first() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let mut history = History::open(&path).unwrap();
    // Two live transfers plus many finished rows: trimming removes finished rows,
    // never the receiving ones.
    history.put(entry("live-a", Status::Receiving));
    history.put(entry("live-b", Status::Receiving));
    for index in 0..150 {
        history.put(entry(&format!("done-{index}"), Status::Saved));
    }
    history.persist().unwrap();
    let reopened = History::open(&path).unwrap();
    assert!(
        reopened.entries().len() <= 100,
        "the persisted history is trimmed: {}",
        reopened.entries().len()
    );
    // An unfinished row is rewritten as `Interrupted` on load (by design); what
    // matters is that the live rows were never the ones trimmed.
    let names: Vec<_> = reopened
        .entries()
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    assert!(
        names.contains(&"live-a".to_owned()) && names.contains(&"live-b".to_owned()),
        "live rows must survive trimming: {names:?}"
    );
    assert!(
        names.contains(&"done-149".to_owned()),
        "the newest finished row must survive: {names:?}"
    );
}

#[test]
fn a_corrupt_file_fails_closed_and_is_never_overwritten_by_reading() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{not json").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    // A broken file fails closed: the caller falls back to memory-only, and the
    // existing file is left exactly as it was instead of being truncated.
    assert!(History::open(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"{not json");
}

#[test]
fn an_abandoned_temp_file_never_becomes_the_history() {
    let fixture = Fixture::new();
    let path = fixture.path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path.parent().unwrap().join(".downloads-1-2-3.tmp"),
        b"{\"version\":1,\"entries\":[]}",
    )
    .unwrap();
    let mut history = History::open(&path).expect("temp files are ignored");
    history.put(entry("only", Status::Saved));
    history.persist().unwrap();
    let reopened = History::open(&path).unwrap();
    assert_eq!(reopened.entries().len(), 1);
}

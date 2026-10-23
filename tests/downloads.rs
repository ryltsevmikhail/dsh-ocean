use dsh_ocean::downloads::{self, Entry, History, Status, Transfer, MAX_BYTES};
use dsh_ocean::navigation::{self, Decision};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
use url::Url;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-download-test-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
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
            .starts_with("dsh-ocean-download-test-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn url(raw: &str) -> Url {
    Url::parse(raw).unwrap()
}

#[test]
fn only_server_http_and_its_blob_origin_are_native_downloads() {
    let base = url("http://127.0.0.1:3080/");
    for raw in [
        "http://127.0.0.1:3080/file?token=synthetic-secret",
        "blob:http://127.0.0.1:3080/opaque-id",
    ] {
        assert_eq!(
            downloads::source_origin(&url(raw), &base).unwrap(),
            "http://127.0.0.1:3080"
        );
    }
    for raw in [
        "http://127.0.0.1:3081/file",
        "https://example.com/file",
        "http://user:password@127.0.0.1:3080/file",
        "blob:null/opaque",
        "data:text/plain,private",
        "file:///etc/passwd",
        "javascript:alert(1)",
    ] {
        assert!(
            downloads::source_origin(&url(raw), &base).is_err(),
            "accepted {raw}"
        );
    }
}

#[test]
fn navigation_fences_exact_origin_and_does_not_open_other_local_apps() {
    let base = url("http://127.0.0.1:3080/");
    assert_eq!(
        navigation::decide(&url("http://127.0.0.1:3080/?token=synthetic"), &base),
        Decision::Allow
    );
    assert_eq!(
        navigation::decide(&url("blob:http://127.0.0.1:3080/object"), &base),
        Decision::Allow
    );
    for raw in [
        "http://127.0.0.1:3081/",
        "http://localhost:3080/",
        "http://127.0.0.2:9000/",
        "http://[::1]:3080/",
        "http://[::ffff:127.0.0.1]:3080/",
        "http://0.0.0.0/",
        "http://2130706433:3081/",
        "http://host.localhost/",
        "blob:https://example.com/id",
        "data:text/html,private",
        "file:///tmp/a",
        "http://user:pass@127.0.0.1:3080/",
    ] {
        assert_eq!(
            navigation::decide(&url(raw), &base),
            Decision::Reject,
            "accepted {raw}"
        );
    }
    assert_eq!(
        navigation::decide(
            &url("https://github.com/deepseek-ai/deepseek-harness"),
            &base
        ),
        Decision::External
    );
    let remote = url("https://server.example:8443/");
    assert_eq!(
        navigation::decide(&url("https://server.example:8443/settings"), &remote),
        Decision::Allow
    );
    assert_eq!(
        navigation::decide(&url("http://server.example:8443/"), &remote),
        Decision::Reject
    );
    assert_eq!(
        navigation::decide(&url("https://server.example:8444/"), &remote),
        Decision::Reject
    );
}

#[test]
fn suggested_names_are_single_bounded_utf8_components() {
    for name in [
        "../unsafe\\name\n.txt",
        "../../.bashrc",
        "\0file",
        "🦀".repeat(200).as_str(),
    ] {
        let result = downloads::suggested_name(name);
        assert!(
            result.len() <= 180
                && !result.contains(['/', '\\'])
                && !result.chars().any(char::is_control)
        );
    }
    assert_eq!(downloads::suggested_name(".."), "download");
    assert_eq!(downloads::suggested_name(""), "download");
}

#[test]
fn publication_is_private_atomic_and_does_not_leave_partial_destination() {
    let f = Fixture::new();
    let target = f.0.join("chosen name.txt");
    let mut transfer = Transfer::prepare(&target).unwrap();
    let stage = transfer.payload.parent().unwrap().to_owned();
    fs::write(&transfer.payload, "fixture bytes\n").unwrap();
    assert!(!target.exists());
    assert_eq!(
        fs::metadata(&stage).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(transfer.publish().unwrap(), target);
    drop(transfer);
    assert_eq!(fs::read_to_string(&target).unwrap(), "fixture bytes\n");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!stage.exists());
}

#[test]
fn existing_or_racing_targets_are_never_overwritten() {
    let f = Fixture::new();
    let target = f.0.join("existing");
    fs::write(&target, "keep this").unwrap();
    assert!(Transfer::prepare(&target).is_err());
    symlink(f.0.join("missing"), f.0.join("link")).unwrap();
    assert!(Transfer::prepare(&f.0.join("link")).is_err());
    assert!(Transfer::prepare(&f.0).is_err());
    let racing = f.0.join("racing");
    let mut transfer = Transfer::prepare(&racing).unwrap();
    fs::write(&transfer.payload, "download").unwrap();
    fs::write(&racing, "concurrent writer").unwrap();
    assert!(transfer.publish().is_err());
    let recovery = transfer.recovery_path().unwrap();
    drop(transfer);
    assert_eq!(fs::read_to_string(&racing).unwrap(), "concurrent writer");
    assert_eq!(fs::read_to_string(recovery).unwrap(), "download");
}

#[test]
fn cancelled_payloads_are_removed_and_symlinks_are_never_published() {
    let f = Fixture::new();
    let target = f.0.join("cancelled");
    let transfer = Transfer::prepare(&target).unwrap();
    let stage = transfer.payload.parent().unwrap().to_owned();
    fs::write(&transfer.payload, "partial").unwrap();
    drop(transfer);
    assert!(!target.exists() && !stage.exists());
    let secret = f.0.join("do-not-touch");
    fs::write(&secret, "original").unwrap();
    let mut transfer = Transfer::prepare(&target).unwrap();
    symlink(&secret, &transfer.payload).unwrap();
    assert!(transfer.publish().is_err());
    assert!(transfer.recovery_path().is_none());
    drop(transfer);
    assert_eq!(fs::read_to_string(secret).unwrap(), "original");
    assert!(!target.exists());
}

#[test]
fn parent_replacement_and_oversized_payload_cannot_publish() {
    let f = Fixture::new();
    let parent = f.0.join("directory");
    fs::create_dir(&parent).unwrap();
    let mut transfer = Transfer::prepare(&parent.join("file")).unwrap();
    fs::write(&transfer.payload, "complete").unwrap();
    fs::rename(&parent, f.0.join("moved")).unwrap();
    fs::create_dir(&parent).unwrap();
    assert!(transfer.publish().is_err());
    let recovery = transfer.recovery_path().unwrap();
    drop(transfer);
    assert!(recovery.starts_with(f.0.join("moved")));
    assert_eq!(fs::read_to_string(recovery).unwrap(), "complete");
    assert!(!parent.join("file").exists());
    let target = f.0.join("huge");
    let mut transfer = Transfer::prepare(&target).unwrap();
    let file = fs::File::create(&transfer.payload).unwrap();
    file.set_len(MAX_BYTES + 1).unwrap();
    assert!(transfer.publish().is_err());
    assert!(transfer.recovery_path().is_none());
    drop(transfer);
    assert!(!target.exists());
}

#[test]
fn staging_replacement_cannot_publish_or_modify_the_replacement() {
    let f = Fixture::new();
    let target = f.0.join("result");
    let mut transfer = Transfer::prepare(&target).unwrap();
    let original_stage = transfer.payload.parent().unwrap().to_owned();
    assert!(Transfer::prepare(&original_stage.join("nested-target")).is_err());
    fs::write(&transfer.payload, "download bytes").unwrap();
    fs::rename(&original_stage, f.0.join("moved-stage")).unwrap();
    fs::create_dir(&original_stage).unwrap();
    fs::write(original_stage.join("payload"), "unrelated replacement").unwrap();
    fs::set_permissions(
        original_stage.join("payload"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(transfer.publish().is_err());
    let recovery = transfer.recovery_path().unwrap();
    drop(transfer);
    assert_eq!(fs::read_to_string(recovery).unwrap(), "download bytes");
    assert_eq!(
        fs::read_to_string(original_stage.join("payload")).unwrap(),
        "unrelated replacement"
    );
    assert_eq!(
        fs::metadata(original_stage.join("payload"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert!(!target.exists());
}

#[test]
fn history_is_private_bounded_and_does_not_persist_source_secrets() {
    let f = Fixture::new();
    let path = f.0.join("private/history.json");
    let mut history = History::open(&path).unwrap();
    for _ in 0..120 {
        history.put(Entry::new(
            Some(
                "http://user:password@127.0.0.1:3080/private?token=synthetic-secret#fragment"
                    .into(),
            ),
            "report.txt".into(),
            Status::Saved,
        ));
    }
    history.persist().unwrap();
    assert_eq!(history.entries().len(), 100);
    let raw = fs::read_to_string(&path).unwrap();
    for secret in ["password", "synthetic-secret", "private?", "fragment"] {
        assert!(!raw.contains(secret));
    }
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(History::open(&path).unwrap().entries().len(), 100);
}

#[test]
fn interrupted_history_and_clear_never_delete_downloaded_files() {
    let f = Fixture::new();
    let path = f.0.join("history.json");
    let output = f.0.join("user-file");
    fs::write(&output, "keep").unwrap();
    let mut history = History::open(&path).unwrap();
    let mut entry = Entry::new(
        Some("http://localhost:3000".into()),
        "user-file".into(),
        Status::Receiving,
    );
    entry.destination = Some(output.clone());
    history.put(entry);
    history.persist().unwrap();
    let mut restored = History::open(&path).unwrap();
    assert_eq!(restored.entries()[0].status, Status::Interrupted);
    restored.clear_finished();
    restored.persist().unwrap();
    assert!(restored.entries().is_empty());
    assert_eq!(fs::read_to_string(output).unwrap(), "keep");
}

#[test]
fn malformed_and_linked_history_fail_closed() {
    let f = Fixture::new();
    let path = f.0.join("bad");
    fs::write(&path, "not json").unwrap();
    assert!(History::open(&path).is_err());
    symlink(&path, f.0.join("alias")).unwrap();
    assert!(History::open(&f.0.join("alias")).is_err());
    fs::hard_link(&path, f.0.join("hardlink")).unwrap();
    assert!(History::open(&path).is_err());
}

//! Whole managed-home transaction snapshots; all credentials/data here synthetic.
use dsh_ocean::runtime_snapshot;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    home: PathBuf,
    backups: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ocean-runtime-snapshot-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            home: root.join("harness"),
            backups: root.join("backups"),
            root,
        }
    }
    fn write(&self, path: &str, body: &[u8]) {
        let path = self.home.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn restores_exact_managed_home_including_plugin_bytes_and_original_absence() {
    let f = Fixture::new();
    f.write("profiles/web/package.json", b"old-config");
    f.write("profiles/web/node_modules/plugin/index.js", b"old-plugin");
    f.write("sessions/chat.json", b"old-session");
    f.write("credentials.json", b"synthetic-old-secret");
    symlink(
        "plugin/index.js",
        f.home.join("profiles/web/node_modules/entry"),
    )
    .unwrap();
    let snapshot = runtime_snapshot::capture(&f.home, &f.backups, || Ok(())).unwrap();
    let path = snapshot.path().to_path_buf();
    f.write("profiles/web/package.json", b"new-config");
    f.write(
        "profiles/web/node_modules/plugin/index.js",
        b"new-abi-plugin",
    );
    f.write("storages/new.db", b"candidate-created-root");
    f.write("credentials.json", b"synthetic-new-secret");
    let result = snapshot.restore(|| Ok(())).unwrap();
    let retained = result
        .retained
        .expect("candidate state must be retained, not erased");
    assert_eq!(
        fs::read(f.home.join("profiles/web/package.json")).unwrap(),
        b"old-config"
    );
    assert_eq!(
        fs::read(f.home.join("profiles/web/node_modules/plugin/index.js")).unwrap(),
        b"old-plugin"
    );
    assert_eq!(
        fs::read(f.home.join("credentials.json")).unwrap(),
        b"synthetic-old-secret"
    );
    assert_eq!(
        fs::read_link(f.home.join("profiles/web/node_modules/entry")).unwrap(),
        PathBuf::from("plugin/index.js")
    );
    assert!(!f.home.join("storages").exists());
    assert_eq!(
        fs::read(retained.join("storages/new.db")).unwrap(),
        b"candidate-created-root"
    );
    let reopened = runtime_snapshot::open(&f.home, &path).unwrap();
    reopened.verify().unwrap();
}
#[test]
fn originally_absent_home_is_restored_as_absent_and_new_data_retained() {
    let f = Fixture::new();
    let snapshot = runtime_snapshot::capture(&f.home, &f.backups, || Ok(())).unwrap();
    assert!(!f.home.exists());
    f.write("sessions/created.json", b"candidate");
    f.write("profiles/web/package.json", b"candidate-profile");
    let result = snapshot.restore(|| Ok(())).unwrap();
    assert!(!f.home.exists());
    assert_eq!(
        fs::read(result.retained.unwrap().join("sessions/created.json")).unwrap(),
        b"candidate"
    );
}
#[test]
fn stopped_writer_guard_is_required_before_snapshot_publication_or_restore() {
    let f = Fixture::new();
    f.write("sessions/chat", b"before");
    assert!(
        runtime_snapshot::capture(&f.home, &f.backups, || anyhow::bail!(
            "writer still running"
        ))
        .is_err()
    );
    assert!(!f.backups.exists());
    let snapshot = runtime_snapshot::capture(&f.home, &f.backups, || Ok(())).unwrap();
    f.write("sessions/chat", b"after");
    assert!(snapshot
        .restore(|| anyhow::bail!("writer restarted"))
        .is_err());
    assert_eq!(fs::read(f.home.join("sessions/chat")).unwrap(), b"after");
}
#[test]
fn refuses_external_links_and_special_files_without_following_them() {
    for special in [false, true] {
        let f = Fixture::new();
        f.write("sessions/regular", b"safe");
        if special {
            let name =
                std::ffi::CString::new(f.home.join("fifo").as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        } else {
            symlink("../outside", f.home.join("escape")).unwrap();
            fs::write(f.root.join("outside"), b"must-not-copy").unwrap();
        }
        assert!(runtime_snapshot::capture(&f.home, &f.backups, || Ok(())).is_err());
        assert_eq!(fs::read(f.home.join("sessions/regular")).unwrap(), b"safe");
    }
}
#[test]
fn corrupt_snapshot_refuses_any_live_replacement() {
    let f = Fixture::new();
    f.write("sessions/chat", b"before");
    let snapshot = runtime_snapshot::capture(&f.home, &f.backups, || Ok(())).unwrap();
    fs::write(snapshot.path().join("data/sessions/chat"), b"tampered").unwrap();
    f.write("sessions/chat", b"candidate-state");
    assert!(snapshot.restore(|| Ok(())).is_err());
    assert_eq!(
        fs::read(f.home.join("sessions/chat")).unwrap(),
        b"candidate-state"
    );
}

#[test]
fn transaction_snapshot_refuses_other_writable_source_state() {
    for file in [false, true] {
        let f = Fixture::new();
        f.write("sessions/chat", b"private");
        fs::set_permissions(
            f.home.join(if file { "sessions/chat" } else { "sessions" }),
            fs::Permissions::from_mode(if file { 0o666 } else { 0o777 }),
        )
        .unwrap();
        assert!(
            runtime_snapshot::capture(&f.home, &f.backups, || Ok(())).is_err(),
            "other-writable source state cannot be treated as quiescent controlled data"
        );
    }
}

#[test]
fn reopened_transaction_snapshots_must_remain_private() {
    let f = Fixture::new();
    f.write("credentials.json", b"synthetic-only-secret");
    let snapshot = runtime_snapshot::capture(&f.home, &f.backups, || Ok(())).unwrap();
    fs::set_permissions(snapshot.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        runtime_snapshot::open(&f.home, snapshot.path()).is_err(),
        "sensitive runtime backup must not be accepted after its private directory mode changed"
    );
}

#[test]
fn concurrent_creation_cannot_be_published_as_an_absent_home_snapshot() {
    let f = Fixture::new();
    let mut checkpoints = 0;
    let result = runtime_snapshot::capture(&f.home, &f.backups, || {
        checkpoints += 1;
        if checkpoints == 2 {
            f.write("sessions/other-writer", b"must-not-ignore");
        }
        Ok(())
    });
    assert!(
        result.is_err(),
        "an absent-root record must be revalidated after the final stopped-writer checkpoint"
    );
    assert_eq!(
        fs::read(f.home.join("sessions/other-writer")).unwrap(),
        b"must-not-ignore"
    );
}

#[test]
fn failed_offline_check_during_restore_never_exchanges_live_state() {
    let f = Fixture::new();
    f.write("sessions/chat", b"old");
    let snapshot = runtime_snapshot::capture(&f.home, &f.backups, || Ok(())).unwrap();
    f.write("sessions/chat", b"candidate");
    let mut checkpoints = 0;
    let result = snapshot.restore(|| {
        checkpoints += 1;
        if checkpoints >= 3 {
            anyhow::bail!("owned writer restarted");
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        fs::read(f.home.join("sessions/chat")).unwrap(),
        b"candidate"
    );
    assert!(
        !fs::read_dir(&f.root).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".ocean-runtime-restore-")),
        "failed precommit staging must be cleaned"
    );
}

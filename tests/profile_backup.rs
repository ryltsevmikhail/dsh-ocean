#![cfg(target_os = "linux")]
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use dsh_ocean::profile_backup::{self, ProfileStore};

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// These tests exercise flock- and rename-heavy restore paths. Serializing the
// fixtures keeps the failure diagnostics about the code under test instead of
/// about how many sibling tests happened to run at the same moment; each test
/// still builds its own isolated directory tree.
static FIXTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture {
    _serial: std::sync::MutexGuard<'static, ()>,
    root: PathBuf,
    home: PathBuf,
    profile: PathBuf,
    backups: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        // Parallel tests must never share a fixture, so combine the clock with a
        // process-wide serial: coarse clocks can return identical nanoseconds.
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _serial = FIXTURE_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-backup-{}-{serial}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("dsh home");
        let profile = home.join("profiles/web");
        let backups = root.join("private backups");
        for dir in [
            profile.join("resources/empty"),
            profile.join("node_modules/pkg"),
            profile.join(".git"),
            home.join("sessions"),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(
            profile.join("package.json"),
            "{\"dsh\":{\"profile\":{\"bundles\":[]}}}\n",
        )
        .unwrap();
        fs::write(profile.join("cordis.patch.yml"), "value: before\n").unwrap();
        fs::write(profile.join("resources/script.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(
            profile.join("resources/script.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::write(
            profile.join("node_modules/pkg/index.js"),
            "external dependency\n",
        )
        .unwrap();
        fs::write(profile.join(".git/HEAD"), "synthetic git cache\n").unwrap();
        fs::write(
            home.join(".credentials.yaml"),
            "synthetic-global-credential-must-not-be-copied\n",
        )
        .unwrap();
        fs::write(
            home.join("sessions/test.json"),
            "synthetic-session-must-not-be-rolled-back\n",
        )
        .unwrap();
        Self {
            _serial,
            root,
            home,
            profile,
            backups,
        }
    }
    fn store(&self) -> ProfileStore {
        ProfileStore::new(&self.home, &self.backups).unwrap()
    }
    fn snapshot(&self) -> PathBuf {
        self.store().create_recording("web", None).unwrap()
    }
    fn assert_clean_staging(&self) {
        for parent in [&self.backups, &self.home.join("profiles")] {
            if parent.exists() {
                assert!(!fs::read_dir(parent).unwrap().any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".ocean-stage-")));
            }
        }
    }
    fn assert_global_unchanged(&self) {
        assert_eq!(
            fs::read_to_string(self.home.join(".credentials.yaml")).unwrap(),
            "synthetic-global-credential-must-not-be-copied\n"
        );
        assert_eq!(
            fs::read_to_string(self.home.join("sessions/test.json")).unwrap(),
            "synthetic-session-must-not-be-rolled-back\n"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-backup-"));
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn snapshot_is_private_bounded_and_explicitly_profile_only() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let manifest = profile_backup::verify(&snapshot).unwrap();
    assert_eq!(manifest.scope, "profile-config");
    assert_eq!(manifest.consistency, "best-effort");
    assert_eq!(manifest.files.len(), 3);
    assert!(manifest.directories.contains(&"resources/empty".to_owned()));
    assert!(!snapshot.join("data/node_modules").exists());
    assert!(!snapshot.join("data/.git").exists());
    assert_eq!(
        fs::metadata(&snapshot).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for file in manifest.files {
        assert_eq!(file.sha256.len(), 64);
        assert_eq!(
            fs::metadata(snapshot.join("data").join(&file.path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let bytes = fs::read(snapshot.join("data").join(file.path)).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-global-credential"));
    }
    f.assert_global_unchanged();
    f.assert_clean_staging();
}

#[test]
fn offline_restore_atomically_retains_the_whole_replaced_profile() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    fs::write(f.profile.join("cordis.patch.yml"), "value: after\n").unwrap();
    fs::write(f.profile.join("new-file"), "new content\n").unwrap();
    let mut checks = 0;
    let receipt = f
        .store()
        .restore("web", &snapshot, false, None, || {
            checks += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(checks, 2);
    assert_eq!(
        fs::read_to_string(f.profile.join("cordis.patch.yml")).unwrap(),
        "value: before\n"
    );
    assert!(!f.profile.join("node_modules").exists());
    assert!(!f.profile.join(".git").exists());
    assert_eq!(
        fs::metadata(f.profile.join("resources/script.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let retained = receipt.retained_profile.unwrap();
    assert_eq!(
        fs::metadata(&retained).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::read_to_string(retained.join("cordis.patch.yml")).unwrap(),
        "value: after\n"
    );
    assert!(retained.join("new-file").is_file());
    assert!(retained.join("node_modules/pkg/index.js").is_file());
    assert!(retained.join(".git/HEAD").is_file());
    profile_backup::verify(&snapshot).unwrap();
    f.assert_global_unchanged();
    f.assert_clean_staging();
}

#[test]
fn can_restore_an_absent_profile_without_overwriting_anything() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    assert!(f.profile.starts_with(&f.root));
    fs::remove_dir_all(&f.profile).unwrap();
    let receipt = f
        .store()
        .restore("web", &snapshot, false, None, || Ok(()))
        .unwrap();
    assert!(receipt.retained_profile.is_none());
    assert!(f.profile.join("package.json").is_file());
    f.assert_global_unchanged();
    f.assert_clean_staging();
}

#[test]
fn corruption_missing_and_extra_data_are_rejected_without_target_changes() {
    for kind in ["corrupt", "missing", "extra", "symlink", "manifest-symlink"] {
        let f = Fixture::new();
        let snapshot = f.snapshot();
        let patch = snapshot.join("data/cordis.patch.yml");
        match kind {
            "corrupt" => fs::write(&patch, "value: tampered\n").unwrap(),
            "missing" => fs::remove_file(&patch).unwrap(),
            "extra" => fs::write(snapshot.join("data/unlisted"), "extra").unwrap(),
            "symlink" => {
                fs::remove_file(&patch).unwrap();
                symlink(f.home.join(".credentials.yaml"), &patch).unwrap();
            }
            _ => {
                fs::remove_file(snapshot.join("manifest.json")).unwrap();
                symlink(
                    f.home.join(".credentials.yaml"),
                    snapshot.join("manifest.json"),
                )
                .unwrap();
            }
        }
        assert!(
            profile_backup::verify(&snapshot).is_err(),
            "accepted {kind}"
        );
        assert!(f
            .store()
            .restore("web", &snapshot, false, None, || Ok(()))
            .is_err());
        assert_eq!(
            fs::read_to_string(f.profile.join("cordis.patch.yml")).unwrap(),
            "value: before\n"
        );
        f.assert_global_unchanged();
        f.assert_clean_staging();
    }
}

#[test]
fn manifest_paths_schema_and_profile_cannot_redirect_restore() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let original = fs::read(snapshot.join("manifest.json")).unwrap();
    for path in [
        "../outside",
        "/absolute",
        "resources/../outside",
        "a//b",
        "node_modules/x",
        "back\\slash",
        "line\nbreak",
        "",
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        value["files"][0]["path"] = path.into();
        fs::write(snapshot.join("manifest.json"), value.to_string()).unwrap();
        assert!(
            f.store()
                .restore("web", &snapshot, false, None, || Ok(()))
                .is_err(),
            "accepted {path:?}"
        );
    }
    for (field, changed) in [
        ("formatVersion", serde_json::json!(2)),
        ("profile", serde_json::json!("other")),
        ("scope", serde_json::json!("entire-home")),
        ("unexpected", serde_json::json!(true)),
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        value[field] = changed;
        fs::write(snapshot.join("manifest.json"), value.to_string()).unwrap();
        assert!(
            f.store()
                .restore("web", &snapshot, false, None, || Ok(()))
                .is_err(),
            "accepted {field}"
        );
    }
    assert_eq!(
        fs::read_to_string(f.profile.join("cordis.patch.yml")).unwrap(),
        "value: before\n"
    );
    assert!(!f.home.join("profiles/other").exists());
    f.assert_global_unchanged();
    f.assert_clean_staging();
}

#[test]
fn malformed_hash_duplicates_and_parent_collisions_are_rejected() {
    for kind in [
        "hash",
        "duplicate",
        "collision",
        "missing-parent",
        "too-many",
    ] {
        let f = Fixture::new();
        let snapshot = f.snapshot();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(snapshot.join("manifest.json")).unwrap()).unwrap();
        match kind {
            "hash" => value["files"][0]["sha256"] = "g".repeat(64).into(),
            "duplicate" => {
                let entry = value["files"][0].clone();
                value["files"].as_array_mut().unwrap().push(entry);
            }
            "collision" => value["directories"]
                .as_array_mut()
                .unwrap()
                .push("package.json".into()),
            "missing-parent" => value["directories"] = serde_json::json!([]),
            _ => value["files"] = serde_json::json!(vec![value["files"][0].clone(); 10001]),
        }
        fs::write(snapshot.join("manifest.json"), value.to_string()).unwrap();
        assert!(
            profile_backup::verify(&snapshot).is_err(),
            "accepted {kind}"
        );
    }
}

#[test]
fn source_symlinks_hardlinks_and_special_files_are_never_followed() {
    for kind in ["symlink", "directory-symlink", "hardlink", "fifo"] {
        let f = Fixture::new();
        let entry = f.profile.join("unsafe");
        match kind {
            "symlink" => symlink(f.home.join(".credentials.yaml"), &entry).unwrap(),
            "directory-symlink" => symlink(f.home.join("sessions"), &entry).unwrap(),
            "hardlink" => fs::hard_link(f.home.join(".credentials.yaml"), &entry).unwrap(),
            _ => {
                use std::os::unix::ffi::OsStrExt;
                let name = std::ffi::CString::new(entry.as_os_str().as_bytes()).unwrap();
                // SAFETY: one controlled fixture path and valid mode.
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
        }
        assert!(
            f.store().create_recording("web", None).is_err(),
            "accepted {kind}"
        );
        f.assert_clean_staging();
        f.assert_global_unchanged();
    }
}

#[test]
fn excluded_package_cache_may_be_a_symlink_but_is_not_copied() {
    let f = Fixture::new();
    fs::remove_dir_all(f.profile.join("node_modules")).unwrap();
    symlink(f.home.join("sessions"), f.profile.join("node_modules")).unwrap();
    let snapshot = f.snapshot();
    assert!(!snapshot.join("data/node_modules").exists());
    profile_backup::verify(&snapshot).unwrap();
}

#[test]
fn source_profile_or_parent_symlinks_are_refused() {
    for parent in [false, true] {
        let f = Fixture::new();
        let original = if parent {
            f.home.join("profiles")
        } else {
            f.profile.clone()
        };
        let moved = f.root.join("moved");
        fs::rename(&original, &moved).unwrap();
        symlink(&moved, &original).unwrap();
        assert!(f.store().create_recording("web", None).is_err());
        f.assert_global_unchanged();
    }
}

#[test]
fn refusing_offline_guard_or_detecting_target_changes_leaves_data_intact() {
    for mode in [
        "first-guard",
        "second-guard",
        "file-change",
        "directory-change",
    ] {
        let f = Fixture::new();
        let snapshot = f.snapshot();
        let mut count = 0;
        let result = f.store().restore("web", &snapshot, false, None, || {
            count += 1;
            match (mode, count) {
                ("first-guard", 1) | ("second-guard", 2) => anyhow::bail!("service still active"),
                ("file-change", 2) => {
                    fs::write(f.profile.join("cordis.patch.yml"), "concurrent edit\n")?
                }
                ("directory-change", 2) => {
                    fs::rename(&f.profile, f.root.join("concurrent-profile"))?;
                    fs::create_dir(&f.profile)?;
                    fs::write(f.profile.join("marker"), "new directory")?;
                }
                _ => {}
            }
            Ok(())
        });
        assert!(result.is_err(), "accepted {mode}");
        if mode == "file-change" {
            assert_eq!(
                fs::read_to_string(f.profile.join("cordis.patch.yml")).unwrap(),
                "concurrent edit\n"
            );
        }
        if mode == "directory-change" {
            assert!(f.profile.join("marker").is_file());
            assert!(f.root.join("concurrent-profile/package.json").is_file());
        }
        f.assert_clean_staging();
        f.assert_global_unchanged();
    }
}

#[test]
fn snapshot_root_symlinks_and_overlapping_destinations_are_refused() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    symlink(&snapshot, f.root.join("alias")).unwrap();
    assert!(profile_backup::verify(&f.root.join("alias")).is_err());
    let inside = f.profile.join("must-not-create");
    assert!(ProfileStore::new(&f.home, &inside).is_err());
    assert!(!inside.exists());
    assert!(ProfileStore::new(&f.home, &f.root).is_err());
}

#[test]
fn directory_enumeration_is_bounded_before_collecting_the_inventory() {
    let f = Fixture::new();
    let crowded = f.profile.join("crowded");
    fs::create_dir(&crowded).unwrap();
    for index in 0..10001 {
        fs::write(crowded.join(format!("file-{index}")), "").unwrap();
    }
    let error = f.store().create_recording("web", None).unwrap_err();
    assert!(format!("{error:#}").contains("directory entry limit"));
    f.assert_clean_staging();
}

#[test]
fn file_limits_and_invalid_names_fail_without_partial_publications() {
    let f = Fixture::new();
    let huge = fs::File::create(f.profile.join("huge")).unwrap();
    huge.set_len(16 * 1024 * 1024 + 1).unwrap();
    assert!(f.store().create_recording("web", None).is_err());
    for name in ["..", "../web", "/absolute", ".hidden", "bad/name", "a\nb"] {
        assert!(f.store().create(name).is_err());
    }
    f.assert_clean_staging();
}

#[test]
fn profile_operations_serialize_and_do_not_follow_lock_symlinks() {
    let f = Fixture::new();
    let lock = f.home.join("profiles/.dsh-ocean-backup.lock");
    let held = profile_backup::operation_lock(&lock).unwrap();
    assert!(f.store().create_recording("web", None).is_err());
    drop(held);
    f.snapshot();
    fs::remove_file(&lock).unwrap();
    symlink(f.home.join(".credentials.yaml"), &lock).unwrap();
    assert!(f.store().create_recording("web", None).is_err());
    f.assert_global_unchanged();
}

#[test]
fn inventory_lists_snapshots_without_hashing_or_exposing_paths() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let mut names = Vec::new();
    for _ in 0..3 {
        names.push(store.create_recording("web", None).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    // Unrelated entries must never be treated as snapshots or followed.
    fs::write(fixture.backups.join("operator-note.txt"), b"keep").unwrap();
    fs::create_dir_all(fixture.backups.join("unrelated-dir")).unwrap();
    symlink("/etc/passwd", fixture.backups.join("profile-web-1-1")).unwrap();
    let listed = profile_backup::snapshots(&fixture.backups).unwrap();
    assert_eq!(listed.len(), 3);
    let listed_names: Vec<_> = listed.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(
        listed_names,
        names
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect::<Vec<_>>()
    );
    for entry in &listed {
        assert_eq!(entry.profile, "web");
        assert!(
            entry.created_at.is_some(),
            "valid timestamps are normalized"
        );
        assert!(entry.files > 0 && entry.bytes > 0 && entry.directories > 0);
    }
    // Ordering places the oldest snapshot first (names carry the sequence).
    assert!(listed.windows(2).all(|pair| pair[0].name < pair[1].name));
    // Listing is read-only.
    for name in &names {
        assert!(name.is_dir());
    }
    fixture.assert_clean_staging();
}

#[test]
fn prune_keeps_the_newest_snapshots_per_profile_and_leaves_other_data_alone() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let mut web = Vec::new();
    for _ in 0..4 {
        web.push(store.create_recording("web", None).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    fs::create_dir_all(fixture.home.join("profiles/tui")).unwrap();
    fs::write(
        fixture.home.join("profiles/tui/package.json"),
        "{\"dsh\":{\"profile\":{\"bundles\":[]}}}\n",
    )
    .unwrap();
    let tui = store.create_recording("tui", None).unwrap();
    let note = fixture.backups.join("operator-note.txt");
    fs::write(&note, b"keep me").unwrap();
    let outcome = profile_backup::prune(&fixture.backups, 2, Some("web"), || Ok(())).unwrap();
    assert_eq!(outcome.removed.len(), 2);
    assert_eq!(outcome.kept.len(), 2);
    assert!(outcome.unreadable.is_empty());
    for (index, snapshot) in web.iter().enumerate() {
        assert_eq!(
            snapshot.is_dir(),
            index >= 2,
            "only the two newest web snapshots may survive"
        );
    }
    assert!(tui.is_dir(), "other profiles are untouched");
    assert_eq!(fs::read(&note).unwrap(), b"keep me");
    fixture.assert_clean_staging();
    // Web files were removed but the profile itself is untouched.
    assert_eq!(
        fs::read_to_string(fixture.profile.join("cordis.patch.yml")).unwrap(),
        "value: before\n"
    );
}

#[test]
fn prune_refuses_unsafe_requests_and_never_follows_links() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for _ in 0..2 {
        store.create_recording("web", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(profile_backup::prune(&fixture.backups, 0, None, || Ok(())).is_err());
    assert!(profile_backup::prune(&fixture.backups, 1001, None, || Ok(())).is_err());
    assert!(profile_backup::prune(&fixture.backups, 1, Some("bad/name"), || Ok(())).is_err());
    // A snapshot-shaped symlink is ignored for listing/selection and its target
    // is never deleted.
    let outside = fixture.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep"), b"outside data").unwrap();
    symlink(&outside, fixture.backups.join("profile-web-1-2")).unwrap();
    let outcome = profile_backup::prune(&fixture.backups, 1, Some("web"), || Ok(())).unwrap();
    assert_eq!(outcome.removed.len(), 1);
    assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside data");
    assert!(fixture.backups.join("profile-web-1-2").is_symlink());
    // A callback error aborts without deleting anything else.
    for _ in 0..2 {
        store.create_recording("web", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let before: Vec<_> = profile_backup::snapshots(&fixture.backups)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    let mut calls = 0;
    let aborted = profile_backup::prune(&fixture.backups, 1, Some("web"), || {
        calls += 1;
        anyhow::ensure!(calls < 3, "synthetic cancellation");
        Ok(())
    });
    assert!(aborted.is_err());
    let after: Vec<_> = profile_backup::snapshots(&fixture.backups)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(after, before);
}

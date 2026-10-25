//! Whole-data (sessions + shared stores) snapshot acceptance.
//!
//! Synthetic DSH home only: no real sessions, stores, service or network.
#![cfg(target_os = "linux")]
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    backups: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self::with_stamp(nanos)
    }

    fn with_stamp(nanos: u128) -> Self {
        let root = loop {
            let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let candidate = std::env::temp_dir().join(format!(
                "dsh-ocean-data-backup-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("reserve synthetic fixture directory: {error}"),
            }
        };
        let home = root.join("dsh");
        let backups = root.join("backups dir");
        let sessions = home.join("sessions/2026-01-01");
        let storages = home.join("storages/cost-meter");
        for directory in [
            &sessions,
            &storages,
            &backups,
            &root.join("home"),
            &root.join("bin"),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(
            sessions.join("session.jsonl"),
            b"{\"message\":\"synthetic\"}\n",
        )
        .unwrap();
        fs::write(storages.join("ledger.json"), b"{\"cost\":1}\n").unwrap();
        // Credentials at the home root must never be copied by a data snapshot.
        fs::write(home.join(".credentials.yaml"), b"global-secret\n").unwrap();
        // Symlinks and special files are skipped, never followed.
        let outside = root.join("outside-secret");
        fs::write(&outside, b"outside\n").unwrap();
        symlink(&outside, home.join("sessions/link.json")).unwrap();
        fs::set_permissions(root.join("home"), fs::Permissions::from_mode(0o700)).unwrap();
        // Offline guard: an inactive unit and an unroutable origin, so restore
        // tests never depend on (or talk to) the real service.
        let sysctl = root.join("bin/systemctl");
        fs::write(&sysctl, "#!/bin/bash\nexit 3\n").unwrap();
        fs::set_permissions(&sysctl, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            root,
            home,
            backups,
        }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(arguments)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("DSH_HOME", &self.home)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("DSH_OCEAN_DATA_DIR", self.root.join("runtime"))
            // Only the read-only systemctl stub: no runtime, no real manager.
            .env("PATH", self.root.join("bin"))
            .env("DSH_OCEAN_URL", "http://127.0.0.1:9/")
            .output()
            .expect("run the client CLI")
    }
}

#[test]
fn fixtures_do_not_share_data_when_the_clock_repeats() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let first = Fixture::with_stamp(stamp);
    let marker = first.root.join("synthetic-owner-marker");
    fs::write(&marker, b"first fixture").unwrap();
    let second = Fixture::with_stamp(stamp);
    assert_ne!(first.root, second.root);
    assert_eq!(fs::read(marker).unwrap(), b"first fixture");
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self
            .root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("dsh-ocean-data-backup-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn describe(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn snapshot_path(output: &Output) -> PathBuf {
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    PathBuf::from(json["snapshot"].as_str().unwrap())
}

#[test]
fn data_snapshot_copies_only_sessions_and_storages_and_verifies() {
    let fixture = Fixture::new();
    let output = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(output.status.success(), "{}", describe(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["scope"], "dsh-data");
    assert_eq!(json["roots"], serde_json::json!(["sessions", "storages"]));
    assert_eq!(json["fileCount"], 2);
    let snapshot = snapshot_path(&output);
    assert!(snapshot.is_dir());
    assert_eq!(
        fs::read(snapshot.join("data/sessions/2026-01-01/session.jsonl")).unwrap(),
        b"{\"message\":\"synthetic\"}\n"
    );
    assert!(snapshot
        .join("data/storages/cost-meter/ledger.json")
        .is_file());
    // Credentials and symlinks are not part of the snapshot.
    assert!(!snapshot.join("data/.credentials.yaml").exists());
    assert!(!snapshot.join("data/sessions/link.json").exists());

    let verified = fixture.run(&["--verify-data-backup", snapshot.to_str().unwrap()]);
    assert!(verified.status.success(), "{}", describe(&verified));
    let json: serde_json::Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(json["fileCount"], 2);
    assert_eq!(json["roots"], serde_json::json!(["sessions", "storages"]));

    // Tampering is detected instead of being reported as intact.
    fs::write(
        snapshot.join("data/sessions/2026-01-01/session.jsonl"),
        b"tampered",
    )
    .unwrap();
    let refused = fixture.run(&["--verify-data-backup", snapshot.to_str().unwrap()]);
    assert!(!refused.status.success());
    assert!(
        describe(&refused).contains("integrity"),
        "{}",
        describe(&refused)
    );
}

#[test]
fn missing_roots_and_tampered_manifests_are_refused() {
    let fixture = Fixture::new();
    fs::remove_dir_all(fixture.home.join("sessions")).unwrap();
    fs::remove_dir_all(fixture.home.join("storages")).unwrap();
    let empty = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(!empty.status.success());
    assert!(
        describe(&empty).contains("no session or storage data"),
        "{}",
        describe(&empty)
    );

    // Only sessions exist: the snapshot records the roots it actually copied.
    let sessions = fixture.home.join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::write(sessions.join("one.jsonl"), b"x").unwrap();
    let partial = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(partial.status.success(), "{}", describe(&partial));
    let json: serde_json::Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(json["roots"], serde_json::json!(["sessions"]));

    // A manifest whose scope was rewritten is refused.
    let snapshot = snapshot_path(&partial);
    let manifest_path = snapshot.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["scope"] = "profile-config".into();
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let refused = fixture.run(&["--verify-data-backup", snapshot.to_str().unwrap()]);
    assert!(!refused.status.success());
    assert!(
        describe(&refused).contains("format/scope"),
        "{}",
        describe(&refused)
    );
}

#[test]
fn retention_keeps_the_newest_and_refuses_without_confirmation() {
    let fixture = Fixture::new();
    let mut snapshots = Vec::new();
    for _ in 0..3 {
        let output = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
        assert!(output.status.success(), "{}", describe(&output));
        snapshots.push(snapshot_path(&output));
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    let unconfirmed = fixture.run(&["--prune-data-backups", "--keep", "1"]);
    assert!(!unconfirmed.status.success());
    for snapshot in &snapshots {
        assert!(snapshot.is_dir());
    }
    let pruned = fixture.run(&[
        "--prune-data-backups",
        "--keep",
        "1",
        "--confirm-prune",
        fixture.backups.to_str().unwrap(),
    ]);
    assert!(pruned.status.success(), "{}", describe(&pruned));
    let json: serde_json::Value = serde_json::from_slice(&pruned.stdout).unwrap();
    assert_eq!(json["removed"].as_array().unwrap().len(), 2);
    assert!(snapshots[2].is_dir());
    assert!(!snapshots[0].exists() && !snapshots[1].exists());
    // The source data is untouched by retention.
    assert!(Path::new(&fixture.home.join("sessions/2026-01-01/session.jsonl")).is_file());
}

#[test]
fn restore_replaces_roots_retains_previous_and_refuses_without_offline_confirmation() {
    let fixture = Fixture::new();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(backup.status.success(), "{}", describe(&backup));
    let snapshot = snapshot_path(&backup);
    // Change the live data so a successful restore is observable.
    fs::write(
        fixture.home.join("sessions/2026-01-01/session.jsonl"),
        b"changed after snapshot",
    )
    .unwrap();
    fs::write(
        fixture.home.join("storages/cost-meter/ledger.json"),
        b"changed",
    )
    .unwrap();

    // Without the explicit offline assertion nothing is touched.
    let refused = fixture.run(&["--restore-data", snapshot.to_str().unwrap()]);
    assert!(!refused.status.success());
    assert!(
        describe(&refused).contains("--confirm-offline"),
        "{}",
        describe(&refused)
    );
    assert_eq!(
        fs::read(fixture.home.join("sessions/2026-01-01/session.jsonl")).unwrap(),
        b"changed after snapshot"
    );

    // The service is stopped in this sandbox (`systemctl` exits 3), so the
    // explicit confirmation performs the restore.
    let restored = fixture.run(&[
        "--restore-data",
        snapshot.to_str().unwrap(),
        "--confirm-offline",
    ]);
    assert!(restored.status.success(), "{}", describe(&restored));
    let json: serde_json::Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(
        json["restored"],
        serde_json::json!(["sessions", "storages"])
    );
    assert_eq!(json["untouched"], serde_json::json!([]));
    assert_eq!(json["retained"].as_array().unwrap().len(), 2);
    assert_eq!(
        fs::read(fixture.home.join("sessions/2026-01-01/session.jsonl")).unwrap(),
        b"{\"message\":\"synthetic\"}\n"
    );
    assert!(fixture
        .home
        .join("storages/cost-meter/ledger.json")
        .is_file());
    // Previous roots are retained next to the live ones, not deleted.
    for retained in json["retained"].as_array().unwrap() {
        let path = PathBuf::from(retained.as_str().unwrap());
        assert!(path.is_dir(), "retained root missing: {}", path.display());
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(".ocean-retained-"));
    }
    // Credentials at the home root are untouched by a data restore.
    assert_eq!(
        fs::read(fixture.home.join(".credentials.yaml")).unwrap(),
        b"global-secret\n"
    );
}

#[test]
fn restore_refuses_a_tampered_snapshot_and_leaves_live_data_alone() {
    let fixture = Fixture::new();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    let snapshot = snapshot_path(&backup);
    fs::write(
        snapshot.join("data/sessions/2026-01-01/session.jsonl"),
        b"tampered",
    )
    .unwrap();
    let before = fs::read(fixture.home.join("sessions/2026-01-01/session.jsonl")).unwrap();
    let refused = fixture.run(&[
        "--restore-data",
        snapshot.to_str().unwrap(),
        "--confirm-offline",
    ]);
    assert!(!refused.status.success());
    assert!(
        describe(&refused).contains("integrity"),
        "{}",
        describe(&refused)
    );
    assert_eq!(
        fs::read(fixture.home.join("sessions/2026-01-01/session.jsonl")).unwrap(),
        before
    );
}

#[test]
fn listing_reports_metadata_only_and_never_reads_snapshot_contents() {
    let fixture = Fixture::new();
    let first = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(first.status.success(), "{}", describe(&first));
    std::thread::sleep(std::time::Duration::from_millis(3));
    let second = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(second.status.success(), "{}", describe(&second));

    let listed = fixture.run(&["--list-data-backups", fixture.backups.to_str().unwrap()]);
    assert!(listed.status.success(), "{}", describe(&listed));
    let json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(json["count"], 2);
    let entries = json["backups"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert!(entry["name"].as_str().unwrap().starts_with("data-"));
        assert!(entry["files"].as_u64().unwrap() >= 2);
        assert!(entry["bytes"].as_u64().unwrap() > 0);
        assert!(entry["createdAt"].is_string());
    }
    // Metadata only: no snapshot paths or synthetic contents leak into output.
    let rendered = describe(&listed);
    assert!(!rendered.contains("session.jsonl"));
    assert!(!rendered.contains("synthetic"), "{rendered}");
    assert!(!rendered.contains("ledger.json"));

    // A directory that is not a backup root lists as empty rather than failing.
    let empty = fixture.run(&[
        "--list-data-backups",
        fixture.root.join("absent").to_str().unwrap(),
    ]);
    assert!(empty.status.success(), "{}", describe(&empty));
    let json: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(json["count"], 0);
}

#[test]
fn a_manifest_that_points_outside_the_roots_is_refused_before_touching_data() {
    let fixture = Fixture::new();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(backup.status.success(), "{}", describe(&backup));
    let snapshot = snapshot_path(&backup);
    let manifest_path = snapshot.join("manifest.json");
    let original = fs::read(&manifest_path).unwrap();
    let before = fs::read(fixture.home.join("sessions/2026-01-01/session.jsonl")).unwrap();

    for tampered_path in ["sessions/../../outside", "/etc/passwd", "storages/.."] {
        let mut manifest: serde_json::Value = serde_json::from_slice(&original).unwrap();
        manifest["files"][0]["path"] = tampered_path.into();
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        for command in [
            vec!["--verify-data-backup", snapshot.to_str().unwrap()],
            vec![
                "--restore-data",
                snapshot.to_str().unwrap(),
                "--confirm-offline",
            ],
        ] {
            let refused = fixture.run(&command);
            assert!(
                !refused.status.success(),
                "tampered manifest {tampered_path} was accepted by {command:?}"
            );
        }
        assert_eq!(
            fs::read(fixture.home.join("sessions/2026-01-01/session.jsonl")).unwrap(),
            before,
            "a refused restore must not touch live data"
        );
    }
}

#[test]
fn restoring_a_snapshot_that_misses_a_root_leaves_that_root_untouched() {
    let fixture = Fixture::new();
    // Snapshot only sessions by removing storages first.
    fs::remove_dir_all(fixture.home.join("storages")).unwrap();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(backup.status.success(), "{}", describe(&backup));
    let snapshot = snapshot_path(&backup);
    // Recreate storages with content that must survive the restore.
    fs::create_dir_all(fixture.home.join("storages/live")).unwrap();
    fs::write(fixture.home.join("storages/live/keep.json"), b"keep").unwrap();
    fs::write(
        fixture.home.join("sessions/2026-01-01/session.jsonl"),
        b"changed",
    )
    .unwrap();

    let restored = fixture.run(&[
        "--restore-data",
        snapshot.to_str().unwrap(),
        "--confirm-offline",
    ]);
    assert!(restored.status.success(), "{}", describe(&restored));
    let json: serde_json::Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(json["restored"], serde_json::json!(["sessions"]));
    assert_eq!(json["untouched"], serde_json::json!(["storages"]));
    assert_eq!(
        fs::read(fixture.home.join("sessions/2026-01-01/session.jsonl")).unwrap(),
        b"{\"message\":\"synthetic\"}\n"
    );
    assert_eq!(
        fs::read(fixture.home.join("storages/live/keep.json")).unwrap(),
        b"keep",
        "a root absent from the snapshot must not be deleted or replaced"
    );
}

#[test]
fn snapshot_cancellation_removes_staging_and_leaves_live_data_untouched() {
    // Library-level check: a checkpoint error aborts before publication.
    let fixture = Fixture::new();
    let mut calls = 0;
    let aborted = dsh_ocean::data_backup::snapshot_with_checkpoint(
        &fixture.home,
        &fixture.backups,
        None,
        &mut || {
            calls += 1;
            anyhow::ensure!(calls < 3, "synthetic cancel");
            Ok(())
        },
    );
    assert!(aborted.is_err());
    assert!(calls >= 3);
    // No published snapshot and no staging residue.
    let entries: Vec<_> = fs::read_dir(&fixture.backups)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(entries.is_empty(), "unexpected backup entries: {entries:?}");
    assert_eq!(
        fs::read(fixture.home.join("sessions/2026-01-01/session.jsonl")).unwrap(),
        b"{\"message\":\"synthetic\"}\n"
    );
    // A normal run afterwards still succeeds.
    let finished = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(finished.status.success(), "{}", describe(&finished));
}

#[test]
fn abandoned_staging_is_listed_and_collected_only_with_confirmation() {
    let fixture = Fixture::new();
    // Simulate a killed snapshot: a staging directory with the engine's prefix.
    let stale = fixture.backups.join(".ocean-stage-synthetic");
    fs::create_dir_all(stale.join("data")).unwrap();
    fs::write(stale.join("data/partial"), b"partial").unwrap();
    // Age it so the default minimum age considers it abandoned.
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    let handle = fs::OpenOptions::new().read(true).open(&stale).unwrap();
    handle
        .set_times(fs::FileTimes::new().set_modified(when).set_accessed(when))
        .unwrap();

    let listed = fixture.run(&["--list-backup-staging", fixture.backups.to_str().unwrap()]);
    assert!(listed.status.success(), "{}", describe(&listed));
    let json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(json["count"], 1);
    let entry = &json["staging"][0];
    assert_eq!(entry["name"], ".ocean-stage-synthetic");
    assert_eq!(entry["bytes"], 7);
    assert!(entry["ageSeconds"].as_u64().unwrap() >= 7000);
    // Listing never deletes.
    assert!(stale.join("data/partial").is_file());

    let unconfirmed = fixture.run(&[
        "--collect-backup-staging",
        "--older-than-minutes",
        "60",
        fixture.backups.to_str().unwrap(),
    ]);
    assert!(!unconfirmed.status.success());
    assert!(stale.is_dir());

    let collected = fixture.run(&[
        "--collect-backup-staging",
        "--older-than-minutes",
        "60",
        "--confirm-collect",
        fixture.backups.to_str().unwrap(),
    ]);
    assert!(collected.status.success(), "{}", describe(&collected));
    let json: serde_json::Value = serde_json::from_slice(&collected.stdout).unwrap();
    assert_eq!(json["removed"].as_array().unwrap().len(), 1);
    assert!(!stale.exists());
    // Live data and real snapshots are untouched.
    assert!(fixture
        .home
        .join("sessions/2026-01-01/session.jsonl")
        .is_file());
}

#[test]
fn exports_are_deterministic_verifiable_and_refuse_overwrite() {
    let fixture = Fixture::new();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(backup.status.success(), "{}", describe(&backup));
    let snapshot = snapshot_path(&backup);
    let first = fixture.root.join("export-one.tar");
    let second = fixture.root.join("export-two.tar");

    for destination in [&first, &second] {
        let export = fixture.run(&[
            "--export-backup",
            snapshot.to_str().unwrap(),
            destination.to_str().unwrap(),
        ]);
        assert!(export.status.success(), "{}", describe(&export));
        let json: serde_json::Value = serde_json::from_slice(&export.stdout).unwrap();
        let receipt = &json["receipt"];
        assert_eq!(receipt["scope"], "dsh-data");
        assert_eq!(receipt["files"], 2);
        assert_eq!(receipt["archiveSha256"].as_str().unwrap().len(), 64);
    }
    // Same snapshot, same bytes: the archive is reproducible.
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());

    let verified = fixture.run(&["--verify-export", first.to_str().unwrap()]);
    assert!(verified.status.success(), "{}", describe(&verified));
    let json: serde_json::Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(json["receipt"]["scope"], "dsh-data");
    assert_eq!(json["receipt"]["files"], 2);

    // An existing destination is never overwritten.
    let refused = fixture.run(&[
        "--export-backup",
        snapshot.to_str().unwrap(),
        first.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(
        describe(&refused).contains("overwrite"),
        "{}",
        describe(&refused)
    );

    // Editing one byte inside the archive is detected.
    let mut bytes = fs::read(&first).unwrap();
    let position = bytes
        .windows(8)
        .position(|window| window == b"{\"cost\":")
        .expect("payload present in the archive");
    bytes[position + 2] = b'9';
    fs::write(&second, &bytes).unwrap();
    let tampered = fixture.run(&["--verify-export", second.to_str().unwrap()]);
    assert!(!tampered.status.success());
    assert!(
        describe(&tampered).contains("integrity") || describe(&tampered).contains("checksum"),
        "{}",
        describe(&tampered)
    );
}

#[test]
fn export_refuses_a_tampered_snapshot_and_unrelated_scope() {
    let fixture = Fixture::new();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    let snapshot = snapshot_path(&backup);
    fs::write(
        snapshot.join("data/storages/cost-meter/ledger.json"),
        b"{\"cost\":999}\n",
    )
    .unwrap();
    let destination = fixture.root.join("export.tar");
    let refused = fixture.run(&[
        "--export-backup",
        snapshot.to_str().unwrap(),
        destination.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(
        !destination.exists(),
        "a failed export must not leave a file"
    );

    let unrelated = fixture.root.join("unrelated");
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(
        unrelated.join("manifest.json"),
        b"{\"scope\":\"something-else\"}",
    )
    .unwrap();
    let refused = fixture.run(&[
        "--export-backup",
        unrelated.to_str().unwrap(),
        destination.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(
        describe(&refused).contains("scope"),
        "{}",
        describe(&refused)
    );
}

#[test]
fn exported_archives_import_back_into_a_verifiable_snapshot() {
    let fixture = Fixture::new();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(backup.status.success(), "{}", describe(&backup));
    let snapshot = snapshot_path(&backup);
    let archive = fixture.root.join("export.tar");
    let exported = fixture.run(&[
        "--export-backup",
        snapshot.to_str().unwrap(),
        archive.to_str().unwrap(),
    ]);
    assert!(exported.status.success(), "{}", describe(&exported));

    let imported = fixture.root.join("imported");
    let imported_run = fixture.run(&[
        "--import-export",
        archive.to_str().unwrap(),
        imported.to_str().unwrap(),
    ]);
    assert!(imported_run.status.success(), "{}", describe(&imported_run));
    let json: serde_json::Value = serde_json::from_slice(&imported_run.stdout).unwrap();
    assert_eq!(json["receipt"]["scope"], "dsh-data");
    assert_eq!(json["receipt"]["files"], 2);
    assert_eq!(
        fs::read(imported.join("data/sessions/2026-01-01/session.jsonl")).unwrap(),
        b"{\"message\":\"synthetic\"}\n"
    );
    // The imported copy is a real snapshot: the engine verifies it.
    let verified = fixture.run(&["--verify-data-backup", imported.to_str().unwrap()]);
    assert!(verified.status.success(), "{}", describe(&verified));
    // Re-exporting the imported copy yields byte-identical bytes.
    let again = fixture.root.join("export-again.tar");
    let reexported = fixture.run(&[
        "--export-backup",
        imported.to_str().unwrap(),
        again.to_str().unwrap(),
    ]);
    assert!(reexported.status.success(), "{}", describe(&reexported));
    assert_eq!(fs::read(&archive).unwrap(), fs::read(&again).unwrap());

    // Importing over an existing path is refused and changes nothing.
    let refused = fixture.run(&[
        "--import-export",
        archive.to_str().unwrap(),
        imported.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(
        describe(&refused).contains("existing"),
        "{}",
        describe(&refused)
    );
}

#[test]
fn a_tampered_archive_cannot_be_imported() {
    let fixture = Fixture::new();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    let snapshot = snapshot_path(&backup);
    let archive = fixture.root.join("export.tar");
    let exported = fixture.run(&[
        "--export-backup",
        snapshot.to_str().unwrap(),
        archive.to_str().unwrap(),
    ]);
    assert!(exported.status.success(), "{}", describe(&exported));
    let mut bytes = fs::read(&archive).unwrap();
    let position = bytes
        .windows(8)
        .position(|window| window == b"{\"cost\":")
        .expect("payload present");
    bytes[position + 2] = b'9';
    fs::write(&archive, &bytes).unwrap();
    let destination = fixture.root.join("should-not-exist");
    let refused = fixture.run(&[
        "--import-export",
        archive.to_str().unwrap(),
        destination.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(
        !destination.exists(),
        "a refused import must leave nothing behind"
    );
    // No staging residue next to the destination either.
    let leftovers: Vec<_> = fs::read_dir(&fixture.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("import-"))
        .collect();
    assert!(leftovers.is_empty(), "leftover staging: {leftovers:?}");
}

#[test]
fn a_verify_sweep_reports_each_snapshot_and_never_deletes() {
    let fixture = Fixture::new();
    let first = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(first.status.success(), "{}", describe(&first));
    std::thread::sleep(std::time::Duration::from_millis(3));
    let second = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(second.status.success(), "{}", describe(&second));
    let broken = snapshot_path(&second);
    fs::write(
        broken.join("data/sessions/2026-01-01/session.jsonl"),
        b"corrupted",
    )
    .unwrap();

    let sweep = fixture.run(&["--verify-backups", fixture.backups.to_str().unwrap()]);
    assert_eq!(sweep.status.code(), Some(1), "{}", describe(&sweep));
    let json: serde_json::Value = serde_json::from_slice(&sweep.stdout).unwrap();
    assert_eq!(json["verifiedCount"], 1);
    assert_eq!(json["failedCount"], 1);
    assert_eq!(json["verified"][0]["kind"], "data");
    assert_eq!(json["failed"][0]["kind"], "data");
    // Both snapshots still exist: a sweep never deletes anything.
    assert!(broken.is_dir());
    assert!(snapshot_path(&first).is_dir());

    // A missing root must not masquerade as a successfully verified empty root.
    let absent = fixture.run(&[
        "--verify-backups",
        fixture.root.join("absent").to_str().unwrap(),
    ]);
    assert_eq!(absent.status.code(), Some(1), "{}", describe(&absent));
    assert!(absent.stdout.is_empty());
    let empty_root = fixture.root.join("empty-backups");
    fs::create_dir(&empty_root).unwrap();
    let empty = fixture.run(&["--verify-backups", empty_root.to_str().unwrap()]);
    assert!(empty.status.success(), "{}", describe(&empty));
    let json: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(json["verifiedCount"], 0);
    assert_eq!(json["failedCount"], 0);
}

#[test]
fn a_verify_sweep_includes_broken_manifests_and_symlinked_candidates() {
    let fixture = Fixture::new();
    for name in ["data-1-1", "profile-web-1-2"] {
        let directory = fixture.backups.join(name);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("manifest.json"), "{broken-private-secret").unwrap();
    }
    let outside = fixture.root.join("outside-snapshot");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "private-secret").unwrap();
    symlink(&outside, fixture.backups.join("data-1-3")).unwrap();
    let output = fixture.run(&["--verify-backups", fixture.backups.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1), "{}", describe(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["verifiedCount"], 0);
    assert_eq!(json["failedCount"], 3);
    assert!(!describe(&output).contains("private-secret"));
    assert_eq!(
        fs::read_to_string(outside.join("keep")).unwrap(),
        "private-secret"
    );
    for name in ["data-1-1", "profile-web-1-2"] {
        assert_eq!(
            fs::read_to_string(fixture.backups.join(name).join("manifest.json")).unwrap(),
            "{broken-private-secret"
        );
    }
}

#[test]
fn a_verify_sweep_refuses_non_directory_and_symlinked_roots() {
    let fixture = Fixture::new();
    let file = fixture.root.join("not-a-root");
    fs::write(&file, "keep").unwrap();
    let alias = fixture.root.join("root-alias");
    symlink(&fixture.backups, &alias).unwrap();
    let trailing_slash = PathBuf::from(format!("{}/", alias.display()));
    let trailing_dot = alias.join(".");
    let repeated_suffix = PathBuf::from(format!("{}//./", alias.display()));
    for path in [
        &file,
        &alias,
        &trailing_slash,
        &trailing_dot,
        &repeated_suffix,
    ] {
        let output = fixture.run(&["--verify-backups", path.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(1), "{}", describe(&output));
        assert!(output.stdout.is_empty());
    }
    assert_eq!(fs::read_to_string(file).unwrap(), "keep");
    assert!(fs::symlink_metadata(alias)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn a_twenty_megabyte_session_file_is_backed_up_and_verified() {
    // The whole-data engine advertises 512 MiB per file; inheriting the 16 MiB
    // profile cap made realistic session files unbackuppable.
    let fixture = Fixture::new();
    let big = fixture.home.join("sessions/big.jsonl");
    fs::write(&big, vec![b'x'; 20_000_000]).unwrap();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(backup.status.success(), "{}", describe(&backup));
    let snapshot = snapshot_path(&backup);
    let verified = fixture.run(&["--verify-data-backup", snapshot.to_str().unwrap()]);
    assert!(verified.status.success(), "{}", describe(&verified));
    assert_eq!(
        fs::metadata(snapshot.join("data/sessions/big.jsonl"))
            .unwrap()
            .len(),
        20_000_000
    );
    // And it survives the export/import path too.
    let archive = fixture.root.join("big.tar");
    for (args, label) in [
        (
            vec![
                "--export-backup".to_owned(),
                snapshot.display().to_string(),
                archive.display().to_string(),
            ],
            "export",
        ),
        (
            vec![
                "--import-export".to_owned(),
                archive.display().to_string(),
                fixture.root.join("big-imported").display().to_string(),
            ],
            "import",
        ),
    ] {
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = fixture.run(&borrowed);
        assert!(output.status.success(), "{label}: {}", describe(&output));
    }
}

#[test]
fn retention_prefers_the_embedded_creation_time_over_the_whole_name() {
    // Names are `data-<pid>-<nanos>`; a lexicographic sort lets the pid dominate and
    // deletes the newest snapshot.
    let fixture = Fixture::new();
    let kept = fixture.backups.join("data-999-1000000000000000000");
    let newer = fixture.backups.join("data-1000-2000000000000000000");
    for pair in [(&kept, 1u64), (&newer, 2u64)] {
        let (path, marker) = pair;
        fs::create_dir_all(path.join("data/sessions")).unwrap();
        fs::write(path.join("data/sessions/one.jsonl"), format!("{marker}")).unwrap();
        fs::write(
            path.join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "formatVersion": 1,
                "scope": "dsh-data",
                "createdAt": "2026-01-01T00:00:00+00:00",
                "clientVersion": "0.0.0",
                "runtimeVersion": null,
                "roots": ["sessions"],
                "directories": ["sessions"],
                "files": [{
                    "path": "sessions/one.jsonl",
                    "size": 1,
                    "sha256": "0".repeat(64),
                    "executable": false
                }],
                "skipped": 0
            }))
            .unwrap(),
        )
        .unwrap();
    }
    // Manifest hashes are synthetic, so prune's verify-before-delete keeps both and
    // the ordering itself is what this test observes.
    let listed = fixture.run(&["--list-data-backups", fixture.backups.to_str().unwrap()]);
    assert!(listed.status.success(), "{}", describe(&listed));
    let json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let names: Vec<String> = json["backups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            "data-999-1000000000000000000".to_owned(),
            "data-1000-2000000000000000000".to_owned()
        ],
        "listing must be ordered by creation time, not by pid"
    );
}

#[test]
fn an_empty_directory_survives_export_and_import() {
    let fixture = Fixture::new();
    // storages/ exists but holds nothing: the manifest records it as a directory.
    let empty = fixture.home.join("storages/empty");
    fs::create_dir_all(&empty).unwrap();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(backup.status.success(), "{}", describe(&backup));
    let snapshot = snapshot_path(&backup);
    let archive = fixture.root.join("empty-dir.tar");
    let exported = fixture.run(&[
        "--export-backup",
        snapshot.to_str().unwrap(),
        archive.to_str().unwrap(),
    ]);
    assert!(exported.status.success(), "{}", describe(&exported));
    let dest = fixture.root.join("empty-imported");
    let imported = fixture.run(&[
        "--import-export",
        archive.to_str().unwrap(),
        dest.to_str().unwrap(),
    ]);
    assert!(imported.status.success(), "{}", describe(&imported));
    assert!(
        dest.join("data/storages/empty").is_dir(),
        "the empty directory was lost in the round trip"
    );
    let verified = fixture.run(&["--verify-data-backup", dest.to_str().unwrap()]);
    assert!(verified.status.success(), "{}", describe(&verified));
}

#[test]
fn an_export_archive_and_its_directory_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let backup = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(backup.status.success(), "{}", describe(&backup));
    let snapshot = snapshot_path(&backup);
    let directory = fixture.root.join("private-export");
    let archive = directory.join("backup.tar");
    let exported = fixture.run(&[
        "--export-backup",
        snapshot.to_str().unwrap(),
        archive.to_str().unwrap(),
    ]);
    assert!(exported.status.success(), "{}", describe(&exported));
    let file_mode = fs::metadata(&archive).unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o600, "the archive must not be world-readable");
    let dir_mode = fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "the export directory must be private");
}

#[test]
fn restore_leftovers_are_listed_but_never_collected() {
    let fixture = Fixture::new();
    // A restore that dies between the exchange and the retain rename leaves the only
    // copy of the previous root under this prefix; collection must refuse it.
    let leftover = fixture.backups.join(".ocean-data-restore-1-2-sessions");
    fs::create_dir_all(&leftover).unwrap();
    fs::write(leftover.join("session.jsonl"), b"only copy").unwrap();
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    let handle = fs::OpenOptions::new().read(true).open(&leftover).unwrap();
    handle
        .set_times(fs::FileTimes::new().set_modified(when).set_accessed(when))
        .unwrap();

    let listed = fixture.run(&["--list-backup-staging", fixture.backups.to_str().unwrap()]);
    assert!(listed.status.success(), "{}", describe(&listed));
    let json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(json["count"], 1);
    assert_eq!(json["staging"][0]["collectable"], false);
    assert!(json["warning"]
        .as_str()
        .unwrap()
        .contains("never deleted automatically"));

    let collected = fixture.run(&[
        "--collect-backup-staging",
        "--older-than-minutes",
        "60",
        "--confirm-collect",
        fixture.backups.to_str().unwrap(),
    ]);
    assert!(collected.status.success(), "{}", describe(&collected));
    let json: serde_json::Value = serde_json::from_slice(&collected.stdout).unwrap();
    assert_eq!(json["removed"].as_array().unwrap().len(), 0);
    assert!(leftover.join("session.jsonl").is_file());
}

#[test]
fn a_snapshot_notices_when_the_source_changed_while_copying() {
    let fixture = Fixture::new();
    // A checkpoint runs between files: mutate the tree from it, exactly like a live
    // session writer would, and the published manifest must say "not frozen".
    let session = fixture.home.join("sessions/2026-01-01");
    let mut mutated = false;
    let created = dsh_ocean::data_backup::snapshot_with_progress(
        &fixture.home,
        &fixture.backups,
        None,
        &mut |update| {
            // Once at least one file has been copied, add another one: it can only
            // appear in the post-copy re-walk.
            if update.copied_files >= 1 && !mutated {
                mutated = true;
                fs::write(session.join("arrived-late.jsonl"), b"late").unwrap();
            }
            Ok(())
        },
    );
    let snapshot = created.expect("snapshot still publishes");
    let manifest = dsh_ocean::data_backup::manifest(&snapshot).unwrap();
    assert!(
        !manifest.consistent,
        "a changed source must be reported as a best-effort copy"
    );
    // An untouched tree is reported as consistent.
    let clean = fixture.run(&["--backup-data", fixture.backups.to_str().unwrap()]);
    assert!(clean.status.success(), "{}", describe(&clean));
    let json: serde_json::Value = serde_json::from_slice(&clean.stdout).unwrap();
    assert_eq!(json["consistent"], true);
    let snapshot = snapshot_path(&clean);
    assert!(
        dsh_ocean::data_backup::manifest(&snapshot)
            .unwrap()
            .consistent
    );
}

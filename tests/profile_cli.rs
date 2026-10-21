#![cfg(target_os = "linux")]
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    profile: PathBuf,
    origin: String,
}
impl Fixture {
    fn new() -> Self {
        Self::new_with_stamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        )
    }
    fn new_with_stamp(nanos: u128) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = loop {
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "dsh-ocean-profile-cli-{}-{nanos}-{serial}",
                std::process::id()
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => break path,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("reserve private profile fixture: {error}"),
            }
        };
        let home = root.join("dsh");
        let profile = home.join("profiles/web");
        for dir in [
            &profile,
            &root.join("home"),
            &root.join("run"),
            &root.join("bin"),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(profile.join("package.json"), "{}\n").unwrap();
        fs::write(
            profile.join("cordis.patch.yml"),
            "synthetic-profile-secret\n",
        )
        .unwrap();
        fs::write(
            home.join(".credentials.yaml"),
            "synthetic-global-credential\n",
        )
        .unwrap();
        let sysctl = root.join("bin/systemctl");
        fs::write(
            &sysctl,
            "#!/bin/bash\n[ \"${MOCK_ACTIVE:-0}\" = 1 ] && exit 0\nexit 3\n",
        )
        .unwrap();
        fs::set_permissions(&sysctl, fs::Permissions::from_mode(0o755)).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        drop(listener);
        Self {
            root,
            home,
            profile,
            origin,
        }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dsh-ocean"));
        cmd.env_clear()
            .args(args)
            .env("HOME", self.root.join("home"))
            .env("DSH_HOME", &self.home)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("DSH_OCEAN_DATA_DIR", self.root.join("runtime"))
            .env("DSH_OCEAN_URL", &self.origin)
            // Only the read-only systemctl stub: no Node/npm or real manager.
            .env("PATH", self.root.join("bin"));
        cmd
    }
    fn snapshot(&self) -> PathBuf {
        let output = self.command(&["--backup-profile"]).output().unwrap();
        assert!(output.status.success(), "{}", describe(&output));
        assert!(!describe(&output).contains("synthetic-profile-secret"));
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        PathBuf::from(json["snapshot"].as_str().unwrap())
    }
    fn restore(&self, snapshot: &std::path::Path) -> Command {
        self.command(&[
            "--restore-profile",
            "web",
            snapshot.to_str().unwrap(),
            "--confirm-offline",
        ])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-profile-cli-"));
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn describe(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn equal_timestamp_fixture_cleanup_never_deletes_another_profiles_data() {
    let first = Fixture::new_with_stamp(0);
    let second = Fixture::new_with_stamp(0);
    drop(first);
    assert!(
        second.profile.join("package.json").is_file(),
        "a peer fixture must not share or delete this fixture's profile"
    );
}

#[test]
fn cli_backup_verify_and_explicit_offline_restore_work_without_node() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let verified = f
        .command(&["--verify-profile-backup", snapshot.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(verified.status.success(), "{}", describe(&verified));
    assert!(!describe(&verified).contains("synthetic-profile-secret"));
    fs::write(f.profile.join("cordis.patch.yml"), "new profile\n").unwrap();
    let missing_flag = f
        .command(&["--restore-profile", "web", snapshot.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!missing_flag.status.success());
    assert!(!f.root.join("runtime").exists());
    let restored = f.restore(&snapshot).output().unwrap();
    assert!(restored.status.success(), "{}", describe(&restored));
    let receipt: serde_json::Value = serde_json::from_slice(&restored.stdout).unwrap();
    let retained = PathBuf::from(receipt["retainedProfile"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(retained.join("cordis.patch.yml")).unwrap(),
        "new profile\n"
    );
    assert_eq!(
        fs::read_to_string(f.profile.join("cordis.patch.yml")).unwrap(),
        "synthetic-profile-secret\n"
    );
    assert_eq!(
        fs::read_to_string(f.home.join(".credentials.yaml")).unwrap(),
        "synthetic-global-credential\n"
    );
}

#[test]
fn cli_refuses_an_active_user_unit_or_running_ocean_socket() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    fs::write(f.profile.join("cordis.patch.yml"), "keep this\n").unwrap();
    let active = f
        .restore(&snapshot)
        .env("MOCK_ACTIVE", "1")
        .output()
        .unwrap();
    assert!(!active.status.success());
    assert!(describe(&active).contains("stop dsh-web.service"));
    let listener = UnixListener::bind(f.root.join("run/dsh-ocean.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let app = f.restore(&snapshot).output().unwrap();
    assert!(!app.status.success());
    assert!(describe(&app).contains("quit DSH Ocean"));
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "offline check must not connect/show the GUI"
    );
    assert_eq!(
        fs::read_to_string(f.profile.join("cordis.patch.yml")).unwrap(),
        "keep this\n"
    );
}

#[test]
fn stale_socket_file_is_not_mistaken_for_a_running_app() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let socket = f.root.join("run/dsh-ocean.sock");
    drop(UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    let output = f.restore(&snapshot).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn cli_refuses_responding_http_server_and_parallel_runtime_operation() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                Err(error) => panic!("test server did not receive a request: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 256];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let n = stream.read(&mut chunk).unwrap();
            if n == 0 {
                return;
            }
            request.extend_from_slice(&chunk[..n]);
        }
        stream
            .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
    });
    let active = f
        .restore(&snapshot)
        .env("DSH_OCEAN_URL", origin)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(!active.status.success());
    assert!(describe(&active).contains("server is still responding"));
    let held =
        dsh_ocean::profile_backup::operation_lock(&f.root.join("runtime/.bootstrap.lock")).unwrap();
    let busy = f.restore(&snapshot).output().unwrap();
    assert!(!busy.status.success());
    assert!(describe(&busy).contains("runtime operation in progress"));
    drop(held);
}

#[test]
fn list_and_prune_commands_are_read_only_until_confirmed() {
    let fixture = Fixture::new();
    let mut snapshots = Vec::new();
    for _ in 0..3 {
        snapshots.push(fixture.snapshot());
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    let listed = fixture
        .command(&["--list-profile-backups"])
        .output()
        .unwrap();
    assert!(listed.status.success(), "{}", describe(&listed));
    let json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(json["count"], 3);
    let entries = json["backups"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    for entry in entries {
        assert_eq!(entry["profile"], "web");
        assert!(entry["files"].as_u64().unwrap() > 0);
        assert!(entry["bytes"].as_u64().unwrap() > 0);
        assert!(entry["createdAt"].is_string());
    }
    // The inventory is content-free: no profile text, filenames or credentials.
    let rendered = describe(&listed);
    assert!(!rendered.contains("synthetic-profile-secret"));
    assert!(!rendered.contains("synthetic-global-credential"));
    assert!(!rendered.contains("cordis.patch.yml"));
    for snapshot in &snapshots {
        assert!(snapshot.is_dir(), "listing must not delete");
    }

    // Pruning refuses without explicit confirmation and without --keep.
    let unconfirmed = fixture
        .command(&["--prune-profile-backups", "--keep", "1"])
        .output()
        .unwrap();
    assert!(!unconfirmed.status.success());
    assert!(describe(&unconfirmed).contains("--confirm-prune"));
    let missing_keep = fixture
        .command(&["--prune-profile-backups", "--confirm-prune"])
        .output()
        .unwrap();
    assert!(!missing_keep.status.success());
    assert!(describe(&missing_keep).contains("--keep"));
    let bad_keep = fixture
        .command(&["--prune-profile-backups", "--keep", "0", "--confirm-prune"])
        .output()
        .unwrap();
    assert!(!bad_keep.status.success());
    for snapshot in &snapshots {
        assert!(snapshot.is_dir(), "refused prune must not delete");
    }

    let pruned = fixture
        .command(&["--prune-profile-backups", "--keep", "1", "--confirm-prune"])
        .output()
        .unwrap();
    assert!(pruned.status.success(), "{}", describe(&pruned));
    let json: serde_json::Value = serde_json::from_slice(&pruned.stdout).unwrap();
    assert_eq!(json["removed"].as_array().unwrap().len(), 2);
    assert_eq!(json["kept"].as_array().unwrap().len(), 1);
    assert_eq!(json["unreadable"].as_array().unwrap().len(), 0);
    assert!(snapshots[2].is_dir());
    assert!(!snapshots[0].exists() && !snapshots[1].exists());
    // The active profile is untouched.
    assert_eq!(
        fs::read_to_string(fixture.profile.join("cordis.patch.yml")).unwrap(),
        "synthetic-profile-secret\n"
    );
    let after = fixture
        .command(&["--list-profile-backups"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&after.stdout).unwrap();
    assert_eq!(json["count"], 1);
}

#[test]
fn a_profile_snapshot_round_trips_through_export_and_import() {
    let fixture = Fixture::new();
    let snapshot = fixture.snapshot();
    let archive = fixture.root.join("profile-export.tar");
    let exported = fixture
        .command(&[
            "--export-backup",
            snapshot.to_str().unwrap(),
            archive.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(exported.status.success(), "{}", describe(&exported));
    let json: serde_json::Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(json["receipt"]["scope"], "profile-config");
    assert_eq!(json["receipt"]["files"], 2);
    // The archive itself must not be mistaken for a data snapshot.
    let verified = fixture
        .command(&["--verify-export", archive.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(verified.status.success(), "{}", describe(&verified));
    let json: serde_json::Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(json["receipt"]["scope"], "profile-config");

    let imported = fixture.root.join("imported-profile-snapshot");
    let imported_run = fixture
        .command(&[
            "--import-export",
            archive.to_str().unwrap(),
            imported.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(imported_run.status.success(), "{}", describe(&imported_run));
    let json: serde_json::Value = serde_json::from_slice(&imported_run.stdout).unwrap();
    assert_eq!(json["receipt"]["scope"], "profile-config");
    // The imported copy is a real profile snapshot, verified by its own engine.
    let profile_verified = fixture
        .command(&["--verify-profile-backup", imported.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        profile_verified.status.success(),
        "{}",
        describe(&profile_verified)
    );
    // Re-exporting the imported snapshot reproduces the same bytes.
    let again = fixture.root.join("profile-export-again.tar");
    let reexported = fixture
        .command(&[
            "--export-backup",
            imported.to_str().unwrap(),
            again.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(reexported.status.success(), "{}", describe(&reexported));
    assert_eq!(fs::read(&archive).unwrap(), fs::read(&again).unwrap());
    // Profile contents never appear in the command output.
    let rendered = describe(&exported) + &describe(&verified) + &describe(&imported_run);
    assert!(!rendered.contains("synthetic-profile-secret"));
    assert!(!rendered.contains("synthetic-global-credential"));
}

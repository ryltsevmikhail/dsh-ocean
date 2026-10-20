//! `--doctor` output in a private sandbox: read-only counters, no secrets.
#![cfg(target_os = "linux")]
use std::fs;
use std::os::unix::fs::{symlink, FileTypeExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    dsh: PathBuf,
    runtime: PathBuf,
    state: PathBuf,
    origin: String,
}

impl Sandbox {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-doctor-cli-{}-{nanos}",
            std::process::id()
        ));
        let home = root.join("home");
        let dsh = root.join("dsh");
        let runtime = root.join("runtime");
        let state = root.join("state");
        let profile = dsh.join("profiles/web");
        let sessions = dsh.join("sessions/2026-01-01");
        let storages = dsh.join("storages/cost-meter");
        for directory in [
            &home,
            &profile,
            &sessions,
            &storages,
            &state,
            &root.join("run"),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(profile.join("package.json"), "{}\n").unwrap();
        fs::write(
            profile.join("cordis.patch.yml"),
            "synthetic-doctor-secret\n",
        )
        .unwrap();
        fs::write(sessions.join("session.jsonl"), b"session-secret\n").unwrap();
        fs::write(storages.join("ledger.json"), b"ledger\n").unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        // A selected generation plus one orphan and stale scratch.
        let generations = runtime.join("generations");
        for name in ["gen.aaa", "gen.bbb"] {
            fs::create_dir_all(generations.join(name)).unwrap();
        }
        fs::write(generations.join("gen.aaa/.previous"), "none\n").unwrap();
        symlink("generations/gen.aaa", runtime.join("current")).unwrap();
        fs::create_dir_all(runtime.join(".download.stale")).unwrap();
        // Deliberately unroutable origin: never probe the real local server.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        drop(listener);
        Self {
            root,
            home,
            dsh,
            runtime,
            state,
            origin,
        }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.run_with_origin(arguments, &self.origin)
    }

    fn run_with_origin(&self, arguments: &[&str], origin: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(arguments)
            .env_clear()
            .env("HOME", &self.home)
            .env("DSH_HOME", &self.dsh)
            .env("DSH_OCEAN_DATA_DIR", &self.runtime)
            .env("DSH_OCEAN_URL", origin)
            .env("XDG_STATE_HOME", &self.state)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            // No systemctl on PATH: the unit probe must fail closed.
            .env("PATH", self.root.join("bin"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run the client CLI");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "doctor fixture exceeded 10 seconds: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self
            .root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("dsh-ocean-doctor-cli-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn doctor_reports_read_only_runtime_and_backup_counters() {
    let sandbox = Sandbox::new();
    let backup = sandbox.run(&["--backup-profile"]);
    assert!(backup.status.success(), "{}", stdout(&backup));
    let output = sandbox.run(&["--doctor"]);
    assert!(output.status.success(), "{}", stdout(&output));
    let text = stdout(&output);
    assert!(
        text.contains("runtime state: selected=gen.aaa previous=none"),
        "{text}"
    );
    assert!(text.contains("known=2"), "{text}");
    assert!(text.contains("unreferenced=1"), "{text}");
    assert!(text.contains("staleScratch=1"), "{text}");
    assert!(text.contains("backups:       count=1 bytes="), "{text}");
    // The ABI probe reports whatever this host loads; a healthy CI/desktop
    // environment has GTK 3 and WebKitGTK 4.1 present.
    assert!(text.contains("webkit abi:    gtk="), "{text}");
    assert!(
        text.contains("(ok)") || text.contains("(missing)") || text.contains("(too old)"),
        "{text}"
    );
    // Storage counters are count-only and never expose names or contents.
    assert!(
        text.contains("storage:       sessions=1/15B storages=1/7B skipped=0"),
        "{text}"
    );
    assert!(!text.contains("session-secret"), "{text}");
    assert!(
        !text.contains("session.jsonl") && !text.contains("ledger.json"),
        "{text}"
    );
    // Counters only: no profile contents, paths inside the profile or garbage.
    assert!(!text.contains("synthetic-doctor-secret"), "{text}");
    assert!(!text.contains("cordis.patch.yml"), "{text}");
    // Doctor is read-only: fixtures are intact afterwards.
    assert!(sandbox.runtime.join("generations/gen.bbb").is_dir());
    assert!(sandbox.runtime.join(".download.stale").is_dir());
    assert!(sandbox.dsh.join("profiles/web/cordis.patch.yml").is_file());
}

#[test]
fn doctor_still_reports_when_the_prefix_and_backups_are_absent() {
    let sandbox = Sandbox::new();
    fs::remove_dir_all(&sandbox.runtime).unwrap();
    let output = sandbox.run(&["--doctor"]);
    assert!(output.status.success(), "{}", stdout(&output));
    let text = stdout(&output);
    assert!(text.contains("runtime state: selected=none previous=none known=0"));
    assert!(text.contains("backups:       count=0 bytes=0"), "{text}");
    // The storage counters remain readable even when the runtime prefix is gone.
    assert!(text.contains("storage:       "), "{text}");
    assert!(!text.contains("storage:       unavailable"), "{text}");
}

#[test]
fn blank_environment_override_preserves_the_config_origin_in_runtime_and_diagnosis() {
    let sandbox = Sandbox::new();
    let config = sandbox.root.join("config/dsh-ocean/config.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        serde_json::json!({"serverUrl": sandbox.origin}).to_string(),
    )
    .unwrap();
    for origin in ["", " ", "\t\n"] {
        let output = sandbox.run_with_origin(&["--doctor", "--json"], origin);
        assert!(output.status.success(), "{}", stdout(&output));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["serverUrl"], sandbox.origin);
        assert_eq!(report["configReport"]["effectiveUrl"], sandbox.origin);
        assert_eq!(report["configReport"]["urlSource"], "config-file");
        assert_eq!(report["configReport"]["issues"], serde_json::json!([]));
    }
}

#[test]
fn doctor_rejects_fifo_config_and_previous_marker_without_hanging() {
    use std::os::unix::ffi::OsStrExt;
    let sandbox = Sandbox::new();
    let config = sandbox.root.join("config/dsh-ocean/config.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let previous = sandbox.runtime.join("generations/gen.aaa/.previous");
    assert_eq!(
        previous,
        sandbox.runtime.join("generations/gen.aaa/.previous")
    );
    fs::remove_file(&previous).unwrap();
    for path in [&config, &previous] {
        let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    }
    let output = sandbox.run(&["--doctor", "--json"]);
    assert!(output.status.success(), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["configReport"]["exists"], true);
    assert_eq!(report["configReport"]["valid"], false);
    assert_eq!(report["configReport"]["effectiveUrl"], sandbox.origin);
    let issues = report["configReport"]["issues"].to_string();
    assert!(issues.contains("could not be read safely"), "{issues}");
    assert!(!issues.contains("is missing"), "{issues}");
    assert!(report["runtimeState"]["previous"].is_null());
    assert!(fs::symlink_metadata(config).unwrap().file_type().is_fifo());
}

#[test]
fn json_and_strict_modes_are_machine_readable_and_honest() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--doctor", "--json"]);
    assert!(output.status.success(), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(report["clientVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["runtimeState"]["selected"], "gen.aaa");
    assert_eq!(report["runtimeState"]["generations"], 2);
    assert_eq!(report["runtimeState"]["unreferenced"], 1);
    assert!(
        report["serverStatus"].is_null(),
        "unroutable origin must be null"
    );
    assert!(report["webkitAbi"]["status"].is_string());
    // Counts are numbers, not strings, so scripts can consume them.
    assert!(report["storage"]["sessionFiles"].is_number());
    // Per-generation accounting is listed by name and marked referenced.
    let generations = report["runtimeGenerations"].as_array().unwrap();
    assert_eq!(generations.len(), 2);
    assert_eq!(generations[0]["name"], "gen.aaa");
    assert_eq!(generations[0]["referenced"], true);
    assert_eq!(generations[1]["name"], "gen.bbb");
    assert_eq!(generations[1]["referenced"], false);
    // Sizes are numbers and no absolute path leaks into the array.
    for generation in generations {
        assert!(generation["bytes"].is_number());
        assert!(!generation["name"].as_str().unwrap().contains('/'));
    }
    assert!(report["backups"].is_string());
    // No cached token in the sandbox, so nothing was checked and nothing is claimed.
    assert_eq!(report["launchToken"], "absent");
    // The sandbox has no config.json, and that fallback is stated, not hidden.
    assert_eq!(report["configReport"]["exists"], false);
    assert_eq!(report["configReport"]["valid"], false);
    assert_eq!(report["configReport"]["urlSource"], "environment");
    assert!(report["configReport"]["effectiveUrl"]
        .as_str()
        .unwrap()
        .starts_with("http://127.0.0.1:"));
    assert!(!report["configReport"]["issues"]
        .as_array()
        .unwrap()
        .is_empty());

    // The same sandbox cannot reach its server, so strict mode must fail.
    let strict = sandbox.run(&["--doctor", "--strict"]);
    assert_eq!(strict.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&strict.stderr).contains("blocking problem"),
        "{}",
        String::from_utf8_lossy(&strict.stderr)
    );
    // Non-strict mode still succeeds for the same environment.
    assert!(sandbox.run(&["--doctor"]).status.success());

    // Unknown flags are refused before printing a report.
    let refused = sandbox.run(&["--doctor", "--everything"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(stdout(&refused).is_empty());
}

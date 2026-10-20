//! Operator maintenance through the packaged client's own CLI surface.
//!
//! Every check runs in a private temporary HOME/XDG prefix; no real runtime,
//! profile, service or network is used.
#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Sandbox {
    root: PathBuf,
    prefix: PathBuf,
    home: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-maintenance-cli-{}-{serial}-{nanos}",
            std::process::id()
        ));
        let prefix = root.join("runtime prefix");
        let home = root.join("private-home");
        fs::create_dir_all(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root, prefix, home }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(arguments)
            .env("HOME", &self.home)
            .env("DSH_OCEAN_DATA_DIR", &self.prefix)
            .env_remove("DSH_OCEAN_URL")
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("DSH_HOME", self.home.join("runtime-not-created"))
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
            .output()
            .expect("run the packaged client CLI")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.root.starts_with(std::env::temp_dir())
            && self.root.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("dsh-ocean-maintenance-cli-")
            })
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn check_mode_reports_the_private_runtime_without_creating_user_state() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--runtime-maintenance", "check"]);
    let text = stdout(&output);
    assert!(output.status.success(), "{text}{}", stderr(&output));
    assert!(text.contains("[bootstrap] check: ok"), "{text}");
    assert!(text.contains("prefix:"), "{text}");
    // The helper creates only the prefix directory it was pointed at.
    assert!(sandbox.prefix.is_dir());
    let home_entries: Vec<_> = fs::read_dir(&sandbox.home)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        home_entries.is_empty(),
        "unexpected HOME writes: {home_entries:?}"
    );
    assert!(!Path::new("/nonexistent").exists());
}

#[test]
fn collect_mode_is_a_no_op_without_generations() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--runtime-maintenance", "gc"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("no runtime generations"));
}

#[test]
fn rollback_requires_confirmation_and_refuses_without_a_selected_runtime() {
    let sandbox = Sandbox::new();
    let refused = sandbox.run(&["--runtime-maintenance", "rollback"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("--confirm-maintenance"),
        "{}",
        stderr(&refused)
    );
    let confirmed = sandbox.run(&["--runtime-maintenance", "rollback", "--confirm-maintenance"]);
    assert!(!confirmed.status.success());
    assert!(
        stderr(&confirmed).contains("no generation selected to roll back"),
        "{}",
        stderr(&confirmed)
    );
}

#[test]
fn rollback_refuses_invalid_configuration_before_starting_a_helper() {
    let sandbox = Sandbox::new();
    let config = sandbox.home.join("config/dsh-ocean/config.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    for text in [
        "{private-secret",
        r#"{"serverUrl":"https://remote.invalid/"}"#,
    ] {
        fs::write(&config, text).unwrap();
        let output = sandbox.run(&["--runtime-maintenance", "rollback", "--confirm-maintenance"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(!stderr(&output).contains("usage:"));
        assert!(!stderr(&output).contains("private-secret"));
        assert!(stdout(&output).is_empty());
        assert!(
            !sandbox.prefix.exists(),
            "rollback helper must not start on invalid configuration"
        );
    }
}

#[test]
fn unknown_or_malformed_modes_are_refused_before_any_work() {
    let sandbox = Sandbox::new();
    for arguments in [
        vec!["--runtime-maintenance"],
        vec!["--runtime-maintenance", "install"],
        vec!["--runtime-maintenance", "check", "gc"],
        vec!["--runtime-maintenance", "check", "--confirm-maintenance"],
    ] {
        let output = sandbox.run(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(stderr(&output).contains("usage:"), "{arguments:?}");
        assert!(stdout(&output).is_empty(), "{arguments:?}");
        assert!(
            !sandbox.prefix.exists(),
            "a refused mode must not create the prefix: {arguments:?}"
        );
    }
}

#[test]
fn help_lists_the_maintenance_mode() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--help"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("--runtime-maintenance"));
}

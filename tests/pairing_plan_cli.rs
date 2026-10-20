//! `--pairing-plan` acceptance: read-only, no config or service changes.
#![cfg(target_os = "linux")]
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct Sandbox {
    root: PathBuf,
    config: PathBuf,
}

impl Sandbox {
    fn new(server_url: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self::at_time(server_url, nanos)
    }

    fn at_time(server_url: &str, nanos: u128) -> Self {
        // Wall-clock samples can be equal on different test threads. Reserve an
        // exclusive root before populating it, so Drop can never remove a peer.
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let root = (0..100)
            .find_map(|_| {
                let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "dsh-ocean-pairing-plan-{}-{nanos}-{serial}",
                    std::process::id()
                ));
                match fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => Some(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!("reserve private fixture directory: {error}"),
                }
            })
            .expect("reserve a unique fixture root");
        let config = root.join("config/dsh-ocean/config.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            &config,
            format!("{{\"serverUrl\":\"{server_url}\",\"zoom\":1.0}}\n"),
        )
        .unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        // A forbidden-tool directory proves nothing is executed.
        let tools = root.join("bin");
        fs::create_dir_all(&tools).unwrap();
        for name in ["systemctl", "ip", "nft", "iptables", "fwupdmgr", "curl"] {
            let path = tools.join(name);
            fs::write(
                &path,
                "#!/bin/sh\nprintf forbidden >> \"$FORBIDDEN_LOG\"\nexit 90\n",
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { root, config }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(arguments)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("DSH_OCEAN_DATA_DIR", self.root.join("runtime"))
            .env("FORBIDDEN_LOG", self.root.join("forbidden-log"))
            .env("PATH", self.root.join("bin"))
            .output()
            .expect("run the client CLI")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.root.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with("dsh-ocean-pairing-plan-")
        }) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).expect("pairing plan JSON")
}

#[test]
fn identical_clock_samples_never_share_fixture_ownership() {
    let timestamp = 42;
    let fixtures: Vec<_> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..3)
            .map(|_| scope.spawn(|| Sandbox::at_time("http://127.0.0.1:3080/", timestamp)))
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    });
    let roots: std::collections::HashSet<_> = fixtures.iter().map(|f| &f.root).collect();
    assert_eq!(roots.len(), fixtures.len());
    let mut fixtures = fixtures.into_iter();
    let survivor = fixtures.next().unwrap();
    let before = fs::read(&survivor.config).unwrap();
    drop(fixtures); // The old same-PID/same-time roots deleted the survivor here.
    assert_eq!(fs::read(&survivor.config).unwrap(), before);
    assert!(survivor.run(&["--pairing-plan"]).status.success());
}

#[test]
fn loopback_configuration_gets_a_plan_without_any_side_effect() {
    let sandbox = Sandbox::new("http://127.0.0.1:3080/");
    let before = fs::read(&sandbox.config).unwrap();
    let output = sandbox.run(&["--pairing-plan"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan = json(&output);
    assert_eq!(plan["loopback"], true);
    assert_eq!(plan["changedAnything"], false);
    assert_eq!(
        plan["exposure"],
        "loopback-only: currently unreachable from another device"
    );
    let warnings = plan["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|warning| warning
        .as_str()
        .is_some_and(|text| text.contains("full control"))));
    assert!(warnings.iter().any(|warning| warning
        .as_str()
        .is_some_and(|text| text.contains("read-only"))));
    // The LAN address may or may not be detectable in a sandbox, but the plan
    // must never emit a half-written command.
    match plan["suggestedHostOverride"].as_str() {
        Some(command) => {
            assert!(
                command.starts_with("--host 0.0.0.0 --trusted-host "),
                "{command}"
            );
            assert!(plan["suggestedClientUrl"]
                .as_str()
                .is_some_and(|url| url.starts_with("http://") && url.ends_with(":3080/")));
        }
        None => assert!(warnings.iter().any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("could be detected")))),
    }
    // Read-only: config unchanged, no helper executed, no token anywhere.
    assert_eq!(fs::read(&sandbox.config).unwrap(), before);
    assert!(!sandbox.root.join("forbidden-log").exists());
    let rendered = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(!rendered.contains("token="));
}

#[test]
fn non_loopback_configuration_reports_revocation_guidance() {
    let sandbox = Sandbox::new("http://10.1.2.3:3080/");
    let output = sandbox.run(&["--pairing-plan"]);
    assert!(output.status.success());
    let plan = json(&output);
    assert_eq!(plan["loopback"], false);
    assert!(plan["suggestedHostOverride"].is_null());
    assert!(plan["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().is_some_and(|text| text.contains("revoke"))));
    assert!(!sandbox.root.join("forbidden-log").exists());
}

#[test]
fn unexpected_arguments_are_refused() {
    let sandbox = Sandbox::new("http://127.0.0.1:3080/");
    let output = sandbox.run(&["--pairing-plan", "extra"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("accepts no arguments"));
}

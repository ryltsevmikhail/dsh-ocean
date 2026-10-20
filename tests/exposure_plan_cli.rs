//! `--prepare-exposure` acceptance: staging only, no service operations.
#![cfg(target_os = "linux")]
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-exposure-cli-{}-{nanos}",
            std::process::id()
        ));
        for directory in ["home", "state", "bin", "runtime"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        // Any attempt to run the unit manager leaves a marker behind.
        let marker = root.join("systemctl-called");
        fs::write(
            root.join("bin/systemctl"),
            format!("#!/bin/bash\ntouch '{}'\nexit 3\n", marker.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(root.join("bin/systemctl"))
            .unwrap()
            .permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(root.join("bin/systemctl"), permissions).unwrap();
        Self { root }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.run_with_origin(arguments, Some("http://127.0.0.1:3080/"))
    }

    fn run_with_origin(&self, arguments: &[&str], origin: Option<&str>) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(arguments)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("DSH_HOME", self.root.join("dsh"))
            .env("DSH_OCEAN_DATA_DIR", self.root.join("runtime"))
            .envs(origin.map(|value| ("DSH_OCEAN_URL", value)))
            .env("PATH", self.root.join("bin"))
            .output()
            .expect("run the client CLI")
    }

    fn manager_called(&self) -> bool {
        self.root.join("systemctl-called").exists()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.root.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with("dsh-ocean-exposure-cli-")
        }) {
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

#[test]
fn public_and_loopback_addresses_are_refused_before_anything_is_staged() {
    let sandbox = Sandbox::new();
    for address in ["8.8.8.8", "127.0.0.1", "169.254.9.9", "::1", "0.0.0.0"] {
        let output = sandbox.run(&["--prepare-exposure", address]);
        assert!(!output.status.success(), "{address} was accepted");
        assert!(
            describe(&output).contains("private LAN address is required"),
            "{address}: {}",
            describe(&output)
        );
        assert!(!sandbox.manager_called());
        assert!(!sandbox.root.join("state/dsh-ocean/exposure").exists());
    }
}

#[test]
fn without_an_installed_unit_nothing_is_staged_and_no_service_command_runs() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--prepare-exposure", "192.168.1.50"]);
    assert!(!output.status.success());
    assert!(
        describe(&output).contains("install the service first"),
        "{}",
        describe(&output)
    );
    assert!(
        !sandbox.manager_called(),
        "the client must never call systemctl"
    );
    assert!(!sandbox.root.join("state/dsh-ocean/exposure").exists());
}

#[test]
fn installed_bytes_are_preserved_without_detecting_or_changing_a_runtime() {
    let sandbox = Sandbox::new();
    let installed = sandbox.root.join("config/systemd/user/dsh-web.service");
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    let original = "[Service]\nEnvironment=PATH=/custom/bin\nExecStart=/custom/node /custom/dsh --profile web --host 127.0.0.1 --port 3080 --no-open\nRestart=always\n";
    fs::write(&installed, original).unwrap();
    let output = sandbox.run(&["--prepare-exposure", "192.168.1.50"]);
    assert!(output.status.success(), "{}", describe(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let staged = PathBuf::from(value["stagedUnit"].as_str().unwrap());
    let backup = staged.parent().unwrap().join("dsh-web.installed.service");
    assert_eq!(fs::read_to_string(&backup).unwrap(), original);
    assert_eq!(fs::read_to_string(&installed).unwrap(), original);
    assert_eq!(
        fs::read_to_string(staged).unwrap(),
        original.replace(
            "--host 127.0.0.1 --port 3080",
            "--host 0.0.0.0 --port 3080 --trusted-host 192.168.1.50:3080"
        )
    );
    assert!(!sandbox.manager_called());

    let mismatched = original.replace("--port 3080", "--port 9999");
    fs::write(&installed, &mismatched).unwrap();
    let failed = sandbox.run(&["--prepare-exposure", "192.168.1.50"]);
    assert!(!failed.status.success(), "{}", describe(&failed));
    assert_eq!(fs::read_to_string(backup).unwrap(), original);
    assert_eq!(fs::read_to_string(installed).unwrap(), mismatched);
    assert!(!sandbox.manager_called());
}

#[test]
fn invalid_existing_configuration_never_stages_a_local_exposure() {
    let sandbox = Sandbox::new();
    let config = sandbox.root.join("config/dsh-ocean/config.json");
    let installed = sandbox.root.join("config/systemd/user/dsh-web.service");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    let original =
        "[Service]\nExecStart=/custom/node /custom/dsh --host 127.0.0.1 --port 3080 --no-open\n";
    fs::write(&installed, original).unwrap();
    for text in [
        r#"{"serverUrl":"https://remote.invalid/","closeToTray":"false"}"#,
        "{private-secret",
    ] {
        fs::write(&config, text).unwrap();
        let output = sandbox.run_with_origin(&["--prepare-exposure", "192.168.1.50"], None);
        assert!(!output.status.success(), "{}", describe(&output));
        assert!(describe(&output).contains("configuration is invalid"));
        assert!(!describe(&output).contains("private-secret"));
        assert!(!sandbox.root.join("state/dsh-ocean/exposure").exists());
        assert_eq!(fs::read_to_string(&installed).unwrap(), original);
        assert!(!sandbox.manager_called());
    }
}

#[test]
fn malformed_arguments_are_refused() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--prepare-exposure", "not-an-ip"]);
    assert!(!output.status.success());
    assert!(
        describe(&output).contains("IP address"),
        "{}",
        describe(&output)
    );
    let extra = sandbox.run(&["--prepare-exposure", "10.0.0.1", "extra"]);
    assert!(!extra.status.success());
    assert!(describe(&extra).contains("usage"), "{}", describe(&extra));
}

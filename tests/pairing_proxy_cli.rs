//! `--pairing-proxy` acceptance: bounded lifetime, loopback-only target, no service.
#![cfg(target_os = "linux")]
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
            "dsh-ocean-pairing-proxy-{}-{nanos}",
            std::process::id()
        ));
        for directory in ["home", "state", "config", "data", "run"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        Self { root }
    }

    fn run(&self, arguments: &[&str], url: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(arguments)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("DSH_HOME", self.root.join("dsh"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("DSH_OCEAN_URL", url)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run the client CLI")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.root.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with("dsh-ocean-pairing-proxy-")
        }) {
            let _ = std::fs::remove_dir_all(&self.root);
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
fn check_mode_binds_a_temporary_forwarder_and_reports_state() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(
        &["--pairing-proxy", "--ttl-seconds", "1", "--check"],
        "http://127.0.0.1:3080/",
    );
    assert!(output.status.success(), "{}", describe(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let status = &json["status"];
    assert_eq!(status["target"], "127.0.0.1:3080");
    assert_eq!(status["ttlSeconds"], 1);
    assert_eq!(status["accepted"], 0);
    assert_eq!(status["revoked"], true);
    assert!(status["listen"].as_str().unwrap().starts_with("127.0.0.1:"));
    assert_eq!(json["checked"], true);
    assert!(json["warning"]
        .as_str()
        .unwrap()
        .contains("no authentication"));
    // The check never prints a token or a LAN URL.
    let rendered = describe(&output);
    assert!(!rendered.contains("token="), "{rendered}");
    assert!(!rendered.contains("0.0.0.0"), "{rendered}");
}

#[test]
fn non_loopback_targets_are_refused() {
    let sandbox = Sandbox::new();
    for url in [
        "http://192.168.1.50:3080/",
        "https://example.com/",
        "http://0.0.0.0:3080/",
        "http://127.0.0.1:3080/?token=private-secret",
        "http://private-secret@127.0.0.1:3080/",
        "not-a-url-private-secret",
    ] {
        let output = sandbox.run(&["--pairing-proxy", "--ttl-seconds", "1", "--check"], url);
        assert!(!output.status.success(), "{url} was accepted");
        assert!(!describe(&output).contains("private-secret"));
        assert!(
            describe(&output).contains("not loopback")
                || describe(&output).contains("invalid configured server origin"),
            "{url}: {}",
            describe(&output)
        );
    }
}

#[test]
fn argument_mistakes_are_refused_before_binding() {
    let sandbox = Sandbox::new();
    let missing = sandbox.run(&["--pairing-proxy", "--check"], "http://127.0.0.1:3080/");
    assert!(!missing.status.success());
    assert!(
        describe(&missing).contains("usage"),
        "{}",
        describe(&missing)
    );
    let unknown = sandbox.run(
        &["--pairing-proxy", "--ttl-seconds", "1", "--verbose"],
        "http://127.0.0.1:3080/",
    );
    assert!(!unknown.status.success());
    assert!(
        describe(&unknown).contains("unknown"),
        "{}",
        describe(&unknown)
    );
}

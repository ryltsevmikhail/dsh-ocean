//! End-to-end acceptance of persisted release anti-rollback state.
//!
//! Manifests are signed locally with a throwaway Ed25519 key created by the test;
//! nothing here trusts a publisher, contacts a network or installs anything.
#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration, Utc};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::Value;

use dsh_ocean::release_feed::hex;

const DOMAIN: &[u8] = b"DSH Ocean release manifest v1\n";
const KEY_ID: &str = "synthetic-test-key";

struct Sandbox {
    root: PathBuf,
    key: SigningKey,
    published: String,
    expires: String,
}

impl Sandbox {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-release-state-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("home")).unwrap();
        fs::set_permissions(root.join("home"), fs::Permissions::from_mode(0o700)).unwrap();
        // A fixed seed keeps failures reproducible without real key material.
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let now = Utc::now();
        Self {
            root,
            key,
            published: (now - Duration::days(1)).to_rfc3339(),
            expires: (now + Duration::days(20)).to_rfc3339(),
        }
    }

    fn key_file(&self) -> PathBuf {
        let path = self.root.join("trusted-key.json");
        let body = format!(
            "{{\"version\":1,\"keyId\":\"{KEY_ID}\",\"publicKeyHex\":\"{}\"}}",
            hex(self.key.verifying_key().as_bytes())
        );
        fs::write(&path, body).unwrap();
        path
    }

    fn signed(&self, name: &str, sequence: u64, release: &str, digest_seed: u8) -> [PathBuf; 2] {
        let manifest = format!(
            "{{\"version\":1,\"product\":\"dsh-ocean\",\"channel\":\"stable\",\"sequence\":{sequence},\"release\":\"{release}\",\"publishedAt\":\"{}\",\"expiresAt\":\"{}\",\"artifacts\":[{{\"target\":\"linux-x86_64\",\"kind\":\"appimage\",\"url\":\"https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v{release}/dsh-ocean-{release}-x86_64.AppImage\",\"size\":10,\"sha256\":\"{}\"}}]}}",
            self.published,
            self.expires,
            format!("{digest_seed:02x}").repeat(32)
        );
        let manifest_path = self.root.join(format!("{name}-manifest.json"));
        fs::write(&manifest_path, manifest.as_bytes()).unwrap();
        let mut message = DOMAIN.to_vec();
        message.extend_from_slice(manifest.as_bytes());
        let signature = self.key.sign(&message);
        let signature_path = self.root.join(format!("{name}-signature.json"));
        fs::write(
            &signature_path,
            format!(
                "{{\"version\":1,\"keyId\":\"{KEY_ID}\",\"signatureHex\":\"{}\"}}",
                hex(&signature.to_bytes())
            ),
        )
        .unwrap();
        [manifest_path, signature_path]
    }

    fn policy(&self, sequence_floor: &str) -> Vec<String> {
        vec![
            "--channel".into(),
            "stable".into(),
            "--target".into(),
            "linux-x86_64".into(),
            "--kind".into(),
            "appimage".into(),
            "--min-sequence".into(),
            sequence_floor.into(),
        ]
    }

    fn invoke(&self, command: &str, manifest: &Path, signature: &Path, floor: &str) -> Output {
        let key = self.key_file();
        let mut arguments = vec![
            command.to_owned(),
            manifest.to_string_lossy().into_owned(),
            signature.to_string_lossy().into_owned(),
            key.to_string_lossy().into_owned(),
        ];
        arguments.extend(self.policy(floor));
        self.run(&arguments)
    }

    fn run(&self, arguments: &[String]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(arguments)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("PATH", "/usr/bin:/bin")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("run the client CLI");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("release-state CLI exceeded its bounded timeout");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn state_file(&self) -> PathBuf {
        self.root.join("state/dsh-ocean/release-state.json")
    }

    fn state(&self) -> Value {
        let output = self.run(&["--release-state".to_owned()]);
        assert!(output.status.success());
        serde_json::from_slice(&output.stdout).expect("release state JSON")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.root.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with("dsh-ocean-release-state-")
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
fn plain_verification_stays_read_only_and_accept_records_once() {
    let sandbox = Sandbox::new();
    let [manifest, signature] = sandbox.signed("newer", 5, "0.2.0", 1);

    let verified = sandbox.invoke("--verify-release", &manifest, &signature, "1");
    assert!(verified.status.success(), "{}", describe(&verified));
    let body: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert!(body["remembered"].is_null());
    assert!(!sandbox.state_file().exists(), "verify must not remember");

    let accepted = sandbox.invoke("--accept-release", &manifest, &signature, "1");
    assert!(accepted.status.success(), "{}", describe(&accepted));
    let body: Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(body["remembered"]["sequence"], 5);
    assert_eq!(body["remembered"]["release"], "0.2.0");
    assert_eq!(body["applied"], false);
    let state = sandbox.state();
    assert_eq!(state["channels"]["stable"]["sequence"], 5);
    let mode = fs::metadata(sandbox.state_file())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "state must stay private");

    // Accepting the same manifest again is idempotent.
    let again = sandbox.invoke("--accept-release", &manifest, &signature, "1");
    assert!(again.status.success(), "{}", describe(&again));
    assert_eq!(sandbox.state()["channels"]["stable"]["sequence"], 5);
}

#[test]
fn older_sequences_conflicting_manifests_and_downgrades_are_refused() {
    let sandbox = Sandbox::new();
    let [newer, newer_sig] = sandbox.signed("newer", 5, "0.2.0", 1);
    assert!(sandbox
        .invoke("--accept-release", &newer, &newer_sig, "1")
        .status
        .success());
    let before = fs::read(sandbox.state_file()).unwrap();

    let [older, older_sig] = sandbox.signed("older", 4, "0.3.0", 2);
    let replay = sandbox.invoke("--accept-release", &older, &older_sig, "1");
    assert!(!replay.status.success());
    let text = describe(&replay);
    assert!(text.contains("rollback"), "{text}");
    assert_eq!(fs::read(sandbox.state_file()).unwrap(), before);

    let [conflict, conflict_sig] = sandbox.signed("conflict", 5, "0.2.0", 3);
    let mismatched = sandbox.invoke("--accept-release", &conflict, &conflict_sig, "1");
    assert!(!mismatched.status.success());
    assert!(
        describe(&mismatched).contains("different manifest"),
        "{}",
        describe(&mismatched)
    );
    assert_eq!(fs::read(sandbox.state_file()).unwrap(), before);

    let [downgrade, downgrade_sig] = sandbox.signed("downgrade", 6, "0.1.0", 4);
    let refused = sandbox.invoke("--accept-release", &downgrade, &downgrade_sig, "1");
    assert!(!refused.status.success());
    assert!(
        describe(&refused).contains("downgrade"),
        "{}",
        describe(&refused)
    );
    assert_eq!(fs::read(sandbox.state_file()).unwrap(), before);

    // A genuinely newer release is still accepted.
    let [newest, newest_sig] = sandbox.signed("newest", 7, "0.2.1", 5);
    let accepted = sandbox.invoke("--accept-release", &newest, &newest_sig, "1");
    assert!(accepted.status.success(), "{}", describe(&accepted));
    assert_eq!(sandbox.state()["channels"]["stable"]["sequence"], 7);
}

#[test]
fn reset_requires_confirmation_and_forgets_the_remembered_sequence() {
    let sandbox = Sandbox::new();
    let [newer, newer_sig] = sandbox.signed("newer", 5, "0.2.0", 1);
    assert!(sandbox
        .invoke("--accept-release", &newer, &newer_sig, "1")
        .status
        .success());

    let unconfirmed = sandbox.run(&["--reset-release-state".to_owned()]);
    assert!(!unconfirmed.status.success());
    assert!(sandbox.state_file().exists());

    let reset = sandbox.run(&[
        "--reset-release-state".to_owned(),
        "--confirm-reset".to_owned(),
    ]);
    assert!(reset.status.success(), "{}", describe(&reset));
    assert!(!sandbox.state_file().exists());
    assert!(sandbox.state()["channels"].as_object().unwrap().is_empty());

    // Reset removes the remembered sequence floor, not the running binary's
    // release-version floor. Use this version so a package version bump does not
    // accidentally turn a state-reset test into a legitimate downgrade refusal.
    let [older, older_sig] = sandbox.signed("older", 4, env!("CARGO_PKG_VERSION"), 2);
    let accepted = sandbox.invoke("--accept-release", &older, &older_sig, "1");
    assert!(accepted.status.success(), "{}", describe(&accepted));
    assert_eq!(sandbox.state()["channels"]["stable"]["sequence"], 4);
}

#[test]
fn fifo_state_is_rejected_without_waiting_for_a_writer() {
    use std::os::unix::ffi::OsStrExt;
    let sandbox = Sandbox::new();
    let path = sandbox.state_file();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let reported = sandbox.run(&["--release-state".to_owned()]);
    assert!(!reported.status.success());
    assert!(describe(&reported).contains("bounded regular file"));
    let [manifest, signature] = sandbox.signed("fifo", 5, "0.2.0", 1);
    let accepted = sandbox.invoke("--accept-release", &manifest, &signature, "1");
    assert!(!accepted.status.success());
    assert!(describe(&accepted).contains("bounded regular file"));
    assert!(fs::symlink_metadata(path).unwrap().file_type().is_fifo());
}

#[test]
fn corrupted_state_is_reported_instead_of_silently_ignored() {
    let sandbox = Sandbox::new();
    let [newer, newer_sig] = sandbox.signed("newer", 5, "0.2.0", 1);
    assert!(sandbox
        .invoke("--accept-release", &newer, &newer_sig, "1")
        .status
        .success());
    fs::write(sandbox.state_file(), b"{ not json").unwrap();
    let reported = sandbox.run(&["--release-state".to_owned()]);
    assert!(!reported.status.success());
    assert!(describe(&reported).contains("invalid release state file"));
    // A corrupted store must block acceptance rather than reset the floor.
    let accepted = sandbox.invoke("--accept-release", &newer, &newer_sig, "1");
    assert!(!accepted.status.success());
}

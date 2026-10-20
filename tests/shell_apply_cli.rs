//! End-to-end acceptance of verified release activation and rollback.
//!
//! Signed with a throwaway key, applied into a private temporary prefix, with no
//! service, download or user state involved.
#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration, Utc};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

use dsh_ocean::release_feed::hex;

const DOMAIN: &[u8] = b"DSH Ocean release manifest v1\n";
const KEY_ID: &str = "synthetic-apply-key";

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
            "dsh-ocean-shell-apply-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("home")).unwrap();
        fs::set_permissions(root.join("home"), fs::Permissions::from_mode(0o700)).unwrap();
        let key = SigningKey::from_bytes(&[11u8; 32]);
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
        fs::write(
            &path,
            format!(
                "{{\"version\":1,\"keyId\":\"{KEY_ID}\",\"publicKeyHex\":\"{}\"}}",
                hex(self.key.verifying_key().as_bytes())
            ),
        )
        .unwrap();
        path
    }

    /// Writes a release artifact and a manifest/signature pair describing it.
    fn release(
        &self,
        name: &str,
        sequence: u64,
        version: &str,
        bytes: &[u8],
    ) -> (PathBuf, PathBuf, PathBuf) {
        let artifact = self.root.join(format!("{name}.AppImage"));
        fs::write(&artifact, bytes).unwrap();
        let digest = hex(&Sha256::digest(bytes));
        let manifest = format!(
            "{{\"version\":1,\"product\":\"dsh-ocean\",\"channel\":\"stable\",\"sequence\":{sequence},\"release\":\"{version}\",\"publishedAt\":\"{}\",\"expiresAt\":\"{}\",\"artifacts\":[{{\"target\":\"linux-x86_64\",\"kind\":\"appimage\",\"url\":\"https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v{version}/dsh-ocean-{version}-x86_64.AppImage\",\"size\":{},\"sha256\":\"{digest}\"}}]}}",
            self.published,
            self.expires,
            bytes.len()
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
        (manifest_path, signature_path, artifact)
    }

    fn run(&self, arguments: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(arguments)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run the client CLI")
    }

    fn apply_args(
        &self,
        manifest: &Path,
        signature: &Path,
        artifact: &Path,
        prefix: &Path,
        confirm: bool,
    ) -> Vec<String> {
        let mut arguments = vec![
            "--apply-release".to_owned(),
            manifest.to_string_lossy().into_owned(),
            signature.to_string_lossy().into_owned(),
            self.key_file().to_string_lossy().into_owned(),
            "--channel".to_owned(),
            "stable".to_owned(),
            "--target".to_owned(),
            "linux-x86_64".to_owned(),
            "--kind".to_owned(),
            "appimage".to_owned(),
            "--min-sequence".to_owned(),
            "1".to_owned(),
            "--artifact".to_owned(),
            artifact.to_string_lossy().into_owned(),
            "--prefix".to_owned(),
            prefix.to_string_lossy().into_owned(),
        ];
        if confirm {
            arguments.push("--confirm-apply".to_owned());
            arguments.push("yes".to_owned());
        }
        arguments
    }

    fn state(&self) -> serde_json::Value {
        let output = self.run(&["--release-state".to_owned()]);
        assert!(output.status.success());
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self
            .root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("dsh-ocean-shell-apply-"))
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

fn current_target(prefix: &Path) -> String {
    fs::read_link(prefix.join("current"))
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn applies_a_verified_release_then_rolls_back_to_the_retained_previous() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root.join("install prefix");
    let (first_manifest, first_sig, first_artifact) =
        sandbox.release("first", 5, "0.2.0", b"synthetic-appimage-first");
    let applied = sandbox.run(&sandbox.apply_args(
        &first_manifest,
        &first_sig,
        &first_artifact,
        &prefix,
        true,
    ));
    assert!(applied.status.success(), "{}", describe(&applied));
    let body: serde_json::Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(body["apply"]["version"], "0.2.0");
    assert_eq!(body["apply"]["serviceRestarted"], false);
    assert_eq!(body["remembered"]["sequence"], 5);
    let target = current_target(&prefix);
    assert!(target.starts_with("versions/0.2.0-5-"), "{target}");
    let binary = prefix.join("current").join("dsh-ocean");
    assert_eq!(
        fs::read(&binary).unwrap(),
        b"synthetic-appimage-first",
        "activated bytes must match the verified artifact"
    );
    assert_eq!(
        fs::metadata(&binary).unwrap().permissions().mode() & 0o111,
        0o111,
        "activated artifact must be executable"
    );
    assert_eq!(sandbox.state()["channels"]["stable"]["sequence"], 5);
    assert!(!prefix.join(".previous").exists());

    // Applying a newer release keeps the previous version retained.
    let (second_manifest, second_sig, second_artifact) =
        sandbox.release("second", 6, "0.2.1", b"synthetic-appimage-second");
    let upgraded = sandbox.run(&sandbox.apply_args(
        &second_manifest,
        &second_sig,
        &second_artifact,
        &prefix,
        true,
    ));
    assert!(upgraded.status.success(), "{}", describe(&upgraded));
    assert!(current_target(&prefix).starts_with("versions/0.2.1-6-"));
    let previous = fs::read_link(prefix.join(".previous"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(previous, target);
    assert!(prefix.join(&previous).join("dsh-ocean").is_file());
    assert_eq!(sandbox.state()["channels"]["stable"]["sequence"], 6);

    let rolled = sandbox.run(&[
        "--rollback-shell".to_owned(),
        "--prefix".to_owned(),
        prefix.to_string_lossy().into_owned(),
        "--confirm-rollback".to_owned(),
        "yes".to_owned(),
    ]);
    assert!(rolled.status.success(), "{}", describe(&rolled));
    assert_eq!(current_target(&prefix), target);
    assert_eq!(
        fs::read(prefix.join("current").join("dsh-ocean")).unwrap(),
        b"synthetic-appimage-first"
    );
    // The version we rolled back from is now the retained previous one.
    assert!(fs::read_link(prefix.join(".previous"))
        .unwrap()
        .to_string_lossy()
        .starts_with("versions/0.2.1-6-"));
}

#[test]
fn refused_operations_leave_the_prefix_and_state_untouched() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root.join("prefix");
    let (manifest, signature, artifact) =
        sandbox.release("first", 5, "0.2.0", b"synthetic-appimage-first");

    // Missing confirmation.
    let unconfirmed =
        sandbox.run(&sandbox.apply_args(&manifest, &signature, &artifact, &prefix, false));
    assert!(!unconfirmed.status.success());
    assert!(!prefix.exists());
    assert!(sandbox.state()["channels"].as_object().unwrap().is_empty());

    // Wrong artifact bytes.
    let tampered = sandbox.root.join("tampered.AppImage");
    fs::write(&tampered, b"different bytes").unwrap();
    let refused = sandbox.run(&sandbox.apply_args(&manifest, &signature, &tampered, &prefix, true));
    assert!(!refused.status.success());
    let message = describe(&refused);
    assert!(
        message.contains("SHA-256 mismatch") || message.contains("size mismatch"),
        "{message}"
    );
    assert!(!prefix.join("current").exists());
    assert!(sandbox.state()["channels"].as_object().unwrap().is_empty());

    // Successful apply of sequence 5, then an older sequence is refused.
    assert!(sandbox
        .run(&sandbox.apply_args(&manifest, &signature, &artifact, &prefix, true))
        .status
        .success());
    let (old_manifest, old_sig, old_artifact) =
        sandbox.release("old", 4, "0.3.0", b"synthetic-appimage-old");
    let replay =
        sandbox.run(&sandbox.apply_args(&old_manifest, &old_sig, &old_artifact, &prefix, true));
    assert!(!replay.status.success());
    assert!(
        describe(&replay).contains("rollback"),
        "{}",
        describe(&replay)
    );
    assert!(current_target(&prefix).starts_with("versions/0.2.0-5-"));

    // A world-writable prefix is refused.
    let loose = sandbox.root.join("loose");
    fs::create_dir_all(&loose).unwrap();
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o777)).unwrap();
    let private = sandbox.run(&sandbox.apply_args(&manifest, &signature, &artifact, &loose, true));
    assert!(!private.status.success());
    assert!(
        describe(&private).contains("not writable by others"),
        "{}",
        describe(&private)
    );
}

#[test]
fn rollback_without_a_previous_version_is_refused() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root.join("prefix");
    let (manifest, signature, artifact) =
        sandbox.release("first", 5, "0.2.0", b"synthetic-appimage-first");
    assert!(sandbox
        .run(&sandbox.apply_args(&manifest, &signature, &artifact, &prefix, true))
        .status
        .success());
    let refused = sandbox.run(&[
        "--rollback-shell".to_owned(),
        "--prefix".to_owned(),
        prefix.to_string_lossy().into_owned(),
        "--confirm-rollback".to_owned(),
        "yes".to_owned(),
    ]);
    assert!(!refused.status.success());
    assert!(
        describe(&refused).contains("no previous activation"),
        "{}",
        describe(&refused)
    );
    assert!(current_target(&prefix).starts_with("versions/0.2.0-5-"));
}

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
            .env(
                "PATH",
                format!("{}/tools:/usr/bin:/bin", self.root.display()),
            )
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

    fn download_args(&self, manifest: &Path, signature: &Path, output: &Path) -> Vec<String> {
        vec![
            "--download-release".into(),
            manifest.to_string_lossy().into_owned(),
            signature.to_string_lossy().into_owned(),
            self.key_file().to_string_lossy().into_owned(),
            "--channel".into(),
            "stable".into(),
            "--target".into(),
            "linux-x86_64".into(),
            "--kind".into(),
            "appimage".into(),
            "--min-sequence".into(),
            "1".into(),
            "--output".into(),
            output.to_string_lossy().into_owned(),
        ]
    }

    // Trusted downloader stand-in: no real HTTP, credentials, profile or service.
    fn fake_curl(&self, bytes: &[u8], exit: i32) {
        let tools = self.root.join("tools");
        fs::create_dir_all(&tools).unwrap();
        let script = format!(
            "#!/usr/bin/python3\nimport os,sys\nfrom pathlib import Path\np=Path(__file__).parent\n(p/'curl-arguments').write_text('\\n'.join(sys.argv[1:]))\n(p/'curl-pid').write_text(str(os.getpid()))\nsys.stderr.write('private-downloader-marker')\nsys.stdout.buffer.write(bytes.fromhex('{}'))\nsys.stdout.buffer.flush()\nsys.exit({exit})\n",
            hex(bytes)
        );
        fs::write(tools.join("curl"), script).unwrap();
        fs::set_permissions(tools.join("curl"), fs::Permissions::from_mode(0o700)).unwrap();
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
fn downloads_signed_bytes_without_executing_then_applies_and_rolls_back() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root.join("prefix");
    let mut first_target = String::new();
    for (name, sequence, version, bytes) in [
        ("first", 5, "0.2.0", b"synthetic-appimage-first".as_slice()),
        (
            "second",
            6,
            "0.2.1",
            b"synthetic-appimage-second".as_slice(),
        ),
    ] {
        let (manifest, signature, _) = sandbox.release(name, sequence, version, bytes);
        sandbox.fake_curl(bytes, 0);
        let output = sandbox.root.join(format!("{name}-download.AppImage"));
        let downloaded = sandbox.run(&sandbox.download_args(&manifest, &signature, &output));
        assert!(downloaded.status.success(), "{}", describe(&downloaded));
        let receipt: serde_json::Value = serde_json::from_slice(&downloaded.stdout).unwrap();
        assert_eq!(receipt["downloaded"], true);
        assert_eq!(receipt["artifactChecked"], true);
        assert_eq!(receipt["applied"], false);
        assert_eq!(fs::read(&output).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!String::from_utf8_lossy(&downloaded.stderr).contains("private-downloader-marker"));
        if sequence == 5 {
            assert!(!prefix.exists());
            assert!(sandbox.state()["channels"].as_object().unwrap().is_empty());
        }
        let curl = fs::read_to_string(sandbox.root.join("tools/curl-arguments")).unwrap();
        let args: Vec<_> = curl.lines().collect();
        assert_eq!(args[0], "--disable", "never load .curlrc");
        for pair in [
            ["--proto", "=https"],
            ["--proto-redir", "=https"],
            ["--max-redirs", "5"],
            ["--connect-timeout", "15"],
            ["--max-time", "180"],
            ["--speed-limit", "1024"],
            ["--speed-time", "30"],
        ] {
            assert!(
                args.windows(2).any(|actual| actual == pair),
                "missing curl bound: {pair:?}"
            );
        }
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--max-filesize", &bytes.len().to_string()]));
        let applied =
            sandbox.run(&sandbox.apply_args(&manifest, &signature, &output, &prefix, true));
        assert!(applied.status.success(), "{}", describe(&applied));
        if sequence == 5 {
            first_target = current_target(&prefix);
        }
    }
    let rolled = sandbox.run(&[
        "--rollback-shell".into(),
        "--prefix".into(),
        prefix.to_string_lossy().into_owned(),
        "--confirm-rollback".into(),
        "yes".into(),
    ]);
    assert!(rolled.status.success(), "{}", describe(&rolled));
    assert_eq!(current_target(&prefix), first_target);
    assert_eq!(
        fs::read(prefix.join("current/dsh-ocean")).unwrap(),
        b"synthetic-appimage-first"
    );
    assert_eq!(
        sandbox.state()["channels"]["stable"]["sequence"],
        6,
        "rollback must not forget the replay floor"
    );
}

#[test]
fn rejected_downloads_never_publish_or_leak_curl_stderr() {
    for (label, bytes, exit) in [
        ("changed", b"Authenticated-bytes".as_slice(), 0),
        ("short", b"short".as_slice(), 0),
        ("long", b"authenticated-bytes-plus-extra".as_slice(), 0),
        ("http", b"authenticated-bytes".as_slice(), 22),
        ("timeout", b"authenticated-bytes".as_slice(), 28),
    ] {
        let sandbox = Sandbox::new();
        let (manifest, signature, _) = sandbox.release("input", 5, "0.2.0", b"authenticated-bytes");
        sandbox.fake_curl(bytes, exit);
        let output = sandbox.root.join(format!("{label}.AppImage"));
        let refused = sandbox.run(&sandbox.download_args(&manifest, &signature, &output));
        assert!(!refused.status.success(), "{label}: {}", describe(&refused));
        assert!(
            sandbox.root.join("tools/curl-arguments").exists(),
            "must reach the downloader: {label}"
        );
        assert!(
            !output.exists(),
            "failed transfers must not publish: {label}"
        );
        assert!(!describe(&refused).contains("private-downloader-marker"));
        assert!(sandbox.state()["channels"].as_object().unwrap().is_empty());
        assert!(!fs::read_dir(&sandbox.root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".dsh-ocean-download-")));
    }
}

#[test]
fn signature_policy_and_remembered_floor_are_checked_before_download() {
    let sandbox = Sandbox::new();
    let (manifest, signature, _) = sandbox.release("input", 5, "0.2.0", b"authenticated-bytes");
    sandbox.fake_curl(b"authenticated-bytes", 0);
    let output = sandbox.root.join("download.AppImage");
    let mut damaged = fs::read(&signature).unwrap();
    damaged[0] = b'!';
    let bad_signature = sandbox.root.join("bad-signature.json");
    fs::write(&bad_signature, damaged).unwrap();
    let refused = sandbox.run(&sandbox.download_args(&manifest, &bad_signature, &output));
    assert!(!refused.status.success());
    assert!(!sandbox.root.join("tools/curl-arguments").exists());
    assert!(!output.exists());
    let (newer, newer_sig, _) = sandbox.release("newer", 6, "0.2.1", b"newer-bytes");
    let mut accept = sandbox.download_args(&newer, &newer_sig, &output);
    accept[0] = "--accept-release".into();
    accept.truncate(accept.len() - 2);
    assert!(sandbox.run(&accept).status.success());
    let replay = sandbox.run(&sandbox.download_args(&manifest, &signature, &output));
    assert!(!replay.status.success());
    assert!(
        describe(&replay).contains("rollback"),
        "{}",
        describe(&replay)
    );
    assert!(!sandbox.root.join("tools/curl-arguments").exists());
    assert!(!output.exists());
}

#[test]
fn download_refuses_overwrite_and_reaps_an_oversized_transfer() {
    let sandbox = Sandbox::new();
    let (manifest, signature, _) = sandbox.release("input", 5, "0.2.0", b"signed");
    let output = sandbox.root.join("existing.AppImage");
    fs::write(&output, b"keep-existing").unwrap();
    sandbox.fake_curl(b"signed", 0);
    let refused = sandbox.run(&sandbox.download_args(&manifest, &signature, &output));
    assert!(!refused.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"keep-existing");
    let oversized = sandbox.root.join("oversized.AppImage");
    sandbox.fake_curl(b"too-many-bytes", 0);
    let curl = sandbox.root.join("tools/curl");
    let code = fs::read_to_string(&curl)
        .unwrap()
        .replace("sys.exit(0)", "import time\ntime.sleep(20)\nsys.exit(0)");
    fs::write(&curl, code).unwrap();
    let start = std::time::Instant::now();
    let refused = sandbox.run(&sandbox.download_args(&manifest, &signature, &oversized));
    assert!(!refused.status.success());
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert!(!oversized.exists());
    let pid: i32 = fs::read_to_string(sandbox.root.join("tools/curl-pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "oversized downloader must be reaped"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[test]
fn failed_state_commit_restores_activation_selectors() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    } // Permission injection needs an unprivileged test user.
    let sandbox = Sandbox::new();
    let prefix = sandbox.root.join("prefix");
    let (m1, s1, a1) = sandbox.release("first", 5, "0.2.0", b"first-bytes");
    let applied = sandbox.run(&sandbox.apply_args(&m1, &s1, &a1, &prefix, true));
    assert!(applied.status.success(), "{}", describe(&applied));
    let original = current_target(&prefix);
    let state_dir = sandbox.root.join("state/dsh-ocean");
    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o500)).unwrap();
    let (m2, s2, a2) = sandbox.release("second", 6, "0.2.1", b"second-bytes");
    let failed = sandbox.run(&sandbox.apply_args(&m2, &s2, &a2, &prefix, true));
    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!failed.status.success(), "{}", describe(&failed));
    assert_eq!(
        current_target(&prefix),
        original,
        "failed acceptance must restore current"
    );
    assert!(!prefix.join(".previous").exists());
    assert!(!prefix.join(".activation-pending.json").exists());
    assert_eq!(sandbox.state()["channels"]["stable"]["sequence"], 5);
}

#[test]
fn apply_and_rollback_share_the_prefix_operation_lock() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root.join("prefix");
    let (m1, s1, a1) = sandbox.release("first", 5, "0.2.0", b"first-bytes");
    assert!(sandbox
        .run(&sandbox.apply_args(&m1, &s1, &a1, &prefix, true))
        .status
        .success());
    let original = current_target(&prefix);
    let _lock =
        dsh_ocean::profile_backup::operation_lock(&prefix.join(".shell-update.lock")).unwrap();
    let (m2, s2, a2) = sandbox.release("second", 6, "0.2.1", b"second-bytes");
    let refused = sandbox.run(&sandbox.apply_args(&m2, &s2, &a2, &prefix, true));
    assert!(!refused.status.success(), "{}", describe(&refused));
    assert!(describe(&refused).contains("operation"));
    let rollback = sandbox.run(&[
        "--rollback-shell".into(),
        "--prefix".into(),
        prefix.to_string_lossy().into_owned(),
        "--confirm-rollback".into(),
        "yes".into(),
    ]);
    assert!(!rollback.status.success());
    assert_eq!(current_target(&prefix), original);
    assert_eq!(sandbox.state()["channels"]["stable"]["sequence"], 5);
}

#[test]
fn tampered_retained_version_is_never_selected_by_rollback() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root.join("prefix");
    for (name, seq, version, bytes) in [
        ("first", 5, "0.2.0", b"first-bytes".as_slice()),
        ("second", 6, "0.2.1", b"second-bytes".as_slice()),
    ] {
        let (m, s, a) = sandbox.release(name, seq, version, bytes);
        let applied = sandbox.run(&sandbox.apply_args(&m, &s, &a, &prefix, true));
        assert!(applied.status.success(), "{}", describe(&applied));
    }
    let original = current_target(&prefix);
    let previous = fs::read_link(prefix.join(".previous")).unwrap();
    fs::write(
        prefix.join(&previous).join("dsh-ocean"),
        b"corrupt-retained-file",
    )
    .unwrap();
    let refused = sandbox.run(&[
        "--rollback-shell".into(),
        "--prefix".into(),
        prefix.to_string_lossy().into_owned(),
        "--confirm-rollback".into(),
        "yes".into(),
    ]);
    assert!(!refused.status.success(), "{}", describe(&refused));
    assert_eq!(current_target(&prefix), original);
    assert_eq!(fs::read_link(prefix.join(".previous")).unwrap(), previous);
    assert_eq!(sandbox.state()["channels"]["stable"]["sequence"], 6);
    // Valid executable bytes still do not authorize a writable receipt.
    fs::write(prefix.join(&previous).join("dsh-ocean"), b"first-bytes").unwrap();
    let receipt = prefix.join(&previous).join("receipt.json");
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o666)).unwrap();
    let refused = sandbox.run(&[
        "--rollback-shell".into(),
        "--prefix".into(),
        prefix.to_string_lossy().into_owned(),
        "--confirm-rollback".into(),
        "yes".into(),
    ]);
    assert!(
        !refused.status.success(),
        "writable receipts are not trusted activation metadata"
    );
    assert_eq!(current_target(&prefix), original);
}

#[test]
fn interrupted_activation_requires_explicit_verified_recovery() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root.join("prefix");
    let (m1, s1, a1) = sandbox.release("first", 5, "0.2.0", b"first-bytes");
    assert!(sandbox
        .run(&sandbox.apply_args(&m1, &s1, &a1, &prefix, true))
        .status
        .success());
    let first = current_target(&prefix);
    let (m2, s2, a2) = sandbox.release("second", 6, "0.2.1", b"second-bytes");
    assert!(sandbox
        .run(&sandbox.apply_args(&m2, &s2, &a2, &prefix, true))
        .status
        .success());
    let second = current_target(&prefix);
    let journal = prefix.join(".activation-pending.json");
    fs::write(&journal, serde_json::to_vec(&serde_json::json!({"schema":1,"before":{"current":first,"previous":null},"after":{"current":second,"previous":first}})).unwrap()).unwrap();
    let refused = sandbox.run(&sandbox.apply_args(&m2, &s2, &a2, &prefix, true));
    assert!(
        !refused.status.success(),
        "pending transaction must not be silently ignored"
    );
    let recover = vec![
        "--recover-shell".into(),
        "--prefix".into(),
        prefix.to_string_lossy().into_owned(),
        "--confirm-recovery".into(),
        "yes".into(),
    ];
    let recovered = sandbox.run(&recover);
    assert!(recovered.status.success(), "{}", describe(&recovered));
    assert_eq!(current_target(&prefix), first);
    assert!(!prefix.join(".previous").exists());
    assert!(!journal.exists());
    assert_eq!(
        sandbox.state()["channels"]["stable"]["sequence"],
        6,
        "recovery never lowers the floor"
    );
    fs::write(&journal, serde_json::to_vec(&serde_json::json!({"schema":1,"before":{"current":"../../outside","previous":null},"after":{"current":first,"previous":null}})).unwrap()).unwrap();
    assert!(!sandbox.run(&recover).status.success());
    assert_eq!(current_target(&prefix), first);
    assert!(
        journal.exists(),
        "invalid recovery input is preserved for diagnosis"
    );
}

#[test]
fn tar_archive_is_not_activated_as_an_executable() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root.join("prefix");
    let (manifest, signature, artifact) =
        sandbox.release("archive", 5, "0.2.0", b"synthetic-tar-bytes");
    let raw = fs::read_to_string(&manifest)
        .unwrap()
        .replace("\"kind\":\"appimage\"", "\"kind\":\"tar.gz\"")
        .replace(".AppImage", ".tar.gz");
    fs::write(&manifest, raw.as_bytes()).unwrap();
    let mut message = DOMAIN.to_vec();
    message.extend_from_slice(raw.as_bytes());
    fs::write(
        &signature,
        format!(
            "{{\"version\":1,\"keyId\":\"{KEY_ID}\",\"signatureHex\":\"{}\"}}",
            hex(&sandbox.key.sign(&message).to_bytes())
        ),
    )
    .unwrap();
    let mut args = sandbox.apply_args(&manifest, &signature, &artifact, &prefix, true);
    let kind = args.iter().position(|arg| arg == "--kind").unwrap();
    args[kind + 1] = "tar.gz".into();
    let refused = sandbox.run(&args);
    assert!(!refused.status.success(), "{}", describe(&refused));
    assert!(
        describe(&refused).contains("AppImage"),
        "{}",
        describe(&refused)
    );
    assert!(!prefix.exists());
    assert!(sandbox.state()["channels"].as_object().unwrap().is_empty());
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

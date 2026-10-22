//! Signed feed/trust/settings tests. Synthetic keys, fake trusted curl, private
//! HOME/XDG only: no real endpoint, publisher secret, Harness profile or service.
#![cfg(target_os = "linux")]
use chrono::{Duration, Utc};
use dsh_ocean::release_feed::hex;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Sandbox {
    root: PathBuf,
    key: SigningKey,
}
impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ocean-feed-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for dir in ["home", "tools", "config/dsh-ocean"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let script = "#!/usr/bin/python3\nimport sys\nfrom pathlib import Path\np=Path(__file__).parent\nu=sys.argv[sys.argv.index('--url')+1]\nwith (p/'calls').open('a') as f: f.write(u+'\\n')\nsys.stderr.write('private-feed-downloader-marker')\nsys.stdout.buffer.write((p/('signature.json' if u.endswith('.sig.json') else 'artifact' if u.endswith('.AppImage') else 'manifest.json')).read_bytes())\nsys.exit(int((p/'exit').read_text()) if (p/'exit').exists() else 0)\n";
        fs::write(root.join("tools/curl"), script).unwrap();
        fs::set_permissions(root.join("tools/curl"), fs::Permissions::from_mode(0o700)).unwrap();
        let sandbox = Self {
            root,
            key: SigningKey::from_bytes(&[37; 32]),
        };
        sandbox.feed("stable", 9, "0.2.0", false, &sandbox.key);
        fs::write(sandbox.root.join("tools/artifact"), b"payload").unwrap();
        sandbox
    }
    fn run(&self, args: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env(
                "PATH",
                format!("{}/tools:/usr/bin:/bin", self.root.display()),
            )
            .output()
            .unwrap()
    }
    fn key_document(key: &SigningKey) -> Value {
        json!({"version":1,"keyId":"synthetic-feed-key","publicKeyHex":hex(key.verifying_key().as_bytes())})
    }
    fn fingerprint(key: &SigningKey) -> String {
        hex(&Sha256::digest(key.verifying_key().as_bytes()))
    }
    fn configuration(key: &SigningKey) -> Value {
        let source = json!({"key":Self::key_document(key),"stable":{"manifestUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/download/channel-stable/manifest.json","signatureUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/download/channel-stable/manifest.sig.json","minimumSequence":1},"next":{"manifestUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/download/channel-next/manifest.json","signatureUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/download/channel-next/manifest.sig.json","minimumSequence":1}});
        json!({"schema":1,"shellChannel":"stable","runtimeChannel":"stable","shell":source,"runtime":null})
    }
    fn configure(&self, key: &SigningKey, replace: bool) -> Output {
        let file = self.root.join("configuration.json");
        fs::write(
            &file,
            serde_json::to_vec(&Self::configuration(key)).unwrap(),
        )
        .unwrap();
        let mut args = vec![
            "--configure-updates".into(),
            file.to_string_lossy().into_owned(),
            "--confirm-shell-key".into(),
            Self::fingerprint(key),
        ];
        if replace {
            args.extend(["--confirm-replace-trust".into(), "yes".into()]);
        }
        self.run(&args)
    }
    fn enroll(&self) {
        let result = self.configure(&self.key, false);
        assert!(result.status.success(), "{}", describe(&result));
    }
    fn settings_path(&self) -> PathBuf {
        self.root.join("config/dsh-ocean/updates.json")
    }
    fn prefix(&self) -> PathBuf {
        self.root.join("home/.local/share/dsh-ocean/shell")
    }
    fn launcher(&self) -> PathBuf {
        self.root
            .join("home/.local/share/applications/dev.dsh.ocean.managed.desktop")
    }
    fn enroll_destination(&self) -> Output {
        self.run(&[
            "--enroll-shell".into(),
            "--confirm-managed".into(),
            "yes".into(),
        ])
    }
    fn install(&self) -> Output {
        self.run(&[
            "--install-managed-shell".into(),
            "--kind".into(),
            "appimage".into(),
            "--confirm-install".into(),
            "yes".into(),
        ])
    }
    fn check(&self) -> Output {
        self.run(&["--check-shell-update".into()])
    }
    fn feed(&self, channel: &str, sequence: u64, version: &str, expired: bool, key: &SigningKey) {
        let arch = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };
        let now = Utc::now();
        let manifest = json!({"version":1,"product":"dsh-ocean","channel":channel,"sequence":sequence,"release":version,"publishedAt":(now-Duration::days(2)).to_rfc3339(),"expiresAt":(now+Duration::days(if expired {-1} else {5})).to_rfc3339(),"artifacts":[{"target":format!("linux-{arch}"),"kind":"appimage","url":format!("https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v{version}/dsh-ocean-{version}-{arch}.AppImage"),"size":7,"sha256":hex(&Sha256::digest(b"payload"))}]});
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let mut message = b"DSH Ocean release manifest v1\n".to_vec();
        message.extend_from_slice(&bytes);
        let signature = json!({"version":1,"keyId":"synthetic-feed-key","signatureHex":hex(&key.sign(&message).to_bytes())});
        fs::write(self.root.join("tools/manifest.json"), bytes).unwrap();
        fs::write(
            self.root.join("tools/signature.json"),
            serde_json::to_vec(&signature).unwrap(),
        )
        .unwrap();
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn describe(result: &Output) -> String {
    format!(
        "{}\n{}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    )
}

#[test]
fn unconfigured_status_is_read_only_and_channels_do_not_change_harness_configuration() {
    let s = Sandbox::new();
    let status = s.run(&["--update-settings".into()]);
    assert!(status.status.success(), "{}", describe(&status));
    let body: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(body["shellConfigured"], false);
    assert!(!s.settings_path().exists());
    assert!(!s.check().status.success());
    assert!(!s.root.join("tools/calls").exists());
    let harness_settings = s.root.join("config/dsh-ocean/config.json");
    fs::write(&harness_settings, b"{\"updateChannel\":\"latest\"}").unwrap();
    s.enroll();
    let status = s.run(&["--set-update-channel".into(), "shell".into(), "next".into()]);
    assert!(status.status.success(), "{}", describe(&status));
    let body: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(body["shellChannel"], "next");
    assert_eq!(body["runtimeChannel"], "stable");
    assert_eq!(
        body["shell"]["keyFingerprint"],
        Sandbox::fingerprint(&s.key)
    );
    assert!(!String::from_utf8_lossy(&status.stdout).contains("publicKeyHex"));
    assert_eq!(
        fs::read(&harness_settings).unwrap(),
        b"{\"updateChannel\":\"latest\"}"
    );
    assert_eq!(
        fs::metadata(s.settings_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn managed_destination_requires_explicit_enrollment_and_preserves_original_installation() {
    let s = Sandbox::new();
    s.enroll();
    let status = s.run(&["--managed-shell-status".into()]);
    assert!(status.status.success(), "{}", describe(&status));
    assert_eq!(
        serde_json::from_slice::<Value>(&status.stdout).unwrap()["enrolled"],
        false
    );
    assert!(!s.prefix().exists());
    assert!(!s.install().status.success());
    assert!(!s.root.join("tools/calls").exists());
    assert!(!s.run(&["--enroll-shell".into()]).status.success());
    assert!(!s.prefix().exists());
    let enrolled = s.enroll_destination();
    assert!(enrolled.status.success(), "{}", describe(&enrolled));
    let original = s.root.join("home/.local/bin/dsh-ocean");
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, b"original-package-or-source-install").unwrap();
    let installed = s.install();
    assert!(installed.status.success(), "{}", describe(&installed));
    let receipt: Value = serde_json::from_slice(&installed.stdout).unwrap();
    assert_eq!(receipt["installedOnDisk"], true);
    assert_eq!(receipt["serviceRestarted"], false);
    assert_eq!(
        fs::read(&original).unwrap(),
        b"original-package-or-source-install"
    );
    assert_eq!(
        fs::read(s.prefix().join("current/dsh-ocean")).unwrap(),
        b"payload"
    );
    assert!(fs::read_to_string(s.launcher())
        .unwrap()
        .contains("DSH Ocean (managed)"));
    let inspected = s.run(&["--managed-shell-status".into()]);
    assert!(inspected.status.success(), "{}", describe(&inspected));
    let status: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(status["enrolled"], true);
    assert_eq!(status["installation"]["current"]["release"], "0.2.0");
    assert!(!s.root.join("home/.dsh").exists());
}

#[test]
fn managed_update_never_silently_downgrades_the_disk_version_even_after_floor_removal() {
    let s = Sandbox::new();
    s.enroll();
    assert!(s.enroll_destination().status.success());
    s.feed("stable", 10, "0.3.0", false, &s.key);
    let installed = s.install();
    assert!(installed.status.success(), "{}", describe(&installed));
    let original = fs::read_link(s.prefix().join("current")).unwrap();
    let floor = s.root.join("state/dsh-ocean/release-state.json");
    assert!(floor.is_file());
    fs::remove_file(&floor).unwrap();
    s.feed("stable", 11, "0.2.0", false, &s.key);
    let refused = s.install();
    assert!(
        !refused.status.success(),
        "managed updates must compare the disk version, not only the old running package"
    );
    assert_eq!(fs::read_link(s.prefix().join("current")).unwrap(), original);
}

#[test]
fn managed_enrollment_and_launcher_refuse_unknown_contents_without_overwrite() {
    let s = Sandbox::new();
    s.enroll();
    fs::create_dir_all(s.prefix()).unwrap();
    let unknown = s.prefix().join("keep");
    fs::write(&unknown, b"user-owned-not-updater-data").unwrap();
    assert!(!s.enroll_destination().status.success());
    assert_eq!(fs::read(&unknown).unwrap(), b"user-owned-not-updater-data");
    assert_eq!(
        fs::read_dir(s.prefix()).unwrap().count(),
        1,
        "refusal must not even create an operation lock in unknown contents"
    );
    let s = Sandbox::new();
    s.enroll();
    assert!(s.enroll_destination().status.success());
    fs::create_dir_all(s.launcher().parent().unwrap()).unwrap();
    fs::write(s.launcher(), b"user desktop entry").unwrap();
    let refused = s.install();
    assert!(!refused.status.success(), "{}", describe(&refused));
    assert_eq!(fs::read(s.launcher()).unwrap(), b"user desktop entry");
    assert!(!s.prefix().join("current").exists());
    assert!(!s.root.join("state/dsh-ocean/release-state.json").exists());
}

#[test]
fn failed_managed_download_never_activates_or_accepts_a_release() {
    let s = Sandbox::new();
    s.enroll();
    assert!(s.enroll_destination().status.success());
    fs::write(s.root.join("tools/artifact"), b"tampered-body").unwrap();
    let failed = s.install();
    assert!(!failed.status.success(), "{}", describe(&failed));
    assert!(!s.prefix().join("current").exists());
    assert!(!s.launcher().exists());
    assert!(!s.root.join("state/dsh-ocean/release-state.json").exists());
    let cache = s.root.join("state/dsh-ocean/shell-downloads");
    assert!(!cache.exists() || fs::read_dir(cache).unwrap().count() == 0);
}

#[test]
fn authenticated_feed_check_never_accepts_installs_or_downloads_an_artifact() {
    let s = Sandbox::new();
    s.enroll();
    let config = fs::read(s.settings_path()).unwrap();
    let checked = s.check();
    assert!(checked.status.success(), "{}", describe(&checked));
    let body: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(body["verification"], "signature-and-policy");
    assert_eq!(body["release"], "0.2.0");
    assert_eq!(body["sequence"], 9);
    assert_eq!(body["channel"], "stable");
    assert_eq!(body["available"], true);
    assert_eq!(body["applied"], false);
    assert_eq!(body["downloaded"], false);
    assert!(!describe(&checked).contains("private-feed-downloader-marker"));
    assert_eq!(fs::read(s.settings_path()).unwrap(), config);
    assert!(!s.root.join("state/dsh-ocean/release-state.json").exists());
    assert!(!s.root.join("home/.dsh").exists());
    let calls = fs::read_to_string(s.root.join("tools/calls")).unwrap();
    assert_eq!(calls.lines().count(), 2);
    assert!(!calls.contains("AppImage"));
    assert!(!calls.contains("key.json"));
}

#[test]
fn feed_authentication_policy_size_and_transport_fail_closed() {
    for case in [
        "tampered",
        "wrong-key",
        "expired",
        "channel",
        "oversize",
        "http",
    ] {
        let s = Sandbox::new();
        s.enroll();
        match case {
            "tampered" => {
                let path = s.root.join("tools/manifest.json");
                let mut bytes = fs::read(&path).unwrap();
                bytes.push(b' ');
                fs::write(path, bytes).unwrap();
            }
            "wrong-key" => s.feed(
                "stable",
                9,
                "0.2.0",
                false,
                &SigningKey::from_bytes(&[38; 32]),
            ),
            "expired" => s.feed("stable", 9, "0.2.0", true, &s.key),
            "channel" => s.feed("next", 9, "0.2.0", false, &s.key),
            "oversize" => fs::write(s.root.join("tools/manifest.json"), vec![b'x'; 65537]).unwrap(),
            "http" => fs::write(s.root.join("tools/exit"), b"22").unwrap(),
            _ => unreachable!(),
        }
        let checked = s.check();
        assert!(!checked.status.success(), "{case}: {}", describe(&checked));
        assert!(!describe(&checked).contains("private-feed-downloader-marker"));
        assert!(!s.root.join("state/dsh-ocean/release-state.json").exists());
        assert!(!s.root.join("home/.dsh").exists());
    }
}

#[test]
fn trust_replacement_requires_fingerprint_and_separate_confirmation() {
    let s = Sandbox::new();
    let input = s.root.join("configuration.json");
    fs::write(
        &input,
        serde_json::to_vec(&Sandbox::configuration(&s.key)).unwrap(),
    )
    .unwrap();
    let refused = s.run(&[
        "--configure-updates".into(),
        input.to_string_lossy().into_owned(),
        "--confirm-shell-key".into(),
        "0".repeat(64),
    ]);
    assert!(!refused.status.success());
    assert!(!s.settings_path().exists());
    s.enroll();
    let original = fs::read(s.settings_path()).unwrap();
    let new = SigningKey::from_bytes(&[38; 32]);
    assert!(!s.configure(&new, false).status.success());
    assert_eq!(fs::read(s.settings_path()).unwrap(), original);
    let changed = s.configure(&new, true);
    assert!(changed.status.success(), "{}", describe(&changed));
    assert!(
        !s.check().status.success(),
        "old publisher signature must no longer authenticate"
    );
}

#[test]
fn unsafe_feeds_and_settings_storage_are_not_authority() {
    for url in ["http://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/manifest.json","https://user:private@github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/manifest.json","https://github.com/other/repo/releases/latest/download/manifest.json","https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/manifest.json?private=1","https://127.0.0.1/manifest.json"] {
        let s=Sandbox::new();s.enroll();let original=fs::read(s.settings_path()).unwrap();
        let mut candidate=Sandbox::configuration(&s.key);candidate["shell"]["stable"]["manifestUrl"]=json!(url);
        let file=s.root.join("bad-config.json");fs::write(&file,serde_json::to_vec(&candidate).unwrap()).unwrap();
        let refused=s.run(&["--configure-updates".into(),file.to_string_lossy().into_owned(),"--confirm-shell-key".into(),Sandbox::fingerprint(&s.key)]);
        assert!(!refused.status.success());assert_eq!(fs::read(s.settings_path()).unwrap(),original);assert!(!s.root.join("tools/calls").exists());
    }
    let s = Sandbox::new();
    s.enroll();
    fs::set_permissions(s.settings_path(), fs::Permissions::from_mode(0o666)).unwrap();
    assert!(!s.check().status.success());
    assert!(!s.root.join("tools/calls").exists());
    fs::set_permissions(s.settings_path(), fs::Permissions::from_mode(0o600)).unwrap();
    let saved = s.root.join("saved-settings.json");
    fs::rename(s.settings_path(), &saved).unwrap();
    std::os::unix::fs::symlink(&saved, s.settings_path()).unwrap();
    assert!(!s.check().status.success());
    assert!(!s.root.join("tools/calls").exists());
}

#[test]
fn runtime_trust_and_channel_are_independent_from_shell_and_npm_settings() {
    let s = Sandbox::new();
    let runtime_key = SigningKey::from_bytes(&[39; 32]);
    let mut configuration = Sandbox::configuration(&s.key);
    configuration["runtime"] = Sandbox::configuration(&runtime_key)["shell"].clone();
    let input = s.root.join("both-products.json");
    fs::write(&input, serde_json::to_vec(&configuration).unwrap()).unwrap();
    let mut arguments = vec![
        "--configure-updates".into(),
        input.to_string_lossy().into_owned(),
        "--confirm-shell-key".into(),
        Sandbox::fingerprint(&s.key),
    ];
    assert!(
        !s.run(&arguments).status.success(),
        "shell confirmation does not provision runtime trust"
    );
    assert!(!s.settings_path().exists());
    arguments.extend([
        "--confirm-runtime-key".into(),
        Sandbox::fingerprint(&runtime_key),
    ]);
    let configured = s.run(&arguments);
    assert!(configured.status.success(), "{}", describe(&configured));
    let changed = s.run(&[
        "--set-update-channel".into(),
        "runtime".into(),
        "next".into(),
    ]);
    assert!(changed.status.success(), "{}", describe(&changed));
    let summary: Value = serde_json::from_slice(&changed.stdout).unwrap();
    assert_eq!(summary["shellChannel"], "stable");
    assert_eq!(summary["runtimeChannel"], "next");
    assert_eq!(
        summary["runtime"]["keyFingerprint"],
        Sandbox::fingerprint(&runtime_key)
    );
    assert_ne!(
        summary["runtime"]["keyFingerprint"],
        summary["shell"]["keyFingerprint"]
    );
    assert!(!s.root.join("tools/calls").exists());
    assert!(!s.root.join("home/.dsh").exists());
}

#[test]
fn configured_check_refuses_remembered_replays_without_modifying_the_floor() {
    let s = Sandbox::new();
    s.enroll();
    s.feed("stable", 10, "0.3.0", false, &s.key);
    let key = s.root.join("public-key.json");
    fs::write(
        &key,
        serde_json::to_vec(&Sandbox::key_document(&s.key)).unwrap(),
    )
    .unwrap();
    let arch = if cfg!(target_arch = "aarch64") {
        "linux-aarch64"
    } else {
        "linux-x86_64"
    };
    let accepted = s.run(&[
        "--accept-release".into(),
        s.root
            .join("tools/manifest.json")
            .to_string_lossy()
            .into_owned(),
        s.root
            .join("tools/signature.json")
            .to_string_lossy()
            .into_owned(),
        key.to_string_lossy().into_owned(),
        "--channel".into(),
        "stable".into(),
        "--target".into(),
        arch.into(),
        "--kind".into(),
        "appimage".into(),
        "--min-sequence".into(),
        "1".into(),
    ]);
    assert!(accepted.status.success(), "{}", describe(&accepted));
    let state = s.root.join("state/dsh-ocean/release-state.json");
    let before = fs::read(&state).unwrap();
    s.feed("stable", 9, "0.2.0", false, &s.key);
    let refused = s.check();
    assert!(!refused.status.success(), "{}", describe(&refused));
    assert_eq!(fs::read(&state).unwrap(), before);
}

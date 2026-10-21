//! Isolated fixture roots/domains. Never executes a runtime or contacts a server.
use chrono::{Duration, Utc};
use dsh_ocean::release_feed::hex;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
const DOMAIN: &[u8] = b"DSH Ocean runtime manifest v1\n";
struct Fixture {
    root: PathBuf,
    key: SigningKey,
    manifest: Value,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ocean-runtime-feed-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(root.join("home")).unwrap();
        let now = Utc::now();
        let manifest = json!({"version":1,"product":"dsh-ocean-runtime","channel":"stable","sequence":9,"release":"0.2.1","publishedAt":(now-Duration::minutes(1)).to_rfc3339(),"expiresAt":(now+Duration::days(1)).to_rfc3339(),"components":{"node":"24.14.1","dsh":"0.2.1","pnpm":"10.30.3"},"stateScope":"web-profile-sessions-storages-v1","artifacts":[{"target":"linux-x86_64","kind":"tar.gz","url":"https://github.com/ryltsevmikhail/dsh-ocean/releases/download/runtime-stable-v0.2.1-r9/dsh-ocean-runtime-stable-0.2.1-9-x86_64.tar.gz","size":6,"sha256":"11".repeat(32),"nodeArchiveSha256":"22".repeat(32)}]});
        let fixture = Self {
            root,
            key: SigningKey::from_bytes(&[81; 32]),
            manifest,
        };
        fixture.write(DOMAIN, &fixture.key);
        fixture
    }
    fn write(&self, domain: &[u8], key: &SigningKey) {
        let bytes = serde_json::to_vec(&self.manifest).unwrap();
        fs::write(self.root.join("manifest.json"), &bytes).unwrap();
        let mut message = domain.to_vec();
        message.extend_from_slice(&bytes);
        fs::write(self.root.join("signature.json"),serde_json::to_vec(&json!({"version":1,"keyId":"runtime-fixture","signatureHex":hex(&key.sign(&message).to_bytes())})).unwrap()).unwrap();
        fs::write(self.root.join("key.json"),serde_json::to_vec(&json!({"version":1,"keyId":"runtime-fixture","publicKeyHex":hex(self.key.verifying_key().as_bytes())})).unwrap()).unwrap();
    }
    fn args(&self, accept: bool) -> Vec<String> {
        let mut args = vec![
            if accept {
                "--accept-runtime-release"
            } else {
                "--verify-runtime-release"
            }
            .into(),
            self.root
                .join("manifest.json")
                .to_string_lossy()
                .into_owned(),
            self.root
                .join("signature.json")
                .to_string_lossy()
                .into_owned(),
            self.root.join("key.json").to_string_lossy().into_owned(),
            "--channel".into(),
            "stable".into(),
            "--target".into(),
            "linux-x86_64".into(),
            "--min-sequence".into(),
            "1".into(),
            "--current-runtime-version".into(),
            "0.2.0".into(),
        ];
        if accept {
            args.extend(["--confirm-accept".into(), "yes".into()]);
        }
        args
    }
    fn run(&self, args: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env(
                "PATH",
                format!("{}:/definitely/no/tools", self.root.join("tools").display()),
            )
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
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
fn runtime_authenticates_complete_bundle_metadata_without_shell_state_or_tools() {
    let f = Fixture::new();
    let output = f.run(&f.args(false));
    assert!(output.status.success(), "{}", describe(&output));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["product"], "dsh-ocean-runtime");
    assert_eq!(result["components"]["dsh"], "0.2.1");
    assert_eq!(result["applied"], false);
    assert!(!f.root.join("state").exists());
    assert!(!f.root.join("home/.dsh").exists());
}
#[test]
fn wrong_key_tampering_and_shell_signature_domain_never_authorize_runtime() {
    for wrong in 0..3 {
        let f = Fixture::new();
        match wrong {
            0 => f.write(DOMAIN, &SigningKey::from_bytes(&[82; 32])),
            1 => f.write(dsh_ocean::release_feed::DOMAIN, &f.key),
            _ => {
                let mut bytes = fs::read(f.root.join("manifest.json")).unwrap();
                bytes.push(b' ');
                fs::write(f.root.join("manifest.json"), bytes).unwrap();
            }
        }
        let output = f.run(&f.args(false));
        assert!(
            !output.status.success(),
            "case {wrong}: {}",
            describe(&output)
        );
        assert!(!f.root.join("state").exists());
    }
}
#[test]
fn runtime_domain_cannot_authorize_even_a_valid_shell_manifest() {
    let mut f = Fixture::new();
    let object = f.manifest.as_object_mut().unwrap();
    object.remove("components");
    object.remove("stateScope");
    f.manifest["product"] = json!("dsh-ocean");
    f.manifest["artifacts"][0]
        .as_object_mut()
        .unwrap()
        .remove("nodeArchiveSha256");
    f.manifest["artifacts"][0]["url"]=json!("https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v0.2.1/dsh-ocean-0.2.1-x86_64.tar.gz");
    let mut args = f.args(false);
    args.truncate(args.len() - 2);
    args[0] = "--verify-release".into();
    args.extend(["--kind".into(), "tar.gz".into()]);
    f.write(dsh_ocean::release_feed::DOMAIN, &f.key);
    let valid = f.run(&args);
    assert!(
        valid.status.success(),
        "control shell manifest must be valid: {}",
        describe(&valid)
    );
    f.write(DOMAIN, &f.key);
    assert!(
        !f.run(&args).status.success(),
        "runtime signature must not authorize shell even with same key and valid shell metadata"
    );
}

#[test]
fn runtime_policy_refuses_unsafe_scope_versions_urls_expiry_and_formats() {
    for case in 0..9 {
        let mut f = Fixture::new();
        match case {
            0 => f.manifest["components"]["dsh"] = json!("0.2.2"),
            1 => f.manifest["stateScope"] = json!("unbounded-home-migration"),
            2 => f.manifest["expiresAt"] = json!((Utc::now() - Duration::hours(1)).to_rfc3339()),
            3 => f.manifest["artifacts"][0]["kind"] = json!("appimage"),
            4 => f.manifest["artifacts"][0]["nodeArchiveSha256"] = json!("upstream"),
            5 => {
                f.manifest["artifacts"][0]["url"] = json!("https://example.invalid/runtime.tar.gz")
            }
            6 => f.manifest["components"]["node"] = json!("24.x"),
            7 => f.manifest["artifacts"][0]["target"] = json!("linux-aarch64"),
            _ => f.manifest["unexpected"] = json!(true),
        }
        f.write(DOMAIN, &f.key);
        let output = f.run(&f.args(false));
        assert!(
            !output.status.success(),
            "case{case}: {}",
            describe(&output)
        );
        assert!(!f.root.join("state").exists());
    }
}
#[test]
fn runtime_floors_are_independent_and_checks_never_implicitly_accept() {
    let mut f = Fixture::new();
    let directory = f.root.join("state/dsh-ocean");
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let shell = json!({"schema":1,"channels":{"stable":{"sequence":999,"release":"9.0.0","manifestSha256":"33".repeat(32),"recordedAt":Utc::now().to_rfc3339()}}});
    fs::write(
        directory.join("release-state.json"),
        serde_json::to_vec(&shell).unwrap(),
    )
    .unwrap();
    let verified = f.run(&f.args(false));
    assert!(verified.status.success(), "{}", describe(&verified));
    assert!(!directory.join("runtime-release-state.json").exists());
    let accepted = f.run(&f.args(true));
    assert!(accepted.status.success(), "{}", describe(&accepted));
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(directory.join("release-state.json")).unwrap())
            .unwrap(),
        shell
    );
    let runtime: Value =
        serde_json::from_slice(&fs::read(directory.join("runtime-release-state.json")).unwrap())
            .unwrap();
    assert_eq!(runtime["channels"]["stable"]["sequence"], 9);
    f.manifest["sequence"] = json!(8);
    f.manifest["artifacts"][0]["url"]=json!("https://github.com/ryltsevmikhail/dsh-ocean/releases/download/runtime-stable-v0.2.1-r8/dsh-ocean-runtime-stable-0.2.1-8-x86_64.tar.gz");
    f.write(DOMAIN, &f.key);
    assert!(!f.run(&f.args(false)).status.success());
}
#[test]
fn configured_runtime_check_uses_its_own_root_channel_and_does_not_accept_or_install() {
    let mut f = Fixture::new();
    let arch = std::env::consts::ARCH;
    f.manifest["artifacts"][0]["target"] = json!(format!("linux-{arch}"));
    f.manifest["artifacts"][0]["url"]=json!(format!("https://github.com/ryltsevmikhail/dsh-ocean/releases/download/runtime-stable-v0.2.1-r9/dsh-ocean-runtime-stable-0.2.1-9-{arch}.tar.gz"));
    f.write(DOMAIN, &f.key);
    let source = |key: &SigningKey| json!({"key":{"version":1,"keyId":"runtime-fixture","publicKeyHex":hex(key.verifying_key().as_bytes())},"stable":{"manifestUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/runtime.json","signatureUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/runtime.sig.json","minimumSequence":1},"next":{"manifestUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/runtime-next.json","signatureUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/runtime-next.sig.json","minimumSequence":1}});
    let mut settings = json!({"schema":1,"shellChannel":"next","runtimeChannel":"stable","shell":source(&SigningKey::from_bytes(&[82;32])),"runtime":source(&f.key)});
    let config = f.root.join("config/dsh-ocean");
    fs::create_dir_all(&config).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        config.join("updates.json"),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    fs::set_permissions(
        config.join("updates.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let tools = f.root.join("tools");
    fs::create_dir(&tools).unwrap();
    fs::write(tools.join("curl"),"#!/usr/bin/python3\nfrom pathlib import Path\nimport sys\np=Path(__file__).parent.parent\nu=sys.argv[sys.argv.index('--url')+1]\nsys.stdout.buffer.write((p/('signature.json' if u.endswith('.sig.json') else 'manifest.json')).read_bytes())\n").unwrap();
    fs::set_permissions(tools.join("curl"), fs::Permissions::from_mode(0o700)).unwrap();
    let args = vec![
        "--check-runtime-update".into(),
        "--current-runtime-version".into(),
        "0.2.0".into(),
    ];
    let output = f.run(&args);
    assert!(output.status.success(), "{}", describe(&output));
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["product"], "dsh-ocean-runtime");
    assert_eq!(summary["channel"], "stable");
    assert_eq!(summary["downloaded"], false);
    assert!(!f.root.join("state").exists());
    settings["runtimeChannel"] = json!("next");
    fs::write(
        config.join("updates.json"),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    assert!(
        !f.run(&args).status.success(),
        "runtime channel must not follow shell channel or ignore the configured selection"
    );
    assert!(!f.root.join("state").exists());
}

#[test]
fn runtime_floor_refuses_writable_linked_or_redirected_storage() {
    for case in 0..3 {
        let f = Fixture::new();
        let directory = f.root.join("state/dsh-ocean");
        fs::create_dir_all(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("runtime-release-state.json");
        fs::write(&path, b"{\"schema\":1,\"channels\":{}}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        match case {
            0 => fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap(),
            1 => fs::hard_link(&path, f.root.join("other-link")).unwrap(),
            _ => fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap(),
        }
        let output = f.run(&f.args(false));
        assert!(
            !output.status.success(),
            "unsafe floor case{case}: {}",
            describe(&output)
        );
    }
}

#[test]
fn runtime_accept_requires_explicit_confirmation() {
    let f = Fixture::new();
    let mut args = f.args(true);
    args.truncate(args.len() - 2);
    assert!(!f.run(&args).status.success());
    assert!(!f.root.join("state").exists());
}

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
    fn bundle(&mut self, invalid: Option<&str>) {
        use sha2::{Digest, Sha256};
        let health = invalid == Some("health");
        let node_bytes = if health {
            let node = std::env::var_os("DSH_OCEAN_TEST_RUNTIME_NODE")
                .expect("explicit isolated real-Node fixture opt-in");
            let output = Command::new(&node)
                .arg("--version")
                .env_clear()
                .output()
                .unwrap();
            assert!(output.status.success());
            self.manifest["components"]["node"] = json!(String::from_utf8(output.stdout)
                .unwrap()
                .trim()
                .trim_start_matches('v'));
            fs::read(node).unwrap()
        } else {
            b"#!/bin/sh\nexit 97\n".to_vec()
        };
        let dsh = if health {
            b"if(process.argv.includes('--version')){console.log('0.2.1');process.exit(0);} const http=require('node:http'); const s=http.createServer((q,r)=>{r.statusCode=q.url.includes('token=')?303:401;r.end();});s.listen(0,'127.0.0.1',()=>console.log('dsh web: http://127.0.0.1:'+s.address().port+'/?token=synthetic-never-print'))".as_slice()
        } else {
            b"// signed fixture entry, never executed".as_slice()
        };
        let pnpm = if health {
            b"console.log('10.30.3');".as_slice()
        } else {
            b"// resolved fixture tool".as_slice()
        };
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gzip);
            let versions = format!(
                "{}\n0.2.1\n10.30.3\n{}\n",
                self.manifest["components"]["node"].as_str().unwrap(),
                "22".repeat(32)
            );
            for (name, body, mode) in [
                ("runtime/node/bin/node", node_bytes.as_slice(), 0o755),
                ("harness/node_modules/.bin/dsh", dsh, 0o755),
                ("tools/node_modules/.bin/pnpm", pnpm, 0o755),
                (
                    "harness/node_modules/native-addon/native.node",
                    b"resolved native bytes".as_slice(),
                    0o644,
                ),
                (
                    "NOTICE",
                    b"complete-runtime-license-evidence".as_slice(),
                    0o644,
                ),
                (".versions", versions.as_bytes(), 0o644),
            ] {
                if invalid == Some("missing") && name == "tools/node_modules/.bin/pnpm" {
                    continue;
                }
                let body = if invalid == Some("versions") && name == ".versions" {
                    b"wrong-component-versions".as_slice()
                } else {
                    body
                };
                let mut header = tar::Header::new_gnu();
                header.set_mode(mode);
                header.set_size(body.len() as u64);
                header.set_cksum();
                tar.append_data(
                    &mut header,
                    format!("dsh-ocean-runtime-stable-0.2.1-9-linux-x86_64/{name}"),
                    body,
                )
                .unwrap();
            }
            if health {
                for name in ["node-pty", "koffi"] {
                    let mut header = tar::Header::new_gnu();
                    header.set_mode(0o644);
                    header.set_size(18);
                    header.set_cksum();
                    tar.append_data(&mut header,format!("dsh-ocean-runtime-stable-0.2.1-9-linux-x86_64/harness/node_modules/@deepseek-ai/dsh/node_modules/{name}/index.js"),b"module.exports={};".as_slice()).unwrap();
                }
            }
            if invalid == Some("link") {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                header.set_link_name("/outside").unwrap();
                header.set_cksum();
                tar.append_data(
                    &mut header,
                    "dsh-ocean-runtime-stable-0.2.1-9-linux-x86_64/escape",
                    std::io::empty(),
                )
                .unwrap();
            }
            if invalid == Some("predecessor") {
                let mut header = tar::Header::new_gnu();
                header.set_mode(0o644);
                header.set_size(4);
                header.set_cksum();
                tar.append_data(
                    &mut header,
                    "dsh-ocean-runtime-stable-0.2.1-9-linux-x86_64/.previous",
                    b"none".as_slice(),
                )
                .unwrap();
            }
            tar.finish().unwrap();
        }
        let bytes = gzip.finish().unwrap();
        self.manifest["artifacts"][0]["size"] = json!(bytes.len());
        self.manifest["artifacts"][0]["sha256"] = json!(hex(&Sha256::digest(&bytes)));
        fs::write(self.root.join("bundle.tar.gz"), bytes).unwrap();
        self.write(DOMAIN, &self.key);
    }
    fn import(&self) -> Output {
        let mut args = self.args(false);
        args[0] = "--import-runtime-release".into();
        args.extend([
            "--artifact".into(),
            self.root
                .join("bundle.tar.gz")
                .to_string_lossy()
                .into_owned(),
            "--prefix".into(),
            self.root
                .join("runtime-prefix")
                .to_string_lossy()
                .into_owned(),
            "--confirm-import".into(),
            "yes".into(),
        ]);
        self.run(&args)
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
fn complete_runtime_import_retains_all_bytes_without_execution_selection_or_floor_acceptance() {
    let mut f = Fixture::new();
    f.bundle(None);
    let prefix = f.root.join("runtime-prefix");
    fs::create_dir_all(prefix.join("generations/gen.old")).unwrap();
    fs::write(prefix.join("generations/gen.old/original"), b"original").unwrap();
    std::os::unix::fs::symlink("generations/gen.old", prefix.join("current")).unwrap();
    let output = f.import();
    assert!(output.status.success(), "{}", describe(&output));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    let generation = PathBuf::from(result["generation"].as_str().unwrap());
    assert_eq!(
        fs::read(generation.join("harness/node_modules/native-addon/native.node")).unwrap(),
        b"resolved native bytes"
    );
    assert_eq!(
        fs::read(generation.join("NOTICE")).unwrap(),
        b"complete-runtime-license-evidence"
    );
    assert!(!generation.join(".previous").exists());
    assert_eq!(
        fs::read_link(prefix.join("current")).unwrap(),
        PathBuf::from("generations/gen.old")
    );
    assert_eq!(
        fs::read(prefix.join("current/original")).unwrap(),
        b"original"
    );
    assert_eq!(result["activated"], false);
    assert_eq!(result["healthChecked"], false);
    assert!(!f.root.join("state").exists());
    assert!(!f.root.join("home/.dsh").exists());
    assert!(
        f.import().status.success(),
        "same exact verified tree may be reused"
    );
    let file = generation.join("harness/node_modules/native-addon/native.node");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&file, b"tampered retained closure").unwrap();
    assert!(
        !f.import().status.success(),
        "retained complete tree must be rechecked, never overwritten"
    );
    assert_eq!(fs::read(file).unwrap(), b"tampered retained closure");
}
#[test]
fn signed_invalid_runtime_trees_never_publish_a_generation() {
    for invalid in ["missing", "versions", "link", "predecessor"] {
        let mut f = Fixture::new();
        f.bundle(Some(invalid));
        let output = f.import();
        assert!(
            !output.status.success(),
            "invalid {invalid}: {}",
            describe(&output)
        );
        let prefix = f.root.join("runtime-prefix");
        assert!(!prefix.join("current").exists());
        if prefix.join("generations").exists() {
            assert_eq!(
                fs::read_dir(prefix.join("generations")).unwrap().count(),
                0,
                "failed private staging must be removed"
            );
        }
        assert!(!f.root.join("state").exists());
        assert!(!f.root.join("home/.dsh").exists());
    }
}
#[test]
fn runtime_import_authenticates_before_touching_destination_or_payload() {
    let mut f = Fixture::new();
    f.bundle(None);
    f.write(dsh_ocean::release_feed::DOMAIN, &f.key);
    assert!(!f.import().status.success());
    assert!(!f.root.join("runtime-prefix").exists());
    f.write(DOMAIN, &f.key);
    fs::write(f.root.join("bundle.tar.gz"), b"wrong signed bytes").unwrap();
    assert!(!f.import().status.success());
    assert!(!f.root.join("runtime-prefix/current").exists());
    assert!(!f.root.join("state").exists());
}

#[test]
fn runtime_import_never_uses_harness_state_as_an_install_destination() {
    let mut f = Fixture::new();
    f.bundle(None);
    let mut args = f.args(false);
    args[0] = "--import-runtime-release".into();
    args.extend([
        "--artifact".into(),
        f.root.join("bundle.tar.gz").to_string_lossy().into_owned(),
        "--prefix".into(),
        f.root
            .join("home/.dsh/runtime")
            .to_string_lossy()
            .into_owned(),
        "--confirm-import".into(),
        "yes".into(),
    ]);
    let output = f.run(&args);
    assert!(
        !output.status.success(),
        "runtime bytes and mutable Harness state must be separate: {}",
        describe(&output)
    );
    assert!(!f.root.join("home/.dsh").exists());
}

#[test]
fn prepared_generation_holds_existing_bootstrap_lock_until_dropped() {
    use dsh_ocean::release_feed::{Channel, Kind, Policy, Target, TrustedKey};
    use std::os::fd::AsRawFd;
    let mut f = Fixture::new();
    f.bundle(None);
    let verified = dsh_ocean::runtime_feed::verify(
        &fs::read(f.root.join("manifest.json")).unwrap(),
        &fs::read(f.root.join("signature.json")).unwrap(),
        &TrustedKey::from_json(&fs::read(f.root.join("key.json")).unwrap()).unwrap(),
        &Policy {
            channel: Channel::Stable,
            target: Target::LinuxX86_64,
            kind: Kind::TarGz,
            minimum_sequence: 1,
            current_release: semver::Version::parse("0.2.0").unwrap(),
            now: Utc::now(),
        },
    )
    .unwrap();
    let prefix = f.root.join("runtime-prefix");
    let prepared =
        dsh_ocean::runtime_bundle::import(&f.root.join("bundle.tar.gz"), &verified, &prefix)
            .unwrap();
    let competing = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(prefix.join(".bootstrap.lock"))
        .unwrap();
    assert_ne!(
        unsafe { libc::flock(competing.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0,
        "ready generation must retain installer lock during health/switch"
    );
    prepared.verify(&verified).unwrap();
    drop(prepared);
    assert_eq!(
        unsafe { libc::flock(competing.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0,
        "normal completion releases installer lock"
    );
}

#[test]
#[ignore = "requires explicitly provided real Node binary; fixed-key synthetic DSH, no user runtime"]
fn signed_runtime_bundle_health_uses_real_node_but_only_disposable_synthetic_state() {
    let mut f = Fixture::new();
    f.bundle(Some("health"));
    let mut args = f.args(false);
    args[0] = "--check-runtime-bundle".into();
    args.extend([
        "--artifact".into(),
        f.root.join("bundle.tar.gz").to_string_lossy().into_owned(),
        "--prefix".into(),
        f.root.join("runtime-prefix").to_string_lossy().into_owned(),
        "--confirm-check".into(),
        "yes".into(),
    ]);
    let output = f.run(&args);
    assert!(output.status.success(), "{}", describe(&output));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["healthChecked"], true);
    assert_eq!(result["activated"], false);
    assert!(!describe(&output).contains("synthetic-never-print"));
    assert!(!f.root.join("home/.dsh").exists());
    assert!(!f.root.join("runtime-prefix/current").exists());
    assert!(!f
        .root
        .join("state/dsh-ocean/runtime-release-state.json")
        .exists());
}

#[test]
fn runtime_download_uses_only_authenticated_url_and_exact_bytes_before_publication() {
    use std::os::unix::fs::PermissionsExt;
    for (bad_signature, transport_failure, bad_payload) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let mut f = Fixture::new();
        f.bundle(None);
        fs::create_dir_all(f.root.join("tools")).unwrap();
        let curl = f.root.join("tools/curl");
        fs::write(&curl,format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{root}/curl-args'\n/bin/cat '{root}/bundle.tar.gz'\nexit {code}\n",root=f.root.display(),code=if transport_failure{28}else{0})).unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
        if bad_payload {
            let path = f.root.join("bundle.tar.gz");
            let mut bytes = fs::read(&path).unwrap();
            bytes[0] ^= 1;
            fs::write(path, bytes).unwrap();
        }
        if bad_signature {
            f.write(dsh_ocean::release_feed::DOMAIN, &f.key);
        }
        let mut args = f.args(false);
        args[0] = "--download-runtime-release".into();
        args.extend([
            "--output".into(),
            f.root
                .join("downloaded.tar.gz")
                .to_string_lossy()
                .into_owned(),
            "--confirm-download".into(),
            "yes".into(),
        ]);
        let output = f.run(&args);
        if bad_signature || transport_failure || bad_payload {
            assert!(!output.status.success(), "{}", describe(&output));
            assert!(!f.root.join("downloaded.tar.gz").exists());
            if bad_signature {
                assert!(
                    !f.root.join("curl-args").exists(),
                    "bad product/domain signature must not start a transport"
                );
            }
        } else {
            assert!(output.status.success(), "{}", describe(&output));
            assert_eq!(
                fs::read(f.root.join("downloaded.tar.gz")).unwrap(),
                fs::read(f.root.join("bundle.tar.gz")).unwrap()
            );
            let args = fs::read_to_string(f.root.join("curl-args")).unwrap();
            assert_eq!(args.lines().next(), Some("--disable"));
            assert!(args.contains(f.manifest["artifacts"][0]["url"].as_str().unwrap()));
            assert!(!f.root.join("state").exists());
            assert!(!f.root.join("home/.dsh").exists());
        }
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

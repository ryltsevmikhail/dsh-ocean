//! Deterministic TEST-ONLY keys, never production trust roots or release keys.
use chrono::{Duration, TimeZone, Utc};
use dsh_ocean::release_feed::{
    self as feed, Channel, DraftArtifact, Kind, Policy, Target, TrustedKey,
};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

const BODY: &[u8] = b"synthetic release artifact\n";
fn signing() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}
fn key_bytes(key: &SigningKey) -> Vec<u8> {
    serde_json::to_vec(&json!({"version":1,"keyId":"test-only","publicKeyHex":feed::hex(key.verifying_key().as_bytes())})).unwrap()
}
fn trust() -> TrustedKey {
    TrustedKey::from_json(&key_bytes(&signing())).unwrap()
}
fn policy() -> Policy {
    Policy {
        channel: Channel::Stable,
        target: Target::LinuxX86_64,
        kind: Kind::AppImage,
        minimum_sequence: 7,
        current_release: semver::Version::parse("0.1.0").unwrap(),
        now: Utc.with_ymd_and_hms(2026, 10, 19, 10, 0, 0).unwrap(),
    }
}
fn manifest() -> Value {
    json!({"version":1,"product":"dsh-ocean","channel":"stable","sequence":7,"release":"0.2.0","publishedAt":"2026-10-19T09:00:00Z","expiresAt":"2026-10-20T09:00:00Z","artifacts":[{"target":"linux-x86_64","kind":"appimage","url":"https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v0.2.0/dsh-ocean-0.2.0-x86_64.AppImage","size":BODY.len(),"sha256":feed::hex(&Sha256::digest(BODY))}]})
}
fn signature(bytes: &[u8], domain: bool) -> Vec<u8> {
    let mut message = if domain {
        feed::DOMAIN.to_vec()
    } else {
        Vec::new()
    };
    message.extend_from_slice(bytes);
    serde_json::to_vec(&json!({"version":1,"keyId":"test-only","signatureHex":feed::hex(&signing().sign(&message).to_bytes())})).unwrap()
}
fn check(value: &Value, policy: &Policy) -> anyhow::Result<feed::Verified> {
    let bytes = serde_json::to_vec(value).unwrap();
    feed::verify(&bytes, &signature(&bytes, true), &trust(), policy)
}
fn cli(fixture: &Fixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dsh-ocean"));
    command
        .current_dir(&fixture.0)
        .env_clear()
        .env("HOME", &fixture.0)
        .env("DSH_HOME", fixture.0.join("must-not-create"))
        .env("PATH", "/nonexistent");
    for (variable, name) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "runtime"),
    ] {
        command.env(variable, fixture.0.join(name));
    }
    command
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dsh-ocean-release-test-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-release-test-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn valid_signature_policy_and_exact_raw_digest_are_bound() {
    let value = manifest();
    let verified = check(&value, &policy()).unwrap();
    assert_eq!(verified.key_id(), "test-only");
    assert_eq!(verified.manifest().sequence, 7);
    assert_eq!(verified.artifact().size, BODY.len() as u64);
    assert_eq!(
        verified.manifest_sha256(),
        feed::hex(&Sha256::digest(serde_json::to_vec(&value).unwrap()))
    );
    let mut other = manifest();
    other["channel"] = json!("next");
    other["release"] = json!("0.2.0-rc.1");
    other["artifacts"][0]["url"]=json!("https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v0.2.0-rc.1/dsh-ocean-0.2.0-rc.1-x86_64.AppImage");
    let mut p = policy();
    p.channel = Channel::Next;
    assert!(check(&other, &p).is_ok());
}
#[test]
fn unsigned_draft_hashes_local_bytes_and_derives_only_canonical_release_metadata() {
    let f = Fixture::new();
    let source = f.0.join("local-private-filename");
    fs::write(&source, BODY).unwrap();
    let mut p = policy();
    p.target = Target::LinuxAarch64;
    p.kind = Kind::TarGz;
    let draft = feed::draft(&source, "0.2.0", 7, 24, &p).unwrap();
    assert_eq!(draft.artifacts[0].size, BODY.len() as u64);
    assert_eq!(draft.artifacts[0].sha256, feed::hex(&Sha256::digest(BODY)));
    assert_eq!(draft.artifacts[0].url, "https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v0.2.0/dsh-ocean-0.2.0-aarch64.tar.gz");
    let bytes = serde_json::to_vec_pretty(&draft).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("local-private-filename"));
    assert!(feed::verify(&bytes, &signature(&bytes, true), &trust(), &p).is_ok());
    assert_eq!(fs::read(&source).unwrap(), BODY);
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}
#[test]
fn multi_draft_hashes_four_unique_pairs_and_each_is_independently_verifiable() {
    let f = Fixture::new();
    let mut sources = Vec::new();
    for (index, (target, kind)) in [
        (Target::LinuxX86_64, Kind::AppImage),
        (Target::LinuxX86_64, Kind::TarGz),
        (Target::LinuxAarch64, Kind::AppImage),
        (Target::LinuxAarch64, Kind::TarGz),
    ]
    .into_iter()
    .enumerate()
    {
        let path = f.0.join(format!("private-local-{index}"));
        fs::write(&path, vec![b'x'; index + 1]).unwrap();
        sources.push(DraftArtifact {
            target,
            kind,
            source: path,
        });
    }
    let draft = feed::draft_many(&sources, "0.2.0", 7, 24, &policy()).unwrap();
    assert_eq!(draft.artifacts.len(), 4);
    let bytes = serde_json::to_vec_pretty(&draft).unwrap();
    let signed = signature(&bytes, true);
    assert!(!String::from_utf8_lossy(&bytes).contains("private-local"));
    for (source, artifact) in sources.iter().zip(&draft.artifacts) {
        let mut selected = policy();
        selected.target = source.target;
        selected.kind = source.kind;
        let verified = feed::verify(&bytes, &signed, &trust(), &selected).unwrap();
        feed::verify_artifact(&source.source, &verified).unwrap();
        assert_eq!(artifact.size, fs::metadata(&source.source).unwrap().len());
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 4);
}
#[test]
fn multi_draft_rejects_duplicate_empty_or_excessive_groups_before_source_reads() {
    let missing = PathBuf::from("/nonexistent-synthetic-private");
    let source = || DraftArtifact {
        target: Target::LinuxX86_64,
        kind: Kind::AppImage,
        source: missing.clone(),
    };
    assert!(feed::draft_many(&[], "0.2.0", 7, 24, &policy()).is_err());
    let error = feed::draft_many(&[source(), source()], "0.2.0", 7, 24, &policy())
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("duplicate") && !error.contains("synthetic-private"));
    let too_many = (0..5).map(|_| source()).collect::<Vec<_>>();
    assert!(feed::draft_many(&too_many, "0.2.0", 7, 24, &policy())
        .err()
        .unwrap()
        .to_string()
        .contains("count"));
}
#[test]
fn multi_cli_refuses_partial_output_or_mixed_input_forms_and_retains_legacy_form() {
    let f = Fixture::new();
    fs::write(f.0.join("good"), BODY).unwrap();
    fs::write(f.0.join("empty"), b"").unwrap();
    let common = [
        "--draft-release",
        "--release",
        "0.2.0",
        "--channel",
        "stable",
        "--sequence",
        "7",
        "--valid-hours",
        "24",
    ];
    let legacy = cli(&f)
        .args(common)
        .args([
            "--target",
            "linux-x86_64",
            "--kind",
            "appimage",
            "--artifact",
            "good",
        ])
        .output()
        .unwrap();
    assert!(legacy.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&legacy.stdout).unwrap()["artifacts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for extra in [
        vec![
            "--asset",
            "linux-x86_64",
            "appimage",
            "good",
            "--asset",
            "linux-aarch64",
            "tar.gz",
            "empty",
        ],
        vec![
            "--asset",
            "linux-x86_64",
            "appimage",
            "good",
            "--artifact",
            "good",
        ],
        vec!["--asset", "linux-x86_64", "appimage"],
    ] {
        let result = cli(&f).args(common).args(extra).output().unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
    }
    assert_eq!(fs::read(f.0.join("good")).unwrap(), BODY);
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 2);
}
#[test]
fn draft_rejects_policy_violations_before_reading_an_artifact() {
    let missing = std::path::Path::new("/nonexistent-synthetic-private-artifact");
    for (release, sequence, hours) in [
        ("0.0.9", 7, 24),
        ("0.2.0+build", 7, 24),
        ("0.2.0-rc.1", 7, 24),
        ("0.2.0", 0, 24),
        ("0.2.0", 7, 0),
        ("0.2.0", 7, 721),
    ] {
        let error = feed::draft(missing, release, sequence, hours, &policy())
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("open verification input") && !error.contains("synthetic-private"));
    }
    let mut p = policy();
    p.channel = Channel::Next;
    let huge = format!("0.2.0-{}", "x".repeat(65536));
    assert!(feed::draft(missing, &huge, 7, 24, &p)
        .err()
        .unwrap()
        .to_string()
        .contains("manifest limit"));
    assert!(dsh_ocean::release_draft_cli::run(&[]).is_err());
    let invalid = [
        "--draft-release",
        "--private-key",
        "synthetic-private-value",
    ]
    .map(str::to_owned);
    let error = dsh_ocean::release_draft_cli::run(&invalid)
        .err()
        .unwrap()
        .to_string();
    assert!(!error.contains("synthetic-private"));
}
#[test]
fn draft_refuses_empty_linked_or_nonregular_artifacts_without_writing_files() {
    let f = Fixture::new();
    let source = f.0.join("source");
    fs::write(&source, b"").unwrap();
    assert!(feed::draft(&source, "0.2.0", 7, 24, &policy()).is_err());
    fs::write(&source, BODY).unwrap();
    symlink(&source, f.0.join("link")).unwrap();
    assert!(feed::draft(&f.0.join("link"), "0.2.0", 7, 24, &policy()).is_err());
    fs::hard_link(&source, f.0.join("hard")).unwrap();
    assert!(feed::draft(&source, "0.2.0", 7, 24, &policy()).is_err());
    assert!(feed::draft(&f.0, "0.2.0", 7, 24, &policy()).is_err());
    assert_eq!(fs::read(&source).unwrap(), BODY);
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 3);
}
#[test]
fn tampering_wrong_keys_and_wrong_domains_fail_before_manifest_parsing() {
    let bytes = serde_json::to_vec(&manifest()).unwrap();
    let sig = signature(&bytes, true);
    let mut changed = bytes.clone();
    changed.push(b' ');
    assert!(feed::verify(&changed, &sig, &trust(), &policy()).is_err());
    let wrong = TrustedKey::from_json(&key_bytes(&SigningKey::from_bytes(&[8u8; 32]))).unwrap();
    assert!(feed::verify(&bytes, &sig, &wrong, &policy()).is_err());
    assert!(feed::verify(&bytes, &signature(&bytes, false), &trust(), &policy()).is_err());
    let error = feed::verify(
        b"{synthetic-private-invalid-json",
        &sig,
        &trust(),
        &policy(),
    )
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("signature"));
    assert!(!error.contains("synthetic-private"));
    let mut envelope: Value = serde_json::from_slice(&sig).unwrap();
    envelope["keyId"] = json!("attacker-key");
    assert!(feed::verify(
        &bytes,
        &serde_json::to_vec(&envelope).unwrap(),
        &trust(),
        &policy()
    )
    .is_err());
}
#[test]
fn weak_keys_noncanonical_signature_formats_and_oversized_inputs_are_refused() {
    let mut identity = [0u8; 32];
    identity[0] = 1;
    assert!(TrustedKey::from_json(
        &serde_json::to_vec(
            &json!({"version":1,"keyId":"test-only","publicKeyHex":feed::hex(&identity)})
        )
        .unwrap()
    )
    .is_err());
    assert!(TrustedKey::from_json(&vec![b'x'; 4097]).is_err());
    let bytes = serde_json::to_vec(&manifest()).unwrap();
    let mut envelope: Value = serde_json::from_slice(&signature(&bytes, true)).unwrap();
    let mut message = feed::DOMAIN.to_vec();
    message.extend_from_slice(&bytes);
    let mut noncanonical = signing().sign(&message).to_bytes();
    noncanonical[32..].fill(0xff);
    for sig in [
        "A".repeat(128),
        "00".repeat(63),
        "00".repeat(65),
        "00".repeat(64),
        feed::hex(&noncanonical),
    ] {
        envelope["signatureHex"] = json!(sig);
        assert!(feed::verify(
            &bytes,
            &serde_json::to_vec(&envelope).unwrap(),
            &trust(),
            &policy()
        )
        .is_err());
    }
    assert!(feed::verify(
        &vec![b' '; 65537],
        &signature(&bytes, true),
        &trust(),
        &policy()
    )
    .is_err());
    assert!(feed::verify(&bytes, &vec![b' '; 4097], &trust(), &policy()).is_err());
}
#[test]
fn signed_channel_scope_sequence_and_version_violations_are_refused() {
    for (field, value) in [
        ("version", json!(2)),
        ("product", json!("another-app")),
        ("channel", json!("next")),
        ("sequence", json!(0)),
        ("sequence", json!(6)),
        ("release", json!("0.0.9")),
        ("release", json!("0.2.0+build")),
        ("release", json!("0.2.0-rc.1")),
    ] {
        let mut data = manifest();
        data[field] = value;
        assert!(check(&data, &policy()).is_err(), "{field}");
    }
    let mut p = policy();
    p.minimum_sequence = 8;
    assert!(check(&manifest(), &p).is_err());
    let mut data = manifest();
    data["unexpected"] = json!("synthetic-private-secret");
    let error = check(&data, &policy()).err().unwrap().to_string();
    assert!(!error.contains("synthetic-private"));
    let raw = String::from_utf8(serde_json::to_vec(&manifest()).unwrap())
        .unwrap()
        .replace("\"sequence\":7", "\"sequence\":7,\"sequence\":7");
    assert!(feed::verify(
        raw.as_bytes(),
        &signature(raw.as_bytes(), true),
        &trust(),
        &policy()
    )
    .is_err());
}
#[test]
fn signed_expiry_future_publication_and_excessive_validity_are_refused() {
    let mut invalid_clock = policy();
    invalid_clock.now = chrono::DateTime::<Utc>::MAX_UTC;
    assert!(check(&manifest(), &invalid_clock).is_err());
    for (field, value) in [
        ("expiresAt", "2026-10-19T10:00:00Z"),
        ("expiresAt", "2026-10-19T08:00:00Z"),
        ("expiresAt", "2026-12-19T09:00:00Z"),
        ("publishedAt", "2026-10-19T10:06:00Z"),
        ("publishedAt", "not-a-time"),
    ] {
        let mut data = manifest();
        data[field] = json!(value);
        assert!(check(&data, &policy()).is_err(), "{field} {value}");
    }
}
#[test]
fn signed_artifact_locations_platforms_duplicates_and_sizes_are_strict() {
    for url in ["http://github.com/ryltsevmikhail/dsh-ocean/releases/download/v0.2.0/dsh-ocean-0.2.0-x86_64.AppImage","https://evil.example/private","https://user:pass@github.com/ryltsevmikhail/dsh-ocean/releases/download/v0.2.0/dsh-ocean-0.2.0-x86_64.AppImage","https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v0.2.0/dsh-ocean-0.2.0-x86_64.AppImage?token=secret","https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v0.2.0/../dsh-ocean-0.2.0-x86_64.AppImage"]{let mut data=manifest();data["artifacts"][0]["url"]=json!(url);assert!(check(&data,&policy()).is_err());}
    for size in [0, feed::MAX_ASSET + 1] {
        let mut data = manifest();
        data["artifacts"][0]["size"] = json!(size);
        assert!(check(&data, &policy()).is_err());
    }
    let mut data = manifest();
    data["artifacts"][0]["sha256"] = json!("A".repeat(64));
    assert!(check(&data, &policy()).is_err());
    let mut data = manifest();
    let duplicate = data["artifacts"][0].clone();
    data["artifacts"].as_array_mut().unwrap().push(duplicate);
    assert!(check(&data, &policy()).is_err());
    let mut p = policy();
    p.target = Target::LinuxAarch64;
    assert!(check(&manifest(), &p).is_err());
    p = policy();
    p.kind = Kind::TarGz;
    assert!(check(&manifest(), &p).is_err());
}
#[test]
fn artifact_verification_is_read_only_and_rejects_changed_or_linked_bytes() {
    let f = Fixture::new();
    let path = f.0.join("artifact");
    fs::write(&path, BODY).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let release = check(&manifest(), &policy()).unwrap();
    feed::verify_artifact(&path, &release).unwrap();
    assert_eq!(fs::read(&path).unwrap(), BODY);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let mut wrong = BODY.to_vec();
    wrong[0] ^= 1;
    fs::write(&path, &wrong).unwrap();
    assert!(feed::verify_artifact(&path, &release).is_err());
    fs::write(&path, b"short").unwrap();
    assert!(feed::verify_artifact(&path, &release).is_err());
    fs::write(&path, BODY).unwrap();
    symlink(&path, f.0.join("link")).unwrap();
    assert!(feed::verify_artifact(&f.0.join("link"), &release).is_err());
    fs::hard_link(&path, f.0.join("hard")).unwrap();
    assert!(feed::verify_artifact(&path, &release).is_err());
    assert!(feed::read_input(&f.0, 4096).is_err());
    assert!(feed::read_input(&path, u64::MAX).is_err());
}
#[test]
fn staging_holds_a_private_readonly_copy_independent_of_later_source_changes() {
    let f = Fixture::new();
    let source = f.0.join("source");
    let target = f.0.join("verified.AppImage");
    fs::write(&source, BODY).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    let release = check(&manifest(), &policy()).unwrap();
    let staged = feed::stage_artifact(&source, &target, &release).unwrap();
    assert_eq!(staged.path(), target);
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&source).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert!(staged
        .reader()
        .try_clone()
        .unwrap()
        .write_all(b"cannot write")
        .is_err());
    fs::write(&source, b"source changed after snapshot").unwrap();
    let mut bytes = Vec::new();
    staged
        .reader()
        .try_clone()
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, BODY);
    assert_eq!(fs::read(&target).unwrap(), BODY);
    assert!(feed::stage_artifact(&source, &target, &release).is_err());
    assert_eq!(fs::read(&target).unwrap(), BODY);
    assert_eq!(
        fs::read_dir(&f.0).unwrap().count(),
        2,
        "only source and published copy remain"
    );
    let retained = f.0.join("original-verified");
    assert_eq!(target.parent(), Some(f.0.as_path()));
    assert_eq!(retained.parent(), Some(f.0.as_path()));
    fs::rename(&target, &retained).unwrap();
    fs::write(&target, b"untrusted path replacement").unwrap();
    let mut reader = staged.reader().try_clone().unwrap();
    reader.seek(SeekFrom::Start(0)).unwrap();
    bytes.clear();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, BODY);
    assert!(feed::verify_artifact(&target, &release).is_err());
    assert_eq!(fs::read(retained).unwrap(), BODY);
}
#[test]
fn headless_draft_and_offline_publisher_are_verified_against_independent_rust_fixture_key() {
    let f = Fixture::new();
    let pem_script = r#"const c=require('node:crypto');const key=c.createPrivateKey({key:Buffer.concat([Buffer.from('302e020100300506032b657004220420','hex'),Buffer.alloc(32,7)]),format:'der',type:'pkcs8'});process.stdout.write(key.export({format:'pem',type:'pkcs8'}));"#;
    let pem = Command::new("node")
        .args(["-e", pem_script])
        .env_remove("NODE_OPTIONS")
        .output()
        .unwrap();
    assert!(pem.status.success());
    let key_path = f.0.join("test-only-private.pem");
    fs::write(&key_path, &pem.stdout).unwrap();
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(f.0.join("artifact"), BODY).unwrap();
    fs::set_permissions(f.0.join("artifact"), fs::Permissions::from_mode(0o640)).unwrap();
    fs::write(f.0.join("archive"), b"synthetic second artifact").unwrap();
    let drafted = cli(&f)
        .args([
            "--draft-release",
            "--release",
            "0.2.0",
            "--channel",
            "stable",
            "--sequence",
            "7",
            "--asset",
            "linux-x86_64",
            "appimage",
            "artifact",
            "--asset",
            "linux-x86_64",
            "tar.gz",
            "archive",
            "--valid-hours",
            "24",
        ])
        .output()
        .unwrap();
    assert!(drafted.status.success());
    assert!(String::from_utf8_lossy(&drafted.stderr).contains("unsigned release draft"));
    let bytes = drafted.stdout;
    let manifest_path = f.0.join("manifest.json");
    fs::write(&manifest_path, &bytes).unwrap();
    let output = Command::new("node")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scripts/sign-release.mjs"
        ))
        .arg("--manifest")
        .arg(&manifest_path)
        .arg("--private-key")
        .arg(&key_path)
        .args(["--key-id", "test-only"])
        .env_remove("NODE_OPTIONS")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "signer failed without exposing key material"
    );
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    let expected_key: Value = serde_json::from_slice(&key_bytes(&signing())).unwrap();
    assert_eq!(bundle["publicKey"], expected_key);
    let signature = serde_json::to_vec(&bundle["signature"]).unwrap();
    let mut current_policy = policy();
    current_policy.now = Utc::now();
    let verified = feed::verify(&bytes, &signature, &trust(), &current_policy).unwrap();
    feed::verify_artifact(&f.0.join("artifact"), &verified).unwrap();
    let mut archive_policy = policy();
    archive_policy.now = current_policy.now;
    archive_policy.kind = Kind::TarGz;
    let archive_release = feed::verify(&bytes, &signature, &trust(), &archive_policy).unwrap();
    feed::verify_artifact(&f.0.join("archive"), &archive_release).unwrap();
    assert_eq!(verified.manifest().artifacts.len(), 2);
    fs::write(f.0.join("signature.json"), &signature).unwrap();
    fs::write(f.0.join("independent-key.json"), key_bytes(&signing())).unwrap();
    let final_check = cli(&f)
        .args([
            "--verify-release",
            "manifest.json",
            "signature.json",
            "independent-key.json",
            "--channel",
            "stable",
            "--target",
            "linux-x86_64",
            "--kind",
            "appimage",
            "--min-sequence",
            "7",
            "--artifact",
            "artifact",
        ])
        .output()
        .unwrap();
    assert!(final_check.status.success());
    let result: Value = serde_json::from_slice(&final_check.stdout).unwrap();
    assert_eq!(result["artifactChecked"], true);
    assert_eq!(result["applied"], false);
    assert!(
        feed::verify(&bytes, b"{}", &trust(), &current_policy).is_err(),
        "draft is never an authenticated proof"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE KEY"));
    assert_eq!(fs::read(&key_path).unwrap(), pem.stdout);
    assert_eq!(fs::read(f.0.join("artifact")).unwrap(), BODY);
    assert_eq!(
        fs::metadata(f.0.join("artifact"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 6);
}
#[test]
fn node_crypto_signatures_interoperate_with_rust_strict_verification() {
    let bytes = serde_json::to_vec(&manifest()).unwrap();
    let script = r#"const c=require('node:crypto');let chunks=[];process.stdin.on('data',x=>chunks.push(x));process.stdin.on('end',()=>{const k=c.createPrivateKey({key:Buffer.concat([Buffer.from('302e020100300506032b657004220420','hex'),Buffer.alloc(32,7)]),format:'der',type:'pkcs8'});const msg=Buffer.concat([Buffer.from('DSH Ocean release manifest v1\n'),Buffer.concat(chunks)]);process.stdout.write(c.sign(null,msg,k).toString('hex'));});"#;
    let mut child = Command::new("node")
        .args(["-e", script])
        .env_remove("NODE_OPTIONS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let detached=serde_json::to_vec(&json!({"version":1,"keyId":"test-only","signatureHex":String::from_utf8(output.stdout).unwrap()})).unwrap();
    assert!(feed::verify(&bytes, &detached, &trust(), &policy()).is_ok());
}
#[test]
fn cli_is_offline_headless_and_does_not_apply_or_write_state() {
    let f = Fixture::new();
    let mut value = manifest();
    let now = Utc::now();
    value["publishedAt"] = json!((now - Duration::hours(1)).to_rfc3339());
    value["expiresAt"] = json!((now + Duration::days(1)).to_rfc3339());
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(f.0.join("manifest.json"), &bytes).unwrap();
    fs::write(f.0.join("signature.json"), signature(&bytes, true)).unwrap();
    fs::write(f.0.join("key.json"), key_bytes(&signing())).unwrap();
    fs::write(f.0.join("artifact"), BODY).unwrap();
    let args = [
        "--verify-release",
        "manifest.json",
        "signature.json",
        "key.json",
        "--channel",
        "stable",
        "--target",
        "linux-x86_64",
        "--kind",
        "appimage",
        "--min-sequence",
        "7",
        "--artifact",
        "artifact",
    ];
    let output = cli(&f).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["applied"], false);
    assert_eq!(summary["artifactChecked"], true);
    assert_eq!(summary["publisherTrust"], "operator-supplied-key-only");
    assert!(!f.0.join("must-not-create").exists());
    for name in [".config", "config", "data", "state", "cache", "runtime"] {
        assert!(
            !f.0.join(name).exists(),
            "verifier must not create app state"
        );
    }
    assert_eq!(fs::read(f.0.join("artifact")).unwrap(), BODY);
    let mut stage_args = args.map(str::to_owned).to_vec();
    stage_args[0] = "--stage-release".into();
    stage_args.extend(["--output".into(), "snapshot.AppImage".into()]);
    let staged = cli(&f).args(&stage_args).output().unwrap();
    assert!(
        staged.status.success(),
        "{}",
        String::from_utf8_lossy(&staged.stderr)
    );
    let result: Value = serde_json::from_slice(&staged.stdout).unwrap();
    assert_eq!(result["applied"], false);
    assert!(result["stagedPath"]
        .as_str()
        .unwrap()
        .ends_with("snapshot.AppImage"));
    assert_eq!(fs::read(f.0.join("snapshot.AppImage")).unwrap(), BODY);
    assert_eq!(
        fs::metadata(f.0.join("snapshot.AppImage"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        cli(&f).args(&stage_args).output().unwrap().status.code(),
        Some(1)
    );
    let mut invalid = args.map(str::to_owned).to_vec();
    invalid.extend(["--output".into(), "must-not-stage".into()]);
    assert_eq!(
        cli(&f).args(invalid).output().unwrap().status.code(),
        Some(1)
    );
    assert!(!f.0.join("must-not-stage").exists());
    assert_eq!(fs::read(f.0.join("artifact")).unwrap(), BODY);
    fs::write(f.0.join("manifest.json"), b"{synthetic-private-tampered").unwrap();
    let mut bad_stage = stage_args.clone();
    *bad_stage.last_mut().unwrap() = "must-not-publish".into();
    assert_eq!(
        cli(&f).args(&bad_stage).output().unwrap().status.code(),
        Some(1)
    );
    assert!(!f.0.join("must-not-publish").exists());
    assert!(!fs::read_dir(&f.0).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".dsh-ocean-download-")));
    let rejected = cli(&f).args(args).output().unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(rejected.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains("synthetic-private"));
    assert!(!f.0.join("must-not-create").exists());
    assert_eq!(fs::read(f.0.join("artifact")).unwrap(), BODY);
    let mut bad = args.map(str::to_owned).to_vec();
    bad.extend(["--channel".into(), "synthetic-private-unknown".into()]);
    let error = dsh_ocean::release_cli::run(&bad).err().unwrap().to_string();
    assert!(!error.contains("synthetic-private"));
}

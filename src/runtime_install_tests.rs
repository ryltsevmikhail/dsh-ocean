use super::*;
const MANIFEST: &[u8] = include_bytes!("../tests/fixtures/runtime-install/manifest.json");
#[test]
fn exact_two_architecture_contract_rejects_changed_payload_and_component() {
    let value: serde_json::Value = serde_json::from_slice(MANIFEST).unwrap();
    for field in ["sha256", "nodeArchiveSha256", "url"] {
        let mut altered = value.clone();
        altered["artifacts"][0][field] = serde_json::json!("unreviewed");
        let manifest = serde_json::from_value(altered).unwrap();
        assert!(contract(&manifest).is_err(), "accepted changed {field}");
    }
    let mut altered = value.clone();
    altered["components"]["node"] = serde_json::json!("99.0.0");
    assert!(contract(&serde_json::from_value(altered).unwrap()).is_err());
    assert!(contract(&serde_json::from_value(value).unwrap()).is_ok());
}
#[test]
fn pending_gate_is_the_coordinated_path() {
    assert_eq!(
        pending_path(Path::new("/private/runtime")),
        Path::new("/private/runtime/.runtime-install-pending.json")
    );
}
#[test]
fn cancellation_refuses_before_creating_any_installation() {
    let control = Arc::new(Control::default());
    control.cancel();
    assert!(install(control).is_err());
}
const SIGNATURE: &[u8] = include_bytes!("../tests/fixtures/runtime-install/signature.json");
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("install-test-{}", crate::downloads::unique_id()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn journal(prefix: &Path) -> Journal {
    use sha2::{Digest, Sha256};
    let m: runtime_feed::Manifest = serde_json::from_slice(MANIFEST).unwrap();
    let digest = crate::release_feed::hex(&Sha256::digest(MANIFEST));
    let a = m
        .artifacts
        .iter()
        .find(|a| a.target == crate::update_feed::native_target().unwrap())
        .unwrap();
    Journal {
        schema: 1,
        prefix: prefix.into(),
        target: PathBuf::from(format!(
            "generations/gen.s2{}{}",
            &digest[..16],
            &a.sha256[..16]
        )),
        manifest: MANIFEST.into(),
        signature: SIGNATURE.into(),
    }
}
#[test]
fn refuses_existing_dangling_foreign_and_legacy_selection() {
    for name in ["current", "harness", "runtime", "foreign"] {
        let temp = Temp::new();
        symlink("missing", temp.0.join(name)).unwrap();
        assert!(unused_prefix(&temp.0).is_err(), "accepted {name}");
    }
}
#[test]
fn interrupted_commit_recovers_only_owned_selector_and_preserves_recorded_floor() {
    let temp = Temp::new();
    let j = journal(&temp.0);
    let floor = temp.0.join("floor-proof");
    assert!(commit(&temp.0, &j, &Control::default(), || {
        fs::write(&floor, b"conservative-floor")?;
        anyhow::bail!("simulated durable floor error")
    })
    .is_err());
    assert!(pending_path(&temp.0).is_file());
    assert_eq!(fs::read_link(temp.0.join("current")).unwrap(), j.target);
    recover(&temp.0).unwrap();
    assert!(!temp.0.join("current").is_symlink());
    assert!(!pending_path(&temp.0).exists());
    assert_eq!(fs::read(floor).unwrap(), b"conservative-floor");
}
#[test]
fn successful_commit_has_selector_and_no_journal() {
    let temp = Temp::new();
    let j = journal(&temp.0);
    commit(&temp.0, &j, &Control::default(), || Ok(())).unwrap();
    assert_eq!(fs::read_link(temp.0.join("current")).unwrap(), j.target);
    assert!(!pending_path(&temp.0).exists());
}
#[test]
fn recovery_refuses_foreign_selector_and_tampered_signed_metadata() {
    for tamper in [false, true] {
        let temp = Temp::new();
        let mut j = journal(&temp.0);
        if tamper {
            j.manifest.push(b' ');
        }
        fs::write(pending_path(&temp.0), serde_json::to_vec(&j).unwrap()).unwrap();
        fs::set_permissions(pending_path(&temp.0), fs::Permissions::from_mode(0o600)).unwrap();
        symlink(
            if tamper {
                j.target.clone()
            } else {
                PathBuf::from("generations/foreign")
            },
            temp.0.join("current"),
        )
        .unwrap();
        assert!(recover(&temp.0).is_err());
        assert!(pending_path(&temp.0).exists());
        assert!(temp.0.join("current").is_symlink());
    }
}
#[test]
fn cancelled_commit_does_not_record_or_select() {
    let temp = Temp::new();
    let c = Control::default();
    c.cancel();
    let mut recorded = false;
    assert!(commit(&temp.0, &journal(&temp.0), &c, || {
        recorded = true;
        Ok(())
    })
    .is_err());
    assert!(!recorded);
    assert!(!pending_path(&temp.0).exists());
    assert!(!temp.0.join("current").is_symlink());
}
#[test]
fn owned_shell_installation_is_not_a_runtime_and_is_preserved() {
    let temp = Temp::new();
    let shell = temp.0.join("shell");
    fs::create_dir(&shell).unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(shell.join("unchanged"), b"shell only").unwrap();
    unused_prefix(&temp.0).unwrap();
    assert_eq!(fs::read(shell.join("unchanged")).unwrap(), b"shell only");
}
#[test]
fn arbitrary_directory_is_not_a_retained_authenticated_import() {
    let temp = Temp::new();
    let generations = temp.0.join("generations");
    fs::create_dir(&generations).unwrap();
    fs::create_dir(generations.join(format!("gen.s2{}", "a".repeat(32)))).unwrap();
    assert!(unused_prefix(&temp.0).is_err());
}
#[test]
fn recovery_refuses_symlink_hardlink_unsafe_mode_and_changed_scope_journals() {
    for mode in ["symlink", "hardlink", "mode", "scope", "target"] {
        let temp = Temp::new();
        let mut j = journal(&temp.0);
        if mode == "scope" {
            j.prefix = PathBuf::from("/elsewhere");
        }
        if mode == "target" {
            j.target = PathBuf::from("../outside");
        }
        let pending = pending_path(&temp.0);
        fs::write(&pending, serde_json::to_vec(&j).unwrap()).unwrap();
        fs::set_permissions(&pending, fs::Permissions::from_mode(0o600)).unwrap();
        if mode == "symlink" {
            let other = temp.0.join("other");
            fs::rename(&pending, &other).unwrap();
            symlink(&other, &pending).unwrap();
        }
        if mode == "hardlink" {
            fs::hard_link(&pending, temp.0.join("other")).unwrap();
        }
        if mode == "mode" {
            fs::set_permissions(&pending, fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert!(recover(&temp.0).is_err(), "accepted {mode}");
        assert!(pending.symlink_metadata().is_ok());
    }
}
#[test]
fn selector_creation_never_overwrites_a_racing_writer() {
    let temp = Temp::new();
    let j = journal(&temp.0);
    symlink("foreign", temp.0.join("current")).unwrap();
    let mut recorded = false;
    assert!(commit(&temp.0, &j, &Control::default(), || {
        recorded = true;
        Ok(())
    })
    .is_err());
    assert!(!recorded);
    assert_eq!(
        fs::read_link(temp.0.join("current")).unwrap(),
        Path::new("foreign")
    );
    assert!(recover(&temp.0).is_err());
}
#[test]
fn recovery_without_selector_and_retry_is_idempotent() {
    let temp = Temp::new();
    let j = journal(&temp.0);
    crate::shell_apply::write_json_new(&pending_path(&temp.0), &j).unwrap();
    recover(&temp.0).unwrap();
    recover(&temp.0).unwrap();
    commit(&temp.0, &j, &Control::default(), || Ok(())).unwrap();
    assert_eq!(fs::read_link(temp.0.join("current")).unwrap(), j.target);
}
#[test]
fn pinned_trust_authenticates_genuine_protected_metadata_and_rejects_tampering() {
    assert_eq!(trust().unwrap().fingerprint(), FINGERPRINT);
    assert_eq!(
        trust().unwrap().fingerprint(),
        crate::release_feed::TrustedKey::from_json(include_bytes!(
            "../tests/fixtures/runtime-install/public-key.json"
        ))
        .unwrap()
        .fingerprint()
    );
    let mut signature = SIGNATURE.to_vec();
    signature[5] ^= 1;
    assert!(Journal::target(Path::new("/private"), MANIFEST.to_vec(), signature).is_err());
    let j = Journal::target(Path::new("/private"), MANIFEST.to_vec(), SIGNATURE.to_vec()).unwrap();
    j.validate(Path::new("/private")).unwrap();
}
#[test]
fn exact_contract_rejects_release_identity_missing_and_duplicate_architecture() {
    let value: serde_json::Value = serde_json::from_slice(MANIFEST).unwrap();
    for (key, bad) in [
        ("product", serde_json::json!("dsh-ocean")),
        ("channel", serde_json::json!("stable")),
        ("release", serde_json::json!("0.2.1-rc.2")),
        ("sequence", serde_json::json!(3)),
    ] {
        let mut altered = value.clone();
        altered[key] = bad;
        assert!(contract(&serde_json::from_value(altered).unwrap()).is_err());
    }
    let mut missing = value.clone();
    missing["artifacts"].as_array_mut().unwrap().pop();
    assert!(contract(&serde_json::from_value(missing).unwrap()).is_err());
    let mut duplicate = value.clone();
    duplicate["artifacts"][1] = duplicate["artifacts"][0].clone();
    assert!(contract(&serde_json::from_value(duplicate).unwrap()).is_err());
}

#[test]
fn genuine_signed_bundle_obeys_native_policy_and_expiry() {
    let m: runtime_feed::Manifest = serde_json::from_slice(MANIFEST).unwrap();
    let issued = chrono::DateTime::parse_from_rfc3339(&m.published_at)
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut policy = crate::release_feed::Policy {
        channel: Channel::Next,
        target: crate::update_feed::native_target().unwrap(),
        kind: Kind::TarGz,
        minimum_sequence: 2,
        current_release: semver::Version::parse("0.0.0-0").unwrap(),
        now: issued + chrono::Duration::minutes(1),
    };
    assert!(runtime_feed::verify(MANIFEST, SIGNATURE, &trust().unwrap(), &policy).is_ok());
    policy.now = chrono::DateTime::parse_from_rfc3339(&m.expires_at)
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert!(runtime_feed::verify(MANIFEST, SIGNATURE, &trust().unwrap(), &policy).is_err());
}

#[test]
fn persisted_runtime_floor_survives_recovery_and_refuses_rebound_sequence() {
    const ENV: &str = "OCEAN_INSTALL_FLOOR_TEST_ROOT";
    if let Some(root) = std::env::var_os(ENV) {
        let root = PathBuf::from(root);
        let prefix = root.join("prefix");
        fs::create_dir(&prefix).unwrap();
        let j = journal(&prefix);
        let digest = release_feed::hex(&sha2::Sha256::digest(MANIFEST));
        let mut floor = release_state::Transaction::begin_for(Product::Runtime).unwrap();
        let _lock = crate::profile_backup::operation_lock(&prefix.join(".bootstrap.lock")).unwrap();
        assert!(commit(&prefix, &j, &Control::default(), || {
            floor.record("next", 2, RELEASE, &digest)?;
            anyhow::bail!("crash after durable record")
        })
        .is_err());
        let path = release_state::path_for(Product::Runtime);
        let before = fs::read(&path).unwrap();
        recover(&prefix).unwrap();
        assert_eq!(fs::read(path).unwrap(), before);
        assert!(floor.check("next", 2, RELEASE, &digest).is_ok());
        assert!(floor.check("next", 2, RELEASE, &"a".repeat(64)).is_err());
        assert!(floor.check("next", 1, RELEASE, &digest).is_err());
        return;
    }
    let temp = Temp::new();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact","runtime_install::tests::persisted_runtime_floor_survives_recovery_and_refuses_rebound_sequence","--nocapture"]).env(ENV,&temp.0);
    for (name, leaf) in [
        ("HOME", "home"),
        ("DSH_HOME", "dsh"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
    ] {
        let path = temp.0.join(leaf);
        fs::create_dir(&path).unwrap();
        command.env(name, path);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn retry_removes_only_private_import_scratch_and_preserves_foreign_entries() {
    let temp = Temp::new();
    let generations = temp.0.join("generations");
    fs::create_dir(&generations).unwrap();
    let scratch = generations.join(format!(".new-runtime-{}", crate::downloads::unique_id()));
    fs::create_dir(&scratch).unwrap();
    fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700)).unwrap();
    let target = crate::update_feed::native_target().unwrap();
    let artifact = pinned_artifacts()
        .into_iter()
        .find(|a| a.0 == target)
        .unwrap()
        .3;
    crate::shell_apply::write_json_new(&scratch.join(".runtime-import-owner.json"), &serde_json::json!({"schema":1,"manifestSha256":"55c70b762e4078ecffce701facf92c39746d18a57f5596e1f1820f1bbc4da525","artifactSha256":artifact})).unwrap();
    fs::write(scratch.join("partial-archive"), b"incomplete").unwrap();
    let _lock = crate::profile_backup::operation_lock(&temp.0.join(".bootstrap.lock")).unwrap();
    unused_prefix(&temp.0).unwrap();
    assert!(!scratch.exists());
    for name in [".new-runtime-foreign", ".new-runtime-1-2-3"] {
        let path = generations.join(name);
        if name.ends_with("foreign") {
            fs::create_dir(&path).unwrap();
        } else {
            symlink(&temp.0, &path).unwrap();
        }
        assert!(unused_prefix(&temp.0).is_err());
        assert!(
            path.symlink_metadata().is_ok(),
            "foreign scratch must survive"
        );
        if path.is_symlink() {
            fs::remove_file(path).unwrap();
        } else {
            fs::remove_dir(path).unwrap();
        }
    }
}

#[test]
fn immutable_bootstrap_is_exact_signed_identity_and_not_mutable_feed_policy() {
    let proof = runtime_feed::verify_immutable_bootstrap(MANIFEST, SIGNATURE).unwrap();
    assert_eq!(
        proof.verified().manifest_sha256(),
        "55c70b762e4078ecffce701facf92c39746d18a57f5596e1f1820f1bbc4da525"
    );
    let mut changed = MANIFEST.to_vec();
    changed.push(b' ');
    assert!(runtime_feed::verify_immutable_bootstrap(&changed, SIGNATURE).is_err());
    let mut signature = SIGNATURE.to_vec();
    signature[5] ^= 1;
    assert!(runtime_feed::verify_immutable_bootstrap(MANIFEST, &signature).is_err());
    let policy = crate::release_feed::Policy {
        channel: Channel::Next,
        target: crate::update_feed::native_target().unwrap(),
        kind: Kind::TarGz,
        minimum_sequence: 2,
        current_release: semver::Version::parse("0.0.0-0").unwrap(),
        now: chrono::DateTime::parse_from_rfc3339("2040-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    };
    assert!(runtime_feed::verify(MANIFEST, SIGNATURE, &trust().unwrap(), &policy).is_err());
}

#[test]
fn recovery_preserves_nonempty_unmarked_and_corrupt_marker_scratch() {
    for marker in [false, true] {
        let temp = Temp::new();
        let generations = temp.0.join("generations");
        fs::create_dir(&generations).unwrap();
        let scratch = generations.join(format!(".new-runtime-{}", crate::downloads::unique_id()));
        fs::create_dir(&scratch).unwrap();
        fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(scratch.join("foreign"), b"preserve").unwrap();
        if marker {
            fs::write(scratch.join(".runtime-import-owner.json"), b"{}").unwrap();
        }
        let _lock = crate::profile_backup::operation_lock(&temp.0.join(".bootstrap.lock")).unwrap();
        assert!(unused_prefix(&temp.0).is_err());
        assert_eq!(fs::read(scratch.join("foreign")).unwrap(), b"preserve");
    }
}

#[test]
fn retry_removes_empty_unmarked_staging_but_refuses_foreign_owner_and_fifo() {
    let temp = Temp::new();
    let generations = temp.0.join("generations");
    fs::create_dir(&generations).unwrap();
    let _lock = crate::profile_backup::operation_lock(&temp.0.join(".bootstrap.lock")).unwrap();
    let empty = generations.join(format!(".new-runtime-{}", crate::downloads::unique_id()));
    fs::create_dir(&empty).unwrap();
    fs::set_permissions(&empty, fs::Permissions::from_mode(0o700)).unwrap();
    unused_prefix(&temp.0).unwrap();
    assert!(!empty.exists());
    let foreign = generations.join(format!(".new-runtime-{}", crate::downloads::unique_id()));
    fs::create_dir(&foreign).unwrap();
    fs::set_permissions(&foreign, fs::Permissions::from_mode(0o700)).unwrap();
    crate::shell_apply::write_json_new(&foreign.join(".runtime-import-owner.json"), &serde_json::json!({"schema":1,"manifestSha256":"a".repeat(64),"artifactSha256":"b".repeat(64)})).unwrap();
    assert!(unused_prefix(&temp.0).is_err());
    assert!(foreign.exists());
    fs::remove_dir_all(&foreign).unwrap();
    let fifo = generations.join(format!(".new-runtime-{}", crate::downloads::unique_id()));
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(unused_prefix(&temp.0).is_err());
    assert!(fifo.symlink_metadata().is_ok());
}

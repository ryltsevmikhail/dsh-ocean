//! Import authenticated *complete* runtime bytes, without installing packages,
//! running payloads, switching the live selector or accepting a sequence floor.
use crate::{checked_archive, release_feed, runtime_feed, shell_apply};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
const RECEIPT: &str = ".ocean-runtime.json";
const FILES: &str = ".ocean-runtime-files.json";
const MAX_FILES: u64 = 64 * 1024 * 1024;
const ENTRIES: [&str; 3] = [
    "runtime/node/bin/node",
    "harness/node_modules/.bin/dsh",
    "tools/node_modules/.bin/pnpm",
];
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    schema: u32,
    release: String,
    sequence: u64,
    channel: release_feed::Channel,
    target: release_feed::Target,
    components: runtime_feed::Components,
    state_scope: String,
    manifest_sha256: String,
    artifact_sha256: String,
    artifact_size: u64,
    node_archive_sha256: String,
    files_sha256: String,
    files_size: u64,
}
pub struct Prepared {
    root: PathBuf,
    expected: checked_archive::Inventory,
    identity: (String, String),
    // Same lock as the existing bootstrap/activation worker. Keep this guard
    // through subsequent health/activation, not merely until extract returns.
    _lock: File,
}
impl Prepared {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn summary(&self) -> Value {
        json!({"generation":self.root,"imported":true,"activated":false,"accepted":false,"healthChecked":false,"warning":"Complete signed bytes imported and indexed only. No payload executed, package manager run, selector/floor changed or profile accessed."})
    }
    pub fn verify(&self, release: &runtime_feed::Verified) -> Result<()> {
        anyhow::ensure!(
            self.identity.0 == release.manifest_sha256()
                && self.identity.1 == release.artifact().sha256,
            "prepared runtime authority changed"
        );
        verify_directory(&self.root, release)?;
        self.expected.verify(&self.root, &[RECEIPT, FILES])
    }
}
fn generation_name(release: &runtime_feed::Verified) -> String {
    // Existing selected_private_root requires an ASCII-alphanumeric gen. suffix.
    format!(
        "gen.s{}{}{}",
        release.manifest().sequence,
        &release.manifest_sha256()[..16],
        &release.artifact().sha256[..16]
    )
}
fn versions(release: &runtime_feed::Verified) -> String {
    let components = &release.manifest().components;
    format!(
        "{}\n{}\n{}\n{}\n",
        components.node,
        components.dsh,
        components.pnpm,
        release.artifact().node_archive_sha256
    )
}
fn owned(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0
            && metadata.nlink() == 1,
        "unsafe runtime import metadata"
    );
    release_feed::read_input(path, limit)
}
fn check_inventory(inventory: &checked_archive::Inventory) -> Result<()> {
    for name in ENTRIES {
        anyhow::ensure!(
            inventory
                .files
                .get(name)
                .is_some_and(|file| file.executable && file.size > 0),
            "runtime archive lacks a regular executable entrypoint"
        );
    }
    for name in [RECEIPT, FILES, ".previous"] {
        anyhow::ensure!(
            !inventory.files.contains_key(name) && !inventory.directories.contains(name),
            "runtime archive collides with transaction-owned metadata"
        );
    }
    anyhow::ensure!(
        inventory.files.contains_key(".versions"),
        "runtime archive lacks component version record"
    );
    Ok(())
}
fn verify_directory(root: &Path, release: &runtime_feed::Verified) -> Result<()> {
    crate::util::check_owned_ancestors(root)?;
    let metadata = fs::symlink_metadata(root)?;
    anyhow::ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "unowned runtime generation"
    );
    let receipt: Receipt =
        serde_json::from_slice(&owned(&root.join(RECEIPT), release_feed::MAX_MANIFEST)?)
            .context("invalid runtime import receipt")?;
    let manifest = release.manifest();
    let artifact = release.artifact();
    anyhow::ensure!(
        receipt.schema == 1
            && receipt.release == manifest.release
            && receipt.sequence == manifest.sequence
            && receipt.channel == manifest.channel
            && receipt.target == artifact.target
            && serde_json::to_value(&receipt.components)?
                == serde_json::to_value(&manifest.components)?
            && receipt.state_scope == manifest.state_scope
            && receipt.manifest_sha256 == release.manifest_sha256()
            && receipt.artifact_sha256 == artifact.sha256
            && receipt.artifact_size == artifact.size
            && receipt.node_archive_sha256 == artifact.node_archive_sha256,
        "runtime generation does not match authenticated bundle"
    );
    anyhow::ensure!(
        receipt.files_size > 0 && receipt.files_size <= MAX_FILES,
        "runtime inventory size refused"
    );
    let bytes = owned(&root.join(FILES), MAX_FILES)?;
    anyhow::ensure!(
        bytes.len() as u64 == receipt.files_size
            && release_feed::hex(&Sha256::digest(&bytes)) == receipt.files_sha256,
        "runtime inventory integrity mismatch"
    );
    let inventory: checked_archive::Inventory =
        serde_json::from_slice(&bytes).context("invalid runtime inventory")?;
    check_inventory(&inventory)?;
    inventory.verify(root, &[RECEIPT, FILES])?;
    anyhow::ensure!(
        owned(&root.join(".versions"), 4096)? == versions(release).as_bytes(),
        "runtime component version record mismatch"
    );
    Ok(())
}
fn sync_directory(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?
        .sync_all()
        .context("sync runtime generation directory")
}

pub fn import(source: &Path, release: &runtime_feed::Verified, prefix: &Path) -> Result<Prepared> {
    let prefix = shell_apply::absolute(prefix)?;
    crate::util::check_owned_ancestors(&prefix)?;
    let home = crate::config::dsh_home();
    let home = fs::canonicalize(&home).unwrap_or(home);
    anyhow::ensure!(
        !prefix.starts_with(&home) && !home.starts_with(&prefix),
        "runtime installation must be separate from Harness state"
    );
    shell_apply::ensure_prefix(&prefix)?;
    let lock = crate::profile_backup::operation_lock(&prefix.join(".bootstrap.lock"))
        .context("another runtime install/update is active")?;
    let generations = prefix.join("generations");
    shell_apply::ensure_prefix(&generations)?;
    let root = generations.join(generation_name(release));
    let temporary = generations.join(format!(".new-runtime-{}", crate::downloads::unique_id()));
    let guard = checked_archive::NewDirectory::create(&temporary)?;
    // Durable ownership is established before any archive/content bytes exist.
    shell_apply::write_json_new(
        &temporary.join(STAGING_OWNER),
        &StagingOwner {
            schema: 1,
            manifest_sha256: release.manifest_sha256().into(),
            artifact_sha256: release.artifact().sha256.clone(),
        },
    )?;
    sync_directory(&temporary)?;
    sync_directory(&generations)?;
    let staged = release_feed::stage_authenticated_artifact(
        source,
        &temporary.join("archive.tar.gz"),
        &release.artifact().artifact(),
    )?;
    let content = temporary.join("content");
    let mut reader = staged.reader().try_clone()?;
    let inventory = checked_archive::extract(
        &mut reader,
        &release.root_name(),
        &content,
        checked_archive::Limits::default(),
    )
    .context("extract authenticated runtime archive")?;
    check_inventory(&inventory)?;
    anyhow::ensure!(
        owned(&content.join(".versions"), 4096)? == versions(release).as_bytes(),
        "runtime component version record mismatch"
    );
    // Retained receipts/inventories are not authority. Reconstruct expectation
    // from freshly hash-authenticated compressed bytes even on retry.
    match fs::symlink_metadata(&root) {
        Ok(_) => {
            verify_directory(&root, release)?;
            inventory.verify(&root, &[RECEIPT, FILES])?;
            return Ok(Prepared {
                root,
                expected: inventory,
                identity: (
                    release.manifest_sha256().into(),
                    release.artifact().sha256.clone(),
                ),
                _lock: lock,
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error).context("inspect retained runtime generation"),
    }
    let bytes = serde_json::to_vec(&inventory)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_FILES,
        "runtime inventory exceeds limit"
    );
    let manifest = release.manifest();
    let artifact = release.artifact();
    let receipt = Receipt {
        schema: 1,
        release: manifest.release.clone(),
        sequence: manifest.sequence,
        channel: manifest.channel,
        target: artifact.target,
        components: manifest.components.clone(),
        state_scope: manifest.state_scope.clone(),
        manifest_sha256: release.manifest_sha256().into(),
        artifact_sha256: artifact.sha256.clone(),
        artifact_size: artifact.size,
        node_archive_sha256: artifact.node_archive_sha256.clone(),
        files_sha256: release_feed::hex(&Sha256::digest(&bytes)),
        files_size: bytes.len() as u64,
    };
    shell_apply::write_json_new(&content.join(FILES), &inventory)?;
    shell_apply::write_json_new(&content.join(RECEIPT), &receipt)?;
    sync_directory(&content)?;
    verify_directory(&content, release)?;
    guard.check()?;
    shell_apply::publish_new(&content, &root)?;
    sync_directory(&generations)?;
    Ok(Prepared {
        root,
        expected: inventory,
        identity: (
            release.manifest_sha256().into(),
            release.artifact().sha256.clone(),
        ),
        _lock: lock,
    })
}

const STAGING_OWNER: &str = ".runtime-import-owner.json";
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StagingOwner {
    schema: u32,
    manifest_sha256: String,
    artifact_sha256: String,
}

/// Caller must hold the prefix lock. Never clean foreign or ambiguous scratch.
pub(crate) fn recover_bootstrap_scratch(path: &Path, manifest: &str, artifact: &str) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("non-UTF8 staging name")?;
    let suffix = name
        .strip_prefix(".new-runtime-")
        .context("not runtime staging")?;
    let fields: Vec<_> = suffix.split('-').collect();
    anyhow::ensure!(
        fields.len() == 3
            && fields
                .iter()
                .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            && fields[0].parse::<u32>().is_ok_and(|v| v > 0)
            && fields[1].parse::<u64>().is_ok()
            && fields[2].parse::<u64>().is_ok(),
        "foreign staging name"
    );
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0,
        "unsafe staging directory"
    );
    let owner_path = path.join(STAGING_OWNER);
    match fs::symlink_metadata(&owner_path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // The sole unmarked recovery case is a crash between mkdir/marker.
            fs::remove_dir(path).context("nonempty unmarked staging preserved")?;
        }
        Err(e) => return Err(e.into()),
        Ok(_) => {
            let owner: StagingOwner = serde_json::from_slice(&owned(&owner_path, 4096)?)?;
            anyhow::ensure!(
                owner.schema == 1
                    && owner.manifest_sha256 == manifest
                    && owner.artifact_sha256 == artifact,
                "foreign staging owner"
            );
            fs::remove_dir_all(path)?;
        }
    }
    sync_directory(path.parent().context("staging parent missing")?)
}

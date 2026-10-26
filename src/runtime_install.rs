//! Authenticated first installation, never an existing-runtime update.
//!
//! GUI callers retain their lifecycle permit. The floor lock precedes the import
//! prefix lock. A pending journal gates all ordinary starts; retry only unselects
//! its own interrupted first install and never rolls back a recorded floor.
use crate::{
    bootstrap, config,
    release_feed::{self, Channel, Kind, Target, TrustedKey},
    release_state, runtime_bundle, runtime_feed, runtime_health, shell_apply,
    update_control::Control,
    update_settings::{Feed, Product, Settings, Source},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{symlink, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

const RELEASE: &str = "0.2.0-rc.2.ocean.2";
const BASE: &str = "https://github.com/ryltsevmikhail/dsh-ocean/releases/download/runtime-next-v0.2.0-rc.2.ocean.2-r2/";
const KEY: &str = r#"{"version":1,"keyId":"ocean-runtime-20261022-v1","publicKeyHex":"6288d3cfd3a9097b634fb964d25dcd96af597404ca4e717d44bb647f7b8ad30c"}"#;
const FINGERPRINT: &str = "91a6ff76087228fb0ae8f45ec72001ae3fb639e1c9ab6a0e4149019219fd3a13";
const MANIFEST_SHA: &str = "55c70b762e4078ecffce701facf92c39746d18a57f5596e1f1820f1bbc4da525";
const JOURNAL_LIMIT: u64 = 1024 * 1024;

pub fn pending_path(prefix: &Path) -> PathBuf {
    prefix.join(".runtime-install-pending.json")
}

fn trust() -> Result<TrustedKey> {
    let key = TrustedKey::from_json(KEY.as_bytes())?;
    anyhow::ensure!(
        key.fingerprint() == FINGERPRINT,
        "bootstrap public key fingerprint mismatch"
    );
    Ok(key)
}
fn settings() -> Result<Settings> {
    trust()?;
    let feed = Feed {
        manifest_url: format!("{BASE}manifest.json"),
        signature_url: format!("{BASE}signature.json"),
        minimum_sequence: 2,
    };
    let result = Settings {
        runtime_channel: Channel::Next,
        runtime: Some(Source {
            key: serde_json::from_str(KEY)?,
            stable: feed.clone(),
            next: feed,
        }),
        ..Settings::default()
    };
    result.validate()?;
    Ok(result)
}
fn pinned_artifacts() -> [(Target, &'static str, u64, &'static str, &'static str); 2] {
    [
        (
            Target::LinuxX86_64,
            "x86_64",
            487101859,
            "05700ecfbbe5e70e495c05927f18903016da3faba38133fa99bcd6540eff64dd",
            "d9f88af72ad34f7ddf8e67289829f1698f54b6254f7d4f54b7453117ff1a3d2c",
        ),
        (
            Target::LinuxAarch64,
            "aarch64",
            480260497,
            "b79d6eeeb46a791238f9c22da39b92fef707c930dfa95b40916291ff893f0c0f",
            "ec53c915c3fdd8786c5685abd25ae3fe9fba0692307f9811d7caaadea1bcaf03",
        ),
    ]
}
fn contract(m: &runtime_feed::Manifest) -> Result<()> {
    anyhow::ensure!(
        m.version == 1
            && m.product == "dsh-ocean-runtime"
            && m.channel == Channel::Next
            && m.release == RELEASE
            && m.sequence == 2
            && m.state_scope == runtime_feed::STATE_SCOPE,
        "unreviewed bootstrap release identity"
    );
    anyhow::ensure!(
        m.components.node == "26.12.0"
            && m.components.dsh == RELEASE
            && m.components.pnpm == "10.34.7",
        "unreviewed bootstrap components"
    );
    anyhow::ensure!(
        m.artifacts.len() == 2,
        "bootstrap requires both reviewed architectures"
    );
    for (target, arch, size, hash, node_hash) in pinned_artifacts() {
        let matches: Vec<_> = m.artifacts.iter().filter(|a| a.target == target).collect();
        anyhow::ensure!(
            matches.len() == 1,
            "missing/duplicate bootstrap architecture"
        );
        let a = matches[0];
        anyhow::ensure!(
            a.kind == Kind::TarGz
                && a.size == size
                && a.sha256 == hash
                && a.node_archive_sha256 == node_hash
                && a.url == format!("{BASE}dsh-ocean-runtime-next-{RELEASE}-2-{arch}.tar.gz"),
            "unreviewed bootstrap archive"
        );
    }
    Ok(())
}

// This policy is deliberately independent of mutable feed freshness. No caller
// can substitute a key, digest, release, channel, component or archive contract.
pub(crate) fn authenticated_bootstrap(
    bytes: &[u8],
    signature: &[u8],
) -> Result<(runtime_feed::Manifest, String)> {
    let key = trust()?;
    key.authenticate(runtime_feed::DOMAIN, bytes, signature)?;
    anyhow::ensure!(
        release_feed::hex(&Sha256::digest(bytes)) == MANIFEST_SHA,
        "unreviewed immutable bootstrap metadata"
    );
    let manifest = serde_json::from_slice(bytes)?;
    contract(&manifest)?;
    Ok((manifest, key.id().into()))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: u32,
    prefix: PathBuf,
    target: PathBuf,
    manifest: Vec<u8>,
    signature: Vec<u8>,
}
impl Journal {
    fn target(prefix: &Path, manifest: Vec<u8>, signature: Vec<u8>) -> Result<Self> {
        let (m, _) = authenticated_bootstrap(&manifest, &signature)?;
        let target = crate::update_feed::native_target()?;
        let a = m
            .artifacts
            .iter()
            .find(|a| a.target == target)
            .context("native bootstrap artifact missing")?;
        let digest = release_feed::hex(&Sha256::digest(&manifest));
        Ok(Self {
            schema: 1,
            prefix: prefix.into(),
            target: PathBuf::from(format!(
                "generations/gen.s2{}{}",
                &digest[..16],
                &a.sha256[..16]
            )),
            manifest,
            signature,
        })
    }
    fn validate(&self, prefix: &Path) -> Result<()> {
        let expected = Self::target(prefix, self.manifest.clone(), self.signature.clone())?;
        anyhow::ensure!(
            self.schema == 1 && self.prefix == prefix && self.target == expected.target,
            "first-install journal scope/target mismatch"
        );
        Ok(())
    }
}
fn sync(prefix: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(prefix)?
        .sync_all()?;
    Ok(())
}
fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn read_journal(path: &Path) -> Result<Journal> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let m = file.metadata()?;
    anyhow::ensure!(
        m.is_file()
            && m.uid() == unsafe { libc::geteuid() }
            && m.mode() & 0o077 == 0
            && m.nlink() == 1
            && m.len() <= JOURNAL_LIMIT,
        "unsafe first-install journal"
    );
    let mut bytes = Vec::new();
    Read::take(&mut file, JOURNAL_LIMIT + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= JOURNAL_LIMIT,
        "first-install journal grew"
    );
    Ok(serde_json::from_slice(&bytes)?)
}
fn recover(prefix: &Path) -> Result<()> {
    let path = pending_path(prefix);
    if !exists(&path)? {
        return Ok(());
    }
    let journal = read_journal(&path)?;
    journal.validate(prefix)?;
    let current = prefix.join("current");
    if exists(&current)? {
        anyhow::ensure!(
            fs::symlink_metadata(&current)?.file_type().is_symlink()
                && fs::read_link(&current)? == journal.target,
            "foreign first-install selector; recovery refused"
        );
        fs::remove_file(&current)?;
        sync(prefix)?;
    }
    fs::remove_file(path)?;
    sync(prefix)
}
// This is only a prefix admission check, not payload authority. Import::verify
// rehashes the full retained tree against authenticated metadata before execution.
fn retained_header(root: &Path, name: &str) -> Result<()> {
    let path = root.join(".ocean-runtime.json");
    let m = fs::symlink_metadata(&path)?;
    anyhow::ensure!(
        m.is_file()
            && m.uid() == unsafe { libc::geteuid() }
            && m.mode() & 0o022 == 0
            && m.nlink() == 1,
        "unsafe retained import receipt"
    );
    let receipt: serde_json::Value = serde_json::from_slice(&release_feed::read_input(
        &path,
        release_feed::MAX_MANIFEST,
    )?)?;
    let digest = receipt["manifestSha256"]
        .as_str()
        .context("missing retained manifest digest")?;
    anyhow::ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid retained digest"
    );
    let target = crate::update_feed::native_target()?;
    let (_, _, size, hash, node_hash) = pinned_artifacts()
        .into_iter()
        .find(|a| a.0 == target)
        .context("unsupported bootstrap target")?;
    anyhow::ensure!(
        receipt["schema"] == 1
            && receipt["channel"] == "next"
            && receipt["release"] == RELEASE
            && receipt["sequence"] == 2
            && receipt["target"] == serde_json::to_value(target)?
            && receipt["artifactSha256"] == hash
            && receipt["artifactSize"] == size
            && receipt["nodeArchiveSha256"] == node_hash
            && receipt["stateScope"] == runtime_feed::STATE_SCOPE
            && receipt["components"]
                == serde_json::json!({"node":"26.12.0","dsh":RELEASE,"pnpm":"10.34.7"})
            && name == format!("gen.s2{}{}", &digest[..16], &hash[..16]),
        "foreign retained import identity"
    );
    Ok(())
}
fn unused_prefix(prefix: &Path) -> Result<()> {
    crate::util::check_owned_ancestors(prefix)?;
    crate::runtime_switch::ensure_no_pending(prefix)?;
    anyhow::ensure!(
        !exists(&pending_path(prefix))?,
        "first-install recovery pending"
    );
    for entry in fs::read_dir(prefix)? {
        let entry = entry?;
        let path = entry.path();
        let m = fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o022 == 0,
            "unowned/writable installation entry"
        );
        if entry.file_name() == ".bootstrap.lock" {
            anyhow::ensure!(m.is_file() && m.nlink() == 1, "unsafe bootstrap lock");
        } else if entry.file_name() == "shell" {
            // Managed Ocean shell state is independent: do not inspect or alter it.
            anyhow::ensure!(m.is_dir(), "unsafe managed shell directory");
        } else if entry.file_name() == "generations" {
            anyhow::ensure!(m.is_dir(), "unsafe generations directory");
            for generation in fs::read_dir(path)? {
                let generation = generation?;
                let name = generation.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(".new-runtime-") {
                    let native = crate::update_feed::native_target()?;
                    let hash = pinned_artifacts()
                        .into_iter()
                        .find(|a| a.0 == native)
                        .context("unsupported target")?
                        .3;
                    runtime_bundle::recover_bootstrap_scratch(
                        &generation.path(),
                        MANIFEST_SHA,
                        hash,
                    )?;
                    continue;
                }
                let g = fs::symlink_metadata(generation.path())?;
                anyhow::ensure!(
                    g.is_dir()
                        && g.uid() == unsafe { libc::geteuid() }
                        && g.mode() & 0o022 == 0
                        && name
                            .strip_prefix("gen.s2")
                            .is_some_and(|suffix| suffix.len() == 32
                                && suffix.bytes().all(|b| b.is_ascii_hexdigit())),
                    "foreign retained runtime generation"
                );
                retained_header(&generation.path(), &name)?;
            }
        } else {
            anyhow::bail!("existing/foreign runtime prefix; first installation refused");
        }
    }
    Ok(())
}
fn no_runtime(prefix: &Path, control: &Control) -> Result<()> {
    unused_prefix(prefix)?;
    anyhow::ensure!(
        bootstrap::detect().is_none(),
        "an existing runtime was detected; first installation refused"
    );
    control.check()
}
fn commit(
    prefix: &Path,
    journal: &Journal,
    control: &Control,
    record: impl FnOnce() -> Result<()>,
) -> Result<()> {
    journal.validate(prefix)?;
    control.begin_commit()?;
    shell_apply::write_json_new(&pending_path(prefix), journal)?;
    sync(prefix)?;
    // symlink(2) is atomic create-if-absent, never a rename over a foreign link.
    symlink(&journal.target, prefix.join("current"))?;
    sync(prefix)?;
    record()?;
    fs::remove_file(pending_path(prefix))?;
    sync(prefix)
}

/// Install only the exact reviewed native runtime; callers own GUI/server leases.
/// Cancellation returns Err with Control.cancel_requested, never false success.
pub fn install(control: Arc<Control>) -> Result<()> {
    control.check()?;
    let prefix = shell_apply::absolute(&config::data_dir())?;
    shell_apply::ensure_prefix(&prefix)?;
    let mut floor = release_state::Transaction::begin_for(Product::Runtime)?;
    {
        let _lock: File = crate::profile_backup::operation_lock(&prefix.join(".bootstrap.lock"))?;
        anyhow::ensure!(
            !exists(&prefix.join(".runtime-pending.json"))?,
            "managed runtime recovery pending"
        );
        recover(&prefix)?; // Before detect: pending selection is not an old runtime.
        no_runtime(&prefix, &control)?;
    }
    let settings = settings()?;
    let feed = settings.source(Product::Runtime)?.feed(Channel::Next);
    let manifest = crate::release_download::fetch_metadata_controlled(
        &feed.manifest_url,
        release_feed::MAX_MANIFEST,
        control.clone(),
    )?;
    let signature = crate::release_download::fetch_metadata_controlled(
        &feed.signature_url,
        release_feed::MAX_AUXILIARY,
        control.clone(),
    )?;
    let proof = runtime_feed::verify_immutable_bootstrap(&manifest, &signature)?;
    let verified = proof.verified();
    floor.check("next", 2, RELEASE, verified.manifest_sha256())?;
    let parent = config::state_dir().join("runtime-first-install-cache");
    shell_apply::ensure_prefix(&parent)?;
    let cache = parent.join(format!("download-{}", crate::downloads::unique_id()));
    let _cache = crate::checked_archive::NewDirectory::create(&cache)?;
    let staged = crate::release_download::download_runtime_preparatory(
        verified,
        &cache.join("artifact.tar.gz"),
        control.clone(),
    )?;
    control.check()?;
    let prepared = runtime_bundle::import(staged.path(), verified, &prefix)?;
    no_runtime(&prefix, &control)?;
    runtime_health::check_immutable_bootstrap(&prepared, &proof, control.clone())?;
    let proof = runtime_feed::verify_immutable_bootstrap(&manifest, &signature)?;
    let verified = proof.verified();
    prepared.verify(verified)?;
    floor.check("next", 2, RELEASE, verified.manifest_sha256())?;
    no_runtime(&prefix, &control)?;
    let journal = Journal::target(&prefix, manifest, signature)?;
    anyhow::ensure!(
        prefix.join(&journal.target) == prepared.root(),
        "imported bootstrap root differs from journal"
    );
    commit(&prefix, &journal, &control, || {
        floor
            .record("next", 2, RELEASE, verified.manifest_sha256())
            .map(|_| ())
    })
}

#[cfg(test)]
#[path = "runtime_install_tests.rs"]
mod tests;

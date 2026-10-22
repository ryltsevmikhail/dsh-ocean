//! Local signed shell activation: global sequence lock, prefix lock, durable
//! selector journal and hash-checked retained versions. Never runs a binary or
//! restarts a service. Abrupt interruption requires explicit recovery.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::release_feed::{self, Verified};
use crate::release_state;

const CURRENT: &str = "current";
const PREVIOUS: &str = ".previous";
const BINARY: &str = "dsh-ocean";
const RECEIPT: &str = "receipt.json";
const PENDING: &str = ".activation-pending.json";
const LOCK: &str = ".shell-update.lock";
const FILES: &str = ".ocean-files.json";
const MAX_FILES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesReceipt {
    size: u64,
    sha256: String,
}
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: u32,
    release: String,
    sequence: u64,
    manifest_sha256: String,
    artifact_sha256: String,
    size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    files: Option<FilesReceipt>,
}
impl Receipt {
    fn from_verified(release: &Verified) -> Self {
        Self {
            schema: 1,
            release: release.manifest().release.clone(),
            sequence: release.manifest().sequence,
            manifest_sha256: release.manifest_sha256().to_owned(),
            artifact_sha256: release.artifact().sha256.clone(),
            size: release.artifact().size,
            files: None,
        }
    }
    fn target(&self) -> Result<String> {
        anyhow::ensure!(
            self.schema == 1
                && self.sequence > 0
                && self.size > 0
                && self.size <= release_feed::MAX_ASSET,
            "invalid retained receipt"
        );
        semver::Version::parse(&self.release).context("invalid retained version")?;
        if let Some(files) = &self.files {
            anyhow::ensure!(
                files.size > 0 && files.size <= MAX_FILES,
                "invalid installed inventory size"
            );
        }
        for digest in [&self.manifest_sha256, &self.artifact_sha256]
            .into_iter()
            .chain(self.files.iter().map(|files| &files.sha256))
        {
            anyhow::ensure!(
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid retained digest"
            );
        }
        let target = format!(
            "versions/{}-{}-{}",
            self.release,
            self.sequence,
            &self.manifest_sha256[..8]
        );
        validate_target(&target)?;
        Ok(target)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selectors {
    current: Option<String>,
    previous: Option<String>,
}
impl Selectors {
    fn read(prefix: &Path) -> Result<Self> {
        Ok(Self {
            current: link_target(prefix, CURRENT)?,
            previous: link_target(prefix, PREVIOUS)?,
        })
    }
    fn validate(&self) -> Result<()> {
        for target in [&self.current, &self.previous].into_iter().flatten() {
            validate_target(target)?;
        }
        Ok(())
    }
    fn verify(&self, prefix: &Path) -> Result<()> {
        self.validate()?;
        for target in [&self.current, &self.previous].into_iter().flatten() {
            verify_retained(prefix, target)?;
        }
        Ok(())
    }
    fn restore(&self, prefix: &Path) -> Result<()> {
        set_selector(prefix, PREVIOUS, self.previous.as_deref())?;
        set_selector(prefix, CURRENT, self.current.as_deref())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    schema: u32,
    before: Selectors,
    after: Selectors,
}

pub(crate) fn absolute(prefix: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        prefix.is_absolute()
            && prefix != Path::new("/")
            && prefix
                .components()
                .all(|p| matches!(p, Component::RootDir | Component::Normal(_))),
        "install prefix must be absolute and normalized, not the filesystem root"
    );
    Ok(prefix.to_path_buf())
}

pub(crate) use crate::util::check_owned_ancestors as check_ancestors;

pub(crate) fn ensure_prefix(prefix: &Path) -> Result<()> {
    check_ancestors(prefix)?;
    match fs::symlink_metadata(prefix) {
        Ok(m) => anyhow::ensure!(m.is_dir(), "install prefix is not a directory"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(prefix)
            .context("create install prefix")?,
        Err(e) => return Err(e).context("inspect install prefix"),
    }
    let m = fs::symlink_metadata(prefix)?;
    anyhow::ensure!(
        m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o022 == 0,
        "install prefix must be owned and not writable by others"
    );
    check_ancestors(prefix)
}

fn validate_target(target: &str) -> Result<()> {
    let name = target
        .strip_prefix("versions/")
        .context("invalid activation selector")?;
    anyhow::ensure!(
        !name.is_empty()
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+')),
        "invalid activation selector"
    );
    Ok(())
}

fn link_target(prefix: &Path, name: &str) -> Result<Option<String>> {
    match fs::symlink_metadata(prefix.join(name)) {
        Ok(m) => {
            anyhow::ensure!(
                m.file_type().is_symlink(),
                "{name} must be a symlink, not a file or directory"
            );
            let target = fs::read_link(prefix.join(name)).context("read activation selector")?;
            let target = target
                .to_str()
                .context("non-UTF8 activation selector")?
                .to_owned();
            validate_target(&target)?;
            Ok(Some(target))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("inspect activation selector"),
    }
}

fn sync_dir(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?
        .sync_all()
        .context("sync activation directory")
}

fn scratch(prefix: &Path, name: &str) -> PathBuf {
    prefix.join(format!(
        ".{name}.new.{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn set_selector(prefix: &Path, name: &str, target: Option<&str>) -> Result<()> {
    // Validate the existing type even when removing an originally absent selector.
    let existing = link_target(prefix, name)?;
    if existing.as_deref() == target {
        return Ok(());
    }
    if let Some(target) = target {
        validate_target(target)?;
        let temporary = scratch(prefix, name);
        std::os::unix::fs::symlink(target, &temporary).context("stage activation selector")?;
        if let Err(e) = fs::rename(&temporary, prefix.join(name)) {
            let _ = fs::remove_file(&temporary);
            return Err(e).context("activate selector");
        }
    } else if existing.is_some() {
        fs::remove_file(prefix.join(name))?;
    }
    sync_dir(prefix)
}

pub(crate) fn write_json_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("missing receipt directory")?;
    anyhow::ensure!(
        fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "activation metadata already exists"
    );
    let temporary = scratch(
        parent,
        path.file_name()
            .context("missing receipt basename")?
            .to_str()
            .context("invalid receipt basename")?,
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        // The private prefix is operation-locked; no other Ocean writer can
        // publish this name. Same-UID hostile filesystem mutation is not a sandbox.
        fs::rename(&temporary, path)?;
        sync_dir(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn read_owned(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "activation metadata must be owned and not writable by others"
    );
    release_feed::read_input(path, limit)
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = read_owned(path, release_feed::MAX_MANIFEST)?;
    serde_json::from_slice(&bytes).context("invalid activation metadata")
}

fn verify_retained(prefix: &Path, target: &str) -> Result<Receipt> {
    verify_directory(&prefix.join(target), target)
}
fn verify_directory(directory: &Path, target: &str) -> Result<Receipt> {
    use sha2::{Digest, Sha256};
    validate_target(target)?;
    check_ancestors(directory)?;
    let receipt: Receipt = read_json(&directory.join(RECEIPT))?;
    anyhow::ensure!(
        receipt.target()? == target,
        "retained receipt does not match selector"
    );
    let binary = directory.join(BINARY);
    let m = fs::symlink_metadata(&binary)?;
    anyhow::ensure!(
        m.is_file()
            && m.uid() == unsafe { libc::geteuid() }
            && m.mode() & 0o022 == 0
            && m.mode() & 0o111 != 0,
        "unsafe retained executable"
    );
    if let Some(files) = &receipt.files {
        let bytes = read_owned(&directory.join(FILES), MAX_FILES)?;
        anyhow::ensure!(
            bytes.len() as u64 == files.size
                && release_feed::hex(&Sha256::digest(&bytes)) == files.sha256,
            "installed inventory integrity mismatch"
        );
        let inventory: crate::checked_archive::Inventory = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid installed inventory"))?;
        anyhow::ensure!(
            inventory
                .files
                .get(BINARY)
                .is_some_and(|entry| entry.executable),
            "package does not contain its executable entrypoint"
        );
        inventory.verify(directory, &[RECEIPT, FILES])?;
    } else {
        let (_, digest) = release_feed::artifact_digest(&binary, Some(receipt.size))?;
        anyhow::ensure!(
            digest == receipt.artifact_sha256,
            "retained executable SHA-256 mismatch"
        );
    }
    Ok(receipt)
}

fn no_pending(prefix: &Path) -> Result<()> {
    anyhow::ensure!(
        fs::symlink_metadata(prefix.join(PENDING))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "incomplete shell activation; use --recover-shell with explicit confirmation"
    );
    Ok(())
}

fn known_partial(prefix: &Path, pending: &Pending) -> Result<()> {
    let actual = Selectors::read(prefix)?;
    anyhow::ensure!(
        (actual.current == pending.before.current || actual.current == pending.after.current)
            && (actual.previous == pending.before.previous
                || actual.previous == pending.after.previous),
        "activation selectors changed outside the pending transaction"
    );
    Ok(())
}

struct Activation {
    prefix: PathBuf,
    _lock: File,
    pending: Pending,
    receipt: Value,
    complete: bool,
}
impl Activation {
    fn begin(
        prefix: PathBuf,
        lock: File,
        before: Selectors,
        after: Selectors,
        receipt: Value,
    ) -> Result<Self> {
        let pending = Pending {
            schema: 1,
            before,
            after,
        };
        write_json_new(&prefix.join(PENDING), &pending)?;
        Ok(Self {
            prefix,
            _lock: lock,
            pending,
            receipt,
            complete: false,
        })
    }
    fn activate(&mut self) -> Result<()> {
        anyhow::ensure!(
            Selectors::read(&self.prefix)? == self.pending.before,
            "activation selectors changed before commit"
        );
        self.pending.after.restore(&self.prefix)
    }
    fn finish(&mut self) -> Result<Value> {
        anyhow::ensure!(
            Selectors::read(&self.prefix)? == self.pending.after,
            "activation selectors changed during commit"
        );
        fs::remove_file(self.prefix.join(PENDING))?;
        sync_dir(&self.prefix)?;
        self.complete = true;
        Ok(self.receipt.clone())
    }
    fn abort(&mut self) -> Result<()> {
        if self.complete {
            return Ok(());
        }
        self.pending.before.verify(&self.prefix)?;
        known_partial(&self.prefix, &self.pending)?;
        if !self.prefix.join(PENDING).try_exists()? {
            write_json_new(&self.prefix.join(PENDING), &self.pending)?;
        }
        self.pending.before.restore(&self.prefix)?;
        fs::remove_file(self.prefix.join(PENDING))?;
        sync_dir(&self.prefix)?;
        self.complete = true;
        Ok(())
    }
}
// Intentionally no automatic Drop recovery: after panic/abort the durable
// journal survives and a later process requires explicit verified recovery.

pub(crate) fn publish_new(source: &Path, destination: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
    // SAFETY: normalized paths inside the owned locked prefix; NUL-terminated
    // CStrings outlive renameat2 and NOREPLACE cannot clobber an existing version.
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .context("publish new version without overwrite");
    }
    Ok(())
}
fn stage_version(
    source: &Path,
    release: &Verified,
    destination: &Path,
    target: &str,
    receipt: &mut Receipt,
) -> Result<()> {
    use sha2::{Digest, Sha256};
    let parent = destination.parent().context("missing versions directory")?;
    let temporary = parent.join(format!(".new-{}", crate::downloads::unique_id()));
    let guard = crate::checked_archive::NewDirectory::create(&temporary)?;
    let content = temporary.join("content");
    match release.artifact().kind {
        release_feed::Kind::AppImage => {
            fs::DirBuilder::new().mode(0o700).create(&content)?;
            let staged = release_feed::stage_artifact(source, &content.join(BINARY), release)?;
            staged
                .reader()
                .set_permissions(fs::Permissions::from_mode(0o755))?;
            staged.reader().sync_all()?;
        }
        release_feed::Kind::TarGz => {
            let archive =
                release_feed::stage_artifact(source, &temporary.join("archive.tar.gz"), release)?;
            let arch = match release.artifact().target {
                release_feed::Target::LinuxX86_64 => "x86_64",
                release_feed::Target::LinuxAarch64 => "aarch64",
            };
            let root = format!("dsh-ocean-{}-linux-{arch}", release.manifest().release);
            let mut input = archive.reader().try_clone()?;
            let inventory = crate::checked_archive::extract(
                &mut input,
                &root,
                &content,
                crate::checked_archive::Limits::default(),
            )
            .context("extract authenticated shell archive")?;
            anyhow::ensure!(
                inventory
                    .files
                    .get(BINARY)
                    .is_some_and(|entry| entry.executable),
                "archive lacks its executable shell entrypoint"
            );
            anyhow::ensure!(
                !inventory.files.contains_key(RECEIPT) && !inventory.files.contains_key(FILES),
                "archive collides with updater metadata"
            );
            let bytes = serde_json::to_vec(&inventory)?;
            anyhow::ensure!(
                bytes.len() as u64 <= MAX_FILES,
                "installed inventory exceeds limit"
            );
            receipt.files = Some(FilesReceipt {
                size: bytes.len() as u64,
                sha256: release_feed::hex(&Sha256::digest(&bytes)),
            });
            write_json_new(&content.join(FILES), &inventory)?;
        }
    }
    write_json_new(&content.join(RECEIPT), receipt)?;
    sync_dir(&content)?;
    verify_directory(&content, target)?;
    guard.check()?;
    publish_new(&content, destination)?;
    sync_dir(parent)
}

fn prepare(staged: &Path, release: &Verified, prefix: &Path) -> Result<Activation> {
    let prefix = absolute(prefix)?;
    ensure_prefix(&prefix)?;
    let lock = crate::profile_backup::operation_lock(&prefix.join(LOCK))?;
    no_pending(&prefix)?;
    let before = Selectors::read(&prefix)?;
    before.verify(&prefix)?;
    let mut expected = Receipt::from_verified(release);
    let target = expected.target()?;
    let directory = prefix.join(&target);
    ensure_prefix(&prefix.join("versions"))?;
    match fs::symlink_metadata(&directory) {
        Ok(_) => {
            let existing = verify_retained(&prefix, &target)?;
            anyhow::ensure!(
                existing.files.is_some() == (release.artifact().kind == release_feed::Kind::TarGz),
                "existing package kind mismatch"
            );
            expected.files = existing.files.clone();
            anyhow::ensure!(existing == expected, "existing activation receipt mismatch");
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            stage_version(staged, release, &directory, &target, &mut expected)?
        }
        Err(error) => return Err(error).context("inspect retained version directory"),
    }
    let mut after = before.clone();
    if before.current.as_ref() != Some(&target) {
        after.previous = before.current.clone();
        after.current = Some(target);
    }
    let receipt = json!({"prefix":prefix,"version":expected.release,"sequence":expected.sequence,"manifestSha256":expected.manifest_sha256,"activated":prefix.join(CURRENT).join(BINARY),"previous":before.current,"retained":directory,"serviceRestarted":false,"warning":"Activated on disk only; the running executable is unchanged. Previous verified bytes are retained for explicit rollback."});
    Activation::begin(prefix, lock, before, after, receipt)
}

/// Global state guard precedes the prefix guard. A normal recording error
/// restores selectors; the remembered floor is never reduced, even if a state
/// rename committed before a later durability error was reported.
pub fn apply(
    staged: &Path,
    release: &Verified,
    prefix: &Path,
) -> Result<(Value, release_state::Entry)> {
    let channel = match release.manifest().channel {
        release_feed::Channel::Stable => "stable",
        release_feed::Channel::Next => "next",
    };
    let mut state = release_state::Transaction::begin()?;
    state.check(
        channel,
        release.manifest().sequence,
        &release.manifest().release,
        release.manifest_sha256(),
    )?;
    let mut activation = prepare(staged, release, prefix)?;
    let result = (|| -> Result<_> {
        activation.activate()?;
        let entry = state.record(
            channel,
            release.manifest().sequence,
            &release.manifest().release,
            release.manifest_sha256(),
        )?;
        Ok((activation.finish()?, entry))
    })();
    match result {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            activation.abort().with_context(|| format!("activation failed ({error:#}); selector recovery incomplete; use --recover-shell"))?;
            Err(error).context("activation refused; original selectors restored")
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub release: String,
    pub sequence: u64,
    pub kind: release_feed::Kind,
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationStatus {
    pub current: Option<VersionInfo>,
    pub previous: Option<VersionInfo>,
    pub pending_recovery: bool,
}

/// Read-only integrity snapshot, not a mutation permit. Every mutator reacquires
/// its locks and rechecks selectors/bytes rather than trusting this UI snapshot.
pub fn inspect(prefix: &Path) -> Result<InstallationStatus> {
    let prefix = absolute(prefix)?;
    check_ancestors(&prefix)?;
    match fs::symlink_metadata(&prefix) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InstallationStatus::default())
        }
        Err(error) => return Err(error).context("inspect shell destination"),
        Ok(meta) => anyhow::ensure!(
            meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o022 == 0,
            "unsafe shell destination"
        ),
    }
    let selectors = Selectors::read(&prefix)?;
    let info = |target: &String| -> Result<VersionInfo> {
        let receipt = verify_retained(&prefix, target)?;
        Ok(VersionInfo {
            release: receipt.release,
            sequence: receipt.sequence,
            kind: if receipt.files.is_some() {
                release_feed::Kind::TarGz
            } else {
                release_feed::Kind::AppImage
            },
        })
    };
    Ok(InstallationStatus {
        current: selectors.current.as_ref().map(info).transpose()?,
        previous: selectors.previous.as_ref().map(info).transpose()?,
        pending_recovery: fs::symlink_metadata(prefix.join(PENDING)).is_ok(),
    })
}

pub fn rollback(prefix: &Path) -> Result<Value> {
    let prefix = absolute(prefix)?;
    ensure_prefix(&prefix)?;
    let lock = crate::profile_backup::operation_lock(&prefix.join(LOCK))?;
    no_pending(&prefix)?;
    let before = Selectors::read(&prefix)?;
    let current = before
        .current
        .clone()
        .context("no activated version to roll back from")?;
    let previous = before
        .previous
        .clone()
        .context("no previous activation is retained")?;
    anyhow::ensure!(
        current != previous,
        "previous activation equals the current version"
    );
    before.verify(&prefix)?;
    let after = Selectors {
        current: Some(previous.clone()),
        previous: Some(current.clone()),
    };
    let receipt = json!({"prefix":prefix,"activated":prefix.join(CURRENT).join(BINARY),"rolledBackFrom":current,"rolledBackTo":previous,"serviceRestarted":false,"warning":"Verified retained version selected; the running process and remembered sequence floor are unchanged."});
    let mut activation = Activation::begin(prefix, lock, before, after, receipt)?;
    let result = activation.activate().and_then(|()| activation.finish());
    if result.is_err() {
        activation
            .abort()
            .context("rollback failed; recovery incomplete")?;
    }
    result
}

/// Restore only the original verified selectors from an interrupted operation.
/// The global antirollback floor is deliberately left unchanged.
pub fn recover(prefix: &Path) -> Result<Value> {
    let prefix = absolute(prefix)?;
    ensure_prefix(&prefix)?;
    let _lock = crate::profile_backup::operation_lock(&prefix.join(LOCK))?;
    let pending: Pending = read_json(&prefix.join(PENDING))?;
    anyhow::ensure!(pending.schema == 1, "unsupported activation journal");
    pending.before.validate()?;
    pending.after.validate()?;
    pending.before.verify(&prefix)?;
    known_partial(&prefix, &pending)?;
    pending.before.restore(&prefix)?;
    fs::remove_file(prefix.join(PENDING))?;
    sync_dir(&prefix)?;
    Ok(
        json!({"prefix":prefix,"recovered":true,"serviceRestarted":false,"warning":"Original verified selectors restored; accepted sequence floor was not reset."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_unsafe_prefixes_and_selectors() {
        assert!(absolute(Path::new("relative")).is_err());
        assert!(absolute(Path::new("/")).is_err());
        assert!(absolute(Path::new("/tmp/../other")).is_err());
        for target in [
            "../../outside",
            "versions/../outside",
            "versions/.hidden",
            "versions/a/b",
        ] {
            assert!(validate_target(target).is_err());
        }
    }
}

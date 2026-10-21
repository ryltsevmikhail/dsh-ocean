//! Linux profile-configuration snapshots. Never copies global credentials or
//! session stores, follows symlinks, installs packages, or restores shared data.
//! Snapshot consistency is best-effort; restore requires an offline guard and
//! always retains the replaced profile. This is integrity checking, not signing.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const EXCLUDED: [&str; 2] = [".git", "node_modules"];
pub(crate) const MAX_ENTRIES: usize = 10000;
pub(crate) const MAX_FILE: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_TOTAL: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_MANIFEST: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub format_version: u32,
    pub profile: String,
    pub created_at: String,
    pub scope: String,
    pub consistency: String,
    pub excluded: Vec<String>,
    pub directories: Vec<String>,
    pub files: Vec<BackupFile>,
    /// Client version that created the snapshot (absent in old manifests).
    #[serde(default)]
    pub client_version: String,
    /// Detected DSH runtime version at snapshot time, when known.
    #[serde(default)]
    pub runtime_version: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub executable: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReceipt {
    pub profile: String,
    pub retained_profile: Option<PathBuf>,
    pub warnings: Vec<String>,
    pub snapshot_runtime_version: Option<String>,
    pub current_runtime_version: Option<String>,
    pub data_downgrade: bool,
    pub data_downgrade_allowed: bool,
}

/// True when restoring `snapshot` would move profile data back to an older DSH
/// runtime than the one currently selected. Schema compatibility is the
/// publisher's/operator's responsibility, so this is a refusal, not a rewrite.
pub fn would_downgrade_data(snapshot: Option<&str>, current: Option<&str>) -> bool {
    let (Some(snapshot), Some(current)) = (snapshot, current) else {
        return false;
    };
    match (
        semver::Version::parse(snapshot),
        semver::Version::parse(current),
    ) {
        (Ok(snapshot), Ok(current)) => snapshot < current,
        // Unknown/unparseable versions cannot be compared safely; do not invent
        // a refusal the operator cannot check, and never claim compatibility.
        _ => false,
    }
}

pub struct ProfileStore {
    home: PathBuf,
    backups: PathBuf,
}
struct Lock {
    _file: File,
} // Closing this descriptor releases flock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) len: u64,
    pub(crate) mtime: i64,
    pub(crate) mtime_ns: i64,
    pub(crate) ctime: i64,
    pub(crate) ctime_ns: i64,
    pub(crate) mode: u32,
}
impl Stamp {
    pub(crate) fn of(file: &File) -> Result<Self> {
        let m = file.metadata()?;
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
            len: m.len(),
            mtime: m.mtime(),
            mtime_ns: m.mtime_nsec(),
            ctime: m.ctime(),
            ctime_ns: m.ctime_nsec(),
            mode: m.mode(),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) directory: bool,
    pub(crate) stamp: Stamp,
}
pub(crate) type Inventory = BTreeMap<String, Item>;

fn component(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && name.len() <= 255
            && !name.contains(['/', '\\'])
            && !name.chars().any(char::is_control),
        "invalid profile path component"
    );
    Ok(())
}
pub fn profile_name(name: &str) -> Result<()> {
    component(name)?;
    anyhow::ensure!(
        name.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            && !name.starts_with('.')
            && name.len() <= 64,
        "invalid profile name (maximum 64 ASCII characters)"
    );
    Ok(())
}
fn relative(path: &str) -> Result<()> {
    anyhow::ensure!(
        !path.is_empty() && path.len() <= 4096,
        "invalid backup path length"
    );
    for part in path.split('/') {
        component(part)?;
        anyhow::ensure!(
            !EXCLUDED.contains(&part),
            "backup references an excluded tree"
        );
    }
    Ok(())
}
fn c_name(name: &str) -> Result<CString> {
    component(name)?;
    Ok(CString::new(name)?)
}
pub(crate) fn proc_path(dir: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()))
}
pub(crate) fn open_dir(path: &Path) -> Result<File> {
    // Linux resolves a trailing slash or `/.` before applying O_NOFOLLOW to
    // the final component. Strip lexical separators/dots, but retain `..`:
    // canonicalization would follow the very link this open must reject.
    let normalized: PathBuf = path.components().collect();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&normalized)
        .with_context(|| format!("open directory {}", path.display()))
}
pub(crate) fn open_at(dir: &File, name: &str, directory: bool) -> Result<File> {
    let name = c_name(name)?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: dir is an owned live directory descriptor; name is NUL-terminated.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("open profile entry without following links");
    }
    // SAFETY: openat returned a new descriptor owned exclusively by this File.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    anyhow::ensure!(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file() && metadata.nlink() == 1
        },
        "only real directories and single-linked regular files are supported"
    );
    Ok(file)
}
pub(crate) fn parent_at(root: &File, path: &str) -> Result<(File, String)> {
    relative(path)?;
    let mut parts: Vec<_> = path.split('/').collect();
    let name = parts.pop().unwrap().to_owned();
    let mut dir = root.try_clone()?;
    for part in parts {
        dir = open_at(&dir, part, true)?;
    }
    Ok((dir, name))
}
pub(crate) fn file_at(root: &File, path: &str) -> Result<File> {
    let (parent, name) = parent_at(root, path)?;
    open_at(&parent, &name, false)
}
pub(crate) fn make_dir_at(root: &File, path: &str) -> Result<File> {
    let (parent, name) = parent_at(root, path)?;
    let name = c_name(&name)?;
    // SAFETY: descriptors and CString remain live for the syscall.
    let rc = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("create private snapshot directory");
    }
    open_at(&parent, name.to_str()?, true)
}
pub(crate) fn create_file_at(root: &File, path: &str, executable: bool) -> Result<File> {
    let (parent, name) = parent_at(root, path)?;
    let name = c_name(&name)?;
    // SAFETY: arguments are owned, valid descriptors/CStrings. O_EXCL prevents replacement.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            if executable { 0o700 } else { 0o600 },
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("create private snapshot file");
    }
    // SAFETY: successful openat transfers one fresh descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub(crate) fn rename_at(parent: &File, old: &str, new: &str, flags: u32) -> Result<()> {
    let old = c_name(old)?;
    let new = c_name(new)?;
    // SAFETY: both names are direct children of the same owned directory.
    let rc = unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            old.as_ptr(),
            parent.as_raw_fd(),
            new.as_ptr(),
            flags,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("atomic profile directory activation");
    }
    Ok(())
}
struct Stage<'a> {
    parent: &'a File,
    name: String,
    remove: bool,
}
impl Drop for Stage<'_> {
    fn drop(&mut self) {
        if self.remove {
            // Only this process's unique temporary child, never a profile name.
            assert!(self.name.starts_with(".ocean-stage-"));
            let _ = fs::remove_dir_all(proc_path(self.parent).join(&self.name));
        }
    }
}
pub(crate) fn id() -> Result<String> {
    Ok(format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ))
}
/// Per-engine caps for the shared, no-follow scanners.
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub max_file: u64,
    pub max_total: u64,
    pub max_entries: usize,
    pub label: &'static str,
}

pub(crate) const PROFILE_LIMITS: Limits = Limits {
    max_file: MAX_FILE,
    max_total: MAX_TOTAL,
    max_entries: MAX_ENTRIES,
    label: "profile",
};

pub(crate) fn names_controlled(
    dir: &File,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<Vec<String>> {
    checkpoint()?;
    let mut names = Vec::new();
    for entry in fs::read_dir(proc_path(dir))? {
        checkpoint()?;
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF8 profile filename"))?;
        component(&name)?;
        anyhow::ensure!(names.len() < MAX_ENTRIES, "directory entry limit exceeded");
        names.push(name);
    }
    names.sort();
    checkpoint()?;
    Ok(names)
}
fn scan(root: &File, exclude: bool) -> Result<Inventory> {
    scan_controlled(root, exclude, PROFILE_LIMITS, &mut || Ok(()))
}
pub(crate) fn scan_controlled(
    root: &File,
    exclude: bool,
    limits: Limits,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<Inventory> {
    struct WalkState<'a> {
        exclude: bool,
        limits: Limits,
        items: &'a mut Inventory,
        total: &'a mut u64,
        checkpoint: &'a mut dyn FnMut() -> Result<()>,
    }

    fn walk(dir: &File, prefix: &str, depth: usize, state: &mut WalkState<'_>) -> Result<()> {
        (state.checkpoint)()?;
        anyhow::ensure!(depth <= 32, "profile nesting limit exceeded");
        for name in names_controlled(dir, state.checkpoint)? {
            (state.checkpoint)()?;
            if state.exclude && EXCLUDED.contains(&name.as_str()) {
                continue;
            }
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            relative(&path)?;
            let kind = fs::symlink_metadata(proc_path(dir).join(&name))?.file_type();
            anyhow::ensure!(
                kind.is_dir() || kind.is_file(),
                "symlink or special profile entry refused: {path}"
            );
            (state.checkpoint)()?;
            let file = open_at(dir, &name, kind.is_dir())?;
            let stamp = Stamp::of(&file)?;
            (state.checkpoint)()?;
            if !kind.is_dir() {
                anyhow::ensure!(
                    stamp.len <= state.limits.max_file,
                    "{} file too large: {path}",
                    state.limits.label
                );
                *state.total = state
                    .total
                    .checked_add(stamp.len)
                    .context("snapshot size overflow")?;
                anyhow::ensure!(
                    *state.total <= state.limits.max_total,
                    "{} exceeds snapshot size limit",
                    state.limits.label
                );
            }
            state.items.insert(
                path.clone(),
                Item {
                    directory: kind.is_dir(),
                    stamp,
                },
            );
            anyhow::ensure!(
                state.items.len() <= state.limits.max_entries,
                "{} entry limit exceeded",
                state.limits.label
            );
            if kind.is_dir() {
                walk(&file, &path, depth + 1, state)?;
            }
        }
        Ok(())
    }
    let mut result = Inventory::new();
    let mut total = 0u64;
    walk(
        root,
        "",
        0,
        &mut WalkState {
            exclude,
            limits,
            items: &mut result,
            total: &mut total,
            checkpoint,
        },
    )?;
    Ok(result)
}
fn digest_copy(
    source: File,
    destination: Option<&mut File>,
    expected: &Stamp,
) -> Result<(u64, String)> {
    digest_copy_controlled(source, destination, expected, MAX_FILE, &mut |_| Ok(()))
}
pub(crate) fn digest_copy_controlled(
    mut source: File,
    mut destination: Option<&mut File>,
    expected: &Stamp,
    limit: u64,
    checkpoint: &mut dyn FnMut(u64) -> Result<()>,
) -> Result<(u64, String)> {
    checkpoint(0)?;
    anyhow::ensure!(
        Stamp::of(&source)? == *expected,
        "profile changed before copy"
    );
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buf = [0u8; 65536];
    loop {
        checkpoint(size)?;
        let n = source.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        anyhow::ensure!(
            size <= limit && size <= expected.len,
            "profile changed or exceeded file limit"
        );
        hash.update(&buf[..n]);
        if let Some(ref mut file) = destination {
            file.write_all(&buf[..n])?;
        }
        checkpoint(size)?;
    }
    anyhow::ensure!(
        size == expected.len && Stamp::of(&source)? == *expected,
        "profile changed while copying"
    );
    if let Some(file) = destination {
        file.sync_all()?;
    }
    Ok((size, crate::release_feed::hex(&hash.finalize())))
}

fn destination_path(path: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        path.is_absolute()
            && path.components().all(|part| matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )),
        "backup destination must be an absolute normalized path"
    );
    let mut ancestor = path;
    let mut tail = Vec::new();
    loop {
        match fs::canonicalize(ancestor) {
            Ok(mut root) => {
                for name in tail.into_iter().rev() {
                    root.push(name);
                }
                return Ok(root);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tail.push(
                    ancestor
                        .file_name()
                        .context("invalid backup destination")?
                        .to_os_string(),
                );
                ancestor = ancestor.parent().context("invalid backup destination")?;
            }
            Err(error) => return Err(error).context("resolve backup destination"),
        }
    }
}

/// Hold this file for the operation lifetime. Never remove a lock file on exit.
pub fn operation_lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    anyhow::ensure!(file.metadata()?.is_file(), "invalid operation lock");
    // SAFETY: a live descriptor; flock does not access file contents.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    anyhow::ensure!(rc == 0, "another operation is in progress");
    Ok(file)
}

impl ProfileStore {
    pub fn new(home: &Path, backups: &Path) -> Result<Self> {
        let home = fs::canonicalize(home).context("DSH home must exist")?;
        let proposed = destination_path(backups)?;
        anyhow::ensure!(
            !proposed.starts_with(&home) && !home.starts_with(&proposed),
            "backups must be outside DSH home"
        );
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(backups)?;
        let backups = fs::canonicalize(backups)?;
        anyhow::ensure!(
            backups == proposed,
            "backup destination changed during creation"
        );
        anyhow::ensure!(
            !backups.starts_with(&home) && !home.starts_with(&backups),
            "backups must be outside DSH home"
        );
        Ok(Self { home, backups })
    }
    fn profiles(&self) -> Result<File> {
        open_at(&open_dir(&self.home)?, "profiles", true)
    }
    fn lock(&self, profiles: &File) -> Result<Lock> {
        Ok(Lock {
            _file: operation_lock(&proc_path(profiles).join(".dsh-ocean-backup.lock"))?,
        })
    }
    pub fn create(&self, profile: &str) -> Result<PathBuf> {
        self.create_recording(
            profile,
            crate::bootstrap::detect().map(|runtime| runtime.version),
        )
    }

    /// Snapshot creation with an explicit runtime version, so callers (and tests)
    /// never depend on a live runtime probe.
    pub fn create_recording(
        &self,
        profile: &str,
        runtime_version: Option<String>,
    ) -> Result<PathBuf> {
        profile_name(profile)?;
        let profiles = self.profiles()?;
        let _lock = self.lock(&profiles)?;
        let source = open_at(&profiles, profile, true)?;
        let identity = source.metadata()?;
        let before = scan(&source, true)?;
        anyhow::ensure!(
            before
                .get("package.json")
                .is_some_and(|item| !item.directory),
            "profile has no package.json"
        );
        let backups = open_dir(&self.backups)?;
        let stamp = id()?;
        let name = format!(".ocean-stage-{stamp}");
        let stage_dir = make_dir_at(&backups, &name)?;
        let mut stage = Stage {
            parent: &backups,
            name,
            remove: true,
        };
        let data = make_dir_at(&stage_dir, "data")?;
        let mut manifest = Manifest {
            format_version: 1,
            profile: profile.to_owned(),
            created_at: chrono::Utc::now().to_rfc3339(),
            scope: "profile-config".into(),
            consistency: "best-effort".into(),
            excluded: EXCLUDED.iter().map(|s| (*s).into()).collect(),
            directories: Vec::new(),
            files: Vec::new(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            runtime_version,
        };
        for (path, item) in &before {
            if item.directory {
                make_dir_at(&data, path)?;
                manifest.directories.push(path.clone());
            } else {
                let mut output = create_file_at(&data, path, false)?;
                let (size, sha256) =
                    digest_copy(file_at(&source, path)?, Some(&mut output), &item.stamp)?;
                manifest.files.push(BackupFile {
                    path: path.clone(),
                    size,
                    sha256,
                    executable: item.stamp.mode & 0o100 != 0,
                });
            }
        }
        anyhow::ensure!(
            scan(&source, true)? == before,
            "profile changed during snapshot; retry after stopping writers"
        );
        let current = open_at(&profiles, profile, true)?.metadata()?;
        anyhow::ensure!(
            current.dev() == identity.dev() && current.ino() == identity.ino(),
            "profile directory replaced during snapshot"
        );
        let mut metadata = create_file_at(&stage_dir, "manifest.json", false)?;
        let json = serde_json::to_vec_pretty(&manifest)?;
        anyhow::ensure!(
            json.len() as u64 <= MAX_MANIFEST,
            "snapshot manifest too large"
        );
        metadata.write_all(&json)?;
        metadata.sync_all()?;
        data.sync_all()?;
        stage_dir.sync_all()?;
        let final_name = format!("profile-{profile}-{stamp}");
        rename_at(&backups, &stage.name, &final_name, libc::RENAME_NOREPLACE)?;
        stage.remove = false;
        // Publication has happened; don't misreport a failed snapshot on fsync.
        let _ = backups.sync_all();
        Ok(self.backups.join(final_name))
    }

    pub fn restore<F>(
        &self,
        profile: &str,
        snapshot: &Path,
        allow_data_downgrade: bool,
        current_runtime: Option<&str>,
        mut require_offline: F,
    ) -> Result<RestoreReceipt>
    where
        F: FnMut() -> Result<()>,
    {
        profile_name(profile)?;
        let canonical = fs::canonicalize(snapshot)?;
        anyhow::ensure!(
            !canonical.starts_with(&self.home),
            "cannot restore a snapshot inside DSH home"
        );
        let profiles = self.profiles()?;
        let _lock = self.lock(&profiles)?;
        require_offline()?;
        let previous = match open_at(&profiles, profile, true) {
            Ok(dir) => Some((dir.metadata()?, scan(&dir, true)?)),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let verified = Verified::open(snapshot)?;
        anyhow::ensure!(
            verified.manifest.profile == profile,
            "snapshot profile does not match requested target"
        );
        let current_runtime = current_runtime.map(str::to_owned);
        let snapshot_runtime = verified.manifest.runtime_version.clone();
        let downgrade =
            would_downgrade_data(snapshot_runtime.as_deref(), current_runtime.as_deref());
        anyhow::ensure!(
            !downgrade || allow_data_downgrade,
            "snapshot was taken with DSH {} but the selected runtime is {}; restoring older profile data may be schema-incompatible. Re-run with --allow-data-downgrade only if you accept that risk",
            snapshot_runtime.as_deref().unwrap_or("unknown"),
            current_runtime.as_deref().unwrap_or("unknown")
        );
        let stamp = id()?;
        let name = format!(".ocean-stage-{stamp}");
        let stage_dir = make_dir_at(&profiles, &name)?;
        let mut stage = Stage {
            parent: &profiles,
            name,
            remove: true,
        };
        for path in verified
            .manifest
            .directories
            .iter()
            .collect::<BTreeSet<_>>()
        {
            make_dir_at(&stage_dir, path)?;
        }
        for entry in &verified.manifest.files {
            let mut output = create_file_at(&stage_dir, &entry.path, entry.executable)?;
            let item = verified.inventory.get(&entry.path).unwrap();
            let (size, hash) = digest_copy(
                file_at(&verified.data, &entry.path)?,
                Some(&mut output),
                &item.stamp,
            )?;
            anyhow::ensure!(
                size == entry.size && hash == entry.sha256,
                "snapshot data changed before restore"
            );
        }
        stage_dir.sync_all()?;
        require_offline()?;
        match &previous {
            Some((identity, before)) => {
                let current = open_at(&profiles, profile, true)?;
                let metadata = current.metadata()?;
                anyhow::ensure!(
                    metadata.dev() == identity.dev()
                        && metadata.ino() == identity.ino()
                        && scan(&current, true)? == *before,
                    "target profile changed; restore refused"
                );
            }
            None => anyhow::ensure!(
                fs::symlink_metadata(proc_path(&profiles).join(profile)).is_err(),
                "target profile appeared during restore"
            ),
        }
        rename_at(
            &profiles,
            &stage.name,
            profile,
            if previous.is_some() {
                libc::RENAME_EXCHANGE
            } else {
                libc::RENAME_NOREPLACE
            },
        )?;
        // Never delete after activation: with EXCHANGE the stage now IS the old profile.
        stage.remove = false;
        let mut warnings = vec![
            "node_modules excluded; review manifests before reinstalling dependencies; no scripts were executed".to_owned(),
        ];
        if downgrade {
            warnings.push(format!(
                "profile data restored from an older runtime ({} -> {}); schema compatibility was explicitly accepted by the operator and is not verified",
                snapshot_runtime.as_deref().unwrap_or("unknown"),
                current_runtime.as_deref().unwrap_or("unknown")
            ));
        }
        if snapshot_runtime.is_none() {
            warnings.push("snapshot does not record the DSH runtime version; data compatibility could not be compared".to_owned());
        }
        let mut receipt = RestoreReceipt {
            profile: profile.to_owned(),
            retained_profile: None,
            warnings,
            snapshot_runtime_version: snapshot_runtime,
            current_runtime_version: current_runtime,
            data_downgrade: downgrade,
            data_downgrade_allowed: allow_data_downgrade,
        };
        if previous.is_some() {
            let retained = format!("ocean-retained-{stamp}");
            let old_name =
                if rename_at(&profiles, &stage.name, &retained, libc::RENAME_NOREPLACE).is_ok() {
                    retained
                } else {
                    receipt
                        .warnings
                        .push("previous profile retained under temporary name; not deleted".into());
                    stage.name.clone()
                };
            if open_at(&profiles, &old_name, true)
                .and_then(|dir| {
                    dir.set_permissions(fs::Permissions::from_mode(0o700))
                        .context("restrict retained profile")
                })
                .is_err()
            {
                receipt
                    .warnings
                    .push("could not restrict retained profile directory permissions".into());
            }
            receipt.retained_profile = Some(self.home.join("profiles").join(old_name));
        }
        if profiles.sync_all().is_err() {
            receipt
                .warnings
                .push("profile activated, but directory fsync failed".into());
        }
        Ok(receipt)
    }
}

struct Verified {
    manifest: Manifest,
    data: File,
    inventory: Inventory,
}
/// Aggregate-only read progress; bytes are hashed, not restored/published.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VerificationStage {
    #[default]
    Opening,
    Inventory,
    Hashing,
    Rechecking,
}
impl VerificationStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Opening => "Opening snapshot metadata",
            Self::Inventory => "Scanning bounded inventory",
            Self::Hashing => "Hashing configuration files",
            Self::Rechecking => "Rechecking inventory and identities",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerificationProgress {
    pub stage: VerificationStage,
    pub hashed_bytes: u64,
    pub checked_files: usize,
    pub total_files: Option<usize>,
    pub total_bytes: Option<u64>,
}
impl Verified {
    fn open(path: &Path) -> Result<Self> {
        Self::open_progress(path, &mut |_| Ok(()))
    }
    fn open_progress(
        path: &Path,
        checkpoint: &mut dyn FnMut(VerificationProgress) -> Result<()>,
    ) -> Result<Self> {
        let mut progress = VerificationProgress::default();
        checkpoint(progress)?;
        let snapshot = open_dir(path)?;
        let snapshot_stamp = Stamp::of(&snapshot)?;
        anyhow::ensure!(
            names_controlled(&snapshot, &mut || checkpoint(progress))? == ["data", "manifest.json"],
            "unexpected snapshot root entry"
        );
        let metadata = open_at(&snapshot, "manifest.json", false)?;
        let metadata_stamp = Stamp::of(&metadata)?;
        anyhow::ensure!(
            metadata.metadata()?.len() <= MAX_MANIFEST,
            "snapshot manifest too large"
        );
        let mut raw = Vec::new();
        let mut reader = (&metadata).take(MAX_MANIFEST + 1);
        let mut buffer = [0u8; 65536];
        loop {
            checkpoint(progress)?;
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            raw.extend_from_slice(&buffer[..count]);
            checkpoint(progress)?;
        }
        anyhow::ensure!(
            raw.len() as u64 <= MAX_MANIFEST,
            "snapshot manifest grew while reading"
        );
        let manifest: Manifest =
            serde_json::from_slice(&raw).context("invalid snapshot manifest")?;
        validate_manifest(&manifest)?;
        progress.total_files = Some(manifest.files.len());
        progress.total_bytes = Some(manifest.files.iter().map(|file| file.size).sum());
        progress.stage = VerificationStage::Inventory;
        checkpoint(progress)?;
        let data = open_at(&snapshot, "data", true)?;
        let data_stamp = Stamp::of(&data)?;
        let inventory =
            scan_controlled(&data, false, PROFILE_LIMITS, &mut || checkpoint(progress))?;
        checkpoint(progress)?;
        anyhow::ensure!(
            inventory.len() == manifest.files.len() + manifest.directories.len(),
            "unexpected or missing snapshot data"
        );
        for directory in &manifest.directories {
            anyhow::ensure!(
                inventory.get(directory).is_some_and(|i| i.directory),
                "missing snapshot directory"
            );
        }
        progress.stage = VerificationStage::Hashing;
        checkpoint(progress)?;
        for entry in &manifest.files {
            let item = inventory
                .get(&entry.path)
                .context("missing snapshot file")?;
            anyhow::ensure!(!item.directory, "snapshot file is a directory");
            let base = progress.hashed_bytes;
            let (size, hash) = digest_copy_controlled(
                file_at(&data, &entry.path)?,
                None,
                &item.stamp,
                MAX_FILE,
                &mut |bytes| {
                    progress.hashed_bytes = base + bytes;
                    checkpoint(progress)
                },
            )?;
            anyhow::ensure!(
                size == entry.size && hash == entry.sha256,
                "snapshot integrity check failed: {}",
                entry.path
            );
            progress.checked_files += 1;
            checkpoint(progress)?;
        }
        progress.stage = VerificationStage::Rechecking;
        checkpoint(progress)?;
        anyhow::ensure!(
            scan_controlled(&data, false, PROFILE_LIMITS, &mut || checkpoint(progress))?
                == inventory,
            "snapshot inventory changed during verification"
        );
        anyhow::ensure!(
            names_controlled(&snapshot, &mut || checkpoint(progress))? == ["data", "manifest.json"],
            "snapshot root changed during verification"
        );
        checkpoint(progress)?;
        anyhow::ensure!(
            Stamp::of(&metadata)? == metadata_stamp
                && Stamp::of(&open_at(&snapshot, "manifest.json", false)?)? == metadata_stamp,
            "snapshot manifest changed during verification"
        );
        anyhow::ensure!(
            Stamp::of(&data)? == data_stamp
                && Stamp::of(&open_at(&snapshot, "data", true)?)? == data_stamp,
            "snapshot data root changed during verification"
        );
        anyhow::ensure!(
            Stamp::of(&snapshot)? == snapshot_stamp
                && Stamp::of(&open_dir(path)?)? == snapshot_stamp,
            "snapshot root identity changed during verification"
        );
        checkpoint(progress)?;
        Ok(Self {
            manifest,
            data,
            inventory,
        })
    }
}
/// One snapshot discovered in the backup root. Content-free: name, validated
/// profile/time metadata and manifest counts; never file paths or bytes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSummary {
    pub name: String,
    pub profile: String,
    pub created_at: Option<String>,
    pub files: usize,
    pub directories: usize,
    pub bytes: u64,
    /// Monotonic ordering key taken from the snapshot name, newest last.
    #[serde(skip)]
    sequence: u128,
}

/// Result of an explicit retention pass.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneOutcome {
    pub removed: Vec<String>,
    pub kept: Vec<String>,
    /// Snapshot-shaped entries that could not be validated; never deleted.
    pub unreadable: Vec<String>,
}

#[cfg(test)]
impl SnapshotSummary {
    /// Deterministic constructor for unit tests of pure renderers.
    pub fn for_test(
        name: &str,
        profile: &str,
        created_at: Option<String>,
        files: usize,
        directories: usize,
        bytes: u64,
    ) -> Self {
        Self {
            name: name.to_owned(),
            profile: profile.to_owned(),
            created_at,
            files,
            directories,
            bytes,
            sequence: 0,
        }
    }
}

/// Splits `profile-<name>-<pid>-<nanos>` without trusting the profile text.
pub(crate) fn snapshot_identity(name: &str) -> Option<(String, u128)> {
    let rest = name.strip_prefix("profile-")?;
    let (head, nanos) = rest.rsplit_once('-')?;
    let (profile, pid) = head.rsplit_once('-')?;
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if nanos.is_empty() || !nanos.bytes().all(|b| b.is_ascii_digit()) || nanos.len() > 39 {
        return None;
    }
    profile_name(profile).ok()?;
    Some((profile.to_owned(), nanos.parse().ok()?))
}

fn read_manifest_only(snapshot: &File) -> Result<Manifest> {
    let metadata = open_at(snapshot, "manifest.json", false)?;
    anyhow::ensure!(
        metadata.metadata()?.len() <= MAX_MANIFEST,
        "snapshot manifest too large"
    );
    let mut raw = Vec::new();
    (&metadata).take(MAX_MANIFEST + 1).read_to_end(&mut raw)?;
    anyhow::ensure!(
        raw.len() as u64 <= MAX_MANIFEST,
        "snapshot manifest grew while reading"
    );
    let manifest: Manifest = serde_json::from_slice(&raw).context("invalid snapshot manifest")?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

/// Lists snapshot-shaped direct children of `backups` without hashing their data.
///
/// Symlinks, special files and unrecognised names are ignored rather than
/// followed. A missing backup root lists as empty.
pub fn snapshots(backups: &Path) -> Result<Vec<SnapshotSummary>> {
    match fs::symlink_metadata(backups) {
        Ok(info) if info.is_dir() => {}
        Ok(_) => anyhow::bail!("backup root is not a directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("inspect backup root"),
    }
    let root = open_dir(backups)?;
    let mut result = Vec::new();
    for name in names_controlled(&root, &mut || Ok(()))? {
        let Some((profile, sequence)) = snapshot_identity(&name) else {
            continue;
        };
        let snapshot = match open_at(&root, &name, true) {
            Ok(directory) => directory,
            Err(_) => continue, // not a real directory: never follow it
        };
        let Ok(manifest) = read_manifest_only(&snapshot) else {
            continue;
        };
        if manifest.profile != profile {
            continue; // misleading name; exclude from retention decisions
        }
        result.push(SnapshotSummary {
            name,
            profile,
            created_at: chrono::DateTime::parse_from_rfc3339(&manifest.created_at)
                .ok()
                .map(|time| time.with_timezone(&chrono::Utc).to_rfc3339()),
            files: manifest.files.len(),
            directories: manifest.directories.len(),
            bytes: manifest.files.iter().map(|file| file.size).sum(),
            sequence,
        });
    }
    result.sort_by(|left, right| (left.sequence, &left.name).cmp(&(right.sequence, &right.name)));
    Ok(result)
}

/// Runs a callback before each mutation so callers can cancel cooperatively.
pub type PruneCheckpoint<'a> = &'a mut dyn FnMut() -> Result<()>;

pub(crate) fn remove_tree(
    parent: &File,
    name: &str,
    checkpoint: PruneCheckpoint<'_>,
) -> Result<()> {
    checkpoint()?;
    let directory = open_at(parent, name, true)?;
    for child in names_controlled(&directory, &mut || Ok(()))? {
        checkpoint()?;
        match open_at(&directory, &child, true) {
            Ok(nested) => {
                drop(nested);
                remove_tree(&directory, &child, checkpoint)?;
            }
            Err(_) => {
                // Must be a plain single-linked regular file; anything else
                // (symlink, FIFO, device, hardlinked file) fails closed.
                let file = open_at(&directory, &child, false)
                    .with_context(|| format!("snapshot contains an unsupported entry: {child}"))?;
                drop(file);
                let c_child = c_name(&child)?;
                // SAFETY: owned descriptor and NUL-terminated name.
                let rc = unsafe { libc::unlinkat(directory.as_raw_fd(), c_child.as_ptr(), 0) };
                if rc != 0 {
                    return Err(std::io::Error::last_os_error()).context("remove snapshot file");
                }
            }
        }
    }
    let c_name_owned = c_name(name)?;
    // SAFETY: as above; AT_REMOVEDIR only removes the now-empty directory.
    let rc = unsafe {
        libc::unlinkat(
            parent.as_raw_fd(),
            c_name_owned.as_ptr(),
            libc::AT_REMOVEDIR,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("remove snapshot directory");
    }
    parent.sync_all()?;
    Ok(())
}

/// Deletes the oldest snapshots beyond `keep` newest ones (optionally only for
/// one profile). Snapshot-shaped entries that fail to list or verify are kept
/// and reported instead of being deleted.
pub fn prune(
    backups: &Path,
    keep: usize,
    profile: Option<&str>,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<PruneOutcome> {
    anyhow::ensure!(keep >= 1, "keep at least one snapshot");
    anyhow::ensure!(keep <= 1000, "keep at most 1000 snapshots");
    if let Some(name) = profile {
        profile_name(name)?;
    }
    let all = snapshots(backups)?;
    let mut outcome = PruneOutcome {
        removed: Vec::new(),
        kept: Vec::new(),
        unreadable: Vec::new(),
    };
    let root = if all.is_empty() {
        None
    } else {
        Some(open_dir(backups)?)
    };
    let profiles: Vec<String> = {
        let mut names: Vec<_> = all
            .iter()
            .filter(|snapshot| profile.is_none_or(|wanted| snapshot.profile == wanted))
            .map(|snapshot| snapshot.profile.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    };
    for wanted in profiles {
        let mut for_profile: Vec<_> = all
            .iter()
            .filter(|snapshot| snapshot.profile == wanted)
            .collect();
        for_profile
            .sort_by(|left, right| (right.sequence, &right.name).cmp(&(left.sequence, &left.name)));
        for (index, snapshot) in for_profile.into_iter().enumerate() {
            checkpoint()?;
            if index < keep {
                outcome.kept.push(snapshot.name.clone());
                continue;
            }
            let root = root.as_ref().context("backup root disappeared")?;
            checkpoint()?;
            match remove_tree(root, &snapshot.name, &mut checkpoint) {
                Ok(()) => outcome.removed.push(snapshot.name.clone()),
                Err(error) => {
                    outcome
                        .unreadable
                        .push(format!("{}: {error:#}", snapshot.name));
                }
            }
        }
    }
    outcome.removed.sort();
    outcome.kept.sort();
    outcome.unreadable.sort();
    Ok(outcome)
}

pub fn verify(snapshot: &Path) -> Result<Manifest> {
    Ok(Verified::open(snapshot)?.manifest)
}
/// Read-only cooperative verification. Returning an error from checkpoint stops
/// at stage/chunk boundaries; an in-flight filesystem read/scan is not killed.
/// The callback receives no names, paths, contents or raw manifest fields.
pub fn verify_with_progress(
    snapshot: &Path,
    mut checkpoint: impl FnMut(VerificationProgress) -> Result<()>,
) -> Result<Manifest> {
    Ok(Verified::open_progress(snapshot, &mut checkpoint)?.manifest)
}
/// Parses and validates manifest bytes (used by the export verifier).
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest> {
    let manifest: Manifest = serde_json::from_slice(bytes).context("invalid snapshot manifest")?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    anyhow::ensure!(
        manifest.format_version == 1
            && manifest.scope == "profile-config"
            && manifest.consistency == "best-effort",
        "unsupported snapshot format/scope"
    );
    profile_name(&manifest.profile)?;
    anyhow::ensure!(
        manifest.excluded == EXCLUDED,
        "unsupported snapshot exclusions"
    );
    anyhow::ensure!(
        manifest.files.len() + manifest.directories.len() <= MAX_ENTRIES,
        "snapshot entry limit exceeded"
    );
    let mut directories = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut total = 0u64;
    for path in &manifest.directories {
        relative(path)?;
        anyhow::ensure!(
            directories.insert(path.as_str()),
            "duplicate snapshot directory"
        );
    }
    for entry in &manifest.files {
        relative(&entry.path)?;
        anyhow::ensure!(
            files.insert(entry.path.as_str()) && !directories.contains(entry.path.as_str()),
            "duplicate or conflicting snapshot path"
        );
        anyhow::ensure!(
            entry.size <= MAX_FILE
                && entry.sha256.len() == 64
                && entry.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid snapshot file metadata"
        );
        total = total
            .checked_add(entry.size)
            .context("snapshot size overflow")?;
        anyhow::ensure!(total <= MAX_TOTAL, "snapshot total size limit exceeded");
    }
    anyhow::ensure!(
        files.contains("package.json"),
        "snapshot has no package.json"
    );
    for path in directories.iter().chain(files.iter()) {
        if let Some((parent, _)) = path.rsplit_once('/') {
            anyhow::ensure!(
                directories.contains(parent),
                "snapshot parent directory missing"
            );
        }
    }
    Ok(())
}
#[cfg(test)]
mod data_downgrade_tests {
    use super::would_downgrade_data;

    #[test]
    fn only_strictly_older_known_versions_are_downgrades() {
        assert!(would_downgrade_data(Some("0.2.0"), Some("0.3.0")));
        assert!(!would_downgrade_data(Some("0.3.0"), Some("0.3.0")));
        assert!(!would_downgrade_data(Some("0.4.0"), Some("0.3.0")));
        // Unknown/unparseable versions never invent a refusal.
        assert!(!would_downgrade_data(None, Some("0.3.0")));
        assert!(!would_downgrade_data(Some("0.2.0"), None));
        assert!(!would_downgrade_data(Some("unparseable"), Some("0.3.0")));
        assert!(!would_downgrade_data(Some("0.2.0"), Some("unparseable")));
    }
}

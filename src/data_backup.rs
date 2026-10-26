//! Whole-data (sessions + shared stores) snapshots.
//!
//! The profile-config engine deliberately backs up only the web profile. Sessions
//! and shared stores are much larger and contain conversation/log data, so they
//! get their own explicit snapshot format with their own limits and scope, and
//! explicit offline restore: `--restore-data SNAPSHOT --confirm-offline`.
//! Restore requires independently stopping all writers and retains previous roots.
//!
//! Snapshot creation is read-only with respect to the source tree: the same
//! no-follow, bounded, checksum-verified primitives used for profile snapshots.
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::profile_backup::{
    self, create_file_at, digest_copy_controlled, file_at, make_dir_at, names_controlled, open_at,
    open_dir, scan_controlled, Stamp,
};

/// Manifest scope of a whole-data snapshot.
pub const SCOPE: &str = "dsh-data";
pub const FORMAT_VERSION: u32 = 1;
/// Larger than the configuration engine: sessions legitimately grow.
pub const MAX_FILE: u64 = 512 * 1024 * 1024;
/// Caps this engine hands to the shared scanners.
const DATA_LIMITS: profile_backup::Limits = profile_backup::Limits {
    max_file: MAX_FILE,
    max_total: MAX_TOTAL,
    max_entries: MAX_ENTRIES,
    label: "data",
};
pub const MAX_TOTAL: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 200_000;
pub const MAX_MANIFEST: u64 = 8 * 1024 * 1024;
/// Directories under the DSH home that are copied. Nothing else is touched.
pub const ROOTS: [&str; 2] = ["sessions", "storages"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataFile {
    /// Path relative to the DSH home, prefixed with its root (`sessions/...`).
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub executable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataManifest {
    pub format_version: u32,
    pub scope: String,
    pub created_at: String,
    pub client_version: String,
    pub runtime_version: Option<String>,
    /// Roots that existed and were copied.
    pub roots: Vec<String>,
    pub directories: Vec<String>,
    pub files: Vec<DataFile>,
    /// Entries skipped because they are symlinks/special/non-UTF8. Recorded so a
    /// partial copy is never mistaken for a complete one.
    #[serde(default)]
    pub skipped: usize,
    /// False when the source changed while it was being copied, i.e. this is a
    /// best-effort point-in-time copy rather than a frozen one.
    #[serde(default = "default_true")]
    pub consistent: bool,
}

fn default_true() -> bool {
    true
}

struct Walked {
    items: BTreeMap<String, Item>,
    skipped: usize,
}

struct Item {
    directory: bool,
    stamp: Stamp,
}

/// Walks one root, skipping entries this format cannot copy instead of failing
/// the whole snapshot; `<root>/` is prefixed to every recorded path.
fn walk(
    root: &File,
    root_name: &str,
    depth: usize,
    out: &mut Walked,
    total: &mut u64,
) -> Result<()> {
    anyhow::ensure!(depth <= 32, "data snapshot nesting limit exceeded");
    for name in names_controlled(root, &mut || Ok(()))? {
        if profile_backup::EXCLUDED.contains(&name.as_str()) {
            continue;
        }
        let full = format!("{root_name}/{name}");
        let kind = match fs::symlink_metadata(profile_backup::proc_path(root).join(&name)) {
            Ok(metadata) => metadata.file_type(),
            Err(_) => {
                out.skipped += 1;
                continue;
            }
        };
        if !(kind.is_dir() || kind.is_file()) {
            out.skipped += 1;
            continue;
        }
        let file = match open_at(root, &name, kind.is_dir()) {
            Ok(file) => file,
            Err(_) => {
                out.skipped += 1;
                continue;
            }
        };
        let stamp = Stamp::of(&file)?;
        if !kind.is_dir() {
            anyhow::ensure!(stamp.len <= MAX_FILE, "data snapshot file is too large");
            *total = total
                .checked_add(stamp.len)
                .context("data snapshot size overflow")?;
            anyhow::ensure!(*total <= MAX_TOTAL, "data snapshot exceeds its size limit");
        }
        anyhow::ensure!(
            out.items.len() < MAX_ENTRIES,
            "data snapshot entry limit exceeded"
        );
        out.items.insert(
            full.clone(),
            Item {
                directory: kind.is_dir(),
                stamp,
            },
        );
        if kind.is_dir() {
            walk(&file, &full, depth + 1, out, total)?;
        }
    }
    Ok(())
}

fn walk_root(root: &File, root_name: &str, out: &mut Walked, total: &mut u64) -> Result<()> {
    walk(root, root_name, 0, out, total)
}

impl DataManifest {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

/// Parses and validates manifest bytes (used by the export verifier).
pub fn parse_manifest(bytes: &[u8]) -> Result<DataManifest> {
    let manifest: DataManifest =
        serde_json::from_slice(bytes).context("invalid data snapshot manifest")?;
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(manifest: &DataManifest) -> Result<()> {
    anyhow::ensure!(
        manifest.format_version == FORMAT_VERSION && manifest.scope == SCOPE,
        "unsupported data snapshot format/scope"
    );
    anyhow::ensure!(
        !manifest.roots.is_empty()
            && manifest
                .roots
                .iter()
                .all(|root| ROOTS.contains(&root.as_str())),
        "unexpected data snapshot roots"
    );
    anyhow::ensure!(
        manifest.files.len() <= MAX_ENTRIES && manifest.directories.len() <= MAX_ENTRIES,
        "data snapshot entry limit exceeded"
    );
    anyhow::ensure!(
        manifest.total_bytes() <= MAX_TOTAL,
        "data snapshot total size exceeded"
    );
    for file in &manifest.files {
        anyhow::ensure!(
            file.size <= MAX_FILE && file.sha256.len() == 64,
            "invalid data snapshot file entry"
        );
        anyhow::ensure!(
            file.path
                .split('/')
                .next()
                .is_some_and(|root| ROOTS.contains(&root)),
            "data snapshot path outside the copied roots"
        );
        // Reject traversal or odd components up front rather than relying on the
        // open helpers to fail later: a manifest must describe a copy of the two
        // roots, not an arbitrary filesystem path.
        for part in file.path.split('/') {
            anyhow::ensure!(
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part.len() <= 255
                    && !part.contains(['\\', '\u{0}'])
                    && !part.chars().any(char::is_control),
                "data snapshot path has an invalid component"
            );
        }
    }
    for directory in &manifest.directories {
        anyhow::ensure!(
            !directory.is_empty() && directory.len() <= 4096,
            "data snapshot directory entry is invalid"
        );
        for part in directory.split('/') {
            anyhow::ensure!(
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part.len() <= 255
                    && !part.contains(['\\', '\u{0}'])
                    && !part.chars().any(char::is_control),
                "data snapshot directory has an invalid component"
            );
        }
        anyhow::ensure!(
            directory
                .split('/')
                .next()
                .is_some_and(|root| manifest.roots.iter().any(|listed| listed == root)),
            "data snapshot directory outside the copied roots"
        );
    }
    Ok(())
}

/// Writes a new snapshot below `backups` and returns its path.
pub fn snapshot(home: &Path, backups: &Path, runtime_version: Option<String>) -> Result<PathBuf> {
    snapshot_with_checkpoint(home, backups, runtime_version, &mut || Ok(()))
}

/// Bytes/files copied so far during a whole-data snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotProgress {
    pub copied_files: usize,
    pub copied_bytes: u64,
    /// Total bytes discovered by the initial walk.
    pub total_bytes: u64,
}

impl SnapshotProgress {
    /// Counts-only text for a UI; never a file name or path.
    pub fn text(&self) -> String {
        if self.total_bytes == 0 {
            return format!("Copying: {} file(s)", self.copied_files);
        }
        let percent = (self.copied_bytes.min(self.total_bytes) * 100) / self.total_bytes;
        format!(
            "Copying {} of {} bytes ({}%), {} file(s)",
            self.copied_bytes, self.total_bytes, percent, self.copied_files
        )
    }
}

/// Snapshot with cooperative cancellation between files and stages.
///
/// A checkpoint error aborts and removes only this invocation's staging directory;
/// the source tree is never modified.
pub fn snapshot_with_checkpoint(
    home: &Path,
    backups: &Path,
    runtime_version: Option<String>,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<PathBuf> {
    snapshot_with_progress(home, backups, runtime_version, &mut |_| checkpoint())
}

/// Like [`snapshot_with_checkpoint`], but reports aggregate copy progress.
pub fn snapshot_with_progress(
    home: &Path,
    backups: &Path,
    runtime_version: Option<String>,
    checkpoint: &mut dyn FnMut(SnapshotProgress) -> Result<()>,
) -> Result<PathBuf> {
    let mut progress = SnapshotProgress::default();
    checkpoint(progress)?;
    let home_fd = open_dir(home).context("open DSH home for a data snapshot")?;
    let backups_fd = open_dir(backups).context("open backup root")?;
    let mut roots = Vec::new();
    let mut walked = Walked {
        items: BTreeMap::new(),
        skipped: 0,
    };
    let mut total = 0u64;
    for root in ROOTS {
        checkpoint(progress)?;
        let directory = match open_at(&home_fd, root, true) {
            Ok(directory) => directory,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue
            }
            Err(error) => return Err(error),
        };
        walk_root(&directory, root, &mut walked, &mut total)?;
        roots.push(root.to_owned());
    }
    let inventory = walked.items;
    let skipped = walked.skipped;
    anyhow::ensure!(!roots.is_empty(), "no session or storage data to snapshot");

    progress.total_bytes = total;
    checkpoint(progress)?;
    let stamp = profile_backup::id()?;
    let stage_name = format!(".ocean-data-stage-{stamp}");
    let stage = make_dir_at(&backups_fd, &stage_name)?;
    let result = (|| -> Result<PathBuf> {
        let data = make_dir_at(&stage, "data")?;
        let mut manifest = DataManifest {
            format_version: FORMAT_VERSION,
            scope: SCOPE.to_owned(),
            created_at: chrono::Utc::now().to_rfc3339(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            runtime_version,
            roots: roots.clone(),
            directories: Vec::new(),
            files: Vec::new(),
            skipped,
            consistent: true,
        };
        // The copied roots themselves are not part of the walked inventory, so
        // create them explicitly and record them as directories.
        for root in &roots {
            make_dir_at(&data, root)?;
            manifest.directories.push(root.clone());
        }
        for (path, item) in &inventory {
            checkpoint(progress)?;
            if item.directory {
                make_dir_at(&data, path)?;
                manifest.directories.push(path.clone());
            } else {
                let mut output = create_file_at(&data, path, false)?;
                let (size, sha256) = digest_copy_controlled(
                    file_at(&home_fd, path)?,
                    Some(&mut output),
                    &item.stamp,
                    MAX_FILE,
                    &mut |_| Ok(()),
                )?;
                manifest.files.push(DataFile {
                    path: path.clone(),
                    size,
                    sha256,
                    executable: item.stamp.mode & 0o100 != 0,
                });
                progress.copied_files += 1;
                progress.copied_bytes = progress.copied_bytes.saturating_add(size);
            }
        }
        checkpoint(progress)?;
        // Files are written through their own descriptors; sync them so a power
        // loss cannot publish a snapshot with missing contents.
        for file in &manifest.files {
            let handle = file_at(&data, &file.path)?;
            handle.sync_all()?;
        }
        for directory in &manifest.directories {
            // Directory entries are paths, so open the parent and the leaf.
            let (parent, name) = profile_backup::parent_at(&data, directory)?;
            open_at(&parent, &name, true)?.sync_all()?;
        }
        // Best-effort check: if the live tree changed while it was copied, say so
        // instead of presenting the snapshot as frozen.
        let mut after = Walked {
            items: BTreeMap::new(),
            skipped: 0,
        };
        let mut after_total = 0u64;
        for root in &roots {
            if let Ok(directory) = open_at(&home_fd, root, true) {
                let _ = walk_root(&directory, root, &mut after, &mut after_total);
            }
        }
        manifest.consistent = after.items.len() == inventory.len()
            && after.items.iter().all(|(path, item)| {
                inventory.get(path).is_some_and(|before| {
                    before.directory == item.directory
                        && before.stamp.len == item.stamp.len
                        && before.stamp.mtime == item.stamp.mtime
                        && before.stamp.mtime_ns == item.stamp.mtime_ns
                })
            });
        validate(&manifest)?;
        let json = serde_json::to_vec_pretty(&manifest)?;
        anyhow::ensure!(
            json.len() as u64 <= MAX_MANIFEST,
            "data snapshot manifest too large"
        );
        let mut metadata = create_file_at(&stage, "manifest.json", false)?;
        std::io::Write::write_all(&mut metadata, &json)?;
        metadata.sync_all()?;
        data.sync_all()?;
        stage.sync_all()?;
        let final_name = format!("data-{stamp}");
        profile_backup::rename_at(
            &backups_fd,
            &stage_name,
            &final_name,
            libc::RENAME_NOREPLACE,
        )?;
        Ok(backups.join(final_name))
    })();
    if result.is_err() {
        // Only this invocation's unique staging directory is removed.
        let _ = fs::remove_dir_all(profile_backup::proc_path(&backups_fd).join(&stage_name));
    }
    let _ = backups_fd.sync_all();
    result
}

/// Reads the manifest of a data snapshot (bounded, no hashing).
pub fn manifest(snapshot: &Path) -> Result<DataManifest> {
    let root = open_dir(snapshot).context("open data snapshot")?;
    anyhow::ensure!(
        names_controlled(&root, &mut || Ok(()))? == ["data", "manifest.json"],
        "unexpected data snapshot root entry"
    );
    let metadata = open_at(&root, "manifest.json", false)?;
    anyhow::ensure!(
        metadata.metadata()?.len() <= MAX_MANIFEST,
        "data snapshot manifest too large"
    );
    let mut raw = Vec::new();
    {
        use std::io::Read;
        (&metadata)
            .take(MAX_MANIFEST + 1)
            .read_to_end(&mut raw)
            .context("read data snapshot manifest")?;
    }
    anyhow::ensure!(
        raw.len() as u64 <= MAX_MANIFEST,
        "data snapshot manifest grew while reading"
    );
    let manifest: DataManifest =
        serde_json::from_slice(&raw).context("invalid data snapshot manifest")?;
    validate(&manifest)?;
    Ok(manifest)
}

/// Full read-only verification: structure, sizes, checksums and final identity.
pub fn verify(snapshot: &Path) -> Result<DataManifest> {
    let manifest = manifest(snapshot)?;
    let root = open_dir(snapshot)?;
    let data = open_at(&root, "data", true)?;
    let inventory = scan_controlled(&data, true, DATA_LIMITS, &mut || Ok(()))?;
    anyhow::ensure!(
        inventory.len() == manifest.files.len() + manifest.directories.len(),
        "unexpected or missing data snapshot entries"
    );
    for directory in &manifest.directories {
        anyhow::ensure!(
            inventory.get(directory).is_some_and(|item| item.directory),
            "missing data snapshot directory"
        );
    }
    for entry in &manifest.files {
        let item = inventory
            .get(&entry.path)
            .context("missing data snapshot file")?;
        anyhow::ensure!(!item.directory, "data snapshot file is a directory");
        let (size, hash) = digest_copy_controlled(
            file_at(&data, &entry.path)?,
            None,
            &item.stamp,
            MAX_FILE,
            &mut |_| Ok(()),
        )?;
        anyhow::ensure!(
            size == entry.size && hash == entry.sha256,
            "data snapshot integrity check failed"
        );
    }
    // Re-scan once so a changed inventory is detected before claiming success.
    anyhow::ensure!(
        scan_controlled(&data, true, DATA_LIMITS, &mut || Ok(()))? == inventory,
        "data snapshot changed during verification"
    );
    Ok(manifest)
}

/// Result of a whole-data restore.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReceipt {
    /// Roots that were actually replaced.
    pub restored: Vec<String>,
    /// Roots left untouched because the snapshot did not contain them.
    pub untouched: Vec<String>,
    /// Where each replaced root's previous contents were retained.
    pub retained: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

fn root_identity(home_fd: &File, root: &str) -> Result<Option<std::fs::Metadata>> {
    match fs::symlink_metadata(profile_backup::proc_path(home_fd).join(root)) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_dir(),
                "refusing to restore over a non-directory data root"
            );
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("inspect the data root target"),
    }
}

fn describe_roots(roots: &[String]) -> String {
    if roots.is_empty() {
        "none".to_owned()
    } else {
        roots.join(", ")
    }
}

fn describe_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "none".to_owned()
    } else {
        paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Wraps a failure so the operator learns what was already replaced.
fn partial_restore_error(
    restored: &[String],
    retained: &[PathBuf],
    error: anyhow::Error,
) -> anyhow::Error {
    if restored.is_empty() && retained.is_empty() {
        return error;
    }
    anyhow::anyhow!(
        "restore stopped after replacing: {}; retained previous copies: {}; cause: {error:#}",
        describe_roots(restored),
        describe_paths(retained),
    )
}

/// Replaces the data roots present in `snapshot` with its contents.
///
/// The caller must have stopped every DSH writer; `require_offline` is invoked
/// before staging and again immediately before each atomic exchange. Previous
/// roots are retained next to the live ones instead of being deleted.
pub fn restore<F>(home: &Path, snapshot: &Path, mut require_offline: F) -> Result<RestoreReceipt>
where
    F: FnMut() -> Result<()>,
{
    let manifest = verify(snapshot)?;
    let home_fd = open_dir(home).context("open DSH home for a data restore")?;
    let snapshot_root = open_dir(snapshot)?;
    let data = open_at(&snapshot_root, "data", true)?;
    let stamp = profile_backup::id()?;
    require_offline()?;

    let mut restored = Vec::new();
    let mut untouched = Vec::new();
    let mut retained_paths = Vec::new();
    let mut warnings = vec![
        "Previous data roots were retained next to the live ones; nothing was deleted."
            .to_owned(),
        "Session data may be schema-incompatible with the selected DSH runtime; this restore does not migrate it."
            .to_owned(),
    ];
    if manifest.skipped > 0 {
        warnings.push(format!(
            "The snapshot skipped {} entr(ies) that were symlinks or special files; those are absent after restore.",
            manifest.skipped
        ));
    }
    for root in ROOTS {
        if !manifest.roots.iter().any(|candidate| candidate == root) {
            untouched.push(root.to_owned());
            continue;
        }
        let source = open_at(&data, root, true)
            .map_err(|error| partial_restore_error(&restored, &retained_paths, error))?;
        let stage_name = format!(".ocean-data-restore-{stamp}-{root}");
        let stage = make_dir_at(&home_fd, &stage_name)
            .map_err(|error| partial_restore_error(&restored, &retained_paths, error))?;
        let staged = (|| -> Result<()> {
            let inventory = scan_controlled(&source, true, DATA_LIMITS, &mut || Ok(()))?;
            for (path, item) in &inventory {
                let target = format!("{stage_name}/{path}");
                if item.directory {
                    make_dir_at(&home_fd, &target)?;
                } else {
                    let mut output =
                        create_file_at(&home_fd, &target, item.stamp.mode & 0o100 != 0)?;
                    let record = manifest
                        .files
                        .iter()
                        .find(|file| file.path == format!("{root}/{path}"))
                        .context("staged file is missing from the manifest")?;
                    let (size, sha256) = digest_copy_controlled(
                        file_at(&source, path)?,
                        Some(&mut output),
                        &item.stamp,
                        MAX_FILE,
                        &mut |_| Ok(()),
                    )?;
                    anyhow::ensure!(
                        size == record.size && sha256 == record.sha256,
                        "snapshot data changed before restore"
                    );
                }
            }
            Ok(())
        })();
        if let Err(error) = staged {
            let _ = fs::remove_dir_all(profile_backup::proc_path(&home_fd).join(&stage_name));
            return Err(partial_restore_error(&restored, &retained_paths, error));
        }
        stage.sync_all()?;
        require_offline()
            .map_err(|error| partial_restore_error(&restored, &retained_paths, error))?;
        let previous = root_identity(&home_fd, root)
            .map_err(|error| partial_restore_error(&restored, &retained_paths, error))?;
        match previous {
            Some(_existing) => {
                // Exchange keeps the old tree in the staging name; never delete it.
                profile_backup::rename_at(&home_fd, &stage_name, root, libc::RENAME_EXCHANGE)
                    .map_err(|error| partial_restore_error(&restored, &retained_paths, error))?;
                let retain = format!("{root}.ocean-retained-{stamp}");
                let fallback = format!("{root}.ocean-retained-fallback-{stamp}");
                // A failed retain rename must not leave the only copy of the previous
                // root under a staging prefix that collection removes later.
                let retained_name = match profile_backup::rename_at(
                    &home_fd,
                    &stage_name,
                    &retain,
                    libc::RENAME_NOREPLACE,
                ) {
                    Ok(()) => retain.clone(),
                    Err(first) => match profile_backup::rename_at(
                        &home_fd,
                        &stage_name,
                        &fallback,
                        libc::RENAME_NOREPLACE,
                    ) {
                        Ok(()) => fallback.clone(),
                        Err(_) => {
                            let path = home.join(&stage_name);
                            return Err(anyhow::anyhow!(
                                "the previous {root} could not be renamed to {retain} ({first}) and the fallback {fallback} also failed; it is still at {} — move it out before collecting staging directories. Already restored: {}. Retained so far: {}",
                                path.display(),
                                describe_roots(&restored),
                                describe_paths(&retained_paths),
                            ));
                        }
                    },
                };
                let retain = retained_name;
                let retained =
                    fs::symlink_metadata(profile_backup::proc_path(&home_fd).join(&retain))
                        .context("inspect the retained data root")?;
                anyhow::ensure!(
                    retained.is_dir(),
                    "previous replacement unexpectedly produced a non-directory root"
                );
                retained_paths.push(home.join(&retain));
            }
            None => {
                profile_backup::rename_at(&home_fd, &stage_name, root, libc::RENAME_NOREPLACE)
                    .map_err(|error| partial_restore_error(&restored, &retained_paths, error))?;
            }
        }
        restored.push(root.to_owned());
    }
    let _ = home_fd.sync_all();
    anyhow::ensure!(
        !restored.is_empty(),
        "the snapshot contains no data roots to restore"
    );
    Ok(RestoreReceipt {
        restored,
        untouched,
        retained: retained_paths,
        warnings,
    })
}

/// One discoverable whole-data snapshot (metadata only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSummary {
    pub name: String,
    /// Validated UTC timestamp, or `None` when the manifest value is unusable.
    pub created_at: Option<String>,
    pub files: usize,
    pub bytes: u64,
}

/// Creation order encoded in a `data-<pid>-<nanos>` name.
pub(crate) fn sequence_of(name: &str) -> Option<(u64, u128)> {
    let rest = name.strip_prefix("data-")?;
    let (pid, nanos) = rest.split_once('-')?;
    let pid = pid.parse::<u64>().ok()?;
    let nanos = nanos.parse::<u128>().ok()?;
    Some((pid, nanos))
}

/// Lists `data-*` snapshots by manifest scope, without hashing their contents.
pub fn list(backups: &Path) -> Result<Vec<DataSummary>> {
    let root = match open_dir(backups) {
        Ok(root) => root,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(Vec::new())
        }
        Err(error) => return Err(error),
    };
    let mut result = Vec::new();
    for name in names_controlled(&root, &mut || Ok(()))? {
        if !name.starts_with("data-") {
            continue;
        }
        let Ok(path) = fs::canonicalize(profile_backup::proc_path(&root).join(&name)) else {
            continue;
        };
        match manifest(&path) {
            Ok(manifest) => result.push(DataSummary {
                name,
                created_at: chrono::DateTime::parse_from_rfc3339(&manifest.created_at)
                    .ok()
                    .map(|time| time.with_timezone(&chrono::Utc).to_rfc3339()),
                files: manifest.files.len(),
                bytes: manifest.total_bytes(),
            }),
            Err(_) => continue,
        }
    }
    result.sort_by(|left, right| {
        // Newest last; the pid is only a tie-breaker for the same nanosecond.
        sequence_of(&left.name).cmp(&sequence_of(&right.name))
    });
    Ok(result)
}

/// Removes only this engine's own `data-*` snapshots beyond the newest `keep`.
pub fn prune(backups: &Path, keep: usize) -> Result<Vec<String>> {
    anyhow::ensure!((1..=1000).contains(&keep), "keep between 1 and 1000");
    let root = match open_dir(backups) {
        Ok(root) => root,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(Vec::new())
        }
        Err(error) => return Err(error),
    };
    let mut names: Vec<String> = names_controlled(&root, &mut || Ok(()))?
        .into_iter()
        .filter(|name| name.starts_with("data-"))
        .collect();
    names.sort_by_key(|name| sequence_of(name));
    let remove = names.len().saturating_sub(keep);
    let mut removed = Vec::new();
    for name in names.into_iter().take(remove) {
        let path = profile_backup::proc_path(&root).join(&name);
        // Verify before deleting: a corrupted snapshot is left for inspection.
        if manifest(&path).is_err() {
            continue;
        }
        fs::remove_dir_all(&path).context("remove old data snapshot")?;
        removed.push(name);
    }
    let _ = root.sync_all();
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with(files: Vec<DataFile>, roots: Vec<&str>) -> DataManifest {
        DataManifest {
            format_version: FORMAT_VERSION,
            scope: SCOPE.to_owned(),
            created_at: chrono::Utc::now().to_rfc3339(),
            client_version: "0.0.0".into(),
            runtime_version: None,
            roots: roots.into_iter().map(str::to_owned).collect(),
            directories: Vec::new(),
            files,
            skipped: 0,
            consistent: true,
        }
    }

    fn file(path: &str, size: u64) -> DataFile {
        DataFile {
            path: path.to_owned(),
            size,
            sha256: "0".repeat(64),
            executable: false,
        }
    }

    #[test]
    fn validation_accepts_only_the_two_known_roots() {
        assert!(validate(&manifest_with(
            vec![file("sessions/a", 1), file("storages/b", 2)],
            vec!["sessions", "storages"]
        ))
        .is_ok());
        // A path outside the copied roots is refused even if the file entry
        // itself looks well-formed.
        assert!(validate(&manifest_with(vec![file("cache/a", 1)], vec!["sessions"])).is_err());
        assert!(validate(&manifest_with(
            vec![file("sessions/../escape", 1)],
            vec!["sessions"]
        ))
        .is_err());
        assert!(validate(&manifest_with(
            vec![file("/etc/passwd", 1)],
            vec!["sessions"]
        ))
        .is_err());
        // Unknown or empty roots are refused.
        assert!(validate(&manifest_with(vec![], vec!["sessions", "secrets"])).is_err());
        assert!(validate(&manifest_with(vec![], vec![])).is_err());
        // Wrong scope/version is refused.
        let mut wrong = manifest_with(vec![], vec!["sessions"]);
        wrong.scope = "profile-config".into();
        assert!(validate(&wrong).is_err());
        wrong.scope = SCOPE.into();
        wrong.format_version = 2;
        assert!(validate(&wrong).is_err());
    }

    #[test]
    fn validation_enforces_entry_and_size_limits() {
        assert!(validate(&manifest_with(
            vec![file("sessions/big", MAX_FILE + 1)],
            vec!["sessions"]
        ))
        .is_err());
        let mut oversized = manifest_with(vec![], vec!["sessions"]);
        // Two files whose sum exceeds the total bound, each within the per-file one.
        let each = MAX_TOTAL / 2 + 1;
        oversized.files = vec![file("sessions/a", each), file("sessions/b", each)];
        assert!(oversized.total_bytes() > MAX_TOTAL);
        assert!(validate(&oversized).is_err());
        let mut bad_hash = manifest_with(vec![file("sessions/a", 1)], vec!["sessions"]);
        bad_hash.files[0].sha256 = "xyz".into();
        assert!(validate(&bad_hash).is_err());
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[test]
    fn progress_text_is_count_only_and_bounded() {
        let mut progress = SnapshotProgress::default();
        assert_eq!(progress.text(), "Copying: 0 file(s)");
        progress.total_bytes = 1_000;
        progress.copied_bytes = 250;
        progress.copied_files = 2;
        assert_eq!(
            progress.text(),
            "Copying 250 of 1000 bytes (25%), 2 file(s)"
        );
        // Counts beyond the discovered total never render above 100%.
        progress.copied_bytes = 5_000;
        assert!(progress.text().contains("(100%)"), "{}", progress.text());
        assert!(!progress.text().contains('/'));
    }
}

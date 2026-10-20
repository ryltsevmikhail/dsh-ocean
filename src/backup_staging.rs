//! Collection of abandoned backup staging directories.
//!
//! Snapshot and restore staging directories are named
//! `.ocean-stage-*`, `.ocean-data-stage-*` or `.ocean-data-restore-*` and live
//! directly inside the backup root or the DSH home. A normal failure removes them;
//! `SIGKILL`, power loss or a full disk can leave them behind.
//!
//! Collection is explicit and narrow: only those exact prefixes are considered,
//! symlinks are never followed or deleted, and only directories older than a
//! caller-chosen age are removed.
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use serde::Serialize;

/// Prefixes this tool may collect.
///
/// `.ocean-data-restore-*` is deliberately absent: a restore that fails between the
/// exchange and the retain rename leaves the only copy of the previous root there,
/// so that prefix is reported for a human instead of being deleted.
pub const PREFIXES: [&str; 2] = [".ocean-stage-", ".ocean-data-stage-"];
/// Restore leftovers that are listed but never collected.
pub const REPORT_ONLY_PREFIXES: [&str; 1] = [".ocean-data-restore-"];

/// Every generated prefix, collected or report-only.
pub fn all_prefixes() -> impl Iterator<Item = &'static str> {
    PREFIXES.into_iter().chain(REPORT_ONLY_PREFIXES)
}

/// True when `collect` may delete an entry with this name.
pub fn collectable(name: &str) -> bool {
    PREFIXES.iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty())
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stale {
    pub name: String,
    pub bytes: u64,
    pub age_seconds: u64,
    /// False for report-only prefixes that may hold the only copy of user data.
    pub collectable: bool,
}

fn matches_prefix(name: &str) -> bool {
    all_prefixes().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty())
    })
}

fn tree_bytes(root: &Path, bytes: &mut u64, files: &mut usize) -> bool {
    let mut truncated = false;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut budget = 100_000usize;
    while let Some((directory, depth)) = stack.pop() {
        if depth > 32 {
            truncated = true;
            continue;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            truncated = true;
            continue;
        };
        for entry in entries.flatten() {
            if budget == 0 {
                return true;
            }
            budget -= 1;
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                truncated = true;
                continue;
            };
            if metadata.file_type().is_dir() {
                stack.push((path, depth + 1));
            } else if metadata.file_type().is_file() {
                *files += 1;
                *bytes = bytes.saturating_add(metadata.len());
            }
        }
    }
    truncated
}

/// Lists staging entries older than `minimum_age`. Read-only.
pub fn stale(root: &Path, minimum_age: Duration) -> Result<Vec<Stale>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("read the backup root"),
    };
    let now = SystemTime::now();
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !matches_prefix(&name) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        // Symlinks are listed by nobody: they are not ours to delete.
        if !metadata.is_dir() {
            continue;
        }
        let age = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .unwrap_or_default();
        if age < minimum_age {
            continue;
        }
        let mut bytes = 0u64;
        let mut files = 0usize;
        let _ = tree_bytes(&path, &mut bytes, &mut files);
        result.push(Stale {
            collectable: collectable(&name),
            name,
            bytes,
            age_seconds: age.as_secs(),
        });
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(result)
}

/// Removes the staging entries that `stale` reports, one by one.
///
/// Returns the names removed and those that could not be removed safely.
pub fn collect(root: &Path, minimum_age: Duration) -> Result<(Vec<String>, Vec<String>)> {
    let candidates = stale(root, minimum_age)?;
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    if candidates.is_empty() {
        return Ok((removed, kept));
    }
    // Removal is fd-relative and never follows a symlink, so an entry swapped
    // between listing and deletion cannot redirect the walk outside the root.
    let directory = match crate::profile_backup::open_dir(root) {
        Ok(directory) => directory,
        Err(_) => {
            return Ok((
                removed,
                candidates.into_iter().map(|item| item.name).collect(),
            ));
        }
    };
    for candidate in candidates {
        if !candidate.collectable {
            kept.push(candidate.name);
            continue;
        }
        match crate::profile_backup::remove_tree(&directory, &candidate.name, &mut || Ok(())) {
            Ok(()) => removed.push(candidate.name),
            Err(_) => kept.push(candidate.name),
        }
    }
    Ok((removed, kept))
}

/// Backup roots that may hold staging directories.
pub fn candidate_roots(home: &Path, backups: &Path) -> Vec<PathBuf> {
    vec![backups.to_path_buf(), home.to_path_buf()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-staging-{}-{}-{}",
            std::process::id(),
            name,
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn age(path: &Path, seconds: u64) {
        let when = SystemTime::now() - Duration::from_secs(seconds);
        let file = fs::OpenOptions::new().read(true).open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(when).set_accessed(when))
            .unwrap();
    }

    #[test]
    fn only_our_prefixes_and_only_old_enough_entries_are_listed() {
        let root = temp("list");
        for name in [
            ".ocean-stage-aaa",
            ".ocean-data-stage-bbb",
            ".ocean-data-restore-ccc",
        ] {
            let path = root.join(name);
            fs::create_dir_all(path.join("nested")).unwrap();
            fs::write(path.join("nested/file"), b"12345").unwrap();
            age(&path, 3600);
        }
        fs::create_dir_all(root.join(".ocean-stage-fresh")).unwrap();
        fs::create_dir_all(root.join("ordinary")).unwrap();
        fs::write(root.join(".ocean-stage-file"), b"not a dir").unwrap();
        let listed = stale(&root, Duration::from_secs(60)).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                ".ocean-data-restore-ccc",
                ".ocean-data-stage-bbb",
                ".ocean-stage-aaa"
            ]
        );
        assert!(listed
            .iter()
            .all(|item| item.bytes == 5 && item.age_seconds >= 60));
        // Nothing was deleted by listing.
        assert!(root.join(".ocean-stage-aaa/nested/file").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn collection_removes_old_staging_and_keeps_fresh_entries() {
        let root = temp("collect");
        let old = root.join(".ocean-stage-old");
        fs::create_dir_all(old.join("data")).unwrap();
        fs::write(old.join("data/file"), b"x").unwrap();
        age(&old, 7200);
        let fresh = root.join(".ocean-stage-fresh");
        fs::create_dir_all(&fresh).unwrap();
        let (removed, kept) = collect(&root, Duration::from_secs(60)).unwrap();
        assert_eq!(removed, vec![".ocean-stage-old".to_owned()]);
        assert!(kept.is_empty());
        assert!(!old.exists());
        assert!(fresh.is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinked_staging_entries_are_never_listed_or_deleted() {
        let root = temp("symlink");
        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep"), b"data").unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".ocean-stage-link")).unwrap();
        let listed = stale(&root, Duration::from_secs(0)).unwrap();
        assert!(listed.is_empty());
        let (removed, _) = collect(&root, Duration::from_secs(0)).unwrap();
        assert!(removed.is_empty());
        assert!(outside.join("keep").is_file());
        fs::remove_dir_all(root).unwrap();
    }
}

//! Read-only inventory of DSH session/global storage.
//!
//! This reports **counts and sizes only**: no file names, no contents, no keys.
//! It exists so a person can see how much space sessions and shared stores use
//! before deciding about backup or cleanup — nothing here deletes, moves, reads
//! file contents or follows a symlink.
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde::Serialize;

/// Bounds keep a hostile or enormous tree from turning a status command into a
/// filesystem scan. Hitting either bound is reported instead of silently cut.
pub const MAX_ENTRIES: usize = 20_000;
pub const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Regular files directly below the sessions tree.
    pub session_files: usize,
    pub session_bytes: u64,
    /// Regular files below the shared storages tree.
    pub storage_files: usize,
    pub storage_bytes: u64,
    /// Entries skipped because they are symlinks, special files or invisible.
    pub skipped: usize,
    /// True when a bound stopped the walk; numbers are a lower bound then.
    pub truncated: bool,
    /// Set when a root exists but could not be inspected.
    pub error: Option<String>,
}

fn walk(root: &Path, summary: &mut Summary, session: bool, depth: usize, budget: &mut usize) {
    if summary.truncated {
        return;
    }
    if depth > MAX_DEPTH {
        summary.truncated = true;
        return;
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            summary.error = Some("a storage directory could not be inspected".to_owned());
            return;
        }
    };
    for entry in entries {
        if *budget == 0 {
            summary.truncated = true;
            return;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                summary.skipped += 1;
                continue;
            }
        };
        *budget -= 1;
        let path = entry.path();
        // `symlink_metadata` never follows; a symlinked file or directory is
        // skipped rather than counted or descended into.
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                summary.skipped += 1;
                continue;
            }
        };
        if metadata.file_type().is_dir() {
            walk(&path, summary, session, depth + 1, budget);
            continue;
        }
        if !metadata.file_type().is_file() || metadata.is_file() && metadata.nlink() == 0 {
            summary.skipped += 1;
            continue;
        }
        if session {
            summary.session_files += 1;
            summary.session_bytes = summary.session_bytes.saturating_add(metadata.len());
        } else {
            summary.storage_files += 1;
            summary.storage_bytes = summary.storage_bytes.saturating_add(metadata.len());
        }
    }
}

/// Counts session and shared-storage files below `dsh_home`.
pub fn inventory(dsh_home: &Path) -> Summary {
    let mut summary = Summary::default();
    let mut budget = MAX_ENTRIES;
    walk(
        &dsh_home.join("sessions"),
        &mut summary,
        true,
        0,
        &mut budget,
    );
    walk(
        &dsh_home.join("storages"),
        &mut summary,
        false,
        0,
        &mut budget,
    );
    summary
}

impl Summary {
    /// Fixed, count-only text for `--doctor`.
    pub fn text(&self) -> String {
        if let Some(error) = &self.error {
            return format!("unavailable ({error})");
        }
        format!(
            "sessions={}/{}B storages={}/{}B skipped={}{}",
            self.session_files,
            self.session_bytes,
            self.storage_files,
            self.storage_bytes,
            self.skipped,
            if self.truncated {
                " (partial: bound reached)"
            } else {
                ""
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-session-inventory-{}-{}-{}",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn counts_only_regular_files_and_never_follows_links() {
        let root = temp("counts");
        fs::create_dir_all(root.join("sessions/a/b")).unwrap();
        fs::write(root.join("sessions/a/one.json"), b"12345").unwrap();
        fs::write(root.join("sessions/a/b/two.json"), b"123").unwrap();
        fs::create_dir_all(root.join("storages/store")).unwrap();
        fs::write(root.join("storages/store/data"), b"ab").unwrap();
        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret"), b"not counted").unwrap();
        symlink(&outside, root.join("sessions/link-dir")).unwrap();
        symlink(outside.join("secret"), root.join("sessions/link-file")).unwrap();
        let summary = inventory(&root);
        assert_eq!(summary.session_files, 2);
        assert_eq!(summary.session_bytes, 8);
        assert_eq!(summary.storage_files, 1);
        assert_eq!(summary.storage_bytes, 2);
        assert_eq!(summary.skipped, 2);
        assert!(!summary.truncated);
        assert!(summary.error.is_none());
        let text = summary.text();
        assert!(text.contains("sessions=2/8B"), "{text}");
        assert!(!text.contains("one.json") && !text.contains("secret"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_roots_report_zero_without_an_error() {
        let root = temp("missing");
        let summary = inventory(&root.join("absent"));
        assert_eq!(summary, Summary::default());
        assert!(summary.text().starts_with("sessions=0/0B"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bounds_are_reported_instead_of_silently_cut() {
        let root = temp("bounds");
        let deep = root.join("sessions");
        let mut path = deep.clone();
        for _ in 0..(MAX_DEPTH + 2) {
            path = path.join("d");
        }
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("file"), b"x").unwrap();
        let summary = inventory(&root);
        assert!(summary.truncated);
        assert!(summary.text().contains("partial"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_where_a_root_belongs_is_reported_not_ignored() {
        let root = temp("file-root");
        fs::write(root.join("sessions"), b"not a directory").unwrap();
        let summary = inventory(&root);
        assert!(summary.error.is_some());
        assert!(summary.text().starts_with("unavailable"));
        fs::remove_dir_all(root).unwrap();
    }
}

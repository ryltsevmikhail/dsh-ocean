//! Consent-based downloads: private staging beside the chosen destination,
//! atomic NOREPLACE publication, and bounded metadata without source URL secrets.
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use url::Url;

pub const MAX_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_RECORDS: usize = 100;
const MAX_HISTORY: u64 = 2 * 1024 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
pub(crate) fn unique_id() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

pub fn source_origin(url: &Url, server: &Url) -> Result<String> {
    if url.scheme() == "blob" {
        let inner = Url::parse(url.path()).context("invalid blob origin")?;
        anyhow::ensure!(
            inner.username().is_empty() && inner.password().is_none(),
            "credential-bearing blob URL refused"
        );
    }
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "credential-bearing download URL refused"
    );
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https" | "blob")
            && matches!(server.scheme(), "http" | "https")
            && url.origin() == server.origin(),
        "only the current server origin and its blobs may download natively"
    );
    Ok(server.origin().ascii_serialization())
}
pub fn suggested_name(raw: &str) -> String {
    let mut result = String::new();
    for c in raw.chars() {
        let c = if c == '/' || c == '\\' || c.is_control() {
            '_'
        } else {
            c
        };
        if result.len() + c.len_utf8() > 180 {
            break;
        }
        result.push(c);
    }
    if result.trim().is_empty() || result == "." || result == ".." {
        "download".into()
    } else {
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Receiving,
    Saved,
    Cancelled,
    Blocked,
    Failed,
    Interrupted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub created_at: String,
    pub source_origin: Option<String>,
    pub name: String,
    pub status: Status,
    pub destination: Option<PathBuf>,
    pub recovery: Option<PathBuf>,
    pub message: Option<String>,
    // Live-session hints only: no disk-format extension or per-chunk writes.
    #[serde(skip)]
    pub received_bytes: u64,
    #[serde(skip)]
    pub reported_total: Option<u64>,
}
impl Entry {
    pub fn new(origin: Option<String>, name: String, status: Status) -> Self {
        Self {
            id: unique_id(),
            created_at: chrono::Utc::now().to_rfc3339(),
            source_origin: origin
                .and_then(|value| Url::parse(&value).ok())
                .filter(|url| matches!(url.scheme(), "http" | "https" | "blob"))
                .map(|url| url.origin().ascii_serialization())
                .filter(|value| value != "null"),
            name,
            status,
            destination: None,
            recovery: None,
            message: None,
            received_bytes: 0,
            reported_total: None,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Disk {
    version: u32,
    entries: Vec<Entry>,
}
#[derive(Default)]
pub struct History {
    path: Option<PathBuf>,
    entries: Vec<Entry>,
}
impl History {
    pub fn open(path: &Path) -> Result<Self> {
        let mut entries = Vec::new();
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => {
                anyhow::ensure!(
                    file.metadata()?.is_file()
                        && file.metadata()?.nlink() == 1
                        && file.metadata()?.len() <= MAX_HISTORY,
                    "invalid download history file"
                );
                let mut raw = Vec::new();
                file.take(MAX_HISTORY + 1).read_to_end(&mut raw)?;
                anyhow::ensure!(
                    raw.len() as u64 <= MAX_HISTORY,
                    "download history grew beyond limit"
                );
                let disk: Disk =
                    serde_json::from_slice(&raw).context("invalid download history")?;
                anyhow::ensure!(
                    disk.version == 1 && disk.entries.len() <= MAX_RECORDS,
                    "unsupported download history"
                );
                for mut entry in disk.entries {
                    anyhow::ensure!(
                        entry.id.len() <= 100 && entry.name.len() <= 4096,
                        "invalid download metadata"
                    );
                    if let Some(origin) = &entry.source_origin {
                        let url = Url::parse(origin)?;
                        anyhow::ensure!(
                            matches!(url.scheme(), "http" | "https")
                                && url.username().is_empty()
                                && url.password().is_none()
                                && url.origin().ascii_serialization() == *origin,
                            "history must not contain URL paths or credentials"
                        );
                    }
                    for path in [&entry.destination, &entry.recovery].into_iter().flatten() {
                        validate_path(path)?;
                    }
                    if entry.status == Status::Receiving {
                        entry.status = Status::Interrupted;
                        entry.message = Some(
                            "application closed before download completion; staging may remain"
                                .into(),
                        );
                    }
                    entries.push(entry);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).context("open download history without following links")
            }
        }
        Ok(Self {
            path: Some(path.to_owned()),
            entries,
        })
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn receiving(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == Status::Receiving)
            .count()
    }
    /// Writes the current entries without holding the history mutex.
    ///
    /// `persist` reaches the state directory with `sync_all`, which can block for a
    /// long time on a slow, remote or full filesystem; callers on the GTK main
    /// thread must take a snapshot under the lock and persist outside it.
    pub fn persist_snapshot(&self, entries: Vec<Entry>) -> Result<()> {
        match &self.path {
            Some(path) => Self::persist_entries_at(path, entries),
            None => Ok(()),
        }
    }

    /// Writes `entries` to `path` atomically; safe to call with no lock held.
    pub fn persist_entries_at(path: &Path, entries: Vec<Entry>) -> Result<()> {
        let path = path.to_path_buf();
        let path = &path;
        let parent = path.parent().context("history parent")?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let temp = parent.join(format!(".downloads-{}.tmp", unique_id()));
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temp)?;
            let raw = serde_json::to_vec(&Disk {
                version: 1,
                entries,
            })?;
            anyhow::ensure!(
                raw.len() as u64 <= MAX_HISTORY,
                "download history size limit exceeded"
            );
            file.write_all(&raw)?;
            file.sync_all()?;
            fs::rename(&temp, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    /// Entries for a caller that persists outside the lock.
    pub fn entries_snapshot(&self) -> Vec<Entry> {
        self.entries.clone()
    }

    /// Whether this history is backed by a file at all.
    pub fn has_path(&self) -> bool {
        self.path.is_some()
    }

    /// The file this history is stored in, for a lock-free writer.
    pub fn path_snapshot(&self) -> Option<PathBuf> {
        self.path.clone()
    }

    pub fn put(&mut self, entry: Entry) {
        if let Some(old) = self.entries.iter_mut().find(|old| old.id == entry.id) {
            *old = entry;
        } else {
            self.entries.push(entry);
        }
        while self.entries.len() > MAX_RECORDS {
            if let Some(index) = self
                .entries
                .iter()
                .position(|e| e.status != Status::Receiving)
            {
                self.entries.remove(index);
            } else {
                break;
            }
        }
    }
    /// In-memory only, monotonic and bounded. Never create/revive a row or
    /// downgrade a terminal status when a late network callback arrives.
    pub fn update_progress(&mut self, id: &str, received: u64) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && entry.status == Status::Receiving)
        else {
            return false;
        };
        entry.received_bytes = entry.received_bytes.max(received).min(MAX_BYTES + 1);
        true
    }
    pub fn clear_finished(&mut self) {
        self.entries.retain(|e| e.status == Status::Receiving);
    }
    pub fn persist(&self) -> Result<()> {
        self.persist_snapshot(self.entries.clone())
    }
}
fn validate_path(path: &Path) -> Result<()> {
    let raw = path.to_str().context("download path must be UTF-8")?;
    anyhow::ensure!(
        path.is_absolute()
            && raw.len() <= 4096
            && !raw.chars().any(char::is_control)
            && path.components().all(|c| matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )),
        "invalid absolute download path"
    );
    Ok(())
}
fn directory(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?)
}

// Armed before opening the new directory, so an EMFILE/open/metadata error
// does not bypass Transfer's later cleanup. Only remove the exact empty inode;
// never recursively remove contents introduced before initialization completes.
struct PendingStage {
    path: PathBuf,
    dev: u64,
    ino: u64,
    armed: bool,
}
impl Drop for PendingStage {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(metadata) = fs::symlink_metadata(&self.path) {
            if metadata.is_dir() && metadata.dev() == self.dev && metadata.ino() == self.ino {
                assert!(
                    self.path.is_absolute()
                        && self
                            .path
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".dsh-ocean-download-")
                );
                let _ = fs::remove_dir(&self.path);
            }
        }
    }
}

pub struct Transfer {
    pub destination: PathBuf,
    pub payload: PathBuf,
    parent: File,
    stage: File,
    stage_path: PathBuf,
    stage_ino: u64,
    stage_dev: u64,
    remove: bool,
}
impl Transfer {
    pub fn prepare(destination: &Path) -> Result<Self> {
        Self::prepare_with_stage_directory(destination, directory)
    }
    fn prepare_with_stage_directory(
        destination: &Path,
        open_stage: impl FnOnce(&Path) -> Result<File>,
    ) -> Result<Self> {
        validate_path(destination)?;
        let name = destination
            .file_name()
            .context("download filename required")?;
        let parent_path = fs::canonicalize(
            destination
                .parent()
                .context("download directory required")?,
        )?;
        let destination = parent_path.join(name);
        validate_path(&destination)?;
        anyhow::ensure!(
            !parent_path.components().any(|part| part
                .as_os_str()
                .to_string_lossy()
                .starts_with(".dsh-ocean-download-")),
            "private download staging is not a save destination"
        );
        anyhow::ensure!(matches!(fs::symlink_metadata(&destination), Err(ref e) if e.kind() == std::io::ErrorKind::NotFound), "existing files, links and special targets are never overwritten; choose a new filename");
        let parent = directory(&parent_path)?;
        let stage_path = parent_path.join(format!(".dsh-ocean-download-{}", unique_id()));
        fs::DirBuilder::new().mode(0o700).create(&stage_path)?;
        // lstat needs no extra descriptor, including when open failed with
        // descriptor exhaustion. If identity cannot be read, fail without an
        // unverified delete; an empty directory may remain in that rare case.
        let created = fs::symlink_metadata(&stage_path)?;
        anyhow::ensure!(created.is_dir(), "new staging directory is not a directory");
        let mut pending = PendingStage {
            path: stage_path.clone(),
            dev: created.dev(),
            ino: created.ino(),
            armed: true,
        };
        let stage = open_stage(&stage_path)?;
        let m = stage.metadata()?;
        let named = fs::symlink_metadata(&stage_path)?;
        anyhow::ensure!(
            m.is_dir()
                && m.dev() == created.dev()
                && m.ino() == created.ino()
                && named.is_dir()
                && named.dev() == created.dev()
                && named.ino() == created.ino(),
            "staging directory changed during initialization"
        );
        pending.armed = false;
        Ok(Self {
            destination,
            payload: stage_path.join("payload"),
            parent,
            stage,
            stage_ino: m.ino(),
            stage_dev: m.dev(),
            stage_path,
            remove: true,
        })
    }
    pub fn recovery_path(&mut self) -> Option<PathBuf> {
        let actual = fs::read_link(format!("/proc/self/fd/{}", self.stage.as_raw_fd()))
            .ok()?
            .join("payload");
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&actual)
            .ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_BYTES {
            return None;
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .ok()?;
        self.remove = false;
        Some(actual)
    }
    pub fn publish(&mut self) -> Result<PathBuf> {
        let current_parent = directory(self.destination.parent().unwrap())?;
        let a = current_parent.metadata()?;
        let b = self.parent.metadata()?;
        anyhow::ensure!(
            a.dev() == b.dev() && a.ino() == b.ino(),
            "destination directory changed during download"
        );
        let current_stage = directory(&self.stage_path)?.metadata()?;
        anyhow::ensure!(
            current_stage.dev() == self.stage_dev && current_stage.ino() == self.stage_ino,
            "staging directory changed during download"
        );
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(format!("/proc/self/fd/{}/payload", self.stage.as_raw_fd()))?;
        let metadata = file.metadata()?;
        anyhow::ensure!(
            metadata.is_file() && metadata.nlink() == 1 && metadata.len() <= MAX_BYTES,
            "invalid or oversized downloaded payload"
        );
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
        let source = CString::new("payload")?;
        use std::os::unix::ffi::OsStrExt;
        let target = CString::new(self.destination.file_name().unwrap().as_bytes())?;
        // SAFETY: held real directory descriptors, direct-child CString names;
        // NOREPLACE prevents races from overwriting a newly-created user file.
        let rc = unsafe {
            libc::renameat2(
                self.stage.as_raw_fd(),
                source.as_ptr(),
                self.parent.as_raw_fd(),
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error())
                .context("could not publish without overwriting destination");
        }
        let _ = self.parent.sync_all();
        Ok(self.destination.clone())
    }
}
impl Drop for Transfer {
    fn drop(&mut self) {
        if !self.remove {
            return;
        }
        // Never delete the selected destination, only the exact private staging
        // directory this operation created, if its path still identifies it.
        if let Ok(metadata) = fs::symlink_metadata(&self.stage_path) {
            if metadata.is_dir()
                && metadata.ino() == self.stage_ino
                && metadata.dev() == self.stage_dev
            {
                assert!(self
                    .stage_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".dsh-ocean-download-"));
                let _ = fs::remove_dir_all(&self.stage_path);
            }
        }
    }
}

#[cfg(test)]
mod initialization_tests {
    use super::*;
    use std::cell::RefCell;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("dsh-ocean-init-test-{}", unique_id()));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            Self(fs::canonicalize(root).unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert!(
                self.0.is_absolute()
                    && self
                        .0
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("dsh-ocean-init-test-")
            );
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn stage_open_failure_cleans_its_exact_empty_directory() {
        let f = Fixture::new();
        let seen = RefCell::new(None);
        let result = Transfer::prepare_with_stage_directory(&f.0.join("save"), |path| {
            assert!(path.is_dir());
            assert_eq!(fs::metadata(path)?.mode() & 0o777, 0o700);
            *seen.borrow_mut() = Some(path.to_owned());
            Err(std::io::Error::from_raw_os_error(libc::EMFILE).into())
        });
        assert!(result.is_err());
        assert!(!seen.borrow().as_ref().unwrap().exists());
        assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
    }
    #[test]
    fn unexpected_directory_descriptor_is_rejected_without_removing_parent() {
        let f = Fixture::new();
        fs::write(f.0.join("user-file"), b"preserve").unwrap();
        let result = Transfer::prepare_with_stage_directory(&f.0.join("save"), |_| directory(&f.0));
        assert!(result.is_err());
        assert_eq!(fs::read(f.0.join("user-file")).unwrap(), b"preserve");
        assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
    }
    #[test]
    fn initialization_guard_leaves_a_replaced_path_untouched() {
        let f = Fixture::new();
        let seen = RefCell::new(None);
        let retained = f.0.join("retained-original");
        let result = Transfer::prepare_with_stage_directory(&f.0.join("save"), |path| {
            assert_eq!(path.parent(), Some(f.0.as_path()));
            assert_eq!(retained.parent(), Some(f.0.as_path()));
            fs::rename(path, &retained)?;
            fs::create_dir(path)?;
            fs::write(path.join("marker"), b"replacement")?;
            *seen.borrow_mut() = Some(path.to_owned());
            directory(&retained)
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read(seen.borrow().as_ref().unwrap().join("marker")).unwrap(),
            b"replacement"
        );
        assert!(retained.is_dir());
        assert!(!f.0.join("save").exists());
    }
    #[test]
    fn initialization_guard_never_recursively_removes_unexpected_contents() {
        let f = Fixture::new();
        let seen = RefCell::new(None);
        let result = Transfer::prepare_with_stage_directory(&f.0.join("save"), |path| {
            fs::write(path.join("marker"), b"leave unexpected contents")?;
            *seen.borrow_mut() = Some(path.to_owned());
            Err(std::io::Error::from_raw_os_error(libc::EIO).into())
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read(seen.borrow().as_ref().unwrap().join("marker")).unwrap(),
            b"leave unexpected contents"
        );
        assert!(!f.0.join("save").exists());
    }
}

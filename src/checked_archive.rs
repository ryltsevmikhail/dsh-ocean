//! Bounded archive boundary for authenticated shell/runtime packages. Authentication
//! belongs to the caller; this module never executes files or follows archive links.
use anyhow::{Context, Result};
use flate2::read::MultiGzDecoder;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_EXTENSION: u64 = 65536;
const MAX_PATH_BYTES: usize = 16 * 1024 * 1024;
const MAX_PATH: usize = 4096;
const MAX_TRAILER: u64 = 65536;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy)]
pub struct Limits {
    pub entries: usize,
    pub bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: 200_000,
            bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRecord {
    pub size: u64,
    pub sha256: String,
    pub executable: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub schema: u32,
    pub files: BTreeMap<String, FileRecord>,
    pub directories: BTreeSet<String>,
}
impl Inventory {
    /// Every installed payload file and directory must match. Only explicit
    /// direct-child manager metadata names may exist outside the signed payload.
    pub fn verify(&self, root: &Path, metadata: &[&str]) -> Result<()> {
        anyhow::ensure!(
            self.schema == 1
                && self.files.len() + self.directories.len() <= Limits::default().entries,
            "invalid installed file inventory"
        );
        safe_directory_chain(root)?;
        let mut actual_files = BTreeSet::new();
        let mut actual_dirs = BTreeSet::new();
        collect(root, root, &mut actual_files, &mut actual_dirs)?;
        let mut expected: BTreeSet<String> = self.files.keys().cloned().collect();
        for name in metadata {
            anyhow::ensure!(
                !name.is_empty()
                    && !name.contains('/')
                    && !name.contains('\\')
                    && *name != "."
                    && *name != "..",
                "invalid manager metadata name"
            );
            anyhow::ensure!(
                !expected.contains(*name),
                "manager metadata overlaps package content"
            );
            let m = fs::symlink_metadata(root.join(name))?;
            anyhow::ensure!(
                m.is_file()
                    && m.uid() == unsafe { libc::geteuid() }
                    && m.nlink() == 1
                    && m.mode() & 0o022 == 0,
                "unsafe manager metadata"
            );
            expected.insert((*name).to_owned());
        }
        anyhow::ensure!(
            actual_files == expected && actual_dirs == self.directories,
            "installed file set changed"
        );
        let mut bytes = 0u64;
        let mut names = 0usize;
        for (relative, record) in &self.files {
            validate_relative(relative)?;
            names = names
                .checked_add(relative.len())
                .context("inventory path overflow")?;
            bytes = bytes
                .checked_add(record.size)
                .context("inventory size overflow")?;
            anyhow::ensure!(
                record.size <= MAX_FILE
                    && bytes <= Limits::default().bytes
                    && names <= MAX_PATH_BYTES,
                "installed inventory exceeds bounds"
            );
            let (size, hash, executable) = hash_file(&root.join(relative))?;
            anyhow::ensure!(
                size == record.size && hash == record.sha256 && executable == record.executable,
                "installed file integrity mismatch"
            );
        }
        for relative in &self.directories {
            validate_relative(relative)?;
        }
        Ok(())
    }
}

fn safe_directory_chain(path: &Path) -> Result<()> {
    anyhow::ensure!(
        path.is_absolute()
            && path
                .components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
        "archive output must be an absolute normalized path"
    );
    let uid = unsafe { libc::geteuid() };
    for parent in path.ancestors() {
        let m = fs::symlink_metadata(parent)?;
        anyhow::ensure!(
            m.is_dir()
                && (m.uid() == uid || m.uid() == 0)
                && (m.mode() & 0o022 == 0 || (m.uid() == 0 && m.mode() & libc::S_ISVTX != 0)),
            "unsafe archive directory ancestor"
        );
    }
    Ok(())
}
pub(crate) struct NewDirectory {
    path: PathBuf,
    dev: u64,
    ino: u64,
    remove: bool,
}
impl NewDirectory {
    pub(crate) fn create(path: &Path) -> Result<Self> {
        let parent = path.parent().context("archive output has no parent")?;
        safe_directory_chain(parent)?;
        anyhow::ensure!(
            path.is_absolute()
                && path
                    .components()
                    .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
            "invalid extraction path"
        );
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .context("archive output must be a new directory")?;
        let m = fs::symlink_metadata(path)?;
        let guard = Self {
            path: path.to_owned(),
            dev: m.dev(),
            ino: m.ino(),
            remove: true,
        };
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        guard.check()?;
        Ok(guard)
    }
    pub(crate) fn check(&self) -> Result<()> {
        let m = fs::symlink_metadata(&self.path)?;
        anyhow::ensure!(
            m.is_dir()
                && m.dev() == self.dev
                && m.ino() == self.ino
                && m.uid() == unsafe { libc::geteuid() }
                && m.mode() & 0o077 == 0,
            "private extraction directory changed"
        );
        Ok(())
    }
}
impl Drop for NewDirectory {
    fn drop(&mut self) {
        if self.remove && self.check().is_ok() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
fn validate_relative(path: &str) -> Result<()> {
    anyhow::ensure!(
        !path.is_empty()
            && path.len() <= MAX_PATH
            && path.split('/').count() <= 128
            && !path.contains('\\')
            && !path.chars().any(char::is_control)
            && path
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != ".."),
        "non-canonical archive path refused"
    );
    Ok(())
}
fn member(bytes: &[u8], root: &str, directory: bool) -> Result<String> {
    let raw =
        std::str::from_utf8(bytes).map_err(|_| anyhow::anyhow!("non-UTF8 archive path refused"))?;
    let raw = if directory {
        raw.strip_suffix('/').unwrap_or(raw)
    } else {
        raw
    };
    validate_relative(raw)?;
    if raw == root {
        anyhow::ensure!(directory, "package root must be a directory");
        return Ok(String::new());
    }
    let relative = raw
        .strip_prefix(root)
        .and_then(|p| p.strip_prefix('/'))
        .context("archive member is outside the expected package root")?;
    validate_relative(relative)?;
    Ok(relative.to_owned())
}
fn mode(header: &tar::Header) -> Result<bool> {
    let mode = header.mode()?;
    anyhow::ensure!(
        mode & !0o7777 == 0 && mode & 0o7022 == 0,
        "unsafe archive permissions refused"
    );
    Ok(mode & 0o111 != 0)
}
fn wire_limit(limits: Limits) -> Result<u64> {
    anyhow::ensure!(
        limits.entries > 0
            && limits.entries <= Limits::default().entries
            && limits.bytes > 0
            && limits.bytes <= Limits::default().bytes,
        "invalid archive bounds"
    );
    Ok(limits.bytes + (limits.entries as u64) * 1024 + MAX_TRAILER)
}
fn drain<R: Read>(input: &mut R) -> Result<()> {
    let mut count = 0u64;
    let mut buffer = [0u8; 8192];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        anyhow::ensure!(
            count <= MAX_TRAILER && buffer[..n].iter().all(|b| *b == 0),
            "unexpected trailing archive data"
        );
    }
    Ok(())
}
/// Raw preflight prevents the tar library from buffering a giant PAX/GNU
/// extension before the logical-entry caller can enforce its size. Stable
/// authenticated input is then rewound and parsed normally by the same crate.
fn preflight<R: Read + Seek>(input: &mut R, limits: Limits, wire: u64) -> Result<()> {
    input.rewind()?;
    let mut archive = tar::Archive::new(MultiGzDecoder::new(&mut *input).take(wire + 1));
    let mut entries = 0usize;
    let mut bytes = 0u64;
    for entry in archive.entries()?.raw(true) {
        let mut entry = entry.context("invalid physical archive header")?;
        entries += 1;
        anyhow::ensure!(
            entries <= limits.entries,
            "archive entry count exceeds limit"
        );
        let kind = entry.header().entry_type();
        let size = entry.size();
        if kind == tar::EntryType::XHeader || kind == tar::EntryType::GNULongName {
            anyhow::ensure!(size <= MAX_EXTENSION, "archive extension exceeds limit");
        } else if kind == tar::EntryType::Directory {
            anyhow::ensure!(size == 0, "directory payload refused");
            mode(entry.header())?;
        } else {
            anyhow::ensure!(
                kind == tar::EntryType::Regular,
                "archive links/special/sparse files refused"
            );
            mode(entry.header())?;
            bytes = bytes.checked_add(size).context("archive size overflow")?;
            anyhow::ensure!(
                size <= MAX_FILE && bytes <= limits.bytes,
                "expanded archive exceeds limit"
            );
        }
        let copied = std::io::copy(&mut entry, &mut std::io::sink())?;
        anyhow::ensure!(copied == size, "truncated archive entry");
    }
    let mut decoder = archive.into_inner();
    drain(&mut decoder)?;
    anyhow::ensure!(
        wire + 1 - decoder.limit() <= wire,
        "expanded archive stream exceeds limit"
    );
    Ok(())
}
fn add_dirs(path: &str, include: bool, inventory: &mut Inventory, names: &mut usize) -> Result<()> {
    let mut directory = if include {
        Some(Path::new(path))
    } else {
        Path::new(path).parent()
    };
    while let Some(dir) = directory {
        if let Some(text) = dir.to_str().filter(|s| !s.is_empty()) {
            if inventory.directories.insert(text.to_owned()) {
                *names = names
                    .checked_add(text.len())
                    .context("archive path overflow")?;
            }
        }
        directory = dir.parent();
    }
    anyhow::ensure!(
        *names <= MAX_PATH_BYTES,
        "archive path inventory exceeds limit"
    );
    Ok(())
}
fn identity(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u64, u32, u32) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.nlink(),
        m.mode(),
        m.uid(),
    )
}
fn hash_file(path: &Path) -> Result<(u64, String, bool)> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let before = file.metadata()?;
    let permissions = before.mode() & 0o777;
    anyhow::ensure!(
        before.is_file()
            && before.nlink() == 1
            && before.uid() == unsafe { libc::geteuid() }
            && before.len() <= MAX_FILE
            && matches!(permissions, 0o644 | 0o755),
        "unsafe installed package file"
    );
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        anyhow::ensure!(
            size <= before.len(),
            "installed file grew during verification"
        );
        hash.update(&buffer[..n]);
    }
    let after = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        identity(&before) == identity(&after)
            && identity(&named) == identity(&after)
            && size == before.len(),
        "installed file changed during verification"
    );
    Ok((
        size,
        crate::release_feed::hex(&hash.finalize()),
        permissions == 0o755,
    ))
}
fn collect(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<String>,
    dirs: &mut BTreeSet<String>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let relative = path
            .strip_prefix(root)?
            .to_str()
            .context("invalid installed filename")?
            .to_owned();
        validate_relative(&relative)?;
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
            "unsafe installed entry ownership/mode"
        );
        if metadata.is_dir() {
            dirs.insert(relative);
            collect(root, &path, files, dirs)?;
        } else {
            anyhow::ensure!(
                metadata.is_file() && metadata.nlink() == 1,
                "installed link/special entry refused"
            );
            files.insert(relative);
        }
        anyhow::ensure!(
            files.len() + dirs.len() <= Limits::default().entries,
            "installed directory entry count exceeds limit"
        );
    }
    Ok(())
}

pub fn extract<R: Read + Seek>(
    input: &mut R,
    expected_root: &str,
    destination: &Path,
    limits: Limits,
) -> Result<Inventory> {
    validate_relative(expected_root)?;
    anyhow::ensure!(
        !expected_root.contains('/'),
        "expected root must be one component"
    );
    let wire = wire_limit(limits)?;
    preflight(input, limits, wire)?;
    input.rewind()?;
    let mut guard = NewDirectory::create(destination)?;
    let mut archive = tar::Archive::new(MultiGzDecoder::new(&mut *input).take(wire + 1));
    let mut inventory = Inventory {
        schema: 1,
        files: BTreeMap::new(),
        directories: BTreeSet::new(),
    };
    let mut seen = BTreeSet::new();
    let mut names = 0usize;
    let mut total = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry.context("invalid logical archive entry")?;
        guard.check()?;
        anyhow::ensure!(
            seen.len() < limits.entries,
            "logical archive entry count exceeds limit"
        );
        let kind = entry.header().entry_type();
        anyhow::ensure!(
            kind == tar::EntryType::Regular || kind == tar::EntryType::Directory,
            "archive links/special/sparse entries refused"
        );
        let directory = kind == tar::EntryType::Directory;
        let relative = member(&entry.path_bytes(), expected_root, directory)?;
        anyhow::ensure!(
            seen.insert(relative.clone()),
            "duplicate archive path refused"
        );
        if relative.is_empty() {
            continue;
        }
        names = names
            .checked_add(relative.len())
            .context("archive path size overflow")?;
        anyhow::ensure!(names <= MAX_PATH_BYTES, "archive paths exceed limit");
        let output = destination.join(&relative);
        let executable = mode(entry.header())?;
        if directory {
            anyhow::ensure!(entry.size() == 0, "directory payload refused");
            add_dirs(&relative, true, &mut inventory, &mut names)?;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&output)?;
            continue;
        }
        let size = entry.size();
        total = total.checked_add(size).context("expanded size overflow")?;
        anyhow::ensure!(
            size <= MAX_FILE && total <= limits.bytes,
            "expanded archive exceeds limit"
        );
        add_dirs(&relative, false, &mut inventory, &mut names)?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(output.parent().context("member has no parent")?)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&output)?;
        let mut hash = Sha256::new();
        let mut copied = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            anyhow::ensure!(copied <= size, "archive member exceeds declared size");
            file.write_all(&buffer[..n])?;
            hash.update(&buffer[..n]);
        }
        anyhow::ensure!(copied == size, "truncated archive member");
        file.set_permissions(fs::Permissions::from_mode(if executable {
            0o755
        } else {
            0o644
        }))?;
        file.sync_all()?;
        inventory.files.insert(
            relative,
            FileRecord {
                size,
                sha256: crate::release_feed::hex(&hash.finalize()),
                executable,
            },
        );
    }
    let mut decoder = archive.into_inner();
    drain(&mut decoder)?;
    anyhow::ensure!(
        wire + 1 - decoder.limit() <= wire && !inventory.files.is_empty(),
        "expanded/empty archive refused"
    );
    for directory in inventory.directories.iter().rev() {
        File::open(destination.join(directory))?.sync_all()?;
    }
    File::open(destination)?.sync_all()?;
    guard.check()?;
    guard.remove = false;
    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::io::{Cursor, Write};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ocean-archive-{}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn output(&self) -> PathBuf {
            self.0.join("extracted")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn archive(entries: &[(&str, &[u8], tar::EntryType, u32)]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, bytes, kind, mode) in entries {
            let mut header = tar::Header::new_ustar();
            header.set_path("placeholder").unwrap();
            let raw = header.as_mut_bytes();
            raw[..100].fill(0);
            assert!(path.len() < 100);
            raw[..path.len()].copy_from_slice(path.as_bytes());
            header.set_entry_type(*kind);
            header.set_size(bytes.len() as u64);
            header.set_mode(*mode);
            header.set_mtime(0);
            if *kind == tar::EntryType::Symlink || *kind == tar::EntryType::Link {
                header.set_link_name("../outside").unwrap();
            }
            header.set_cksum();
            tar.append(&header, *bytes).unwrap();
        }
        let raw = tar.into_inner().unwrap();
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&raw).unwrap();
        gzip.finish().unwrap()
    }
    fn sample() -> Vec<u8> {
        archive(&[
            ("package/", b"", tar::EntryType::Directory, 0o755),
            (
                "package/dsh-ocean",
                b"signed-binary",
                tar::EntryType::Regular,
                0o755,
            ),
            (
                "package/docs/NOTICE",
                b"public-notice",
                tar::EntryType::Regular,
                0o644,
            ),
            ("package/empty", b"", tar::EntryType::Regular, 0o644),
        ])
    }
    #[test]
    fn retains_all_package_bytes_modes_and_integrity_index() {
        let root = Fixture::new();
        let output = root.output();
        let inventory = extract(
            &mut Cursor::new(sample()),
            "package",
            &output,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("docs/NOTICE")).unwrap(),
            b"public-notice"
        );
        assert_eq!(inventory.files.len(), 3);
        assert!(inventory.directories.contains("docs"));
        assert_eq!(
            inventory.files["dsh-ocean"].sha256,
            crate::release_feed::hex(&Sha256::digest(b"signed-binary"))
        );
        assert_eq!(
            fs::metadata(output.join("dsh-ocean"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o700
        );
        inventory.verify(&output, &[]).unwrap();
        fs::write(output.join("docs/NOTICE"), b"changed-notice").unwrap();
        assert!(inventory.verify(&output, &[]).is_err());
        fs::write(output.join("docs/NOTICE"), b"public-notice").unwrap();
        fs::write(output.join("unexpected.js"), b"extra").unwrap();
        assert!(
            inventory.verify(&output, &[]).is_err(),
            "unexpected files are not trusted code"
        );
    }
    #[test]
    fn refuses_traversal_aliases_links_special_files_and_permission_escalation() {
        for (path, kind, mode) in [
            ("../outside", tar::EntryType::Regular, 0o644),
            ("/package/absolute", tar::EntryType::Regular, 0o644),
            ("package/../outside", tar::EntryType::Regular, 0o644),
            ("package/./alias", tar::EntryType::Regular, 0o644),
            ("package//alias", tar::EntryType::Regular, 0o644),
            ("other/file", tar::EntryType::Regular, 0o644),
            ("package/link", tar::EntryType::Symlink, 0o755),
            ("package/hardlink", tar::EntryType::Link, 0o644),
            ("package/pipe", tar::EntryType::Fifo, 0o644),
            ("package/device", tar::EntryType::Char, 0o644),
            ("package/suid", tar::EntryType::Regular, 0o4755),
            ("package/writeable", tar::EntryType::Regular, 0o666),
        ] {
            let root = Fixture::new();
            let input = archive(&[(path, b"", kind, mode)]);
            assert!(
                extract(
                    &mut Cursor::new(input),
                    "package",
                    &root.output(),
                    Limits::default()
                )
                .is_err(),
                "{path}"
            );
            assert!(
                !root.output().exists(),
                "failed extraction removes only its new private directory"
            );
            assert!(!root.0.join("outside").exists());
        }
    }
    #[test]
    fn refuses_duplicates_overflow_bombs_corruption_and_existing_destinations() {
        let duplicate = archive(&[
            ("package/file", b"a", tar::EntryType::Regular, 0o644),
            ("package/file", b"b", tar::EntryType::Regular, 0o644),
        ]);
        let root = Fixture::new();
        assert!(extract(
            &mut Cursor::new(duplicate),
            "package",
            &root.output(),
            Limits::default()
        )
        .is_err());
        assert!(!root.output().exists());
        for limits in [
            Limits {
                entries: 1,
                bytes: 1024,
            },
            Limits {
                entries: 10,
                bytes: 4,
            },
        ] {
            let root = Fixture::new();
            assert!(extract(
                &mut Cursor::new(sample()),
                "package",
                &root.output(),
                limits
            )
            .is_err());
            assert!(!root.output().exists());
        }
        let root = Fixture::new();
        let mut bad = sample();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(extract(
            &mut Cursor::new(bad),
            "package",
            &root.output(),
            Limits::default()
        )
        .is_err());
        assert!(!root.output().exists());
        let root = Fixture::new();
        fs::create_dir(root.output()).unwrap();
        fs::write(root.output().join("keep"), b"unchanged").unwrap();
        assert!(extract(
            &mut Cursor::new(sample()),
            "package",
            &root.output(),
            Limits::default()
        )
        .is_err());
        assert_eq!(fs::read(root.output().join("keep")).unwrap(), b"unchanged");
    }
    #[test]
    fn supports_bounded_pax_paths_but_rejects_oversized_extension_headers() {
        let root = Fixture::new();
        let long = format!("package/{}/NOTICE", "nested".repeat(24));
        let mut builder = tar::Builder::new(Vec::new());
        builder
            .append_pax_extensions([("path", long.as_bytes())])
            .unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_path("package/placeholder").unwrap();
        header.set_size(6);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, b"notice".as_slice()).unwrap();
        let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
        compressed
            .write_all(&builder.into_inner().unwrap())
            .unwrap();
        let inventory = extract(
            &mut Cursor::new(compressed.finish().unwrap()),
            "package",
            &root.output(),
            Limits::default(),
        )
        .unwrap();
        assert!(inventory
            .files
            .contains_key(long.strip_prefix("package/").unwrap()));
        let root = Fixture::new();
        let oversized = archive(&[("pax", &vec![b'x'; 65537], tar::EntryType::XHeader, 0o644)]);
        assert!(extract(
            &mut Cursor::new(oversized),
            "package",
            &root.output(),
            Limits::default()
        )
        .is_err());
        assert!(!root.output().exists());
    }
}

//! Deterministic, verifiable export of a backup snapshot into one plain tar.
//!
//! The client holds no private key, so an export is **not signed**: it carries the
//! snapshot's own manifest (which contains per-file SHA-256 values) plus the files,
//! in a byte-for-byte reproducible archive. [`verify_export`] re-reads the archive
//! and re-checks every hash against that manifest, so a truncated or edited export
//! is refused. Authenticity of the original snapshot is a separate question.
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

const BLOCK: usize = 512;
const MAX_ENTRIES: usize = 200_000;
const MAX_TOTAL: u64 = 8 * 1024 * 1024 * 1024;
/// Per-entry bound: a single archive member is read into memory, so it must not be
/// able to allocate gigabytes. Snapshots larger than this are refused on export too.
const MAX_ENTRY: u64 = 1024 * 1024 * 1024;
const MAX_NAME: usize = 255;
const MAX_MANIFEST: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportReceipt {
    pub archive: PathBuf,
    pub scope: String,
    pub files: usize,
    pub bytes: u64,
    pub archive_sha256: String,
}

/// Which engine's manifest describes the exported snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Profile,
    Data,
}

fn scope_kind(scope: &str) -> Option<Kind> {
    match scope {
        "profile-config" => Some(Kind::Profile),
        "dsh-data" => Some(Kind::Data),
        _ => None,
    }
}

fn scope_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Profile => "profile-config",
        Kind::Data => "dsh-data",
    }
}

/// Cheap description for a chooser/confirmation: manifest only, no file hashing.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Description {
    pub scope: String,
    pub files: usize,
    pub bytes: u64,
}

/// Reads `manifest.json` without following a symlink or blocking on a FIFO,
/// and without unbounded allocation.
fn read_manifest_bytes(snapshot: &Path) -> Result<Vec<u8>> {
    let path = snapshot.join("manifest.json");
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .context("open the snapshot manifest")?;
    let metadata = file.metadata().context("inspect the snapshot manifest")?;
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "snapshot manifest is not a regular file"
    );
    anyhow::ensure!(
        metadata.len() <= MAX_MANIFEST,
        "snapshot manifest is implausibly large"
    );
    let mut raw = Vec::new();
    (&file)
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut raw)
        .context("read the snapshot manifest")?;
    anyhow::ensure!(
        raw.len() as u64 <= MAX_MANIFEST,
        "snapshot manifest grew while reading"
    );
    Ok(raw)
}

pub fn describe(snapshot: &Path) -> Result<Description> {
    let raw = read_manifest_bytes(snapshot)?;
    let value: serde_json::Value =
        serde_json::from_slice(&raw).context("invalid snapshot manifest")?;
    let scope = value
        .get("scope")
        .and_then(|scope| scope.as_str())
        .unwrap_or_default();
    let kind = scope_kind(scope).context("unrelated snapshot scope")?;
    let files = manifest_files(kind, &raw)?;
    Ok(Description {
        scope: scope_name(kind).to_owned(),
        files: files.len(),
        bytes: files.iter().map(|entry| entry.1).sum(),
    })
}

/// Reads a snapshot's `manifest.json`, decides its kind and verifies the snapshot.
pub fn inspect(snapshot: &Path) -> Result<(Kind, Vec<u8>)> {
    let raw = read_manifest_bytes(snapshot)?;
    let value: serde_json::Value =
        serde_json::from_slice(&raw).context("invalid snapshot manifest")?;
    let scope = value
        .get("scope")
        .and_then(|scope| scope.as_str())
        .unwrap_or_default();
    let kind = scope_kind(scope).context("unrelated snapshot scope")?;
    match kind {
        Kind::Profile => {
            crate::profile_backup::verify(snapshot)?;
        }
        Kind::Data => {
            crate::data_backup::verify(snapshot)?;
        }
    }
    Ok((kind, raw))
}

/// `(path, size, sha256, executable)` for every file in a manifest.
fn manifest_files(kind: Kind, manifest: &[u8]) -> Result<Vec<(String, u64, String, bool)>> {
    let entries: Vec<(String, u64, String, bool)> = match kind {
        Kind::Profile => crate::profile_backup::parse_manifest(manifest)?
            .files
            .into_iter()
            .map(|file| (file.path, file.size, file.sha256, file.executable))
            .collect(),
        Kind::Data => crate::data_backup::parse_manifest(manifest)?
            .files
            .into_iter()
            .map(|file| (file.path, file.size, file.sha256, file.executable))
            .collect(),
    };
    Ok(entries)
}

/// Directory entries recorded in a manifest's own layout.
fn manifest_directories(kind: Kind, manifest: &[u8]) -> Result<Vec<String>> {
    Ok(match kind {
        Kind::Profile => crate::profile_backup::parse_manifest(manifest)?.directories,
        Kind::Data => crate::data_backup::parse_manifest(manifest)?.directories,
    })
}

fn octal(value: u64, field: usize) -> Vec<u8> {
    let digits = field - 1;
    let text = format!("{value:0digits$o}");
    let mut bytes = text.into_bytes();
    bytes.truncate(digits);
    bytes.push(0);
    bytes
}

/// Appends one ustar header (512 bytes) with fixed metadata: no owner names,
/// mtime 0 and no PAX extensions, so the same snapshot always exports identically.
fn header_bytes(name: &str, size: u64, mode: u32) -> Result<[u8; BLOCK]> {
    anyhow::ensure!(
        name.len() <= MAX_NAME && !name.contains('\u{0}'),
        "export path is invalid"
    );
    let mut header = [0u8; BLOCK];
    let (prefix, base) = match name.rsplit_once('/') {
        Some((dir, base)) if dir.len() <= 155 && base.len() <= 100 => (dir, base),
        _ => ("", name),
    };
    anyhow::ensure!(base.len() <= 100, "export entry name is too long");
    header[..base.len()].copy_from_slice(base.as_bytes());
    header[100..108].copy_from_slice(&octal(mode as u64, 8));
    header[108..116].copy_from_slice(&octal(0, 8));
    header[116..124].copy_from_slice(&octal(0, 8));
    header[124..136].copy_from_slice(&octal(size, 12));
    header[136..148].copy_from_slice(&octal(0, 12));
    header[148..156].copy_from_slice(b"        ");
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    if !prefix.is_empty() {
        header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    }
    let checksum: u64 = header.iter().map(|byte| *byte as u64).sum();
    header[148..154].copy_from_slice(format!("{checksum:06o}").as_bytes());
    header[154] = 0;
    header[155] = b' ';
    Ok(header)
}

fn write_entry(out: &mut File, name: &str, size: u64, mode: u32, bytes: &[u8]) -> Result<()> {
    out.write_all(&header_bytes(name, size, mode)?)?;
    out.write_all(bytes)?;
    let remainder = bytes.len() % BLOCK;
    if remainder != 0 {
        out.write_all(&vec![0u8; BLOCK - remainder])?;
    }
    Ok(())
}

/// Lookup table for scanned entries keyed by path.
fn entry_index(entries: &[(String, u64, String)]) -> std::collections::BTreeMap<&str, (u64, &str)> {
    entries
        .iter()
        .map(|(path, size, sha)| (path.as_str(), (*size, sha.as_str())))
        .collect()
}

/// fsync a directory so a renamed entry inside it survives a power loss.
fn sync_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Ok(directory) = fs::File::open(parent) {
                let _ = directory.sync_all();
            }
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(crate::release_feed::hex(&hasher.finalize()))
}

/// Writes a deterministic tar of a verified snapshot. Refuses to overwrite.
pub fn export(snapshot: &Path, destination: &Path) -> Result<ExportReceipt> {
    export_with_checkpoint(snapshot, destination, &mut || Ok(()))
}

/// Like [`export`], but the caller can abort between files; a cancelled export
/// removes the partial archive before returning the error.
pub fn export_with_checkpoint(
    snapshot: &Path,
    destination: &Path,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<ExportReceipt> {
    checkpoint()?;
    let (kind, manifest) = inspect(snapshot)?;
    let entries = manifest_files(kind, &manifest)?;
    anyhow::ensure!(!entries.is_empty(), "snapshot has no files to export");
    anyhow::ensure!(entries.len() <= MAX_ENTRIES, "snapshot has too many files");
    let total: u64 = entries.iter().map(|entry| entry.1).sum();
    anyhow::ensure!(total <= MAX_TOTAL, "snapshot is too large to export");
    anyhow::ensure!(
        entries.iter().all(|entry| entry.1 <= MAX_ENTRY),
        "snapshot contains a file larger than the export limit"
    );
    if let Some(parent) = destination.parent() {
        if !parent.as_os_str().is_empty() {
            // Exports carry session/profile data, so the directory is private too.
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .context("create the export directory")?;
        }
    }
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .context("refusing to overwrite an existing export")?;
    let result = (|| -> Result<ExportReceipt> {
        write_entry(
            &mut out,
            "manifest.json",
            manifest.len() as u64,
            0o600,
            &manifest,
        )?;
        // Snapshot layout: profile snapshots keep files under `data/`, data
        // snapshots already prefix every path with its root.
        let data_root = snapshot.join("data");
        let base = if data_root.is_dir() {
            data_root
        } else {
            snapshot.to_path_buf()
        };
        for (path, size, expected, executable) in &entries {
            checkpoint()?;
            anyhow::ensure!(
                path.split('/')
                    .all(|part| !part.is_empty() && part != "." && part != ".."),
                "snapshot path escapes its root"
            );
            let input = base.join(path);
            let metadata = fs::symlink_metadata(&input)
                .with_context(|| format!("inspect snapshot file {path}"))?;
            anyhow::ensure!(
                metadata.file_type().is_file(),
                "snapshot entry is not a regular file"
            );
            let bytes = fs::read(&input).with_context(|| format!("read snapshot file {path}"))?;
            anyhow::ensure!(bytes.len() as u64 == *size, "snapshot file changed size");
            anyhow::ensure!(
                crate::release_feed::hex(&Sha256::digest(&bytes)) == *expected,
                "snapshot file changed contents"
            );
            write_entry(
                &mut out,
                &format!("data/{path}"),
                *size,
                if *executable { 0o755 } else { 0o644 },
                &bytes,
            )?;
        }
        out.write_all(&[0u8; BLOCK * 2])?;
        out.sync_all()?;
        sync_parent(destination)?;
        Ok(ExportReceipt {
            archive: destination.to_path_buf(),
            scope: scope_name(kind).to_owned(),
            files: entries.len(),
            bytes: total,
            archive_sha256: hash_file(destination)?,
        })
    })();
    drop(out);
    if result.is_err() {
        // Only the archive this call created is removed.
        let _ = fs::remove_file(destination);
    }
    result
}

struct Header {
    name: String,
    size: u64,
}

fn parse_octal(bytes: &[u8]) -> Result<u64> {
    let text: String = bytes
        .iter()
        .take_while(|byte| **byte != 0 && **byte != b' ')
        .map(|byte| *byte as char)
        .collect();
    anyhow::ensure!(
        !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()),
        "invalid octal field in export"
    );
    u64::from_str_radix(&text, 8).context("invalid octal field in export")
}

/// Reads exactly `count` bytes, refusing a short archive.
fn read_exact_bytes(file: &mut File, count: usize) -> Result<()> {
    let mut buffer = vec![0u8; count];
    let mut filled = 0usize;
    while filled < count {
        let read = file.read(&mut buffer[filled..])?;
        anyhow::ensure!(read > 0, "truncated export archive");
        filled += read;
    }
    Ok(())
}

fn read_exact_block(file: &mut File) -> Result<Option<[u8; BLOCK]>> {
    let mut block = [0u8; BLOCK];
    let mut filled = 0usize;
    while filled < BLOCK {
        let read = file.read(&mut block[filled..])?;
        if read == 0 {
            anyhow::ensure!(filled == 0, "truncated export archive");
            return Ok(None);
        }
        filled += read;
    }
    Ok(Some(block))
}

fn read_header(file: &mut File) -> Result<Option<Header>> {
    let Some(header) = read_exact_block(file)? else {
        return Ok(None);
    };
    if header.iter().all(|byte| *byte == 0) {
        return Ok(None);
    }
    let stored = parse_octal(&header[148..156])?;
    let mut copy = header;
    copy[148..156].copy_from_slice(b"        ");
    let computed: u64 = copy.iter().map(|byte| *byte as u64).sum();
    anyhow::ensure!(stored == computed, "export header checksum mismatch");
    anyhow::ensure!(&header[257..262] == b"ustar", "export entry is not ustar");
    anyhow::ensure!(
        header[156] == b'0' || header[156] == 0,
        "unexpected export entry type"
    );
    let base: String = header[..100]
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as char)
        .collect();
    let prefix: String = header[345..500]
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as char)
        .collect();
    let name = if prefix.is_empty() {
        base
    } else {
        format!("{prefix}/{base}")
    };
    anyhow::ensure!(!name.is_empty(), "export entry has an empty name");
    Ok(Some(Header {
        name,
        size: parse_octal(&header[124..136])?,
    }))
}

/// Result of importing an export back into a snapshot directory.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportReceipt {
    pub snapshot: PathBuf,
    pub scope: String,
    pub files: usize,
    pub bytes: u64,
}

/// One scanned archive entry: path, size and content hash.
type ScannedEntry = (String, u64, String);

/// What a single pass over an archive yields.
struct Scanned {
    manifest: Vec<u8>,
    kind: Kind,
    entries: Vec<ScannedEntry>,
}

/// Single pass over an archive: hashes every entry and hands its bytes to `sink`.
fn scan_archive<F>(archive: &Path, mut sink: F) -> Result<Scanned>
where
    F: FnMut(&str, &[u8]) -> Result<()>,
{
    let mut file = File::open(archive).context("open the export archive")?;
    let mut manifest: Option<Vec<u8>> = None;
    let mut entries: Vec<ScannedEntry> = Vec::new();
    let mut total_read = 0u64;
    while let Some(header) = read_header(&mut file)? {
        anyhow::ensure!(entries.len() <= MAX_ENTRIES, "export has too many entries");
        anyhow::ensure!(
            header.size <= MAX_ENTRY,
            "export entry is implausibly large"
        );
        total_read = total_read.saturating_add(header.size);
        anyhow::ensure!(total_read <= MAX_TOTAL, "export is implausibly large");
        let mut content = Vec::new();
        (&mut file)
            .take(header.size)
            .read_to_end(&mut content)
            .context("read export entry")?;
        anyhow::ensure!(
            content.len() as u64 == header.size,
            "truncated export entry"
        );
        let remainder = (header.size % BLOCK as u64) as usize;
        if remainder != 0 {
            read_exact_bytes(&mut file, BLOCK - remainder)?;
        }
        if header.name == "manifest.json" {
            anyhow::ensure!(manifest.is_none(), "duplicate manifest in export");
            manifest = Some(content.clone());
        } else {
            let path = header
                .name
                .strip_prefix("data/")
                .context("unexpected entry in export")?
                .to_owned();
            anyhow::ensure!(
                path.split('/')
                    .all(|part| !part.is_empty() && part != "." && part != ".."),
                "export path escapes its root"
            );
            entries.push((
                path,
                header.size,
                crate::release_feed::hex(&Sha256::digest(&content)),
            ));
        }
        sink(&header.name, &content)?;
    }
    let manifest = manifest.context("export has no manifest")?;
    anyhow::ensure!(
        manifest.len() as u64 <= MAX_MANIFEST,
        "export manifest is too large"
    );
    let value: serde_json::Value = serde_json::from_slice(&manifest)?;
    let scope = value
        .get("scope")
        .and_then(|scope| scope.as_str())
        .unwrap_or_default();
    let kind = scope_kind(scope).context("unrelated snapshot scope in export")?;
    Ok(Scanned {
        manifest,
        kind,
        entries,
    })
}

/// Extracts an export into a new snapshot directory, re-verifying every hash.
///
/// The destination must not exist yet. Extraction happens in a private sibling
/// directory and is only renamed into place after the whole archive matched its
/// manifest, so a rejected archive never leaves a half-written snapshot behind.
pub fn import(archive: &Path, destination: &Path) -> Result<ImportReceipt> {
    anyhow::ensure!(
        !destination.exists(),
        "refusing to import into an existing path"
    );
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).context("create the import parent directory")?;
    let stamp = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    );
    let mut staging = destination.to_path_buf();
    staging.set_file_name(format!(
        ".{}.import-{stamp}",
        destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "snapshot".to_owned())
    ));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir(&staging).context("create the import staging directory")?;
    let result = (|| -> Result<ImportReceipt> {
        fs::create_dir_all(staging.join("data"))?;
        let Scanned {
            manifest,
            kind,
            entries,
        } = scan_archive(archive, |name, content| {
            let relative = if name == "manifest.json" {
                PathBuf::from("manifest.json")
            } else {
                let path = name
                    .strip_prefix("data/")
                    .context("unexpected entry in export")?;
                anyhow::ensure!(
                    path.split('/')
                        .all(|part| !part.is_empty() && part != "." && part != ".."),
                    "export path escapes its root"
                );
                PathBuf::from("data").join(path)
            };
            let target = staging.join(relative);
            if let Some(directory) = target.parent() {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(directory)?;
            }
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true).mode(0o600);
            let mut file = options.open(&target)?;
            file.write_all(content)?;
            file.sync_all()?;
            Ok(())
        })?;
        let expected = manifest_files(kind, &manifest)?;
        anyhow::ensure!(
            expected.len() == entries.len(),
            "export entry count does not match its manifest"
        );
        // Directories recorded in the manifest may hold no files at all; the engine
        // verify requires every one of them, so recreate them explicitly.
        for directory in manifest_directories(kind, &manifest)? {
            anyhow::ensure!(
                directory
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != ".."),
                "export directory path escapes its root"
            );
            let target = staging.join("data").join(&directory);
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&target)
                .with_context(|| format!("create imported directory {directory}"))?;
        }
        // The engine stores the executable bit in the manifest, and the plain tar
        // member mode already reflects it; apply it to the extracted files.
        for (path, _size, _sha, executable) in &expected {
            let target = staging.join("data").join(path);
            fs::set_permissions(
                &target,
                fs::Permissions::from_mode(if *executable { 0o700 } else { 0o600 }),
            )
            .with_context(|| format!("set permissions on imported {path}"))?;
        }
        let index = entry_index(&entries);
        anyhow::ensure!(
            index.len() == entries.len(),
            "export contains duplicate entries"
        );
        for (path, size, sha, _executable) in &expected {
            let (found_size, found_sha) = index
                .get(path.as_str())
                .with_context(|| format!("export is missing {path}"))?;
            anyhow::ensure!(
                found_size == size && found_sha == sha,
                "export integrity check failed"
            );
        }
        // Let the owning engine verify the extracted snapshot before publishing it.
        match kind {
            Kind::Profile => {
                crate::profile_backup::verify(&staging)?;
            }
            Kind::Data => {
                crate::data_backup::verify(&staging)?;
            }
        }
        fs::rename(&staging, destination).context("publish the imported snapshot")?;
        sync_parent(destination)?;
        Ok(ImportReceipt {
            snapshot: destination.to_path_buf(),
            scope: scope_name(kind).to_owned(),
            files: expected.len(),
            bytes: expected.iter().map(|entry| entry.1).sum(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Re-reads an export and re-checks every file hash against its manifest.
pub fn verify_export(archive: &Path) -> Result<ExportReceipt> {
    let Scanned {
        manifest,
        kind,
        entries,
    } = scan_archive(archive, |_, _| Ok(()))?;
    let expected = manifest_files(kind, &manifest)?;
    anyhow::ensure!(
        expected.len() == entries.len(),
        "export entry count does not match its manifest"
    );
    let index = entry_index(&entries);
    anyhow::ensure!(
        index.len() == entries.len(),
        "export contains duplicate entries"
    );
    let mut total = 0u64;
    for (path, size, sha, _executable) in &expected {
        let (found_size, found_sha) = index
            .get(path.as_str())
            .with_context(|| format!("export is missing {path}"))?;
        anyhow::ensure!(
            found_size == size && found_sha == sha,
            "export integrity check failed"
        );
        total = total.saturating_add(*size);
    }
    Ok(ExportReceipt {
        archive: archive.to_path_buf(),
        scope: scope_name(kind).to_owned(),
        files: expected.len(),
        bytes: total,
        archive_sha256: hash_file(archive)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_fields_are_padded_and_parsed_back() {
        assert_eq!(octal(0, 12), b"00000000000\0".to_vec());
        assert_eq!(octal(0o755, 8), b"0000755\0".to_vec());
        assert_eq!(parse_octal(&octal(123_456, 12)).unwrap(), 123_456);
        assert!(parse_octal(b"    ").is_err());
        assert!(parse_octal(b"12x9\0").is_err());
    }

    #[test]
    fn long_names_use_the_ustar_prefix_and_oversize_names_are_refused() {
        let long = format!("sessions/{}/payload.json", "a".repeat(120));
        let header = header_bytes(&long, 10, 0o644).unwrap();
        assert_eq!(&header[257..262], b"ustar");
        assert!(header[345..].starts_with(b"sessions/"));
        assert!(header[..100].starts_with(b"payload.json"));
        assert!(header_bytes(&"x".repeat(300), 1, 0o644).is_err());
        assert!(header_bytes("a\0b", 1, 0o644).is_err());
    }

    #[test]
    fn an_archive_that_claims_an_oversized_entry_is_refused() {
        // Hand-build a tar with one header claiming a 2 GiB member: the reader must
        // refuse on the declared size instead of trying to allocate it.
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-export-oversize-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("oversize.tar");
        let header = header_bytes("data/huge.bin", MAX_ENTRY + 1, 0o600).unwrap();
        let bytes = header.to_vec();
        // No content: the size check must fire before any read.
        std::fs::write(&archive, &bytes).unwrap();
        let outcome = verify_export(&archive);
        assert!(outcome.is_err(), "an oversized entry was accepted");
        assert!(
            format!("{:#}", outcome.unwrap_err()).contains("implausibly large"),
            "unexpected error"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn header_checksum_matches_a_recomputed_sum() {
        let header = header_bytes("data/a.json", 1, 0o600).unwrap();
        let stored = parse_octal(&header[148..156]).unwrap();
        let mut copy = header;
        copy[148..156].copy_from_slice(b"        ");
        let computed: u64 = copy.iter().map(|byte| *byte as u64).sum();
        assert_eq!(stored, computed);
    }
}

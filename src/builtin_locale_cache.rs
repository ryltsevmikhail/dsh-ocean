//! Atomic, content-addressed cache for embedded resources. Low-level only: the
//! caller MUST establish profile consent/owned-start and pending-operation gates
//! before calling this; it must never be called from an attached runtime path.
use crate::{builtin_locales, profile_backup as fs_at};
use anyhow::{Context, Result};
use std::fs::{self, File, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn own(metadata: &fs::Metadata) -> bool {
    // SAFETY: geteuid has no pointer or memory preconditions.
    metadata.uid() == unsafe { libc::geteuid() }
}
fn missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}
fn sealed_directory(dir: &File) -> Result<()> {
    let metadata = dir.metadata()?;
    anyhow::ensure!(
        metadata.is_dir() && own(&metadata) && metadata.mode() & 0o222 == 0,
        "unsafe built-in resource directory"
    );
    Ok(())
}
fn names(dir: &File, maximum: usize) -> Result<Vec<String>> {
    let mut names = fs::read_dir(fs_at::proc_path(dir))?
        .take(maximum + 1)
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("invalid built-in resource name"))
        })
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(names.len() <= maximum, "too many built-in resource entries");
    names.sort();
    Ok(names)
}
fn sealed_tree(root: &File, id: &str) -> Result<File> {
    let tree = fs_at::open_at(root, id, true)?;
    sealed_directory(&tree)?;
    anyhow::ensure!(
        names(&tree, 6)?
            == [
                "LICENSE",
                "builtin-locale-preflight.mjs",
                "builtin-locale-sdk-preflight.mjs",
                "lib",
                "overlay.yml",
                "package.json"
            ],
        "unexpected built-in resource entries"
    );
    let lib = fs_at::open_at(&tree, "lib", true)?;
    sealed_directory(&lib)?;
    anyhow::ensure!(
        names(&lib, 2)? == ["client.js", "index.js"],
        "unexpected built-in client entries"
    );
    Ok(tree)
}
fn verify(root: &File, id: &str) -> Result<()> {
    let tree = sealed_tree(root, id)?;
    for resource in builtin_locales::resources() {
        let file = fs_at::file_at(&tree, resource.path)?;
        let metadata = file.metadata()?;
        anyhow::ensure!(
            own(&metadata)
                && metadata.mode() & 0o222 == 0
                && metadata.len() == resource.bytes.len() as u64,
            "invalid built-in resource file"
        );
        let mut bytes = Vec::new();
        file.take(resource.bytes.len() as u64 + 1)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes == resource.bytes,
            "built-in resource bytes differ from embedded snapshot"
        );
    }
    Ok(())
}
/// Read-only recognition of a previously emitted private schema-one cache. A
/// content digest is local cache integrity, not a signature or permission to
/// execute old code. Callers may use it only with an exact owned unit template.
pub fn find_retained_overlay(
    cache_root: &Path,
    matches: impl Fn(&Path) -> bool,
) -> Result<Option<PathBuf>> {
    use sha2::{Digest, Sha256};
    anyhow::ensure!(
        cache_root.is_absolute()
            && !cache_root
                .components()
                .any(|c| matches!(c, Component::ParentDir)),
        "absolute retained cache root required"
    );
    crate::util::check_owned_ancestors(cache_root)?;
    let root = match fs_at::open_dir(cache_root) {
        Ok(root) => root,
        Err(error) if missing(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = root.metadata()?;
    anyhow::ensure!(
        own(&metadata) && metadata.mode() & 0o077 == 0,
        "private owned retained cache root required"
    );
    for id in names(&root, 128)? {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            continue;
        }
        let overlay = cache_root.join(&id).join("overlay.yml");
        if !matches(&overlay) {
            continue;
        }
        let tree = sealed_tree(&root, &id)?;
        let mut digest = Sha256::new();
        let mut total = 0u64;
        for resource in builtin_locales::resources() {
            let file = fs_at::file_at(&tree, resource.path)?;
            let metadata = file.metadata()?;
            anyhow::ensure!(
                metadata.is_file()
                    && own(&metadata)
                    && metadata.nlink() == 1
                    && metadata.mode() & 0o222 == 0
                    && metadata.len() <= 8 * 1024 * 1024,
                "unsafe retained resource file"
            );
            total = total
                .checked_add(metadata.len())
                .context("retained resource size overflow")?;
            anyhow::ensure!(
                total <= 16 * 1024 * 1024,
                "retained resource tree too large"
            );
            let mut bytes = Vec::new();
            file.take(metadata.len() + 1).read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() as u64 == metadata.len(),
                "retained resource changed while reading"
            );
            if resource.path == "package.json" {
                let package: serde_json::Value = serde_json::from_slice(&bytes)?;
                anyhow::ensure!(
                    package.get("name").and_then(|name| name.as_str())
                        == Some("@dsh-ocean/builtin-locales"),
                    "retained package is not an Ocean native resource"
                );
            }
            digest.update((resource.path.len() as u64).to_be_bytes());
            digest.update(resource.path.as_bytes());
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
        }
        let actual = digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        anyhow::ensure!(actual == id, "retained resource content identity differs");
        return Ok(Some(overlay));
    }
    Ok(None)
}
fn cleanup(root: &File, stage: &File, name: &str) {
    // Delete only the same staging inode we created, under the pinned root FD.
    // Never repair/chmod/remove a pre-existing content-addressed destination.
    let Ok(current) = fs_at::open_at(root, name, true) else {
        return;
    };
    let (Ok(expected), Ok(actual)) = (stage.metadata(), current.metadata()) else {
        return;
    };
    if expected.dev() != actual.dev() || expected.ino() != actual.ino() {
        return;
    }
    let path = fs_at::proc_path(root).join(name);
    assert!(
        path.is_absolute()
            && path.parent() == Some(fs_at::proc_path(root).as_path())
            && name.starts_with(".builtin-locales-")
            && !name.contains('/')
    );
    let _ = stage.set_permissions(Permissions::from_mode(0o700));
    if let Ok(lib) = fs_at::open_at(stage, "lib", true) {
        let _ = lib.set_permissions(Permissions::from_mode(0o700));
    }
    let _ = fs::remove_dir_all(path);
}
/// Reuse only an exact sealed tree. Conflicts are errors, never repaired or
/// overwritten. No state inside a runtime generation or DSH profile is changed.
/// This is not a security boundary against a concurrent process of the same UID.
pub fn materialize(cache_root: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        cache_root.is_absolute()
            && !cache_root
                .components()
                .any(|c| matches!(c, Component::ParentDir)),
        "absolute built-in cache path required"
    );
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(cache_root)?;
    let root = fs_at::open_dir(cache_root)?;
    let metadata = root.metadata()?;
    anyhow::ensure!(
        own(&metadata) && metadata.mode() & 0o077 == 0,
        "private owned built-in cache directory required"
    );
    let id = builtin_locales::bundle_id();
    match fs_at::open_at(&root, &id, true) {
        Ok(_) => {
            verify(&root, &id)?;
            return Ok(cache_root.join(id));
        }
        Err(error) if missing(&error) => {}
        Err(error) => return Err(error).context("open existing built-in resource tree"),
    }
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        ".builtin-locales-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    );
    let stage = fs_at::make_dir_at(&root, &name)?;
    let result = (|| -> Result<()> {
        let lib = fs_at::make_dir_at(&stage, "lib")?;
        for resource in builtin_locales::resources() {
            let mut file = fs_at::create_file_at(&stage, resource.path, false)?;
            file.write_all(resource.bytes)?;
            file.set_permissions(Permissions::from_mode(0o400))?;
            file.sync_all()?;
        }
        lib.set_permissions(Permissions::from_mode(0o500))?;
        lib.sync_all()?;
        stage.set_permissions(Permissions::from_mode(0o500))?;
        stage.sync_all()?;
        match fs_at::rename_at(&root, &name, &id, libc::RENAME_NOREPLACE) {
            Ok(()) => {
                root.sync_all()?;
                Ok(())
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists) =>
            {
                verify(&root, &id)
            }
            Err(error) => Err(error),
        }
    })();
    // After a successful rename the staging name is absent, so cleanup is a no-op.
    cleanup(&root, &stage, &name);
    result?;
    verify(&root, &id)?;
    Ok(cache_root.join(id))
}

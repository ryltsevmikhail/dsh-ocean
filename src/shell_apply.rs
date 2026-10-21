//! Activating a verified release artifact into a user-owned prefix.
//!
//! This is the "apply" half that the verifier deliberately does not do. It works
//! only on an already verified + staged local file, never downloads anything,
//! never touches a system path, never restarts a service, and keeps the previous
//! version so an explicit rollback is possible.
//!
//! Layout (all inside one private prefix):
//!
//! ```text
//! <prefix>/versions/<release>-<sequence>-<digest8>/dsh-ocean
//! <prefix>/current -> versions/<…>     (atomic symlink swap)
//! <prefix>/.previous -> versions/<…>   (previous activation, kept on disk)
//! ```
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::release_feed::{self, Verified};

const VERSIONS: &str = "versions";
const CURRENT: &str = "current";
const PREVIOUS: &str = ".previous";
const BINARY: &str = "dsh-ocean";

fn absolute(prefix: &Path) -> Result<PathBuf> {
    anyhow::ensure!(prefix.is_absolute(), "install prefix must be absolute");
    anyhow::ensure!(prefix != Path::new("/"), "refusing the filesystem root");
    Ok(prefix.to_path_buf())
}

/// Creates (or validates) the private install prefix.
fn ensure_prefix(prefix: &Path) -> Result<()> {
    match fs::symlink_metadata(prefix) {
        Ok(meta) => anyhow::ensure!(meta.is_dir(), "install prefix is not a directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(prefix)
            .context("create install prefix")?,
        Err(error) => return Err(error).context("inspect install prefix"),
    }
    let meta = fs::symlink_metadata(prefix)?;
    // SAFETY: geteuid takes no pointers and has no side effects.
    let uid = unsafe { libc::geteuid() };
    use std::os::unix::fs::MetadataExt;
    anyhow::ensure!(
        meta.uid() == uid && meta.mode() & 0o022 == 0,
        "install prefix must be owned and not writable by others"
    );
    Ok(())
}

fn link_target(prefix: &Path, name: &str) -> Result<Option<String>> {
    match fs::symlink_metadata(prefix.join(name)) {
        Ok(meta) => {
            anyhow::ensure!(
                meta.file_type().is_symlink(),
                "{name} must be a symlink, not a file or directory"
            );
            let target = fs::read_link(prefix.join(name)).context("read activation selector")?;
            let components: Vec<_> = target.iter().collect();
            anyhow::ensure!(
                components.len() == 2
                    && components[0] == VERSIONS
                    && components[1]
                        .to_str()
                        .is_some_and(|name| !name.is_empty() && !name.starts_with('.')),
                "invalid activation selector"
            );
            Ok(Some(target.to_string_lossy().into_owned()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("inspect activation selector"),
    }
}

fn set_selector(prefix: &Path, name: &str, target: &str) -> Result<()> {
    let scratch = prefix.join(format!(".{name}.new.{}", std::process::id()));
    let _ = fs::remove_file(&scratch);
    std::os::unix::fs::symlink(target, &scratch).context("stage activation selector")?;
    fs::rename(&scratch, prefix.join(name)).context("activate selector")?;
    // fsync the directory so the rename is durable before we report success.
    if let Ok(directory) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(prefix)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn copy_verified(source: &Path, destination: &Path, release: &Verified) -> Result<()> {
    release_feed::verify_artifact(source, release)?;
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(source)
        .context("open staged artifact")?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(destination)
        .context("create version file")?;
    std::io::copy(&mut input, &mut output).context("copy verified artifact")?;
    output.sync_all()?;
    output
        .set_permissions(fs::Permissions::from_mode(0o755))
        .context("mark activated artifact executable")?;
    // Re-verify the bytes that actually landed on disk.
    release_feed::verify_artifact(destination, release)?;
    Ok(())
}

/// Activates `staged` and keeps the previous version for rollback.
pub fn apply(staged: &Path, release: &Verified, prefix: &Path) -> Result<Value> {
    anyhow::ensure!(
        release.artifact().kind == release_feed::Kind::AppImage,
        "local activation currently supports AppImage only; tar.gz extraction is not implemented"
    );
    let prefix = absolute(prefix)?;
    ensure_prefix(&prefix)?;
    let manifest = release.manifest();
    let sequence = manifest.sequence;
    let versions = prefix.join(VERSIONS);
    match fs::symlink_metadata(&versions) {
        Ok(meta) => anyhow::ensure!(meta.is_dir(), "versions entry is not a directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(&versions)?
        }
        Err(error) => return Err(error).context("inspect versions directory"),
    }
    let digest8: String = release.manifest_sha256().chars().take(8).collect();
    let directory_name = format!("{}-{sequence}-{digest8}", manifest.release);
    let directory = versions.join(&directory_name);
    match fs::symlink_metadata(&directory) {
        Ok(_) => anyhow::ensure!(
            release.manifest_sha256().len() >= 8 && directory.is_dir(),
            "version directory exists but is not reusable"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(&directory)?
        }
        Err(error) => return Err(error).context("inspect version directory"),
    }
    let binary = directory.join(BINARY);
    if binary.exists() {
        // Idempotent re-apply: the bytes must still match the manifest.
        release_feed::verify_artifact(&binary, release)
            .context("existing version does not match the verified release")?;
    } else {
        let scratch = directory.join(format!(".{BINARY}.new"));
        let _ = fs::remove_file(&scratch);
        copy_verified(staged, &scratch, release)?;
        fs::rename(&scratch, &binary).context("publish version binary")?;
        if let Ok(handle) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(&directory)
        {
            let _ = handle.sync_all();
        }
    }
    let target = format!("{VERSIONS}/{directory_name}");
    let previous = link_target(&prefix, CURRENT)?;
    if previous.as_deref() != Some(target.as_str()) {
        if let Some(old) = previous.as_ref() {
            set_selector(&prefix, PREVIOUS, old)?;
        }
        set_selector(&prefix, CURRENT, &target)?;
    }
    Ok(json!({
        "prefix": prefix,
        "version": manifest.release,
        "sequence": sequence,
        "manifestSha256": release.manifest_sha256(),
        "activated": prefix.join(CURRENT).join(BINARY),
        "previous": previous,
        "retained": directory,
        "serviceRestarted": false,
        "warning": "The new file is activated on disk. No service was restarted; running processes keep the old executable until you restart them. The previous version is retained for --rollback-shell.",
    }))
}

/// Switches back to the previously activated version, if one is retained.
pub fn rollback(prefix: &Path) -> Result<Value> {
    let prefix = absolute(prefix)?;
    ensure_prefix(&prefix)?;
    let current =
        link_target(&prefix, CURRENT)?.context("no activated version to roll back from")?;
    let previous = link_target(&prefix, PREVIOUS)?.context("no previous activation is retained")?;
    anyhow::ensure!(
        previous != current,
        "previous activation equals the current version"
    );
    let directory = prefix.join(&previous);
    anyhow::ensure!(
        directory.is_dir() && directory.join(BINARY).is_file(),
        "retained previous version is missing"
    );
    set_selector(&prefix, CURRENT, &previous)?;
    set_selector(&prefix, PREVIOUS, &current)?;
    Ok(json!({
        "prefix": prefix,
        "activated": prefix.join(CURRENT).join(BINARY),
        "rolledBackFrom": current,
        "rolledBackTo": previous,
        "serviceRestarted": false,
        "warning": "The selector was switched back. Running processes keep the executable they already started; restart them yourself if needed.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_unsafe_prefixes_and_selectors() {
        assert!(absolute(Path::new("relative")).is_err());
        assert!(absolute(Path::new("/")).is_err());
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-shell-apply-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        ensure_prefix(&root).unwrap();
        assert!(link_target(&root, CURRENT).unwrap().is_none());
        fs::write(root.join(CURRENT), b"not a symlink").unwrap();
        assert!(link_target(&root, CURRENT).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}

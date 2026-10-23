//! Small versioned Ocean settings: bounded, owner-controlled, atomic replacement.
//! Schema/permission meaning belongs to the caller; never used for DSH profiles.
use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::os::{
    fd::AsRawFd,
    unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
fn parent(path: &Path, create: bool) -> Result<Option<File>> {
    anyhow::ensure!(
        path.is_absolute() && path.file_name().is_some(),
        "absolute native setting filename required"
    );
    let directory = path.parent().context("native setting parent")?;
    if create {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
    }
    let directory: PathBuf = directory.components().collect();
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory)
    {
        Ok(file) => file,
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("open native setting directory"),
    };
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no pointer arguments or memory preconditions.
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "unsafe native setting directory"
    );
    Ok(Some(file))
}
fn entry(parent: &File, name: &std::ffi::OsStr) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name)
}
fn read_at<T: DeserializeOwned>(
    parent: &File,
    name: &std::ffi::OsStr,
    validate: fn(&T) -> Result<()>,
) -> Result<Option<T>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(entry(parent, name))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("open native setting"),
    };
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no pointer arguments or memory preconditions.
    anyhow::ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0
            && metadata.len() <= 8192,
        "invalid native setting file"
    );
    let mut bytes = Vec::new();
    file.take(8193).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 8192, "native setting exceeds size limit");
    let document: T = serde_json::from_slice(&bytes).context("invalid native setting document")?;
    validate(&document)?;
    Ok(Some(document))
}
pub(crate) fn load<T: DeserializeOwned>(
    path: &Path,
    validate: fn(&T) -> Result<()>,
) -> Result<Option<T>> {
    match parent(path, false)? {
        Some(parent) => read_at(&parent, path.file_name().unwrap(), validate),
        None => Ok(None),
    }
}
pub(crate) fn save<T: DeserializeOwned + Serialize>(
    path: &Path,
    document: &T,
    validate: fn(&T) -> Result<()>,
) -> Result<()> {
    validate(document)?;
    let bytes = serde_json::to_vec(document)?;
    anyhow::ensure!(bytes.len() <= 8192, "native setting exceeds size limit");
    let parent = parent(path, true)?.context("native setting directory")?;
    let name = path.file_name().unwrap();
    read_at(&parent, name, validate)?;
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let temporary_name = format!(
        ".native-setting-{}-{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = entry(&parent, std::ffi::OsStr::new(&temporary_name));
    let destination = entry(&parent, name);
    let mut created = false;
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        created = true;
        file.write_all(&bytes)?;
        file.sync_all()?;
        read_at(&parent, name, validate)?;
        anyhow::ensure!(
            temporary.parent() == destination.parent()
                && temporary_name.starts_with(".native-setting-"),
            "invalid native setting publication path"
        );
        fs::rename(&temporary, &destination)?;
        created = false;
        Ok(())
    })();
    if created {
        assert!(
            temporary.is_absolute()
                && temporary.parent() == destination.parent()
                && temporary_name.starts_with(".native-setting-")
                && temporary_name.ends_with(".tmp")
        );
        let _ = fs::remove_file(&temporary);
    }
    result
}

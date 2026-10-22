//! Transaction-only managed-home rollback, including absence and plugin bytes.
//! Sensitive local snapshots are private, not manual exports or release inputs.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{symlink, DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
const MAX_INDEX: u64 = 64 * 1024 * 1024;
const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_TOTAL: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 200000;
type Inventory = BTreeMap<String, Entry>;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Entry {
    Directory {
        mode: u32,
        seconds: i64,
        nanos: i64,
    },
    File {
        mode: u32,
        seconds: i64,
        nanos: i64,
        size: u64,
        sha256: String,
    },
    Link {
        target: PathBuf,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema: u32,
    home: PathBuf,
    present: bool,
    inventory: Inventory,
}
pub struct Snapshot {
    path: PathBuf,
    home: PathBuf,
}
#[derive(Debug, Serialize)]
pub struct Restored {
    pub retained: Option<PathBuf>,
}
fn sync(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?
        .sync_all()
        .context("sync runtime snapshot directory")
}
fn normalized(path: &Path) -> Result<PathBuf> {
    crate::shell_apply::absolute(path)
}
fn owned(metadata: &fs::Metadata) -> Result<()> {
    anyhow::ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "runtime state must be owned by the current user"
    );
    Ok(())
}
fn link_inside(home: &Path, relative: &Path, target: &Path) -> Result<()> {
    let mut parts: Vec<_> = if target.is_absolute() {
        Vec::new()
    } else {
        relative
            .parent()
            .unwrap_or(Path::new(""))
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name.to_os_string()),
                _ => None,
            })
            .collect()
    };
    let target = if target.is_absolute() {
        target
            .strip_prefix(home)
            .context("runtime snapshot refuses links outside managed home")?
    } else {
        target
    };
    for part in target.components() {
        match part {
            Component::Normal(name) => parts.push(name.to_os_string()),
            Component::CurDir => (),
            Component::ParentDir => {
                anyhow::ensure!(
                    parts.pop().is_some(),
                    "runtime snapshot link escapes managed home"
                );
            }
            _ => anyhow::bail!("runtime snapshot link scope refused"),
        }
    }
    Ok(())
}
fn stamp(metadata: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        metadata.mode(),
    )
}
fn require_controlled_permissions(mode: u32) -> Result<()> {
    anyhow::ensure!(
        mode & 0o022 == 0,
        "runtime snapshot refuses group/other-writable existing data; stop all DSH writers and review profile permissions before retrying. Ocean does not change existing permissions; see the runtime snapshot eligibility guidance."
    );
    Ok(())
}
fn digest(
    path: &Path,
    mut output: Option<&mut File>,
    offline: &mut dyn FnMut() -> Result<()>,
) -> Result<(u64, String)> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let before = file.metadata()?;
    owned(&before)?;
    anyhow::ensure!(
        before.is_file() && before.nlink() == 1 && before.len() <= MAX_FILE,
        "runtime snapshot requires bounded single-linked regular files"
    );
    require_controlled_permissions(before.mode())?;
    let mut total = 0u64;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        offline()?;
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        total += size as u64;
        anyhow::ensure!(total <= MAX_FILE, "runtime snapshot file exceeds limit");
        hash.update(&buffer[..size]);
        if let Some(output) = &mut output {
            output.write_all(&buffer[..size])?;
        }
    }
    anyhow::ensure!(
        stamp(&before) == stamp(&file.metadata()?) && total == before.len(),
        "runtime state changed during snapshot copy"
    );
    Ok((total, crate::release_feed::hex(&hash.finalize())))
}
fn inventory(
    root: &Path,
    home: &Path,
    offline: &mut dyn FnMut() -> Result<()>,
) -> Result<Inventory> {
    let mut records = Inventory::new();
    let mut pending = vec![PathBuf::new()];
    let mut total = 0u64;
    let root_metadata = fs::symlink_metadata(root)?;
    owned(&root_metadata)?;
    anyhow::ensure!(
        root_metadata.is_dir(),
        "runtime home snapshot root is not a directory"
    );
    let device = root_metadata.dev();
    while let Some(relative) = pending.pop() {
        offline()?;
        let path = root.join(&relative);
        let metadata = fs::symlink_metadata(&path)?;
        owned(&metadata)?;
        anyhow::ensure!(
            metadata.dev() == device,
            "runtime snapshot refuses nested mount boundaries"
        );
        let name = relative
            .to_str()
            .context("runtime snapshot requires UTF-8 relative paths")?
            .to_owned();
        anyhow::ensure!(
            name.len() <= 4096
                && relative.components().count() <= 128
                && records.len() < MAX_ENTRIES,
            "runtime snapshot tree exceeds bounds"
        );
        let entry = if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            anyhow::ensure!(
                target.as_os_str().len() <= 4096,
                "runtime snapshot link exceeds limit"
            );
            link_inside(home, &relative, &target)?;
            Entry::Link { target }
        } else {
            anyhow::ensure!(
                metadata.mode() & 0o7000 == 0,
                "runtime snapshot refuses special permission bits"
            );
            let mode = metadata.mode() & 0o777;
            let seconds = metadata.mtime();
            let nanos = metadata.mtime_nsec();
            if metadata.is_dir() {
                require_controlled_permissions(mode)?;
                for child in fs::read_dir(&path)? {
                    pending.push(relative.join(child?.file_name()));
                }
                Entry::Directory {
                    mode,
                    seconds,
                    nanos,
                }
            } else {
                anyhow::ensure!(metadata.is_file(), "runtime snapshot refuses special files");
                let (size, sha256) = digest(&path, None, offline)?;
                total = total
                    .checked_add(size)
                    .context("runtime snapshot size overflow")?;
                anyhow::ensure!(total <= MAX_TOTAL, "runtime snapshot exceeds total limit");
                Entry::File {
                    mode,
                    seconds,
                    nanos,
                    size,
                    sha256,
                }
            }
        };
        records.insert(name, entry);
    }
    Ok(records)
}
fn set_attributes(path: &Path, mode: u32, seconds: i64, nanos: i64) -> Result<()> {
    use std::os::fd::AsRawFd;
    anyhow::ensure!(
        (0..1_000_000_000).contains(&nanos),
        "invalid snapshot timestamp"
    );
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    let times = [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: seconds,
            tv_nsec: nanos,
        },
    ];
    anyhow::ensure!(
        unsafe { libc::futimens(file.as_raw_fd(), times.as_ptr()) } == 0,
        "could not restore snapshot timestamp"
    );
    Ok(())
}
fn copy(
    root: &Path,
    destination: &Path,
    records: &Inventory,
    offline: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    for (relative, entry) in records {
        offline()?;
        let source = root.join(relative);
        let target = destination.join(relative);
        match entry {
            Entry::Directory { .. } => {
                if !relative.is_empty() {
                    fs::DirBuilder::new().mode(0o700).create(&target)?;
                }
            }
            Entry::Link { target: link } => {
                anyhow::ensure!(
                    fs::read_link(&source)? == *link,
                    "runtime snapshot link changed"
                );
                symlink(link, &target)?;
            }
            Entry::File {
                mode,
                seconds,
                nanos,
                size,
                sha256,
            } => {
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(&target)?;
                let (copied, hash) = digest(&source, Some(&mut output), offline)?;
                anyhow::ensure!(
                    copied == *size && hash == *sha256,
                    "runtime snapshot file integrity mismatch"
                );
                set_attributes(&target, *mode, *seconds, *nanos)?;
                output.sync_all()?;
            }
        }
    }
    for (relative, entry) in records.iter().rev() {
        if let Entry::Directory {
            mode,
            seconds,
            nanos,
        } = entry
        {
            let target = destination.join(relative);
            set_attributes(&target, *mode, *seconds, *nanos)?;
            sync(&target)?;
        }
    }
    Ok(())
}
impl Snapshot {
    pub fn path(&self) -> &Path {
        &self.path
    }
    fn manifest(&self) -> Result<Manifest> {
        anyhow::ensure!(
            !self.path.starts_with(&self.home) && !self.home.starts_with(&self.path),
            "transaction snapshot must remain outside managed home"
        );
        crate::util::check_owned_ancestors(&self.path)?;
        let directory = fs::symlink_metadata(&self.path)?;
        owned(&directory)?;
        anyhow::ensure!(
            directory.is_dir() && directory.mode() & 0o077 == 0,
            "runtime snapshot directory must remain private"
        );
        let metadata = fs::symlink_metadata(self.path.join("snapshot.json"))?;
        owned(&metadata)?;
        anyhow::ensure!(
            metadata.is_file() && metadata.nlink() == 1 && metadata.mode() & 0o077 == 0,
            "runtime snapshot manifest must be private"
        );
        let bytes = crate::release_feed::read_input(&self.path.join("snapshot.json"), MAX_INDEX)?;
        let manifest: Manifest =
            serde_json::from_slice(&bytes).context("invalid runtime transaction snapshot")?;
        anyhow::ensure!(
            manifest.schema == 1 && manifest.home == self.home,
            "runtime snapshot home/scope mismatch"
        );
        Ok(manifest)
    }
    pub fn verify(&self) -> Result<()> {
        let manifest = self.manifest()?;
        if manifest.present {
            anyhow::ensure!(
                inventory(&self.path.join("data"), &self.home, &mut || Ok(()))?
                    == manifest.inventory,
                "runtime snapshot inventory mismatch"
            );
        } else {
            anyhow::ensure!(
                manifest.inventory.is_empty()
                    && fs::symlink_metadata(self.path.join("data"))
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
                "absent-home snapshot contains unexpected data"
            );
        }
        Ok(())
    }
    pub fn restore(&self, mut offline: impl FnMut() -> Result<()>) -> Result<Restored> {
        offline()?;
        self.verify()?;
        let manifest = self.manifest()?;
        let parent = self.home.parent().context("runtime home has no parent")?;
        crate::util::check_owned_ancestors(parent)?;
        let current = match fs::symlink_metadata(&self.home) {
            Ok(metadata) => {
                owned(&metadata)?;
                Some(metadata)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("inspect current runtime home"),
        };
        let id = crate::downloads::unique_id();
        let retained = parent.join(format!(".ocean-runtime-retained-{id}"));
        if !manifest.present {
            offline()?;
            if current.is_some() {
                let guard = crate::checked_archive::NewDirectory::create(&retained)?;
                crate::shell_apply::publish_new(&self.home, &retained.join("data"))?;
                guard.preserve();
                sync(&retained)?;
                sync(parent)?;
                return Ok(Restored {
                    retained: Some(retained.join("data")),
                });
            }
            return Ok(Restored { retained: None });
        }
        let stage = parent.join(format!(".ocean-runtime-restore-{id}"));
        let guard = crate::checked_archive::NewDirectory::create(&stage)?;
        let data = stage.join("data");
        fs::DirBuilder::new().mode(0o700).create(&data)?;
        copy(
            &self.path.join("data"),
            &data,
            &manifest.inventory,
            &mut offline,
        )?;
        anyhow::ensure!(
            inventory(&data, &self.home, &mut offline)? == manifest.inventory,
            "restored runtime home verification failed"
        );
        offline()?;
        guard.check()?;
        if current.is_some() {
            use std::os::fd::AsRawFd;
            let from = crate::profile_backup::open_dir(&stage)?;
            let to = crate::profile_backup::open_dir(parent)?;
            let name = std::ffi::CString::new(
                self.home
                    .file_name()
                    .context("runtime home name missing")?
                    .as_encoded_bytes(),
            )?;
            let result = unsafe {
                libc::renameat2(
                    from.as_raw_fd(),
                    c"data".as_ptr(),
                    to.as_raw_fd(),
                    name.as_ptr(),
                    libc::RENAME_EXCHANGE,
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("exchange restored runtime home");
            }
            // Once the container holds live candidate data it must never be
            // auto-cleaned, even if publication/fsync fails. Keep it private.
            guard.preserve();
            sync(&stage)?;
            crate::shell_apply::publish_new(&stage, &retained).with_context(|| {
                format!(
                    "home restored; candidate state retained at {} requires recovery",
                    stage.display()
                )
            })?;
            sync(parent)?;
            Ok(Restored {
                retained: Some(retained.join("data")),
            })
        } else {
            crate::shell_apply::publish_new(&data, &self.home)?;
            sync(parent)?;
            Ok(Restored { retained: None })
        }
    }
}
pub fn capture(
    home: &Path,
    backups: &Path,
    mut offline: impl FnMut() -> Result<()>,
) -> Result<Snapshot> {
    offline()?;
    let home = normalized(home)?;
    let backups = normalized(backups)?;
    crate::util::check_owned_ancestors(home.parent().context("runtime home parent missing")?)?;
    crate::util::check_owned_ancestors(&backups)?;
    anyhow::ensure!(
        !backups.starts_with(&home) && !home.starts_with(&backups),
        "runtime snapshots must be outside managed home"
    );
    let present = match fs::symlink_metadata(&home) {
        Ok(metadata) => {
            owned(&metadata)?;
            anyhow::ensure!(metadata.is_dir(), "runtime home is not an owned directory");
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("inspect runtime home"),
    };
    let records = if present {
        inventory(&home, &home, &mut offline)?
    } else {
        Inventory::new()
    };
    crate::shell_apply::ensure_prefix(&backups)?;
    let _lock = crate::profile_backup::operation_lock(&backups.join(".runtime-snapshot.lock"))?;
    let id = crate::downloads::unique_id();
    let temporary = backups.join(format!(".new-runtime-snapshot-{id}"));
    let guard = crate::checked_archive::NewDirectory::create(&temporary)?;
    if present {
        let data = temporary.join("data");
        fs::DirBuilder::new().mode(0o700).create(&data)?;
        copy(&home, &data, &records, &mut offline)?;
        anyhow::ensure!(
            inventory(&home, &home, &mut offline)? == records,
            "managed home changed during transaction snapshot"
        );
    }
    let manifest = Manifest {
        schema: 1,
        home: home.clone(),
        present,
        inventory: records,
    };
    let bytes = serde_json::to_vec(&manifest)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_INDEX,
        "runtime snapshot inventory exceeds limit"
    );
    crate::shell_apply::write_json_new(&temporary.join("snapshot.json"), &manifest)?;
    let staged = Snapshot {
        path: temporary.clone(),
        home: home.clone(),
    };
    staged.verify()?;
    offline()?;
    if present {
        anyhow::ensure!(
            inventory(&home, &home, &mut offline)? == manifest.inventory,
            "managed home changed before snapshot publication"
        );
    } else {
        anyhow::ensure!(
            fs::symlink_metadata(&home)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "managed home appeared during absent-home snapshot"
        );
    }
    guard.check()?;
    let path = backups.join(format!("runtime-snapshot-{id}"));
    crate::shell_apply::publish_new(&temporary, &path)?;
    sync(&backups)?;
    Ok(Snapshot { path, home })
}
pub fn open(home: &Path, snapshot: &Path) -> Result<Snapshot> {
    let result = Snapshot {
        home: normalized(home)?,
        path: normalized(snapshot)?,
    };
    result.verify()?;
    Ok(result)
}

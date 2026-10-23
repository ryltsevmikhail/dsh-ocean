//! Ocean-only language preference. No DSH profile or runtime is touched here.
use crate::i18n::Language;
use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preference {
    #[default]
    System,
    Language(Language),
}
impl Preference {
    pub fn parse(code: &str) -> Option<Self> {
        match code {
            "system" => Some(Self::System),
            "en" => Some(Self::Language(Language::En)),
            "ru" => Some(Self::Language(Language::Ru)),
            "es" => Some(Self::Language(Language::Es)),
            "pt-BR" => Some(Self::Language(Language::PtBr)),
            "de" => Some(Self::Language(Language::De)),
            "fr" => Some(Self::Language(Language::Fr)),
            "ja" => Some(Self::Language(Language::Ja)),
            "ko" => Some(Self::Language(Language::Ko)),
            _ => None,
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Language(language) => language.code(),
        }
    }
}
impl Serialize for Preference {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}
impl<'de> Deserialize<'de> for Preference {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Self::parse(&code).ok_or_else(|| serde::de::Error::custom("unsupported Ocean language"))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    language: Preference,
}

fn open_parent(path: &Path, create: bool) -> Result<Option<File>> {
    anyhow::ensure!(
        path.is_absolute() && path.file_name().is_some(),
        "absolute preference filename required"
    );
    let parent = path.parent().context("language preference parent")?;
    if create {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let parent: PathBuf = parent.components().collect();
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&parent)
    {
        Ok(file) => file,
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("open language preference directory"),
    };
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no pointer arguments or memory preconditions.
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "unsafe language preference directory"
    );
    Ok(Some(file))
}
fn entry(parent: &File, name: &std::ffi::OsStr) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name)
}
fn load_at(parent: &File, name: &std::ffi::OsStr) -> Result<Preference> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(entry(parent, name))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preference::System),
        Err(e) => return Err(e).context("open language preference"),
    };
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no pointer arguments or memory preconditions.
    anyhow::ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0
            && metadata.len() <= 8192,
        "invalid language preference file"
    );
    let mut bytes = Vec::new();
    file.take(8193).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 8192,
        "language preference exceeds size limit"
    );
    let document: Document =
        serde_json::from_slice(&bytes).context("invalid language preference")?;
    anyhow::ensure!(
        document.version == 1,
        "unsupported language preference version"
    );
    Ok(document.language)
}
/// Missing preference means system; malformed/unsafe/future documents are errors.
pub fn load(path: &Path) -> Result<Preference> {
    match open_parent(path, false)? {
        Some(parent) => load_at(&parent, path.file_name().unwrap()),
        None => Ok(Preference::System),
    }
}
/// Atomic replacement in a pinned, owner-controlled directory. The preference is
/// not a security boundary against another concurrent process of the same UID.
pub fn save(path: &Path, preference: Preference) -> Result<()> {
    let parent = open_parent(path, true)?.context("language preference directory")?;
    let name = path.file_name().unwrap();
    load_at(&parent, name)?; // Never silently replace a malformed/link/future file.
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let temporary_name = format!(
        ".localization-{}-{}.tmp",
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
        file.write_all(&serde_json::to_vec(&Document {
            version: 1,
            language: preference,
        })?)?;
        file.sync_all()?;
        load_at(&parent, name)?;
        anyhow::ensure!(
            temporary.parent() == destination.parent()
                && temporary_name.starts_with(".localization-"),
            "invalid language preference publication path"
        );
        fs::rename(&temporary, &destination)?;
        created = false;
        Ok(())
    })();
    if created {
        assert!(
            temporary.parent() == destination.parent()
                && temporary_name.starts_with(".localization-")
                && temporary_name.ends_with(".tmp")
        );
        let _ = fs::remove_file(&temporary);
    }
    result
}

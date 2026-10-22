//! Operator-provisioned updater trust and channels, separate from Harness and
//! npm/core configuration. No network, implicit trust or private-key handling.
use crate::release_feed::{Channel, PublicKeyDocument, TrustedKey};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_SETTINGS: u64 = 16384;
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Product {
    Shell,
    Runtime,
}
impl Product {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "shell" => Ok(Self::Shell),
            "runtime" => Ok(Self::Runtime),
            _ => anyhow::bail!("update product must be shell or runtime"),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Feed {
    pub manifest_url: String,
    pub signature_url: String,
    pub minimum_sequence: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub key: PublicKeyDocument,
    pub stable: Feed,
    pub next: Feed,
}
impl Source {
    pub fn trust(&self) -> Result<TrustedKey> {
        TrustedKey::from_json(&serde_json::to_vec(&self.key)?)
    }
    pub fn feed(&self, channel: Channel) -> &Feed {
        match channel {
            Channel::Stable => &self.stable,
            Channel::Next => &self.next,
        }
    }
    fn validate(&self) -> Result<()> {
        self.trust()?;
        for feed in [&self.stable, &self.next] {
            validate_feed_url(&feed.manifest_url)?;
            validate_feed_url(&feed.signature_url)?;
            anyhow::ensure!(
                feed.minimum_sequence > 0,
                "feed sequence floor must be positive"
            );
            anyhow::ensure!(
                feed.manifest_url != feed.signature_url,
                "manifest and signature locations must differ"
            );
        }
        Ok(())
    }
    fn summary(&self) -> Result<Value> {
        let trust = self.trust()?;
        Ok(
            json!({"keyId":trust.id(),"keyFingerprint":trust.fingerprint(),"stable":self.stable,"next":self.next}),
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub schema: u32,
    pub shell_channel: Channel,
    pub runtime_channel: Channel,
    pub shell: Option<Source>,
    pub runtime: Option<Source>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: 1,
            shell_channel: Channel::Stable,
            runtime_channel: Channel::Stable,
            shell: None,
            runtime: None,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.schema == 1, "unsupported update settings schema");
        for source in [&self.shell, &self.runtime].into_iter().flatten() {
            source.validate()?;
        }
        anyhow::ensure!(
            serde_json::to_vec(self)?.len() as u64 <= MAX_SETTINGS,
            "update settings exceed limit"
        );
        Ok(())
    }
    pub fn source(&self, product: Product) -> Result<&Source> {
        match product {
            Product::Shell => self.shell.as_ref(),
            Product::Runtime => self.runtime.as_ref(),
        }
        .context("signed update source is not configured")
    }
    pub fn channel(&self, product: Product) -> Channel {
        match product {
            Product::Shell => self.shell_channel,
            Product::Runtime => self.runtime_channel,
        }
    }
    pub fn digest(&self) -> Result<String> {
        Ok(crate::release_feed::hex(&Sha256::digest(
            serde_json::to_vec(self)?,
        )))
    }
    pub fn summary(&self) -> Result<Value> {
        Ok(
            json!({"schema":self.schema,"shellConfigured":self.shell.is_some(),"runtimeConfigured":self.runtime.is_some(),"shellChannel":self.shell_channel,"runtimeChannel":self.runtime_channel,"shell":self.shell.as_ref().map(Source::summary).transpose()?,"runtime":self.runtime.as_ref().map(Source::summary).transpose()?,"trust":"operator-confirmed-public-key; no key is fetched from a feed"}),
        )
    }
}

pub fn validate_feed_url(value: &str) -> Result<()> {
    anyhow::ensure!(value.len() <= 2048, "feed URL exceeds limit");
    let url = url::Url::parse(value).map_err(|_| anyhow::anyhow!("invalid feed URL"))?;
    anyhow::ensure!(
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.as_str() == value,
        "feed must be a canonical public GitHub HTTPS asset URL"
    );
    let tail = url
        .path()
        .strip_prefix("/ryltsevmikhail/dsh-ocean/releases/")
        .context("feed must belong to the configured Ocean repository")?;
    let parts: Vec<_> = tail.split('/').collect();
    let segment = |s: &str| {
        !s.is_empty()
            && s.len() <= 128
            && s.as_bytes()[0].is_ascii_alphanumeric()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    anyhow::ensure!(
        parts.len() == 3
            && segment(parts[2])
            && ((parts[0] == "download" && segment(parts[1]))
                || (parts[0] == "latest" && parts[1] == "download")),
        "unsupported feed asset location"
    );
    Ok(())
}

pub fn path() -> PathBuf {
    crate::config::config_dir().join("updates.json")
}
fn checked_parents(path: &Path) -> Result<()> {
    let uid = unsafe { libc::geteuid() };
    for parent in path.ancestors() {
        match fs::symlink_metadata(parent) {
            Ok(m) => anyhow::ensure!(
                m.is_dir()
                    && (m.uid() == 0 || m.uid() == uid)
                    && (m.mode() & 0o022 == 0 || (m.uid() == 0 && m.mode() & libc::S_ISVTX != 0)),
                "unsafe update settings directory"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e).context("inspect update settings directory"),
        }
    }
    Ok(())
}
pub fn read_candidate(path: &Path) -> Result<Settings> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let path = absolute.as_path();
    checked_parents(path.parent().context("update settings have no parent")?)?;
    let m = fs::symlink_metadata(path).context("read updater configuration")?;
    anyhow::ensure!(
        m.is_file()
            && (m.uid() == unsafe { libc::geteuid() } || m.uid() == 0)
            && m.mode() & 0o022 == 0,
        "update settings must be owned regular non-writable-by-others files"
    );
    let bytes = crate::release_feed::read_input(path, MAX_SETTINGS)?;
    let settings: Settings =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid update settings"))?;
    settings.validate()?;
    Ok(settings)
}
pub fn load() -> Result<Settings> {
    let file = path();
    checked_parents(file.parent().context("update settings have no parent")?)?;
    match fs::symlink_metadata(&file) {
        Ok(_) => read_candidate(&file),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e).context("inspect update settings"),
    }
}

/// Lock order for configured updates: settings -> product floor -> prefix.
/// Check/status calls use load() and remain read-only; mutation uses this guard.
pub struct Guard {
    _lock: File,
    settings: Settings,
}
impl Guard {
    pub fn begin() -> Result<Self> {
        let file = path();
        let directory = file.parent().context("update settings have no directory")?;
        checked_parents(directory)?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        checked_parents(directory)?;
        let m = fs::symlink_metadata(directory)?;
        anyhow::ensure!(
            m.uid() == unsafe { libc::geteuid() },
            "updater configuration directory is not owned by this user"
        );
        let lock = crate::profile_backup::operation_lock(&directory.join(".updates.lock"))?;
        Ok(Self {
            _lock: lock,
            settings: load()?,
        })
    }
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    fn save(&mut self, settings: Settings) -> Result<()> {
        settings.validate()?;
        let file = path();
        let directory = file.parent().context("missing settings directory")?;
        let temporary = directory.join(format!(
            ".updates.new.{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        let result = (|| -> Result<()> {
            output.write_all(&serde_json::to_vec_pretty(&settings)?)?;
            output.sync_all()?;
            fs::rename(&temporary, &file)?;
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
                .open(directory)?
                .sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        self.settings = settings;
        Ok(())
    }
}
#[derive(Default)]
pub struct Confirmation {
    pub shell: Option<String>,
    pub runtime: Option<String>,
    pub replace: bool,
}
pub fn configure(candidate: Settings, confirmation: &Confirmation) -> Result<Settings> {
    candidate.validate()?;
    // Validate independent fingerprints BEFORE creating config/lock files.
    for (source, confirmed) in [
        (&candidate.shell, &confirmation.shell),
        (&candidate.runtime, &confirmation.runtime),
    ] {
        match (source, confirmed) {
            (Some(source), Some(fp)) => anyhow::ensure!(
                source.trust()?.fingerprint() == *fp,
                "public-key fingerprint confirmation does not match"
            ),
            (None, None) => (),
            _ => anyhow::bail!(
                "each configured trust root requires its independent fingerprint confirmation"
            ),
        }
    }
    let mut guard = Guard::begin()?;
    for (old, new) in [
        (&guard.settings.shell, &candidate.shell),
        (&guard.settings.runtime, &candidate.runtime),
    ] {
        if let (Some(old), Some(new)) = (old, new) {
            anyhow::ensure!(
                old.trust()?.fingerprint() == new.trust()?.fingerprint() || confirmation.replace,
                "replacing updater trust requires --confirm-replace-trust yes"
            );
        }
    }
    guard.save(candidate.clone())?;
    Ok(candidate)
}
pub fn set_channel(product: Product, channel: Channel) -> Result<Settings> {
    let mut guard = Guard::begin()?;
    let mut updated = guard.settings.clone();
    match product {
        Product::Shell => updated.shell_channel = channel,
        Product::Runtime => updated.runtime_channel = channel,
    }
    guard.save(updated.clone())?;
    Ok(updated)
}

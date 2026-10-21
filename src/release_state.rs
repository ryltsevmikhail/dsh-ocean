//! Persisted anti-rollback / freeze state for verified release manifests.
//!
//! The verifier itself stays stateless: callers pass an explicit sequence floor.
//! This module remembers what was already **accepted** so a later unsigned or
//! replay attempt cannot silently downgrade the client back to an older signed
//! manifest, and two different manifests cannot claim the same sequence.
//!
//! It is a local defence against replay/downgrade of *signed* manifests. It is
//! not a signature, not a trusted clock, and a same-user attacker can delete or
//! edit the file; the point is to make silent regressions require an explicit,
//! visible operator action.
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::config;

pub const SCHEMA: u32 = 1;
pub const MAX_STATE: u64 = 64 * 1024;
const LOCK: &str = ".release-state.lock";
const FILE: &str = "release-state.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub sequence: u64,
    pub release: String,
    /// SHA-256 of the exact authenticated manifest bytes.
    pub manifest_sha256: String,
    pub recorded_at: String,
}

#[derive(Serialize, Deserialize)]
struct Document {
    schema: u32,
    #[serde(default)]
    channels: BTreeMap<String, Entry>,
}

pub fn path() -> PathBuf {
    config::state_dir().join(FILE)
}

fn valid_channel(channel: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(channel, "stable" | "next"),
        "unsupported release channel"
    );
    Ok(())
}

fn valid_digest(digest: &str) -> Result<()> {
    anyhow::ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid manifest digest"
    );
    Ok(())
}

fn decode_document(bytes: &[u8]) -> Result<BTreeMap<String, Entry>> {
    let document: Document = serde_json::from_slice(bytes).context("invalid release state file")?;
    anyhow::ensure!(
        document.schema == SCHEMA,
        "unsupported release state schema"
    );
    for (channel, entry) in &document.channels {
        valid_channel(channel).with_context(|| format!("stored channel {channel} is invalid"))?;
        anyhow::ensure!(entry.sequence > 0, "stored sequence is invalid");
        valid_digest(&entry.manifest_sha256)?;
        Version::parse(&entry.release).map_err(|_| anyhow!("stored release version is invalid"))?;
    }
    Ok(document.channels)
}

fn read_at(path: &Path) -> Result<BTreeMap<String, Entry>> {
    // Inspect the opened descriptor, but never wait for a FIFO writer first.
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error).context("open release state"),
    };
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= MAX_STATE,
        "release state must be a bounded regular file"
    );
    let mut bytes = Vec::new();
    (&mut file).take(MAX_STATE + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_STATE,
        "release state grew while reading"
    );
    decode_document(&bytes)
}

/// Reads the remembered state without creating or modifying anything.
pub fn read() -> Result<BTreeMap<String, Entry>> {
    read_at(&path())
}

/// Decides whether `(channel, sequence, release, digest)` may be accepted given
/// the remembered state. Returns the entry that would be stored.
pub fn check(
    existing: &BTreeMap<String, Entry>,
    channel: &str,
    sequence: u64,
    release: &str,
    manifest_sha256: &str,
) -> Result<Entry> {
    valid_channel(channel)?;
    anyhow::ensure!(sequence > 0, "release sequence must be positive");
    valid_digest(manifest_sha256)?;
    let parsed = Version::parse(release).map_err(|_| anyhow!("invalid release version"))?;
    if let Some(previous) = existing.get(channel) {
        anyhow::ensure!(
            sequence >= previous.sequence,
            "refusing release rollback: sequence {sequence} is older than the remembered {}",
            previous.sequence
        );
        if sequence == previous.sequence {
            anyhow::ensure!(
                manifest_sha256 == previous.manifest_sha256,
                "refusing a different manifest for already accepted sequence {sequence}"
            );
        } else {
            let remembered = Version::parse(&previous.release)
                .map_err(|_| anyhow!("stored release version is invalid"))?;
            anyhow::ensure!(
                parsed >= remembered,
                "refusing release downgrade: {release} is older than the remembered {}; reset the state explicitly if the publisher intends this",
                previous.release
            );
        }
    }
    Ok(Entry {
        sequence,
        release: release.to_owned(),
        manifest_sha256: manifest_sha256.to_owned(),
        recorded_at: Utc::now().to_rfc3339(),
    })
}

fn write_at(path: &Path, channels: &BTreeMap<String, Entry>) -> Result<()> {
    let directory = path.parent().context("release state has no directory")?;
    let body = serde_json::to_vec_pretty(&Document {
        schema: SCHEMA,
        channels: channels.clone(),
    })?;
    anyhow::ensure!(
        body.len() as u64 <= MAX_STATE,
        "release state grew beyond its bound"
    );
    let temporary = directory.join(format!(
        ".release-state.tmp.{}.{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)
            .context("create release state scratch")?;
        file.write_all(&body)?;
        file.sync_all()?;
        // Replaces only this file's own previous content; the directory is
        // private and app-owned.
        fs::rename(&temporary, path).context("commit release state")?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(directory)?;
        directory.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Hold the global sequence lock across checking, activation and recording.
/// Prefix operations acquire their own lock only AFTER this guard (one order).
pub struct Transaction {
    _lock: std::fs::File,
    path: PathBuf,
    existing: BTreeMap<String, Entry>,
}

impl Transaction {
    pub fn begin() -> Result<Self> {
        let path = path();
        let directory = path.parent().context("release state has no directory")?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .context("create private state directory")?;
        let lock = crate::profile_backup::operation_lock(&directory.join(LOCK))
            .context("another release state update is in progress")?;
        let existing = read_at(&path)?;
        Ok(Self {
            _lock: lock,
            path,
            existing,
        })
    }

    pub fn check(
        &self,
        channel: &str,
        sequence: u64,
        release: &str,
        digest: &str,
    ) -> Result<Entry> {
        check(&self.existing, channel, sequence, release, digest)
    }

    pub fn record(
        &mut self,
        channel: &str,
        sequence: u64,
        release: &str,
        digest: &str,
    ) -> Result<Entry> {
        let entry = self.check(channel, sequence, release, digest)?;
        let mut updated = self.existing.clone();
        updated.insert(channel.to_owned(), entry.clone());
        write_at(&self.path, &updated)?;
        self.existing = updated;
        Ok(entry)
    }
}

/// Record under the same guard used by activation, acceptance and reset.
pub fn record(channel: &str, sequence: u64, release: &str, manifest_sha256: &str) -> Result<Entry> {
    Transaction::begin()?.record(channel, sequence, release, manifest_sha256)
}

/// Removes the remembered state. Deliberately operator-only and explicit.
pub fn reset() -> Result<bool> {
    let path = path();
    let directory = path.parent().context("release state has no directory")?;
    if !directory.exists() {
        return Ok(false);
    }
    let _lock = crate::profile_backup::operation_lock(&directory.join(LOCK))
        .context("another release state update is in progress")?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("remove release state"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(seed: u8) -> String {
        format!("{seed:02x}").repeat(32)
    }

    #[test]
    fn accepts_newer_and_identical_sequences_and_refuses_regressions() {
        let mut state = BTreeMap::new();
        let entry = check(&state, "stable", 5, "0.2.0", &digest(1)).unwrap();
        assert_eq!(entry.sequence, 5);
        state.insert("stable".into(), entry);
        // Identical sequence + identical digest is idempotent.
        assert!(check(&state, "stable", 5, "0.2.0", &digest(1)).is_ok());
        // Same sequence, different manifest is refused.
        assert!(check(&state, "stable", 5, "0.2.0", &digest(2)).is_err());
        // Older sequence is refused even with a newer version.
        assert!(check(&state, "stable", 4, "0.3.0", &digest(3)).is_err());
        // Higher sequence but older release is a downgrade.
        assert!(check(&state, "stable", 6, "0.1.0", &digest(4)).is_err());
        // Higher sequence and newer release is fine.
        assert!(check(&state, "stable", 6, "0.3.0", &digest(5)).is_ok());
        // Channels are independent.
        assert!(check(&state, "next", 1, "0.2.0", &digest(6)).is_ok());
    }

    #[test]
    fn rejects_invalid_inputs() {
        let state = BTreeMap::new();
        assert!(check(&state, "beta", 1, "0.2.0", &digest(1)).is_err());
        assert!(check(&state, "stable", 0, "0.2.0", &digest(1)).is_err());
        assert!(check(&state, "stable", 1, "not-a-version", &digest(1)).is_err());
        assert!(check(&state, "stable", 1, "0.2.0", "ABCDEF").is_err());
    }

    #[test]
    fn document_roundtrip_is_bounded_and_schema_checked() {
        let mut channels = BTreeMap::new();
        channels.insert(
            "stable".to_owned(),
            Entry {
                sequence: 3,
                release: "0.2.0".to_owned(),
                manifest_sha256: digest(9),
                recorded_at: "2023-11-14T22:13:20+00:00".to_owned(),
            },
        );
        let body = serde_json::to_vec(&Document {
            schema: SCHEMA,
            channels: channels.clone(),
        })
        .unwrap();
        assert_eq!(decode_document(&body).unwrap(), channels);
        let wrong = serde_json::to_vec(&Document {
            schema: 99,
            channels: channels.clone(),
        })
        .unwrap();
        assert!(decode_document(&wrong).is_err());
        let mut bad = channels.clone();
        bad.get_mut("stable").unwrap().manifest_sha256 = "xyz".into();
        assert!(decode_document(
            &serde_json::to_vec(&Document {
                schema: SCHEMA,
                channels: bad
            })
            .unwrap()
        )
        .is_err());
    }
}

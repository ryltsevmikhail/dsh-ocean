//! Offline verification and explicit local staging. No download/install, key discovery, service
//! operation or persistent rollback-floor mutation occurs in this module.
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub const DOMAIN: &[u8] = b"DSH Ocean release manifest v1\n";
pub const MAX_MANIFEST: u64 = 65536;
pub const MAX_AUXILIARY: u64 = 4096;
pub const MAX_ASSET: u64 = 1024 * 1024 * 1024;
const REPOSITORY: &str = "ryltsevmikhail/dsh-ocean";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Next,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    #[serde(rename = "linux-x86_64")]
    LinuxX86_64,
    #[serde(rename = "linux-aarch64")]
    LinuxAarch64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    #[serde(rename = "appimage")]
    AppImage,
    #[serde(rename = "tar.gz")]
    TarGz,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Artifact {
    pub target: Target,
    pub kind: Kind,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub product: String,
    pub channel: Channel,
    pub sequence: u64,
    pub release: String,
    pub published_at: String,
    pub expires_at: String,
    pub artifacts: Vec<Artifact>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Detached {
    version: u32,
    key_id: String,
    signature_hex: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KeyFile {
    version: u32,
    key_id: String,
    public_key_hex: String,
}
pub struct TrustedKey {
    id: String,
    key: VerifyingKey,
}
impl TrustedKey {
    /// This file must be provisioned independently of the feed. Parsing it does
    /// not establish publisher trust; that is the operator/caller's decision.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_AUXILIARY,
            "trusted key file exceeds limit"
        );
        let value: KeyFile =
            serde_json::from_slice(bytes).map_err(|_| anyhow!("invalid trusted key file"))?;
        anyhow::ensure!(
            value.version == 1 && valid_id(&value.key_id),
            "unsupported trusted key identity"
        );
        let key = VerifyingKey::from_bytes(&decode::<32>(&value.public_key_hex)?)
            .map_err(|_| anyhow!("invalid Ed25519 public key"))?;
        anyhow::ensure!(!key.is_weak(), "weak Ed25519 public key refused");
        Ok(Self {
            id: value.key_id,
            key,
        })
    }
}
pub struct Policy {
    pub channel: Channel,
    pub target: Target,
    pub kind: Kind,
    pub minimum_sequence: u64,
    pub current_release: Version,
    pub now: DateTime<Utc>,
}
pub struct Verified {
    manifest: Manifest,
    artifact: Artifact,
    digest: String,
    key_id: String,
}
impl Verified {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }
    pub fn manifest_sha256(&self) -> &str {
        &self.digest
    }
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}
fn decode<const N: usize>(value: &str) -> Result<[u8; N]> {
    anyhow::ensure!(
        value.len() == N * 2
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "invalid lowercase hex encoding"
    );
    let mut result = [0u8; N];
    for (i, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let nibble = |v: u8| if v <= b'9' { v - b'0' } else { v - b'a' + 10 };
        result[i] = (nibble(pair[0]) << 4) | nibble(pair[1]);
    }
    Ok(result)
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn verify(
    bytes: &[u8],
    detached: &[u8],
    trust: &TrustedKey,
    policy: &Policy,
) -> Result<Verified> {
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() as u64 <= MAX_MANIFEST,
        "release manifest size refused"
    );
    anyhow::ensure!(
        detached.len() as u64 <= MAX_AUXILIARY,
        "release signature size refused"
    );
    let signature: Detached = serde_json::from_slice(detached)
        .map_err(|_| anyhow!("invalid detached signature format"))?;
    anyhow::ensure!(
        signature.version == 1 && signature.key_id == trust.id,
        "untrusted release signing key"
    );
    let signature = Signature::from_bytes(&decode::<64>(&signature.signature_hex)?);
    let mut message = Vec::with_capacity(DOMAIN.len() + bytes.len());
    message.extend_from_slice(DOMAIN);
    message.extend_from_slice(bytes);
    trust
        .key
        .verify_strict(&message, &signature)
        .map_err(|_| anyhow!("release signature verification failed"))?;
    // Parse and apply policy only after authenticating the exact raw bytes.
    let manifest: Manifest = serde_json::from_slice(bytes)
        .map_err(|_| anyhow!("invalid authenticated release manifest format"))?;
    let artifact = validate_policy(&manifest, policy)?;
    Ok(Verified {
        manifest,
        artifact,
        digest: hex(&Sha256::digest(bytes)),
        key_id: trust.id.clone(),
    })
}
// This validates metadata only. It must never produce an authenticated proof.
fn validate_policy(manifest: &Manifest, policy: &Policy) -> Result<Artifact> {
    anyhow::ensure!(
        manifest.version == 1 && manifest.product == "dsh-ocean",
        "unsupported release manifest scope"
    );
    anyhow::ensure!(
        manifest.channel == policy.channel,
        "release channel mismatch"
    );
    anyhow::ensure!(
        manifest.sequence > 0 && manifest.sequence >= policy.minimum_sequence,
        "release sequence below trusted floor"
    );
    let release =
        Version::parse(&manifest.release).map_err(|_| anyhow!("invalid release version"))?;
    anyhow::ensure!(
        release.build.is_empty() && release >= policy.current_release,
        "release downgrade or build metadata refused"
    );
    anyhow::ensure!(
        manifest.channel != Channel::Stable || release.pre.is_empty(),
        "prerelease refused on stable channel"
    );
    let published = DateTime::parse_from_rfc3339(&manifest.published_at)
        .map_err(|_| anyhow!("invalid publication time"))?
        .with_timezone(&Utc);
    let expires = DateTime::parse_from_rfc3339(&manifest.expires_at)
        .map_err(|_| anyhow!("invalid expiration time"))?
        .with_timezone(&Utc);
    let latest_publication = policy
        .now
        .checked_add_signed(Duration::minutes(5))
        .context("verification clock is out of range")?;
    anyhow::ensure!(
        published <= latest_publication,
        "release publication time is in the future"
    );
    anyhow::ensure!(
        expires > policy.now && expires > published && expires - published <= Duration::days(30),
        "release expired or validity interval refused"
    );
    anyhow::ensure!(
        !manifest.artifacts.is_empty() && manifest.artifacts.len() <= 4,
        "release artifact count refused"
    );
    let mut seen = HashSet::new();
    let mut selected = None;
    for artifact in &manifest.artifacts {
        anyhow::ensure!(
            seen.insert((artifact.target, artifact.kind)),
            "duplicate release artifact"
        );
        anyhow::ensure!(
            artifact.size > 0 && artifact.size <= MAX_ASSET,
            "release artifact size refused"
        );
        decode::<32>(&artifact.sha256)?;
        let expected = artifact_url(&release, artifact.target, artifact.kind);
        anyhow::ensure!(
            artifact.url == expected,
            "release artifact location or filename refused"
        );
        if artifact.target == policy.target && artifact.kind == policy.kind {
            selected = Some(artifact.clone());
        }
    }
    selected.context("requested release artifact is unavailable")
}
fn artifact_url(release: &Version, target: Target, kind: Kind) -> String {
    let arch = match target {
        Target::LinuxX86_64 => "x86_64",
        Target::LinuxAarch64 => "aarch64",
    };
    let extension = match kind {
        Kind::AppImage => "AppImage",
        Kind::TarGz => "tar.gz",
    };
    format!("https://github.com/{REPOSITORY}/releases/download/v{release}/dsh-ocean-{release}-{arch}.{extension}")
}
fn open_regular(path: &Path, limit: u64) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .context("open verification input")?;
    let metadata = file
        .metadata()
        .context("read verification input metadata")?;
    anyhow::ensure!(
        metadata.is_file() && metadata.nlink() == 1 && metadata.len() <= limit,
        "verification input type, links or size refused"
    );
    Ok(file)
}
pub fn read_input(path: &Path, limit: u64) -> Result<Vec<u8>> {
    anyhow::ensure!(limit <= MAX_ASSET, "verification read bound exceeds limit");
    let file = open_regular(path, limit)?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .context("read verification input")?;
    anyhow::ensure!(
        bytes.len() as u64 <= limit,
        "verification input grew beyond limit"
    );
    Ok(bytes)
}
pub fn verify_artifact(path: &Path, release: &Verified) -> Result<()> {
    let (_, digest) = artifact_digest(path, Some(release.artifact.size))?;
    anyhow::ensure!(
        digest == release.artifact.sha256,
        "release artifact SHA-256 mismatch"
    );
    Ok(())
}
fn artifact_digest(path: &Path, expected_size: Option<u64>) -> Result<(u64, String)> {
    let mut file = open_regular(path, MAX_ASSET)?;
    let before = file.metadata()?;
    let size = expected_size.unwrap_or(before.len());
    anyhow::ensure!(
        size > 0 && size <= MAX_ASSET && before.len() == size,
        "release artifact size mismatch"
    );
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).context("read release artifact")?;
        if n == 0 {
            break;
        }
        total += n as u64;
        anyhow::ensure!(total <= size, "release artifact grew during verification");
        hash.update(&buffer[..n]);
    }
    let after = file.metadata()?;
    anyhow::ensure!(
        total == size
            && before.len() == after.len()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
            && after.nlink() == 1,
        "release artifact changed during verification"
    );
    let named = std::fs::symlink_metadata(path).context("recheck artifact pathname")?;
    anyhow::ensure!(
        named.is_file()
            && named.nlink() == 1
            && named.dev() == after.dev()
            && named.ino() == after.ino(),
        "release artifact pathname changed during verification"
    );
    Ok((total, hex(&hash.finalize())))
}

pub struct DraftArtifact {
    pub target: Target,
    pub kind: Kind,
    pub source: PathBuf,
}

/// Unsigned single-artifact publisher draft, never an authenticated proof.
pub fn draft(
    source: &Path,
    release: &str,
    sequence: u64,
    valid_hours: u16,
    policy: &Policy,
) -> Result<Manifest> {
    draft_many(
        &[DraftArtifact {
            target: policy.target,
            kind: policy.kind,
            source: source.to_owned(),
        }],
        release,
        sequence,
        valid_hours,
        policy,
    )
}
/// At most four unique target/kind pairs. No source paths appear in output.
pub fn draft_many(
    sources: &[DraftArtifact],
    release: &str,
    sequence: u64,
    valid_hours: u16,
    policy: &Policy,
) -> Result<Manifest> {
    anyhow::ensure!(
        !sources.is_empty() && sources.len() <= 4,
        "draft artifact count refused"
    );
    anyhow::ensure!(
        (1..=720).contains(&valid_hours),
        "draft validity must be 1 to 720 hours"
    );
    let version = Version::parse(release).map_err(|_| anyhow!("invalid draft release version"))?;
    let expires = policy
        .now
        .checked_add_signed(Duration::hours(i64::from(valid_hours)))
        .context("draft clock is out of range")?;
    let mut manifest = Manifest {
        version: 1,
        product: "dsh-ocean".into(),
        channel: policy.channel,
        sequence,
        release: version.to_string(),
        published_at: policy.now.to_rfc3339(),
        expires_at: expires.to_rfc3339(),
        artifacts: sources
            .iter()
            .map(|source| Artifact {
                target: source.target,
                kind: source.kind,
                url: artifact_url(&version, source.target, source.kind),
                size: 1,
                sha256: "0".repeat(64),
            })
            .collect(),
    };
    // Reject bad version/channel/sequence/time before reading any artifact bytes.
    validate_policy(&manifest, policy)?;
    anyhow::ensure!(
        (serde_json::to_vec_pretty(&manifest)?.len() as u64) < MAX_MANIFEST,
        "unsigned draft exceeds manifest limit"
    );
    for (artifact, source) in manifest.artifacts.iter_mut().zip(sources) {
        let (size, sha256) = artifact_digest(&source.source, None)?;
        artifact.size = size;
        artifact.sha256 = sha256;
    }
    validate_policy(&manifest, policy)?;
    anyhow::ensure!(
        (serde_json::to_vec_pretty(&manifest)?.len() as u64) < MAX_MANIFEST,
        "unsigned draft exceeds manifest limit"
    );
    Ok(manifest)
}

/// Verified local copy, not an installed executable. Keep the read-only
/// descriptor for a future consumer; the pathname can be replaced after return.
pub struct Staged {
    path: PathBuf,
    reader: File,
}
impl Staged {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn reader(&self) -> &File {
        &self.reader
    }
}
pub fn stage_artifact(source: &Path, destination: &Path, release: &Verified) -> Result<Staged> {
    let mut input = open_regular(source, MAX_ASSET)?;
    anyhow::ensure!(
        input.metadata()?.len() == release.artifact.size,
        "release artifact size mismatch"
    );
    stage_reader(&mut input, destination, release)
}
pub(crate) fn stage_reader(
    input: &mut impl Read,
    destination: &Path,
    release: &Verified,
) -> Result<Staged> {
    stage_reader_with_io(
        input,
        destination,
        release,
        |file, bytes| file.write_all(bytes),
        File::sync_all,
    )
}
// Private operation seam for deterministic I/O-failure tests. Production always
// uses File::write_all and File::sync_all, never an environment/remote override.
fn stage_reader_with_io(
    input: &mut impl Read,
    destination: &Path,
    release: &Verified,
    mut write: impl FnMut(&mut File, &[u8]) -> std::io::Result<()>,
    sync: impl FnOnce(&File) -> std::io::Result<()>,
) -> Result<Staged> {
    let mut transfer = crate::downloads::Transfer::prepare(destination)
        .context("prepare new release staging destination")?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&transfer.payload)
        .context("create private release staging file")?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let size = input.read(&mut buffer).context("read release source")?;
        if size == 0 {
            break;
        }
        total += size as u64;
        anyhow::ensure!(
            total <= release.artifact.size,
            "release source exceeds signed size"
        );
        write(&mut output, &buffer[..size]).context("write private release staging file")?;
        hash.update(&buffer[..size]);
    }
    anyhow::ensure!(
        total == release.artifact.size && hex(&hash.finalize()) == release.artifact.sha256,
        "staged release size or SHA-256 mismatch"
    );
    sync(&output).context("sync verified release staging file")?;
    let reader = open_regular(&transfer.payload, MAX_ASSET)?;
    let a = output.metadata()?;
    let b = reader.metadata()?;
    anyhow::ensure!(
        a.dev() == b.dev() && a.ino() == b.ino() && b.len() == total,
        "private release staging file changed"
    );
    let path = transfer
        .publish()
        .context("publish verified release without overwriting")?;
    // The copy is private mode 0600, deliberately not executable. No artifact
    // is applied or launched, and no source file is ever removed/modified.
    Ok(Staged { path, reader })
}

#[cfg(test)]
mod staging_tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "dsh-ocean-release-stream-{}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert!(self
                .0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("dsh-ocean-release-stream-"));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn proof(bytes: &[u8]) -> Verified {
        // Stream fault-injection unit proof only. Public callers cannot construct
        // Verified and must authenticate a manifest first.
        let artifact = Artifact {
            target: Target::LinuxX86_64,
            kind: Kind::AppImage,
            url: String::new(),
            size: bytes.len() as u64,
            sha256: hex(&Sha256::digest(bytes)),
        };
        let manifest = Manifest {
            version: 1,
            product: "dsh-ocean".into(),
            channel: Channel::Stable,
            sequence: 1,
            release: "0.1.0".into(),
            published_at: String::new(),
            expires_at: String::new(),
            artifacts: vec![artifact.clone()],
        };
        Verified {
            manifest,
            artifact,
            digest: String::new(),
            key_id: "unit-test-only".into(),
        }
    }
    struct Interrupt {
        first: bool,
    }
    impl Read for Interrupt {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.first {
                self.first = false;
                out[0] = b'x';
                Ok(1)
            } else {
                Err(std::io::Error::other("synthetic interrupted source"))
            }
        }
    }
    #[test]
    fn incomplete_wrong_and_interrupted_streams_never_publish_or_leave_staging() {
        let f = Fixture::new();
        let target = f.0.join("new.AppImage");
        let release = proof(b"expected");
        assert!(stage_reader(&mut Cursor::new(b"short"), &target, &release).is_err());
        assert!(stage_reader(&mut Cursor::new(b"not-same"), &target, &release).is_err());
        assert!(stage_reader(&mut Cursor::new(b"expected extra"), &target, &release).is_err());
        assert!(stage_reader(&mut Interrupt { first: true }, &target, &release).is_err());
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 0);
    }
    struct Race {
        bytes: Cursor<Vec<u8>>,
        target: PathBuf,
        fired: bool,
    }
    impl Read for Race {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if !self.fired {
                self.fired = true;
                std::fs::write(&self.target, b"user file wins")?;
            }
            self.bytes.read(out)
        }
    }
    #[test]
    fn destination_created_while_copying_is_preserved_without_publication() {
        let f = Fixture::new();
        let target = f.0.join("race.AppImage");
        let mut input = Race {
            bytes: Cursor::new(b"expected".to_vec()),
            target: target.clone(),
            fired: false,
        };
        assert!(stage_reader(&mut input, &target, &proof(b"expected")).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"user file wins");
        assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 1);
    }
    #[test]
    fn failed_or_short_writes_remove_partial_payload_without_sync_or_publish() {
        for (prefix, code) in [(0, libc::ENOSPC), (3, libc::ENOSPC), (2, libc::EIO)] {
            let f = Fixture::new();
            let target = f.0.join("new.AppImage");
            let synced = std::cell::Cell::new(false);
            let error = stage_reader_with_io(
                &mut Cursor::new(b"expected"),
                &target,
                &proof(b"expected"),
                |file, bytes| {
                    file.write_all(&bytes[..prefix])?;
                    Err(std::io::Error::from_raw_os_error(code))
                },
                |_| {
                    synced.set(true);
                    Ok(())
                },
            )
            .err()
            .unwrap();
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(code)
            );
            assert!(!synced.get());
            assert!(!target.exists());
            assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 0);
        }
        let f = Fixture::new();
        let target = f.0.join("write-zero.AppImage");
        let error = stage_reader_with_io(
            &mut Cursor::new(b"expected"),
            &target,
            &proof(b"expected"),
            |file, bytes| {
                file.write_all(&bytes[..1])?;
                Err(std::io::ErrorKind::WriteZero.into())
            },
            File::sync_all,
        )
        .err()
        .unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::WriteZero
        );
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 0);
    }
    #[test]
    fn sync_failure_discards_verified_partial_and_preserves_a_late_user_target() {
        let f = Fixture::new();
        let target = f.0.join("sync.AppImage");
        let error = stage_reader_with_io(
            &mut Cursor::new(b"expected"),
            &target,
            &proof(b"expected"),
            |file, bytes| file.write_all(bytes),
            |file| {
                assert_eq!(file.metadata()?.len(), 8);
                std::fs::write(&target, b"user bytes survive failed fsync")?;
                Err(std::io::Error::from_raw_os_error(libc::EIO))
            },
        )
        .err()
        .unwrap();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(libc::EIO)
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"user bytes survive failed fsync"
        );
        assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 1);
    }
    #[test]
    fn wrong_hash_never_reaches_sync_or_publication() {
        let f = Fixture::new();
        let target = f.0.join("wrong-hash.AppImage");
        let synced = std::cell::Cell::new(false);
        let result = stage_reader_with_io(
            &mut Cursor::new(b"not-same"),
            &target,
            &proof(b"expected"),
            |file, bytes| file.write_all(bytes),
            |_| {
                synced.set(true);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!synced.get());
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 0);
    }
}

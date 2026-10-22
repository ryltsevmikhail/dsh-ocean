//! Signed complete-runtime metadata. A shell signature/state is never runtime
//! authority. The client consumes resolved publisher-built bytes, not npm tags.
use crate::release_feed::{self, Artifact, Channel, Kind, Policy, Target, TrustedKey};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
pub const DOMAIN: &[u8] = b"DSH Ocean runtime manifest v1\n";
pub const STATE_SCOPE: &str = "web-profile-sessions-storages-v1";

pub struct Candidate {
    verified: Verified,
    manifest: Vec<u8>,
    signature: Vec<u8>,
    settings_digest: String,
    fingerprint: String,
    current_version: semver::Version,
}
impl Candidate {
    pub fn verified(&self) -> &Verified {
        &self.verified
    }
    pub fn signed_bytes(&self) -> (&[u8], &[u8]) {
        (&self.manifest, &self.signature)
    }
    pub fn summary(&self) -> Value {
        let mut value = self.verified.summary();
        value["downloaded"] = json!(false);
        value["keyFingerprint"] = json!(self.fingerprint);
        value["publisherTrust"] = json!("operator-confirmed-public-key");
        value
    }
    pub fn revalidate(
        &self,
        settings: &crate::update_settings::Settings,
        current_version: &semver::Version,
    ) -> Result<Verified> {
        anyhow::ensure!(
            settings.digest()? == self.settings_digest,
            "runtime update settings changed; check again"
        );
        verify_configured(
            settings,
            &self.manifest,
            &self.signature,
            std::cmp::max(current_version, &self.current_version),
        )
    }
}
fn verify_configured(
    settings: &crate::update_settings::Settings,
    manifest: &[u8],
    signature: &[u8],
    current_version: &semver::Version,
) -> Result<Verified> {
    use crate::update_settings::Product;
    settings.validate()?;
    let source = settings.source(Product::Runtime)?;
    let channel = settings.channel(Product::Runtime);
    let policy = Policy {
        channel,
        target: crate::update_feed::native_target()?,
        kind: Kind::TarGz,
        minimum_sequence: source.feed(channel).minimum_sequence,
        current_release: current_version.clone(),
        now: chrono::Utc::now(),
    };
    let verified = verify(manifest, signature, &source.trust()?, &policy)?;
    let name = match channel {
        Channel::Stable => "stable",
        Channel::Next => "next",
    };
    crate::release_state::check(
        &crate::release_state::read_for(Product::Runtime)?,
        name,
        verified.manifest.sequence,
        &verified.manifest.release,
        verified.manifest_sha256(),
    )?;
    Ok(verified)
}
pub fn check(
    settings: &crate::update_settings::Settings,
    current_version: &semver::Version,
    control: std::sync::Arc<crate::update_control::Control>,
) -> Result<Candidate> {
    use crate::update_settings::Product;
    control.check()?;
    settings.validate()?;
    let source = settings.source(Product::Runtime)?;
    crate::release_state::read_for(Product::Runtime)?;
    let feed = source.feed(settings.channel(Product::Runtime));
    let manifest = crate::release_download::fetch_metadata_controlled(
        &feed.manifest_url,
        release_feed::MAX_MANIFEST,
        control.clone(),
    )?;
    let signature = crate::release_download::fetch_metadata_controlled(
        &feed.signature_url,
        release_feed::MAX_AUXILIARY,
        control.clone(),
    )?;
    control.check()?;
    let verified = verify_configured(settings, &manifest, &signature, current_version)?;
    control.check()?;
    Ok(Candidate {
        verified,
        manifest,
        signature,
        settings_digest: settings.digest()?,
        fingerprint: source.trust()?.fingerprint(),
        current_version: current_version.clone(),
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Components {
    pub node: String,
    pub dsh: String,
    pub pnpm: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeArtifact {
    pub target: Target,
    pub kind: Kind,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub node_archive_sha256: String,
}
impl RuntimeArtifact {
    pub fn artifact(&self) -> Artifact {
        Artifact {
            target: self.target,
            kind: self.kind,
            url: self.url.clone(),
            size: self.size,
            sha256: self.sha256.clone(),
        }
    }
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
    pub components: Components,
    pub state_scope: String,
    pub artifacts: Vec<RuntimeArtifact>,
}
impl Manifest {
    fn metadata(&self) -> release_feed::Manifest {
        release_feed::Manifest {
            version: self.version,
            product: self.product.clone(),
            channel: self.channel,
            sequence: self.sequence,
            release: self.release.clone(),
            published_at: self.published_at.clone(),
            expires_at: self.expires_at.clone(),
            artifacts: self
                .artifacts
                .iter()
                .map(RuntimeArtifact::artifact)
                .collect(),
        }
    }
}
pub struct Verified {
    manifest: Manifest,
    artifact: RuntimeArtifact,
    digest: String,
    key_id: String,
}
impl Verified {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn artifact(&self) -> &RuntimeArtifact {
        &self.artifact
    }
    pub fn manifest_sha256(&self) -> &str {
        &self.digest
    }
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
    pub fn root_name(&self) -> String {
        format!(
            "dsh-ocean-runtime-{}-{}-{}-linux-{}",
            channel(self.manifest.channel),
            self.manifest.release,
            self.manifest.sequence,
            arch(self.artifact.target)
        )
    }
    pub fn summary(&self) -> Value {
        json!({"product":"dsh-ocean-runtime","verification":"signature-and-policy","release":self.manifest.release,"sequence":self.manifest.sequence,"channel":self.manifest.channel,"components":self.manifest.components,"stateScope":self.manifest.state_scope,"artifact":self.artifact,"manifestSha256":self.digest,"keyId":self.key_id,"applied":false,"warning":"Authenticated complete-bundle metadata only; no runtime executed/installed, version accepted, service restarted or profile accessed."})
    }
}
fn channel(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "stable",
        Channel::Next => "next",
    }
}
fn arch(target: Target) -> &'static str {
    match target {
        Target::LinuxX86_64 => "x86_64",
        Target::LinuxAarch64 => "aarch64",
    }
}
pub fn artifact_url(
    channel_name: Channel,
    release: &semver::Version,
    sequence: u64,
    target: Target,
) -> String {
    let channel = channel(channel_name);
    let arch = arch(target);
    format!("https://github.com/ryltsevmikhail/dsh-ocean/releases/download/runtime-{channel}-v{release}-r{sequence}/dsh-ocean-runtime-{channel}-{release}-{sequence}-{arch}.tar.gz")
}
pub fn verify(
    bytes: &[u8],
    signature: &[u8],
    trust: &TrustedKey,
    policy: &Policy,
) -> Result<Verified> {
    trust.authenticate(DOMAIN, bytes, signature)?;
    let manifest: Manifest = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("invalid authenticated runtime manifest"))?;
    anyhow::ensure!(
        policy.kind == Kind::TarGz
            && manifest.artifacts.len() <= 2
            && manifest
                .artifacts
                .iter()
                .all(|artifact| artifact.kind == Kind::TarGz),
        "runtime requires complete per-architecture tar bundles"
    );
    anyhow::ensure!(
        manifest.state_scope == STATE_SCOPE,
        "unsupported runtime writable-state compatibility scope"
    );
    for version in [
        &manifest.components.node,
        &manifest.components.dsh,
        &manifest.components.pnpm,
    ] {
        let parsed =
            semver::Version::parse(version).context("invalid signed runtime component version")?;
        anyhow::ensure!(
            parsed.build.is_empty(),
            "runtime component build metadata refused"
        );
    }
    anyhow::ensure!(
        manifest.components.dsh == manifest.release,
        "bundle release and DSH version mismatch"
    );
    for artifact in &manifest.artifacts {
        release_feed::decode::<32>(&artifact.node_archive_sha256)?;
    }
    let common = release_feed::validate_metadata(
        &manifest.metadata(),
        policy,
        "dsh-ocean-runtime",
        |release, target, _kind| artifact_url(manifest.channel, release, manifest.sequence, target),
    )?;
    let artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.target == common.target)
        .context("selected runtime target is absent")?
        .clone();
    Ok(Verified {
        manifest,
        artifact,
        digest: release_feed::hex(&Sha256::digest(bytes)),
        key_id: trust.id().to_owned(),
    })
}

//! Authenticate configured shell feeds without accepting a version or installing
//! anything. Candidates retain exact signed bytes and must be revalidated at use.
use crate::release_feed::{self, Kind, Policy, Target, Verified};
use crate::update_settings::{Product, Settings};
use anyhow::{Context, Result};
use serde_json::{json, Value};

pub fn native_target() -> Result<Target> {
    match std::env::consts::ARCH {
        "x86_64" => Ok(Target::LinuxX86_64),
        "aarch64" => Ok(Target::LinuxAarch64),
        _ => anyhow::bail!("signed updates do not support this architecture"),
    }
}
fn verify(settings: &Settings, bytes: &[u8], signature: &[u8], kind: Kind) -> Result<Verified> {
    settings.validate()?;
    let source = settings.source(Product::Shell)?;
    let channel = settings.channel(Product::Shell);
    let policy = Policy {
        channel,
        target: native_target()?,
        kind,
        minimum_sequence: source.feed(channel).minimum_sequence,
        current_release: semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("package version"),
        now: chrono::Utc::now(),
    };
    let verified = release_feed::verify(bytes, signature, &source.trust()?, &policy)?;
    let channel_name = match channel {
        release_feed::Channel::Stable => "stable",
        release_feed::Channel::Next => "next",
    };
    crate::release_state::check(
        &crate::release_state::read()?,
        channel_name,
        verified.manifest().sequence,
        &verified.manifest().release,
        verified.manifest_sha256(),
    )?;
    Ok(verified)
}

pub struct ShellCandidate {
    verified: Verified,
    manifest: Vec<u8>,
    signature: Vec<u8>,
    settings_digest: String,
    fingerprint: String,
}
impl ShellCandidate {
    pub fn verified(&self) -> &Verified {
        &self.verified
    }
    pub fn signed_bytes(&self) -> (&[u8], &[u8]) {
        (&self.manifest, &self.signature)
    }
    /// Re-check configured identity/channel, current UTC expiry and latest floor.
    /// A configured apply must hold update_settings::Guard through this check and
    /// activation, so CLI trust replacement cannot race the final commit.
    pub fn revalidate(&self, settings: &Settings) -> Result<Verified> {
        anyhow::ensure!(
            settings.digest()? == self.settings_digest,
            "update settings changed; check again before using this candidate"
        );
        verify(
            settings,
            &self.manifest,
            &self.signature,
            self.verified.artifact().kind,
        )
    }
    pub fn summary(&self) -> Result<Value> {
        let release = &self.verified.manifest().release;
        let available = semver::Version::parse(release).context("verified release version")?
            > semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("package version");
        Ok(
            json!({"verification":"signature-and-policy","publisherTrust":"operator-confirmed-public-key","keyId":self.verified.key_id(),"keyFingerprint":self.fingerprint,"manifestSha256":self.verified.manifest_sha256(),"release":release,"runningRelease":env!("CARGO_PKG_VERSION"),"channel":self.verified.manifest().channel,"sequence":self.verified.manifest().sequence,"expiresAt":self.verified.manifest().expires_at,"artifact":self.verified.artifact(),"available":available,"downloaded":false,"applied":false,"warning":"Authenticated metadata only. No key was fetched, version accepted, artifact downloaded, process restarted or profile changed."}),
        )
    }
}

pub fn check_shell(settings: &Settings, kind: Kind) -> Result<ShellCandidate> {
    check_shell_controlled(
        settings,
        kind,
        std::sync::Arc::new(crate::update_control::Control::default()),
    )
}
pub fn check_shell_controlled(
    settings: &Settings,
    kind: Kind,
    control: std::sync::Arc<crate::update_control::Control>,
) -> Result<ShellCandidate> {
    control.check()?;
    settings.validate()?;
    let source = settings.source(Product::Shell)?;
    let trust = source.trust()?;
    // Corrupt remembered state blocks even metadata networking rather than
    // silently using an empty floor. Final verification reads it again.
    crate::release_state::read()?;
    let feed = source.feed(settings.channel(Product::Shell));
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
    let verified = verify(settings, &manifest, &signature, kind)?;
    control.check()?;
    Ok(ShellCandidate {
        verified,
        manifest,
        signature,
        settings_digest: settings.digest()?,
        fingerprint: trust.fingerprint(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release_feed::{Channel, PublicKeyDocument};
    use crate::update_settings::{Feed, Source};
    use chrono::{Duration, Utc};
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};

    // Authenticated at an older injected time, but expired now. This fixture
    // never fetches, loads settings/state from disk, or changes process env.
    fn expired_candidate() -> (Settings, ShellCandidate) {
        let key = SigningKey::from_bytes(&[73; 32]);
        let document = PublicKeyDocument {
            version: 1,
            key_id: "test-candidate".into(),
            public_key_hex: release_feed::hex(key.verifying_key().as_bytes()),
        };
        let feed = Feed { manifest_url: "https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/manifest.json".into(), signature_url: "https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/manifest.sig.json".into(), minimum_sequence: 1 };
        let source = Source {
            key: document,
            stable: feed.clone(),
            next: feed,
        };
        let settings = Settings {
            shell: Some(source),
            ..Settings::default()
        };
        let target = native_target().unwrap();
        let arch = match target {
            Target::LinuxX86_64 => "x86_64",
            Target::LinuxAarch64 => "aarch64",
        };
        let now = Utc::now();
        let manifest = serde_json::to_vec(&json!({"version":1,"product":"dsh-ocean","channel":"stable","sequence":5,"release":"0.2.0","publishedAt":(now-Duration::days(3)).to_rfc3339(),"expiresAt":(now-Duration::days(1)).to_rfc3339(),"artifacts":[{"target":target,"kind":"appimage","url":format!("https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v0.2.0/dsh-ocean-0.2.0-{arch}.AppImage"),"size":7,"sha256":release_feed::hex(&Sha256::digest(b"payload"))}]})).unwrap();
        let mut message = release_feed::DOMAIN.to_vec();
        message.extend_from_slice(&manifest);
        let signature = serde_json::to_vec(&json!({"version":1,"keyId":"test-candidate","signatureHex":release_feed::hex(&key.sign(&message).to_bytes())})).unwrap();
        let policy = Policy {
            channel: Channel::Stable,
            target,
            kind: Kind::AppImage,
            minimum_sequence: 1,
            current_release: semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap(),
            now: now - Duration::days(2),
        };
        let trust = settings.source(Product::Shell).unwrap().trust().unwrap();
        let verified = release_feed::verify(&manifest, &signature, &trust, &policy).unwrap();
        let candidate = ShellCandidate {
            verified,
            manifest,
            signature,
            settings_digest: settings.digest().unwrap(),
            fingerprint: trust.fingerprint(),
        };
        (settings, candidate)
    }
    #[test]
    fn pre_cancelled_feed_checks_stop_before_even_validating_or_reading_state() {
        let (mut settings, _) = expired_candidate();
        // Invalid URL is a no-network safety trap while the new entry point is
        // red: the cancellation error must win before source/state/transport.
        settings.shell.as_mut().unwrap().stable.manifest_url = "http://not-an-update/".into();
        let control = std::sync::Arc::new(crate::update_control::Control::default());
        assert!(control.cancel());
        let error = check_shell_controlled(&settings, Kind::AppImage, control)
            .err()
            .expect("cancelled operation");
        assert!(error.to_string().contains("cancelled"));
    }
    #[test]
    fn stale_candidates_recheck_time_and_channel_before_any_state_read() {
        let (settings, candidate) = expired_candidate();
        let error = candidate
            .revalidate(&settings)
            .err()
            .expect("expiry must be rechecked");
        assert!(error.to_string().contains("expired"));
        let mut changed = settings;
        changed.shell_channel = Channel::Next;
        let error = candidate
            .revalidate(&changed)
            .err()
            .expect("configuration invalidates prior candidate");
        assert!(error.to_string().contains("settings changed"));
    }
}

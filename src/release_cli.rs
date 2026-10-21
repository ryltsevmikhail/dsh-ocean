//! Signed release verification, explicit HTTPS staging, and local apply/rollback.
use crate::release_feed::{self, Channel, Kind, Policy, Target, TrustedKey};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub const USAGE: &str = "--verify-release MANIFEST SIGNATURE TRUSTED_KEY --channel stable|next --target linux-x86_64|linux-aarch64 --kind appimage|tar.gz --min-sequence N [--artifact FILE]; --stage-release uses the same policy and requires --artifact SOURCE --output NEW_FILE; --download-release uses the same authenticated policy, requires --output NEW_FILE and downloads only the signed URL via trusted curl; --accept-release verifies and remembers the accepted sequence; --release-state reports it; --reset-release-state --confirm-reset forgets it";
/// `--apply-release MANIFEST SIGNATURE KEY <policy> --artifact STAGED --prefix DIR --confirm-apply`
///
/// Verifies, checks the remembered sequence, activates the file, and only then
/// remembers it. Nothing is downloaded, and no service is restarted.
fn apply(args: &[String]) -> Result<Value> {
    anyhow::ensure!(args.len() >= 4, "{USAGE}");
    let mut flags = HashMap::new();
    let (pairs, remainder) = args[4..].as_chunks::<2>();
    for pair in pairs {
        anyhow::ensure!(
            matches!(
                pair[0].as_str(),
                "--channel"
                    | "--target"
                    | "--kind"
                    | "--min-sequence"
                    | "--artifact"
                    | "--prefix"
                    | "--confirm-apply"
            ),
            "unknown apply option"
        );
        anyhow::ensure!(
            flags.insert(pair[0].as_str(), pair[1].as_str()).is_none(),
            "duplicate apply option"
        );
    }
    anyhow::ensure!(remainder.is_empty(), "apply option requires a value");
    anyhow::ensure!(
        flags.get("--confirm-apply").copied() == Some("yes"),
        "activating a release requires --confirm-apply yes"
    );
    let channel = match flags.get("--channel").copied() {
        Some("stable") => Channel::Stable,
        Some("next") => Channel::Next,
        _ => return Err(anyhow!("release channel must be stable or next")),
    };
    let target = match flags.get("--target").copied() {
        Some("linux-x86_64") => Target::LinuxX86_64,
        Some("linux-aarch64") => Target::LinuxAarch64,
        _ => return Err(anyhow!("unsupported release target")),
    };
    let kind = match flags.get("--kind").copied() {
        Some("appimage") => Kind::AppImage,
        Some("tar.gz") => Kind::TarGz,
        _ => return Err(anyhow!("unsupported release artifact kind")),
    };
    let minimum_sequence = flags
        .get("--min-sequence")
        .context("explicit --min-sequence is required")?
        .parse::<u64>()
        .map_err(|_| anyhow!("invalid sequence floor"))?;
    let policy = Policy {
        channel,
        target,
        kind,
        minimum_sequence,
        current_release: semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("valid package version"),
        now: chrono::Utc::now(),
    };
    let key = TrustedKey::from_json(&release_feed::read_input(
        Path::new(&args[3]),
        release_feed::MAX_AUXILIARY,
    )?)?;
    let bytes = release_feed::read_input(Path::new(&args[1]), release_feed::MAX_MANIFEST)?;
    let signature = release_feed::read_input(Path::new(&args[2]), release_feed::MAX_AUXILIARY)?;
    let release = release_feed::verify(&bytes, &signature, &key, &policy)?;
    let staged = Path::new(
        flags
            .get("--artifact")
            .context("--artifact STAGED is required")?,
    );
    let prefix = Path::new(flags.get("--prefix").context("--prefix DIR is required")?);
    release_feed::verify_artifact(staged, &release)?;
    let channel_name = match channel {
        Channel::Stable => "stable",
        Channel::Next => "next",
    };
    // Check the remembered floor *before* touching the prefix; the entry is only
    // recorded after a successful activation.
    let remembered = crate::release_state::read()?;
    crate::release_state::check(
        &remembered,
        channel_name,
        release.manifest().sequence,
        &release.manifest().release,
        release.manifest_sha256(),
    )?;
    let receipt = crate::shell_apply::apply(staged, &release, prefix)?;
    let entry = crate::release_state::record(
        channel_name,
        release.manifest().sequence,
        &release.manifest().release,
        release.manifest_sha256(),
    )?;
    Ok(json!({
        "verification": "signature-and-policy",
        "artifactChecked": true,
        "apply": receipt,
        "remembered": {
            "channel": channel_name,
            "sequence": entry.sequence,
            "release": entry.release,
            "recordedAt": entry.recorded_at,
        },
        "downloaded": false,
        "serviceRestarted": false,
    }))
}

/// `--rollback-shell --prefix DIR --confirm-rollback yes`
fn rollback_shell(args: &[String]) -> Result<Value> {
    let mut prefix = None;
    let mut confirmed = false;
    let mut iter = args.iter().skip(1);
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "--confirm-rollback" => {
                let value = iter.next().context("--confirm-rollback needs a value")?;
                anyhow::ensure!(value == "yes", "rollback requires --confirm-rollback yes");
                confirmed = true;
            }
            "--prefix" => {
                anyhow::ensure!(prefix.is_none(), "--prefix given twice");
                prefix = Some(iter.next().context("--prefix needs a directory")?.clone());
            }
            other => anyhow::bail!("unexpected rollback argument: {other}"),
        }
    }
    anyhow::ensure!(confirmed, "rollback requires --confirm-rollback yes");
    let prefix = prefix.context("--prefix DIR is required")?;
    crate::shell_apply::rollback(Path::new(&prefix))
}

/// Read or clear the persisted anti-rollback store.
fn state(args: &[String], command: &str) -> Result<Value> {
    if command == "--release-state" {
        anyhow::ensure!(args.len() == 1, "usage: --release-state");
        let channels = crate::release_state::read()?;
        return Ok(json!({
            "path": crate::release_state::path(),
            "channels": channels,
            "warning": "Local replay/downgrade protection for signed manifests only. Not a signature, trusted clock or proof of publisher trust; a same-user attacker can delete this file.",
        }));
    }
    anyhow::ensure!(
        args.len() == 2 && args[1] == "--confirm-reset",
        "usage: --reset-release-state --confirm-reset; this forgets the remembered sequence and allows older signed manifests again"
    );
    let removed = crate::release_state::reset()?;
    Ok(json!({
        "reset": removed,
        "warning": "Remembered release sequences were cleared. Older signed manifests will be accepted again until a newer one is accepted.",
    }))
}

pub fn run(args: &[String]) -> Result<Value> {
    if let Some(first) = args.first().map(String::as_str) {
        if first == "--release-state" || first == "--reset-release-state" {
            return state(args, first);
        }
        if first == "--rollback-shell" {
            return rollback_shell(args);
        }
        if first == "--apply-release" {
            return apply(args);
        }
    }
    anyhow::ensure!(
        matches!(
            args.first().map(String::as_str),
            Some(
                "--verify-release" | "--stage-release" | "--accept-release" | "--download-release"
            )
        ) && args.len() >= 4,
        "{USAGE}"
    );
    let staging = args[0] == "--stage-release";
    let downloading = args[0] == "--download-release";
    let accepting = args[0] == "--accept-release";
    let mut flags = HashMap::new();
    let (pairs, remainder) = args[4..].as_chunks::<2>();
    for pair in pairs {
        anyhow::ensure!(
            matches!(
                pair[0].as_str(),
                "--channel" | "--target" | "--kind" | "--min-sequence" | "--artifact"
            ) || ((staging || downloading) && pair[0] == "--output"),
            "unknown release verification option"
        );
        anyhow::ensure!(
            flags.insert(pair[0].as_str(), pair[1].as_str()).is_none(),
            "duplicate release verification option"
        );
    }
    anyhow::ensure!(
        remainder.is_empty(),
        "release verification option requires a value"
    );
    anyhow::ensure!(
        !staging || (flags.contains_key("--artifact") && flags.contains_key("--output")),
        "--stage-release requires --artifact SOURCE and --output NEW_FILE"
    );
    anyhow::ensure!(
        !downloading || (flags.contains_key("--output") && !flags.contains_key("--artifact")),
        "--download-release requires --output NEW_FILE and uses only the signed artifact URL"
    );
    anyhow::ensure!(
        !accepting || !flags.contains_key("--output"),
        "--accept-release does not write an artifact; use --stage-release for that"
    );
    let channel = match flags.get("--channel").copied() {
        Some("stable") => Channel::Stable,
        Some("next") => Channel::Next,
        _ => return Err(anyhow!("release channel must be stable or next")),
    };
    let target = match flags.get("--target").copied() {
        Some("linux-x86_64") => Target::LinuxX86_64,
        Some("linux-aarch64") => Target::LinuxAarch64,
        _ => return Err(anyhow!("unsupported release target")),
    };
    let kind = match flags.get("--kind").copied() {
        Some("appimage") => Kind::AppImage,
        Some("tar.gz") => Kind::TarGz,
        _ => return Err(anyhow!("unsupported release artifact kind")),
    };
    let minimum_sequence = flags
        .get("--min-sequence")
        .context("explicit --min-sequence is required")?
        .parse::<u64>()
        .map_err(|_| anyhow!("invalid sequence floor"))?;
    let policy = Policy {
        channel,
        target,
        kind,
        minimum_sequence,
        current_release: semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("valid package version"),
        now: chrono::Utc::now(),
    };
    let key = TrustedKey::from_json(&release_feed::read_input(
        Path::new(&args[3]),
        release_feed::MAX_AUXILIARY,
    )?)?;
    let bytes = release_feed::read_input(Path::new(&args[1]), release_feed::MAX_MANIFEST)?;
    let signature = release_feed::read_input(Path::new(&args[2]), release_feed::MAX_AUXILIARY)?;
    let release = release_feed::verify(&bytes, &signature, &key, &policy)?;
    let mut staged_path = None;
    let checked = if downloading {
        let channel_name = match channel {
            Channel::Stable => "stable",
            Channel::Next => "next",
        };
        crate::release_state::check(
            &crate::release_state::read()?,
            channel_name,
            release.manifest().sequence,
            &release.manifest().release,
            release.manifest_sha256(),
        )?;
        let output = Path::new(flags["--output"]);
        let absolute = if output.is_absolute() {
            output.to_owned()
        } else {
            std::env::current_dir()
                .context("resolve release download output")?
                .join(output)
        };
        let staged = crate::release_download::download(&release, &absolute)?;
        staged_path = Some(staged.path().to_owned());
        true
    } else if let Some(path) = flags.get("--artifact") {
        if staging {
            let output = Path::new(flags["--output"]);
            let absolute = if output.is_absolute() {
                output.to_owned()
            } else {
                std::env::current_dir()
                    .context("resolve release staging output")?
                    .join(output)
            };
            let staged = release_feed::stage_artifact(Path::new(path), &absolute, &release)?;
            staged_path = Some(staged.path().to_owned());
        } else {
            release_feed::verify_artifact(Path::new(path), &release)?;
        }
        true
    } else {
        false
    };
    // Only an explicitly accepted, fully verified manifest is remembered. A
    // remembered entry blocks later replays/downgrades of signed manifests.
    let remembered = if accepting {
        let channel = match release.manifest().channel {
            Channel::Stable => "stable",
            Channel::Next => "next",
        };
        let entry = crate::release_state::record(
            channel,
            release.manifest().sequence,
            &release.manifest().release,
            release.manifest_sha256(),
        )?;
        Some(json!({
            "channel": channel,
            "sequence": entry.sequence,
            "release": entry.release,
            "manifestSha256": entry.manifest_sha256,
            "recordedAt": entry.recorded_at,
        }))
    } else {
        None
    };
    Ok(
        json!({"verification":"signature-and-policy","publisherTrust":"operator-supplied-key-only","keyId":release.key_id(),"manifestSha256":release.manifest_sha256(),"release":release.manifest().release,"channel":release.manifest().channel,"sequence":release.manifest().sequence,"artifact":release.artifact(),"artifactChecked":checked,"downloaded":downloading,"stagedPath":staged_path,"remembered":remembered,"applied":false,"warning":if remembered.is_some(){"No production trust root is embedded. No download, install or service change occurred. The accepted sequence is remembered locally to refuse later replays/downgrades of signed manifests; this is not a signature or a trusted clock."}else if downloading{"Artifact downloaded over HTTPS and verified into private staging. No production trust root is embedded. Nothing was executed, installed or remembered, and no service changed. Core/bootstrap updates remain unsigned."}else{"No production trust root is embedded. No download, install, remembered sequence or service change occurred. This does not sign or secure current core/bootstrap update paths."}}),
    )
}

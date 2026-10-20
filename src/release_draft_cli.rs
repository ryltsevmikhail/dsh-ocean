//! Read-only unsigned manifest generation. No key, signature or trusted proof.
use crate::release_feed::{self, Channel, DraftArtifact, Kind, Manifest, Policy, Target};
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

pub const USAGE: &str = "--draft-release --release VERSION --channel stable|next --sequence N --valid-hours N (--target TARGET --kind KIND --artifact FILE | --asset TARGET KIND FILE [--asset TARGET KIND FILE ...])";
fn target(value: &str) -> Result<Target> {
    match value {
        "linux-x86_64" => Ok(Target::LinuxX86_64),
        "linux-aarch64" => Ok(Target::LinuxAarch64),
        _ => Err(anyhow!("invalid draft target")),
    }
}
fn kind(value: &str) -> Result<Kind> {
    match value {
        "appimage" => Ok(Kind::AppImage),
        "tar.gz" => Ok(Kind::TarGz),
        _ => Err(anyhow!("invalid draft kind")),
    }
}
pub fn run(args: &[String]) -> Result<Manifest> {
    anyhow::ensure!(
        args.first().map(String::as_str) == Some("--draft-release"),
        "{USAGE}"
    );
    let mut flags = HashMap::new();
    let mut sources = Vec::new();
    let mut at = 1;
    while at < args.len() {
        if args[at] == "--asset" {
            anyhow::ensure!(at + 3 < args.len(), "--asset requires TARGET KIND FILE");
            anyhow::ensure!(sources.len() < 4, "draft artifact count refused");
            sources.push(DraftArtifact {
                target: target(&args[at + 1])?,
                kind: kind(&args[at + 2])?,
                source: PathBuf::from(&args[at + 3]),
            });
            at += 4;
        } else {
            anyhow::ensure!(
                matches!(
                    args[at].as_str(),
                    "--release"
                        | "--channel"
                        | "--sequence"
                        | "--target"
                        | "--kind"
                        | "--artifact"
                        | "--valid-hours"
                ),
                "unknown draft option"
            );
            anyhow::ensure!(at + 1 < args.len(), "draft option requires a value");
            anyhow::ensure!(
                flags
                    .insert(args[at].as_str(), args[at + 1].as_str())
                    .is_none(),
                "duplicate draft option"
            );
            at += 2;
        }
    }
    let get = |key| {
        flags
            .get(key)
            .copied()
            .context("required draft option is missing")
    };
    if sources.is_empty() {
        sources.push(DraftArtifact {
            target: target(get("--target")?)?,
            kind: kind(get("--kind")?)?,
            source: PathBuf::from(get("--artifact")?),
        });
    } else {
        anyhow::ensure!(
            !["--target", "--kind", "--artifact"]
                .iter()
                .any(|key| flags.contains_key(key)),
            "cannot mix --asset groups with single-artifact options"
        );
    }
    let channel = match get("--channel")? {
        "stable" => Channel::Stable,
        "next" => Channel::Next,
        _ => return Err(anyhow!("invalid draft channel")),
    };
    let sequence = get("--sequence")?
        .parse::<u64>()
        .map_err(|_| anyhow!("invalid draft sequence"))?;
    let hours = get("--valid-hours")?
        .parse::<u16>()
        .map_err(|_| anyhow!("invalid draft validity"))?;
    let policy = Policy {
        channel,
        target: sources[0].target,
        kind: sources[0].kind,
        minimum_sequence: 0,
        current_release: semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("valid package version"),
        now: chrono::Utc::now(),
    };
    release_feed::draft_many(&sources, get("--release")?, sequence, hours, &policy)
}

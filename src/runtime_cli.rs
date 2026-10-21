//! Explicit headless runtime verification/acceptance; no installer or service IO.
use crate::release_feed::{self, Channel, Kind, Policy, Target, TrustedKey};
use crate::update_settings::Product;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn run(args: &[String]) -> Result<Value> {
    if args
        .first()
        .is_some_and(|argument| argument == "--check-runtime-update")
    {
        anyhow::ensure!(
            args.len() == 3 && args[1] == "--current-runtime-version",
            "usage: --check-runtime-update --current-runtime-version VERSION"
        );
        let current =
            semver::Version::parse(&args[2]).context("invalid current runtime version")?;
        return Ok(crate::runtime_feed::check(
            &crate::update_settings::load()?,
            &current,
            std::sync::Arc::new(crate::update_control::Control::default()),
        )?
        .summary());
    }
    anyhow::ensure!(args.len()>=4,"usage: --verify-runtime-release MANIFEST SIGNATURE TRUSTED_KEY --channel stable|next --target linux-x86_64|linux-aarch64 --min-sequence N --current-runtime-version VERSION");
    let accept = match args[0].as_str() {
        "--verify-runtime-release" | "--import-runtime-release" => false,
        "--accept-runtime-release" => true,
        _ => anyhow::bail!("unsupported runtime metadata operation"),
    };
    let import = args[0] == "--import-runtime-release";
    let mut flags = HashMap::new();
    let (pairs, rest) = args[4..].as_chunks::<2>();
    anyhow::ensure!(
        rest.is_empty(),
        "runtime verification option requires a value"
    );
    for pair in pairs {
        anyhow::ensure!(
            matches!(
                pair[0].as_str(),
                "--channel"
                    | "--target"
                    | "--min-sequence"
                    | "--current-runtime-version"
                    | "--confirm-accept"
            ) || (import
                && matches!(
                    pair[0].as_str(),
                    "--artifact" | "--prefix" | "--confirm-import"
                )),
            "unsupported runtime verification option"
        );
        anyhow::ensure!(
            flags.insert(pair[0].as_str(), pair[1].as_str()).is_none(),
            "duplicate runtime verification option"
        );
    }
    let value = |name: &str| {
        flags
            .get(name)
            .copied()
            .with_context(|| format!("missing runtime policy option {name}"))
    };
    if import {
        anyhow::ensure!(
            value("--confirm-import")? == "yes",
            "runtime import requires --confirm-import yes"
        );
        value("--artifact")?;
        value("--prefix")?;
    }
    if accept {
        anyhow::ensure!(
            value("--confirm-accept")? == "yes",
            "runtime acceptance requires --confirm-accept yes"
        );
    } else {
        anyhow::ensure!(
            !flags.contains_key("--confirm-accept"),
            "read-only runtime verification cannot accept a version"
        );
    }
    let channel = match value("--channel")? {
        "stable" => Channel::Stable,
        "next" => Channel::Next,
        _ => anyhow::bail!("invalid runtime channel"),
    };
    let channel_name = match channel {
        Channel::Stable => "stable",
        Channel::Next => "next",
    };
    let policy = Policy {
        channel,
        target: match value("--target")? {
            "linux-x86_64" => Target::LinuxX86_64,
            "linux-aarch64" => Target::LinuxAarch64,
            _ => anyhow::bail!("unsupported runtime target"),
        },
        kind: Kind::TarGz,
        minimum_sequence: value("--min-sequence")?
            .parse()
            .context("invalid runtime sequence floor")?,
        current_release: semver::Version::parse(value("--current-runtime-version")?)
            .context("invalid current runtime version")?,
        now: chrono::Utc::now(),
    };
    let manifest =
        release_feed::read_input(std::path::Path::new(&args[1]), release_feed::MAX_MANIFEST)?;
    let signature =
        release_feed::read_input(std::path::Path::new(&args[2]), release_feed::MAX_AUXILIARY)?;
    let key =
        release_feed::read_input(std::path::Path::new(&args[3]), release_feed::MAX_AUXILIARY)?;
    let verified = crate::runtime_feed::verify(
        &manifest,
        &signature,
        &TrustedKey::from_json(&key)?,
        &policy,
    )?;
    let metadata = verified.manifest();
    crate::release_state::check(
        &crate::release_state::read_for(Product::Runtime)?,
        channel_name,
        metadata.sequence,
        &metadata.release,
        verified.manifest_sha256(),
    )?;
    if import {
        return Ok(crate::runtime_bundle::import(
            std::path::Path::new(value("--artifact")?),
            &verified,
            std::path::Path::new(value("--prefix")?),
        )?
        .summary());
    }
    let mut summary = verified.summary();
    if accept {
        let entry = crate::release_state::Transaction::begin_for(Product::Runtime)?.record(
            channel_name,
            metadata.sequence,
            &metadata.release,
            verified.manifest_sha256(),
        )?;
        summary["accepted"] = json!(entry);
        summary["warning"]=json!("Explicit runtime metadata acceptance only; no artifact downloaded, runtime installed, service restarted or profile accessed.");
    }
    Ok(summary)
}

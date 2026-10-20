//! CLI surface for manual profile snapshots. Restore requires an explicit
//! offline assertion plus conservative checks for Ocean/service/HTTP writers.
//! Other CLI writers must be stopped by the operator; these do not use our lock.
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::{config, http, profile_backup, systemd};

fn store() -> Result<profile_backup::ProfileStore> {
    profile_backup::ProfileStore::new(&config::dsh_home(), &config::profile_backup_dir())
}
fn offline_guard() -> Result<()> {
    anyhow::ensure!(!systemd::is_active(), "stop dsh-web.service and all DSH writers before restoring; this command does not stop them");
    let runtime = config::runtime_dir();
    let socket = runtime.join("dsh-ocean.sock");
    // Do not connect: the single-instance listener would show the GUI. Kernel
    // socket inventory distinguishes an active listener from a stale socket file.
    let sockets = fs::read_to_string("/proc/net/unix")
        .context("cannot establish whether Ocean is running")?;
    anyhow::ensure!(
        !sockets
            .lines()
            .any(|line| line.ends_with(&format!(" {}", socket.display()))),
        "quit DSH Ocean before restoring"
    );
    let mut base = config::checked_server_url()?;
    anyhow::ensure!(
        base.scheme() == "http"
            && matches!(
                base.host_str(),
                Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
            )
            && base.username().is_empty()
            && base.password().is_none()
            && base.port_or_known_default() != Some(0),
        "restore requires a configured nonzero loopback HTTP origin for its offline check"
    );
    base.set_path("/");
    base.set_query(None);
    base.set_fragment(None);
    anyhow::ensure!(
        http::status(&base).is_none(),
        "a server is still responding at the configured origin; stop it before restore"
    );
    Ok(())
}
fn summary(snapshot: &Path, manifest: &profile_backup::Manifest) -> Value {
    json!({
        "snapshot": snapshot,
        "profile": manifest.profile,
        "scope": manifest.scope,
        "consistency": manifest.consistency,
        "createdAt": manifest.created_at,
        "fileCount": manifest.files.len(),
        "totalBytes": manifest.files.iter().map(|f| f.size).sum::<u64>(),
        "excluded": manifest.excluded,
        "warning": "profile files may contain secrets; keep snapshots private. Global credentials, sessions and dependency bytes are not backed up. Checksums are not signatures."
    })
}

/// `--keep N [--profile NAME] --confirm-prune`; refuses anything else.
fn prune(args: &[String]) -> Result<Value> {
    let mut keep = None;
    let mut profile = None;
    let mut confirmed = false;
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "--confirm-prune" => confirmed = true,
            "--keep" => {
                anyhow::ensure!(keep.is_none(), "--keep given twice");
                let value = iter.next().context("--keep needs a number")?;
                keep = Some(value.parse::<usize>().context("--keep expects a number")?);
            }
            "--profile" => {
                anyhow::ensure!(profile.is_none(), "--profile given twice");
                let value = iter.next().context("--profile needs a name")?;
                profile_backup::profile_name(value)?;
                profile = Some(value.clone());
            }
            other => anyhow::bail!("unexpected prune argument: {other}"),
        }
    }
    let keep = keep.context("missing --keep N")?;
    anyhow::ensure!(
        confirmed,
        "pruning deletes snapshots; re-run with --confirm-prune"
    );
    let outcome = profile_backup::prune(
        &config::profile_backup_dir(),
        keep,
        profile.as_deref(),
        || Ok(()),
    )?;
    Ok(json!({
        "removed": outcome.removed,
        "kept": outcome.kept,
        "unreadable": outcome.unreadable,
        "warning": "deleted snapshots are not recoverable; profile files may contain secrets.",
    }))
}

pub fn run(args: &[String]) -> Result<Value> {
    match args.first().map(String::as_str) {
        Some("--backup-profile") => {
            anyhow::ensure!(args.len() <= 2, "usage: --backup-profile [NAME]");
            let name = args.get(1).map(String::as_str).unwrap_or("web");
            profile_backup::profile_name(name)?;
            let snapshot = store()?.create(name)?;
            let manifest = profile_backup::verify(&snapshot)?;
            Ok(summary(&snapshot, &manifest))
        }
        Some("--verify-profile-backup") => {
            anyhow::ensure!(args.len() == 2, "usage: --verify-profile-backup SNAPSHOT");
            let path = Path::new(&args[1]);
            Ok(summary(path, &profile_backup::verify(path)?))
        }
        Some("--restore-profile") => {
            let allow_downgrade = args.get(4).map(String::as_str) == Some("--allow-data-downgrade");
            anyhow::ensure!(
                (args.len() == 4 || (args.len() == 5 && allow_downgrade))
                    && args[3] == "--confirm-offline",
                "usage: --restore-profile NAME SNAPSHOT --confirm-offline [--allow-data-downgrade]; first stop Ocean, the web service, and ALL other DSH writers"
            );
            profile_backup::profile_name(&args[1])?;
            let prefix = config::data_dir();
            anyhow::ensure!(
                prefix.is_absolute() && prefix != Path::new("/"),
                "invalid runtime lock directory"
            );
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&prefix)?;
            let _runtime_lock = profile_backup::operation_lock(&prefix.join(".bootstrap.lock"))
                .context("runtime operation in progress or lock unavailable")?;
            offline_guard()?;
            let current_runtime = crate::bootstrap::detect().map(|runtime| runtime.version);
            let receipt = store()?.restore(
                &args[1],
                Path::new(&args[2]),
                allow_downgrade,
                current_runtime.as_deref(),
                offline_guard,
            )?;
            Ok(serde_json::to_value(receipt)?)
        }
        Some("--backup-data") => {
            anyhow::ensure!(
                args.len() <= 2,
                "usage: --backup-data [BACKUP_DIR]; default is the configured snapshot root"
            );
            let backups = match args.get(1) {
                Some(directory) => Path::new(directory).to_owned(),
                None => config::profile_backup_dir(),
            };
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&backups)
                .context("create the data snapshot directory")?;
            let home = config::dsh_home();
            let runtime_version = crate::bootstrap::detect().map(|runtime| runtime.version);
            let snapshot = crate::data_backup::snapshot(&home, &backups, runtime_version)?;
            let manifest = crate::data_backup::verify(&snapshot)?;
            Ok(json!({
                "snapshot": snapshot,
                "scope": manifest.scope,
                "roots": manifest.roots,
                "fileCount": manifest.files.len(),
                "totalBytes": manifest.total_bytes(),
                "createdAt": manifest.created_at,
                "consistent": manifest.consistent,
                "warning": "Sessions and shared stores can contain conversation content and tokens. Snapshot restore is not implemented; copy the directory back manually only while DSH is stopped.",
            }))
        }
        Some("--verify-data-backup") => {
            anyhow::ensure!(args.len() == 2, "usage: --verify-data-backup SNAPSHOT");
            let manifest = crate::data_backup::verify(Path::new(&args[1]))?;
            Ok(json!({
                "snapshot": args[1],
                "scope": manifest.scope,
                "roots": manifest.roots,
                "fileCount": manifest.files.len(),
                "totalBytes": manifest.total_bytes(),
                "warning": "Checksums are not signatures; this is a point-in-time copy, not an authenticity proof.",
            }))
        }
        Some("--restore-data") => {
            anyhow::ensure!(
                args.len() == 3 && args[2] == "--confirm-offline",
                "usage: --restore-data SNAPSHOT --confirm-offline; first stop Ocean, the web service, and ALL other DSH writers"
            );
            let prefix = config::data_dir();
            anyhow::ensure!(
                prefix.is_absolute() && prefix != Path::new("/"),
                "invalid runtime lock directory"
            );
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&prefix)?;
            let _runtime_lock = profile_backup::operation_lock(&prefix.join(".bootstrap.lock"))
                .context("runtime operation in progress or lock unavailable")?;
            offline_guard()?;
            let home = config::dsh_home();
            anyhow::ensure!(
                home.is_absolute() && home != Path::new("/"),
                "invalid DSH home"
            );
            let receipt = crate::data_backup::restore(&home, Path::new(&args[1]), offline_guard)?;
            Ok(serde_json::to_value(receipt)?)
        }
        Some("--pairing-proxy") => run_pairing_proxy(&args[1..]),
        Some("--prepare-exposure") => {
            anyhow::ensure!(args.len() <= 2, "usage: --prepare-exposure [LAN_ADDRESS]");
            let address = match args.get(1) {
                Some(value) => value
                    .parse::<std::net::IpAddr>()
                    .context("--prepare-exposure expects an IP address")?,
                None => crate::pairing_plan::detect_lan_address().context(
                    "no LAN address was detected; pass it explicitly, e.g. --prepare-exposure 192.168.1.50",
                )?,
            };
            anyhow::ensure!(
                crate::exposure_plan::acceptable_lan(address),
                "a private LAN address is required; refusing to stage an exposure candidate for {address}"
            );
            let base = config::checked_server_url()?;
            let port = config::server_port(&base);
            let staged = crate::exposure_plan::prepare(
                &config::state_dir(),
                port,
                address,
                &crate::systemd::unit_path(),
            )?;
            Ok(serde_json::to_value(staged)?)
        }
        Some("--export-backup") => {
            anyhow::ensure!(args.len() == 3, "usage: --export-backup SNAPSHOT DEST.tar");
            let receipt = crate::backup_export::export(Path::new(&args[1]), Path::new(&args[2]))?;
            Ok(json!({
                "receipt": receipt,
                "warning": "Exports are unsigned checksum-bearing tar archives: they prove integrity, not who created the snapshot. Keep them private; snapshots can contain secrets.",
            }))
        }
        Some("--verify-export") => {
            anyhow::ensure!(args.len() == 2, "usage: --verify-export ARCHIVE.tar");
            let receipt = crate::backup_export::verify_export(Path::new(&args[1]))?;
            Ok(json!({
                "receipt": receipt,
                "warning": "Integrity only, not authenticity: a re-signed archive would need a trust root this client does not have.",
            }))
        }
        Some("--import-export") => {
            anyhow::ensure!(
                args.len() == 3,
                "usage: --import-export ARCHIVE.tar DEST_DIR"
            );
            let receipt = crate::backup_export::import(Path::new(&args[1]), Path::new(&args[2]))?;
            Ok(json!({
                "receipt": receipt,
                "warning": "Imports are verified against the archive's own manifest only; integrity, not authorship. Verify the imported snapshot and keep it private.",
            }))
        }
        Some("--verify-backups") => {
            anyhow::ensure!(args.len() <= 2, "usage: --verify-backups [DIR]");
            let backups = match args.get(1) {
                Some(directory) => Path::new(directory).to_owned(),
                None => config::profile_backup_dir(),
            };
            // Retention inventories intentionally omit broken manifests. A
            // verification sweep must not: enumerate recognizable names first,
            // then report every invalid/symlinked candidate as failed.
            let root = crate::profile_backup::open_dir(&backups)
                .context("cannot open backup root for verification")?;
            let mut candidates = Vec::new();
            for name in crate::profile_backup::names_controlled(&root, &mut || Ok(()))? {
                let kind = if crate::profile_backup::snapshot_identity(&name).is_some() {
                    "profile"
                } else if crate::data_backup::sequence_of(&name).is_some() {
                    "data"
                } else {
                    continue;
                };
                candidates.push((kind, name));
                anyhow::ensure!(
                    candidates.len() <= 200,
                    "too many snapshots to verify in one pass"
                );
            }
            let mut verified = Vec::new();
            let mut failed = Vec::new();
            for (kind, name) in candidates {
                let path = backups.join(&name);
                let result = match kind {
                    "profile" => crate::profile_backup::verify(&path).map(|_| ()),
                    _ => crate::data_backup::verify(&path).map(|_| ()),
                };
                match result {
                    Ok(()) => verified.push(json!({"kind": kind, "name": name})),
                    Err(_) => failed.push(json!({"kind": kind, "name": name})),
                }
            }
            Ok(json!({
                "verified": verified,
                "failed": failed,
                "verifiedCount": verified.len(),
                "failedCount": failed.len(),
                "warning": "Read-only sweep. Nothing was deleted or moved; an unreadable snapshot is listed as failed and left in place.",
            }))
        }
        Some("--list-backup-staging") => {
            anyhow::ensure!(args.len() <= 2, "usage: --list-backup-staging [DIR]");
            let backups = match args.get(1) {
                Some(directory) => Path::new(directory).to_owned(),
                None => config::profile_backup_dir(),
            };
            let mut listed = Vec::new();
            for root in crate::backup_staging::candidate_roots(&config::dsh_home(), &backups) {
                for entry in crate::backup_staging::stale(&root, std::time::Duration::ZERO)? {
                    listed.push(serde_json::json!({
                        "root": root,
                        "name": entry.name,
                        "bytes": entry.bytes,
                        "ageSeconds": entry.age_seconds,
                        "collectable": entry.collectable,
                    }));
                }
            }
            Ok(json!({
                "staging": listed,
                "count": listed.len(),
                "warning": "Read-only. Abandoned staging copies are listed by generated name only; nothing was removed. Entries flagged collectable:false may hold the only copy of replaced data and are never deleted automatically.",
            }))
        }
        Some("--collect-backup-staging") => {
            anyhow::ensure!(
                (args.len() == 4 || args.len() == 5)
                    && args[1] == "--older-than-minutes"
                    && args[3] == "--confirm-collect",
                "usage: --collect-backup-staging --older-than-minutes N --confirm-collect [DIR]"
            );
            let backups = match args.get(4) {
                Some(directory) => Path::new(directory).to_owned(),
                None => config::profile_backup_dir(),
            };
            let minutes = args[2]
                .parse::<u64>()
                .context("--older-than-minutes expects a number")?;
            anyhow::ensure!(minutes >= 1, "use at least one minute of age");
            let age = std::time::Duration::from_secs(minutes.saturating_mul(60));
            let mut removed = Vec::new();
            let mut kept = Vec::new();
            for root in crate::backup_staging::candidate_roots(&config::dsh_home(), &backups) {
                let (gone, stay) = crate::backup_staging::collect(&root, age)?;
                removed.extend(
                    gone.into_iter()
                        .map(|name| serde_json::json!({"root": root, "name": name})),
                );
                kept.extend(
                    stay.into_iter()
                        .map(|name| serde_json::json!({"root": root, "name": name})),
                );
            }
            Ok(json!({
                "removed": removed,
                "kept": kept,
                "warning": "Only generated snapshot staging directories were removed; restore leftovers, snapshots, backups and live data were not touched.",
            }))
        }
        Some("--list-data-backups") => {
            anyhow::ensure!(args.len() <= 2, "usage: --list-data-backups [DIR]");
            let backups = match args.get(1) {
                Some(directory) => Path::new(directory).to_owned(),
                None => config::profile_backup_dir(),
            };
            let snapshots = crate::data_backup::list(&backups)?;
            Ok(json!({
                "backups": snapshots,
                "count": snapshots.len(),
                "warning": "Read-only inventory; manifests only. Sessions and stores may contain conversation content.",
            }))
        }
        Some("--prune-data-backups") => {
            anyhow::ensure!(
                (args.len() == 4 || args.len() == 5)
                    && args[1] == "--keep"
                    && args[3] == "--confirm-prune",
                "usage: --prune-data-backups --keep N --confirm-prune [DIR]"
            );
            let keep = args[2]
                .parse::<usize>()
                .context("--keep expects a number")?;
            let backups = match args.get(4) {
                Some(directory) => Path::new(directory).to_owned(),
                None => config::profile_backup_dir(),
            };
            let removed = crate::data_backup::prune(&backups, keep)?;
            Ok(json!({
                "removed": removed,
                "warning": "Removed data snapshots are unrecoverable.",
            }))
        }
        Some("--list-profile-backups") => {
            anyhow::ensure!(args.len() == 1, "usage: --list-profile-backups");
            let snapshots = profile_backup::snapshots(&config::profile_backup_dir())?;
            Ok(json!({
                "backups": snapshots,
                "count": snapshots.len(),
                "warning": "read-only inventory; snapshot profile files may contain secrets. Listing reads manifests only.",
            }))
        }
        Some("--prune-profile-backups") => prune(&args[1..]),
        _ => anyhow::bail!("unknown profile backup command"),
    }
}

/// Foreground short-lived LAN proxy for a loopback-only server.
fn run_pairing_proxy(arguments: &[String]) -> anyhow::Result<serde_json::Value> {
    let mut ttl_seconds: Option<u64> = None;
    let mut port: Option<u16> = None;
    let mut check = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--ttl-seconds" => {
                let value = arguments
                    .get(index + 1)
                    .context("--ttl-seconds expects a number")?;
                ttl_seconds = Some(value.parse().context("--ttl-seconds expects a number")?);
                index += 2;
            }
            "--port" => {
                let value = arguments
                    .get(index + 1)
                    .context("--port expects a number")?;
                port = Some(value.parse().context("--port expects a number")?);
                index += 2;
            }
            "--check" => {
                check = true;
                index += 1;
            }
            other => anyhow::bail!("unknown --pairing-proxy argument: {other}"),
        }
    }
    let ttl_seconds =
        ttl_seconds.context("usage: --pairing-proxy --ttl-seconds N [--port P] [--check]")?;
    let base = config::checked_server_url()?;
    let host = base.host_str().unwrap_or_default();
    anyhow::ensure!(
        matches!(host, "127.0.0.1" | "127.0.0.2" | "localhost" | "::1"),
        "the configured server is not loopback; a pairing proxy would expose it twice"
    );
    let target: std::net::SocketAddr = format!("127.0.0.1:{}", config::server_port(&base))
        .parse()
        .context("build the loopback target")?;
    let bind: std::net::SocketAddr = if check {
        "127.0.0.1:0".parse().expect("valid loopback bind")
    } else {
        format!("0.0.0.0:{}", port.unwrap_or(3091).max(1024))
            .parse()
            .context("build the LAN bind address")?
    };
    let ttl = std::time::Duration::from_secs(ttl_seconds);
    let mut proxy = crate::pairing_proxy::Proxy::start(bind, target, ttl)?;
    let status = proxy.status();
    if !check {
        // Operator-visible warning on stderr; stdout stays machine-readable JSON.
        eprintln!(
            "dsh-ocean: pairing proxy listening on {} until the TTL expires; Ctrl-C revokes it.\n\
             Anyone who can reach this port still needs the Harness launch token. No TLS and no\
             per-client authentication are provided.",
            status.listen
        );
        let deadline = std::time::Instant::now() + ttl;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    proxy.revoke();
    let final_status = proxy.status();
    Ok(serde_json::json!({
        "status": final_status,
        "checked": check,
        "warning": "The proxy is transparent: it adds no TLS and no authentication, and nothing is logged. The launch token remains the only access control.",
    }))
}

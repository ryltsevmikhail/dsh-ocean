//! Build a detached user-manager transaction. The worker survives closing the
//! desktop app and keeps stdout/stderr in a private file, not a breakable pipe.
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::process::Command;
use url::Url;

use crate::bootstrap::{self, Runtime};
use crate::{auth, config, systemd};

const MARKER: &str = "@@DSH_OCEAN_GENERATION@@";

pub struct UpdateTask {
    pub command: Command,
    pub log: PathBuf,
    pub unit: String,
}

pub fn prepare(runtime: &Runtime, target: &str, base: &Url) -> Result<UpdateTask> {
    prepare_at(
        runtime,
        target,
        base,
        &config::data_dir(),
        &systemd::unit_path(),
        &config::state_dir().join("updates"),
    )
}

pub fn prepare_at(
    runtime: &Runtime,
    target: &str,
    base: &Url,
    prefix: &Path,
    unit_path: &Path,
    log_dir: &Path,
) -> Result<UpdateTask> {
    prepare_with_environment(runtime, target, base, prefix, unit_path, log_dir, None)
}

/// Real-service validation only: requires an explicit test-unit namespace and a
/// fresh HOME below the private runtime prefix. No ambient credentials are kept.
pub fn prepare_isolated_at(
    runtime: &Runtime,
    target: &str,
    base: &Url,
    prefix: &Path,
    unit_path: &Path,
    log_dir: &Path,
    home: &Path,
) -> Result<UpdateTask> {
    prepare_with_environment(
        runtime,
        target,
        base,
        prefix,
        unit_path,
        log_dir,
        Some(home),
    )
}

fn prepare_with_environment(
    runtime: &Runtime,
    target: &str,
    base: &Url,
    prefix: &Path,
    unit_path: &Path,
    log_dir: &Path,
    isolated: Option<&Path>,
) -> Result<UpdateTask> {
    anyhow::ensure!(
        runtime.is_generation_managed(),
        "runtime is not generation-managed"
    );
    semver::Version::parse(target).context("invalid update version")?;
    let service_unit = unit_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid managed unit filename")?;
    anyhow::ensure!(
        unit_path.is_absolute()
            && if isolated.is_some() {
                systemd::test_unit_name(service_unit)
            } else {
                service_unit == config::UNIT_NAME
            },
        "invalid managed unit path or isolation namespace"
    );
    anyhow::ensure!(
        base.scheme() == "http"
            && base.host_str() == Some("127.0.0.1")
            && base.username().is_empty()
            && base.password().is_none()
            && base.path() == "/"
            && base.query().is_none()
            && base.fragment().is_none(),
        "generation updates require a clean loopback HTTP origin"
    );
    let prefix = fs::canonicalize(prefix).context("resolve runtime prefix")?;
    let isolated = isolated
        .map(fs::canonicalize)
        .transpose()
        .context("resolve isolated HOME")?;
    if let Some(home) = &isolated {
        anyhow::ensure!(
            home.starts_with(&prefix) && home != &prefix,
            "isolated HOME must be below the private prefix"
        );
    }
    let render = |runtime: &Runtime| -> Result<String> {
        if let Some(home) = &isolated {
            systemd::render_isolated_unit(runtime, base, home)
        } else {
            Ok(systemd::render_unit(runtime, base))
        }
    };
    let root = bootstrap::selected_private_root(&prefix)?;
    anyhow::ensure!(
        runtime.node == root.join("runtime/node/bin/node")
            && runtime.dsh == root.join("harness/node_modules/.bin/dsh"),
        "runtime selection changed; retry update"
    );
    let manifest = bootstrap::manifest()?;
    let arch = match std::env::consts::ARCH {
        "x86_64" => "linux-x64",
        "aarch64" => "linux-arm64",
        other => anyhow::bail!("unsupported architecture: {other}"),
    };
    let sha = &manifest
        .node
        .archives
        .get(arch)
        .context("missing Node checksum")?
        .sha256;
    let candidate = prefix.join("generations").join(MARKER);
    let candidate_runtime = Runtime {
        node: candidate.join("runtime/node/bin/node"),
        dsh: candidate.join("harness/node_modules/.bin/dsh"),
        version: target.to_owned(),
        install_root: Some(candidate.join("harness")),
    };
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(log_dir)
        .context("create private update log directory")?;
    let log_dir = fs::canonicalize(log_dir).context("resolve update log directory")?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let unit = format!("dsh-ocean-update-{}-{stamp}", std::process::id());
    let log = log_dir.join(format!("{unit}.log"));
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&log)
        .context("create private update log")?;

    let mut command = Command::new("systemd-run");
    command
        .args([
            "--user",
            "--wait",
            "--collect",
            "--quiet",
            "--expand-environment=no",
        ])
        .arg(format!("--unit={unit}"))
        .args([
            "--property=RuntimeMaxSec=20min",
            "--property=TimeoutStopSec=90s",
        ])
        .arg(format!(
            "--property=StandardOutput=append:{}",
            log.display()
        ))
        .arg(format!("--property=StandardError=append:{}", log.display()));
    let worker_home = isolated.clone().unwrap_or_else(config::home_dir);
    let mut env = vec![
        ("HOME", worker_home.display().to_string()),
        (
            "PATH",
            if isolated.is_some() {
                systemd::isolated_path_env(runtime)
            } else {
                bootstrap::path_env(Some(&runtime.node))
            },
        ),
        ("DSH_OCEAN_DATA_DIR", prefix.display().to_string()),
        ("DSH_OCEAN_UNIT_PATH", unit_path.display().to_string()),
        ("DSH_OCEAN_SERVICE_UNIT", service_unit.to_owned()),
        ("DSH_OCEAN_EXPECTED_ROOT", root.display().to_string()),
        ("DSH_OCEAN_OLD_UNIT", render(runtime)?),
        ("DSH_OCEAN_NEW_UNIT", render(&candidate_runtime)?),
        ("DSH_OCEAN_SERVICE_URL", base.as_str().to_owned()),
        ("DSH_NODE_VERSION", manifest.node.version),
        ("DSH_NODE_SHA256", sha.to_owned()),
        ("DSH_VERSION", target.to_owned()),
        ("DSH_PNPM_VERSION", manifest.pnpm.version),
        (
            "DSH_OCEAN_INSTALL_SCRIPT",
            bootstrap::install_script().to_owned(),
        ),
        ("DSH_OCEAN_HEALTH_JS", bootstrap::health_script().to_owned()),
        (
            "DSH_OCEAN_SERVICE_HEALTH_JS",
            include_str!("../scripts/check-service.mjs").to_owned(),
        ),
    ];
    if isolated.is_some() {
        // Only the health worker needs access to the user-manager bus; the DSH
        // service has a separate private XDG_RUNTIME_DIR in its env -i template.
        // SAFETY: getuid takes no pointers and has no side effects.
        let uid = unsafe { libc::getuid() };
        let runtime_dir =
            std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{uid}"));
        env.push(("XDG_RUNTIME_DIR", runtime_dir));
    }
    for (name, value) in &env {
        command.arg(format!("--setenv={name}={value}"));
    }
    if isolated.is_some() {
        command.args(["/usr/bin/env", "-i"]);
        for (name, value) in &env {
            command.arg(format!("{name}={value}"));
        }
    }
    command.args([
        "/bin/bash",
        "-c",
        include_str!("../scripts/update-runtime.sh"),
    ]);
    // The systemd-run waiter may disappear with the GUI; the transaction unit
    // and its file-backed output remain independently owned by the user manager.
    Ok(UpdateTask { command, log, unit })
}

pub fn log_tail(path: &Path) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let Ok(size) = file.metadata().map(|m| m.len()) else {
        return String::new();
    };
    let offset = size.saturating_sub(65536);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return String::new();
    }
    let mut data = Vec::new();
    if file.take(65536).read_to_end(&mut data).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&data);
    // Drop a truncated first line rather than risk displaying a partial token.
    let text = if offset > 0 {
        text.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
    } else {
        &text
    };
    auth::redact(
        &text
            .lines()
            .rev()
            .take(24)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

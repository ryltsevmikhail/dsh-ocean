use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{config, util};

#[derive(Clone, Debug)]
pub struct Runtime {
    pub dsh: PathBuf,
    pub node: PathBuf,
    pub version: String,
    pub install_root: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub dsh: String,
    pub node: String,
    pub version: String,
    pub target_version: String,
    pub install_root: Option<String>,
    pub ready: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeManifest {
    pub schema: u32,
    pub node: NodeManifest,
    pub dsh: DshManifest,
    pub pnpm: PnpmManifest,
}

#[derive(Clone, Debug, Deserialize)]
pub struct NodeManifest {
    pub version: String,
    pub archives: std::collections::BTreeMap<String, NodeArchive>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct NodeArchive {
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DshManifest {
    pub package: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PnpmManifest {
    pub version: String,
}

const RUNTIME_MANIFEST: &str = include_str!("../runtime-manifest.json");

impl RuntimeManifest {
    pub fn parse(raw: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_str(raw).context("parse runtime-manifest.json")?;
        anyhow::ensure!(manifest.schema == 1, "unsupported runtime manifest schema");
        anyhow::ensure!(
            manifest.dsh.package == "@deepseek-ai/dsh",
            "unexpected DSH package in runtime manifest"
        );
        semver::Version::parse(&manifest.node.version).context("invalid pinned Node version")?;
        semver::Version::parse(&manifest.dsh.version).context("invalid pinned DSH version")?;
        semver::Version::parse(&manifest.pnpm.version).context("invalid pinned pnpm version")?;
        for arch in ["linux-x64", "linux-arm64"] {
            let archive = manifest
                .node
                .archives
                .get(arch)
                .with_context(|| format!("missing Node archive for {arch}"))?;
            anyhow::ensure!(
                archive.sha256.len() == 64
                    && archive.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid Node SHA-256 for {arch}"
            );
        }
        Ok(manifest)
    }
}

pub fn manifest() -> Result<RuntimeManifest> {
    RuntimeManifest::parse(RUNTIME_MANIFEST)
}

pub fn node_archive_sha256(arch: &str) -> Option<String> {
    let manifest = manifest().ok()?;
    manifest
        .node
        .archives
        .get(arch)
        .map(|archive| archive.sha256.clone())
}

impl Runtime {
    pub fn is_generation_managed(&self) -> bool {
        self.node.ends_with("runtime/node/bin/node")
            && self.node.ancestors().nth(4).is_some_and(|root| {
                root.join(".versions").is_file()
                    && self.dsh.starts_with(root.join("harness"))
                    && root
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|name| name == "generations")
            })
    }

    pub fn info(&self) -> RuntimeInfo {
        RuntimeInfo {
            dsh: self.dsh.display().to_string(),
            node: self.node.display().to_string(),
            version: self.version.clone(),
            target_version: manifest()
                .map(|manifest| manifest.dsh.version)
                .unwrap_or_default(),
            install_root: self
                .install_root
                .as_ref()
                .map(|path| path.display().to_string()),
            ready: true,
        }
    }
}

pub fn install_script() -> &'static str {
    include_str!("../scripts/bootstrap-runtime.sh")
}

pub fn health_script() -> &'static str {
    include_str!("../scripts/check-runtime.mjs")
}

/// Resolve one immutable generation. With no selector, retain legacy layout
/// support. Broken/untrusted selectors are errors, never a silent legacy fallback.
pub fn selected_private_root(data: &Path) -> Result<PathBuf> {
    let current = data.join("current");
    match fs::symlink_metadata(&current) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(data.to_path_buf()),
        Err(error) => return Err(error).context("inspect private runtime selector"),
        Ok(metadata) => anyhow::ensure!(
            metadata.file_type().is_symlink(),
            "runtime selector is not a symlink"
        ),
    }
    let target = fs::read_link(&current).context("read private runtime selector")?;
    let components: Vec<_> = target.iter().collect();
    anyhow::ensure!(
        components.len() == 2 && components[0] == "generations",
        "invalid private runtime selector"
    );
    let name = components[1].to_string_lossy();
    anyhow::ensure!(
        name.strip_prefix("gen.").is_some_and(
            |suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        ),
        "invalid private runtime generation name"
    );
    let root = fs::canonicalize(&current).context("resolve private runtime generation")?;
    let expected_parent = fs::canonicalize(data)
        .context("resolve runtime data directory")?
        .join("generations");
    anyhow::ensure!(
        root.parent() == Some(expected_parent.as_path()),
        "runtime generation escapes its prefix"
    );
    Ok(root)
}

fn private_directory(name: &str) -> PathBuf {
    let data = config::data_dir();
    selected_private_root(&data)
        .unwrap_or_else(|_| data.join(".invalid-selection"))
        .join(name)
}

pub fn runtime_dir() -> PathBuf {
    private_directory("runtime")
}

pub fn harness_dir() -> PathBuf {
    private_directory("harness")
}

pub fn tools_dir() -> PathBuf {
    private_directory("tools")
}

pub fn path_env(node: Option<&Path>) -> String {
    let mut parts: Vec<PathBuf> = Vec::new();
    if let Some(node) = node {
        if let Some(parent) = node.parent() {
            parts.push(parent.to_path_buf());
        }
    }
    // A running server must keep its own generation's tools even if `current`
    // changes. System runtimes may use the currently selected private tools.
    let root = node
        .filter(|node| node.ends_with("runtime/node/bin/node"))
        .and_then(|node| node.ancestors().nth(4).map(Path::to_path_buf))
        .or_else(|| selected_private_root(&config::data_dir()).ok());
    if let Some(root) = root {
        parts.push(root.join("tools/node_modules/.bin"));
        parts.push(root.join("harness/node_modules/.bin"));
    }
    parts.push(config::home_dir().join(".local/share/mise/shims"));
    parts.push(config::home_dir().join(".local/bin"));
    parts.push(PathBuf::from("/usr/local/bin"));
    parts.push(PathBuf::from("/usr/bin"));
    parts.push(PathBuf::from("/bin"));
    if let Some(current) = env::var_os("PATH") {
        parts.extend(env::split_paths(&current));
    }

    let mut unique = Vec::new();
    for part in parts {
        if !part.as_os_str().is_empty() && !unique.contains(&part) {
            unique.push(part);
        }
    }
    env::join_paths(unique)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".to_string())
}

pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if util::is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn runs(candidate: &Path, version_arg: &str) -> Option<String> {
    let output = Command::new(candidate).arg(version_arg).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn find_node() -> Option<PathBuf> {
    if let Some(path) = env::var_os("DSH_OCEAN_NODE").map(PathBuf::from) {
        return (util::is_executable(&path) && runs(&path, "--version").is_some()).then_some(path);
    }

    if let Some(path) = find_in_path("node") {
        if runs(&path, "--version").is_some() {
            return Some(path);
        }
    }

    let installs = config::home_dir().join(".local/share/mise/installs/node");
    if let Ok(entries) = fs::read_dir(&installs) {
        let mut versions: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        versions.sort();
        versions.reverse();
        for version in versions {
            let candidate = version.join("bin").join("node");
            if util::is_executable(&candidate) && runs(&candidate, "--version").is_some() {
                return Some(candidate);
            }
        }
    }

    [
        PathBuf::from("/usr/bin/node"),
        PathBuf::from("/usr/local/bin/node"),
    ]
    .into_iter()
    .find(|candidate| util::is_executable(candidate) && runs(candidate, "--version").is_some())
}

pub fn find_dsh() -> Option<PathBuf> {
    let root = selected_private_root(&config::data_dir()).ok();
    find_dsh_at(root.as_deref())
}

fn find_dsh_at(private_root: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = env::var_os("DSH_OCEAN_DSH_BIN") {
        let path = PathBuf::from(path);
        return (path.is_file() && util::is_executable(&path)).then_some(path);
    }
    let mut candidates = Vec::new();
    if let Some(root) = private_root {
        candidates.push(root.join("harness/node_modules/.bin/dsh"));
    }
    candidates.push(config::home_dir().join(".local/opt/dsh/node_modules/.bin/dsh"));
    if let Some(path) = find_in_path("dsh") {
        candidates.push(path);
    }
    candidates.push(config::home_dir().join(".local/bin/dsh"));
    candidates.push(config::home_dir().join(".npm-global/bin/dsh"));

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file() && util::is_executable(candidate))
}

fn private_node_at(root: &Path) -> Option<PathBuf> {
    let candidate = root.join("runtime/node/bin/node");
    (util::is_executable(&candidate) && runs(&candidate, "--version").is_some())
        .then_some(candidate)
}

pub fn runtime_node() -> Option<PathBuf> {
    private_node_at(&selected_private_root(&config::data_dir()).ok()?)
}

/// Runs `--version` and kills the child if it outlives `limit`.
fn version_output(
    node: &Path,
    dsh: &Path,
    limit: std::time::Duration,
) -> Option<std::process::Output> {
    use std::io::Read;
    let mut child = Command::new(node)
        .arg(dsh)
        .arg("--version")
        .env("PATH", path_env(Some(node)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_end(&mut stdout);
                }
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_end(&mut stderr);
                }
                return Some(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => return None,
        }
    }
}

pub fn detect() -> Option<Runtime> {
    detect_within(std::time::Duration::from_secs(5))
}

/// Detects the runtime, giving a version probe at most `limit` before refusing.
pub fn detect_within(limit: std::time::Duration) -> Option<Runtime> {
    // Resolve current exactly once, so Node and DSH cannot straddle a switch.
    let root = selected_private_root(&config::data_dir()).ok();
    let dsh = find_dsh_at(root.as_deref())?;
    let node = if let Some(root) = root.filter(|root| dsh.starts_with(root.join("harness"))) {
        private_node_at(&root)?
    } else {
        find_node()?
    };
    let output = version_output(&node, &dsh, limit)?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout
        .lines()
        .map(str::trim)
        .find(|line| {
            let mut chars = line.chars();
            chars.next().is_some_and(|c| c.is_ascii_digit())
                && line.split('.').count() >= 3
                && line
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        })?
        .to_string();

    let components: Vec<_> = dsh.components().collect();
    let install_root = components
        .iter()
        .position(|component| component.as_os_str() == "node_modules")
        .map(|index| {
            let mut root = PathBuf::new();
            for component in &components[..index] {
                root.push(component.as_os_str());
            }
            root
        });

    Some(Runtime {
        dsh,
        node,
        version,
        install_root,
    })
}

pub fn info() -> Option<RuntimeInfo> {
    detect().map(|runtime| runtime.info())
}

pub fn npm_path(runtime: &Runtime) -> PathBuf {
    runtime
        .node
        .parent()
        .map(|parent| parent.join("npm"))
        .filter(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("npm"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_manifest_fields() {
        for (pointer, value) in [
            ("/schema", serde_json::json!(2)),
            ("/dsh/package", serde_json::json!("untrusted-package")),
            ("/node/version", serde_json::json!("latest")),
            ("/dsh/version", serde_json::json!("next")),
            ("/pnpm/version", serde_json::json!("10; malicious")),
            ("/pnpm/version", serde_json::json!("10")),
            (
                "/node/archives/linux-x64/sha256",
                serde_json::json!("g".repeat(64)),
            ),
            (
                "/node/archives/linux-arm64/sha256",
                serde_json::json!("short"),
            ),
        ] {
            let mut raw: serde_json::Value = serde_json::from_str(RUNTIME_MANIFEST).unwrap();
            *raw.pointer_mut(pointer).unwrap() = value;
            assert!(
                RuntimeManifest::parse(&raw.to_string()).is_err(),
                "accepted {pointer}"
            );
        }
        let mut raw: serde_json::Value = serde_json::from_str(RUNTIME_MANIFEST).unwrap();
        raw["node"]["archives"]
            .as_object_mut()
            .unwrap()
            .remove("linux-arm64");
        assert!(RuntimeManifest::parse(&raw.to_string()).is_err());
    }

    #[test]
    fn runtime_manifest_pins_both_linux_arches() {
        let manifest = manifest().expect("runtime manifest");
        assert_eq!(manifest.schema, 1);
        assert!(!manifest.dsh.version.is_empty());
        assert!(!manifest.pnpm.version.is_empty());
        for arch in ["linux-x64", "linux-arm64"] {
            let sha = node_archive_sha256(arch).expect("node checksum");
            assert_eq!(sha.len(), 64);
        }
    }
}

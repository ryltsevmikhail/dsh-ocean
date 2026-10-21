//! Explicit user-prefix destination authority and configured signed shell flow.
//! Environment/install hints are NOT permission to replace a package executable.
use crate::release_feed::{Kind, Staged};
use crate::update_control::Control;
use crate::{release_feed, shell_apply, update_feed, update_settings};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MARKER: &str = ".ocean-managed-shell.json";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    schema: u32,
    purpose: String,
    id: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub prefix: PathBuf,
    pub enrolled: bool,
    pub installation: Option<shell_apply::InstallationStatus>,
    pub installation_error: Option<String>,
    pub pending_recovery: bool,
    pub launcher: PathBuf,
}
pub fn prefix() -> PathBuf {
    crate::config::data_dir().join("shell")
}
pub fn launcher() -> PathBuf {
    crate::config::xdg_dir("XDG_DATA_HOME", ".local/share")
        .join("applications/dev.dsh.ocean.managed.desktop")
}
fn destination() -> Result<PathBuf> {
    let prefix = shell_apply::absolute(&prefix())?;
    shell_apply::check_ancestors(&prefix)?;
    let home = crate::config::dsh_home();
    let home = fs::canonicalize(&home).unwrap_or(home);
    anyhow::ensure!(
        !prefix.starts_with(&home) && !home.starts_with(&prefix),
        "managed shell destination must be separate from Harness home"
    );
    Ok(prefix)
}
fn owned_bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    shell_apply::check_ancestors(path.parent().context("missing managed file directory")?)?;
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "managed state must be owned regular non-writable-by-others files"
    );
    release_feed::read_input(path, limit)
}
fn marker(prefix: &Path) -> Result<Marker> {
    let metadata = fs::symlink_metadata(prefix)?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "unsafe managed destination"
    );
    let marker: Marker = serde_json::from_slice(&owned_bytes(&prefix.join(MARKER), 4096)?)
        .map_err(|_| anyhow::anyhow!("invalid managed enrollment"))?;
    anyhow::ensure!(
        marker.schema == 1
            && marker.purpose == "dsh-ocean-managed-shell"
            && !marker.id.is_empty()
            && marker.id.len() <= 128
            && marker
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "unsupported managed enrollment"
    );
    Ok(marker)
}
fn layout(prefix: &Path) -> Result<()> {
    for entry in fs::read_dir(prefix)? {
        let entry = entry?;
        let name = entry.file_name();
        anyhow::ensure!(
            matches!(
                name.to_str(),
                Some(
                    MARKER
                        | "versions"
                        | "current"
                        | ".previous"
                        | ".activation-pending.json"
                        | ".shell-update.lock"
                )
            ),
            "unknown existing destination contents; not adopting or overwriting them"
        );
    }
    Ok(())
}
fn authority() -> Result<(PathBuf, Marker)> {
    let prefix = destination()?;
    let marker = marker(&prefix).context("managed destination is not explicitly enrolled")?;
    layout(&prefix)?;
    Ok((prefix, marker))
}
pub fn status() -> Result<Status> {
    let prefix = destination()?;
    let present = match fs::symlink_metadata(prefix.join(MARKER)) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e).context("inspect managed enrollment"),
    };
    let mut status = Status {
        prefix: prefix.clone(),
        enrolled: false,
        installation: None,
        installation_error: None,
        pending_recovery: false,
        launcher: launcher(),
    };
    if !present {
        return Ok(status);
    }
    marker(&prefix)?;
    layout(&prefix)?;
    status.enrolled = true;
    status.pending_recovery = fs::symlink_metadata(prefix.join(".activation-pending.json")).is_ok();
    match shell_apply::inspect(&prefix) {
        Ok(installation) => status.installation = Some(installation),
        Err(error) => status.installation_error = Some(crate::auth::redact(&format!("{error:#}"))),
    }
    Ok(status)
}
/// This is the explicit opt-in. No key, remote manifest, profile or original
/// executable is inferred/modified. Unknown existing destination contents refuse.
pub fn enroll() -> Result<Status> {
    let prefix = destination()?;
    match fs::symlink_metadata(&prefix) {
        Ok(_) => {
            // Validate BEFORE creating even a lock in an unknown directory.
            let metadata = fs::symlink_metadata(&prefix)?;
            anyhow::ensure!(
                metadata.is_dir()
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.mode() & 0o022 == 0,
                "unsafe managed destination"
            );
            layout(&prefix)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => shell_apply::ensure_prefix(&prefix)?,
        Err(e) => return Err(e).context("inspect destination before enrollment"),
    }
    let _lock = crate::profile_backup::operation_lock(&prefix.join(".shell-update.lock"))?;
    layout(&prefix)?;
    if fs::symlink_metadata(prefix.join(MARKER)).is_ok() {
        marker(&prefix)?;
        return status();
    }
    anyhow::ensure!(
        fs::symlink_metadata(prefix.join(".activation-pending.json"))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "recover the interrupted operator installation before enrollment"
    );
    let installation = shell_apply::inspect(&prefix)?;
    if installation.current.is_none() {
        anyhow::ensure!(
            installation.previous.is_none(),
            "unknown retained installation cannot be adopted"
        );
        if fs::symlink_metadata(prefix.join("versions")).is_ok() {
            shell_apply::check_ancestors(&prefix.join("versions"))?;
            anyhow::ensure!(
                fs::read_dir(prefix.join("versions"))?.next().is_none(),
                "unselected existing versions require operator investigation"
            );
        }
    }
    let marker = Marker {
        schema: 1,
        purpose: "dsh-ocean-managed-shell".into(),
        id: crate::downloads::unique_id(),
    };
    shell_apply::write_json_new(&prefix.join(MARKER), &marker)?;
    status()
}
fn desktop_exec(path: &Path) -> Result<String> {
    let raw = path
        .to_str()
        .context("managed launcher needs a UTF-8 path")?;
    anyhow::ensure!(
        path.is_absolute() && !raw.chars().any(char::is_control),
        "invalid managed launcher path"
    );
    // Same two-layer quoting as the audited package installer: Desktop Exec
    // argument quoting, then Desktop string-value escaping; %% is literal %.
    let raw = raw.replace('%', "%%");
    let mut argument = String::new();
    for character in raw.chars() {
        if matches!(character, '\\' | '"' | '$' | '`') {
            argument.push('\\');
        }
        argument.push(character);
    }
    Ok(format!("\"{argument}\"").replace('\\', "\\\\"))
}
fn desktop(prefix: &Path, marker: &Marker) -> Result<Vec<u8>> {
    Ok(format!("[Desktop Entry]\nType=Application\nName=DSH Ocean (managed)\nComment=Explicit managed user installation; original package is unchanged\nExec={}\nIcon=dsh-ocean\nTerminal=false\nCategories=Development;\nX-DSH-Ocean-Managed-ID={}\n",desktop_exec(&prefix.join("current/dsh-ocean"))?,marker.id).into_bytes())
}
fn launcher_preflight(prefix: &Path, marker: &Marker) -> Result<Vec<u8>> {
    let expected = desktop(prefix, marker)?;
    let path = launcher();
    shell_apply::check_ancestors(path.parent().context("launcher has no directory")?)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => anyhow::ensure!(
            owned_bytes(&path, 16384)? == expected,
            "existing managed launcher is not ours; refusing overwrite"
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e).context("inspect managed launcher"),
    }
    Ok(expected)
}
fn create_launcher(expected: &[u8]) -> Result<()> {
    let path = launcher();
    if fs::symlink_metadata(&path).is_ok() {
        anyhow::ensure!(
            owned_bytes(&path, 16384)? == expected,
            "launcher changed; refusing overwrite"
        );
        return Ok(());
    }
    let parent = path.parent().context("launcher has no directory")?;
    shell_apply::ensure_prefix(parent)?;
    let temporary = parent.join(format!(".ocean-launcher-{}", crate::downloads::unique_id()));
    let _guard = crate::checked_archive::NewDirectory::create(&temporary)?;
    let source = temporary.join("entry.desktop");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&source)?;
    file.write_all(expected)?;
    file.sync_all()?;
    shell_apply::publish_new(&source, &path)?;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(parent)?
        .sync_all()?;
    Ok(())
}

pub struct CachedArtifact {
    _directory: crate::checked_archive::NewDirectory,
    staged: Staged,
    manifest: String,
    artifact: String,
    kind: Kind,
}
pub fn download(
    candidate: &update_feed::ShellCandidate,
    control: Arc<Control>,
) -> Result<CachedArtifact> {
    control.check()?;
    let settings = update_settings::load()?;
    let verified = candidate.revalidate(&settings)?;
    let parent = crate::config::state_dir().join("shell-downloads");
    shell_apply::ensure_prefix(&parent)?;
    let directory = parent.join(format!("download-{}", crate::downloads::unique_id()));
    let guard = crate::checked_archive::NewDirectory::create(&directory)?;
    let staged = crate::release_download::download_controlled(
        &verified,
        &directory.join("artifact"),
        control,
    )?;
    candidate.revalidate(&update_settings::load()?)?;
    Ok(CachedArtifact {
        _directory: guard,
        staged,
        manifest: verified.manifest_sha256().to_owned(),
        artifact: verified.artifact().sha256.clone(),
        kind: verified.artifact().kind,
    })
}
pub fn apply(
    candidate: &update_feed::ShellCandidate,
    cached: &CachedArtifact,
    control: &Control,
) -> Result<Value> {
    control.check()?;
    let settings = update_settings::Guard::begin()?;
    let (prefix, marker) = authority()?;
    let expected = launcher_preflight(&prefix, &marker)?;
    let verified = candidate.revalidate(settings.settings())?;
    anyhow::ensure!(
        cached.manifest == verified.manifest_sha256()
            && cached.artifact == verified.artifact().sha256
            && cached.kind == verified.artifact().kind,
        "cached artifact does not belong to this candidate"
    );
    let installation = shell_apply::inspect(&prefix)?;
    anyhow::ensure!(
        !installation.pending_recovery,
        "recover the interrupted managed transaction first"
    );
    if let Some(current) = installation.current {
        anyhow::ensure!(
            current.kind == verified.artifact().kind,
            "managed format conversion is not an ordinary update"
        );
        anyhow::ensure!(semver::Version::parse(&verified.manifest().release)? >= semver::Version::parse(&current.release)?, "refusing managed disk-version downgrade; use explicit retained rollback or operator reinstallation");
    }
    control.begin_commit()?;
    let (receipt, entry) = shell_apply::apply(cached.staged.path(), &verified, &prefix)?;
    let launcher_error = create_launcher(&expected)
        .err()
        .map(|error| crate::auth::redact(&format!("{error:#}")));
    Ok(
        json!({"installedOnDisk":true,"release":entry.release,"sequence":entry.sequence,"apply":receipt,"launcher":launcher(),"launcherCreated":launcher_error.is_none(),"launcherError":launcher_error,"serviceRestarted":false,"warning":"Running client and original installation are unchanged. Quit this client, then open DSH Ocean (managed) to use the selected version."}),
    )
}
pub fn rollback(control: &Control) -> Result<Value> {
    control.check()?;
    let (prefix, _) = authority()?;
    control.begin_commit()?;
    shell_apply::rollback(&prefix)
}
pub fn recover(control: &Control) -> Result<Value> {
    control.check()?;
    let (prefix, _) = authority()?;
    control.begin_commit()?;
    shell_apply::recover(&prefix)
}
pub fn install_latest(kind: Kind) -> Result<Value> {
    let (prefix, marker) = authority()?;
    launcher_preflight(&prefix, &marker)?;
    let settings = update_settings::load()?;
    let candidate = update_feed::check_shell(&settings, kind)?;
    let cached = download(&candidate, Arc::new(Control::default()))?;
    apply(&candidate, &cached, &Control::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launcher_uses_desktop_not_shell_quoting() {
        assert_eq!(
            desktop_exec(Path::new("/safe/space name")).unwrap(),
            "\"/safe/space name\""
        );
        assert_eq!(
            desktop_exec(Path::new("/safe/percent%f")).unwrap(),
            "\"/safe/percent%%f\""
        );
        assert!(desktop_exec(Path::new("/bad/new\nline")).is_err());
        assert!(desktop_exec(Path::new("relative")).is_err());
    }
}

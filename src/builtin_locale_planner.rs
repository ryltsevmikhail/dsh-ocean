//! Native locale planning uses the actual launch environment, not configuration
//! fallbacks which could silently select a different shared SDK profile.
use anyhow::{Context, Result};
use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchScope {
    pub process_home: PathBuf,
    pub dsh_home: PathBuf,
    pub cwd: PathBuf,
}
fn absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.as_os_str().len() <= 4096
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
}
pub fn selected_sdk(runtime: &crate::bootstrap::Runtime) -> Result<(PathBuf, String)> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        name: String,
        version: String,
        #[serde(default)]
        bin: serde_json::Value,
    }
    anyhow::ensure!(
        absolute(&runtime.dsh),
        "absolute selected SDK executable required"
    );
    let mut executable =
        std::fs::canonicalize(&runtime.dsh).context("resolve selected SDK executable")?;
    // The authenticated runtime packager replaces JS-entry symlinks with this
    // exact in-place Node shim. Never interpret arbitrary executable contents.
    let packaged_shim = executable.ends_with("node_modules/.bin/dsh");
    if packaged_shim {
        const SHIM: &str = "#!/usr/bin/env node\nconst path=require('node:path');const entry=path.resolve(__dirname,\"../@deepseek-ai/dsh/lib/bin.js\");if(typeof process.execve!=='function')throw new Error('pinned Node execve required');process.execve(process.execPath,[process.execPath,entry,...process.argv.slice(2)],process.env);\n";
        anyhow::ensure!(
            crate::release_feed::read_input(&executable, 4096)? == SHIM.as_bytes(),
            "unrecognized selected SDK executable shim"
        );
        let target = executable
            .parent()
            .and_then(Path::parent)
            .context("selected SDK node_modules directory")?
            .join("@deepseek-ai/dsh/lib/bin.js");
        let resolved = std::fs::canonicalize(&target).context("resolve packaged SDK entry")?;
        anyhow::ensure!(
            resolved == target && resolved.is_file(),
            "ambiguous packaged SDK entry"
        );
        executable = resolved;
    }
    for parent in executable.ancestors().skip(1).take(16) {
        let anchor = parent.join("package.json");
        match std::fs::symlink_metadata(&anchor) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        let manifest: Manifest =
            serde_json::from_slice(&crate::release_feed::read_input(&anchor, 1024 * 1024)?)
                .context("read selected SDK manifest")?;
        anyhow::ensure!(
            manifest.name == "@deepseek-ai/dsh",
            "selected executable belongs to a different SDK package"
        );
        if packaged_shim {
            anyhow::ensure!(
                manifest.version == runtime.version
                    && matches!(
                        manifest.bin.get("dsh").and_then(serde_json::Value::as_str),
                        Some("lib/bin.js" | "./lib/bin.js")
                    )
                    && parent.join("lib/bin.js") == executable,
                "packaged SDK manifest does not match selected runtime entry"
            );
        }
        return Ok((anchor, manifest.version));
    }
    anyhow::bail!("selected SDK package manifest not found")
}
/// Native approval is deliberately outside webview IPC. A refusal or dismissed
/// prompt is an ordinary plain launch; only a matching persisted grant can plan.
pub async fn prepare(
    app: &tauri::AppHandle,
    runtime: &crate::bootstrap::Runtime,
    permit: &crate::state::RuntimePermit,
    launch: &LaunchScope,
) -> Result<Option<PathBuf>> {
    permit.ensure_open()?;
    crate::runtime_switch::ensure_no_pending(&crate::config::data_dir())?;
    launch.verify()?;
    let (_, version) = selected_sdk(runtime)?;
    let scope =
        crate::builtin_locale_consent::Scope::new(&launch.dsh_home.join("profiles/web"), &version)?;
    if !crate::builtin_locale_ui::request(app, &scope, permit).await? {
        return Ok(None);
    }
    recorded_plan(
        runtime,
        permit,
        launch,
        &crate::config::data_dir().join("builtin-locales"),
        &crate::builtin_locale_ui::preference_path(),
    )
    .await
}

/// Require the real profile/SDK permission record; there is no environment or
/// boolean override. The caller holds owned launch admission for this prefix.
pub async fn recorded_plan(
    runtime: &crate::bootstrap::Runtime,
    permit: &crate::state::RuntimePermit,
    launch: &LaunchScope,
    cache: &Path,
    consent: &Path,
) -> Result<Option<PathBuf>> {
    permit.ensure_open()?;
    crate::runtime_switch::ensure_no_pending(cache.parent().context("locale cache parent")?)?;
    launch.verify()?;
    let (anchor, version) = selected_sdk(runtime)?;
    let profile = launch.dsh_home.join("profiles/web");
    let scope = crate::builtin_locale_consent::Scope::new(&profile, &version)?;
    if crate::builtin_locale_consent::load(consent, &scope)? != Some(true) {
        return Ok(None);
    }
    let tree = crate::builtin_locale_cache::materialize(cache)?;
    let request = crate::builtin_locale_worker::Request {
        install_anchor: anchor,
        profile_dir: profile,
        home: launch.dsh_home.clone(),
        cwd: launch.cwd.clone(),
        telemetry_disabled_env: std::env::var("DSH_TELEMETRY_DISABLED").ok(),
    };
    let presence = crate::builtin_locale_worker::inspect(
        &runtime.node,
        &tree.join("builtin-locale-sdk-preflight.mjs"),
        &request,
        &launch.process_home,
    )
    .await?;
    permit.ensure_open()?;
    launch.verify()?;
    crate::runtime_switch::ensure_no_pending(cache.parent().context("locale cache parent")?)?;
    // A read-only metadata subprocess may outlive a user changing the private
    // grant. Never turn its earlier approval snapshot into a new launch recipe.
    if crate::builtin_locale_consent::load(consent, &scope)? != Some(true) {
        return Ok(None);
    }
    Ok((presence == crate::builtin_locale_worker::Presence::Absent)
        .then(|| tree.join("overlay.yml")))
}
impl LaunchScope {
    pub fn verify(&self) -> Result<()> {
        crate::util::check_owned_ancestors(&self.process_home)?;
        crate::util::check_owned_ancestors(&self.cwd)?;
        crate::util::check_owned_ancestors(&self.dsh_home.join("profiles/web"))
    }
    pub fn checked(
        process_home: PathBuf,
        dsh_home: Option<OsString>,
        cwd: PathBuf,
    ) -> Result<Self> {
        let dsh_home = dsh_home
            .map(PathBuf::from)
            .unwrap_or_else(|| process_home.join(".dsh"));
        anyhow::ensure!(
            [process_home.as_path(), dsh_home.as_path(), cwd.as_path()]
                .into_iter()
                .all(absolute),
            "absolute unambiguous native locale launch scope required"
        );
        Ok(Self {
            process_home,
            dsh_home,
            cwd,
        })
    }
    pub fn child() -> Result<Self> {
        let home = match std::env::var_os("HOME") {
            Some(value) => PathBuf::from(value),
            None => crate::config::account_home().context("resolve actual launch home")?,
        };
        Self::checked(home, std::env::var_os("DSH_HOME"), std::env::current_dir()?)
    }
}

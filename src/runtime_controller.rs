//! Owned runtime Controller adapters for the durable switch engine.
//!
//! A controller proves it owns the live runtime — either a managed child handle
//! leased from the runtime permit, or the exact generated `dsh-web.service` unit
//! plus its manager provenance — before the engine stops, snapshots, switches and
//! restarts it. Attached servers and loopback/path heuristics are never authority:
//! an owned switch requires the actual `ServerMode::Child` handle or a proven,
//! exactly-generated systemd unit.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tauri::{AppHandle, Manager};
use url::Url;

use crate::bootstrap::Runtime;
use crate::managed_child;
use crate::runtime_switch::{Controller, UnitImage};
use crate::service::{self, Server, ServerMode};
use crate::state::RuntimePermit;
use crate::{auth, config, systemd};

/// Owns a runtime's lifecycle for the whole switch-engine run. The runtime
/// permit is retained inside so lifecycle exclusion lasts across health,
/// snapshot, switch and rollback, and is exposed via [`OwnedController::permit`]
/// for the final service window refresh.
pub struct OwnedController {
    app: Option<AppHandle>,
    handle: tauri::async_runtime::RuntimeHandle,
    permit: RuntimePermit,
    mode: ServerMode,
    base: Url,
    old: Runtime,
    candidate: PathBuf,
    old_locale_patch: Option<PathBuf>,
    candidate_locale_patch: Option<PathBuf>,
    active_locale_patch: Option<PathBuf>,
    expected_home: PathBuf,
    old_root: Option<PathBuf>,
    old_unit: Option<Vec<u8>>,
    new_unit: Option<Vec<u8>>,
    unit_path: PathBuf,
    unit_name: String,
    /// Recovery mode proves ownership against the journal's captured before/after
    /// unit images instead of re-rendering the OLD runtime's unit, so a recovery
    /// that begins after the unit was already switched to the candidate is still
    /// provable. `start`/`check_unit`/`switch_unit` use the captured images.
    recovery: bool,
}

/// Resolve a path to its canonical form, failing closed on symlink ambiguity or
/// a missing path rather than silently comparing a raw path that may alias a
/// different runtime root.
fn canonical(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).with_context(|| format!("resolve {}", path.display()))
}

/// Compare two absolute paths, resolving symlinks when both exist. A lexical
/// match is enough when the (possibly not-yet-created) directories do not exist.
fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// Parse a raw `show-environment` value as a plain absolute path. Shell quoting,
/// escaping, an empty value, or a relative path are all refused with a static
/// message: the value is never shell-evaluated and never echoed, so a quoted or
/// synthetic value cannot leak into an error.
fn parse_plain_absolute(value: &[u8], what: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        !value.is_empty(),
        "the user manager {what} is empty; refusing an owned systemd switch"
    );
    anyhow::ensure!(
        value[0] == b'/',
        "the user manager {what} is not an absolute path; refusing an owned systemd switch"
    );
    anyhow::ensure!(
        value.iter().all(|&b| {
            b >= 0x21
                && b != 0x7f
                && b != b'"'
                && b != b'\''
                && b != b'\\'
                && b != b'$'
                && b != b'`'
        }),
        "the user manager {what} is quoted or escaped; refusing an owned systemd switch"
    );
    Ok(PathBuf::from(OsString::from_vec(value.to_vec())))
}

/// Resolve the manager's effective `HOME`: a single plain absolute `HOME=`
/// value, or the passwd login home when `HOME` is absent. A duplicate or
/// unparseable `HOME` fails closed.
fn resolve_manager_home(env: &systemd::ManagerEnvironment) -> Result<PathBuf> {
    match env.home.as_slice() {
        [] => config::account_home()
            .context("cannot resolve the login home that the systemd unit (%h) uses"),
        [value] => parse_plain_absolute(value, "HOME"),
        _ => anyhow::bail!(
            "the user manager environment defines HOME more than once; refusing an owned systemd switch"
        ),
    }
}

/// Resolve the manager's effective `DSH_HOME`: a single plain absolute
/// `DSH_HOME=` value, or `<HOME>/.dsh` when `DSH_HOME` is absent. A duplicate or
/// unparseable `DSH_HOME` fails closed.
fn resolve_manager_dsh_home(env: &systemd::ManagerEnvironment, home: &Path) -> Result<PathBuf> {
    match env.dsh_home.as_slice() {
        [] => Ok(home.join(".dsh")),
        [value] => parse_plain_absolute(value, "DSH_HOME"),
        _ => anyhow::bail!(
            "the user manager environment defines DSH_HOME more than once; refusing an owned systemd switch"
        ),
    }
}

/// Prove the snapshot scope matches the directory the production systemd unit
/// will actually write to. The production unit sets `WorkingDirectory=%h` and
/// pins neither `HOME` nor `DSH_HOME`, so systemd runs the service under the
/// *user manager's* environment; the effective data home is the manager's
/// `DSH_HOME`, or `<manager HOME>/.dsh` when it is unset. That manager scope is
/// independent of the client's own `HOME`/`DSH_HOME`, so it is read directly
/// from `systemctl --user show-environment` (filtered to only `HOME=`/`DSH_HOME=`,
/// never shell-evaluated). Anything that cannot be proven — a query failure,
/// timeout, oversized block, or a quoted/duplicate/relative/empty value — is
/// refused (fail closed) instead of silently snapshotting the wrong environment.
fn validate_systemd_scope(expected_home: &Path) -> Result<()> {
    // Round1 strict refusal preserved: a client DSH_HOME override makes the
    // snapshot scope ambiguous and is refused before any manager query.
    if std::env::var_os("DSH_HOME").is_some() {
        anyhow::bail!(
            "DSH_HOME is set, so the systemd service data home cannot be proven to \
             match the snapshot scope; unset DSH_HOME (or use a managed-child server) \
             before an owned switch"
        );
    }
    let env = systemd::show_environment()?;
    let home = resolve_manager_home(&env)?;
    let service_home = resolve_manager_dsh_home(&env, &home)?;
    anyhow::ensure!(
        same_path(expected_home, &service_home),
        "the user manager's effective service data home does not match the snapshot scope; \
         refusing an owned systemd switch"
    );
    Ok(())
}

fn generation_root(runtime: &Runtime) -> Result<PathBuf> {
    anyhow::ensure!(
        runtime.is_generation_managed(),
        "the running runtime is not generation-managed; refusing to own it"
    );
    let root = runtime
        .node
        .ancestors()
        .nth(4)
        .context("runtime generation root is missing")?
        .to_path_buf();
    anyhow::ensure!(
        runtime.dsh == root.join("harness/node_modules/.bin/dsh"),
        "runtime dsh lives outside its generation"
    );
    Ok(root)
}

fn generation_runtime(root: &Path) -> Runtime {
    Runtime {
        dsh: root.join("harness/node_modules/.bin/dsh"),
        node: root.join("runtime/node/bin/node"),
        version: String::new(),
        install_root: Some(root.join("harness")),
    }
}

impl OwnedController {
    /// Set an already admitted candidate recipe before ownership/stop begins.
    /// The captured old recipe is immutable, including when candidate is plain.
    pub fn with_candidate_locale_patch(mut self, patch: Option<PathBuf>) -> Result<Self> {
        self.permit.ensure_open()?;
        anyhow::ensure!(
            !self.recovery && self.old_root.is_none(),
            "candidate locale recipe must be planned before the switch"
        );
        service::managed_arguments(patch.as_deref())?;
        self.candidate_locale_patch = patch;
        Ok(self)
    }
    fn locale_patch_for_root(&self, root: &Path) -> Result<Option<PathBuf>> {
        let root = canonical(root)?;
        if root == canonical(&self.candidate)? {
            Ok(self.candidate_locale_patch.clone())
        } else if Some(root.as_path()) == self.old_root.as_deref() {
            Ok(self.old_locale_patch.clone())
        } else {
            anyhow::bail!("unknown runtime root requested to start")
        }
    }
    /// Production constructor: keeps `app` for the final window reload and runs
    /// on a dedicated blocking worker, never the GTK/Tokio core thread.
    ///
    /// `expected_home` is the exact data-home scope the engine will snapshot and
    /// restore (`runtime_switch::Paths.home`). For a systemd-owned switch it must
    /// equal the directory the production unit actually writes to; a mismatch is
    /// refused before any switch is attempted.
    pub fn owned(
        app: AppHandle,
        permit: RuntimePermit,
        old: Runtime,
        candidate: PathBuf,
        expected_home: PathBuf,
    ) -> Result<Self> {
        Self::construct(Some(app), permit, old, candidate, expected_home)
    }

    /// Headless worker/tests: identical owned lifecycle without a window reload.
    /// `finish()` is a no-op without a window (the fresh token is already
    /// published and cached during `start`/`health`).
    pub fn headless(
        permit: RuntimePermit,
        old: Runtime,
        candidate: PathBuf,
        expected_home: PathBuf,
    ) -> Result<Self> {
        Self::construct(None, permit, old, candidate, expected_home)
    }

    /// Explicit-recovery constructor (production): the OLD runtime is derived
    /// from the pending journal's `old` generation (never `bootstrap::detect`),
    /// and ownership is proven against the journal's captured before/after unit
    /// images. `unit` is `None` for a managed child (which still refuses a fresh
    /// session that no longer owns a child handle).
    pub fn owned_recover(
        app: AppHandle,
        permit: RuntimePermit,
        old_root: PathBuf,
        candidate: PathBuf,
        expected_home: PathBuf,
        unit: Option<UnitImage>,
    ) -> Result<Self> {
        Self::construct_recover(Some(app), permit, old_root, candidate, expected_home, unit)
    }

    /// Headless/tests recovery constructor, same authority as [`Self::owned_recover`].
    pub fn headless_recover(
        permit: RuntimePermit,
        old_root: PathBuf,
        candidate: PathBuf,
        expected_home: PathBuf,
        unit: Option<UnitImage>,
    ) -> Result<Self> {
        Self::construct_recover(None, permit, old_root, candidate, expected_home, unit)
    }

    fn construct_recover(
        app: Option<AppHandle>,
        permit: RuntimePermit,
        old_root: PathBuf,
        candidate: PathBuf,
        expected_home: PathBuf,
        unit: Option<UnitImage>,
    ) -> Result<Self> {
        let old = generation_runtime(&old_root);
        if unit.is_some() {
            // Systemd recovery is unit-based and cross-session: a fresh session
            // has no published server (the pending gate forbids starting one,
            // and the health monitor skips while pending), but the journal's
            // captured before/after unit images are sufficient authority once
            // the exact fragment, no drop-ins and the manager scope are proven.
            // Reconstruct the fixed loopback origin from configuration and
            // publish the fresh launch token only after the recovered OLD root
            // starts and proves authenticated health; a remote origin cannot
            // own the local unit.
            let base = config::checked_server_url()?;
            anyhow::ensure!(
                service::permits_local_start(&base),
                "an owned systemd recovery requires a loopback configured origin; \
                 a remote origin cannot own the local unit"
            );
            anyhow::ensure!(
                base.host_str() == Some("127.0.0.1"),
                "owned systemd recovery requires the fixed loopback unit origin"
            );
            validate_systemd_scope(&expected_home)?;
            let mut controller = Self {
                app,
                handle: tauri::async_runtime::handle(),
                permit,
                mode: ServerMode::Systemd,
                old_locale_patch: None,
                candidate_locale_patch: None,
                active_locale_patch: None,
                base,
                old,
                candidate,
                expected_home,
                old_root: None,
                old_unit: None,
                new_unit: None,
                unit_path: systemd::unit_path(),
                unit_name: config::UNIT_NAME.to_string(),
                recovery: true,
            };
            controller.old_unit = unit.as_ref().map(|image| image.before.clone());
            controller.new_unit = unit.as_ref().map(|image| image.after.clone());
            return Ok(controller);
        }
        // Managed-child recovery is same-session only: it needs a live owned
        // child handle. A fresh session (no published server) must refuse with
        // clear guidance rather than a misleading "no running server" or a PID
        // reconstructed from a path.
        if permit.server()?.is_none() {
            anyhow::bail!(
                "a managed-child runtime can only be recovered in the same session \
                 that started it; this fresh session no longer owns the stopped child, \
                 so it cannot be recovered safely. Start or reinstall DSH with the \
                 original package manager; the pending transaction is retained."
            );
        }
        let mut controller = Self::construct(app, permit, old, candidate, expected_home)?;
        controller.recovery = true;
        Ok(controller)
    }

    fn construct(
        app: Option<AppHandle>,
        permit: RuntimePermit,
        old: Runtime,
        candidate: PathBuf,
        expected_home: PathBuf,
    ) -> Result<Self> {
        let server = permit
            .server()?
            .context("there is no running server to own")?;
        anyhow::ensure!(
            server.mode != ServerMode::Attached,
            "an attached server is not owned by this client; refusing to switch it"
        );
        anyhow::ensure!(
            service::permits_local_start(&server.base_url),
            "an owned switch requires a loopback HTTP origin"
        );
        anyhow::ensure!(
            old.is_generation_managed(),
            "the running runtime is not generation-managed; migrate it before an owned switch"
        );
        anyhow::ensure!(
            candidate.is_absolute(),
            "candidate runtime root must be absolute"
        );
        let base = server.base_url.clone();
        if server.mode == ServerMode::Systemd {
            anyhow::ensure!(
                base.host_str() == Some("127.0.0.1"),
                "owned systemd switch requires the fixed loopback unit origin"
            );
            anyhow::ensure!(
                server.unit.as_deref() == Some(config::UNIT_NAME),
                "unexpected systemd unit name"
            );
            validate_systemd_scope(&expected_home)?;
        } else {
            anyhow::ensure!(
                permit.owned_server()?.is_some(),
                "no owned managed child to switch"
            );
        }
        Ok(Self {
            app,
            handle: tauri::async_runtime::handle(),
            permit,
            mode: server.mode,
            old_locale_patch: server.locale_patch.clone(),
            candidate_locale_patch: server.locale_patch.clone(),
            active_locale_patch: server.locale_patch.clone(),
            base,
            old,
            candidate,
            expected_home,
            old_root: None,
            old_unit: None,
            new_unit: None,
            unit_path: systemd::unit_path(),
            unit_name: config::UNIT_NAME.to_string(),
            recovery: false,
        })
    }

    pub fn permit(&self) -> &RuntimePermit {
        &self.permit
    }

    /// Publish the fresh server/token and reload the window after a successful
    /// apply or rollback. The engine already published/cached the fresh token
    /// during `start`/`health`; this navigates the window and, for systemd,
    /// re-resolves the freshest launch token before reloading. Without a window
    /// (`headless`) it is a no-op: the token is already cached from `start`.
    pub fn finish(&self) -> Result<()> {
        let server = self.permit.server()?.context("no server to reload")?;
        anyhow::ensure!(
            server.mode != ServerMode::Attached,
            "an attached server is not owned"
        );
        let Some(app) = &self.app else {
            return Ok(());
        };
        match self.mode {
            ServerMode::Systemd => {
                let base = server.base_url.clone();
                self.handle
                    .block_on(service::reload_systemd_server(app, &base, &self.permit))
            }
            ServerMode::Child => {
                self.handle
                    .block_on(service::reload_window(app, &self.permit, server, false))
            }
            ServerMode::Attached => unreachable!("attached refused at construction"),
        }
    }

    fn read_unit(&self) -> Result<Vec<u8>> {
        systemd::read_owned_unit(&self.unit_path)?.context("managed unit file is missing")
    }

    fn verify_provenance(&self) -> Result<()> {
        let (fragment, drop_ins) = systemd::unit_provenance(&self.unit_name)?;
        anyhow::ensure!(
            fragment == self.unit_path,
            "active unit is not the managed unit file"
        );
        anyhow::ensure!(
            drop_ins.is_empty(),
            "custom unit drop-ins are not supported by an owned switch"
        );
        Ok(())
    }

    /// Prove the on-disk unit is the exact generated unit for the captured OLD
    /// runtime, and that the manager resolves exactly that fragment with no
    /// drop-ins. This is the only systemd ownership authority. In recovery mode
    /// the on-disk unit may already be the candidate image, so the captured
    /// before/after images from the journal are the authority instead of a fresh
    /// render of the old runtime.
    fn prove_unit_owned(&mut self) -> Result<()> {
        let on_disk = self.read_unit()?;
        if self.recovery {
            let old = self
                .old_unit
                .clone()
                .context("recovery unit image not captured")?;
            let new = self
                .new_unit
                .clone()
                .context("recovery unit image not captured")?;
            anyhow::ensure!(
                on_disk == old || on_disk == new,
                "the managed unit does not match the pending transaction images; recovery refused"
            );
            self.verify_provenance()?;
            return Ok(());
        }
        anyhow::ensure!(
            systemd::matches_launch_unit(
                &self.old,
                &self.base,
                self.old_locale_patch.as_deref(),
                &on_disk
            ),
            "the managed unit differs from the exact generated unit; automatic switch refused"
        );
        self.verify_provenance()?;
        // Preserve even the exact legacy pre-UMask image for rollback. The
        // candidate is rendered with the current private-creation policy.
        self.old_unit = Some(on_disk);
        Ok(())
    }

    fn expected_unit(&self, root: &Path) -> Result<Vec<u8>> {
        let root = canonical(root)?;
        if root == canonical(&self.candidate)? {
            self.new_unit
                .clone()
                .context("candidate unit image not rendered")
        } else if Some(root.as_path()) == self.old_root.as_deref() {
            self.old_unit.clone().context("old unit image not captured")
        } else {
            anyhow::bail!("unknown runtime root requested to start")
        }
    }
}

impl Controller for OwnedController {
    fn locale_patches(&self) -> (Option<PathBuf>, Option<PathBuf>) {
        (
            self.old_locale_patch.clone(),
            self.candidate_locale_patch.clone(),
        )
    }
    fn restore_locale_patches(
        &mut self,
        old: Option<PathBuf>,
        candidate: Option<PathBuf>,
    ) -> Result<()> {
        self.permit.ensure_open()?;
        anyhow::ensure!(
            self.recovery && self.old_root.is_none(),
            "locale journal recipes must precede recovery ownership"
        );
        service::managed_arguments(old.as_deref())?;
        service::managed_arguments(candidate.as_deref())?;
        self.old_locale_patch = old;
        self.candidate_locale_patch = candidate;
        Ok(())
    }
    fn ownership(&mut self, old: &Path) -> Result<()> {
        let root = canonical(&generation_root(&self.old)?)?;
        let old = canonical(old)?;
        anyhow::ensure!(
            root == old,
            "runtime selection changed after capture; refusing to own a different root"
        );
        self.old_root = Some(root);
        match self.mode {
            ServerMode::Systemd => {
                // Recheck the manager-env scope alongside the unit ownership
                // proof: the effective service data home must still match the
                // snapshot scope before any further engine step.
                validate_systemd_scope(&self.expected_home)?;
                self.prove_unit_owned()
            }
            ServerMode::Child => {
                let server = self
                    .permit
                    .owned_server()?
                    .context("no owned managed child")?;
                anyhow::ensure!(
                    server.mode == ServerMode::Child,
                    "owned child metadata is inconsistent"
                );
                Ok(())
            }
            ServerMode::Attached => unreachable!("attached refused at construction"),
        }
    }

    fn stop(&mut self) -> Result<()> {
        match self.mode {
            ServerMode::Systemd => systemd::stop(),
            ServerMode::Child => {
                let mut lease = self.permit.lease_child()?;
                if let Some(child) = lease.child_mut() {
                    if let Err(error) = self.handle.block_on(managed_child::stop_child(child)) {
                        // The lease drops and restores the child: no discard, no
                        // replacement is started from a partially stopped child.
                        return Err(error).context("could not stop owned managed child");
                    }
                }
                lease.discard_stopped();
                Ok(())
            }
            ServerMode::Attached => unreachable!("attached refused at construction"),
        }
    }

    fn stopped(&mut self) -> Result<()> {
        match self.mode {
            ServerMode::Systemd => {
                anyhow::ensure!(
                    systemd::is_inactive()?,
                    "the managed service did not become inactive"
                );
                Ok(())
            }
            ServerMode::Child => {
                anyhow::ensure!(
                    self.permit.owned_server()?.is_none(),
                    "the managed child is still owned"
                );
                Ok(())
            }
            ServerMode::Attached => unreachable!("attached refused at construction"),
        }
    }

    fn start(&mut self, root: &Path) -> Result<()> {
        let patch = self.locale_patch_for_root(root)?;
        let result = match self.mode {
            ServerMode::Systemd => {
                // Recheck the manager-env scope immediately before activation:
                // the manager environment that the freshly started service will
                // inherit must still resolve to the snapshot scope.
                validate_systemd_scope(&self.expected_home)?;
                let expected = self.expected_unit(root)?;
                let on_disk = self.read_unit()?;
                anyhow::ensure!(
                    on_disk == expected,
                    "the unit does not match the requested runtime root"
                );
                systemd::start()
            }
            ServerMode::Child => {
                let runtime = generation_runtime(root);
                let log: managed_child::Log = match &self.app {
                    Some(app) => {
                        let app = app.clone();
                        std::sync::Arc::new(move |line: String| {
                            app.state::<crate::state::AppState>().log(line);
                        })
                    }
                    None => std::sync::Arc::new(|_| {}),
                };
                self.handle
                    .block_on(service::spawn_managed_logged_with_patch(
                        &runtime,
                        &self.permit,
                        log,
                        patch.as_deref(),
                    ))?;
                Ok(())
            }
            ServerMode::Attached => unreachable!("attached refused at construction"),
        };
        result?;
        self.active_locale_patch = patch;
        Ok(())
    }

    fn health(&mut self) -> Result<()> {
        match self.mode {
            ServerMode::Systemd => {
                // Type=simple start is not application readiness. The same
                // bounded authenticated wait covers candidate, rollback and
                // fresh-session recovery without publishing an early success.
                let token = self.handle.block_on(auth::wait_for_systemd(
                    &self.base,
                    &self.unit_name,
                    std::time::Duration::from_secs(30),
                ))?;
                self.permit.publish_server(Server {
                    base_url: self.base.clone(),
                    token,
                    mode: ServerMode::Systemd,
                    unit: Some(self.unit_name.clone()),
                    locale_patch: self.active_locale_patch.clone(),
                })?;
                Ok(())
            }
            ServerMode::Child => {
                let server = self.permit.server()?.context("no managed child server")?;
                anyhow::ensure!(
                    server.mode == ServerMode::Child,
                    "managed child metadata is inconsistent"
                );
                anyhow::ensure!(
                    crate::http::status(&server.base_url) == Some(401),
                    "server did not enforce the anonymous gate"
                );
                anyhow::ensure!(
                    auth::validate(&server.base_url, &server.token),
                    "launch token did not validate"
                );
                Ok(())
            }
            ServerMode::Attached => unreachable!("attached refused at construction"),
        }
    }

    fn unit(&mut self) -> Result<Option<UnitImage>> {
        match self.mode {
            ServerMode::Child => Ok(None),
            ServerMode::Systemd => {
                self.prove_unit_owned()?;
                let before = self
                    .old_unit
                    .clone()
                    .context("owned unit image not captured")?;
                let candidate = generation_runtime(&self.candidate);
                let after = match self.candidate_locale_patch.as_deref() {
                    Some(patch) => systemd::render_unit_with_patch(&candidate, &self.base, patch)?,
                    None => systemd::render_unit(&candidate, &self.base),
                }
                .into_bytes();
                self.new_unit = Some(after.clone());
                Ok(Some(UnitImage { before, after }))
            }
            ServerMode::Attached => unreachable!("attached refused at construction"),
        }
    }

    fn check_unit(&mut self, image: &Option<UnitImage>) -> Result<()> {
        if self.mode == ServerMode::Child {
            anyhow::ensure!(image.is_none(), "a managed child has no unit image");
            return Ok(());
        }
        let image = image
            .as_ref()
            .context("systemd check requires a unit image")?;
        self.verify_provenance()?;
        let on_disk = self.read_unit()?;
        anyhow::ensure!(
            on_disk == image.before || on_disk == image.after,
            "unit changed outside the transaction; refusing overwrite"
        );
        Ok(())
    }

    fn switch_unit(&mut self, image: &Option<UnitImage>, forward: bool) -> Result<()> {
        if self.mode == ServerMode::Child {
            anyhow::ensure!(image.is_none(), "a managed child has no unit image");
            return Ok(());
        }
        let image = image
            .as_ref()
            .context("systemd switch requires a unit image")?;
        self.check_unit(&Some(image.clone()))?;
        let content = if forward { &image.after } else { &image.before };
        systemd::replace_owned_unit(&self.unit_path, &[&image.before, &image.after], content)?;
        systemd::daemon_reload()?;
        self.verify_provenance()?;
        let on_disk = self.read_unit()?;
        anyhow::ensure!(on_disk == *content, "unit replacement did not persist");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_lifecycle::{Lifecycle, Operation};

    fn child_server() -> Server {
        Server {
            base_url: Url::parse("http://127.0.0.1:3080/").expect("valid base"),
            token: "test-token".into(),
            mode: ServerMode::Child,
            unit: None,
            locale_patch: None,
        }
    }

    #[test]
    fn manager_env_resolution_defaults_and_parses_plain_values() {
        // Missing HOME defaults to the passwd login home.
        let empty = systemd::ManagerEnvironment {
            home: vec![],
            dsh_home: vec![],
        };
        assert_eq!(
            resolve_manager_home(&empty).unwrap(),
            config::account_home().expect("test account has a login home")
        );
        // Missing DSH_HOME defaults to <HOME>/.dsh.
        assert_eq!(
            resolve_manager_dsh_home(&empty, Path::new("/home/alice")).unwrap(),
            PathBuf::from("/home/alice/.dsh")
        );
        // A plain absolute value is parsed verbatim (no shell evaluation).
        assert_eq!(
            parse_plain_absolute(b"/home/alice", "HOME").unwrap(),
            PathBuf::from("/home/alice")
        );
        let explicit = systemd::ManagerEnvironment {
            home: vec![b"/home/alice".to_vec()],
            dsh_home: vec![b"/home/alice/.dsh".to_vec()],
        };
        assert_eq!(
            resolve_manager_home(&explicit).unwrap(),
            PathBuf::from("/home/alice")
        );
        assert_eq!(
            resolve_manager_dsh_home(&explicit, Path::new("/home/alice")).unwrap(),
            PathBuf::from("/home/alice/.dsh")
        );
    }

    #[test]
    fn manager_env_rejects_unparseable_or_duplicate_values() {
        // Quoted, escaped, relative and empty values fail closed, and the value
        // is never shell-evaluated or echoed back.
        for bad in [
            &b""[..],
            b"relative/path",
            b"/home/alice with space",
            b"\"/home/alice\"",
            b"/home/alice\\ here",
            b"/home/$secret",
            b"/home/`secret`",
        ] {
            let outcome = parse_plain_absolute(bad, "HOME");
            assert!(outcome.is_err(), "accepted {bad:?}");
            assert!(
                !outcome.err().unwrap().to_string().contains("secret"),
                "the raw value must never be echoed: {bad:?}"
            );
        }
        // Duplicate HOME / DSH_HOME fail closed.
        let dup_home = systemd::ManagerEnvironment {
            home: vec![b"/a".to_vec(), b"/b".to_vec()],
            dsh_home: vec![],
        };
        assert!(resolve_manager_home(&dup_home).is_err());
        let dup_dsh = systemd::ManagerEnvironment {
            home: vec![b"/a".to_vec()],
            dsh_home: vec![b"/a/.dsh".to_vec(), b"/b/.dsh".to_vec()],
        };
        assert!(resolve_manager_dsh_home(&dup_dsh, Path::new("/a")).is_err());
    }

    #[test]
    fn canonical_fails_closed_on_a_missing_path() {
        let missing = std::env::temp_dir().join(format!(
            "dsh-definitely-missing-{}",
            crate::downloads::unique_id()
        ));
        assert!(canonical(&missing).is_err());
    }

    #[test]
    fn a_dropped_child_lease_restores_ownership() {
        let lifecycle: Lifecycle<Server, String> = Lifecycle::new(|_| {});
        let permit = lifecycle.try_begin(Operation::Update).unwrap();
        permit
            .publish_child(child_server(), "owned-child".to_string())
            .unwrap();
        assert!(permit.owned_server().unwrap().is_some());
        {
            let _lease = permit.lease_child().unwrap();
            // Dropped without `discard_stopped()` = the controller's stop() error
            // path, which must restore the child rather than discard it.
        }
        assert!(
            permit.owned_server().unwrap().is_some(),
            "a failed stop must restore the child, never discard it"
        );
        assert_eq!(permit.server().unwrap().unwrap().mode, ServerMode::Child);
        // Contrast: a successful stop discards, so ownership becomes absent.
        let lease = permit.lease_child().unwrap();
        lease.discard_stopped();
        assert!(
            permit.owned_server().unwrap().is_none(),
            "a successful stop discards ownership"
        );
    }

    #[test]
    fn recovery_refuses_a_fresh_session_without_an_owned_child() {
        use crate::runtime_lifecycle::Lifecycle;
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-recover-child-{}",
            crate::downloads::unique_id()
        ));
        let old_root = root.join("generations/gen.old");
        std::fs::create_dir_all(&old_root).unwrap();
        std::fs::write(old_root.join(".versions"), b"fixture\n").unwrap();
        // A managed-child server without a published child handle: the fresh
        // session no longer owns the stopped child, so recovery must refuse
        // rather than kill an arbitrary PID reconstructed from a path.
        let lifecycle: Lifecycle<Server, tokio::process::Child> =
            Lifecycle::new(crate::managed_child::kill_owned_child);
        let permit = lifecycle.try_begin(Operation::Update).unwrap();
        permit.publish_server(child_server()).unwrap();
        let error = OwnedController::headless_recover(
            permit,
            old_root.clone(),
            std::env::temp_dir().join("candidate"),
            std::env::temp_dir().join(".dsh"),
            None,
        )
        .err()
        .expect("fresh child recovery must be refused");
        assert!(error.to_string().contains("child"), "{error:#}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fresh_child_recovery_gives_clear_same_session_guidance() {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-recover-fresh-child-{}",
            crate::downloads::unique_id()
        ));
        let old_root = root.join("generations/gen.old");
        std::fs::create_dir_all(&old_root).unwrap();
        std::fs::write(old_root.join(".versions"), b"fixture\n").unwrap();
        // No server published at all: the fresh session lost the owned child
        // handle. Recovery must refuse with clear same-session guidance, never
        // the misleading "no running server to own" or a reconstructed PID.
        let lifecycle: Lifecycle<Server, tokio::process::Child> =
            Lifecycle::new(crate::managed_child::kill_owned_child);
        let permit = lifecycle.try_begin(Operation::Update).unwrap();
        let error = OwnedController::headless_recover(
            permit,
            old_root.clone(),
            std::env::temp_dir().join("candidate"),
            std::env::temp_dir().join(".dsh"),
            None,
        )
        .err()
        .expect("fresh child recovery must be refused");
        let text = error.to_string();
        assert!(text.contains("child"), "{text}");
        assert!(text.contains("same session"), "{text}");
        assert!(
            !text.contains("no running server to own"),
            "guidance must not mislead: {text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

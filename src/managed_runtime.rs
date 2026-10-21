//! Native signed installed-runtime apply/recovery orchestration.
//!
//! This facade replaces the old `npm view` updater path: it checks the
//! independent configured runtime Source (settings/key/channel), downloads a
//! signed complete runtime bundle into a private cache, imports it under the
//! runtime prefix (retaining the `.bootstrap.lock`), and drives the existing
//! owned switch engine with a real [`OwnedController`]. Recovery reads the
//! pending journal and rebuilds the owned controller from the journal's OLD
//! runtime — never from `bootstrap::detect`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use semver::Version;

use crate::bootstrap::Runtime;
use crate::release_feed::Channel;
use crate::runtime_controller::OwnedController;
use crate::runtime_feed::Candidate;
use crate::runtime_switch::{self, Controller, Paths, UnitImage};
use crate::service::{Server, ServerMode};
use crate::state::RuntimePermit;
use crate::update_control::Control;
use crate::update_settings::{self, Product};

/// Private, non-forgeable authority for constructing the owned controller.
/// Production passes `Owned(app)`; tests pass `Headless`. It is `pub(crate)`
/// so no public caller can manufacture an owned apply without an app handle.
pub(crate) enum Authority {
    Owned(tauri::AppHandle),
    Headless,
}

impl Authority {
    fn from_app(app: Option<tauri::AppHandle>) -> Self {
        match app {
            Some(app) => Authority::Owned(app),
            None => Authority::Headless,
        }
    }
}

/// Result of a configured runtime check, ready for a native dialog.
pub struct Check {
    pub candidate: Candidate,
    pub summary: serde_json::Value,
    pub source: SourceSummary,
    pub status: Status,
}

/// Whether the installed generation is byte-identical to the signed candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The selected generation's verified receipt matches the candidate's exact
    /// manifest and artifact (same release AND Node/pnpm/native closure AND bytes).
    Current,
    /// A different (newer, or same-release-different-components) signed bundle
    /// is available for apply.
    Available,
}

/// Why an owned native apply cannot be offered for the running runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyBlock {
    /// Not generation-managed: an external npm/package-manager or legacy install.
    External,
    /// The published server is attached (owned by another process).
    Attached,
    /// The published server origin is not a local loopback.
    Remote,
    /// No server is published; the ownership mode is unknown.
    NoServer,
}

impl ApplyBlock {
    /// Operator-facing check-only guidance; never an apply offer and never a
    /// download/import of an orphaned generation.
    pub fn guidance(self) -> &'static str {
        match self {
            ApplyBlock::External => "This is an external or legacy installation. Ocean does not overwrite external npm prefixes or legacy layouts; update it with the original package manager (safe migration to private generations is not implemented yet). No files were changed.",
            ApplyBlock::Attached => "This DSH server is attached and owned by another process. Ocean only updates its own systemd service or managed child runtime; update the server where it was started. No files were changed.",
            ApplyBlock::Remote => "The configured DSH origin is remote. An owned local runtime update is not offered for a remote server; update that installation directly. No files were changed.",
            ApplyBlock::NoServer => "No local DSH server is running. An owned runtime update requires the local dsh-web.service or a managed child. Start DSH first, then check again. No files were changed.",
        }
    }
}

/// Prove, from the detected runtime and the current published server, whether an
/// owned native apply may be offered. This is the offer policy: it runs before
/// the confirm dialog and before any download, and `apply_native` re-proves it
/// before downloading. Ownership is derived from the actual generation layout
/// and server mode — never from the runtime mode string alone.
pub fn apply_block(runtime: &Runtime, server: Option<&Server>) -> Option<ApplyBlock> {
    if !runtime.is_generation_managed() {
        return Some(ApplyBlock::External);
    }
    let Some(server) = server else {
        return Some(ApplyBlock::NoServer);
    };
    if !crate::service::permits_local_start(&server.base_url) {
        return Some(ApplyBlock::Remote);
    }
    if server.mode == ServerMode::Attached {
        return Some(ApplyBlock::Attached);
    }
    None
}

/// Public feed locations only (no secret material) for the confirm dialog.
#[derive(Clone, Debug)]
pub struct SourceSummary {
    pub channel: String,
    pub manifest_url: String,
    pub signature_url: String,
}

/// Check the independent configured runtime source and authenticate its
/// metadata. No artifact download, install, selector/floor change or profile
/// access. Fails closed before any network when no runtime source is
/// configured, so there is no npm fallback.
pub fn check(current: &Version, control: Arc<Control>) -> Result<Check> {
    let settings = update_settings::load()?;
    let source = settings.source(Product::Runtime)?;
    let candidate = crate::runtime_feed::check(&settings, current, control)?;
    let feed = source.feed(settings.channel(Product::Runtime));
    let summary = candidate.summary();
    let status = if selected_is_current(candidate.verified())? {
        Status::Current
    } else {
        Status::Available
    };
    Ok(Check {
        candidate,
        summary,
        source: SourceSummary {
            channel: channel_name(settings.channel(Product::Runtime)).to_owned(),
            manifest_url: feed.manifest_url.clone(),
            signature_url: feed.signature_url.clone(),
        },
        status,
    })
}

fn channel_name(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "stable",
        Channel::Next => "next",
    }
}

/// Whether the currently-selected generation's verified import receipt is
/// byte-identical to `release` (same manifest digest and artifact digest). A DSH
/// release-string match alone is NOT proof of "up to date": the manifest digest
/// covers the Node/pnpm/native component closure and the artifact digest covers
/// the shipped bytes, so a same-DSH release with changed components is still an
/// available update. A missing receipt (legacy/external generation) is "not
/// current", never a false "up to date".
fn selected_is_current(release: &crate::runtime_feed::Verified) -> Result<bool> {
    let Ok(root) = crate::bootstrap::selected_private_root(&crate::config::data_dir()) else {
        return Ok(false);
    };
    let path = root.join(".ocean-runtime.json");
    let bytes = match crate::release_feed::read_input(&path, crate::release_feed::MAX_MANIFEST) {
        Ok(bytes) => bytes,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let receipt: serde_json::Value =
        serde_json::from_slice(&bytes).context("invalid runtime import receipt")?;
    Ok(
        receipt.get("schema").and_then(serde_json::Value::as_u64) == Some(1)
            && receipt
                .get("manifestSha256")
                .and_then(serde_json::Value::as_str)
                == Some(release.manifest_sha256())
            && receipt
                .get("artifactSha256")
                .and_then(serde_json::Value::as_str)
                == Some(release.artifact().sha256.as_str()),
    )
}

/// Pin the production runtime transaction paths. `prefix` is the ordinary
/// pending gate (`config::data_dir`), `home` is the DSH data home
/// (`config::dsh_home`), and `backups` is a private directory outside home.
pub fn runtime_paths() -> Result<Paths> {
    Ok(Paths {
        prefix: crate::config::data_dir(),
        home: crate::config::dsh_home(),
        backups: crate::config::state_dir().join("runtime-backups"),
    })
}

/// Read-only pending-transaction status for a native recovery dialog.
pub fn pending_status() -> Result<Option<runtime_switch::PendingInfo>> {
    runtime_switch::pending_info(&runtime_paths()?)
}

pub enum ApplyOutcome {
    Applied { release: String },
    RolledBack { error: String },
    PendingRecovery { error: String },
}

pub enum RecoverOutcome {
    Recovered,
}

/// Full owned apply: settings guard -> runtime floor transaction -> candidate
/// revalidate -> preparatory download -> import (`.bootstrap.lock`) -> owned
/// switch engine -> exact floor record, with a fresh revalidate immediately
/// before the NEW root starts (the OLD root's rollback start remains allowed
/// even if the candidate has since expired).
pub fn apply_native(
    app: Option<tauri::AppHandle>,
    permit: RuntimePermit,
    old: Runtime,
    candidate: Candidate,
    control: Arc<Control>,
) -> Result<ApplyOutcome> {
    // 0. Preflight owned mode before any settings load, feed read, download or
    //    import: an external/attached/remote install is refused before an
    //    orphaned generation is downloaded or imported. The full unit/child
    //    ownership proof still happens later in `construct`/`ownership`.
    if let Some(block) = apply_block(&old, permit.server()?.as_ref()) {
        anyhow::bail!("{}", block.guidance());
    }

    // 1. settings guard (retained through the whole workflow).
    let guard = update_settings::Guard::begin()?;
    let settings = guard.settings().clone();

    // 2. running runtime version.
    let old_version = Version::parse(&old.version).context("invalid running runtime version")?;

    // 3. runtime floor transaction (lock order: settings -> floor -> prefix).
    let mut floor = crate::release_state::Transaction::begin_for(Product::Runtime)?;

    // 4. revalidate before any download: settings unchanged + policy/floor/expiry fresh.
    let verified = candidate.revalidate(&settings, &old_version)?;

    let channel = channel_name(verified.manifest().channel);
    let sequence = verified.manifest().sequence;
    let release = verified.manifest().release.clone();
    let digest = verified.manifest_sha256().to_owned();
    floor.check(channel, sequence, &release, &digest)?;

    let paths = runtime_paths()?;

    // 5. preparatory download into a private, never-selected cache.
    let cache_parent = crate::config::state_dir().join("runtime-cache");
    crate::shell_apply::ensure_prefix(&cache_parent)?;
    let cache_dir = cache_parent.join(format!("download-{}", crate::downloads::unique_id()));
    let cache_guard = crate::checked_archive::NewDirectory::create(&cache_dir)?;
    let staged = crate::release_download::download_runtime_preparatory(
        &verified,
        &cache_dir.join("artifact.tar.gz"),
        control.clone(),
    )?;
    control.check()?;

    // 6. import (acquires and retains the runtime prefix `.bootstrap.lock`).
    let prepared = crate::runtime_bundle::import(staged.path(), &verified, &paths.prefix)?;
    control.check()?;

    // 7. build the owned controller (owns the permit).
    let mut controller = build_controller(
        Authority::from_app(app),
        permit,
        old,
        prepared.root().to_path_buf(),
        paths.home.clone(),
    )?;

    // 8. apply with a fresh pre-start revalidate decorator + exact floor record.
    let new_root = prepared.root().to_path_buf();
    let revalidate = || candidate.revalidate(&settings, &old_version).map(|_| ());
    let apply_result = {
        let mut decorated = RevalidateStart {
            inner: &mut controller,
            revalidate: &revalidate,
            new_root,
        };
        runtime_switch::apply(
            &paths,
            &prepared,
            &verified,
            control.clone(),
            &mut decorated,
            || {
                floor
                    .record(channel, sequence, &release, &digest)
                    .map(|_| ())
            },
        )
    };

    // 9. resolve the outcome. A retained pending journal means rollback did not
    //    restore the old runtime; do not reload or restart from here.
    if runtime_switch::ensure_no_pending(&paths.prefix).is_err() {
        let error = apply_result
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_else(|| "runtime switch left a pending recovery journal".to_owned());
        return Ok(ApplyOutcome::PendingRecovery { error });
    }
    let finish_error = controller.finish().err().map(|e| format!("{e:#}"));
    // Keep the private download cache alive until the owned worker is reaped;
    // dropping it now removes the staged artifact, never the imported root.
    drop(cache_guard);
    match apply_result {
        Ok(()) => {
            let detail = match finish_error {
                Some(e) => format!("{release} (window refresh failed: {e})"),
                None => release,
            };
            Ok(ApplyOutcome::Applied { release: detail })
        }
        Err(e) => {
            let mut error = format!("{e:#}");
            if let Some(fe) = finish_error {
                error.push_str(&format!("\nwindow refresh after rollback failed: {fe}"));
            }
            Ok(ApplyOutcome::RolledBack { error })
        }
    }
}

/// Explicit recovery: hold the prefix lock and runtime permit, rebuild the owned
/// controller from the pending journal's OLD runtime and captured before/after
/// unit images, then run the durable rollback. A fresh session that no longer
/// owns a stopped child handle is refused safely (never a PID from a path), and
/// a failed restore stays pending.
pub fn recover_native(
    app: Option<tauri::AppHandle>,
    permit: RuntimePermit,
    control: Arc<Control>,
) -> Result<RecoverOutcome> {
    control.check()?;
    let paths = runtime_paths()?;

    // 1. validated read-only journal summary.
    let info = runtime_switch::pending_info(&paths)?
        .context("there is no pending runtime transaction to recover")?;

    // 2. hold the runtime prefix lock (same as apply/import).
    let _prefix_lock = crate::profile_backup::operation_lock(&paths.prefix.join(".bootstrap.lock"))
        .context("another runtime install/update is active")?;

    // 3. rebuild the owned controller from the journal OLD runtime + unit images.
    let mut controller = build_recover_controller(
        Authority::from_app(app),
        permit,
        info.old.clone(),
        info.new.clone(),
        paths.home.clone(),
        info.unit.clone(),
    )?;

    // 4. cross the non-cancellable commit boundary before mutating state.
    control.begin_commit()?;

    // 5. durable rollback.
    runtime_switch::recover(&paths, &mut controller)?;

    // 6. refresh the window/token to the restored OLD runtime.
    controller
        .finish()
        .context("runtime recovered; window refresh failed")?;
    Ok(RecoverOutcome::Recovered)
}

fn build_controller(
    authority: Authority,
    permit: RuntimePermit,
    old: Runtime,
    candidate: PathBuf,
    expected_home: PathBuf,
) -> Result<OwnedController> {
    match authority {
        Authority::Owned(app) => OwnedController::owned(app, permit, old, candidate, expected_home),
        Authority::Headless => OwnedController::headless(permit, old, candidate, expected_home),
    }
}

fn build_recover_controller(
    authority: Authority,
    permit: RuntimePermit,
    old_root: PathBuf,
    candidate: PathBuf,
    expected_home: PathBuf,
    unit: Option<UnitImage>,
) -> Result<OwnedController> {
    match authority {
        Authority::Owned(app) => {
            OwnedController::owned_recover(app, permit, old_root, candidate, expected_home, unit)
        }
        Authority::Headless => {
            OwnedController::headless_recover(permit, old_root, candidate, expected_home, unit)
        }
    }
}

/// Narrow controller decorator: revalidates the raw Candidate (settings, clock,
/// floor) immediately before the NEW root starts, and leaves the OLD root's
/// rollback start allowed even when the candidate has since expired.
struct RevalidateStart<'a, C: Controller> {
    inner: &'a mut C,
    revalidate: &'a dyn Fn() -> Result<()>,
    new_root: PathBuf,
}

impl<C: Controller> Controller for RevalidateStart<'_, C> {
    fn ownership(&mut self, old: &Path) -> Result<()> {
        self.inner.ownership(old)
    }
    fn stop(&mut self) -> Result<()> {
        self.inner.stop()
    }
    fn stopped(&mut self) -> Result<()> {
        self.inner.stopped()
    }
    fn start(&mut self, root: &Path) -> Result<()> {
        if same_root(root, &self.new_root) {
            (self.revalidate)()?;
        }
        self.inner.start(root)
    }
    fn health(&mut self) -> Result<()> {
        self.inner.health()
    }
    fn unit(&mut self) -> Result<Option<UnitImage>> {
        self.inner.unit()
    }
    fn check_unit(&mut self, image: &Option<UnitImage>) -> Result<()> {
        self.inner.check_unit(image)
    }
    fn switch_unit(&mut self, image: &Option<UnitImage>, forward: bool) -> Result<()> {
        self.inner.switch_unit(image, forward)
    }
}

fn same_root(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        starts: Vec<PathBuf>,
    }
    impl Controller for Fake {
        fn ownership(&mut self, _old: &Path) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
        fn stopped(&mut self) -> Result<()> {
            Ok(())
        }
        fn start(&mut self, root: &Path) -> Result<()> {
            self.starts.push(root.to_path_buf());
            Ok(())
        }
        fn health(&mut self) -> Result<()> {
            Ok(())
        }
        fn unit(&mut self) -> Result<Option<UnitImage>> {
            Ok(None)
        }
    }

    #[test]
    fn revalidate_decorator_rechecks_only_the_new_root_and_allows_rollback() {
        let tmp = std::env::temp_dir().join(format!(
            "dsh-managed-runtime-revalidate-{}",
            crate::downloads::unique_id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let new_root = tmp.join("generations/gen.new");
        let old_root = tmp.join("generations/gen.old");
        std::fs::create_dir_all(&new_root).unwrap();
        std::fs::create_dir_all(&old_root).unwrap();

        let calls = std::cell::Cell::new(0usize);
        let revalidate = || -> Result<()> {
            calls.set(calls.get() + 1);
            anyhow::bail!("candidate expired");
        };
        let mut inner = Fake { starts: Vec::new() };
        {
            let mut decorated = RevalidateStart {
                inner: &mut inner,
                revalidate: &revalidate,
                new_root: new_root.clone(),
            };
            assert!(
                decorated.start(&new_root).is_err(),
                "an expired/changed candidate must refuse a new-root start"
            );
            assert!(
                decorated.start(&old_root).is_ok(),
                "rollback must remain allowed even after the candidate expired"
            );
        }
        assert_eq!(calls.get(), 1, "revalidate must run only for the new root");
        assert_eq!(inner.starts, vec![old_root]);
    }

    #[test]
    fn apply_block_policy_refuses_external_attached_remote_and_allows_owned() {
        let root =
            std::env::temp_dir().join(format!("dsh-apply-block-{}", crate::downloads::unique_id()));
        let gen = root.join("generations/gen.x");
        std::fs::create_dir_all(&gen).unwrap();
        std::fs::write(gen.join(".versions"), b"fixture\n").unwrap();
        let managed = Runtime {
            node: gen.join("runtime/node/bin/node"),
            dsh: gen.join("harness/node_modules/.bin/dsh"),
            version: "0.2.0".into(),
            install_root: Some(gen.join("harness")),
        };
        let external = Runtime {
            node: root.join("elsewhere/bin/node"),
            dsh: root.join("elsewhere/dsh"),
            version: "0.2.0".into(),
            install_root: None,
        };
        let loopback = url::Url::parse("http://127.0.0.1:3080/").unwrap();
        let remote = url::Url::parse("http://192.168.1.5:3080/").unwrap();
        let server = |mode: ServerMode, base: &url::Url| Server {
            base_url: base.clone(),
            token: "t".into(),
            mode,
            unit: match mode {
                ServerMode::Systemd => Some("dsh-web.service".into()),
                _ => None,
            },
        };

        assert_eq!(apply_block(&external, None), Some(ApplyBlock::External));
        assert_eq!(apply_block(&managed, None), Some(ApplyBlock::NoServer));
        assert_eq!(
            apply_block(&managed, Some(&server(ServerMode::Attached, &loopback))),
            Some(ApplyBlock::Attached)
        );
        assert_eq!(
            apply_block(&managed, Some(&server(ServerMode::Attached, &remote))),
            Some(ApplyBlock::Remote)
        );
        assert_eq!(
            apply_block(&managed, Some(&server(ServerMode::Systemd, &loopback))),
            None
        );
        assert_eq!(
            apply_block(&managed, Some(&server(ServerMode::Child, &loopback))),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

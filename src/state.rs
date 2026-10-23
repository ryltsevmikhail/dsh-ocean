use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::tray::TrayIcon;
use tokio::process::Child;

use crate::bootstrap::{self, RuntimeInfo};
use crate::config;
use crate::service::{Server, ServerMode};

#[derive(Clone, Debug)]
pub enum Phase {
    Idle,
    Preparing,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub base_url: String,
    pub mode: String,
    pub unit: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupState {
    pub phase: String,
    /// True while a runtime installer child is owned by this process.
    pub installing: bool,
    /// True when a running installer has an accepted cancellation request.
    pub cancel_requested: bool,
    pub message: Option<String>,
    pub logs: Vec<String>,
    pub server: Option<ServerInfo>,
    pub runtime: Option<RuntimeInfo>,
    pub can_install: bool,
    /// UI affordance only: native recovery still validates the journal and ownership.
    pub recovery_required: bool,
    pub updating: bool,
    pub config_path: String,
    pub diagnostics_path: String,
}

#[derive(Default)]
struct BootstrapState {
    active: bool,
    installer: bool,
    cancel: bool,
}

pub struct AppState {
    pub runtime: crate::runtime_lifecycle::Lifecycle<Server, Child>,
    phase: Mutex<Phase>,
    logs: Mutex<VecDeque<String>>,
    tray: Mutex<Option<TrayIcon>>,
    pub downloads: Mutex<Option<crate::download_engine::SharedHistory>>,
    pub download_cancel: Mutex<Option<crate::download_engine::Cancellation>>,
    pub notifier: std::sync::Arc<crate::notifications::Client>,
    pub profile_verifying: std::sync::Arc<AtomicBool>,
    pub data_snapshot: std::sync::Arc<AtomicBool>,
    pub backup_export: std::sync::Arc<AtomicBool>,
    pub updates: std::sync::Arc<crate::native_updates::Registry>,
    /// Cached runtime detection for the setup window's poll.
    runtime_cache: Mutex<Option<(std::time::Instant, Option<bootstrap::Runtime>)>>,
    /// Short-lived LAN pairing proxy, owned only while explicitly started.
    pairing: Mutex<Option<crate::pairing_proxy::Proxy>>,
    bootstrap: std::sync::Arc<Mutex<BootstrapState>>,
    /// Process-group id of the running installer, used for cooperative shutdown.
    pub bootstrap_pgid: Mutex<Option<i32>>,
    pub zoom: Mutex<f64>,
    pub quitting: AtomicBool,
}

pub struct BootstrapGuard(std::sync::Arc<Mutex<BootstrapState>>);
impl Drop for BootstrapGuard {
    fn drop(&mut self) {
        let mut bootstrap = self.0.lock().expect("bootstrap mutex");
        bootstrap.active = false;
        bootstrap.installer = false;
        bootstrap.cancel = false;
    }
}

pub type RuntimePermit = crate::runtime_lifecycle::Permit<Server, Child>;

impl Default for AppState {
    fn default() -> Self {
        Self {
            runtime: crate::runtime_lifecycle::Lifecycle::new(
                crate::managed_child::kill_owned_child,
            ),
            phase: Mutex::new(Phase::Idle),
            logs: Mutex::new(VecDeque::new()),
            tray: Mutex::new(None),
            downloads: Mutex::new(None),
            download_cancel: Mutex::new(None),
            notifier: std::sync::Arc::new(crate::notifications::Client::default()),
            profile_verifying: std::sync::Arc::default(),
            data_snapshot: std::sync::Arc::default(),
            backup_export: std::sync::Arc::default(),
            updates: std::sync::Arc::default(),
            runtime_cache: Mutex::new(None),
            pairing: Mutex::new(None),
            bootstrap: std::sync::Arc::default(),
            bootstrap_pgid: Mutex::new(None),
            zoom: Mutex::new(1.0),
            quitting: AtomicBool::new(false),
        }
    }
}

impl AppState {
    pub fn begin_update(&self) -> Result<RuntimePermit, crate::runtime_lifecycle::Refused> {
        self.runtime
            .try_begin(crate::runtime_lifecycle::Operation::Update)
    }

    /// One-shot ownership of a whole runtime-install command (installer plus
    /// the service preparation that follows it).
    pub fn begin_bootstrap(&self) -> Option<BootstrapGuard> {
        let mut bootstrap = self.bootstrap.lock().expect("bootstrap mutex");
        if bootstrap.active {
            return None;
        }
        bootstrap.active = true;
        bootstrap.installer = false;
        bootstrap.cancel = false;
        Some(BootstrapGuard(self.bootstrap.clone()))
    }

    pub fn bootstrap_running(&self) -> bool {
        self.bootstrap.lock().expect("bootstrap mutex").active
    }

    /// Marks the installer child as owned. Clears any stale cancellation request
    /// from a previous attempt in the same critical section, so a late click can
    /// never cancel the next run.
    pub fn installer_started(&self) {
        let mut bootstrap = self.bootstrap.lock().expect("bootstrap mutex");
        bootstrap.installer = true;
        bootstrap.cancel = false;
    }

    pub fn installer_finished(&self) {
        self.bootstrap.lock().expect("bootstrap mutex").installer = false;
    }

    pub fn is_installing(&self) -> bool {
        self.bootstrap.lock().expect("bootstrap mutex").installer
    }

    /// Accepts a cooperative cancellation request in one critical section.
    /// False means no installer child is owned (not started yet or already
    /// finished), so a request cannot change its outcome.
    pub fn request_bootstrap_cancel(&self) -> bool {
        let mut bootstrap = self.bootstrap.lock().expect("bootstrap mutex");
        if !bootstrap.installer {
            return false;
        }
        bootstrap.cancel = true;
        true
    }

    pub fn bootstrap_cancel_requested(&self) -> bool {
        self.bootstrap.lock().expect("bootstrap mutex").cancel
    }

    /// A cheap cloneable view of the cancellation flag for the supervisor loop.
    pub fn bootstrap_cancel_flag(
        &self,
    ) -> std::sync::Arc<dyn crate::bootstrap_supervisor::CancelFlag> {
        let shared = self.bootstrap.clone();
        std::sync::Arc::new(move || shared.lock().expect("bootstrap mutex").cancel)
    }

    pub fn set_bootstrap_pgid(&self, pgid: Option<i32>) {
        *self.bootstrap_pgid.lock().expect("bootstrap pgid mutex") = pgid;
    }

    pub fn bootstrap_pgid(&self) -> Option<i32> {
        *self.bootstrap_pgid.lock().expect("bootstrap pgid mutex")
    }

    /// Returns to a retryable state after cooperative cancellation.
    pub fn reset_idle(&self) {
        let mut phase = self.phase.lock().expect("phase mutex");
        if matches!(*phase, Phase::Idle | Phase::Preparing | Phase::Failed(_)) {
            *phase = Phase::Idle;
        }
    }

    pub fn is_updating(&self) -> bool {
        self.runtime.is_updating()
    }

    pub fn log(&self, message: impl Into<String>) {
        let message = crate::auth::redact(&message.into());
        eprintln!("dsh-ocean: {message}");
        let mut logs = self.logs.lock().expect("log mutex");
        logs.push_back(message);
        while logs.len() > 160 {
            logs.pop_front();
        }
    }

    pub fn begin_prepare(
        &self,
    ) -> Result<Option<RuntimePermit>, crate::runtime_lifecycle::Refused> {
        // Keep no-op preparation from advancing the revision of a queued window.
        // Lock order is phase -> lifecycle; lifecycle never locks phase or GTK.
        let mut phase = self.phase.lock().expect("phase mutex");
        if matches!(*phase, Phase::Preparing | Phase::Ready) {
            return Ok(None);
        }
        let permit = self
            .runtime
            .try_begin(crate::runtime_lifecycle::Operation::Prepare)?;
        *phase = Phase::Preparing;
        Ok(Some(permit))
    }

    pub fn ready(&self) {
        *self.phase.lock().expect("phase mutex") = Phase::Ready;
    }

    pub fn fail(&self, message: impl Into<String>) {
        let message = crate::auth::redact(&message.into());
        self.log(
            crate::i18n::render(
                crate::i18n::active_language(),
                "notice.error",
                &[("message", &message)],
            )
            .expect("validated error log caption"),
        );
        *self.phase.lock().expect("phase mutex") = Phase::Failed(message);
    }

    pub fn set_tray(&self, tray: TrayIcon) {
        *self.tray.lock().expect("tray mutex") = Some(tray);
    }

    pub fn server(&self) -> Option<Server> {
        self.runtime.server()
    }

    pub fn shutdown(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        self.runtime.close();
        // Ask a running installer to stop cooperatively so its traps can remove
        // staged work and release the runtime lock.
        if let Some(pgid) = self.bootstrap_pgid().filter(|pgid| *pgid > 1) {
            // Signal only a group that still exists: the id may have been reaped by
            // the time the window closes, and a recycled id would hit an unrelated
            // process group.
            // SAFETY: signal 0 only probes the group, negative pid targets it.
            let alive = unsafe { libc::kill(-pgid, 0) == 0 };
            if alive {
                // SAFETY: negative pid targets only the installer's own group.
                unsafe {
                    libc::kill(-pgid, libc::SIGTERM);
                }
            }
        }
    }

    /// Starts the pairing proxy towards the loopback server, or returns its status.
    pub fn start_pairing(
        &self,
        ttl: std::time::Duration,
        bind: std::net::SocketAddr,
    ) -> anyhow::Result<crate::pairing_proxy::Status> {
        let server = self
            .server()
            .ok_or_else(|| anyhow::anyhow!("DSH is not running yet"))?;
        let host = server.base_url.host_str().unwrap_or_default();
        anyhow::ensure!(
            matches!(host, "127.0.0.1" | "127.0.0.2" | "localhost" | "::1"),
            "the server is not loopback; a pairing proxy would expose it twice"
        );
        let target: std::net::SocketAddr =
            format!("127.0.0.1:{}", crate::config::server_port(&server.base_url))
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid loopback target"))?;
        let mut guard = self.pairing.lock().expect("pairing mutex");
        // An expired or revoked proxy is still owned but unusable; replace it
        // instead of silently returning a dead status.
        if guard.as_ref().is_some_and(|proxy| proxy.is_stopped()) {
            if let Some(mut stopped) = guard.take() {
                stopped.revoke();
            }
        }
        if guard.is_none() {
            *guard = Some(crate::pairing_proxy::Proxy::start(bind, target, ttl)?);
        }
        let proxy = guard.as_ref().expect("proxy present");
        Ok(proxy.status())
    }

    pub fn stop_pairing(&self) -> bool {
        let mut guard = self.pairing.lock().expect("pairing mutex");
        match guard.take() {
            Some(mut proxy) => {
                proxy.revoke();
                true
            }
            None => false,
        }
    }

    pub fn pairing_status(&self) -> Option<crate::pairing_proxy::Status> {
        let guard = self.pairing.lock().expect("pairing mutex");
        guard.as_ref().and_then(|proxy| {
            let status = proxy.status();
            if proxy.is_stopped() || status.remaining_seconds == 0 {
                None
            } else {
                Some(status)
            }
        })
    }

    /// Detection is a subprocess call, so the setup window's poll joins a short
    /// cache instead of spawning `dsh --version` twice per refresh.
    fn runtime_probe(&self) -> Option<bootstrap::Runtime> {
        const TTL: std::time::Duration = std::time::Duration::from_secs(3);
        let mut cache = self
            .runtime_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some((at, runtime)) = cache.as_ref() {
            if at.elapsed() < TTL {
                return runtime.clone();
            }
        }
        let detected = bootstrap::detect_within(std::time::Duration::from_secs(2));
        *cache = Some((std::time::Instant::now(), detected.clone()));
        detected
    }

    pub fn snapshot(&self) -> SetupState {
        let phase = self.phase.lock().expect("phase mutex").clone();
        let (phase_name, message) = match phase {
            Phase::Idle => ("idle".to_string(), None),
            Phase::Preparing => ("preparing".to_string(), None),
            Phase::Ready => ("ready".to_string(), None),
            Phase::Failed(message) => ("failed".to_string(), Some(message)),
        };
        let logs = self
            .logs
            .lock()
            .expect("log mutex")
            .iter()
            .cloned()
            .collect();
        let server = self.server().map(|server| ServerInfo {
            base_url: server.base_url.to_string(),
            mode: match server.mode {
                ServerMode::Attached => "attached",
                ServerMode::Systemd => "systemd",
                ServerMode::Child => "child",
            }
            .to_string(),
            unit: server.unit,
        });
        let installing = self.is_installing();
        let probe = self.runtime_probe();
        SetupState {
            phase: phase_name,
            installing,
            cancel_requested: installing && self.bootstrap_cancel_requested(),
            message,
            logs,
            server,
            runtime: probe.as_ref().map(|runtime| runtime.info()),
            can_install: probe.is_none(),
            recovery_required: config::checked_server_url()
                .is_ok_and(|base| crate::service::permits_local_start(&base))
                && crate::runtime_switch::ensure_no_pending(&config::data_dir()).is_err(),
            updating: self.is_updating(),
            config_path: config::config_path().display().to_string(),
            diagnostics_path: config::diagnostics_dir().display().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_guard_serializes_and_always_releases() {
        let state = AppState::default();
        let guard = state.begin_update().unwrap();
        assert!(state.is_updating());
        assert!(state.begin_update().is_err());
        drop(guard);
        assert!(!state.is_updating());
        assert!(state.begin_update().is_ok());
    }

    #[test]
    fn no_op_preparation_does_not_invalidate_its_queued_window() {
        let state = AppState::default();
        let permit = state.begin_prepare().unwrap().unwrap();
        let window = permit.stamp();
        assert!(state.begin_prepare().unwrap().is_none());
        assert!(window.is_current());
        assert_eq!(
            state.begin_update().err(),
            Some(crate::runtime_lifecycle::Refused::Busy(
                crate::runtime_lifecycle::Operation::Prepare
            ))
        );
        state.ready();
        drop(permit);
        assert!(state.begin_prepare().unwrap().is_none());
        assert!(window.is_current());
    }

    #[test]
    fn refused_preparation_leaves_phase_retryable_and_reuses_the_common_gate() {
        let state = AppState::default();
        let update = state.begin_update().unwrap();
        assert!(state.begin_prepare().is_err());
        assert!(matches!(*state.phase.lock().unwrap(), Phase::Idle));
        drop(update);
        let prepare = state.begin_prepare().unwrap().unwrap();
        state.fail("synthetic window failure");
        drop(prepare);
        let retry = state.begin_prepare().unwrap().unwrap();
        assert!(state.begin_update().is_err());
        drop(retry);
    }

    #[test]
    fn bootstrap_gate_serializes_and_cancellation_never_releases_early() {
        let state = AppState::default();
        assert!(!state.request_bootstrap_cancel());
        assert!(!state.bootstrap_running());
        let guard = state.begin_bootstrap().unwrap();
        assert!(state.begin_bootstrap().is_none());
        assert!(state.bootstrap_running());
        // The command owns a slot, but only a started installer can be cancelled.
        assert!(!state.is_installing());
        assert!(!state.request_bootstrap_cancel());
        state.installer_started();
        assert!(state.is_installing());
        assert!(!state.bootstrap_cancel_requested());
        assert!(state.request_bootstrap_cancel());
        assert!(state.bootstrap_cancel_requested());
        // A cancellation request is not a release: a competing start is refused
        // even after the child is reaped, while service preparation runs.
        state.installer_finished();
        assert!(!state.is_installing());
        assert!(!state.request_bootstrap_cancel());
        assert!(state.begin_bootstrap().is_none());
        drop(guard);
        assert!(!state.bootstrap_running());
        assert!(state.begin_bootstrap().is_some());
        // A stale request cannot be inherited by the next attempt.
        assert!(!state.request_bootstrap_cancel());
    }

    #[test]
    fn starting_an_installer_clears_a_stale_cancellation_request() {
        let state = AppState::default();
        let guard = state.begin_bootstrap().unwrap();
        state.installer_started();
        assert!(state.request_bootstrap_cancel());
        assert!(state.bootstrap_cancel_requested());
        drop(guard);
        let _guard = state.begin_bootstrap().unwrap();
        state.installer_started();
        // The stale request did not leak; a fresh request for the new run is
        // accepted normally.
        assert!(!state.bootstrap_cancel_requested());
        assert!(state.request_bootstrap_cancel());
        assert!(state.bootstrap_cancel_requested());
    }

    #[test]
    fn shutdown_records_the_installer_group_for_cooperative_stop() {
        let state = AppState::default();
        assert_eq!(state.bootstrap_pgid(), None);
        state.set_bootstrap_pgid(Some(1));
        state.shutdown();
        assert!(state.quitting.load(Ordering::SeqCst));
        state.set_bootstrap_pgid(Some(2_000_000_000));
        state.shutdown();
        assert_eq!(state.bootstrap_pgid(), Some(2_000_000_000));
    }

    #[test]
    fn cancel_reset_returns_only_retryable_phases_to_idle() {
        let state = AppState::default();
        state.ready();
        state.reset_idle();
        assert_eq!(state.snapshot().phase, "ready");
        let state = AppState::default();
        state.fail("synthetic");
        assert_eq!(state.snapshot().phase, "failed");
        state.reset_idle();
        assert_eq!(state.snapshot().phase, "idle");
    }

    #[test]
    fn localized_startup_logs_still_redact_before_storage_without_changing_phase() {
        for (language, _) in crate::i18n::LANGUAGES {
            let state = AppState::default();
            let message = crate::i18n::render(
                language,
                "startup.ready",
                &[("url", "http://127.0.0.1/?token=synthetic-private")],
            )
            .expect("embedded localized startup message");
            state.log(message);
            let logs = state.logs.lock().unwrap();
            assert_eq!(logs.len(), 1);
            assert!(!logs[0].contains("synthetic-private") && logs[0].contains("token=***"));
            assert!(matches!(*state.phase.lock().unwrap(), Phase::Idle));
        }
    }
    #[test]
    fn localized_notice_errors_keep_message_literal_and_redact_tokens() {
        for (language, _) in crate::i18n::LANGUAGES {
            let state = AppState::default();
            state.log(
                crate::i18n::render(
                    language,
                    "notice.error",
                    &[("message", "<b>{path}</b> token=synthetic-private")],
                )
                .unwrap(),
            );
            let logs = state.logs.lock().unwrap();
            assert!(logs[0].contains("<b>{path}</b>"));
            assert!(!logs[0].contains("synthetic-private"));
        }
    }
    #[test]
    fn logs_and_failure_state_redact_tokens_before_storage() {
        let state = AppState::default();
        state.log("dsh web: http://127.0.0.1:1234/?token=synthetic-secret");
        state.fail("restart failed: token=another-secret");
        for line in state.logs.lock().unwrap().iter() {
            assert!(!line.contains("synthetic-secret"));
            assert!(!line.contains("another-secret"));
        }
        let phase = state.phase.lock().unwrap();
        assert!(
            matches!(&*phase, Phase::Failed(message) if message.contains("token=***") && !message.contains("another-secret"))
        );
    }
}

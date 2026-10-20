//! Cooperative supervision of the runtime-bootstrap child process.
//!
//! The installer is a bash script that traps `TERM` and restores the previous
//! runtime selector in its `EXIT` trap. Cancellation therefore signals the
//! child's process group so that a running `curl`/`npm` also receives `TERM`,
//! and escalates to `KILL` only after a bounded grace period.
//!
//! This module is deliberately independent of Tauri state so the behaviour can
//! be exercised with real short-lived local processes in tests.

use std::process::ExitStatus;
use std::sync::Mutex;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::process::Child;

/// How often the supervisor polls the cancellation flag.
pub const POLL: Duration = Duration::from_millis(100);

/// Default time a cooperative `TERM` is given before `KILL`.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(20);

/// Terminal classification of a supervised installer run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The script exited successfully; it committed a runtime selector.
    Committed,
    /// The script was interrupted by an accepted cancellation request.
    Cancelled,
    /// The script failed without an accepted cancellation (or was killed by
    /// something else such as the OOM killer).
    Failed,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Committed => "committed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug)]
pub struct Supervised {
    pub status: ExitStatus,
    /// True once this supervisor sent a signal because cancellation was asked.
    pub signal_sent: bool,
    /// True when the grace period expired and `SIGKILL` was used.
    pub escalated: bool,
}

impl Supervised {
    /// A successful exit always means the script committed its work, even if a
    /// cancellation request arrived late: cancellation never rewrites success.
    pub fn outcome(&self) -> Outcome {
        if self.status.success() {
            Outcome::Committed
        } else if self.signal_sent {
            Outcome::Cancelled
        } else {
            Outcome::Failed
        }
    }
}

fn signal(target: i32, signal: i32) {
    // SAFETY: plain libc wrapper; failures (already exited, no such group) are
    // intentionally ignored and handled by the escalation path.
    unsafe {
        libc::kill(target, signal);
    }
}

fn terminate(pid: Option<u32>, signal_number: i32) {
    if let Some(pid) = pid.filter(|pid| *pid > 1) {
        // The child is spawned as its own group leader, so the group covers the
        // script and every descendant it started. Signalling the leader directly
        // as well would only duplicate `TERM` and can race a trap-in-progress, so
        // the group is the single target.
        signal(-(pid as i32), signal_number);
    }
}

/// True while any process still belongs to `pgid`.
fn group_alive(pgid: i32) -> bool {
    if pgid <= 1 {
        return false;
    }
    // SAFETY: signal 0 performs only an existence/permission probe.
    unsafe { libc::kill(-pgid, 0) == 0 }
}

/// Signal any process still left in the installer's own group.
///
/// Only runs after the direct child was reaped and only while the group is still
/// known to have members, so the pgid cannot have been recycled by an unrelated
/// process group: a live member pins the identifier.
async fn sweep_group(pid: Option<u32>, grace: Duration) -> bool {
    let Some(pgid) = pid.map(|pid| pid as i32).filter(|pgid| *pgid > 1) else {
        return false;
    };
    if !group_alive(pgid) {
        return false;
    }
    let mut escalated = false;
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        signal(-pgid, libc::SIGTERM);
        let step = tokio::time::Instant::now() + Duration::from_millis(200);
        while tokio::time::Instant::now() < step {
            if !group_alive(pgid) {
                return escalated;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if tokio::time::Instant::now() >= deadline {
            signal(-pgid, libc::SIGKILL);
            escalated = true;
            // Give the kernel a bounded moment to reap the killed members.
            let give_up = tokio::time::Instant::now() + Duration::from_secs(2);
            while tokio::time::Instant::now() < give_up {
                if !group_alive(pgid) {
                    return escalated;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            return escalated;
        }
    }
}

/// Convenience adapter so callers can pass a closure as the cancellation source.
pub fn flag(source: impl Fn() -> bool + Send + Sync + 'static) -> Arc<dyn CancelFlag> {
    Arc::new(source)
}

/// Adapter for `Arc<Mutex<T>>`-style state that exposes a boolean cancellation bit.
pub fn from_mutex(bit: Arc<Mutex<bool>>) -> Arc<dyn CancelFlag> {
    Arc::new(move || *bit.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Reads a cancellation request from shared application state.
pub trait CancelFlag: Send + Sync {
    fn requested(&self) -> bool;
}

impl<F> CancelFlag for F
where
    F: Fn() -> bool + Send + Sync,
{
    fn requested(&self) -> bool {
        self()
    }
}

/// Wait for `child`, sending `TERM` to its process group once cancellation is
/// observed and `KILL` if it has not exited within `grace`.
///
/// No signal is sent when the child finishes before cancellation is observed.
pub async fn supervise(
    mut child: Child,
    cancel: Arc<dyn CancelFlag>,
    grace: Duration,
) -> Result<Supervised> {
    let pid = child.id();
    let mut signal_sent = false;
    let mut escalated = false;
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            status = child.wait() => {
                let status = status.context("wait for bootstrap child")?;
                // bash can exit while a descendant keeps the group (and possibly
                // the output pipes or the runtime lock) alive. Sweep whatever is
                // still in the group we created, with or without a cancel request.
                let swept = sweep_group(pid, grace).await;
                escalated |= swept;
                return Ok(Supervised {
                    status,
                    signal_sent,
                    escalated,
                });
            }
            _ = tokio::time::sleep(POLL) => {
                if !signal_sent && cancel.requested() {
                    signal_sent = true;
                    terminate(pid, libc::SIGTERM);
                    deadline = Some(tokio::time::Instant::now() + grace);
                } else if let Some(limit) = deadline {
                    if !escalated && tokio::time::Instant::now() >= limit {
                        escalated = true;
                        terminate(pid, libc::SIGKILL);
                    }
                }
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    fn script(body: &str) -> Child {
        tokio::process::Command::new("bash")
            .arg("-c")
            .arg(body)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn synthetic installer")
    }

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only probes existence.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[tokio::test]
    async fn no_request_completes_without_signalling() {
        let child = script("exit 0");
        let supervised = supervise(child, flag(|| false), DEFAULT_GRACE)
            .await
            .unwrap();
        assert!(supervised.status.success());
        assert!(!supervised.signal_sent && !supervised.escalated);
        assert_eq!(supervised.outcome(), Outcome::Committed);
    }

    #[tokio::test]
    async fn accepted_cancel_signals_term_lets_traps_run_and_never_claims_success() {
        let dir = std::env::temp_dir().join(format!(
            "dsh-ocean-bootstrap-supervisor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let marker = dir.join("staged-generation");
        let ready = dir.join("trap-installed");
        std::fs::write(&marker, b"staged").unwrap();
        let body = format!(
            "trap 'rm -f -- \"{}\"; exit 143' TERM\nprintf ready > \"{}\"\nsleep 30 &\nwait\n",
            marker.display(),
            ready.display()
        );
        let child = script(&body);
        let pid = child.id().unwrap() as i32;
        let cancel = Arc::new(AtomicBool::new(false));
        let supervisor_flag = {
            let viewed = cancel.clone();
            flag(move || viewed.load(Ordering::SeqCst))
        };
        // Arm the cancellation only after the script proved its trap is
        // installed; a fixed delay races a loaded CI runner and can deliver
        // TERM before the trap exists.
        let armed = cancel.clone();
        let ready_path = ready.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while tokio::time::Instant::now() < deadline {
                if ready_path.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(ready_path.exists(), "the fixture never installed its trap");
            armed.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let supervised = supervise(child, supervisor_flag, DEFAULT_GRACE)
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(supervised.signal_sent && !supervised.escalated);
        assert!(!supervised.status.success());
        assert_eq!(supervised.outcome(), Outcome::Cancelled);
        // The TERM trap ran, so the partially staged generation was removed.
        assert!(!Path::new(&marker).exists(), "TERM trap did not clean up");
        assert!(!alive(pid), "supervised child outlived cancellation");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn repeated_requests_do_not_escalate_while_term_is_respected() {
        let child = script("trap 'exit 130' TERM\nsleep 30 &\nwait\n");
        let supervised = supervise(child, flag(|| true), DEFAULT_GRACE)
            .await
            .unwrap();
        assert!(supervised.signal_sent && !supervised.escalated);
        assert_eq!(supervised.outcome(), Outcome::Cancelled);
    }

    #[tokio::test]
    async fn surviving_grandchild_is_swept_after_a_fast_successful_exit() {
        // bash exits 0 immediately while a TERM-ignoring grandchild stays in the
        // installer group; the sweep must not abandon it.
        let child = script("trap '' TERM\n(trap '' TERM; sleep 30) &\nexit 0\n");
        let pgid = child.id().unwrap() as i32;
        let supervised = supervise(child, flag(|| true), Duration::from_millis(500))
            .await
            .unwrap();
        assert!(!supervised.signal_sent, "no cancel was requested");
        assert!(supervised.escalated, "grandchild was never swept");
        assert!(
            !group_alive(pgid),
            "a process from the installer group survived"
        );
    }

    #[tokio::test]
    async fn ignored_term_escalates_to_kill_within_bounded_grace() {
        let child = script("trap '' TERM\nsleep 30 &\nwait\n");
        let started = Instant::now();
        let supervised = supervise(child, flag(|| true), Duration::from_millis(300))
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(supervised.signal_sent && supervised.escalated);
        assert!(!supervised.status.success());
        assert_eq!(supervised.outcome(), Outcome::Cancelled);
    }

    #[tokio::test]
    async fn late_cancel_cannot_rewrite_a_committed_exit() {
        let child = script("exit 0");
        // The flag is already set, but the child exits successfully first.
        let supervised = supervise(child, flag(|| true), DEFAULT_GRACE)
            .await
            .unwrap();
        assert!(supervised.status.success());
        assert_eq!(supervised.outcome(), Outcome::Committed);
    }

    #[tokio::test]
    async fn failure_without_cancellation_is_reported_as_failed() {
        let child = script("exit 7");
        let supervised = supervise(child, flag(|| false), DEFAULT_GRACE)
            .await
            .unwrap();
        assert!(!supervised.signal_sent);
        assert_eq!(supervised.outcome(), Outcome::Failed);
    }
}

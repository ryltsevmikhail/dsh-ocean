//! One native update operation's cancellation/progress boundary. No GTK objects.
use anyhow::Result;
use serde::Serialize;
use std::process::Child;
use std::sync::{Arc, Mutex};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    #[default]
    Checking,
    Downloading,
    Preparing,
    Verifying,
    Committing,
    Finished,
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: Phase,
    pub bytes: u64,
    pub total: Option<u64>,
    pub cancel_requested: bool,
    pub finished: bool,
}
#[derive(Default)]
struct State {
    progress: Progress,
    child: Option<Arc<Mutex<Child>>>,
}
#[derive(Default)]
pub struct Control(Mutex<State>);
impl Control {
    /// True means cancellation won the operation boundary. Kill is non-waiting;
    /// the worker remains responsible for reaping its exact owned child.
    pub fn cancel(&self) -> bool {
        let child = {
            let mut state = self.0.lock().expect("update control");
            if state.progress.finished || state.progress.phase == Phase::Committing {
                return false;
            }
            state.progress.cancel_requested = true;
            state.child.clone()
        };
        if let Some(child) = child {
            match child.try_lock() {
                Ok(mut child) => {
                    let _ = child.kill();
                }
                Err(std::sync::TryLockError::Poisoned(error)) => {
                    let _ = error.into_inner().kill();
                }
                // The worker holds this lock only for try_wait or kill+reap.
                // Its next cancellation checkpoint/Drop performs cleanup; GTK
                // must never wait behind the reaper's blocking wait.
                Err(std::sync::TryLockError::WouldBlock) => (),
            };
        }
        true
    }
    pub fn check(&self) -> Result<()> {
        let state = self.0.lock().expect("update control");
        anyhow::ensure!(
            !state.progress.cancel_requested && !state.progress.finished,
            "update operation cancelled or finished"
        );
        Ok(())
    }
    pub fn phase(&self, phase: Phase, total: Option<u64>) -> Result<()> {
        let mut state = self.0.lock().expect("update control");
        anyhow::ensure!(
            !state.progress.cancel_requested
                && !state.progress.finished
                && state.progress.phase != Phase::Committing,
            "update operation is not preparable"
        );
        anyhow::ensure!(
            !matches!(phase, Phase::Committing | Phase::Finished),
            "use the update commitment boundary"
        );
        state.progress.phase = phase;
        state.progress.bytes = 0;
        state.progress.total = total;
        Ok(())
    }
    pub fn observe(&self, bytes: u64) -> Result<()> {
        let mut state = self.0.lock().expect("update control");
        anyhow::ensure!(
            !state.progress.cancel_requested && !state.progress.finished,
            "update operation cancelled or finished"
        );
        state.progress.bytes = bytes;
        Ok(())
    }
    pub fn begin_commit(&self) -> Result<()> {
        let mut state = self.0.lock().expect("update control");
        anyhow::ensure!(
            !state.progress.cancel_requested
                && !state.progress.finished
                && state.progress.phase != Phase::Committing,
            "update cancellation/commit boundary already crossed"
        );
        state.progress.phase = Phase::Committing;
        Ok(())
    }
    /// Linearize read-only results too, so late cancellation cannot publish an
    /// Available result after the dialog already acknowledged cancellation.
    pub fn complete(&self) -> bool {
        let mut state = self.0.lock().expect("update control");
        state.progress.finished = true;
        state.progress.phase = Phase::Finished;
        !state.progress.cancel_requested
    }
    pub fn snapshot(&self) -> Progress {
        self.0.lock().expect("update control").progress.clone()
    }
    pub(crate) fn attach(&self, child: Arc<Mutex<Child>>) -> Result<()> {
        let mut state = self.0.lock().expect("update control");
        anyhow::ensure!(
            !state.progress.cancel_requested
                && !state.progress.finished
                && state.progress.phase != Phase::Committing
                && state.child.is_none(),
            "update transfer cancelled, finished or already active"
        );
        state.child = Some(child);
        Ok(())
    }
    pub(crate) fn detach(&self, child: &Arc<Mutex<Child>>) {
        let mut state = self.0.lock().expect("update control");
        if state
            .child
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, child))
        {
            state.child = None;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_wins_before_commit_and_blocks_late_publication() {
        let control = Control::default();
        assert!(control.cancel());
        assert!(control.check().is_err());
        assert!(control.begin_commit().is_err());
        assert!(!control.complete());
    }
    #[test]
    fn commit_wins_once_and_cannot_be_reported_as_cancelled() {
        let control = Control::default();
        control.begin_commit().unwrap();
        assert!(!control.cancel());
        assert!(control.check().is_ok());
        assert!(control.begin_commit().is_err());
        assert!(control.complete());
        assert!(!control.cancel());
    }
    #[test]
    fn cancellation_does_not_wait_for_the_workers_child_mutex() {
        let control = Arc::new(Control::default());
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let child = Arc::new(Mutex::new(child));
        control.attach(child.clone()).unwrap();
        let held = child.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || tx.send(worker_control.cancel()).unwrap());
        let prompt = rx.recv_timeout(std::time::Duration::from_millis(400)).ok();
        drop(held);
        if prompt.is_none() {
            assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap());
        }
        worker.join().unwrap();
        {
            let mut child = child.lock().unwrap();
            let _ = child.kill();
            child.wait().unwrap();
        }
        control.detach(&child);
        assert_eq!(
            prompt,
            Some(true),
            "native cancel must not block on worker cleanup"
        );
    }
    #[test]
    fn readonly_result_commit_and_late_cancel_are_linearized() {
        let control = Control::default();
        assert!(control.complete());
        assert!(!control.cancel());
        assert!(control.begin_commit().is_err());
    }
}

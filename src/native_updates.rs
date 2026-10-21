//! Native update operation gate and exit deferral; never a WebView capability.
use crate::update_control::Control;
use anyhow::Result;
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct State {
    active: Option<Arc<Control>>,
    closing: bool,
    waiting: bool,
}
#[derive(Default)]
pub struct Registry(Mutex<State>);
pub enum Exit {
    Allow,
    Wait(Arc<Control>),
    Waiting,
}
pub struct Operation {
    pub control: Arc<Control>,
    registry: Arc<Registry>,
}
impl Registry {
    pub fn begin(self: &Arc<Self>) -> Result<Operation> {
        let mut state = self.0.lock().expect("native update registry");
        anyhow::ensure!(
            !state.closing && state.active.is_none(),
            "another native update is running or the application is closing"
        );
        let control = Arc::new(Control::default());
        state.active = Some(control.clone());
        Ok(Operation {
            control,
            registry: self.clone(),
        })
    }
    pub fn busy(&self) -> bool {
        self.0
            .lock()
            .expect("native update registry")
            .active
            .is_some()
    }
    pub fn request_exit(&self) -> Exit {
        let active = {
            let mut state = self.0.lock().expect("native update registry");
            state.closing = true;
            state.active.clone()
        };
        let Some(control) = active.filter(|control| !control.snapshot().finished) else {
            return Exit::Allow;
        };
        control.cancel();
        let mut state = self.0.lock().expect("native update registry");
        if state.waiting {
            Exit::Waiting
        } else {
            state.waiting = true;
            Exit::Wait(control)
        }
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        self.control.complete();
        let mut state = self.registry.0.lock().expect("native update registry");
        if state
            .active
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &self.control))
        {
            state.active = None;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_operations_are_refused_and_drop_releases_only_its_own_gate() {
        let registry = Arc::new(Registry::default());
        let operation = registry.begin().unwrap();
        assert!(registry.busy());
        assert!(registry.begin().is_err());
        drop(operation);
        assert!(!registry.busy());
        assert!(registry.begin().is_ok());
    }
    #[test]
    fn exit_cancels_preparation_and_waits_for_owned_cleanup_without_new_work() {
        let registry = Arc::new(Registry::default());
        let operation = registry.begin().unwrap();
        assert!(matches!(registry.request_exit(), Exit::Wait(_)));
        assert!(operation.control.snapshot().cancel_requested);
        assert!(registry.begin().is_err());
        assert!(matches!(registry.request_exit(), Exit::Waiting));
        let control = operation.control.clone();
        drop(operation);
        assert!(control.snapshot().finished);
        assert!(matches!(registry.request_exit(), Exit::Allow));
    }
    #[test]
    fn exit_does_not_cancel_a_committing_local_transaction() {
        let registry = Arc::new(Registry::default());
        let operation = registry.begin().unwrap();
        operation.control.begin_commit().unwrap();
        assert!(matches!(registry.request_exit(), Exit::Wait(_)));
        assert!(!operation.control.snapshot().cancel_requested);
        drop(operation);
        assert!(matches!(registry.request_exit(), Exit::Allow));
        assert!(registry.begin().is_err());
    }
}

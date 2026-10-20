//! In-process runtime admission and ownership; no OS, GTK or blocking work under
//! the metadata lock. This does not arbitrate independent CLI/update processes or
//! roll back external side effects when an async operation is cancelled.
use std::fmt;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Prepare,
    Restart,
    Recover,
    Update,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Busy(Operation),
    Closed,
    Stale,
    ChildOwned,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy(operation) => write!(
                f,
                "Runtime {operation:?} is in progress; wait before retrying."
            ),
            Self::Closed => f.write_str("Ocean is shutting down; runtime changes are refused."),
            Self::Stale => f.write_str("The runtime changed; the old operation was discarded."),
            Self::ChildOwned => {
                f.write_str("A managed child is already owned; refusing replacement.")
            }
        }
    }
}
impl std::error::Error for Refused {}

struct State<S, C> {
    active: Option<Operation>,
    revision: u64,
    closed: bool,
    server: Option<S>,
    child: Option<C>,
    leased: bool,
}

struct Shared<S, C> {
    state: Mutex<State<S, C>>,
    cleanup: fn(C),
}

pub struct Lifecycle<S, C>(Arc<Shared<S, C>>);

impl<S, C> Lifecycle<S, C> {
    /// Cleanup runs outside the lock for a rejected late child or shutdown.
    pub fn new(cleanup: fn(C)) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                active: None,
                revision: 0,
                closed: false,
                server: None,
                child: None,
                leased: false,
            }),
            cleanup,
        }))
    }

    pub fn try_begin(&self, operation: Operation) -> Result<Permit<S, C>, Refused> {
        self.begin(operation, None)
    }

    /// Atomically reject a health observation taken before any intervening
    /// operation, even if the URL is unchanged or that operation failed.
    pub fn try_recover(&self, observed_revision: u64) -> Result<Permit<S, C>, Refused> {
        self.begin(Operation::Recover, Some(observed_revision))
    }

    fn begin(&self, operation: Operation, expected: Option<u64>) -> Result<Permit<S, C>, Refused> {
        let mut state = self.0.state.lock().expect("runtime lifecycle mutex");
        if state.closed {
            return Err(Refused::Closed);
        }
        if let Some(active) = state.active {
            return Err(Refused::Busy(active));
        }
        if expected.is_some_and(|revision| revision != state.revision) {
            return Err(Refused::Stale);
        }
        state.revision = state.revision.wrapping_add(1);
        state.active = Some(operation);
        Ok(Permit(Arc::new(Reservation {
            shared: self.0.clone(),
            revision: state.revision,
            operation,
        })))
    }

    pub fn server(&self) -> Option<S>
    where
        S: Clone,
    {
        self.0
            .state
            .lock()
            .expect("runtime lifecycle mutex")
            .server
            .clone()
    }

    pub fn is_updating(&self) -> bool {
        self.0.state.lock().expect("runtime lifecycle mutex").active == Some(Operation::Update)
    }

    /// A consistent idle server/revision pair, or no usable health observation.
    pub fn observation(&self) -> Option<(u64, S)>
    where
        S: Clone,
    {
        let state = self.0.state.lock().expect("runtime lifecycle mutex");
        if state.closed || state.active.is_some() {
            return None;
        }
        state.server.clone().map(|server| (state.revision, server))
    }

    pub fn is_idle_revision(&self, revision: u64) -> bool {
        let state = self.0.state.lock().expect("runtime lifecycle mutex");
        !state.closed && state.active.is_none() && state.revision == revision
    }

    /// Terminal transition, atomic with publication. Never waits for an async
    /// operation; a temporarily leased child is cleaned up when its lease drops.
    pub fn close(&self) {
        let child = {
            let mut state = self.0.state.lock().expect("runtime lifecycle mutex");
            state.closed = true;
            state.child.take()
        };
        if let Some(child) = child {
            (self.0.cleanup)(child);
        }
    }
}

struct Reservation<S, C> {
    shared: Arc<Shared<S, C>>,
    revision: u64,
    operation: Operation,
}

impl<S, C> Reservation<S, C> {
    fn check(&self, state: &State<S, C>) -> Result<(), Refused> {
        if state.closed {
            Err(Refused::Closed)
        } else if state.revision != self.revision || state.active != Some(self.operation) {
            Err(Refused::Stale)
        } else {
            Ok(())
        }
    }
}

impl<S, C> Drop for Reservation<S, C> {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().expect("runtime lifecycle mutex");
        if state.revision == self.revision {
            state.active = None;
        }
    }
}

/// Owned and Send when the resources are Send. No MutexGuard crosses an await.
/// A child lease extends admission until ownership has been restored/discarded.
pub struct Permit<S, C>(Arc<Reservation<S, C>>);

impl<S, C> Permit<S, C> {
    pub fn operation(&self) -> Operation {
        self.0.operation
    }

    pub fn ensure_open(&self) -> Result<(), Refused> {
        self.0
            .check(&self.0.shared.state.lock().expect("runtime lifecycle mutex"))
    }

    pub fn server(&self) -> Result<Option<S>, Refused>
    where
        S: Clone,
    {
        let state = self.0.shared.state.lock().expect("runtime lifecycle mutex");
        self.0.check(&state)?;
        Ok(state.server.clone())
    }

    /// Existing child ownership must be handled before any fresh provisioning.
    pub fn owned_server(&self) -> Result<Option<S>, Refused>
    where
        S: Clone,
    {
        let state = self.0.shared.state.lock().expect("runtime lifecycle mutex");
        self.0.check(&state)?;
        Ok(if state.child.is_some() || state.leased {
            state.server.clone()
        } else {
            None
        })
    }

    pub fn publish_server(&self, server: S) -> Result<(), Refused> {
        let mut state = self.0.shared.state.lock().expect("runtime lifecycle mutex");
        self.0.check(&state)?;
        if state.child.is_some() || state.leased {
            return Err(Refused::ChildOwned);
        }
        state.server = Some(server);
        Ok(())
    }

    /// The caller keeps a rejected child and must stop it outside this lock.
    pub fn publish_child(&self, server: S, child: C) -> Result<(), (Refused, C)> {
        let mut state = self.0.shared.state.lock().expect("runtime lifecycle mutex");
        if let Err(error) = self.0.check(&state) {
            return Err((error, child));
        }
        if state.child.is_some() || state.leased {
            return Err((Refused::ChildOwned, child));
        }
        state.child = Some(child);
        state.server = Some(server);
        Ok(())
    }

    pub fn lease_child(&self) -> Result<ChildLease<S, C>, Refused> {
        let mut state = self.0.shared.state.lock().expect("runtime lifecycle mutex");
        self.0.check(&state)?;
        if state.leased {
            return Err(Refused::ChildOwned);
        }
        state.leased = true;
        Ok(ChildLease {
            reservation: self.0.clone(),
            child: state.child.take(),
        })
    }

    pub fn stamp(&self) -> Stamp<S, C> {
        Stamp {
            shared: self.0.shared.clone(),
            revision: self.0.revision,
        }
    }
}

pub struct ChildLease<S, C> {
    reservation: Arc<Reservation<S, C>>,
    child: Option<C>,
}

impl<S, C> ChildLease<S, C> {
    pub fn child_mut(&mut self) -> Option<&mut C> {
        self.child.as_mut()
    }

    /// Only after successful stopping/reaping: the old handle is no longer owned.
    pub fn discard_stopped(mut self) {
        self.child.take();
    }
}

impl<S, C> Drop for ChildLease<S, C> {
    fn drop(&mut self) {
        let child = self.child.take();
        let mut state = self
            .reservation
            .shared
            .state
            .lock()
            .expect("runtime lifecycle mutex");
        state.leased = false;
        if let Some(child) = child {
            if !state.closed && state.child.is_none() {
                state.child = Some(child);
            } else {
                drop(state);
                (self.reservation.shared.cleanup)(child);
            }
        }
    }
}

/// A queued GTK callback may run after admission is released. It may use this
/// stamp to skip obsolete navigation without acquiring/waiting for admission.
pub struct Stamp<S, C> {
    shared: Arc<Shared<S, C>>,
    revision: u64,
}

impl<S, C> Stamp<S, C> {
    pub fn is_current(&self) -> bool {
        let state = self.shared.state.lock().expect("runtime lifecycle mutex");
        !state.closed && state.revision == self.revision
    }
}

#[derive(Default)]
pub struct HealthFailures {
    revision: Option<u64>,
    failures: u8,
}

impl HealthFailures {
    pub fn reset(&mut self) {
        self.revision = None;
        self.failures = 0;
    }

    pub fn failed(&mut self, revision: u64) -> bool {
        if self.revision != Some(revision) {
            self.failures = 0;
            self.revision = Some(revision);
        }
        self.failures += 1;
        if self.failures < 2 {
            return false;
        }
        self.reset();
        true
    }
}

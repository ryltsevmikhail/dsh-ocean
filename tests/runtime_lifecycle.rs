//! Deterministic production-coordinator tests. No GTK, DSH, network, environment
//! mutation, user services or real processes. Existing managed-child/native CI
//! suites separately cover OS group cleanup and window recovery.
use dsh_ocean::runtime_lifecycle::{HealthFailures, Lifecycle, Operation, Refused};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Debug)]
struct Child {
    id: u64,
    dropped: Arc<AtomicUsize>,
}
impl Drop for Child {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
type Runtime = Lifecycle<String, Child>;
fn runtime() -> (Arc<Runtime>, Arc<AtomicUsize>) {
    let dropped = Arc::new(AtomicUsize::new(0));
    (Arc::new(Lifecycle::new(drop)), dropped)
}
fn seed(runtime: &Runtime, dropped: &Arc<AtomicUsize>) {
    let permit = runtime.try_begin(Operation::Prepare).unwrap();
    permit
        .publish_child(
            "same-origin".into(),
            Child {
                id: 1,
                dropped: dropped.clone(),
            },
        )
        .unwrap();
}
const OPERATIONS: [Operation; 4] = [
    Operation::Prepare,
    Operation::Restart,
    Operation::Recover,
    Operation::Update,
];

#[test]
fn every_operation_excludes_every_other_operation() {
    for first in OPERATIONS {
        for second in OPERATIONS {
            let (runtime, _) = runtime();
            let permit = runtime.try_begin(first).unwrap();
            assert_eq!(runtime.try_begin(second).err(), Some(Refused::Busy(first)));
            assert!(runtime.observation().is_none());
            assert_eq!(runtime.is_updating(), first == Operation::Update);
            drop(permit);
            assert!(!runtime.is_updating());
            assert!(runtime.try_begin(second).is_ok());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn competing_async_mutations_never_queue_or_overlap_even_during_refresh() {
    // Includes manual/manual, manual/health, systemd recovery/update and startup.
    for first in OPERATIONS {
        for second in OPERATIONS {
            let (runtime, _) = runtime();
            let live = Arc::new(AtomicUsize::new(0));
            let peak = Arc::new(AtomicUsize::new(0));
            let starts = Arc::new(AtomicUsize::new(0));
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
            let task = {
                let runtime = runtime.clone();
                let live = live.clone();
                let peak = peak.clone();
                let starts = starts.clone();
                tokio::spawn(async move {
                    let permit = runtime.try_begin(first).unwrap();
                    starts.fetch_add(1, Ordering::SeqCst);
                    peak.fetch_max(live.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    permit.publish_server("same-origin".into()).unwrap();
                    // Models backend completion followed by GUI refresh/rollback
                    // acknowledgement: admission must remain owned across await.
                    entered_tx.send(()).unwrap();
                    finish_rx.await.unwrap();
                    permit.ensure_open().unwrap();
                    live.fetch_sub(1, Ordering::SeqCst);
                })
            };
            entered_rx.await.unwrap();
            assert_eq!(runtime.try_begin(second).err(), Some(Refused::Busy(first)));
            assert_eq!(starts.load(Ordering::SeqCst), 1);
            finish_tx.send(()).unwrap();
            task.await.unwrap();
            assert_eq!(peak.load(Ordering::SeqCst), 1);
            assert_eq!(live.load(Ordering::SeqCst), 0);
            assert!(runtime.try_begin(second).is_ok());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_reserves_the_empty_child_slot_until_replacement_is_published() {
    let (runtime, dropped) = runtime();
    seed(&runtime, &dropped);
    let (old_revision, _) = runtime.observation().unwrap();
    let (empty_tx, empty_rx) = tokio::sync::oneshot::channel();
    let (publish_tx, publish_rx) = tokio::sync::oneshot::channel();
    let task = {
        let runtime = runtime.clone();
        let dropped = dropped.clone();
        tokio::spawn(async move {
            let permit = runtime.try_begin(Operation::Restart).unwrap();
            let mut old = permit.lease_child().unwrap();
            assert_eq!(old.child_mut().unwrap().id, 1);
            old.discard_stopped();
            empty_tx.send(()).unwrap();
            publish_rx.await.unwrap();
            permit
                .publish_child("same-origin".into(), Child { id: 2, dropped })
                .unwrap();
        })
    };
    empty_rx.await.unwrap();
    assert_eq!(
        runtime.try_begin(Operation::Restart).err(),
        Some(Refused::Busy(Operation::Restart))
    );
    assert_eq!(
        runtime.try_recover(old_revision).err(),
        Some(Refused::Busy(Operation::Restart))
    );
    assert_eq!(
        runtime.try_begin(Operation::Update).err(),
        Some(Refused::Busy(Operation::Restart))
    );
    publish_tx.send(()).unwrap();
    task.await.unwrap();
    assert_eq!(
        runtime.try_recover(old_revision).err(),
        Some(Refused::Stale)
    );
    let permit = runtime.try_begin(Operation::Restart).unwrap();
    let mut current = permit.lease_child().unwrap();
    assert_eq!(current.child_mut().unwrap().id, 2);
    drop(current);
    drop(permit);
    runtime.close();
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
}

#[test]
fn stop_error_or_unwind_restores_handle_and_never_replaces_it() {
    let (runtime, dropped) = runtime();
    seed(&runtime, &dropped);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let permit = runtime.try_begin(Operation::Restart).unwrap();
        let mut old = permit.lease_child().unwrap();
        assert_eq!(old.child_mut().unwrap().id, 1);
        panic!("synthetic stop failure before replacement");
    }));
    assert!(result.is_err());
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    let permit = runtime.try_begin(Operation::Restart).unwrap();
    let mut old = permit.lease_child().unwrap();
    assert_eq!(old.child_mut().unwrap().id, 1);
    drop(old);
    drop(permit);
    runtime.close();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_abort_releases_admission_but_preserves_a_leased_old_handle() {
    let (runtime, dropped) = runtime();
    seed(&runtime, &dropped);
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            let permit = runtime.try_begin(Operation::Restart).unwrap();
            let mut child = permit.lease_child().unwrap();
            assert_eq!(child.child_mut().unwrap().id, 1);
            entered_tx.send(()).unwrap();
            std::future::pending::<()>().await;
            drop(child);
        })
    };
    entered_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let permit = runtime.try_begin(Operation::Restart).unwrap();
    let mut old = permit.lease_child().unwrap();
    assert_eq!(old.child_mut().unwrap().id, 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    drop(old);
    drop(permit);
    runtime.close();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn lease_extends_admission_until_ownership_is_restored() {
    let (runtime, dropped) = runtime();
    seed(&runtime, &dropped);
    let permit = runtime.try_begin(Operation::Restart).unwrap();
    let lease = permit.lease_child().unwrap();
    drop(permit);
    assert_eq!(
        runtime.try_begin(Operation::Update).err(),
        Some(Refused::Busy(Operation::Restart))
    );
    drop(lease);
    assert!(runtime.try_begin(Operation::Update).is_ok());
    runtime.close();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn shutdown_before_or_after_publication_keeps_exactly_once_ownership() {
    for publish_first in [false, true] {
        let (runtime, dropped) = runtime();
        let permit = runtime.try_begin(Operation::Prepare).unwrap();
        let stamp = permit.stamp();
        let child = Child {
            id: 1,
            dropped: dropped.clone(),
        };
        if publish_first {
            permit.publish_child("same-origin".into(), child).unwrap();
            runtime.close();
        } else {
            runtime.close();
            let (error, rejected) = permit
                .publish_child("same-origin".into(), child)
                .unwrap_err();
            assert_eq!(error, Refused::Closed);
            drop(rejected); // caller cleans up outside coordinator lock.
        }
        runtime.close();
        assert!(!stamp.is_current());
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        drop(permit);
        assert_eq!(
            runtime.try_begin(Operation::Restart).err(),
            Some(Refused::Closed)
        );
    }
    let (runtime, dropped) = runtime();
    seed(&runtime, &dropped);
    let permit = runtime.try_begin(Operation::Restart).unwrap();
    let lease = permit.lease_child().unwrap();
    runtime.close();
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    drop(permit);
    drop(lease);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime.try_begin(Operation::Update).err(),
        Some(Refused::Closed)
    );
}

#[test]
fn publication_cannot_silently_drop_a_previously_owned_child() {
    let (runtime, dropped) = runtime();
    seed(&runtime, &dropped);
    let permit = runtime.try_begin(Operation::Restart).unwrap();
    let (error, rejected) = permit
        .publish_child(
            "new-origin".into(),
            Child {
                id: 2,
                dropped: dropped.clone(),
            },
        )
        .unwrap_err();
    assert_eq!(error, Refused::ChildOwned);
    assert_eq!(runtime.server().as_deref(), Some("same-origin"));
    drop(rejected);
    let mut old = permit.lease_child().unwrap();
    assert_eq!(old.child_mut().unwrap().id, 1);
    drop(old);
    drop(permit);
    runtime.close();
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
}

#[test]
fn stale_probes_and_queued_reload_stamps_are_invalid_even_when_url_is_unchanged() {
    let (runtime, _) = runtime();
    let prepare = runtime.try_begin(Operation::Prepare).unwrap();
    prepare.publish_server("same-origin".into()).unwrap();
    let old_callback = prepare.stamp();
    drop(prepare);
    let (revision, _) = runtime.observation().unwrap();
    assert!(old_callback.is_current());
    let update = runtime.try_begin(Operation::Update).unwrap();
    assert!(!old_callback.is_current());
    assert!(!runtime.is_idle_revision(revision));
    // Even failure with no publication invalidates the old health observation.
    drop(update);
    assert_eq!(runtime.try_recover(revision).err(), Some(Refused::Stale));
    let (new_revision, server) = runtime.observation().unwrap();
    assert_eq!(server, "same-origin");
    let recovery = runtime.try_recover(new_revision).unwrap();
    let callback = recovery.stamp();
    drop(recovery);
    assert!(callback.is_current());
    runtime.close();
    assert!(!callback.is_current());
}

#[test]
fn leased_ownership_cannot_be_replaced_or_leased_twice_even_by_the_same_permit() {
    let (runtime, dropped) = runtime();
    seed(&runtime, &dropped);
    let permit = runtime.try_begin(Operation::Restart).unwrap();
    let lease = permit.lease_child().unwrap();
    assert_eq!(permit.lease_child().err(), Some(Refused::ChildOwned));
    assert_eq!(
        permit.owned_server().unwrap().as_deref(),
        Some("same-origin")
    );
    assert_eq!(
        permit.publish_server("unrelated-origin".into()),
        Err(Refused::ChildOwned)
    );
    let (error, rejected) = permit
        .publish_child(
            "replacement".into(),
            Child {
                id: 2,
                dropped: dropped.clone(),
            },
        )
        .unwrap_err();
    assert_eq!(error, Refused::ChildOwned);
    drop(rejected);
    drop(lease); // Failed stopping restores the original handle, not replacement.
    let mut old = permit.lease_child().unwrap();
    assert_eq!(old.child_mut().unwrap().id, 1);
    old.discard_stopped();
    assert!(permit.owned_server().unwrap().is_none());
    permit
        .publish_child(
            "replacement".into(),
            Child {
                id: 3,
                dropped: dropped.clone(),
            },
        )
        .unwrap();
    drop(permit);
    runtime.close();
    assert_eq!(dropped.load(Ordering::SeqCst), 3);
}

#[test]
fn failed_window_dispatch_preserves_the_owned_server_for_a_prepare_retry() {
    let (runtime, dropped) = runtime();
    seed(&runtime, &dropped);
    // Publication succeeded but a GTK dispatch failed/timed out. No teardown is
    // implied by the operation ending; a retry can recover the actual random-port
    // server rather than provisioning against the configured fixed origin.
    let retry = runtime.try_begin(Operation::Prepare).unwrap();
    assert_eq!(
        retry.owned_server().unwrap().as_deref(),
        Some("same-origin")
    );
    assert_eq!(
        retry.publish_server("fixed-origin".into()),
        Err(Refused::ChildOwned)
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    drop(retry);
    runtime.close();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn shutdown_during_update_preparation_refuses_the_late_launch_check() {
    let (runtime, _) = runtime();
    let permit = runtime.try_begin(Operation::Update).unwrap();
    runtime.close(); // Could occur while synchronous update preparation finishes.
    let starts = AtomicUsize::new(0);
    if permit.ensure_open().is_ok() {
        starts.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(permit.ensure_open(), Err(Refused::Closed));
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    drop(permit);
    assert_eq!(
        runtime.try_begin(Operation::Restart).err(),
        Some(Refused::Closed)
    );
}

#[test]
fn consecutive_health_failures_are_scoped_to_one_idle_revision() {
    let mut failures = HealthFailures::default();
    assert!(!failures.failed(1));
    assert!(!failures.failed(2));
    assert!(failures.failed(2));
    assert!(!failures.failed(2));
    failures.reset(); // healthy probe, busy, closed or absent server.
    assert!(!failures.failed(2));
    assert!(failures.failed(2));
    assert!(!failures.failed(3));
}

//! Real GTK lifecycle of the progress windows, without a worker or user data.
#![cfg(target_os = "linux")]
use gtk::prelude::*;
use std::time::{Duration, Instant};

struct WindowGuard(gtk::Dialog);
impl Drop for WindowGuard {
    fn drop(&mut self) {
        // Test-owned widget: no later operations use it after teardown.
        unsafe { self.0.destroy() };
    }
}
fn pump() {
    let until = Instant::now() + Duration::from_millis(100);
    while Instant::now() < until {
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn request_cancel(dialog: &gtk::Dialog) {
    dialog.show_all();
    dialog
        .widget_for_response(gtk::ResponseType::Cancel)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap()
        .emit_clicked();
    pump();
    assert!(
        dialog.is_visible(),
        "Cancel must keep the running cleanup visible"
    );
    assert!(!dialog
        .widget_for_response(gtk::ResponseType::Cancel)
        .unwrap()
        .is_sensitive());
}

fn terminal_close(name: &str, dialog: gtk::Dialog, failures: &mut Vec<String>) {
    let _guard = WindowGuard(dialog.clone());
    let destroyed = std::rc::Rc::new(std::cell::Cell::new(false));
    let observed = destroyed.clone();
    dialog.connect_destroy(move |_| observed.set(true));
    let delete_events = std::rc::Rc::new(std::cell::Cell::new(0));
    let observed = delete_events.clone();
    dialog.connect_delete_event(move |_, _| {
        observed.set(observed.get() + 1);
        gtk::glib::Propagation::Proceed
    });
    dialog.show_all();
    assert!(dialog.is_modal());
    // The production completion timer and WM both enter through GTK close/delete-event.
    dialog.close();
    pump();
    if dialog.is_visible() || !destroyed.get() {
        failures.push(format!(
            "{name}: delete_events={}, visible={}, destroyed={}",
            delete_events.get(),
            dialog.is_visible(),
            destroyed.get()
        ));
    }
}

#[test]
#[ignore = "requires explicitly isolated GTK display"]
fn progress_windows_preserve_running_view_and_release_all_terminal_modals() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let running_snapshot = dsh_ocean::data_snapshot_ui::build_progress(None);
    let running_export = dsh_ocean::backup_export_ui::build_progress(None);
    let dialogs = [
        ("snapshot", running_snapshot.dialog.clone()),
        ("export", running_export.dialog.clone()),
    ];
    let mut closed = Vec::new();
    for (name, dialog) in dialogs {
        let _guard = WindowGuard(dialog.clone());
        dialog.show_all();
        dialog.close();
        pump();
        if !dialog.is_visible() {
            closed.push(name);
        }
    }
    assert!(
        closed.is_empty(),
        "running progress view closed before cooperative completion: {closed:?}"
    );
    assert!(running_snapshot.state.lock().unwrap().cancel_requested);
    assert!(running_export.state.lock().unwrap().cancel_requested);

    let parent = gtk::Dialog::new();
    let _parent_guard = WindowGuard(parent.clone());
    parent.show_all();
    let mut blocked = Vec::new();
    use dsh_ocean::{backup_export_ui as export, data_snapshot_ui as snapshot};
    // Keep each case independent: GTK can filter close events for a dialog
    // behind another active modal in the same window group.
    let cancelled_snapshot = snapshot::build_progress(Some(parent.upcast_ref()));
    request_cancel(&cancelled_snapshot.dialog);
    assert!(cancelled_snapshot.state.lock().unwrap().cancel_requested);
    cancelled_snapshot.state.lock().unwrap().outcome = Some(snapshot::Outcome::Cancelled);
    terminal_close(
        "snapshot after Cancel",
        cancelled_snapshot.dialog,
        &mut blocked,
    );
    let cancelled_export = export::build_progress(Some(parent.upcast_ref()));
    request_cancel(&cancelled_export.dialog);
    assert!(cancelled_export.state.lock().unwrap().cancel_requested);
    cancelled_export.state.lock().unwrap().outcome = Some(export::Outcome::Cancelled);
    terminal_close("export after Cancel", cancelled_export.dialog, &mut blocked);
    for outcome in [
        snapshot::Outcome::Created {
            files: 1,
            bytes: 2,
            consistent: true,
        },
        snapshot::Outcome::Cancelled,
        snapshot::Outcome::Failed,
    ] {
        let window = snapshot::build_progress(Some(parent.upcast_ref()));
        window.state.lock().unwrap().outcome = Some(outcome);
        terminal_close(
            &format!("snapshot {outcome:?}"),
            window.dialog,
            &mut blocked,
        );
        let state = window.state.lock().unwrap();
        assert!(
            !state.cancel_requested,
            "closing a finished operation must not request cancellation"
        );
        assert_eq!(state.outcome, Some(outcome));
    }
    for outcome in [
        export::Outcome::Exported { files: 1, bytes: 2 },
        export::Outcome::Cancelled,
        export::Outcome::Failed,
    ] {
        let window = export::build_progress(Some(parent.upcast_ref()));
        window.state.lock().unwrap().outcome = Some(outcome);
        terminal_close(&format!("export {outcome:?}"), window.dialog, &mut blocked);
        let state = window.state.lock().unwrap();
        assert!(!state.cancel_requested);
        assert_eq!(state.outcome, Some(outcome));
    }
    assert!(parent.is_visible());
    assert!(
        blocked.is_empty(),
        "finished modal still vetoes close and blocks its parent: {blocked:?}"
    );
}

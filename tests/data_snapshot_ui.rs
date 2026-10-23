//! GTK presentation only: synthetic counters, no inventory, worker, profile or snapshot I/O.
use dsh_ocean::{data_snapshot_ui as ui, session_inventory::Summary};
use gtk::prelude::*;

#[test]
#[ignore = "requires explicitly enabled isolated GTK dialog display"]
fn snapshot_consent_has_no_default_accept_and_progress_stays_plain() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let summary = Summary {
        session_files: 3,
        session_bytes: 1000,
        storage_files: 2,
        storage_bytes: 500,
        skipped: 0,
        truncated: false,
        error: Some("/private/secret".into()),
    };
    let body = ui::confirmation_text(&summary);
    let window = ui::build_confirm(None, &body);
    assert_eq!(window.dialog.title().as_deref(), Some(ui::TITLE));
    assert!(
        window.dialog.default_widget().is_none(),
        "copy must require explicit acceptance"
    );
    assert!(!window.label.uses_markup());
    assert!(!window.label.text().contains("/private/secret"));
    assert!(window
        .dialog
        .widget_for_response(gtk::ResponseType::Cancel)
        .is_some());
    assert!(window
        .dialog
        .widget_for_response(gtk::ResponseType::Accept)
        .is_some());
    assert!(window
        .dialog
        .widget_for_response(gtk::ResponseType::Apply)
        .is_none());
    let progress = ui::build_progress(None);
    assert!(!progress.label.uses_markup());
    assert!(progress
        .dialog
        .widget_for_response(gtk::ResponseType::Accept)
        .is_none());
    progress.dialog.close();
    window.dialog.close();
}

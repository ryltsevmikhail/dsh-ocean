//! One language per process; synthetic controls only, no profile, services or action callbacks.
use dsh_ocean::{i18n, localization::Preference};
use gtk::prelude::*;

fn named_button(widget: &gtk::Widget, name: &str) -> Option<gtk::Button> {
    if widget.widget_name() == name {
        return widget.clone().downcast().ok();
    }
    widget
        .clone()
        .downcast::<gtk::Container>()
        .ok()
        .and_then(|container| {
            container
                .children()
                .iter()
                .find_map(|child| named_button(child, name))
        })
}
fn button(dialog: &gtk::Dialog, response: gtk::ResponseType, key: &str) {
    let widget = dialog
        .widget_for_response(response)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    assert_eq!(
        widget.label().as_deref(),
        Some(i18n::message(i18n::active_language(), key).expect("embedded native label"))
    );
}

#[test]
#[ignore = "requires private GTK display and explicit test language"]
fn native_controls_follow_immutable_process_language_without_granting_actions() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    let code = std::env::var("DSH_OCEAN_TEST_LANGUAGE").expect("explicit test language");
    let language = i18n::LANGUAGES
        .into_iter()
        .find(|(l, _)| l.code() == code)
        .unwrap()
        .0;
    i18n::initialize(language).unwrap();
    assert!(
        i18n::initialize(if language == i18n::Language::En {
            i18n::Language::Ru
        } else {
            i18n::Language::En
        })
        .is_err(),
        "language snapshot must remain immutable"
    );
    assert_eq!(i18n::active_language(), language);
    gtk::init().unwrap();
    let body = "<b>synthetic & {path}</b>";
    let confirm =
        dsh_ocean::ui::build_confirmation(None, language, i18n::text("language.title"), body);
    assert_eq!(confirm.text().as_deref(), Some(body));
    assert!(!confirm.property::<bool>("use-markup"));
    button(confirm.upcast_ref(), gtk::ResponseType::Yes, "common.yes");
    button(confirm.upcast_ref(), gtk::ResponseType::No, "common.no");
    assert_eq!(
        confirm
            .default_widget()
            .map(|w| confirm.response_for_widget(&w)),
        Some(gtk::ResponseType::No)
    );
    confirm.close();
    let (picker, combo) =
        dsh_ocean::localization_ui::build(None, language, Preference::parse("ja").unwrap());
    assert_eq!(
        picker.title().as_deref(),
        Some(i18n::text("language.title"))
    );
    assert_eq!(combo.active_id().as_deref(), Some("ja"));
    button(&picker, gtk::ResponseType::Cancel, "common.cancel");
    picker.close();
    let (permission, label) = dsh_ocean::builtin_locale_ui::build(
        None,
        language,
        std::path::Path::new("/synthetic/<b>{path}</b>"),
    );
    assert!(!label.property::<bool>("use-markup"));
    assert!(label.text().contains("/synthetic/<b>{path}</b>"));
    assert_eq!(
        permission
            .default_widget()
            .map(|w| permission.response_for_widget(&w)),
        Some(gtk::ResponseType::Reject)
    );
    permission.close();
    // A real private fixture folder, never the user's Downloads or profile directory.
    let folder = std::path::PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap());
    assert!(
        folder.starts_with("/tmp")
            && folder
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("dsh-ocean-native-test.")
    );
    let chooser = dsh_ocean::download_chooser::build(
        None,
        "synthetic.txt",
        "http://user:secret@127.0.0.1:9/private?token=hidden",
        Some(&folder),
    );
    let title = i18n::render(
        language,
        "downloads.save_title",
        &[("origin", "http://127.0.0.1:9")],
    )
    .unwrap();
    assert_eq!(chooser.title().as_deref(), Some(title.as_str()));
    button(
        chooser.upcast_ref(),
        gtk::ResponseType::Accept,
        "downloads.save_new",
    );
    let hint = chooser
        .extra_widget()
        .unwrap()
        .downcast::<gtk::Label>()
        .unwrap();
    assert_eq!(hint.text().as_str(), i18n::text("downloads.no_overwrite"));
    assert!(!hint.property::<bool>("use-markup"));
    chooser.close();
    let verify_picker = dsh_ocean::profile_verify_ui::build_picker(None, Some(&folder));
    let export_picker = dsh_ocean::backup_export_ui::build_snapshot_picker(None, Some(&folder));
    let destination = dsh_ocean::backup_export_ui::build_destination_picker(None, Some(&folder));
    for (dialog, title, accept) in [
        (
            &verify_picker,
            "verification.picker_title",
            "verification.select",
        ),
        (
            &export_picker,
            "export.snapshot_picker",
            "export.snapshot_choose",
        ),
        (
            &destination,
            "export.destination_picker",
            "export.destination_choose",
        ),
    ] {
        assert_eq!(
            dialog.title().as_deref(),
            Some(i18n::message(language, title).unwrap())
        );
        assert!(dialog.is_local_only());
        button(
            dialog.upcast_ref(),
            gtk::ResponseType::Cancel,
            "common.cancel",
        );
        button(dialog.upcast_ref(), gtk::ResponseType::Accept, accept);
        dialog.close();
    }
    let data = dsh_ocean::data_snapshot_ui::build_confirm(None, body);
    let export = dsh_ocean::backup_export_ui::build_confirm(None, body);
    for (dialog, label, title, accept) in [
        (
            &data.dialog,
            &data.label,
            "data_snapshot.title",
            "data_snapshot.create",
        ),
        (
            &export.dialog,
            &export.label,
            "export.title",
            "export.export",
        ),
    ] {
        dialog.show_all();
        assert_eq!(
            dialog.title().as_deref(),
            Some(i18n::message(language, title).unwrap())
        );
        assert_eq!(label.text().as_str(), body);
        assert!(!label.property::<bool>("use-markup"));
        assert!(
            dialog.default_widget().is_none(),
            "snapshot/export must never default to acceptance"
        );
        button(dialog, gtk::ResponseType::Cancel, "common.cancel");
        button(dialog, gtk::ResponseType::Accept, accept);
        dialog.close();
    }
    let data_progress = dsh_ocean::data_snapshot_ui::build_progress(None);
    let export_progress = dsh_ocean::backup_export_ui::build_progress(None);
    for (dialog, label, title, cancel) in [
        (
            &data_progress.dialog,
            &data_progress.label,
            "data_snapshot.title",
            "data_snapshot.cancel",
        ),
        (
            &export_progress.dialog,
            &export_progress.label,
            "export.title",
            "export.cancel",
        ),
    ] {
        assert_eq!(
            dialog.title().as_deref(),
            Some(i18n::message(language, title).unwrap())
        );
        assert!(!label.property::<bool>("use-markup"));
        button(dialog, gtk::ResponseType::Cancel, cancel);
        dialog.close();
    }
    let records = std::sync::Arc::new(std::sync::Mutex::new(
        dsh_ocean::downloads::History::default(),
    ));
    let downloads = dsh_ocean::download_history::build(None, records.clone());
    assert_eq!(
        downloads.dialog.title().as_deref(),
        Some(i18n::text("downloads.history.title"))
    );
    button(
        &downloads.dialog,
        gtk::ResponseType::Other(1),
        "downloads.history.clear_finished",
    );
    button(
        &downloads.dialog,
        gtk::ResponseType::Other(2),
        "downloads.history.cancel_all",
    );
    button(
        &downloads.dialog,
        gtk::ResponseType::Other(3),
        "downloads.history.cancel_selected",
    );
    assert!(!downloads.cancel.is_sensitive() && !downloads.cancel_selected.is_sensitive());
    assert!(records.lock().unwrap().entries().is_empty());
    downloads.dialog.close();
    // Verification of our own synthetic snapshot: no existing home/profile is inspected.
    let home = folder.join("synthetic-home");
    let profile = home.join("profiles/web");
    std::fs::create_dir_all(&profile).unwrap();
    let content = b"{\"dsh\":{\"profile\":{\"bundles\":[]}}}\n";
    std::fs::write(profile.join("package.json"), content).unwrap();
    let snapshot = dsh_ocean::profile_backup::ProfileStore::new(&home, &folder.join("backups"))
        .unwrap()
        .create("web")
        .unwrap();
    let manifest = std::fs::read(snapshot.join("manifest.json")).unwrap();
    let summary = dsh_ocean::profile_verify_ui::verify_snapshot(&snapshot).unwrap();
    let verified = dsh_ocean::profile_verify_ui::build_summary(None, &summary);
    assert_eq!(
        verified.dialog.title().as_deref(),
        Some(i18n::text("verification.summary_title"))
    );
    assert_eq!(verified.label.text().as_str(), summary.text_in(language));
    assert!(!verified.label.property::<bool>("use-markup"));
    button(&verified.dialog, gtk::ResponseType::Close, "common.close");
    verified.dialog.close();
    let work = std::sync::Arc::new(dsh_ocean::profile_verify_ui::Work::default());
    let verifying = dsh_ocean::profile_verify_ui::build_progress(None, work.clone());
    assert_eq!(
        verifying.dialog.title().as_deref(),
        Some(i18n::text("verification.progress_title"))
    );
    assert_eq!(
        verifying.label.text().as_str(),
        work.progress_text_in(language)
    );
    button(
        &verifying.dialog,
        gtk::ResponseType::Cancel,
        "verification.cancel",
    );
    verifying.dialog.response(gtk::ResponseType::Cancel);
    assert!(verifying
        .label
        .text()
        .contains(i18n::text("verification.cancelling")));
    assert!(work.take_result().is_none() && !work.finished());
    // No worker started; run only the read-only verification of this synthetic snapshot to settle the cancellation.
    work.run(&snapshot);
    assert!(work.finished());
    assert!(matches!(
        work.take_result(),
        Some(dsh_ocean::profile_verify_ui::Outcome::Cancelled)
    ));
    verifying.dialog.close();
    assert_eq!(
        std::fs::read(snapshot.join("manifest.json")).unwrap(),
        manifest
    );
    assert_eq!(
        std::fs::read(profile.join("package.json")).unwrap(),
        content
    );

    // The updater builder auto-inspects state, so override inherited explicit paths before it starts.
    std::env::set_var("DSH_HOME", folder.join("isolated-dsh"));
    std::env::set_var("DSH_OCEAN_DATA_DIR", folder.join("isolated-ocean"));
    std::env::set_var("DSH_OCEAN_URL", "http://127.0.0.1:9");
    let settings = dsh_ocean::update_settings::path();
    assert!(!settings.exists());
    let registry = std::sync::Arc::new(dsh_ocean::native_updates::Registry::default());
    let report = dsh_ocean::shell_status::classify(None, false, false);
    let shell = dsh_ocean::shell_status_ui::build_with_registry(None, &report, registry.clone());
    shell.dialog.show_all();
    let refresh = named_button(shell.dialog.upcast_ref(), "shell-update-refresh").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while registry.busy() || !refresh.is_sensitive() {
        assert!(
            std::time::Instant::now() < deadline,
            "private updater inspection did not settle"
        );
        for _ in 0..32 {
            if !gtk::glib::MainContext::default().pending() {
                break;
            }
            gtk::glib::MainContext::default().iteration(false);
        }
        std::thread::yield_now();
    }
    assert_eq!(
        shell.dialog.title().as_deref(),
        Some(i18n::text("shell_ui.title"))
    );
    assert_eq!(
        shell.label.text().as_str(),
        dsh_ocean::shell_status::introduction_text_in(&report, language)
    );
    assert!(!shell.label.property::<bool>("use-markup"));
    for action in [
        "refresh", "check", "download", "apply", "enroll", "rollback", "recover", "cancel",
    ] {
        let widget =
            named_button(shell.dialog.upcast_ref(), &format!("shell-update-{action}")).unwrap();
        assert_eq!(
            widget.label().as_deref(),
            Some(i18n::message(language, &format!("shell_ui.{action}")).unwrap())
        );
        if ["check", "download", "apply", "rollback", "recover"].contains(&action) {
            assert!(
                !widget.is_sensitive(),
                "unconfigured updater must not offer {action}"
            );
        }
    }
    button(&shell.dialog, gtk::ResponseType::Close, "common.close");
    shell.dialog.close();
    assert!(!registry.busy() && !settings.exists());
    assert!(!dsh_ocean::managed_shell::prefix().exists());
    let storage = dsh_ocean::storage_ui::build(None, body);
    let plugins = dsh_ocean::plugins_ui::build(None, body);
    let history = dsh_ocean::profile_history_ui::build(None, body);
    for (dialog, label, key) in [
        (&storage.dialog, &storage.label, "storage.title"),
        (&plugins.dialog, &plugins.label, "plugins.title"),
        (&history.dialog, &history.label, "backup_history.title"),
    ] {
        assert_eq!(
            dialog.title().as_deref(),
            Some(i18n::message(i18n::active_language(), key).expect("embedded native label"))
        );
        assert_eq!(label.text().as_str(), body);
        assert!(!label.property::<bool>("use-markup"));
        button(dialog, gtk::ResponseType::Close, "common.close");
        dialog.close();
    }
}

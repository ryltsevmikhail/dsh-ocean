//! One language per process; synthetic controls only, no profile, services or action callbacks.
use dsh_ocean::{i18n, localization::Preference};
use gtk::prelude::*;

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

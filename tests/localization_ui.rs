//! Synthetic GTK controls only; never touches actual preferences/profiles/services.
use dsh_ocean::{i18n::Language, localization::Preference, localization_ui};
use gtk::prelude::*;

#[test]
#[ignore = "requires private GTK display"]
fn localized_language_picker_and_confirmation_keep_response_ids_and_plain_text() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    gtk::init().unwrap();
    let (dialog, combo) =
        localization_ui::build(None, Language::Ru, Preference::parse("ja").unwrap());
    assert_eq!(dialog.title().as_deref(), Some("Язык"));
    assert_eq!(combo.active_id().as_deref(), Some("ja"));
    assert!(!combo.set_active_id(Some("../../external")));
    let cancel = dialog
        .widget_for_response(gtk::ResponseType::Cancel)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    assert_eq!(cancel.label().as_deref(), Some("Отмена"));
    dialog.close();
    let body = "<b>untrusted & {count}</b>";
    let confirm = dsh_ocean::ui::build_confirmation(None, Language::Ru, "Title", body);
    assert_eq!(confirm.text().as_deref(), Some(body));
    let yes = confirm
        .widget_for_response(gtk::ResponseType::Yes)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    let no = confirm
        .widget_for_response(gtk::ResponseType::No)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    assert!(!confirm.property::<bool>("use-markup"));
    assert_eq!(yes.label().as_deref(), Some("Да"));
    assert_eq!(no.label().as_deref(), Some("Нет"));
    assert_eq!(
        confirm
            .default_widget()
            .map(|w| confirm.response_for_widget(&w)),
        Some(gtk::ResponseType::No)
    );
    confirm.close();
}

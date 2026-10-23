use dsh_ocean::{commands, i18n::Language};
#[test]
fn only_owned_pages_can_read_native_catalogs() {
    for label in ["setup", "remote"] {
        let snapshot = commands::native_locale_snapshot(label, Language::Ru).unwrap();
        assert_eq!(snapshot["language"], "ru");
        assert_eq!(snapshot["messages"]["common.cancel"], "Отмена");
    }
    for label in ["main", "unknown", ""] {
        assert!(commands::native_locale_snapshot(label, Language::Ru).is_err());
    }
}
#[test]
fn language_dialog_cannot_be_opened_by_embedded_runtime_or_pairing_page() {
    assert!(commands::language_settings_allowed("setup"));
    for label in ["main", "remote", "unknown", ""] {
        assert!(!commands::language_settings_allowed(label));
    }
}

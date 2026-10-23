use dsh_ocean::i18n::{active_language, initialize, text, Language};

#[test]
fn startup_language_is_a_single_immutable_snapshot_without_environment_mutation() {
    assert_eq!(active_language(), Language::En);
    initialize(Language::Ru).unwrap();
    assert_eq!(active_language(), Language::Ru);
    assert_eq!(text("common.cancel"), "Отмена");
    assert!(initialize(Language::Fr).is_err());
    assert_eq!(active_language(), Language::Ru);
    assert_eq!(text("unknown.stable.key"), "unknown.stable.key");
}

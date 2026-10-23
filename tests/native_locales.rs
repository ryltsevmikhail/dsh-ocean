use dsh_ocean::i18n::{message, render, resolve_language, Language};

#[test]
fn locale_resolution_is_pure_and_respects_posix_precedence_and_fallback() {
    assert_eq!(
        resolve_language(None, None, None, Some("ru_RU.UTF-8")),
        Language::Ru
    );
    assert_eq!(
        resolve_language(Some("ja"), Some("de_DE"), None, Some("ru_RU")),
        Language::Ja
    );
    assert_eq!(
        resolve_language(Some("system"), Some("C.UTF-8"), Some("ru"), Some("de")),
        Language::En
    );
    assert_eq!(
        resolve_language(None, Some(""), Some("pt_BR.UTF-8@variant"), Some("ru")),
        Language::PtBr
    );
    assert_eq!(
        resolve_language(None, None, None, Some("unknown")),
        Language::En
    );
    assert_eq!(
        resolve_language(Some("not-a-shipped-locale"), None, None, Some("ru")),
        Language::En
    );
    assert_eq!(
        resolve_language(None, None, None, Some("POSIX")),
        Language::En
    );
}

#[test]
fn native_catalogs_are_embedded_and_not_read_from_installed_addons() {
    for code in ["en", "ru", "es", "pt-BR", "de", "fr", "ja", "ko"] {
        let locale = Language::from_locale(code).unwrap();
        assert_eq!(locale.code(), code);
        assert!(message(locale, "language.title").is_some());
    }
    assert_eq!(message(Language::Ru, "common.cancel"), Some("Отмена"));
    assert_eq!(message(Language::En, "unknown.key"), None);
}

#[test]
fn named_arguments_are_substituted_once_as_plain_text() {
    assert_eq!(
        render(Language::Ru, "setup.log_lines", &[("count", "12")]).unwrap(),
        "Строк журнала: 12"
    );
    let untrusted = "</script><img src=x onerror=alert(1)> {count}";
    assert_eq!(
        render(Language::En, "setup.log_lines", &[("count", untrusted)]).unwrap(),
        format!("Log lines: {untrusted}")
    );
    // Rendering remains plain text. HTML/GTK consumers must use text APIs, never markup.
    assert!(render(Language::En, "setup.log_lines", &[]).is_err());
    assert!(render(Language::En, "common.ok", &[("extra", "value")]).is_err());
    assert!(render(
        Language::En,
        "setup.log_lines",
        &[("count", "1"), ("count", "2")]
    )
    .is_err());
    assert!(render(Language::En, "unknown.key", &[]).is_err());
}

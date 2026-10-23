use dsh_ocean::{i18n::Language, notifications::Kind};
#[test]
fn localized_notifications_keep_protocol_identifiers_fixed_and_payloads_private() {
    for language in [
        Language::En,
        Language::Ru,
        Language::Es,
        Language::PtBr,
        Language::De,
        Language::Fr,
        Language::Ja,
        Language::Ko,
    ] {
        for kind in [Kind::Task, Kind::DownloadSaved, Kind::DownloadFailed] {
            let value = kind.parameters_for(language, 42);
            assert_eq!(value.type_().as_str(), "(susssasa{sv}i)");
            assert_eq!(value.child_value(0).str(), Some("DSH Ocean"));
            assert_eq!(value.child_value(1).get::<u32>(), Some(42));
            assert_eq!(value.child_value(2).str(), Some("dsh-ocean"));
            assert_eq!(value.child_value(5).child_value(0).str(), Some("default"));
            assert_eq!(value.child_value(7).get::<i32>(), Some(5000));
            let rendered = value.print(false).to_string();
            for secret in ["token=", "cookie", "Bearer", "/home/", "provider-secret"] {
                assert!(!rendered.contains(secret));
            }
        }
    }
    let ru = Kind::Task.parameters_for(Language::Ru, 0);
    assert_eq!(ru.child_value(3).str(), Some("Активность задачи DSH"));
    assert_eq!(
        ru.child_value(4).str(),
        Some("Откройте DSH Ocean, чтобы посмотреть обновление.")
    );
    assert_eq!(
        ru.child_value(5).child_value(1).str(),
        Some("Открыть DSH Ocean")
    );
}

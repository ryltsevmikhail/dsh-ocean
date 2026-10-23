use dsh_ocean::builtin_locale_consent::{load, record, Scope};
use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "dsh-ocean-locale-consent-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.0.is_absolute()
                && self.0.parent() == Some(std::env::temp_dir().as_path())
                && self
                    .0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("dsh-ocean-locale-consent-test-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn absence_is_not_consent_and_choices_are_bound_to_exact_profile_and_sdk_contract() {
    let f = Fixture::new();
    let path = f.0.join("preferences/locale-consent.json");
    let scope = Scope::new(&f.0.join("a/profiles/web"), "0.2.0-rc.2").unwrap();
    let other = Scope::new(&f.0.join("b/profiles/web"), "0.2.0-rc.2").unwrap();
    assert_eq!(load(&path, &scope).unwrap(), None);
    assert!(!path.parent().unwrap().exists());
    record(&path, &scope, true).unwrap();
    assert_eq!(load(&path, &scope).unwrap(), Some(true));
    assert_eq!(load(&path, &other).unwrap(), None);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    record(&path, &scope, false).unwrap();
    assert_eq!(load(&path, &scope).unwrap(), Some(false));
    assert!(
        !f.0.join("a").exists(),
        "consent storage must not create or modify the selected profile"
    );
    assert!(Scope::new(&f.0.join("a/profiles/web"), "0.3.0").is_err());
    assert!(Scope::new(std::path::Path::new("relative/profiles/web"), "0.2.0-rc.2").is_err());
    assert!(Scope::new(&f.0.join("a/../profiles/web"), "0.2.0-rc.2").is_err());
}
#[test]
fn malformed_future_linked_or_unknown_documents_are_preserved_not_authorized() {
    let f = Fixture::new();
    let path = f.0.join("consent.json");
    let scope = Scope::new(&f.0.join("profiles/web"), "0.2.0-rc.2").unwrap();
    for bytes in [
        "invalid",
        "{\"version\":2}",
        "{\"version\":1,\"allowed\":true}",
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(load(&path, &scope).is_err());
        assert!(record(&path, &scope, true).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
    }
    let link = f.0.join("link.json");
    symlink(&path, &link).unwrap();
    assert!(record(&link, &scope, true).is_err());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{\"version\":1,\"allowed\":true}"
    );
}

#[test]
fn complete_but_future_or_extended_consent_cannot_be_reused_or_replaced() {
    let f = Fixture::new();
    let path = f.0.join("consent.json");
    let scope = Scope::new(&f.0.join("profiles/web"), "0.2.0-rc.2").unwrap();
    record(&path, &scope, true).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for extended in [false, true] {
        let mut value = original.clone();
        if extended {
            value["futurePermission"] = serde_json::json!(true);
        } else {
            value["version"] = serde_json::json!(2);
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(load(&path, &scope).is_err());
        assert!(record(&path, &scope, false).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

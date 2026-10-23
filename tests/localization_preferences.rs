use dsh_ocean::localization::{load, save, Preference};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
struct Fixture(PathBuf);
impl Fixture {
    fn path(&self) -> &Path {
        &self.0
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
                    .starts_with("dsh-ocean-native-prefs-test-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn tempdir() -> std::io::Result<Fixture> {
    let p = std::env::temp_dir().join(format!(
        "dsh-ocean-native-prefs-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::DirBuilder::new().mode(0o700).create(&p)?;
    Ok(Fixture(p))
}

#[test]
fn absent_preference_is_system_and_does_not_create_anything() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("new-config");
    assert_eq!(
        load(&parent.join("localization.json")).unwrap(),
        Preference::System
    );
    assert!(!parent.exists());
}

#[test]
fn missing_parent_is_created_private_only_when_saving() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("new/config");
    let path = parent.join("localization.json");
    save(&path, Preference::parse("ja").unwrap()).unwrap();
    assert_eq!(
        fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(load(&path).unwrap().code(), "ja");
}

#[test]
fn only_canonical_shipped_codes_are_accepted() {
    for code in ["system", "en", "ru", "es", "pt-BR", "de", "fr", "ja", "ko"] {
        assert_eq!(Preference::parse(code).unwrap().code(), code);
    }
    for code in [
        "",
        "ru_RU",
        "pt-br",
        "../../ru",
        "https://example.test/ru",
        "zh",
        "EN",
    ] {
        assert!(Preference::parse(code).is_none(), "{code}");
    }
}

#[test]
fn saves_separate_versioned_private_preference_and_preserves_other_settings() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("config");
    fs::create_dir(&parent).unwrap();
    let config = parent.join("config.json");
    fs::write(&config, b"unrelated server settings").unwrap();
    let path = parent.join("localization.json");
    let ru = Preference::parse("ru").unwrap();
    save(&path, ru).unwrap();
    assert_eq!(load(&path).unwrap(), ru);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(json, serde_json::json!({"version":1,"language":"ru"}));
    save(&path, Preference::System).unwrap();
    assert_eq!(load(&path).unwrap(), Preference::System);
    assert_eq!(fs::read(&config).unwrap(), b"unrelated server settings");
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 2);
}

#[test]
fn malformed_future_and_unknown_preferences_are_never_replaced() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("localization.json");
    for bytes in [
        b"invalid".as_slice(),
        b"{\"version\":2,\"language\":\"ru\"}",
        b"{\"version\":1,\"language\":\"zh\"}",
        b"{\"version\":1,\"language\":\"ru\",\"extra\":true}",
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(load(&path).is_err());
        assert!(save(&path, Preference::System).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::write(&path, vec![b' '; 8193]).unwrap();
    assert!(load(&path).is_err());
    assert!(save(&path, Preference::System).is_err());
    assert_eq!(fs::read(&path).unwrap().len(), 8193);
}

#[test]
fn linked_files_and_parent_redirects_are_refused_without_touching_the_target() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("target.json");
    let bytes = b"{\"version\":1,\"language\":\"en\"}";
    fs::write(&target, bytes).unwrap();
    let link = dir.path().join("localization.json");
    symlink(&target, &link).unwrap();
    assert!(load(&link).is_err());
    assert!(save(&link, Preference::System).is_err());
    assert_eq!(fs::read(&target).unwrap(), bytes);
    let hard = dir.path().join("hard.json");
    fs::hard_link(&target, &hard).unwrap();
    assert!(load(&hard).is_err());
    assert!(save(&hard, Preference::System).is_err());
    let real = dir.path().join("real");
    fs::create_dir(&real).unwrap();
    let redirect = dir.path().join("redirect");
    symlink(&real, &redirect).unwrap();
    assert!(save(&redirect.join("localization.json"), Preference::System).is_err());
    assert_eq!(fs::read_dir(&real).unwrap().count(), 0);
}

#[test]
fn unsafe_parent_is_refused_without_changing_existing_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("localization.json");
    let bytes = b"{\"version\":1,\"language\":\"en\"}";
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(load(&path).is_err());
    assert!(save(&path, Preference::System).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
}

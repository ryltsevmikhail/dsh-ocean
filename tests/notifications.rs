use dsh_ocean::notifications::{self, Kind, Limiter};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "dsh-ocean-notification-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-notification-test-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn notifications_are_enabled_by_default_and_user_disable_is_persistent() {
    let f = Fixture::new();
    let path = f.0.join("private/notifications.json");
    assert!(notifications::load_preference(&path).unwrap());
    notifications::save_preference(&path, false).unwrap();
    assert!(!notifications::load_preference(&path).unwrap());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    notifications::save_preference(&path, true).unwrap();
    assert!(notifications::load_preference(&path).unwrap());
}
#[test]
fn invalid_linked_future_or_oversized_preferences_are_not_replaced() {
    let f = Fixture::new();
    for (name, data) in [
        ("bad", "broken"),
        ("future", "{\"version\":2,\"enabled\":true}"),
        ("unknown", "{\"version\":1,\"enabled\":true,\"other\":1}"),
    ] {
        let path = f.0.join(name);
        fs::write(&path, data).unwrap();
        assert!(notifications::load_preference(&path).is_err());
        assert!(notifications::save_preference(&path, false).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), data);
    }
    let valid = f.0.join("valid");
    notifications::save_preference(&valid, true).unwrap();
    symlink(&valid, f.0.join("symlink")).unwrap();
    assert!(notifications::save_preference(&f.0.join("symlink"), false).is_err());
    fs::hard_link(&valid, f.0.join("hardlink")).unwrap();
    assert!(notifications::load_preference(&valid).is_err());
    let huge = f.0.join("huge");
    fs::File::create(&huge).unwrap().set_len(8193).unwrap();
    assert!(notifications::load_preference(&huge).is_err());
}
#[test]
fn payloads_are_fixed_generic_freedesktop_packets() {
    for kind in [Kind::Task, Kind::DownloadSaved, Kind::DownloadFailed] {
        let params = kind.parameters(42);
        assert_eq!(params.type_().as_str(), "(susssasa{sv}i)");
        let (app, id, icon, summary, body, actions, hints, timeout) = params
            .get::<(
                String,
                u32,
                String,
                String,
                String,
                Vec<String>,
                HashMap<String, gtk::glib::Variant>,
                i32,
            )>()
            .unwrap();
        assert_eq!(
            (app.as_str(), id, icon.as_str(), timeout),
            ("DSH Ocean", 42, "dsh-ocean", 5000)
        );
        assert_eq!(actions, ["default", "Open DSH Ocean"]);
        assert!(!summary.contains(['<', '>']) && !body.contains(['<', '>']));
        assert!(!body.contains("http") && !body.contains('/') && !body.contains("token"));
        assert_eq!(
            hints["desktop-entry"].get::<String>().as_deref(),
            Some("dsh-ocean")
        );
        assert_eq!(hints["transient"].get::<bool>(), Some(true));
        assert_eq!(hints["suppress-sound"].get::<bool>(), Some(true));
        assert_eq!(hints["urgency"].get::<u8>(), Some(0));
    }
}
#[test]
fn bursts_and_same_kind_repeats_are_rate_limited_without_sleeping() {
    let mut limiter = Limiter::default();
    let start = Instant::now();
    assert!(limiter.admit(Kind::Task, start));
    assert!(!limiter.admit(Kind::Task, start + Duration::from_secs(9)));
    assert!(limiter.admit(Kind::DownloadSaved, start));
    assert!(limiter.admit(Kind::DownloadFailed, start));
    assert!(!limiter.admit(Kind::Task, start + Duration::from_secs(10)));
    assert!(limiter.admit(Kind::Task, start + Duration::from_secs(30)));
}

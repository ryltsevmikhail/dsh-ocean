use dsh_ocean::shell_status::{self, Hint};
use gtk::{glib, prelude::*};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn installation_hints_never_claim_package_ownership_or_apply_authority() {
    for (path, appimage, flatpak, expected) in [
        (None, false, false, Hint::Unknown),
        (
            Some("/tmp/.mount-ocean/usr/bin/dsh-ocean"),
            true,
            false,
            Hint::AppImageEnvironment,
        ),
        (
            Some("/app/bin/dsh-ocean"),
            false,
            true,
            Hint::FlatpakEnvironment,
        ),
        (
            Some("/private/work/target/release/dsh-ocean"),
            false,
            false,
            Hint::DevelopmentPath,
        ),
        (Some("/usr/bin/dsh-ocean"), false, false, Hint::SystemPrefix),
        (
            Some("/usr/local/bin/dsh-ocean"),
            true,
            false,
            Hint::Conflicting,
        ),
        (
            Some("/private/work/target/debug/dsh-ocean"),
            true,
            false,
            Hint::Conflicting,
        ),
        (None, true, true, Hint::Conflicting),
        (Some("/usr/binary/dsh-ocean"), false, false, Hint::Unknown),
        (
            Some("/private/work/target/release/not-ocean"),
            false,
            false,
            Hint::Unknown,
        ),
    ] {
        let report = shell_status::classify(path.map(Path::new), appimage, flatpak);
        assert_eq!(report.installation_hint, expected);
        assert!(
            !report.ownership_verified
                && !report.automatic_update_available
                && !report.production_signed_feed_configured
        );
        assert!(report.limits.contains("separate") && report.limits.contains("read-only"));
    }
}
#[test]
fn report_exposes_no_executable_path_or_environment_payload() {
    let report = shell_status::classify(
        Some(Path::new(
            "/synthetic-private/project/target/debug/dsh-ocean",
        )),
        false,
        false,
    );
    let json = serde_json::to_string(&report).unwrap();
    let text = shell_status::text(&report);
    assert!(!json.contains("synthetic-private") && !text.contains("synthetic-private"));
    assert!(text.contains(env!("CARGO_PKG_VERSION")));
    assert!(text.contains("Automatic shell update: unavailable"));
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-shell-status-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.0.is_absolute()
                && self
                    .0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("dsh-ocean-shell-status-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn cli_is_headless_and_does_not_probe_package_managers_profiles_or_services() {
    let f = Fixture::new();
    let bin = f.0.join("bin");
    fs::create_dir(&bin).unwrap();
    for tool in ["pacman", "dpkg-query", "rpm", "flatpak", "systemctl", "dsh"] {
        let path = bin.join(tool);
        fs::write(&path, "#!/bin/sh\nprintf called > probe-called\nexit 1\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .arg("--shell-status")
            .args(extra)
            .current_dir(&f.0)
            .env_clear()
            .env("PATH", &bin)
            .env("HOME", &f.0)
            .env("DSH_HOME", f.0.join("must-not-create"))
            .env("XDG_CONFIG_HOME", f.0.join("config"))
            .env("APPIMAGE", "/synthetic-private-image?token=secret")
            .env("FLATPAK_ID", "synthetic-private-flatpak")
            .output()
            .unwrap()
    };
    let result = run(&[]);
    assert!(result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["installationHint"], "conflicting");
    assert_eq!(value["ownershipVerified"], false);
    assert_eq!(value["automaticUpdateAvailable"], false);
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(!output.contains("synthetic-private") && !output.contains("token=secret"));
    assert!(!f.0.join("probe-called").exists());
    assert!(!f.0.join("must-not-create").exists());
    assert!(!f.0.join("config").exists());
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
    let invalid = run(&["synthetic-private-argument"]);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(invalid.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains("synthetic-private"));
}
fn tick() {
    for _ in 0..32 {
        if !glib::MainContext::default().pending() {
            break;
        }
        glib::MainContext::default().iteration(false);
    }
}
#[test]
#[ignore = "requires explicitly enabled isolated GTK dialog display"]
fn native_shell_status_has_only_readonly_guidance_and_close() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    gtk::init().unwrap();
    let parent = gtk::Window::new(gtk::WindowType::Toplevel);
    parent.show_all();
    let report = shell_status::classify(None, true, false);
    let window = dsh_ocean::shell_status_ui::build(Some(&parent), &report);
    window.dialog.show_all();
    tick();
    assert!(window.dialog.is_modal());
    assert_eq!(window.dialog.transient_for(), Some(parent.clone()));
    assert!(window.label.is_selectable() && !window.label.uses_markup());
    assert!(window.label.text().contains("AppImage"));
    assert!(window.label.text().contains("not a network update check"));
    assert!(window
        .label
        .text()
        .contains("Automatic shell update: unavailable"));
    assert!(window
        .dialog
        .widget_for_response(gtk::ResponseType::Accept)
        .is_none());
    assert!(window
        .dialog
        .widget_for_response(gtk::ResponseType::Apply)
        .is_none());
    window
        .dialog
        .widget_for_response(gtk::ResponseType::Close)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap()
        .emit_clicked();
    let end = Instant::now() + Duration::from_secs(5);
    while window.dialog.is_visible() {
        tick();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
    parent.close();
}

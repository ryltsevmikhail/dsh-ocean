use dsh_ocean::shell_status::{self, Hint};
use gtk::{glib, prelude::*};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn installation_introduction_uses_selected_locale_without_changing_machine_ownership_report() {
    for (path, appimage, flatpak, key) in [
        (None, true, false, "appimage"),
        (None, false, true, "flatpak"),
        (
            Some("/private-secret/target/debug/dsh-ocean"),
            false,
            false,
            "development",
        ),
        (Some("/usr/bin/dsh-ocean"), false, false, "system"),
        (None, true, true, "conflicting"),
        (None, false, false, "unknown"),
    ] {
        let report = shell_status::classify(path.map(Path::new), appimage, flatpak);
        let before = serde_json::to_string(&report).unwrap();
        for (language, _) in dsh_ocean::i18n::LANGUAGES {
            let text = shell_status::introduction_text_in(&report, language);
            assert!(text.contains(env!("CARGO_PKG_VERSION")) && !text.contains("private-secret"));
            assert!(text.contains(
                dsh_ocean::i18n::message(language, &format!("installation.{key}.label")).unwrap()
            ));
            assert!(text.contains(
                dsh_ocean::i18n::message(language, &format!("installation.{key}.guidance"))
                    .unwrap()
            ));
        }
        assert_eq!(serde_json::to_string(&report).unwrap(), before);
        assert!(!report.ownership_verified && !report.automatic_update_available);
    }
}
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
fn native_shell_updater_has_channels_explicit_enrollment_and_no_unconfigured_apply() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    let private = PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap());
    assert!(private.starts_with("/tmp"));
    // This is the only ignored case in this test binary, before GTK/workers:
    // never let inherited user overrides point inspection at real Harness data.
    std::env::set_var("DSH_HOME", private.join("isolated-harness"));
    std::env::set_var("DSH_OCEAN_DATA_DIR", private.join("isolated-ocean"));
    std::env::set_var("DSH_OCEAN_URL", "http://127.0.0.1:9");
    let tools = private.join("native-update-tools");
    fs::create_dir(&tools).unwrap();
    fs::write(tools.join("curl"),"#!/usr/bin/python3\nimport sys\nfrom pathlib import Path\np=Path(__file__).parent\nu=sys.argv[sys.argv.index('--url')+1]\nwith (p/'calls').open('a') as f:f.write(u+'\\n')\nsys.stdout.buffer.write((p/('signature.json' if u.endswith('.sig.json') else 'artifact' if u.endswith('.AppImage') else 'manifest.json')).read_bytes())\n").unwrap();
    fs::set_permissions(tools.join("curl"), fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", tools.display(), std::env::var("PATH").unwrap()),
    );
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
    fn named(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
        if root.widget_name() == name {
            return Some(root.clone());
        }
        if let Ok(container) = root.clone().downcast::<gtk::Container>() {
            for child in container.children() {
                if let Some(found) = named(&child, name) {
                    return Some(found);
                }
            }
        }
        None
    }
    let root: gtk::Widget = window.dialog.clone().upcast();
    let check = named(&root, "shell-update-check")
        .expect("signed updater check button")
        .downcast::<gtk::Button>()
        .unwrap();
    let apply = named(&root, "shell-update-apply")
        .expect("managed apply button")
        .downcast::<gtk::Button>()
        .unwrap();
    let enroll = named(&root, "shell-update-enroll")
        .expect("explicit enrollment button")
        .downcast::<gtk::Button>()
        .unwrap();
    let status = named(&root, "shell-update-status")
        .unwrap()
        .downcast::<gtk::Label>()
        .unwrap();
    let channel = named(&root, "shell-update-channel")
        .unwrap()
        .downcast::<gtk::ComboBoxText>()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while !status.text().contains("Destination: not enrolled") {
        tick();
        assert!(Instant::now() < end, "{}", status.text());
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!check.is_sensitive() && !apply.is_sensitive());
    assert!(enroll.is_sensitive());
    assert_eq!(channel.active_id().as_deref(), Some("stable"));
    channel.set_active_id(Some("next"));
    assert!(!enroll.is_sensitive(),"worker start must synchronously disable other actions, not silently drop enabled clicks before the next timer tick");
    let end = Instant::now() + Duration::from_secs(5);
    while dsh_ocean::update_settings::load().unwrap().shell_channel
        != dsh_ocean::release_feed::Channel::Next
    {
        tick();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        !dsh_ocean::managed_shell::prefix().exists(),
        "settings changes must not silently enroll/install"
    );
    assert!(!private.join("isolated-harness").exists());
    fn until(stage: &str, mut ready: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(8);
        while !ready() {
            tick();
            assert!(
                Instant::now() < end,
                "native action did not finish: {stage}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn yes(parent: &gtk::Dialog) {
        until("confirmation or apply completion", || {
            for window in gtk::Window::list_toplevels() {
                if let Ok(dialog) = window.downcast::<gtk::MessageDialog>() {
                    if dialog.is_visible()
                        && dialog.transient_for().as_ref() == Some(parent.upcast_ref())
                    {
                        for (response, key) in [
                            (gtk::ResponseType::No, "common.no"),
                            (gtk::ResponseType::Yes, "common.yes"),
                        ] {
                            let button = dialog
                                .widget_for_response(response)
                                .unwrap()
                                .downcast::<gtk::Button>()
                                .unwrap();
                            assert_eq!(button.label().as_deref(), Some(dsh_ocean::i18n::text(key)));
                        }
                        dialog.response(gtk::ResponseType::Yes);
                        return true;
                    }
                }
            }
            false
        });
    }
    until("enroll enabled", || enroll.is_sensitive());
    enroll.emit_clicked();
    yes(&window.dialog);
    until("enrollment completion", || {
        status.text().contains("Destination: enrolled")
    });
    assert!(dsh_ocean::managed_shell::status().unwrap().enrolled);
    assert!(!check.is_sensitive() && !apply.is_sensitive());
    assert!(!tools.join("calls").exists());
    use dsh_ocean::{release_feed, update_settings};
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    let key = SigningKey::from_bytes(&[73; 32]); // Explicit fixture-only trust, not publisher keys.
    let key_document = release_feed::PublicKeyDocument {
        version: 1,
        key_id: "native-fixture".into(),
        public_key_hex: release_feed::hex(key.verifying_key().as_bytes()),
    };
    let feed=update_settings::Feed{manifest_url:"https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/native-manifest.json".into(),signature_url:"https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/native-manifest.sig.json".into(),minimum_sequence:1};
    let mut settings = update_settings::load().unwrap();
    settings.shell = Some(update_settings::Source {
        key: key_document,
        stable: feed.clone(),
        next: feed,
    });
    let fingerprint = release_feed::hex(&Sha256::digest(key.verifying_key().as_bytes()));
    update_settings::configure(
        settings,
        &update_settings::Confirmation {
            shell: Some(fingerprint),
            runtime: None,
            replace: false,
        },
    )
    .unwrap();
    let write_feed = |sequence: u64, version: &str, payload: &[u8]| {
        let now = chrono::Utc::now();
        let arch = std::env::consts::ARCH;
        let manifest = serde_json::json!({"version":1,"product":"dsh-ocean","channel":"next","sequence":sequence,"release":version,"publishedAt":(now-chrono::Duration::minutes(1)).to_rfc3339(),"expiresAt":(now+chrono::Duration::days(1)).to_rfc3339(),"artifacts":[{"target":format!("linux-{arch}"),"kind":"appimage","url":format!("https://github.com/ryltsevmikhail/dsh-ocean/releases/download/v{version}/dsh-ocean-{version}-{arch}.AppImage"),"size":payload.len(),"sha256":release_feed::hex(&Sha256::digest(payload))}]});
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let mut message = b"DSH Ocean release manifest v1\n".to_vec();
        message.extend_from_slice(&bytes);
        fs::write(tools.join("manifest.json"), bytes).unwrap();
        fs::write(tools.join("artifact"), payload).unwrap();
        fs::write(tools.join("signature.json"),serde_json::to_vec(&serde_json::json!({"version":1,"keyId":"native-fixture","signatureHex":release_feed::hex(&key.sign(&message).to_bytes())})).unwrap()).unwrap();
    };
    write_feed(1, "0.2.0", b"fixture-one");
    let button = |name: &str| {
        named(&root, name)
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap()
    };
    let refresh = button("shell-update-refresh");
    let download = button("shell-update-download");
    let rollback = button("shell-update-rollback");
    until("refresh enabled", || refresh.is_sensitive());
    refresh.emit_clicked();
    until("check enabled", || check.is_sensitive());
    let kind = named(&root, "shell-update-kind")
        .unwrap()
        .downcast::<gtk::ComboBoxText>()
        .unwrap();
    kind.set_active_id(Some("appimage"));
    for (sequence, version, payload) in [
        (1, "0.2.0", b"fixture-one".as_slice()),
        (2, "0.2.1", b"fixture-two".as_slice()),
    ] {
        write_feed(sequence, version, payload);
        until("check enabled", || check.is_sensitive());
        check.emit_clicked();
        until("verified metadata / download enabled", || {
            download.is_sensitive()
        });
        if sequence == 1 {
            assert_eq!(
                fs::read_to_string(tools.join("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                2,
                "check only fetches metadata"
            );
        }
        download.emit_clicked();
        until("verified download / apply enabled", || apply.is_sensitive());
        apply.emit_clicked();
        yes(&window.dialog);
        until("confirmation or apply completion", || {
            status
                .text()
                .contains(&format!("Selected on disk: {version}"))
        });
        assert_eq!(
            fs::read(dsh_ocean::managed_shell::prefix().join("current/dsh-ocean")).unwrap(),
            payload
        );
    }
    assert!(dsh_ocean::managed_shell::launcher().is_file());
    until("rollback enabled", || rollback.is_sensitive());
    rollback.emit_clicked();
    yes(&window.dialog);
    until("rollback completion", || {
        status.text().contains("Selected on disk: 0.2.0")
    });
    assert_eq!(
        dsh_ocean::release_state::read().unwrap()["next"].sequence,
        2
    );
    assert!(!private.join("isolated-harness").exists());
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

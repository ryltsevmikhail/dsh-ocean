use dsh_ocean::{bootstrap::Runtime, systemd};
use std::path::{Path, PathBuf};
#[test]
fn published_server_retains_actual_launch_recipe_independently_of_future_preferences() {
    let server = dsh_ocean::service::Server {
        base_url: url::Url::parse("http://127.0.0.1:12345").unwrap(),
        token: "private-fixture".into(),
        mode: dsh_ocean::service::ServerMode::Child,
        unit: None,
        locale_patch: Some(PathBuf::from("/private/old-resource/overlay.yml")),
    };
    assert_eq!(server.clone().locale_patch, server.locale_patch);
    let refreshed = server.with_token("rotated-token".into());
    assert_eq!(refreshed.locale_patch, server.locale_patch);
    assert_eq!(refreshed.token, "rotated-token");
    assert_eq!(refreshed.base_url, server.base_url);
}
#[test]
fn optional_locale_patch_changes_only_execstart_and_is_escaped_as_one_argument() {
    let runtime = Runtime {
        node: PathBuf::from("/private/runtime/node"),
        dsh: PathBuf::from("/private/runtime/dsh.js"),
        version: "0.2.0-rc.2".into(),
        install_root: None,
    };
    let base = url::Url::parse("http://127.0.0.1:3080").unwrap();
    let plain = systemd::render_unit(&runtime, &base);
    let patch = Path::new("/private/Ocean $HOME 100% \"quoted\"/overlay.yml");
    let args = dsh_ocean::service::managed_arguments(Some(patch)).unwrap();
    assert_eq!(&args[..2], &["--profile", "web"]);
    assert_eq!(
        &args[2..4],
        &["--patch".to_owned(), patch.to_str().unwrap().to_owned()]
    );
    assert_eq!(
        &args[4..],
        &dsh_ocean::service::managed_arguments(None).unwrap()[2..]
    );
    assert!(dsh_ocean::service::managed_arguments(Some(Path::new("relative.yml"))).is_err());
    let translated = systemd::render_unit_with_patch(&runtime, &base, patch).unwrap();
    let addition = " --patch \"/private/Ocean $$HOME 100%% \\\"quoted\\\"/overlay.yml\"";
    assert!(translated.contains(addition));
    assert!(
        translated.contains(&format!("--profile web{addition} --host")),
        "global overlay option must precede forwarded web-app arguments"
    );
    assert_eq!(translated.replacen(addition, "", 1), plain);
    assert!(systemd::matches_launch_unit(
        &runtime,
        &base,
        Some(patch),
        translated.as_bytes()
    ));
    assert!(!systemd::matches_launch_unit(
        &runtime,
        &base,
        Some(patch),
        plain.as_bytes()
    ));
    assert!(!systemd::matches_launch_unit(
        &runtime,
        &base,
        None,
        translated.as_bytes()
    ));
    assert!(!systemd::matches_launch_unit(
        &runtime,
        &base,
        Some(Path::new("/other/overlay.yml")),
        translated.as_bytes()
    ));
    let legacy = translated.replacen("Type=simple\nUMask=0077\n", "Type=simple\n", 1);
    assert!(systemd::matches_launch_unit(
        &runtime,
        &base,
        Some(patch),
        legacy.as_bytes()
    ));
    assert!(
        systemd::render_unit_with_patch(&runtime, &base, Path::new("relative/overlay.yml"))
            .is_err()
    );
    assert!(systemd::render_unit_with_patch(
        &runtime,
        &base,
        Path::new("/private/../foreign/overlay.yml")
    )
    .is_err());
}

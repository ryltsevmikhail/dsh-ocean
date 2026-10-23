use dsh_ocean::{bootstrap::Runtime, systemd};
use std::path::{Path, PathBuf};
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

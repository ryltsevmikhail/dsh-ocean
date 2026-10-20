//! Malformed CLI input fails before GUI initialization and is never echoed.
#![cfg(target_os = "linux")]
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::process::Command;

#[test]
fn non_utf8_arguments_are_usage_errors_not_panics() {
    for arguments in [
        vec![OsString::from_vec(b"--doctor\xff".to_vec())],
        vec![
            OsString::from("--doctor"),
            OsString::from_vec(b"private-secret\xff".to_vec()),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .args(arguments)
            .env_clear()
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("arguments must be valid UTF-8"), "{error}");
        assert!(
            !error.contains("panicked") && !error.contains("private-secret"),
            "{error}"
        );
    }
}

#[test]
fn unknown_options_do_not_echo_credentials_or_terminal_controls() {
    let output = Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
        .arg("--unknown=private-secret\x1b[31m?token=private-token")
        .env_clear()
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "dsh-ocean: unknown option; use --help\n"
    );
}

#[test]
fn source_desktop_entry_does_not_advertise_file_or_url_arguments() {
    let desktop = include_str!("../packaging/dsh-ocean.desktop");
    let exec: Vec<_> = desktop
        .lines()
        .filter_map(|line| line.strip_prefix("Exec="))
        .collect();
    assert_eq!(exec, ["dsh-ocean"]);
}

#[test]
fn release_metadata_versions_match_the_binary() {
    let version = env!("CARGO_PKG_VERSION");
    let tauri: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    assert_eq!(tauri["version"], version);
    let pkgbuild = include_str!("../packaging/PKGBUILD");
    assert_eq!(
        pkgbuild
            .lines()
            .find_map(|line| line.strip_prefix("pkgver=")),
        Some(version)
    );
    assert!(include_str!("../CHANGELOG.md")
        .lines()
        .any(|line| line == format!("## {version}")));
}

#[test]
fn version_remains_headless_and_independent_of_home() {
    let output = Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
        .arg("--version")
        .env_clear()
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains(env!("CARGO_PKG_VERSION")));
    assert!(output.stderr.is_empty());
}

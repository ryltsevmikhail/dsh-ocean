#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use dsh_ocean::bootstrap::{path_env, selected_private_root};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-select-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("generations")).unwrap();
        Self(root)
    }
    fn generation(&self, name: &str) -> PathBuf {
        let root = self.0.join("generations").join(name);
        let bin = root.join("runtime/node/bin");
        let harness = root.join("harness/node_modules/.bin");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&harness).unwrap();
        fs::write(bin.join("node"), "#!/bin/bash\nif [ \"$1\" = --version ]; then echo v26.12.0; else exec /bin/bash \"$@\"; fi\n").unwrap();
        fs::write(harness.join("dsh"), "#!/bin/bash\necho 0.2.0-rc.2\n").unwrap();
        fs::write(
            root.join(".versions"),
            "26.12.0\n0.2.0-rc.2\n10.34.7\nfixture\n",
        )
        .unwrap();
        for executable in [bin.join("node"), harness.join("dsh")] {
            fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        root
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-select-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn legacy_fallback_only_applies_when_no_selector_exists() {
    let f = Fixture::new();
    assert_eq!(selected_private_root(&f.0).unwrap(), f.0);
    symlink("generations/gen.missing", f.0.join("current")).unwrap();
    assert!(selected_private_root(&f.0).is_err());
    fs::remove_file(f.0.join("current")).unwrap();
    fs::create_dir(f.0.join("current")).unwrap();
    assert!(selected_private_root(&f.0).is_err());
}

#[test]
fn selector_cannot_escape_its_prefix() {
    let f = Fixture::new();
    let outside = Fixture::new();
    symlink(&outside.0, f.0.join("current")).unwrap();
    assert!(selected_private_root(&f.0).is_err());
    fs::remove_file(f.0.join("current")).unwrap();
    symlink(&outside.0, f.0.join("generations/gen.outside")).unwrap();
    symlink("generations/gen.outside", f.0.join("current")).unwrap();
    assert!(selected_private_root(&f.0).is_err());
}

#[test]
fn detection_and_service_unit_use_immutable_generation_paths() {
    let f = Fixture::new();
    let root = f.generation("gen.first");
    symlink("generations/gen.first", f.0.join("current")).unwrap();
    assert_eq!(selected_private_root(&f.0).unwrap(), root);
    let run = |arg| {
        Command::new(env!("CARGO_BIN_EXE_dsh-ocean"))
            .arg(arg)
            .env("DSH_OCEAN_DATA_DIR", &f.0)
            .env("HOME", f.0.join("home"))
            .env("PATH", "/usr/bin:/bin")
            .env_remove("DSH_OCEAN_NODE")
            .env_remove("DSH_OCEAN_DSH_BIN")
            .output()
            .unwrap()
    };
    let info = run("--runtime");
    assert!(
        info.status.success(),
        "{}",
        String::from_utf8_lossy(&info.stderr)
    );
    let info: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(
        info["node"],
        root.join("runtime/node/bin/node").to_str().unwrap()
    );
    assert_eq!(
        info["dsh"],
        root.join("harness/node_modules/.bin/dsh").to_str().unwrap()
    );
    let unit = run("--print-unit");
    assert!(unit.status.success());
    let unit = String::from_utf8(unit.stdout).unwrap();
    assert!(unit.contains(root.to_str().unwrap()));
    assert!(!unit.contains("/current/"));
}

#[test]
fn generation_runtime_is_protected_from_in_place_updates() {
    let f = Fixture::new();
    let root = f.generation("gen.first");
    let mut runtime = dsh_ocean::bootstrap::Runtime {
        node: root.join("runtime/node/bin/node"),
        dsh: root.join("harness/node_modules/.bin/dsh"),
        version: "0.2.0-rc.2".into(),
        install_root: Some(root.join("harness")),
    };
    assert!(runtime.is_generation_managed());
    runtime.dsh = f.0.join("external/node_modules/.bin/dsh");
    assert!(!runtime.is_generation_managed());
}

#[test]
fn tool_path_remains_bound_to_the_running_generation() {
    let f = Fixture::new();
    let first = f.generation("gen.first");
    let second = f.generation("gen.second");
    symlink("generations/gen.second", f.0.join("current")).unwrap();
    let path = path_env(Some(&first.join("runtime/node/bin/node")));
    let parts: Vec<_> = std::env::split_paths(&path).collect();
    assert!(parts.contains(&first.join("tools/node_modules/.bin")));
    assert!(parts.contains(&first.join("harness/node_modules/.bin")));
    assert!(!parts.contains(&second.join("tools/node_modules/.bin")));
}

//! Resolve environment paths in separate processes without changing the test
//! runner's environment or touching any real user configuration/state.
#![cfg(target_os = "linux")]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dsh_ocean::config;

#[test]
fn path_probe() {
    let Some(output) = std::env::var_os("DSH_OCEAN_TEST_PATH_OUTPUT") else {
        return;
    };
    let value = serde_json::json!({
        "home": config::home_dir(),
        "config": config::config_dir(),
        "state": config::state_dir(),
        "data": config::data_dir(),
        "dsh": config::dsh_home(),
        "runtime": config::runtime_dir(),
    });
    fs::write(output, value.to_string()).unwrap();
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-path-environment-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("cwd")).unwrap();
        Self(root)
    }

    fn probe(&self, home: Option<&Path>, override_value: Option<&Path>) -> serde_json::Value {
        let output = self.0.join("result.json");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "path_probe", "--nocapture"])
            .env_clear()
            .env("DSH_OCEAN_TEST_PATH_OUTPUT", &output)
            .current_dir(self.0.join("cwd"));
        if let Some(home) = home {
            command.env("HOME", home);
        }
        for variable in [
            "XDG_CONFIG_HOME",
            "XDG_STATE_HOME",
            "XDG_DATA_HOME",
            "XDG_RUNTIME_DIR",
            "DSH_OCEAN_DATA_DIR",
            "DSH_HOME",
        ] {
            if let Some(value) = override_value {
                command.env(variable, value);
            }
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&fs::read(output).unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-path-environment-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn unset_empty_and_relative_overrides_use_absolute_home_defaults() {
    let fixture = Fixture::new();
    let home = fixture.0.join("private-home");
    let expected = serde_json::json!({
        "home": home,
        "config": home.join(".config/dsh-ocean"),
        "state": home.join(".local/state/dsh-ocean"),
        "runtime": home.join(".local/state/dsh-ocean"),
        "data": home.join(".local/share/dsh-ocean"),
        "dsh": home.join(".dsh"),
    });
    for value in [None, Some(Path::new("")), Some(Path::new("relative"))] {
        assert_eq!(fixture.probe(Some(&home), value), expected);
    }
    assert_eq!(fs::read_dir(fixture.0.join("cwd")).unwrap().count(), 0);
    assert!(
        !home.exists(),
        "path resolution must not create directories"
    );
}

#[test]
fn absolute_overrides_remain_supported() {
    let fixture = Fixture::new();
    let home = fixture.0.join("home");
    let override_path = fixture.0.join("override");
    let value = fixture.probe(Some(&home), Some(&override_path));
    assert_eq!(value["home"], home.to_str().unwrap());
    assert_eq!(
        value["config"],
        override_path.join("dsh-ocean").to_str().unwrap()
    );
    assert_eq!(
        value["state"],
        override_path.join("dsh-ocean").to_str().unwrap()
    );
    for key in ["data", "runtime", "dsh"] {
        assert_eq!(value[key], override_path.to_str().unwrap());
    }
}

#[test]
fn absent_empty_and_relative_home_use_the_same_account_home() {
    let fixture = Fixture::new();
    // The probe only computes paths; it never reads or creates account files.
    let baseline = fixture.probe(None, None);
    assert!(Path::new(baseline["home"].as_str().unwrap()).is_absolute());
    for value in [Path::new(""), Path::new("relative-home")] {
        assert_eq!(fixture.probe(Some(value), None), baseline);
    }
    assert_eq!(fs::read_dir(fixture.0.join("cwd")).unwrap().count(), 0);
}

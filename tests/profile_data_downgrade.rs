//! Data-downgrade gate for profile restore, driven through the real CLI.
//!
//! A fake `node`/`dsh` pair reports whatever DSH version the test wants; no Node,
//! runtime, service or real profile is involved.
#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    dsh_home: PathBuf,
    profile: PathBuf,
    node: PathBuf,
    dsh: PathBuf,
    origin: String,
}

impl Fixture {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-data-downgrade-{}-{nanos}",
            std::process::id()
        ));
        let dsh_home = root.join("dsh");
        let profile = dsh_home.join("profiles/web");
        for directory in [
            &profile,
            &root.join("home"),
            &root.join("run"),
            &root.join("bin"),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(profile.join("package.json"), "{}\n").unwrap();
        fs::write(profile.join("cordis.patch.yml"), "value: synthetic\n").unwrap();
        fs::set_permissions(root.join("home"), fs::Permissions::from_mode(0o700)).unwrap();
        let sysctl = root.join("bin/systemctl");
        fs::write(&sysctl, "#!/bin/bash\nexit 3\n").unwrap();
        fs::set_permissions(&sysctl, fs::Permissions::from_mode(0o755)).unwrap();
        // Fake interpreter: `node <script> --version` becomes `bash <script>`.
        let node = root.join("node");
        fs::write(
            &node,
            "#!/bin/bash\nif [ \"$1\" = \"--version\" ]; then printf 'v0.0.0-synthetic\\n'; exit 0; fi\nexec bash \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).unwrap();
        let dsh = root.join("fake-dsh");
        let home = root.join("home");
        Self {
            root,
            home,
            dsh_home,
            profile,
            node,
            dsh,
            origin: "http://127.0.0.1:9/".to_owned(),
        }
    }

    fn set_runtime_version(&self, version: &str) {
        fs::write(&self.dsh, format!("#!/bin/bash\nprintf '{version}\\n'\n")).unwrap();
        fs::set_permissions(&self.dsh, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dsh-ocean"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", &self.home)
            .env("DSH_HOME", &self.dsh_home)
            .env("DSH_OCEAN_NODE", &self.node)
            .env("DSH_OCEAN_DSH_BIN", &self.dsh)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("DSH_OCEAN_DATA_DIR", self.root.join("runtime"))
            .env("DSH_OCEAN_URL", &self.origin)
            .env("PATH", self.root.join("bin"));
        command
    }

    fn snapshot(&self) -> PathBuf {
        let output = self.command(&["--backup-profile"]).output().unwrap();
        assert!(output.status.success(), "{}", describe(&output));
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        PathBuf::from(json["snapshot"].as_str().unwrap())
    }

    fn restore(&self, snapshot: &Path, allow_downgrade: bool) -> Output {
        let mut arguments = vec![
            "--restore-profile",
            "web",
            snapshot.to_str().unwrap(),
            "--confirm-offline",
        ];
        if allow_downgrade {
            arguments.push("--allow-data-downgrade");
        }
        self.command(&arguments).output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with("dsh-ocean-data-downgrade-")
        }) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn describe(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn restore_refuses_older_profile_data_unless_explicitly_accepted() {
    let fixture = Fixture::new();
    fixture.set_runtime_version("0.3.0");
    let snapshot = fixture.snapshot();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(snapshot.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["runtimeVersion"], "0.3.0");
    assert!(manifest["clientVersion"].is_string());
    let before = fs::read(fixture.profile.join("cordis.patch.yml")).unwrap();

    // Mutate the live profile so a successful restore is observable.
    fs::write(fixture.profile.join("cordis.patch.yml"), "value: changed\n").unwrap();

    // Restoring data captured by an older runtime into a newer one is refused.
    fixture.set_runtime_version("0.4.0");
    let refused = fixture.restore(&snapshot, false);
    assert!(!refused.status.success());
    let text = describe(&refused);
    assert!(text.contains("schema-incompatible"), "{text}");
    assert!(text.contains("--allow-data-downgrade"), "{text}");
    assert_eq!(
        fs::read(fixture.profile.join("cordis.patch.yml")).unwrap(),
        b"value: changed\n",
        "a refused restore must not touch the profile"
    );

    // The explicit flag performs the restore and reports what was accepted.
    let accepted = fixture.restore(&snapshot, true);
    assert!(accepted.status.success(), "{}", describe(&accepted));
    let receipt: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(receipt["dataDowngrade"], true);
    assert_eq!(receipt["dataDowngradeAllowed"], true);
    assert_eq!(receipt["snapshotRuntimeVersion"], "0.3.0");
    assert_eq!(receipt["currentRuntimeVersion"], "0.4.0");
    let warnings = receipt["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|warning| warning
        .as_str()
        .is_some_and(|text| text.contains("explicitly accepted"))));
    assert_eq!(
        fs::read(fixture.profile.join("cordis.patch.yml")).unwrap(),
        before
    );

    // Restoring into an older runtime is an upgrade of data, not a downgrade.
    fixture.set_runtime_version("0.2.0");
    fs::write(
        fixture.profile.join("cordis.patch.yml"),
        "value: changed again\n",
    )
    .unwrap();
    let upgraded = fixture.restore(&snapshot, false);
    assert!(upgraded.status.success(), "{}", describe(&upgraded));
    let receipt: serde_json::Value = serde_json::from_slice(&upgraded.stdout).unwrap();
    assert_eq!(receipt["dataDowngrade"], false);
    assert_eq!(receipt["currentRuntimeVersion"], "0.2.0");
}

#[test]
fn snapshot_without_a_runtime_version_never_invents_compatibility() {
    let fixture = Fixture::new();
    // No detectable runtime at snapshot time.
    let output = {
        let mut command = fixture.command(&["--backup-profile"]);
        command.env_remove("DSH_OCEAN_DSH_BIN");
        command.output().unwrap()
    };
    assert!(output.status.success(), "{}", describe(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let snapshot = PathBuf::from(json["snapshot"].as_str().unwrap());
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(snapshot.join("manifest.json")).unwrap()).unwrap();
    assert!(manifest["runtimeVersion"].is_null());

    fixture.set_runtime_version("0.4.0");
    let restored = fixture.restore(&snapshot, false);
    assert!(restored.status.success(), "{}", describe(&restored));
    let receipt: serde_json::Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(receipt["dataDowngrade"], false);
    assert!(receipt["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("does not record the DSH runtime version"))));
}

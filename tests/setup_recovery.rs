//! Snapshot affordances are derived from the real filesystem gate, not error text.
use dsh_ocean::state::AppState;
use std::{fs, process::Command};

#[test]
fn setup_snapshot_reports_recovery_gate_and_native_busy_state() {
    if std::env::var_os("OCEAN_SETUP_RECOVERY_CHILD").is_some() {
        let state = AppState::default();
        let snapshot = || serde_json::to_value(state.snapshot()).unwrap();
        assert_eq!(snapshot()["recoveryRequired"], false);
        let pending = dsh_ocean::config::data_dir().join(".runtime-pending.json");
        // Even a corrupt marker blocks startup; the action must inspect it and
        // report refusal, never hide the gate or trust it as recovery authority.
        fs::write(&pending, b"{}").unwrap();
        assert_eq!(snapshot()["recoveryRequired"], true);
        let permit = state.begin_update().unwrap();
        assert_eq!(snapshot()["updating"], true);
        drop(permit);
        assert_eq!(snapshot()["updating"], false);
        std::env::set_var("DSH_OCEAN_URL", "https://example.invalid/");
        assert_eq!(snapshot()["recoveryRequired"], false);
        std::env::set_var("DSH_OCEAN_URL", "http://127.0.0.1:3080/");
        fs::remove_file(&pending).unwrap();
        assert_eq!(snapshot()["recoveryRequired"], false);
        let first = dsh_ocean::runtime_install::pending_path(&dsh_ocean::config::data_dir());
        fs::write(&first, b"incomplete first install").unwrap();
        state.invalidate_runtime_probe();
        assert_eq!(
            snapshot()["recoveryRequired"],
            false,
            "update recovery cannot handle first-install journal"
        );
        assert_eq!(
            snapshot()["canInstall"],
            true,
            "retry installation must remain reachable"
        );
        fs::remove_file(first).unwrap();
        return;
    }
    use std::os::unix::fs::DirBuilderExt;
    let home = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "ocean-setup-recovery-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
    let prefix = home.join("runtime");
    fs::create_dir(&prefix).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "setup_snapshot_reports_recovery_gate_and_native_busy_state",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("DSH_OCEAN_DATA_DIR", &prefix)
        .env("DSH_OCEAN_URL", "http://127.0.0.1:3080/")
        .env("OCEAN_SETUP_RECOVERY_CHILD", "1")
        .output()
        .unwrap();
    assert_eq!(home.canonicalize().unwrap(), home);
    fs::remove_dir_all(&home).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn bootstrap_check_mode_reports_a_usable_target() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = root.join("scripts/bootstrap-runtime.sh");
    let temp = std::env::temp_dir().join(format!(
        "dsh-ocean-bootstrap-check-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));

    let output = Command::new("bash")
        .arg(&script)
        .arg("--check")
        .env("DSH_OCEAN_DATA_DIR", &temp)
        .output()
        .expect("run bootstrap check");

    let _ = fs::remove_dir_all(&temp);
    assert!(
        output.status.success(),
        "bootstrap --check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("check: ok"), "unexpected output: {stdout}");
}

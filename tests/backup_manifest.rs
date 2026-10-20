//! Special snapshot manifests must fail before any blocking read.
#![cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
use std::{
    fs,
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[test]
fn fifo_manifest_is_rejected_without_a_writer() {
    if let Some(root) = std::env::var_os("OCEAN_MANIFEST_CHILD") {
        let root = std::path::PathBuf::from(root);
        for result in [
            dsh_ocean::backup_export::describe(&root).map(|_| ()),
            dsh_ocean::backup_export::inspect(&root).map(|_| ()),
            dsh_ocean::backup_export::export(&root, &root.join("export.tar")).map(|_| ()),
        ] {
            assert!(format!("{:#}", result.unwrap_err()).contains("not a regular file"));
        }
        assert!(!root.join("export.tar").exists());
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "dsh-ocean-manifest-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let fifo = std::ffi::CString::new(root.join("manifest.json").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fifo_manifest_is_rejected_without_a_writer",
            "--nocapture",
        ])
        .env_clear()
        .env("HOME", &root)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("OCEAN_MANIFEST_CHILD", &root)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // This exact absolute directory was created above, not supplied by the child.
    assert!(
        root.is_absolute()
            && root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("dsh-ocean-manifest-")
    );
    fs::remove_dir_all(&root).unwrap();
    assert!(status.expect("manifest read blocked on a FIFO").success());
}

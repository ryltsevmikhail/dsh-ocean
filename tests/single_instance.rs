//! The session lock must be exclusive and free of the unlink race.
#![cfg(target_os = "linux")]
use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-instance-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn lock_path(&self) -> PathBuf {
        self.0.join("dsh-ocean.lock")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self
            .0
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("dsh-ocean-instance-"))
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn lock(path: &PathBuf) -> Option<std::fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .ok()?;
    // SAFETY: the descriptor is owned and valid for the call.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 };
    locked.then_some(file)
}

#[test]
fn only_one_process_can_hold_the_session_lock() {
    let fixture = Fixture::new();
    let first = lock(&fixture.lock_path()).expect("the first claim succeeds");
    assert!(
        lock(&fixture.lock_path()).is_none(),
        "a second process must not claim the same session"
    );
    let mode = fs::metadata(fixture.lock_path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the lock file must be private");
    // Releasing the owner frees the session for the next launch.
    drop(first);
    assert!(lock(&fixture.lock_path()).is_some());
}

#[test]
fn a_crashed_owner_does_not_lose_the_session() {
    // flock is released by the kernel on exit, so a leftover file is not a lock.
    let fixture = Fixture::new();
    fs::write(fixture.lock_path(), b"").unwrap();
    assert!(
        lock(&fixture.lock_path()).is_some(),
        "a stale lock file must not block a new launch"
    );
}

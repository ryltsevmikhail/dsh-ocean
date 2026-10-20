//! The launch-token cache must not follow links or be briefly world-readable.
#![cfg(target_os = "linux")]
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-token-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let state = root.join("state");
        fs::create_dir_all(&root).unwrap();
        Self { root, state }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self
            .root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("dsh-ocean-token-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn the_token_cache_is_private_and_refuses_links() {
    if std::env::var_os("OCEAN_TOKEN_CACHE_CHILD").is_none() {
        let fixture = Fixture::new();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "the_token_cache_is_private_and_refuses_links",
                "--nocapture",
            ])
            .env_clear()
            .env("HOME", &fixture.root)
            .env("OCEAN_TOKEN_CACHE_CHILD", "1")
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("token cache read blocked on a FIFO");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    let fixture = Fixture::new();
    std::env::set_var("XDG_STATE_HOME", &fixture.state);
    dsh_ocean::auth::cache_token("synthetic-token-value");
    let path = fixture.state.join("dsh-ocean/web-token");
    let metadata = fs::metadata(&path).expect("token written");
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    let directory = fs::metadata(path.parent().unwrap()).unwrap();
    assert_eq!(
        directory.permissions().mode() & 0o777,
        0o700,
        "the state directory holds a token and must be private"
    );
    assert_eq!(
        dsh_ocean::auth::cached_token().as_deref(),
        Some("synthetic-token-value")
    );

    // A symlink at the cache path must be refused, and its target must be untouched.
    fs::remove_file(&path).unwrap();
    let target = fixture.root.join("victim.txt");
    fs::write(&target, b"untouched").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    dsh_ocean::auth::cache_token("another-token");
    // The link itself is replaced by an atomic rename; its target is never written.
    assert_eq!(fs::read(&target).unwrap(), b"untouched");
    let replaced = fs::symlink_metadata(&path).unwrap();
    assert!(replaced.file_type().is_file());
    assert_eq!(replaced.nlink(), 1);
    assert_eq!(replaced.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        dsh_ocean::auth::cached_token().as_deref(),
        Some("another-token")
    );

    // Reading refuses a link that appears after the write.
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert_eq!(dsh_ocean::auth::cached_token(), None);

    // An oversized file is not read.
    fs::remove_file(&path).unwrap();
    fs::write(&path, vec![b'x'; 8192]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(dsh_ocean::auth::cached_token(), None);

    // Hard links retain the one-link rejection even with nonblocking opens.
    fs::remove_file(&path).unwrap();
    fs::hard_link(&target, &path).unwrap();
    assert_eq!(dsh_ocean::auth::cached_token(), None);

    // No writer ever connects: a blocking open would hang this subprocess.
    fs::remove_file(&path).unwrap();
    use std::os::unix::ffi::OsStrExt;
    let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert_eq!(dsh_ocean::auth::cached_token(), None);
}

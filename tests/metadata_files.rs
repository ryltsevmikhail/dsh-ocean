//! Bounded reads of untrusted local metadata, never special files or symlinks.
#![cfg(target_os = "linux")]
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

use dsh_ocean::util::read_regular_text;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-metadata-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
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
            .starts_with("dsh-ocean-metadata-test-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn only_regular_utf8_files_within_the_limit_are_read() {
    let fixture = Fixture::new();
    let path = fixture.0.join("metadata");
    fs::write(&path, b"text").unwrap();
    assert_eq!(read_regular_text(&path, 4).unwrap(), "text");
    assert!(read_regular_text(&path, 3).is_err());
    assert!(read_regular_text(&fixture.0, 100).is_err());
    assert!(read_regular_text(&fixture.0.join("missing"), 100).is_err());
    fs::write(&path, b"\xff").unwrap();
    assert!(read_regular_text(&path, 100).is_err());
    fs::write(&path, b"").unwrap();
    assert_eq!(read_regular_text(&path, 0).unwrap(), "");
}

#[test]
fn symlinks_and_fifos_are_rejected_without_reading_the_target() {
    let fixture = Fixture::new();
    let target = fixture.0.join("target");
    fs::write(&target, "private-data").unwrap();
    let link = fixture.0.join("link");
    symlink(&target, &link).unwrap();
    assert!(read_regular_text(&link, 100).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "private-data");
    let path = fixture.0.join("fifo");
    let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(read_regular_text(&path, 100).is_err());
}

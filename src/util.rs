use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Check existing ancestry without creating anything. A root-owned sticky
/// shared parent (e.g. /tmp) is permitted; symlinks and other writable ancestors
/// are not. This does not sandbox hostile code running under the same UID.
pub(crate) fn check_owned_ancestors(path: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::os::unix::fs::MetadataExt;
    let uid = unsafe { libc::geteuid() };
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(m) => anyhow::ensure!(
                m.is_dir()
                    && (m.uid() == 0 || m.uid() == uid)
                    && (m.mode() & 0o022 == 0 || (m.uid() == 0 && m.mode() & libc::S_ISVTX != 0)),
                "unsafe install directory ancestor; must be owned and not writable by others"
            ),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e).context("inspect install directory"),
        }
    }
    Ok(())
}

/// Bounded UTF-8 metadata read. Never follows the final symlink or blocks
/// opening a FIFO; re-check the byte count in case a regular file grows.
pub fn read_regular_text(path: &Path, limit: u64) -> io::Result<String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected metadata file type or size",
        ));
    }
    let mut text = String::new();
    file.take(limit.saturating_add(1))
        .read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "metadata file exceeds size limit",
        ));
    }
    Ok(text)
}

pub fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Start and reap on a worker: dropping a Child does not reap it, and waiting
/// on the GTK thread would freeze the UI if the desktop opener stays running.
fn run_external(
    mut command: std::process::Command,
) -> io::Result<std::thread::JoinHandle<io::Result<std::process::ExitStatus>>> {
    std::thread::Builder::new()
        .name("ocean-external-opener".to_owned())
        .spawn(move || command.status())
}

fn open_external(target: &std::ffi::OsStr) {
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Spawn the worker before any child exists: a thread-creation failure must
    // not leave a child without a waiter. Error text never includes the target.
    if run_external(command).is_err() {
        eprintln!("dsh-ocean: could not start the external opener worker");
    }
}

pub fn open_path(path: &Path) {
    open_external(path.as_os_str());
}

pub fn open_url(url: &str) {
    open_external(std::ffi::OsStr::new(url));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn external_opener_worker_waits_for_and_reaps_its_child() {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-opener-test-{}",
            crate::downloads::unique_id()
        ));
        fs::create_dir(&root).unwrap();
        let pid_path = root.join("pid");
        let mut command = Command::new("/bin/bash");
        command
            .args(["-c", "printf '%s' \"$$\" > \"$1\"", "opener-fixture"])
            .arg(&pid_path);
        let worker = run_external(command).unwrap();
        assert!(worker.join().unwrap().unwrap().success());
        let pid: libc::pid_t = fs::read_to_string(pid_path).unwrap().parse().unwrap();
        let mut status = 0;
        // SAFETY: only tests whether our known child still needs reaping; a
        // completed worker must already have waited for it.
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-opener-test-"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_opener_worker_reports_spawn_failures_without_panicking() {
        let command = Command::new("/nonexistent-dsh-ocean-opener-test/executable");
        let result = run_external(command).unwrap().join().unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}

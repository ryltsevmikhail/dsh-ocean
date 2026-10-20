use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::AppHandle;

use crate::config;
use crate::windows;

/// Held for the lifetime of the owning process; the kernel releases the flock on
/// exit, crash included, so a stale lock file is harmless.
static LOCK: Mutex<Option<File>> = Mutex::new(None);

#[cfg(target_os = "linux")]
fn try_lock(file: &File) -> bool {
    // SAFETY: the descriptor is owned and valid for the call.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

#[cfg(not(target_os = "linux"))]
fn try_lock(_file: &File) -> bool {
    true
}

fn lock_path() -> PathBuf {
    let directory = config::runtime_dir();
    directory.join("dsh-ocean.lock")
}

/// Takes exclusive ownership with `flock`, which has no unlink race: a second
/// process cannot remove the owner's socket while the owner holds the lock.
fn claim_session() -> Option<File> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .ok()?;
    if try_lock(&file) {
        Some(file)
    } else {
        None
    }
}

fn socket_path() -> PathBuf {
    let directory = config::runtime_dir();
    directory.join("dsh-ocean.sock")
}

/// One idle/malformed peer must not hold the sequential activation listener.
/// Use a total deadline, not a fresh timeout for every partial read.
fn show_requested(stream: &mut UnixStream) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut request = [0u8; 5];
    let mut received = 0;
    while received < request.len() {
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            return false;
        };
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            return false;
        }
        match stream.read(&mut request[received..]) {
            Ok(0) => return false,
            Ok(count) => received += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
    request == *b"show\n"
}

/// Connect to a running instance and ask it to show its window.
///
/// Returns false when this process should exit because another instance owns
/// the desktop session.
pub fn acquire(app: AppHandle) -> bool {
    let path = socket_path();

    let Some(lock) = claim_session() else {
        // Another instance owns the session: ask it to show its window. If its
        // socket is momentarily unavailable the request is simply dropped; the
        // other instance is still the owner and this process must exit.
        if let Ok(mut stream) = UnixStream::connect(&path) {
            let _ = stream.write_all(b"show\n");
        }
        return false;
    };
    *LOCK.lock().unwrap_or_else(|error| error.into_inner()) = Some(lock);

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Only the lock holder reaches this point, so removing a leftover socket cannot
    // steal a live instance's socket.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("dsh-ocean: cannot bind single-instance socket: {error}");
            return true;
        }
    };

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    if !show_requested(&mut stream) {
                        continue;
                    }
                    let app_for_main = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        windows::show_main_or_setup(&app_for_main);
                    });
                }
                Err(error) => {
                    eprintln!("dsh-ocean: single-instance socket error: {error}");
                    break;
                }
            }
        }
    });
    true
}

#[cfg(test)]
mod request_tests {
    use super::*;

    #[test]
    fn only_a_complete_show_request_is_accepted() {
        for (request, accepted) in [
            (b"show\n".as_slice(), true),
            (b"nope\n", false),
            (b"show", false),
            (b"", false),
        ] {
            let (mut server, mut peer) = UnixStream::pair().unwrap();
            peer.write_all(request).unwrap();
            peer.shutdown(std::net::Shutdown::Write).unwrap();
            assert_eq!(show_requested(&mut server), accepted);
        }
    }

    #[test]
    fn fragmented_show_requests_are_not_mistaken_for_invalid_messages() {
        let (mut server, mut peer) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            peer.write_all(b"sh").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
            peer.write_all(b"ow\n").unwrap();
        });
        assert!(show_requested(&mut server));
        writer.join().unwrap();
    }

    #[test]
    fn an_idle_peer_is_rejected_without_waiting_for_it_to_close() {
        let (mut server, peer) = UnixStream::pair().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || sender.send(show_requested(&mut server)).unwrap());
        let result = receiver.recv_timeout(std::time::Duration::from_secs(3));
        // Unblock even a regressed implementation before failing the assertion.
        let _ = peer.shutdown(std::net::Shutdown::Both);
        reader.join().unwrap();
        assert!(!result.unwrap());
    }
}

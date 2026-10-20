//! Real HTTP attachment without Node, DSH, journalctl, systemctl or GUI access.
#![cfg(target_os = "linux")]
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use dsh_ocean::{auth, config, service};

#[test]
fn attachment_probe() {
    let Some(output) = std::env::var_os("DSH_OCEAN_TEST_ATTACH_OUTPUT") else {
        return;
    };
    auth::cache_token("fixture-attach-token");
    let base = config::checked_server_url().unwrap();
    let server = service::attach_existing(&base).unwrap();
    if let Some(server) = &server {
        assert_eq!(server.mode, service::ServerMode::Attached);
        assert_eq!(server.base_url, base);
        assert_eq!(server.token, "fixture-attach-token");
        assert!(server.unit.is_none());
    }
    fs::write(
        output,
        if server.is_some() {
            "attached"
        } else {
            "not-attached"
        },
    )
    .unwrap();
}

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-service-attach-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("empty-bin")).unwrap();
        let unit = root.join("config/systemd/user/dsh-web.service");
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        fs::write(unit, "unchanged existing unit\n").unwrap();
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
            .starts_with("dsh-ocean-service-attach-"));
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn exercise(first_status: u16, expected: &str) {
    let fixture = Fixture::new();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let expected_requests = if first_status == 401 { 2 } else { 1 };
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut requests = Vec::new();
        while requests.len() < expected_requests && Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("accept fixture request: {error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut raw = Vec::new();
            while !raw.ends_with(b"\r\n\r\n") && raw.len() < 8192 {
                let mut byte = [0];
                if stream.read(&mut byte).unwrap() == 0 {
                    break;
                }
                raw.push(byte[0]);
            }
            let request = String::from_utf8(raw).unwrap();
            let status = if requests.is_empty() {
                first_status
            } else {
                assert!(request.starts_with("GET /?token=fixture-attach-token HTTP/1.1\r\n"));
                303
            };
            stream.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).unwrap();
            requests.push(request);
        }
        requests
    });
    let output = fixture.0.join("result");
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "attachment_probe", "--nocapture"])
        .env_clear()
        .env("HOME", fixture.0.join("home"))
        .env("XDG_CONFIG_HOME", fixture.0.join("config"))
        .env("XDG_STATE_HOME", fixture.0.join("state"))
        .env("XDG_DATA_HOME", fixture.0.join("data"))
        .env("DSH_OCEAN_DATA_DIR", fixture.0.join("absent-runtime"))
        .env("DSH_OCEAN_URL", origin)
        .env("PATH", fixture.0.join("empty-bin"))
        .env("DSH_OCEAN_TEST_ATTACH_OUTPUT", &output)
        .output()
        .unwrap();
    let requests = server.join().unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    assert_eq!(requests.len(), expected_requests);
    assert!(requests[0].starts_with("GET / HTTP/1.1\r\n"));
    assert_eq!(fs::read_to_string(output).unwrap(), expected);
    assert!(!fixture.0.join("absent-runtime").exists());
    assert_eq!(
        fs::read_to_string(fixture.0.join("config/systemd/user/dsh-web.service")).unwrap(),
        "unchanged existing unit\n"
    );
}

#[test]
fn authenticated_existing_server_attaches_without_a_local_runtime() {
    exercise(401, "attached");
}

#[test]
fn an_unrelated_http_listener_is_not_adopted_or_sent_a_cached_token() {
    exercise(200, "not-attached");
}

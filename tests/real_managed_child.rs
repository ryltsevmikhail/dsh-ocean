//! Opt-in production helper lifecycle, not Tauri fallback/window dispatch.
//! Run with DSH_OCEAN_TEST_RUNTIME_ROOT set to an absolute, canonical installed
//! generation. No bootstrap, provider requests, systemd or GUI is involved.
//! The controller is a subreaper because start() gives Node its own process group.
//! SIGKILL of the controller itself cannot guarantee cleanup without a cgroup.
//! Ownership invariant: this dedicated binary has no unrelated subprocess tests;
//! every controller holds CONTROLLER until all descendants are reaped. Cleanup
//! intentionally reaps all direct/adopted children of this process, not the host.
#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::fs::{self, DirBuilder, OpenOptions};
use std::net::SocketAddr;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dsh_ocean::managed_child::{self, ManagedChild};

static CONTROLLER: Mutex<()> = Mutex::new(());
const WORKER: &str = "DSH_OCEAN_LIFECYCLE_WORKER";
const ROOT: &str = "DSH_OCEAN_TEST_RUNTIME_ROOT";
const FIXTURE: &str = "DSH_OCEAN_LIFECYCLE_FIXTURE";

struct Controller {
    root: PathBuf,
    worker: Option<Child>,
    previous_subreaper: libc::c_int,
    clean: bool,
    success: bool,
    adopted: usize,
}

impl Controller {
    fn new() -> Self {
        let root = PathBuf::from("/tmp").join(format!(
            "dsh-ocean-real-child-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        DirBuilder::new().mode(0o700).create(&root).unwrap();
        for name in [
            "home", "dsh", "config", "data", "state", "cache", "runtime", "tmp", "cwd",
        ] {
            DirBuilder::new()
                .mode(0o700)
                .create(root.join(name))
                .unwrap();
        }
        let mut previous = 0;
        // SAFETY: GET writes one integer; SET changes only this controller.
        unsafe {
            assert_eq!(libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut previous), 0);
            assert_eq!(libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1), 0);
        }
        Self {
            root,
            worker: None,
            previous_subreaper: previous,
            clean: false,
            success: false,
            adopted: 0,
        }
    }

    fn spawn(&mut self, test: &str, runtime: Option<&Path>) {
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.root.join("worker-output"))
            .unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", test, "--ignored", "--test-threads=1"])
            .env_clear()
            .env(WORKER, "real")
            .env(FIXTURE, &self.root)
            .env("HOME", self.root.join("home"))
            .env("DSH_HOME", self.root.join("dsh"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("TMP", self.root.join("tmp"))
            .env("TEMP", self.root.join("tmp"))
            .env("DSH_TELEMETRY_DISABLED", "1")
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent-ocean-lifecycle-bus",
            )
            .env("LANG", "C.UTF-8")
            .env("PATH", "/usr/bin:/bin")
            .current_dir(self.root.join("cwd"))
            .stdin(Stdio::null())
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .process_group(0);
        if let Some(runtime) = runtime {
            command.env(ROOT, runtime);
        }
        self.worker = Some(command.spawn().unwrap());
    }

    fn wait(&mut self, deadline: Duration) -> bool {
        let until = Instant::now() + deadline;
        loop {
            if let Some(status) = self.worker.as_mut().unwrap().try_wait().unwrap() {
                // Already reaped: do not signal a potentially reused worker PID.
                self.worker.take();
                return status.success();
            }
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn cleanup(&mut self) -> bool {
        if let Some(mut worker) = self.worker.take() {
            // The controller created and still owns this worker PID/group.
            unsafe {
                libc::kill(-(worker.id() as i32), libc::SIGKILL);
            }
            let _ = worker.kill();
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                match worker.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < until => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    _ => {
                        self.worker = Some(worker);
                        return false;
                    }
                }
            }
        }
        let until = Instant::now() + Duration::from_secs(8);
        loop {
            let Ok(children) = direct_children() else {
                return false;
            };
            self.adopted += children.len();
            for pid in children {
                // Only actual direct children from all of our task lists are
                // eligible. Never signal a foreign group just because it is
                // the group an adopted child happens to belong to.
                unsafe {
                    if libc::getpgid(pid) == pid {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                    libc::kill(pid, libc::SIGKILL);
                }
            }
            loop {
                let mut status = 0;
                let reaped = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
                if reaped > 0 {
                    continue;
                }
                if reaped == -1 {
                    match std::io::Error::last_os_error().raw_os_error() {
                        Some(libc::ECHILD) => {
                            self.clean = true;
                            return true;
                        }
                        Some(libc::EINTR) => continue,
                        _ => return false,
                    }
                }
                break;
            }
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        if !self.clean {
            self.cleanup();
        }
        // Restore only after cleanup, including during assertion unwinding.
        unsafe {
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, self.previous_subreaper);
        }
        if self.clean && self.success && !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.root);
        } else {
            eprintln!("lifecycle fixture preserved: {}", self.root.display());
        }
    }
}

fn direct_children() -> std::io::Result<BTreeSet<i32>> {
    let mut children = BTreeSet::new();
    for task in fs::read_dir("/proc/self/task")? {
        let path = task?.path().join("children");
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for pid in text.split_whitespace() {
            let pid: i32 = pid.parse().map_err(std::io::Error::other)?;
            if pid > 0 {
                children.insert(pid);
            }
        }
    }
    Ok(children)
}

#[test]
fn watchdog_reaps_unpublished_process_group() {
    let _serial = CONTROLLER.lock().unwrap_or_else(|e| e.into_inner());
    let mut controller = Controller::new();
    controller.spawn("watchdog_fixture_worker", None);
    let until = Instant::now() + Duration::from_secs(10);
    while !controller.root.join("ready").exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        controller.root.join("ready").exists(),
        "fixture worker did not become ready"
    );
    assert!(
        !controller.wait(Duration::from_millis(150)),
        "worker must stall"
    );
    assert!(controller.cleanup(), "watchdog did not reap every child");
    assert!(
        controller.adopted > 0,
        "regression must exercise an adopted orphan"
    );
    assert!(direct_children().unwrap().is_empty());
    controller.success = true;
}

#[test]
#[ignore = "internal isolated watchdog worker"]
#[allow(clippy::zombie_processes)] // Intentionally orphaned; controller must reap it.
fn watchdog_fixture_worker() {
    assert_eq!(std::env::var(WORKER).as_deref(), Ok("real"));
    let root = assert_worker_isolation();
    // Deliberately never publish the PID and never wait on this orphan.
    let _sleep = Command::new("/bin/sleep")
        .arg("120")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    fs::write(root.join("ready"), b"ready").unwrap();
    loop {
        std::thread::park_timeout(Duration::from_secs(60));
    }
}

#[test]
#[ignore = "requires explicit DSH_OCEAN_TEST_RUNTIME_ROOT; starts real Node/DSH"]
fn real_managed_child_lifecycle() {
    if std::env::var(WORKER).as_deref() == Ok("real") {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(real_worker(false));
        return;
    }
    let _serial = CONTROLLER.lock().unwrap_or_else(|e| e.into_inner());
    let root = runtime_root();
    let mut controller = Controller::new();
    controller.spawn("real_managed_child_lifecycle", Some(&root));
    let success = controller.wait(Duration::from_secs(210));
    let clean = controller.cleanup();
    assert!(clean, "watchdog could not confirm complete child cleanup");
    assert!(
        success,
        "real lifecycle worker failed or exceeded deadline; inspect private fixture"
    );
    assert_eq!(
        controller.adopted, 0,
        "normal stop must not rely on orphan cleanup"
    );
    controller.success = true;
}

#[test]
#[ignore = "requires explicit private runtime; verifies embedded locale overlay boot"]
fn real_managed_child_builtin_locales() {
    if std::env::var(WORKER).as_deref() == Ok("real") {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(real_worker(true));
        return;
    }
    let _serial = CONTROLLER.lock().unwrap_or_else(|e| e.into_inner());
    let root = runtime_root();
    let mut controller = Controller::new();
    controller.spawn("real_managed_child_builtin_locales", Some(&root));
    let success = controller.wait(Duration::from_secs(210));
    let clean = controller.cleanup();
    assert!(clean, "locale worker descendants were not fully reaped");
    assert!(
        success,
        "private embedded-locale worker failed; inspect retained fixture"
    );
    assert_eq!(controller.adopted, 0);
    controller.success = true;
}

const PRIVATE_DIRS: &[(&str, &str)] = &[
    ("HOME", "home"),
    ("DSH_HOME", "dsh"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];

fn assert_private_environment(
    environment: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    fixture: &Path,
) {
    use std::ffi::OsStr;
    for (key, directory) in PRIVATE_DIRS {
        let expected = fixture.join(directory);
        assert!(
            environment
                .get(OsStr::new(key))
                .is_some_and(|value| value == expected.as_os_str()),
            "private directory environment must match the fixture"
        );
    }
    for key in environment.keys() {
        assert!(
            PRIVATE_DIRS.iter().any(|(name, _)| key == OsStr::new(name))
                || [
                    WORKER,
                    ROOT,
                    FIXTURE,
                    "PATH",
                    "LANG",
                    "DSH_TELEMETRY_DISABLED",
                    "DBUS_SESSION_BUS_ADDRESS"
                ]
                .iter()
                .any(|name| key == OsStr::new(name)),
            "unexpected inherited environment variable"
        );
    }
    assert!(environment
        .get(OsStr::new("DSH_TELEMETRY_DISABLED"))
        .is_some_and(|v| v == "1"));
    assert!(environment
        .get(OsStr::new("DBUS_SESSION_BUS_ADDRESS"))
        .is_some_and(|v| v == "unix:path=/nonexistent-ocean-lifecycle-bus"));
}

fn assert_worker_isolation() -> PathBuf {
    let fixture = PathBuf::from(std::env::var_os(FIXTURE).expect("private fixture required"));
    assert!(
        fixture.parent() == Some(Path::new("/tmp"))
            && fixture
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("dsh-ocean-real-child-"))
            && fixture.canonicalize().is_ok_and(|p| p == fixture)
    );
    assert_eq!(std::env::current_dir().unwrap(), fixture.join("cwd"));
    assert_private_environment(&std::env::vars_os().collect(), &fixture);
    fixture
}

fn assert_child_isolation(child: &ManagedChild, fixture: &Path, node: &Path) {
    use std::os::unix::ffi::OsStringExt;
    let pid = child.child.id().expect("ready child PID");
    let proc = PathBuf::from(format!("/proc/{pid}"));
    assert_eq!(
        fs::read_link(proc.join("exe")).unwrap(),
        node.canonicalize().unwrap()
    );
    assert!(fs::read_link(proc.join("cwd"))
        .unwrap()
        .starts_with(fixture));
    let mut environment = std::collections::BTreeMap::new();
    for entry in fs::read(proc.join("environ"))
        .unwrap()
        .split(|byte| *byte == 0)
    {
        if entry.is_empty() {
            continue;
        }
        let equals = entry
            .iter()
            .position(|byte| *byte == b'=')
            .expect("environment entry");
        let key = std::ffi::OsString::from_vec(entry[..equals].to_vec());
        let value = std::ffi::OsString::from_vec(entry[equals + 1..].to_vec());
        assert!(
            environment.insert(key, value).is_none(),
            "duplicate environment key"
        );
    }
    assert_private_environment(&environment, fixture);
}

fn runtime_root() -> PathBuf {
    let root = PathBuf::from(std::env::var_os(ROOT).expect("explicit runtime root required"));
    assert!(root.is_absolute(), "runtime root must be absolute");
    assert!(
        root.canonicalize().is_ok_and(|canonical| canonical == root),
        "runtime root must be an existing canonical generation, not a current symlink"
    );
    for relative in ["runtime/node/bin/node", "harness/node_modules/.bin/dsh"] {
        let file = root.join(relative);
        assert!(
            file.is_file() && file.canonicalize().is_ok_and(|p| p.starts_with(&root)),
            "runtime executable must belong to the supplied generation"
        );
    }
    root
}

fn validate_base(child: &ManagedChild) -> SocketAddr {
    let base = &child.base_url;
    assert_eq!(base.scheme(), "http");
    assert_eq!(base.path(), "/");
    assert!(base.query().is_none() && base.fragment().is_none());
    assert!(base.username().is_empty() && base.password().is_none());
    let port = base.port().expect("explicit real port");
    assert!(port != 0 && port != 3080, "must not contact the user's GUI");
    let ip = match base.host().unwrap() {
        url::Host::Ipv4(ip) => std::net::IpAddr::V4(ip),
        url::Host::Ipv6(ip) => std::net::IpAddr::V6(ip),
        _ => panic!("literal loopback required"),
    };
    assert!(ip.is_loopback());
    SocketAddr::new(ip, port)
}

fn status(address: SocketAddr, token: Option<&str>) -> u16 {
    let mut url = url::Url::parse(&format!("http://{address}/")).unwrap();
    if let Some(token) = token {
        url.query_pairs_mut().append_pair("token", token);
    }
    // Use the production bounded, non-redirecting probe. Never print URL/token.
    dsh_ocean::http::status(&url).expect("bounded HTTP probe must answer")
}

async fn stop(child: &mut ManagedChild) {
    if let Some(pid) = child.child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.child.start_kill();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.child.wait())
            .await
            .is_ok_and(|result| result.is_ok()),
        "child must be reaped"
    );
}

fn authenticated_html(address: SocketAddr, token: &str) -> String {
    use std::io::{Read, Write};
    fn get(address: SocketAddr, path: &str, cookie: &str) -> Vec<u8> {
        assert!(address.ip().is_loopback());
        let mut stream =
            std::net::TcpStream::connect_timeout(&address, Duration::from_secs(3)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let request=format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        let mut bytes = Vec::new();
        stream
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .unwrap();
        assert!(bytes.len() <= 2 * 1024 * 1024);
        bytes
    }
    let mut url = url::Url::parse(&format!("http://{address}/")).unwrap();
    url.query_pairs_mut().append_pair("token", token);
    let login = get(address, &format!("/?{}", url.query().unwrap()), "");
    let login = String::from_utf8(login).unwrap();
    let headers = login.split("\r\n\r\n").next().unwrap();
    let cookies = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.trim().split(';').next().unwrap().to_owned())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        !cookies.is_empty(),
        "private login did not issue a session cookie"
    );
    let html = get(address, "/", &cookies);
    String::from_utf8(html).unwrap()
}

async fn real_worker(builtin_locales: bool) {
    let root = runtime_root();
    let fixture = assert_worker_isolation();
    let marker = fixture.join("dsh/lifecycle-marker");
    fs::write(&marker, b"survives restart").unwrap();
    let path = format!("{}:/usr/bin:/bin", root.join("runtime/node/bin").display());
    let patch = if builtin_locales {
        Some(
            dsh_ocean::builtin_locale_cache::materialize(&fixture.join("cache/builtin-locales"))
                .unwrap()
                .join("overlay.yml"),
        )
    } else {
        None
    };
    let args = dsh_ocean::service::managed_arguments(patch.as_deref()).unwrap();
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let node = root.join("runtime/node/bin/node");
    let dsh = root.join("harness/node_modules/.bin/dsh");
    let log_path = fixture.join("private-runtime-output");
    let log: managed_child::Log = Arc::new(move |line| {
        use std::io::Write;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log_path)
            .unwrap();
        writeln!(file, "{line}").unwrap();
    });
    let mut first = managed_child::start(
        &node,
        &dsh,
        &args,
        &path,
        Duration::from_secs(75),
        log.clone(),
    )
    .await
    .expect("first managed start");
    let address = validate_base(&first);
    assert_child_isolation(&first, &fixture, &node);
    assert!(!first.token.is_empty());
    assert_eq!(status(address, None), 401);
    assert_eq!(status(address, Some(&first.token)), 303);
    if builtin_locales {
        let html = authenticated_html(address, &first.token);
        assert!(
            html.contains("@dsh-ocean/builtin-locales"),
            "native locale package absent from served boot graph"
        );
        // Stock SDK starts from an empty generated root and composes argv
        // overlays in memory. Later Loader writeback may persist rows, but the
        // language feature must not manually write them into this file.
        assert!(
            fs::read_to_string(fixture.join("dsh/profiles/web/cordis.yml"))
                .unwrap()
                .ends_with("[]\n")
        );
        assert!(
            !fs::read_to_string(fixture.join("dsh/profiles/web/package.json"))
                .unwrap()
                .contains("@dsh-ocean/"),
            "native dictionaries must not become an installed profile dependency"
        );
    }
    stop(&mut first).await;
    let mut second = managed_child::start(
        &node,
        &dsh,
        &args,
        &path,
        Duration::from_secs(75),
        log.clone(),
    )
    .await
    .expect("second managed start");
    let address = validate_base(&second);
    assert_child_isolation(&second, &fixture, &node);
    assert!(!second.token.is_empty());
    assert!(
        second.token != first.token,
        "restart must rotate authentication token"
    );
    assert_eq!(status(address, None), 401);
    assert_eq!(status(address, Some(&first.token)), 401);
    assert_eq!(status(address, Some(&second.token)), 303);
    if builtin_locales {
        assert!(authenticated_html(address, &second.token).contains("@dsh-ocean/builtin-locales"));
    }
    assert!(fs::read(&marker).is_ok_and(|bytes| bytes == b"survives restart"));
    stop(&mut second).await;
    if let Some(patch) = patch {
        use std::os::unix::fs::PermissionsExt;
        let tree = patch.parent().unwrap();
        assert!(tree.is_absolute() && tree.starts_with(fixture.join("cache/builtin-locales")));
        // Only this test's own sealed tree, after both workers were reaped.
        fs::set_permissions(tree, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(tree.join("lib"), fs::Permissions::from_mode(0o700)).unwrap();
    }
}

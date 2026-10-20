//! Real process lifecycle of the managed DSH child, driven by local scripts.
//!
//! No DSH, Node, network listener or service is involved: /bin/bash stands in
//! for Node and reads a script that prints a synthetic URL or misbehaves.
//! Using an installed interpreter avoids ETXTBSY from executing a fresh writable
//! fixture inode while other tests fork in parallel.
#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use dsh_ocean::managed_child;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    node: PathBuf,
    dsh: PathBuf,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let serial = NEXT.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-managed-child-{}-{serial}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let node = PathBuf::from("/bin/bash");
        let dsh = root.join("dsh");
        fs::write(&dsh, format!("#!/bin/bash\n{body}")).unwrap();
        fs::set_permissions(&dsh, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root, node, dsh }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with("dsh-ocean-managed-child-")
        }) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn logs() -> (managed_child::Log, Arc<Mutex<Vec<String>>>) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    let log: managed_child::Log = Arc::new(move |line: String| {
        sink.lock().unwrap().push(line);
    });
    (log, lines)
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only probes existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[tokio::test]
async fn discovers_the_authenticated_url_and_keeps_a_live_child() {
    let fixture = Fixture::new(
        "printf 'booting\\n'\nprintf 'dsh web: http://127.0.0.1:4567/?token=synthetic-token\\n'\nexec sleep 30\n",
    );
    let (log, lines) = logs();
    let started = managed_child::start(
        &fixture.node,
        &fixture.dsh,
        &["--profile", "web"],
        "/usr/bin:/bin",
        Duration::from_secs(10),
        log,
    )
    .await
    .unwrap_or_else(|error| panic!("the child announces its URL: {error:#}"));
    assert_eq!(started.base_url.as_str(), "http://127.0.0.1:4567/");
    assert_eq!(started.token, "synthetic-token");
    let pid = started.child.id().unwrap() as i32;
    assert!(alive(pid), "a ready child must still be running");
    let mut child = started.child;
    managed_child::stop_child(&mut child).await.unwrap();
    let status = child.wait().await.unwrap();
    assert!(!status.success());
    assert!(!alive(pid) || status.code().is_some(), "child was reaped");
    let seen = lines.lock().unwrap().join("\n");
    assert!(seen.contains("[dsh] booting"), "{seen}");
    assert!(
        seen.contains("synthetic-token"),
        "log keeps raw harness output"
    );
}

#[tokio::test]
async fn silent_child_times_out_and_is_killed() {
    let fixture = Fixture::new("printf 'no url here\\n'\nexec sleep 30\n");
    let (log, _) = logs();
    let began = std::time::Instant::now();
    let error = managed_child::start(
        &fixture.node,
        &fixture.dsh,
        &[],
        "/usr/bin:/bin",
        Duration::from_secs(1),
        log,
    )
    .await
    .expect_err("no URL means no readiness");
    assert!(error.to_string().contains("timed out"), "{error:#}");
    assert!(began.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn immediate_exit_reports_a_closed_stream() {
    let fixture = Fixture::new("exit 3\n");
    let (log, _) = logs();
    let error = managed_child::start(
        &fixture.node,
        &fixture.dsh,
        &[],
        "/usr/bin:/bin",
        Duration::from_secs(10),
        log,
    )
    .await
    .expect_err("an exited child cannot be ready");
    assert!(error.to_string().contains("exited before"), "{error:#}");
}

#[tokio::test]
async fn tokenless_url_is_ignored_until_a_valid_line_arrives() {
    let fixture = Fixture::new(
        "printf 'dsh web: http://127.0.0.1:4567/\\n'\nprintf 'still starting\\n'\nsleep 0.3\nprintf 'dsh web: http://127.0.0.1:4567/?token=late-token\\n'\nexec sleep 30\n",
    );
    let (log, _) = logs();
    let started = managed_child::start(
        &fixture.node,
        &fixture.dsh,
        &[],
        "/usr/bin:/bin",
        Duration::from_secs(10),
        log,
    )
    .await
    .unwrap_or_else(|error| panic!("the later valid line is accepted: {error:#}"));
    assert_eq!(started.token, "late-token");
    let mut child = started.child;
    // The child may already be gone under heavy parallel load; the interesting
    // assertion is the discovered token, not the kill result.
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[tokio::test]
async fn missing_executable_is_an_error_without_a_child() {
    let fixture = Fixture::new("exit 0\n");
    let (log, _) = logs();
    let missing = fixture.root.join("absent-node");
    let error = managed_child::start(
        &missing,
        &fixture.dsh,
        &[],
        "/usr/bin:/bin",
        Duration::from_secs(1),
        log,
    )
    .await
    .expect_err("spawning a missing executable fails");
    assert!(
        error.to_string().contains("spawn DSH web process"),
        "{error:#}"
    );
    assert!(Path::new(&missing).is_absolute());
}

#[tokio::test]
async fn a_foreign_origin_is_never_adopted_and_the_child_is_killed() {
    // The readiness line is untrusted child output: a non-loopback origin must be
    // ignored, the start must fail, and the child must not survive.
    let fixture = Fixture::new(
        "printf 'dsh web: http://example.invalid:9/?token=synthetic-token\n'\nexec sleep 30\n",
    );
    let script_pid_file = fixture.root.join("pid");
    fs::write(
        &fixture.dsh,
        format!(
            "#!/bin/bash\necho $$ >'{}'\nprintf 'dsh web: http://example.invalid:9/?token=synthetic-token\n'\nexec sleep 30\n",
            script_pid_file.display()
        ),
    )
    .unwrap();
    let (log, _) = logs();
    let error = managed_child::start(
        &fixture.node,
        &fixture.dsh,
        &[],
        "/usr/bin:/bin",
        Duration::from_millis(600),
        log,
    )
    .await
    .expect_err("a foreign origin must not be adopted");
    assert!(error.to_string().contains("timed out"), "{error:#}");
    let pid: i32 = fs::read_to_string(&script_pid_file)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    if pid > 0 {
        // The killed child must not answer signal 0 as a live process.
        for _ in 0..50 {
            if !alive(pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !alive(pid),
            "the rejected child {pid} survived the failed start"
        );
    }
}

#[tokio::test]
async fn a_failed_start_kills_the_whole_child_group() {
    // The harness can start shells and tools; a failed readiness must not leave them.
    let fixture = Fixture::new("true");
    let grandchild = fixture.root.join("grandchild.pid");
    fs::write(
        &fixture.dsh,
        format!(
            "#!/bin/bash\nsleep 30 &\necho $! >'{}'\nexec sleep 30\n",
            grandchild.display()
        ),
    )
    .unwrap();
    let (log, _) = logs();
    let error = managed_child::start(
        &fixture.node,
        &fixture.dsh,
        &[],
        "/usr/bin:/bin",
        Duration::from_millis(700),
        log,
    )
    .await
    .expect_err("no readiness line");
    assert!(error.to_string().contains("timed out"), "{error:#}");
    let pid: i32 = fs::read_to_string(&grandchild)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    assert!(pid > 0, "the fixture recorded a descendant");
    for _ in 0..60 {
        if !alive(pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!alive(pid), "descendant {pid} outlived the failed start");
}

#[tokio::test]
async fn group_stop_reaps_a_dead_leader_without_leaking_its_descendant() {
    // A dedicated controller subreaps the worker's children even if it panics
    // or times out before stop_child. Never change subreaper state in this
    // parallel integration-test runner.
    if std::env::var_os("DSH_OCEAN_GROUP_STOP_CONTROLLER").is_some() {
        group_stop_controller();
        return;
    }
    let mut worker = tokio::process::Command::new(std::env::current_exe().unwrap());
    worker
        .args([
            "--exact",
            "group_stop_reaps_a_dead_leader_without_leaking_its_descendant",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("DSH_OCEAN_GROUP_STOP_CONTROLLER", "1")
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(60), worker.output())
        .await
        .expect("isolated group-stop controller must finish within 60s")
        .unwrap();
    assert!(
        output.status.success(),
        "worker failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn group_stop_children() -> std::io::Result<std::collections::BTreeSet<i32>> {
    let mut children = std::collections::BTreeSet::new();
    for task in fs::read_dir("/proc/self/task")? {
        match fs::read_to_string(task?.path().join("children")) {
            Ok(text) => {
                for pid in text.split_whitespace() {
                    children.insert(pid.parse().map_err(std::io::Error::other)?);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(children)
}

fn group_stop_quiesce() -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let Ok(children) = group_stop_children() else {
            return false;
        };
        for pid in children {
            // These are this dedicated controller's actual unreaped children.
            // Never signal a non-leader's arbitrary process group.
            unsafe {
                if libc::getpgid(pid) == pid {
                    libc::kill(-pid, libc::SIGKILL);
                }
                libc::kill(pid, libc::SIGKILL);
            }
        }
        loop {
            let mut status = 0;
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid > 0 {
                continue;
            }
            if pid == -1 {
                match std::io::Error::last_os_error().raw_os_error() {
                    Some(libc::ECHILD) => return true,
                    Some(libc::EINTR) => continue,
                    _ => return false,
                }
            }
            break;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn group_stop_controller() {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    // No unrelated subprocess tests execute in this exact-selected process.
    assert_eq!(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) }, 0);
    let marker_fixture = Fixture::new("exit 0\n");
    let marker = marker_fixture.root.join("fault-ready");
    for intentional_failure in [false, true] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "owned_group_stop_worker",
                "--ignored",
                "--nocapture",
            ])
            .env_remove("DSH_OCEAN_GROUP_STOP_CONTROLLER")
            .env("DSH_OCEAN_GROUP_STOP_WORKER", "1")
            .stdin(Stdio::null())
            .process_group(0);
        if intentional_failure {
            command
                .env("DSH_OCEAN_GROUP_STOP_FAULT", "1")
                .env("DSH_OCEAN_GROUP_STOP_FAULT_MARKER", &marker)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
        }
        let mut worker = command.spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let outcome = loop {
            match worker.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => break None,
            }
        };
        if outcome.is_none() {
            let _ = worker.kill();
            // quiesce also reaps this still-owned worker and any adopted child.
        }
        let clean = group_stop_quiesce();
        assert!(clean, "controller could not quiesce its worker descendants");
        assert!(
            !intentional_failure || marker.is_file(),
            "fault must follow ready descendant creation"
        );
        assert!(
            outcome.is_some_and(|status| status.success() != intentional_failure),
            "worker result or bounded deadline was unexpected"
        );
    }
}

#[tokio::test]
#[ignore = "invoked only by the isolated group-stop parent test"]
async fn owned_group_stop_worker() {
    if std::env::var_os("DSH_OCEAN_GROUP_STOP_WORKER").is_none() {
        return;
    }
    // SAFETY: this dedicated process has no unrelated tests or children.
    assert_eq!(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) }, 0);
    for kill_leader_first in [false, true] {
        let fixture = Fixture::new(
            "sleep 30 &\nprintf '%s\\n' $! > \"$1\"\nprintf 'dsh web: http://127.0.0.1:4567/?token=stop-test\\n'\nwait\n",
        );
        let pid_file = fixture.root.join("descendant.pid");
        let (log, _) = logs();
        let mut ready = managed_child::start(
            &fixture.node,
            &fixture.dsh,
            &[pid_file.to_str().unwrap()],
            "/usr/bin:/bin",
            Duration::from_secs(3),
            log,
        )
        .await
        .unwrap();
        let leader = ready.child.id().unwrap() as i32;
        let descendant: i32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SAFETY: both PIDs were just obtained from this exact owned fixture.
        assert_eq!(unsafe { libc::getpgid(descendant) }, leader);
        if std::env::var_os("DSH_OCEAN_GROUP_STOP_FAULT").is_some() {
            fs::write(
                std::env::var_os("DSH_OCEAN_GROUP_STOP_FAULT_MARKER").unwrap(),
                b"ready",
            )
            .unwrap();
            panic!("intentional worker failure before group stop");
        }
        if kill_leader_first {
            assert_eq!(unsafe { libc::kill(leader, libc::SIGKILL) }, 0);
            // Observe death without reaping: Child must still own the group ID.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
                let result = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        leader as libc::id_t,
                        info.as_mut_ptr(),
                        libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                    )
                };
                assert_eq!(result, 0);
                if unsafe { info.assume_init().si_pid() } == leader {
                    break;
                }
                assert!(tokio::time::Instant::now() < deadline, "leader did not die");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(
                alive(descendant),
                "leader-only kill must leave the descendant"
            );
        }
        managed_child::stop_child(&mut ready.child).await.unwrap();
        assert!(ready.child.id().is_none(), "leader must be reaped");
        // Only reap the exact adopted descendant, never waitpid(-1).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut status = 0;
        let mut reaped_here = false;
        loop {
            let result = unsafe { libc::waitpid(descendant, &mut status, libc::WNOHANG) };
            if result == descendant {
                reaped_here = true;
                break;
            }
            // When both processes are killed together, the shell can reap its
            // terminated child before its own SIGKILL is delivered. Adoption is
            // not required in that legal ordering, but disappearance still is.
            if !kill_leader_first
                && result == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
            {
                let gone = unsafe { libc::kill(descendant, 0) } == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
                assert!(
                    gone,
                    "a non-adopted descendant must be absent, not inaccessible"
                );
                break;
            }
            assert_eq!(
                result, 0,
                "exact descendant must be adopted by this subreaper"
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "descendant survived stop"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if reaped_here {
            assert!(libc::WIFSIGNALED(status));
            assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
        }
        assert!(!alive(leader));
        assert!(!alive(descendant));
        // No cached PID/PGID is reused after reaping.
        managed_child::stop_child(&mut ready.child).await.unwrap();
    }
}

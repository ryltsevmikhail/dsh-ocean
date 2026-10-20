//! Cooperative cancellation of the real embedded runtime installer, offline.
//!
//! A fake `curl` blocks the installer mid-download so no network or real user
//! data is involved. The test then signals the installer's process group and
//! checks that the script's own traps removed its staged generation and
//! download scratch, leaving the runtime prefix empty.
#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Prefix {
    root: PathBuf,
    data: PathBuf,
    tools: PathBuf,
    home: PathBuf,
}

impl Prefix {
    fn new() -> Self {
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-bootstrap-cancel-{}-{serial}",
            std::process::id()
        ));
        let data = root.join("runtime prefix");
        let tools = root.join("fake-tools");
        let home = root.join("private-home");
        for directory in [&data, &tools, &home] {
            fs::create_dir_all(directory).expect("create private fixture directory");
        }
        fs::set_permissions(&tools, fs::Permissions::from_mode(0o755)).unwrap();
        // Fake curl: honour `-o`, then block so cancellation happens mid-download.
        let curl = tools.join("curl");
        fs::write(
            &curl,
            "#!/usr/bin/env bash\nset -euo pipefail\nif [ \"${FAKE_CURL_FAIL:-0}\" = 1 ]; then exit 7; fi\nout=''\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    -o) out=\"$2\"; shift 2 ;;\n    *) shift ;;\n  esac\ndone\n[ -n \"$out\" ] && printf 'synthetic-archive' > \"$out\"\nsleep 30\n",
        )
        .unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
        // Cancellation tests need to reach the download irrespective of runner
        // capacity. The threshold itself is covered by bootstrap_install.
        let df = tools.join("df");
        fs::write(&df, "#!/bin/bash\nprintf 'Filesystem 1024-blocks Used Available Capacity Mounted on\\nfixture 8000000 0 4000000 0%% /\\n'\n").unwrap();
        fs::set_permissions(&df, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            root,
            data,
            tools,
            home,
        }
    }

    fn generations(&self) -> Vec<PathBuf> {
        let directory = self.data.join("generations");
        match fs::read_dir(&directory) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .map(|e| e.path())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn scratch(&self) -> Vec<PathBuf> {
        match fs::read_dir(&self.data) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .map(|e| e.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.starts_with(".download.") || name.starts_with(".pointer.")
                        })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

impl Drop for Prefix {
    fn drop(&mut self) {
        // Only this test's unique temporary directory.
        if self.root.starts_with(std::env::temp_dir())
            && self.root.file_name().is_some_and(|n| {
                n.to_string_lossy()
                    .starts_with("dsh-ocean-bootstrap-cancel-")
            })
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/bootstrap-runtime.sh")
}

#[test]
fn the_shipped_script_file_matches_the_embedded_installer() {
    // The app feeds `include_str!` to bash via stdin; these tests drive the file
    // on disk. Fail loudly if the two ever differ instead of drifting silently.
    let embedded = dsh_ocean::bootstrap::install_script();
    let on_disk = fs::read_to_string(script()).expect("read the runtime installer");
    assert_eq!(on_disk, embedded);
}

fn signal_group(pid: u32, signal: i32) {
    assert!(pid > 1);
    // SAFETY: negative pid targets only the child's own group, which this
    // process created. Signalling the leader directly as well would duplicate
    // TERM and can kill bash mid-cleanup, so the group is the single target.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

fn wait_until(mut condition: impl FnMut() -> bool, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    condition()
}

#[test]
fn cancelled_installer_removes_its_staged_generation_and_download_scratch() {
    let prefix = Prefix::new();
    let path = format!(
        "{}:{}",
        prefix.tools.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let child = Command::new("bash")
        .arg(script())
        .env("PATH", &path)
        .env("HOME", &prefix.home)
        .env("DSH_OCEAN_DATA_DIR", &prefix.data)
        .env("DSH_NODE_VERSION", "0.0.0-synthetic")
        .env("DSH_NODE_SHA256", "0".repeat(64))
        .env("DSH_VERSION", "latest")
        .env("DSH_PNPM_VERSION", "0.0.0-synthetic")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("spawn the real installer script");
    let pid = child.id();

    // Wait until the installer has created its staged generation and download
    // scratch: that is the exact window cancellation must clean up.
    assert!(
        wait_until(
            || !prefix.generations().is_empty() || !prefix.scratch().is_empty(),
            Duration::from_secs(10)
        ),
        "installer never reached its staging phase"
    );
    assert!(
        wait_until(
            || !prefix.generations().is_empty() && !prefix.scratch().is_empty(),
            Duration::from_secs(10)
        ),
        "installer did not create both staging paths before cancellation"
    );

    signal_group(pid, libc::SIGTERM);
    let output = child.wait_with_output().expect("reap installer");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "cancelled installer must not report success: {stderr}"
    );
    assert!(
        output.status.code() == Some(143) || output.status.code() == Some(130),
        "expected the script's TERM/INT trap exit, got {:?}: {stderr}",
        output.status
    );

    // The script's own EXIT cleanup must have run: no staged generation, no
    // download/pointer scratch, and no selector was activated.
    assert!(
        prefix.generations().is_empty(),
        "staged generation survived cancellation: {:?}",
        prefix.generations()
    );
    assert!(
        prefix.scratch().is_empty(),
        "download/pointer scratch survived cancellation: {:?}",
        prefix.scratch()
    );
    assert!(
        !prefix.data.join("current").exists(),
        "cancellation must not activate a runtime selector"
    );
    assert!(!prefix.data.join("runtime").exists());
    assert!(!prefix.data.join("harness").exists());
    // Only the installer lock file and the now-empty `generations` directory
    // may remain inside the private prefix.
    assert!(prefix.generations().is_empty());
    for entry in fs::read_dir(&prefix.data).expect("read prefix") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        let name = name.to_string_lossy().into_owned();
        assert!(
            name == ".bootstrap.lock" || name == "generations",
            "unexpected leftover: {name}"
        );
    }
    // Nothing was created in the private HOME either.
    assert_eq!(
        fs::read_dir(&prefix.home).expect("read home").count(),
        0,
        "cancelled installer must not populate HOME"
    );
}

#[test]
fn a_second_installer_is_refused_while_one_holds_the_lock() {
    let prefix = Prefix::new();
    let path = format!(
        "{}:{}",
        prefix.tools.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut first = Command::new("bash")
        .arg(script())
        .env("PATH", &path)
        .env("HOME", &prefix.home)
        .env("DSH_OCEAN_DATA_DIR", &prefix.data)
        .env("DSH_NODE_VERSION", "0.0.0-synthetic")
        .env("DSH_NODE_SHA256", "0".repeat(64))
        .env("DSH_VERSION", "latest")
        .env("DSH_PNPM_VERSION", "0.0.0-synthetic")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn first installer");
    assert!(
        wait_until(
            || prefix.data.join(".bootstrap.lock").exists(),
            Duration::from_secs(10)
        ),
        "first installer never acquired its lock"
    );
    let second = Command::new("bash")
        .arg(script())
        .env("PATH", &path)
        .env("HOME", &prefix.home)
        .env("DSH_OCEAN_DATA_DIR", &prefix.data)
        .env("DSH_NODE_VERSION", "0.0.0-synthetic")
        .env("DSH_NODE_SHA256", "0".repeat(64))
        .env("DSH_VERSION", "latest")
        .env("DSH_PNPM_VERSION", "0.0.0-synthetic")
        .output()
        .expect("run competing installer");
    assert!(!second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("another bootstrap is already running"),
        "unexpected refusal: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    signal_group(first.id(), libc::SIGTERM);
    let _ = first.wait();
    assert!(prefix.generations().is_empty());
    assert!(prefix.scratch().is_empty());
}

fn run_gc(data: &Path, home: &Path) -> std::process::Output {
    Command::new("bash")
        .arg(script())
        .arg("--gc")
        .env("HOME", home)
        .env("DSH_OCEAN_DATA_DIR", data)
        .output()
        .expect("run --gc")
}

fn generation(prefix: &Path, name: &str, previous: &str) {
    let directory = prefix.join("generations").join(name);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("payload"), b"synthetic runtime").unwrap();
    fs::write(directory.join(".previous"), format!("{previous}\n")).unwrap();
}

#[test]
fn gc_keeps_the_selected_generation_and_one_rollback_hop() {
    let prefix = Prefix::new();
    fs::create_dir_all(prefix.data.join("generations")).unwrap();
    generation(&prefix.data, "gen.aaa", "none");
    generation(&prefix.data, "gen.bbb", "generations/gen.aaa");
    generation(&prefix.data, "gen.ccc", "generations/gen.bbb");
    fs::create_dir_all(prefix.data.join(".download.stale")).unwrap();
    fs::write(prefix.data.join(".download.stale/file"), b"partial").unwrap();
    fs::create_dir_all(prefix.data.join(".pointer.stale")).unwrap();
    fs::write(prefix.data.join("notes.txt"), b"operator file").unwrap();
    fs::create_dir_all(prefix.data.join("harness")).unwrap();
    fs::create_dir_all(prefix.data.join("runtime")).unwrap();
    std::os::unix::fs::symlink("generations/gen.bbb", prefix.data.join("current")).unwrap();

    let output = run_gc(&prefix.data, &prefix.home);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(prefix.data.join("generations/gen.aaa").is_dir());
    assert!(prefix.data.join("generations/gen.bbb").is_dir());
    assert!(!prefix.data.join("generations/gen.ccc").exists());
    assert!(!prefix.data.join(".download.stale").exists());
    assert!(!prefix.data.join(".pointer.stale").exists());
    assert_eq!(
        fs::read(prefix.data.join("notes.txt")).unwrap(),
        b"operator file"
    );
    assert!(prefix.data.join("harness").is_dir());
    assert!(prefix.data.join("runtime").is_dir());
    assert_eq!(
        fs::read_link(prefix.data.join("current")).unwrap(),
        Path::new("generations/gen.bbb")
    );
}

#[test]
fn gc_is_a_clean_no_op_without_generations_and_shows_help() {
    let prefix = Prefix::new();
    let output = run_gc(&prefix.data, &prefix.home);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("no runtime generations"));
    let help = Command::new("bash")
        .arg(script())
        .arg("--help")
        .output()
        .expect("run --help");
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(help.status.success());
    assert!(text.contains("--gc") && text.contains("one rollback hop"));
    assert!(!prefix.data.read_dir().unwrap().any(|entry| {
        entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".download.")
    }));
}

#[test]
fn gc_refuses_a_symlinked_generations_directory() {
    let prefix = Prefix::new();
    let outside = prefix.root.join("outside-generations");
    fs::create_dir_all(outside.join("gen.aaa")).unwrap();
    std::os::unix::fs::symlink(&outside, prefix.data.join("generations")).unwrap();
    let output = run_gc(&prefix.data, &prefix.home);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("generations must not be a symlink"));
    assert!(outside.join("gen.aaa").is_dir());
}

#[test]
fn install_sweeps_stale_scratch_before_downloading() {
    let prefix = Prefix::new();
    fs::create_dir_all(prefix.data.join(".download.dead")).unwrap();
    fs::write(
        prefix.data.join(".download.dead/file"),
        b"left by a killed run",
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        prefix.tools.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("bash")
        .arg(script())
        .env("PATH", &path)
        .env("HOME", &prefix.home)
        .env("DSH_OCEAN_DATA_DIR", &prefix.data)
        .env("DSH_NODE_VERSION", "0.0.0-synthetic")
        .env("DSH_NODE_SHA256", "0".repeat(64))
        .env("DSH_VERSION", "latest")
        .env("DSH_PNPM_VERSION", "0.0.0-synthetic")
        .env("FAKE_CURL_FAIL", "1")
        .output()
        .expect("run installer with failing download");
    // The download fails, but the stale scratch must already be gone.
    assert!(!output.status.success());
    assert!(!prefix.data.join(".download.dead").exists());
    assert!(prefix.generations().is_empty());
    assert!(prefix.scratch().is_empty());
    assert!(!prefix.data.join("current").exists());
}

#[test]
fn gc_keeps_the_generation_the_installed_unit_pins() {
    // The unit starts a concrete generation; after a rollback that generation is no
    // longer `current`, and collecting it would leave the service unable to start.
    let prefix = Prefix::new();
    fs::create_dir_all(prefix.data.join("generations")).unwrap();
    generation(&prefix.data, "gen.aaa", "none");
    generation(&prefix.data, "gen.bbb", "generations/gen.aaa");
    generation(&prefix.data, "gen.pinned", "generations/gen.aaa");
    std::os::unix::fs::symlink("generations/gen.bbb", prefix.data.join("current")).unwrap();
    let unit_dir = prefix.home.join(".config/systemd/user");
    fs::create_dir_all(&unit_dir).unwrap();
    fs::write(
        unit_dir.join("dsh-web.service"),
        format!(
            "[Service]\nExecStart=\"{}/generations/gen.pinned/runtime/node/bin/node\" \"--version\"\nWorkingDirectory=%h\n",
            prefix.data.display()
        ),
    )
    .unwrap();

    let output = run_gc(&prefix.data, &prefix.home);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        prefix.data.join("generations/gen.pinned").is_dir(),
        "the unit-pinned generation was collected: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(prefix.data.join("generations/gen.bbb").is_dir());
    assert!(prefix.data.join("generations/gen.aaa").is_dir());
}

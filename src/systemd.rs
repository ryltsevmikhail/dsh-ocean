use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use url::Url;

use crate::bootstrap::{path_env, Runtime};
use crate::config;

fn systemctl(args: &[&str]) -> Result<Output> {
    Command::new("systemctl")
        .args(args)
        .output()
        .context("run systemctl")
}

pub fn available() -> bool {
    if !Path::new("/run/systemd/system").exists() {
        return false;
    }
    match systemctl(&["--user", "is-system-running"]) {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            !stderr.contains("failed to connect") && !stderr.contains("no such file")
        }
        Err(_) => false,
    }
}

pub fn unit_path() -> PathBuf {
    config::xdg_dir("XDG_CONFIG_HOME", ".config")
        .join("systemd")
        .join("user")
        .join(config::UNIT_NAME)
}

/// Escaping for a command line in `ExecStart=`, where systemd performs `$VAR`
/// expansion even inside double quotes.
fn exec_escape(value: &str) -> String {
    systemd_escape(value).replace('$', "$$")
}

fn systemd_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "%%")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

pub fn render_unit(runtime: &Runtime, base: &Url) -> String {
    render(runtime, base, None, None)
}

/// Pure explicit recipe; caller must validate ownership, consent and cache before
/// installing it. Never consults ambient preferences or writes a unit.
pub fn render_unit_with_patch(runtime: &Runtime, base: &Url, patch: &Path) -> Result<String> {
    anyhow::ensure!(
        patch.is_absolute()
            && !patch
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
        "absolute locale patch path required"
    );
    let patch = patch.to_str().context("locale patch path must be UTF-8")?;
    Ok(render(runtime, base, None, Some(patch)))
}

/// Recognize only our exact current template or its historical pre-UMask form.
/// Compatibility is for capturing the before-image, not rewriting arbitrary
/// units or altering existing data. New units always get the private mask.
pub(crate) fn is_generated_unit(runtime: &Runtime, base: &Url, bytes: &[u8]) -> bool {
    let current = render_unit(runtime, base);
    bytes == current.as_bytes()
        || bytes
            == current
                .replacen("Type=simple\nUMask=0077\n", "Type=simple\n", 1)
                .as_bytes()
}

/// Renders the same unit with a non-loopback bind and explicit trusted hosts.
///
/// Used only to stage a candidate file for a human to inspect; this function never
/// writes or installs anything itself.
pub fn render_exposed_unit(
    runtime: &Runtime,
    base: &Url,
    trusted_hosts: &[String],
) -> anyhow::Result<String> {
    anyhow::ensure!(
        !trusted_hosts.is_empty(),
        "at least one trusted host is required"
    );
    let port = config::server_port(base);
    let mut units = String::new();
    for host in trusted_hosts {
        anyhow::ensure!(
            !host.is_empty()
                && host.len() <= 255
                && !host.chars().any(|c| c.is_control() || c.is_whitespace())
                && !host.contains(['"', '\\', '$', '`', '%']),
            "unexpected trusted host value"
        );
        units.push_str(&format!(" --trusted-host {}", systemd_escape(host)));
    }
    let plain = render(runtime, base, None, None);
    Ok(plain.replace(
        &format!("--host 127.0.0.1 --port {port} --no-open"),
        &format!("--host 0.0.0.0 --port {port}{units} --no-open"),
    ))
}

/// Isolated validation services do not inherit provider credentials, npm config,
/// user profiles, runtime sockets or automatic restart behavior from the manager.
pub fn render_isolated_unit(runtime: &Runtime, base: &Url, home: &Path) -> Result<String> {
    anyhow::ensure!(
        home.is_absolute()
            && home.is_dir()
            && home != Path::new("/")
            && !home.to_string_lossy().chars().any(char::is_control)
            && !home.to_string_lossy().ends_with('\\')
            && fs::canonicalize(home)? != fs::canonicalize(config::home_dir())?,
        "a separate existing service HOME is required"
    );
    Ok(render(runtime, base, Some(home), None))
}

pub fn test_unit_name(name: &str) -> bool {
    name.len() <= 128
        && name
            .strip_prefix("dsh-ocean-test-")
            .and_then(|s| s.strip_suffix(".service"))
            .is_some_and(|s| {
                !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
}

pub fn isolated_path_env(runtime: &Runtime) -> String {
    let mut paths = Vec::new();
    if let Some(bin) = runtime.node.parent() {
        paths.push(bin.display().to_string());
    }
    if let Some(root) = runtime.install_root.as_ref().and_then(|path| path.parent()) {
        paths.push(root.join("tools/node_modules/.bin").display().to_string());
    }
    paths.extend(["/usr/bin".to_owned(), "/bin".to_owned()]);
    paths.join(":")
}

fn render(runtime: &Runtime, base: &Url, isolated: Option<&Path>, patch: Option<&str>) -> String {
    let locale = patch
        .map(|path| format!(" --patch \"{}\"", exec_escape(path)))
        .unwrap_or_default();
    let port = config::server_port(base);
    let node = exec_escape(&runtime.node.display().to_string());
    let dsh = exec_escape(&runtime.dsh.display().to_string());
    let path = systemd_escape(&if isolated.is_some() {
        isolated_path_env(runtime)
    } else {
        path_env(Some(&runtime.node))
    });
    let (prefix, working, restart, limits) = if let Some(home) = isolated {
        let values = [
            ("HOME", home.to_path_buf()),
            ("DSH_HOME", home.join(".dsh")),
            ("XDG_CONFIG_HOME", home.join(".config")),
            ("XDG_DATA_HOME", home.join(".local/share")),
            ("XDG_STATE_HOME", home.join(".local/state")),
            ("XDG_CACHE_HOME", home.join(".cache")),
            ("XDG_RUNTIME_DIR", home.join(".run")),
        ];
        let mut prefix = "/usr/bin/env -i".to_owned();
        for (key, value) in values {
            prefix.push_str(&format!(
                " \"{key}={}\"",
                systemd_escape(&value.display().to_string())
            ));
        }
        prefix.push_str(&format!(" \"PATH={path}\" DSH_TELEMETRY_DISABLED=1 "));
        (
            prefix,
            home.display().to_string().replace('%', "%%"),
            "no",
            "RuntimeMaxSec=20min\n",
        )
    } else {
        (String::new(), "%h".to_owned(), "on-failure", "")
    };
    format!(
        "[Unit]\n\
         Description=DeepSeek Harness web UI (localhost:{port})\n\
         Documentation=https://github.com/deepseek-ai/deepseek-harness\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         UMask=0077\n\
         ExecStart={prefix}\"{node}\" \"{dsh}\" --profile web --host 127.0.0.1 --port {port} --no-open{locale}\n\
         WorkingDirectory={working}\n\
         Environment=\"PATH={path}\"\n\
         Restart={restart}\n\
         RestartSec=2\n\
         {limits}\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

pub fn is_active() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", config::UNIT_NAME])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn daemon_reload() -> Result<()> {
    let output = systemctl(&["--user", "daemon-reload"])?;
    if !output.status.success() {
        anyhow::bail!(
            "systemctl --user daemon-reload failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

const MAX_UNIT_BYTES: u64 = 64 * 1024;

pub(crate) fn read_owned_unit(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("refuse unsafe unit {}", path.display())),
    };
    let before = file.metadata()?;
    anyhow::ensure!(
        before.is_file() && before.nlink() == 1 && before.len() <= MAX_UNIT_BYTES,
        "refuse non-regular, multiply linked or oversized unit {}",
        path.display()
    );
    let mut bytes = Vec::new();
    (&file).take(MAX_UNIT_BYTES + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_UNIT_BYTES
            && after.nlink() == 1
            && named.nlink() == 1
            && named.is_file()
            && before.dev() == named.dev()
            && before.ino() == named.ino()
            && before.len() == after.len()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec(),
        "unit changed while reading {}",
        path.display()
    );
    Ok(Some(bytes))
}

/// Existing files are never replaced, even if they look like an older generated unit.
/// Publication links complete private bytes into the missing name without overwriting.
fn install_missing_unit(path: &Path, content: &[u8]) -> Result<bool> {
    for _ in 0..3 {
        if let Some(current) = read_owned_unit(path)? {
            anyhow::ensure!(
                current == content,
                "unit has differing existing content; automatic replacement refused: {}",
                path.display()
            );
            return Ok(false);
        }
        let parent = path.parent().context("unit has no parent")?;
        fs::create_dir_all(parent)?;
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let stage = parent.join(format!(
            ".dsh-ocean-unit-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&stage)?;
        let candidate = stage.join("unit");
        let result = (|| -> Result<bool> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&candidate)?;
            file.write_all(content)?;
            file.sync_all()?;
            match fs::hard_link(&candidate, path) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                Err(e) => Err(e.into()),
            }
        })();
        // Only these exact private staging paths are removed; never the destination.
        let _ = fs::remove_file(&candidate);
        let _ = fs::remove_dir(&stage);
        if result? {
            anyhow::ensure!(
                read_owned_unit(path)?.as_deref() == Some(content),
                "published unit changed; refusing service action"
            );
            return Ok(true);
        }
    }
    anyhow::bail!("unit path kept changing; automatic creation refused")
}

/// Write and enable the neutral `dsh-web.service` unit. Never rewrites an
/// active unit, so an already-running harness keeps its live sessions.
pub fn ensure_unit(runtime: &Runtime, base: &Url) -> Result<()> {
    if is_active() {
        return Ok(());
    }
    let path = unit_path();
    let content = render_unit(runtime, base);
    if install_missing_unit(&path, content.as_bytes())? {
        daemon_reload()?;
    }
    let output = systemctl(&["--user", "enable", config::UNIT_NAME])?;
    if !output.status.success() {
        anyhow::bail!(
            "could not enable {}: {}",
            config::UNIT_NAME,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub fn start() -> Result<()> {
    let output = systemctl(&["--user", "start", config::UNIT_NAME])?;
    if !output.status.success() {
        anyhow::bail!(
            "could not start {}: {}",
            config::UNIT_NAME,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub fn restart() -> Result<()> {
    let output = systemctl(&["--user", "restart", config::UNIT_NAME])?;
    if !output.status.success() {
        anyhow::bail!(
            "could not restart {}: {}",
            config::UNIT_NAME,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Checked stop for the owned switch engine: a stop that fails must refuse the
/// whole transaction, never be silently treated as inactive.
pub fn stop() -> Result<()> {
    let output = systemctl(&["--user", "stop", config::UNIT_NAME])?;
    if !output.status.success() {
        anyhow::bail!(
            "could not stop {}: {}",
            config::UNIT_NAME,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Checked inactive proof. `systemctl --user show <unit> --property=ActiveState
/// --value` returns the exact state; only an explicit `inactive` proves the
/// service is stopped. `active`, `failed`, transitional (`activating` /
/// `deactivating` / `reloading`), unknown or unreadable states all fail closed,
/// so a caller can never mistake a failed or transitional service for "stopped".
/// (The legacy `is-active --quiet` exit 3 covered every non-active state, so a
/// "failed" unit would have masqueraded as "inactive"; the exact-state read does
/// not.)
pub fn is_inactive() -> Result<bool> {
    let output = systemctl(&[
        "--user",
        "show",
        config::UNIT_NAME,
        "--property=ActiveState",
        "--value",
    ])?;
    if !output.status.success() {
        anyhow::bail!("could not query the managed service state; the service state is unknown");
    }
    let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    match state.as_str() {
        "inactive" => Ok(true),
        "active" => Ok(false),
        "failed" | "activating" | "deactivating" | "reloading" | "maintenance" => {
            anyhow::bail!("the managed service is not inactive (state {state}); the inactive proof requires an explicit inactive state")
        }
        "" => anyhow::bail!(
            "the managed service state could not be determined; the inactive proof requires an explicit inactive state"
        ),
        _ => anyhow::bail!(
            "the managed service reported an unexpected state; the inactive proof requires an explicit inactive state"
        ),
    }
}

/// The two manager-environment variables the owned-switch engine must prove,
/// captured as raw bytes exactly as the manager emitted them (never
/// shell-evaluated). Other variables are never retained.
pub(crate) struct ManagerEnvironment {
    /// Each `HOME=` value, in order of appearance.
    pub home: Vec<Vec<u8>>,
    /// Each `DSH_HOME=` value, in order of appearance.
    pub dsh_home: Vec<Vec<u8>>,
}

/// A normal user-manager environment is far smaller; a larger block produces no
/// proof rather than an unbounded read.
const MAX_MANAGER_ENV_BYTES: usize = 64 * 1024;
const MANAGER_ENV_TIMEOUT: Duration = Duration::from_secs(5);

/// Read the user manager environment, retaining only the `HOME=` and `DSH_HOME=`
/// lines. Every other variable is discarded as it streams in and is never logged
/// or retained; the query is bounded in size and time so an oversized or hung
/// manager environment produces no proof (fail closed), and `stderr` is never
/// captured. This is the source of truth for the effective systemd service data
/// home, independent of the client's own `HOME`/`DSH_HOME`.
pub(crate) fn show_environment() -> Result<ManagerEnvironment> {
    // Reaps the child (and its process group) on every early-return path so a
    // hung/oversized query cannot leak a live process.
    struct Guard(std::process::Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let mut child = Guard(
        Command::new("systemctl")
            .args(["--user", "show-environment"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // Never capture stderr: it can carry paths/errors we must not retain.
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .context("run systemctl --user show-environment")?,
    );
    let mut stdout = child
        .0
        .stdout
        .take()
        .context("capture show-environment stdout")?;
    set_nonblocking(&stdout)?;

    let start = Instant::now();
    let mut filter = EnvironmentFilter::default();
    let mut buf = [0u8; 4096];
    let status = loop {
        if start.elapsed() >= MANAGER_ENV_TIMEOUT {
            anyhow::bail!(
                "the user manager environment query timed out; refusing an owned systemd switch"
            );
        }
        let mut progressed = false;
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    filter.push(&buf[..n])?;
                    progressed = true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => anyhow::bail!(
                    "the user manager environment could not be read; refusing an owned systemd switch"
                ),
            }
        }
        if let Some(status) = child.0.try_wait().context("reap show-environment")? {
            break status;
        }
        if !progressed {
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    // Child exited: drain any final buffered bytes before verifying the result.
    loop {
        match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => filter.push(&buf[..n])?,
        }
    }
    anyhow::ensure!(
        status.success(),
        "the user manager environment could not be read; refusing an owned systemd switch"
    );
    Ok(filter.finish())
}

fn set_nonblocking(pipe: &impl AsRawFd) -> Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    anyhow::ensure!(
        flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "could not configure the manager environment query"
    );
    Ok(())
}

/// Streams `show-environment` output, retaining only the `HOME=`/`DSH_HOME=`
/// lines and discarding every other variable immediately.
#[derive(Default)]
struct EnvironmentFilter {
    pending: Vec<u8>,
    home: Vec<Vec<u8>>,
    dsh_home: Vec<Vec<u8>>,
    total: usize,
}

impl EnvironmentFilter {
    fn push(&mut self, chunk: &[u8]) -> Result<()> {
        self.total += chunk.len();
        anyhow::ensure!(
            self.total <= MAX_MANAGER_ENV_BYTES,
            "the user manager environment is too large to prove; refusing an owned systemd switch"
        );
        self.pending.extend_from_slice(chunk);
        while let Some(newline) = self.pending.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.pending.drain(..=newline).collect();
            line.pop(); // the '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.classify(&line);
        }
        Ok(())
    }

    fn classify(&mut self, line: &[u8]) {
        if let Some(value) = line.strip_prefix(b"HOME=") {
            self.home.push(value.to_vec());
        } else if let Some(value) = line.strip_prefix(b"DSH_HOME=") {
            self.dsh_home.push(value.to_vec());
        }
        // Every other variable is dropped here and never retained or logged.
    }

    fn finish(mut self) -> ManagerEnvironment {
        if !self.pending.is_empty() {
            let trailing = std::mem::take(&mut self.pending);
            self.classify(&trailing);
        }
        ManagerEnvironment {
            home: self.home,
            dsh_home: self.dsh_home,
        }
    }
}

/// Manager provenance for the owned unit: the exact fragment path it resolves to
/// and any drop-ins. The switch engine refuses a fragment that is not the fixed
/// generated unit file, and refuses any drop-in.
pub(crate) fn unit_provenance(unit: &str) -> Result<(PathBuf, Vec<PathBuf>)> {
    let fragment = systemctl(&["--user", "show", unit, "--property=FragmentPath", "--value"])?;
    if !fragment.status.success() {
        anyhow::bail!(
            "could not inspect {unit} fragment: {}",
            String::from_utf8_lossy(&fragment.stderr).trim()
        );
    }
    let fragment_path = PathBuf::from(String::from_utf8_lossy(&fragment.stdout).trim().to_owned());
    let drop_ins = systemctl(&["--user", "show", unit, "--property=DropInPaths", "--value"])?;
    if !drop_ins.status.success() {
        anyhow::bail!(
            "could not inspect {unit} drop-ins: {}",
            String::from_utf8_lossy(&drop_ins.stderr).trim()
        );
    }
    let drop_in_paths: Vec<PathBuf> = String::from_utf8_lossy(&drop_ins.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect();
    Ok((fragment_path, drop_in_paths))
}

/// Replace the owned unit file only when its current content is one of the known
/// transaction images (before/after); the original file mode is preserved. This
/// is a check-then-rename guard: it refuses an external edit that is already on
/// disk, but the window between the content check and the rename is not a
/// hostile-race proof — a same-UID process deliberately racing the rename can
/// still be overwritten. The guard is cooperative, not a security boundary.
pub(crate) fn replace_owned_unit(path: &Path, allowed: &[&[u8]], content: &[u8]) -> Result<()> {
    let current = read_owned_unit(path)?.context("owned unit disappeared before replacement")?;
    anyhow::ensure!(
        allowed.iter().any(|known| *known == current),
        "unit changed outside the transaction; refusing to overwrite"
    );
    let mode = fs::symlink_metadata(path)?.mode() & 0o777;
    let parent = path.parent().context("unit has no parent")?;
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let stage = parent.join(format!(
        ".dsh-ocean-unit-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&stage)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::set_permissions(&stage, fs::Permissions::from_mode(mode))?;
        fs::rename(&stage, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&stage);
        return result;
    }
    let after = read_owned_unit(path)?.context("replaced unit disappeared")?;
    anyhow::ensure!(
        after == content,
        "unit replacement did not persist; refusing service action"
    );
    Ok(())
}

pub fn status_text() -> String {
    match systemctl(&[
        "--user",
        "status",
        config::UNIT_NAME,
        "--no-pager",
        "-n",
        "30",
    ]) {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            text
        }
        Err(error) => format!("systemctl unavailable: {error}"),
    }
}

pub fn journal_tail(lines: usize) -> String {
    let count = lines.to_string();
    let output = Command::new("journalctl")
        .args([
            "--user",
            "-u",
            config::UNIT_NAME,
            "-n",
            &count,
            "--no-pager",
            "-o",
            "cat",
        ])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        Ok(output) => String::from_utf8_lossy(&output.stderr).into_owned(),
        Err(error) => format!("journalctl unavailable: {error}"),
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    fn runtime() -> Runtime {
        Runtime {
            dsh: "/opt/runtime/dsh".into(),
            node: "/opt/runtime/node".into(),
            version: "0.2.0".into(),
            install_root: Some("/opt/runtime".into()),
        }
    }

    fn base() -> Url {
        Url::parse("http://127.0.0.1:3080/").expect("valid base")
    }

    #[test]
    fn concurrent_creators_never_replace_the_winner() {
        let root = std::env::temp_dir().join(format!(
            "dsh-unit-race-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("unit");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = [b"first".as_slice(), b"second".as_slice()]
            .into_iter()
            .map(|content| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    install_missing_unit(&path, content)
                })
            })
            .collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| matches!(r, Ok(true))).count(), 1);
        assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
        let bytes = fs::read(&path).unwrap();
        assert!(bytes == b"first" || bytes == b"second");
        assert_eq!(fs::metadata(&path).unwrap().nlink(), 1);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unit_publication_preserves_unsafe_and_custom_files() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "dsh-unit-safety-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("unit");
        assert!(install_missing_unit(&path, b"complete generated unit\n").unwrap());
        assert!(!install_missing_unit(&path, b"complete generated unit\n").unwrap());
        assert!(install_missing_unit(&path, b"different unit\n").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"complete generated unit\n");
        let target = root.join("target");
        fs::rename(&path, &target).unwrap();
        symlink(&target, &path).unwrap();
        assert!(install_missing_unit(&path, b"replacement").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"complete generated unit\n");
        fs::remove_file(&path).unwrap();
        fs::hard_link(&target, &path).unwrap();
        assert!(install_missing_unit(&path, b"replacement").is_err());
        fs::remove_file(&path).unwrap();
        let fifo = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(install_missing_unit(&path, b"replacement").is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, vec![b'x'; MAX_UNIT_BYTES as usize + 1]).unwrap();
        assert!(install_missing_unit(&path, b"replacement").is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn exec_start_escapes_dollar_signs_that_systemd_would_expand() {
        let dollar = std::path::Path::new("/home/a$b/.local/share/dsh-ocean");
        let runtime = Runtime {
            node: dollar.join("runtime/node/bin/node"),
            dsh: dollar.join("harness/node_modules/.bin/dsh"),
            version: "test".to_owned(),
            install_root: Some(dollar.to_path_buf()),
        };
        let unit = render_unit(&runtime, &base());
        let exec = unit
            .lines()
            .find(|line| line.starts_with("ExecStart="))
            .expect("an ExecStart line");
        assert!(exec.contains("a$$b"), "{exec}");
        assert!(!exec.contains("a$b"), "{exec}");
        // Environment values keep a literal dollar: systemd does not expand them.
        let environment = unit
            .lines()
            .find(|line| line.starts_with("Environment=\"PATH="))
            .expect("a PATH line");
        assert!(environment.contains("a$b"), "{environment}");
    }

    #[test]
    fn the_default_unit_binds_loopback_without_trusted_hosts() {
        let unit = render_unit(&runtime(), &base());
        assert!(unit.contains("--host 127.0.0.1 --port 3080"), "{unit}");
        assert!(!unit.contains("--trusted-host"), "{unit}");
        assert!(!unit.contains("0.0.0.0"), "{unit}");
        assert!(unit.contains("Restart=on-failure"), "{unit}");
    }

    #[test]
    fn the_exposed_unit_rewrites_only_the_bind_and_adds_trusted_hosts() {
        let plain = render_unit(&runtime(), &base());
        let exposed = render_exposed_unit(
            &runtime(),
            &base(),
            &["192.168.1.50:3080".to_owned(), "fd00::5:3080".to_owned()],
        )
        .expect("exposed unit renders");
        assert!(exposed.contains("--host 0.0.0.0 --port 3080"), "{exposed}");
        assert!(
            exposed.contains("--trusted-host 192.168.1.50:3080 --trusted-host fd00::5:3080"),
            "{exposed}"
        );
        assert!(!exposed.contains("--host 127.0.0.1"), "{exposed}");
        // Everything except the ExecStart bind stays byte-identical.
        let strip = |unit: &str| {
            unit.replace("--host 127.0.0.1", "")
                .replace("--host 0.0.0.0", "")
                .replace(" --trusted-host 192.168.1.50:3080", "")
                .replace(" --trusted-host fd00::5:3080", "")
        };
        assert_eq!(strip(&plain), strip(&exposed));
    }

    #[test]
    fn the_port_always_comes_from_the_same_base_url() {
        let odd_port = Url::parse("http://127.0.0.1:9999/").expect("valid base");
        let exposed = render_exposed_unit(&runtime(), &odd_port, &["192.168.1.50:9999".to_owned()])
            .expect("renders");
        assert!(exposed.contains("--host 0.0.0.0 --port 9999"), "{exposed}");
        assert!(
            exposed.contains("--trusted-host 192.168.1.50:9999"),
            "{exposed}"
        );
    }

    #[test]
    fn bad_or_missing_trusted_hosts_are_refused() {
        for bad in [
            String::new(),
            "host with space".to_owned(),
            "quote\"inside".to_owned(),
            "back\\slash".to_owned(),
            "dollar$sign".to_owned(),
            "newline\ninside".to_owned(),
        ] {
            let outcome = render_exposed_unit(&runtime(), &base(), std::slice::from_ref(&bad));
            assert!(outcome.is_err(), "{bad:?} was accepted");
        }
        assert!(render_exposed_unit(&runtime(), &base(), &[]).is_err());
    }
}

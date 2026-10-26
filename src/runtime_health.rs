//! Bounded process-group ownership for isolated candidate checks.
use crate::update_control::Control;
use anyhow::Result;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
fn fresh(published: &str, expires: &str, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
    use anyhow::Context;
    let published =
        chrono::DateTime::parse_from_rfc3339(published).context("invalid publication time")?;
    let expires =
        chrono::DateTime::parse_from_rfc3339(expires).context("invalid expiration time")?;
    let latest = now
        .checked_add_signed(chrono::Duration::minutes(5))
        .context("health verification clock out of range")?;
    anyhow::ensure!(
        expires > now && published <= latest,
        "runtime metadata expired or execution clock window refused"
    );
    Ok(())
}
/// Health is evidence for this exact staged tree, not an activation permit.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    generation: std::path::PathBuf,
    manifest_sha256: String,
    components: crate::runtime_feed::Components,
    health_checked: bool,
    activated: bool,
}
pub fn check(
    prepared: &crate::runtime_bundle::Prepared,
    release: &crate::runtime_feed::Verified,
    control: Arc<Control>,
) -> Result<Health> {
    check_authorized(prepared, release, control, || {
        fresh(
            &release.manifest().published_at,
            &release.manifest().expires_at,
            chrono::Utc::now(),
        )
    })
}

// Only the sealed, exact compiled bootstrap proof authorizes timeless health.
// There is intentionally no public expiry flag or Verified -> proof conversion.
pub(crate) fn check_immutable_bootstrap(
    prepared: &crate::runtime_bundle::Prepared,
    proof: &crate::runtime_feed::ImmutableBootstrap,
    control: Arc<Control>,
) -> Result<Health> {
    check_authorized(prepared, proof.verified(), control, || Ok(()))
}
fn check_authorized(
    prepared: &crate::runtime_bundle::Prepared,
    release: &crate::runtime_feed::Verified,
    control: Arc<Control>,
    authority: impl Fn() -> Result<()>,
) -> Result<Health> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    control.phase(crate::update_control::Phase::Verifying, None)?;
    anyhow::ensure!(
        release.artifact().target == crate::update_feed::native_target()?,
        "cannot execute a foreign-architecture runtime"
    );
    authority()?;
    prepared.verify(release)?;
    let node = prepared.root().join("runtime/node/bin/node");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&node)?;
    let mut header = [0u8; 20];
    file.read_exact(&mut header)?;
    let machine = match release.artifact().target {
        crate::release_feed::Target::LinuxX86_64 => 62,
        crate::release_feed::Target::LinuxAarch64 => 183,
    };
    anyhow::ensure!(
        &header[..4] == b"\x7fELF"
            && header[4] == 2
            && header[5] == 1
            && u16::from_le_bytes([header[18], header[19]]) == machine,
        "candidate Node must be a native Linux ELF64 executable"
    );
    let parent = crate::config::state_dir().join("candidate-checks");
    crate::shell_apply::ensure_prefix(&parent)?;
    let temporary = parent.join(format!("check-{}", crate::downloads::unique_id()));
    let _guard = crate::checked_archive::NewDirectory::create(&temporary)?;
    let mut command = Command::new(&node);
    command
        .args([
            "--input-type=module",
            "-e",
            crate::bootstrap::health_script(),
            "--",
            "--owned-group",
            "--expected-components",
            &serde_json::to_string(&release.manifest().components)?,
        ])
        .arg(prepared.root());
    command
        .env_clear()
        .current_dir(&temporary)
        .env("HOME", &temporary)
        .env("DSH_HOME", temporary.join(".dsh"))
        .env("TMPDIR", &temporary)
        .env("LANG", "C.UTF-8")
        .env(
            "PATH",
            format!(
                "{}:/usr/bin:/bin",
                node.parent().expect("node directory").display()
            ),
        );
    for name in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ] {
        command.env(name, temporary.join(name));
    }
    run(&mut command, control.clone(), Duration::from_secs(95))?;
    control.check()?;
    authority()?;
    prepared.verify(release)?;
    Ok(Health {
        generation: prepared.root().into(),
        manifest_sha256: release.manifest_sha256().into(),
        components: release.manifest().components.clone(),
        health_checked: true,
        activated: false,
    })
}
struct Owned(std::process::Child);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    anyhow::ensure!(
        flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "could not configure candidate output"
    );
    Ok(())
}
fn drain(pipe: &mut impl std::io::Read, total: &mut usize) -> Result<()> {
    let mut bytes = [0u8; 8192];
    loop {
        match pipe.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(size) => {
                *total += size;
                anyhow::ensure!(*total <= 65536, "candidate health output exceeds limit");
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => anyhow::bail!("candidate output read failed"),
        }
    }
}
fn run(command: &mut Command, control: Arc<Control>, limit: Duration) -> Result<()> {
    use anyhow::Context;
    use std::os::unix::process::CommandExt;
    control.check()?;
    let mut process = Owned(
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .context("start isolated candidate checker")?,
    );
    let mut stdout = process
        .0
        .stdout
        .take()
        .context("candidate stdout unavailable")?;
    let mut stderr = process
        .0
        .stderr
        .take()
        .context("candidate stderr unavailable")?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let start = std::time::Instant::now();
    let mut total = 0;
    loop {
        control.check()?;
        anyhow::ensure!(start.elapsed() < limit, "candidate health timed out");
        drain(&mut stdout, &mut total)?;
        drain(&mut stderr, &mut total)?;
        if let Some(status) = process.0.try_wait().context("reap candidate checker")? {
            drain(&mut stdout, &mut total)?;
            drain(&mut stderr, &mut total)?;
            anyhow::ensure!(
                status.success(),
                "candidate native/version/authenticated health failed"
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_metadata_must_still_be_fresh_at_execution_boundary() {
        let now = chrono::Utc::now();
        let published = (now - chrono::Duration::minutes(1)).to_rfc3339();
        let expiry = (now + chrono::Duration::minutes(1)).to_rfc3339();
        fresh(&published, &expiry, now).unwrap();
        assert!(
            fresh(&published, &expiry, now + chrono::Duration::minutes(2)).is_err(),
            "a proof that expired while staging cannot authorize execution"
        );
        assert!(
            fresh(&published, &expiry, now - chrono::Duration::hours(1)).is_err(),
            "clock rollback cannot bypass publication window"
        );
    }
    #[test]
    fn runs_and_reaps_owned_child_with_bounded_output() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf complete; printf diagnostics >&2"]);
        run(
            &mut command,
            Arc::new(Control::default()),
            Duration::from_secs(2),
        )
        .unwrap();
    }
    #[test]
    fn timeout_and_output_overflow_fail_promptly() {
        for script in [
            "while :; do :; done",
            "while :; do printf 'oversized-output-line\\n'; done",
        ] {
            let start = std::time::Instant::now();
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            assert!(run(
                &mut command,
                Arc::new(Control::default()),
                Duration::from_millis(100)
            )
            .is_err());
            assert!(start.elapsed() < Duration::from_secs(3));
        }
    }
    #[test]
    fn cancellation_waits_for_process_group_cleanup() {
        let directory = std::env::temp_dir().join(format!(
            "ocean-health-cancel-{}",
            crate::downloads::unique_id()
        ));
        let _guard = crate::checked_archive::NewDirectory::create(&directory).unwrap();
        let pid_file = directory.join("pid");
        let worker_pid = pid_file.clone();
        let control = Arc::new(Control::default());
        let worker_control = control.clone();
        let thread = std::thread::spawn(move || {
            let mut command = Command::new("/bin/sh");
            command.env_clear().env("PID_FILE", worker_pid).args([
                "-c",
                "printf '%s' \"$$\" > \"$PID_FILE\"; exec /bin/sleep 60",
            ]);
            run(&mut command, worker_control, Duration::from_secs(5))
        });
        let start = std::time::Instant::now();
        while !pid_file.exists() && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(2));
        }
        let pid = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.parse::<i32>().ok());
        control.cancel();
        assert!(thread.join().unwrap().is_err());
        let pid = pid.expect("owned checker must have started before cancellation");
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "cancelled checker must be reaped, not left running or zombie"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}

//! Spawning and readiness supervision of a managed DSH web child.
//!
//! Extracted from the Tauri glue so the real process lifecycle (spawn, URL/token
//! discovery, silent-hang timeout, immediate exit, kill) can be exercised with
//! short local scripts instead of a live harness.
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::{timeout_at, Instant};
use url::Url;

/// Log sink for child output; must stay usable after `start` returns.
pub type Log = Arc<dyn Fn(String) + Send + Sync>;

/// Printed by the harness once its authenticated web server is listening.
///
/// The line is child output, so only a plain `http` loopback origin with a real port
/// and the root path is accepted: a compromised or buggy harness cannot redirect the
/// window (or the cached token) to a foreign origin.
pub fn parse_server_line(line: &str) -> Option<(Url, String)> {
    let tail = line.split("dsh web: ").nth(1)?;
    let raw = tail.split_whitespace().next()?;
    let url = Url::parse(raw).ok()?;
    let token = url
        .query_pairs()
        .find_map(|(key, value)| (key == "token").then(|| value.into_owned()))?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) | None => false,
    };
    if url.scheme() != "http"
        || !loopback
        || url.port().is_none_or(|port| port == 0)
        || url.path() != "/"
        || url.username() != ""
        || url.password().is_some()
    {
        return None;
    }
    let mut base = url;
    base.set_query(None);
    Some((base, token))
}

/// A managed child that has printed its authenticated URL.
#[derive(Debug)]
pub struct ManagedChild {
    pub child: Child,
    pub base_url: Url,
    pub token: String,
}

/// Spawns `node <dsh> <args…>` and waits for the authenticated URL line.
///
/// Every failure path kills the child before returning, so a caller never has to
/// clean up after an unsuccessful start.
pub async fn start(
    node: &Path,
    dsh: &Path,
    args: &[&str],
    path_env: &str,
    timeout: Duration,
    log: Log,
) -> Result<ManagedChild> {
    let mut command = Command::new(node);
    command
        .arg(dsh)
        .args(args)
        .env("PATH", path_env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Own process group: the harness starts shells and tools, and a failed start or
    // a quit must not leave them behind as orphans.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().context("spawn DSH web process")?;
    let stdout = child.stdout.take().context("capture DSH stdout")?;
    let stderr = child.stderr.take().context("capture DSH stderr")?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    spawn_log_reader(stdout, tx.clone(), log.clone());
    spawn_log_reader(stderr, tx, log);

    let deadline = Instant::now() + timeout;
    loop {
        let line = match timeout_at(deadline, rx.recv()).await {
            Ok(Some(line)) => line,
            Ok(None) => {
                return failed_start(
                    &mut child,
                    "the DSH process exited before it printed an authenticated URL",
                )
                .await;
            }
            Err(_) => {
                return failed_start(
                    &mut child,
                    "timed out waiting for DSH to print its authenticated URL",
                )
                .await;
            }
        };
        if let Some((base_url, token)) = parse_server_line(&line) {
            return Ok(ManagedChild {
                child,
                base_url,
                token,
            });
        }
    }
}

/// Kill the process group owned by a child returned by `start`, then reap its
/// leader within two seconds. Do not reap/probe the leader before signalling:
/// even a dead, unreaped leader reserves the PID used as its group's identity.
/// A previously reaped child has no safe group identity and is a no-op.
///
/// Success confirms signal delivery (or an absent group) and leader reaping,
/// not reaping of descendants, which belong to their parent or a subreaper.
/// Descendants that escape the owned process group cannot be stopped here.
/// On error the caller should retain the child rather than start a replacement.
pub async fn stop_child(child: &mut Child) -> Result<()> {
    let Some(pid) = child.id() else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        let pgid = libc::pid_t::try_from(pid).context("invalid managed child process group")?;
        // SAFETY: start creates this child's own group. Its unreaped leader
        // still reserves this identity; never cache it across a wait/try_wait.
        if unsafe { libc::kill(-pgid, libc::SIGKILL) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error).context("kill managed DSH process group");
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        child.start_kill().context("kill managed DSH process")?;
    }
    tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .context("timed out reaping managed DSH process")?
        .context("reap managed DSH process")?;
    Ok(())
}

/// Best-effort synchronous shutdown for an owned, unpublished/leased child.
/// Async restart paths use stop_child so they can report failures and reap.
pub(crate) fn kill_owned_child(mut child: Child) {
    #[cfg(unix)]
    if let Some(pgid) = child.id().and_then(|pid| libc::pid_t::try_from(pid).ok()) {
        // SAFETY: start owns this group; an unreaped child reserves its identity.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

async fn failed_start(child: &mut Child, message: &str) -> Result<ManagedChild> {
    if let Err(error) = stop_child(child).await {
        // Still attempt leader cleanup if group signalling failed; kill_on_drop
        // alone neither reports errors nor provides bounded reaping.
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        anyhow::bail!("{message}; cleanup failed: {error:#}");
    }
    anyhow::bail!("{message}")
}

fn spawn_log_reader<R>(reader: R, tx: mpsc::UnboundedSender<String>, log: Log)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            log(format!("[dsh] {line}"));
            if tx.send(line).is_err() {
                // Readiness was already decided; keep draining so the child
                // never blocks on a full pipe.
                continue;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_authenticated_url_and_strips_query() {
        let (base, token) =
            parse_server_line("dsh web: http://127.0.0.1:4567/?token=abc123 extra").unwrap();
        assert_eq!(base.as_str(), "http://127.0.0.1:4567/");
        assert_eq!(token, "abc123");
        assert!(base.query().is_none());
    }

    #[test]
    fn rejects_lines_without_a_token_or_url() {
        for line in [
            "dsh web: http://127.0.0.1:1/",
            "dsh web: not-a-url",
            "starting",
            "dsh web: ",
        ] {
            assert!(parse_server_line(line).is_none(), "{line}");
        }
    }

    #[test]
    fn rejects_a_foreign_origin_with_a_plausible_token() {
        // Child output is untrusted: anything but a loopback http origin must be
        // dropped, so the window and the token cache never follow it.
        for line in [
            "dsh web: http://example.invalid:4567/?token=abc123",
            "dsh web: https://127.0.0.1:4567/?token=abc123",
            "dsh web: http://10.0.0.5:4567/?token=abc123",
            "dsh web: http://user@127.0.0.1:4567/?token=abc123",
            "dsh web: file:///etc/passwd?token=abc123",
            "dsh web: http://127.0.0.1/?token=abc123",
            "dsh web: http://127.0.0.1:0/?token=abc123",
            "dsh web: http://127.0.0.1:4567/admin?token=abc123",
        ] {
            assert!(parse_server_line(line).is_none(), "{line}");
        }
        // A bracketed IPv6 loopback is accepted.
        let (base, token) = parse_server_line("dsh web: http://[::1]:4567/?token=abc123").unwrap();
        assert_eq!(base.as_str(), "http://[::1]:4567/");
        assert_eq!(token, "abc123");
    }
}

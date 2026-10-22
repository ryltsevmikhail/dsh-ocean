use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::process::Command;

use anyhow::Result;
use url::Url;

use crate::{config, http};

/// Extract the launch token from a `dsh web: http://…?token=…` log line.
pub fn parse_token_line(line: &str) -> Option<String> {
    let tail = line.split("dsh web: ").nth(1)?;
    let raw = tail.split_whitespace().next()?;
    let url = Url::parse(raw).ok()?;
    url.query_pairs()
        .find_map(|(key, value)| (key == "token").then(|| value.into_owned()))
}

/// All launch tokens found in the user journal, newest first.
///
/// `dsh web` prints its authenticated URL exactly once at startup. Reading it
/// back from journald is what lets the desktop client attach to a server it
/// did not spawn without restarting the server (and losing live sessions).
/// The journal can contain several starts, so callers must validate candidates
/// instead of trusting the first line.
pub fn tokens_from_journal(unit: Option<&str>) -> Vec<String> {
    let mut command = Command::new("journalctl");
    command.args(["--user", "-n", "5000", "--no-pager", "-o", "cat"]);
    if let Some(unit) = unit {
        command.args(["-u", unit]);
    }
    let output = match command.output() {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .rev()
        .filter_map(parse_token_line)
        .filter(|token| seen.insert(token.clone()))
        .collect()
}

fn token_cache_path() -> PathBuf {
    config::state_dir().join("web-token")
}

/// Upper bound for the cached token; a launch token is far shorter.
const MAX_TOKEN_BYTES: u64 = 4096;

pub fn cached_token() -> Option<String> {
    // No symlinks, one link, bounded read: a replaced or over-long file must not
    // feed the startup path an unbounded string or a device. Nonblocking open
    // lets the descriptor type check reject a FIFO even when it has no writer.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(token_cache_path())
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file()
        || metadata.len() > MAX_TOKEN_BYTES
        || std::os::unix::fs::MetadataExt::nlink(&metadata) != 1
    {
        return None;
    }
    let mut raw = Vec::new();
    (&file)
        .take(MAX_TOKEN_BYTES + 1)
        .read_to_end(&mut raw)
        .ok()?;
    if raw.len() as u64 > MAX_TOKEN_BYTES {
        return None;
    }
    let token = String::from_utf8(raw).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

pub fn cache_token(token: &str) {
    if token.is_empty() || token.len() > MAX_TOKEN_BYTES as usize {
        return;
    }
    let path = token_cache_path();
    let Some(parent) = path.parent() else {
        return;
    };
    // 0700 (this runs first on a fresh profile) and a private atomic replacement:
    // a symlink at the target is never followed, and the token is never briefly
    // world-readable.
    if fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .is_err()
    {
        return;
    }
    let temp = parent.join(format!(".web-token-{}.tmp", crate::downloads::unique_id()));
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        file.write_all(token.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, &path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
}

pub fn token_url(base: &Url, token: &str) -> Url {
    let mut url = base.clone();
    url.query_pairs_mut().clear().append_pair("token", token);
    url
}

/// Verify a launch token by performing the index exchange without following
/// the redirect. A valid token produces `303 See Other` and a session cookie.
pub fn validate(base: &Url, token: &str) -> bool {
    let url = token_url(base, token);
    http::status(&url) == Some(303)
}

pub fn redact(text: &str) -> String {
    let re = regex::Regex::new(r"(?i)(token=)[A-Za-z0-9._~%+-]+").expect("static redaction regex");
    re.replace_all(text, "${1}***").into_owned()
}

pub fn resolve_token(base: &Url, unit: Option<&str>) -> Result<String> {
    let mut candidates = Vec::new();
    candidates.extend(tokens_from_journal(unit));
    candidates.extend(tokens_from_journal(None));
    if let Some(token) = cached_token() {
        candidates.push(token);
    }

    let mut seen = std::collections::HashSet::new();
    for token in candidates {
        if !seen.insert(token.clone()) {
            continue;
        }
        if validate(base, &token) {
            cache_token(&token);
            return Ok(token);
        }
    }

    anyhow::bail!(
        "No valid DSH launch token was found. The server may have restarted; restart it so it prints a fresh authenticated URL."
    )
}

/// Await a newly started owned systemd service, not merely a listening port.
/// This intentionally does not use the attachment resolver's all-unit journal
/// search: only the fixed owned unit and the bounded token cache are candidates.
/// All retries and HTTP attempts share a deadline. Stock `journalctl --no-pager`
/// is a single-process helper: it is reaped on exit; timeout/oversize paths kill
/// its owned group before reaping. This is not supervision of substituted
/// wrappers that exit after detaching their own workers.
pub(crate) async fn wait_for_systemd(
    base: &Url,
    unit: &str,
    timeout: std::time::Duration,
) -> Result<String> {
    use std::time::{Duration, Instant};
    anyhow::ensure!(
        base.scheme() == "http"
            && base.host_str() == Some("127.0.0.1")
            && unit == config::UNIT_NAME,
        "authenticated readiness requires the owned loopback systemd unit"
    );
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            anyhow::bail!("authenticated readiness timed out: no valid DSH launch token and anonymous gate became ready");
        }
        if http::status_until(base, deadline) == Some(401) {
            let mut command = tokio::process::Command::new("journalctl");
            command.args([
                "--user",
                "-u",
                unit,
                "-n",
                "5000",
                "--no-pager",
                "-o",
                "cat",
            ]);
            let bytes = journal_output_until(
                &mut command,
                deadline.min(Instant::now() + Duration::from_secs(2)),
            )
            .await?;
            let text = String::from_utf8_lossy(&bytes);
            let mut seen = std::collections::HashSet::new();
            let candidates = text
                .lines()
                .rev()
                .filter_map(parse_token_line)
                .filter(|token| !token.is_empty() && token.len() <= MAX_TOKEN_BYTES as usize)
                .filter(|token| seen.insert(token.clone()))
                .take(64)
                .chain(cached_token());
            for token in candidates {
                if Instant::now() >= deadline {
                    break;
                }
                if http::status_until(&token_url(base, &token), deadline) == Some(303)
                    && http::status_until(base, deadline) == Some(401)
                {
                    cache_token(&token);
                    return Ok(token);
                }
            }
        }
        tokio::time::sleep(
            Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
        )
        .await;
    }
}

async fn journal_output_until(
    command: &mut tokio::process::Command,
    deadline: std::time::Instant,
) -> Result<Vec<u8>> {
    use anyhow::Context;
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    const MAX_JOURNAL_BYTES: u64 = 1024 * 1024;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .context("run owned-unit journal query")?;
    let stdout = child.stdout.take().context("capture owned-unit journal")?;
    let result = tokio::time::timeout_at(deadline.into(), async {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_JOURNAL_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .context("read owned-unit journal")?;
        anyhow::ensure!(
            bytes.len() <= MAX_JOURNAL_BYTES as usize,
            "owned-unit journal exceeds readiness limit"
        );
        let status = child
            .wait()
            .await
            .context("reap owned-unit journal query")?;
        anyhow::ensure!(status.success(), "owned-unit journal query failed");
        Ok(bytes)
    })
    .await
    .unwrap_or_else(|_| {
        Err(anyhow::anyhow!(
            "owned-unit journal query exceeded readiness deadline"
        ))
    });
    if result.is_err() {
        crate::managed_child::stop_child(&mut child)
            .await
            .context("could not clean up owned-unit journal query")?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn journal_deadline_kills_and_reaps_the_owned_query_without_echoing_output() {
        use std::os::unix::fs::DirBuilderExt;
        use std::time::{Duration, Instant};
        let root = std::env::temp_dir().join(format!(
            "ocean-journal-bound-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let pid_path = root.join("pid");
        let mut command = tokio::process::Command::new("/bin/bash");
        command
            .args([
                "-c",
                "printf '%s' \"$$\" > \"$1\"; printf 'token=fixture-secret\\n'; exec sleep 30",
                "fixture",
            ])
            .arg(&pid_path);
        let start = Instant::now();
        let error = journal_output_until(&mut command, start + Duration::from_millis(200))
            .await
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("deadline"), "{message}");
        assert!(!message.contains("fixture-secret"));
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "helper must not inherit its30s sleep"
        );
        let pid: i32 = fs::read_to_string(&pid_path).unwrap().parse().unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "owned journal leader was killed and reaped"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn oversized_or_failed_journal_is_bounded_and_never_used_as_a_diagnostic() {
        use std::time::{Duration, Instant};
        for (script, expected) in [
            ("head -c 1100000 /dev/zero; exec sleep 30", "limit"),
            (
                "printf 'token=fixture-secret'; printf 'token=fixture-secret' >&2; exit 1",
                "query failed",
            ),
        ] {
            let mut command = tokio::process::Command::new("/bin/bash");
            command.args(["-c", script]);
            let start = Instant::now();
            let error = journal_output_until(&mut command, start + Duration::from_secs(2))
                .await
                .unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains("fixture-secret"));
            assert!(start.elapsed() < Duration::from_secs(4));
        }
    }

    #[test]
    fn parses_launch_token_from_journal_line() {
        let line = "dsh web: http://127.0.0.1:3080/?token=abc_DEF-123";
        assert_eq!(parse_token_line(line), Some("abc_DEF-123".to_string()));
    }

    #[test]
    fn ignores_lines_without_a_launch_url() {
        assert_eq!(parse_token_line("dsh web: starting"), None);
        assert_eq!(parse_token_line("unrelated"), None);
    }

    #[test]
    fn redacts_tokens_in_diagnostics() {
        let text = "dsh web: http://127.0.0.1:3080/?token=secret";
        assert_eq!(redact(text), "dsh web: http://127.0.0.1:3080/?token=***");
    }
}

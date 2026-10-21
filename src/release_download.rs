//! HTTPS download of an authenticated artifact into existing verified staging.
//! curl is trusted system tooling, not a sandbox for arbitrary PATH wrappers.
use crate::release_feed::{self, Staged, Verified};
use anyhow::{Context, Result};
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};

struct CurlSource {
    child: Child,
    stdout: ChildStdout,
    finished: bool,
}

impl Read for CurlSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.finished {
            return Ok(0);
        }
        let count = self.stdout.read(buffer)?;
        if count == 0 {
            let status = self.child.wait()?;
            self.finished = true;
            // A complete, matching body is not enough: curl must also report
            // success before stage_reader can publish the verified copy.
            if !status.success() {
                return Err(io::Error::other("HTTPS release download failed"));
            }
        }
        Ok(count)
    }
}

impl Drop for CurlSource {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn source(url: &str, limit: u64) -> Result<CurlSource> {
    let mut child = Command::new("curl")
        // Must be the first argument: ignore user/system curl configuration.
        .args(["--disable", "--fail", "--silent", "--location"])
        .args(["--proto", "=https", "--proto-redir", "=https"])
        .args(["--max-redirs", "5", "--connect-timeout", "15"])
        .args([
            "--max-time",
            "180",
            "--speed-limit",
            "1024",
            "--speed-time",
            "30",
        ])
        .arg("--max-filesize")
        .arg(limit.to_string())
        .arg("--url")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Never publish curl's stderr (URLs, proxy data, paths or credentials).
        .stderr(Stdio::null())
        .spawn()
        .context("start trusted curl downloader")?;
    let stdout = child.stdout.take().expect("piped curl stdout");
    Ok(CurlSource {
        child,
        stdout,
        finished: false,
    })
}

/// No key discovery, execution, acceptance or installation. Caller authenticates
/// metadata and enforces its floor; URL comes only from Verified's artifact policy.
pub fn download(release: &Verified, destination: &Path) -> Result<Staged> {
    let mut source = source(&release.artifact().url, release.artifact().size)?;
    release_feed::stage_reader(&mut source, destination, release)
}

/// Bounded untrusted metadata, never authority until the independent signature
/// verifier accepts it. No disk publication, key retrieval or persistent state.
pub(crate) fn fetch_metadata(url: &str, limit: u64) -> Result<Vec<u8>> {
    crate::update_settings::validate_feed_url(url)?;
    anyhow::ensure!(
        limit > 0 && limit <= release_feed::MAX_MANIFEST,
        "invalid update metadata bound"
    );
    let mut source = source(url, limit)?;
    let mut bytes = Vec::new();
    (&mut source)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .context("read update metadata")?;
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() as u64 <= limit,
        "update metadata size refused"
    );
    Ok(bytes)
}

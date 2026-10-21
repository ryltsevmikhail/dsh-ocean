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

/// No key discovery, manifest download, execution, acceptance or installation.
/// The caller must authenticate the manifest and enforce its remembered floor.
/// The selected URL comes only from Verified's canonical GitHub artifact policy.
pub fn download(release: &Verified, destination: &Path) -> Result<Staged> {
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
        .arg(release.artifact().size.to_string())
        .arg("--url")
        .arg(&release.artifact().url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Never publish curl's stderr (URLs, proxy data, paths or credentials).
        .stderr(Stdio::null())
        .spawn()
        .context("start trusted curl downloader")?;
    let stdout = child.stdout.take().expect("piped curl stdout");
    let mut source = CurlSource {
        child,
        stdout,
        finished: false,
    };
    release_feed::stage_reader(&mut source, destination, release)
}

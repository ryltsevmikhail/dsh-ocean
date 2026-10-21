//! HTTPS download of an authenticated artifact into existing verified staging.
//! curl is trusted system tooling, not a sandbox for arbitrary PATH wrappers.
use crate::release_feed::{self, Staged, Verified};
use crate::update_control::Control;
use anyhow::{Context, Result};
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

struct CurlSource {
    child: Arc<Mutex<Child>>,
    stdout: ChildStdout,
    finished: bool,
    control: Arc<Control>,
    received: u64,
}

impl Read for CurlSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.finished {
            return Ok(0);
        }
        self.control.check().map_err(io::Error::other)?;
        let count = self.stdout.read(buffer)?;
        self.control.check().map_err(io::Error::other)?;
        self.received = self
            .received
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("download byte count overflow"))?;
        self.control
            .observe(self.received)
            .map_err(io::Error::other)?;
        if count == 0 {
            let total = self.control.snapshot().total;
            self.control
                .phase(crate::update_control::Phase::Verifying, total)
                .map_err(io::Error::other)?;
            self.control
                .observe(self.received)
                .map_err(io::Error::other)?;
            let status = loop {
                self.control.check().map_err(io::Error::other)?;
                // Never hold the child mutex across wait(): native cancellation
                // must be able to kill a process even after it closed stdout.
                if let Some(status) = self.child.lock().expect("update child").try_wait()? {
                    break status;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            self.finished = true;
            self.control.detach(&self.child);
            self.control.check().map_err(io::Error::other)?;
            // Matching bytes alone are insufficient: successful exit is required.
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
            let mut child = self.child.lock().expect("update child");
            let _ = child.kill();
            let _ = child.wait();
        }
        self.control.detach(&self.child);
    }
}

fn source(url: &str, limit: u64, control: Arc<Control>) -> Result<CurlSource> {
    control.check()?;
    let child = Command::new("curl")
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
    from_child(child, control)
}
fn from_child(mut child: Child, control: Arc<Control>) -> Result<CurlSource> {
    let stdout = child.stdout.take().expect("piped curl stdout");
    let source = CurlSource {
        child: Arc::new(Mutex::new(child)),
        stdout,
        finished: false,
        control,
        received: 0,
    };
    // On a spawn/cancel race Source's Drop kills/reaps the newly owned process.
    source.control.attach(source.child.clone())?;
    Ok(source)
}

/// No key discovery, execution, acceptance or installation. Caller authenticates
/// metadata and enforces its floor; URL comes only from Verified's artifact policy.
pub fn download(release: &Verified, destination: &Path) -> Result<Staged> {
    download_controlled(release, destination, Arc::new(Control::default()))
}
pub fn download_controlled(
    release: &Verified,
    destination: &Path,
    control: Arc<Control>,
) -> Result<Staged> {
    control.phase(
        crate::update_control::Phase::Downloading,
        Some(release.artifact().size),
    )?;
    let mut source = source(
        &release.artifact().url,
        release.artifact().size,
        control.clone(),
    )?;
    release_feed::stage_reader_guarded(
        &mut source,
        destination,
        release,
        |bytes| control.observe(bytes),
        || control.begin_commit(),
    )
}
/// Separate public product proof; a shell signature cannot authorize this API.
/// Publication completes this transfer control; preparation/apply uses a new operation.
pub fn download_runtime_controlled(
    release: &crate::runtime_feed::Verified,
    destination: &Path,
    control: Arc<Control>,
) -> Result<Staged> {
    download_artifact(&release.artifact().artifact(), destination, control)
}
fn download_artifact(
    artifact: &release_feed::Artifact,
    destination: &Path,
    control: Arc<Control>,
) -> Result<Staged> {
    control.phase(
        crate::update_control::Phase::Downloading,
        Some(artifact.size),
    )?;
    let mut source = source(&artifact.url, artifact.size, control.clone())?;
    release_feed::stage_artifact_guarded(
        &mut source,
        destination,
        artifact,
        |bytes| control.observe(bytes),
        || control.begin_commit(),
    )
}

/// Crate-private preparatory transfer for the owned apply workflow. It shares
/// the terminal downloader's controlled `CurlSource` and authenticated
/// `Artifact` guard, but the publication boundary is `control.check()` (still
/// cancellable) instead of `control.begin_commit()`: the bytes are staged into
/// a private cache directory that is never externally selected, and the caller
/// still runs `control.check()`/`control.complete()` around the whole operation.
/// A late cancellation after the cache is published leaves the private RAII
/// cache to be removed, and a `check`/`complete` failure prevents the result
/// from ever being reported as available. The public CLI terminal download
/// (`download_runtime_controlled`) is unchanged.
pub(crate) fn download_runtime_preparatory(
    release: &crate::runtime_feed::Verified,
    destination: &Path,
    control: Arc<Control>,
) -> Result<Staged> {
    let artifact = release.artifact().artifact();
    control.phase(
        crate::update_control::Phase::Downloading,
        Some(artifact.size),
    )?;
    let mut source = source(&artifact.url, artifact.size, control.clone())?;
    release_feed::stage_artifact_guarded(
        &mut source,
        destination,
        &artifact,
        |bytes| control.observe(bytes),
        || control.check(),
    )
}

/// Bounded untrusted metadata, never authority until the independent signature
/// verifier accepts it. No disk publication, key retrieval or persistent state.
pub(crate) fn fetch_metadata_controlled(
    url: &str,
    limit: u64,
    control: Arc<Control>,
) -> Result<Vec<u8>> {
    crate::update_settings::validate_feed_url(url)?;
    anyhow::ensure!(
        limit > 0 && limit <= release_feed::MAX_MANIFEST,
        "invalid update metadata bound"
    );
    control.phase(crate::update_control::Phase::Checking, None)?;
    let mut source = source(url, limit, control.clone())?;
    let mut bytes = Vec::new();
    (&mut source)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .context("read update metadata")?;
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() as u64 <= limit,
        "update metadata size refused"
    );
    control.check()?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_remains_prompt_after_the_child_closed_stdout() {
        let control = Arc::new(Control::default());
        let child = Command::new("/bin/sh")
            .args(["-c", "exec 1>&-; exec /bin/sleep 3"])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut source = from_child(child, control.clone()).unwrap();
        let worker = std::thread::spawn(move || {
            let mut byte = [0u8];
            let failed = source.read(&mut byte).is_err();
            drop(source);
            failed
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while control.snapshot().phase != crate::update_control::Phase::Verifying
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let waiting = control.snapshot().phase == crate::update_control::Phase::Verifying;
        let start = std::time::Instant::now();
        assert!(control.cancel());
        let elapsed = start.elapsed();
        let failed = worker.join().unwrap();
        assert!(waiting, "worker must reach the transport-exit wait");
        assert!(failed);
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "child wait must not hold the cancellation mutex"
        );
    }
    #[test]
    fn cancelling_a_stalled_transfer_kills_and_reaps_only_its_owned_child() {
        let control = Arc::new(Control::default());
        let child = Command::new("/bin/sh")
            .args(["-c", "printf x; exec /bin/sleep 30"])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut source = from_child(child, control.clone()).unwrap();
        let owned = source.child.clone();
        let pid = owned.lock().unwrap().id();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut byte = [0u8];
            assert_eq!(source.read(&mut byte).unwrap(), 1);
            ready_tx.send(()).unwrap();
            let failed = source.read(&mut byte).is_err();
            drop(source);
            let _ = done_tx.send(failed);
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(control.cancel());
        let done = done_rx
            .recv_timeout(std::time::Duration::from_millis(800))
            .ok();
        if done.is_none() {
            let _ = owned.lock().unwrap().kill();
        }
        worker.join().unwrap();
        assert_eq!(
            done,
            Some(true),
            "cancellation must wake the blocked reader promptly"
        );
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}

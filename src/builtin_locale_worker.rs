//! Bounded metadata subprocess. Caller establishes owned admission, profile
//! consent and verified immutable script identity before invoking this helper.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub install_anchor: PathBuf,
    pub profile_dir: PathBuf,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub telemetry_disabled_env: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Absent,
    Enabled,
    Disabled,
    Unknown,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    state: Presence,
}
fn absolute(path: &Path) -> bool {
    path.is_absolute() && !path.components().any(|c| matches!(c, Component::ParentDir))
}
pub async fn inspect(
    node: &Path,
    script: &Path,
    request: &Request,
    process_home: &Path,
) -> Result<Presence> {
    anyhow::ensure!(
        [
            node,
            script,
            process_home,
            &request.install_anchor,
            &request.profile_dir,
            &request.home,
            &request.cwd
        ]
        .into_iter()
        .all(absolute),
        "absolute metadata-worker scope required"
    );
    let payload = serde_json::to_string(request)?;
    anyhow::ensure!(
        payload.len() <= 16384,
        "metadata-worker scope exceeds limit"
    );
    let mut child = Command::new(node)
        .arg("--experimental-import-meta-resolve")
        .arg(script)
        .arg(payload)
        .env_clear()
        .env("HOME", process_home)
        .env("DSH_HOME", &request.home)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&request.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("start native locale metadata worker")?;
    let stdout = child.stdout.take().context("metadata-worker stdout")?;
    let stderr = child.stderr.take().context("metadata-worker stderr")?;
    let mut stdout = stdout.take(8193);
    let mut stderr = stderr.take(8193);
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut output),
            stderr.read_to_end(&mut errors)
        )?;
        anyhow::ensure!(
            output.len() <= 8192 && errors.len() <= 8192,
            "metadata-worker output exceeds limit"
        );
        anyhow::ensure!(status.success(), "native locale metadata worker failed");
        let response: Response = serde_json::from_slice(&output)
            .map_err(|_| anyhow::anyhow!("invalid native locale metadata response"))?;
        Ok::<_, anyhow::Error>(response.state)
    })
    .await;
    match result {
        Ok(Ok(presence)) => Ok(presence),
        Ok(Err(error)) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            anyhow::bail!("native locale metadata worker timed out")
        }
    }
}

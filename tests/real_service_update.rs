//! Opt-in network/systemd test. Uses only runtime-linked test units, private
//! HOME/XDG directories and OS-assigned ports. Never stops dsh-web.service.
#![cfg(target_os = "linux")]
use std::cell::{Cell, RefCell};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use dsh_ocean::{auth, bootstrap, runtime_update, systemd};
use url::Url;

const OLD: &str = "0.2.0-rc.2";
const NEW: &str = "0.2.1-rc.2";
fn control(args: &[&str]) -> std::process::Output {
    Command::new("/usr/bin/systemctl")
        .arg("--user")
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .expect("user manager command")
}
fn property(unit: &str, key: &str) -> Option<String> {
    let result = control(&["show", unit, &format!("--property={key}"), "--value"]);
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).trim().to_owned())
}
fn production_pid() -> Option<String> {
    property("dsh-web.service", "MainPID")
}

struct Fixture {
    root: PathBuf,
    prefix: PathBuf,
    home: PathBuf,
    unit: String,
    unit_path: PathBuf,
    linked: Cell<bool>,
    workers: RefCell<Vec<String>>,
    stamp: String,
}
impl Fixture {
    fn new() -> Result<Self> {
        let stamp = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        let root = std::env::temp_dir().join(format!("dsh-ocean-real-update-{stamp}"));
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        let prefix = root.join("runtime prefix");
        let home = prefix.join("service home%literal");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(home.join(".run"))?;
        let unit = format!("dsh-ocean-test-real-{stamp}.service");
        assert!(systemd::test_unit_name(&unit));
        let unit_path = root.join(&unit);
        Ok(Self {
            root,
            prefix,
            home,
            unit,
            unit_path,
            linked: Cell::new(false),
            workers: RefCell::new(Vec::new()),
            stamp,
        })
    }
    fn track(&self, name: &str) {
        assert!(
            systemd::test_unit_name(name)
                || name.starts_with(&format!("dsh-ocean-update-{}-", std::process::id()))
        );
        self.workers.borrow_mut().push(name.to_owned());
    }
    fn bootstrap(&self) -> Result<()> {
        let manifest = bootstrap::manifest()?;
        let arch = match std::env::consts::ARCH {
            "x86_64" => "linux-x64",
            "aarch64" => "linux-arm64",
            _ => anyhow::bail!("unsupported test arch"),
        };
        let sha = &manifest
            .node
            .archives
            .get(arch)
            .context("Node checksum")?
            .sha256;
        let name = format!("dsh-ocean-test-bootstrap-{}.service", self.stamp);
        self.track(&name);
        let log = self.root.join("bootstrap.log");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&log)?;
        let environment = [
            format!("HOME={}", self.home.display()),
            "PATH=/usr/bin:/bin".into(),
            format!("DSH_HOME={}", self.home.join(".dsh").display()),
            format!("DSH_OCEAN_DATA_DIR={}", self.prefix.display()),
            format!("DSH_NODE_VERSION={}", manifest.node.version),
            format!("DSH_NODE_SHA256={sha}"),
            format!("DSH_PNPM_VERSION={}", manifest.pnpm.version),
            format!("DSH_VERSION={OLD}"),
            format!("DSH_OCEAN_HEALTH_JS={}", bootstrap::health_script()),
        ];
        let status = Command::new("/usr/bin/systemd-run")
            .args([
                "--user",
                "--wait",
                "--collect",
                "--quiet",
                "--expand-environment=no",
            ])
            .arg(format!("--unit={name}"))
            .args([
                "--property=RuntimeMaxSec=15min",
                "--property=TimeoutStopSec=90s",
            ])
            .arg(format!(
                "--property=StandardOutput=append:{}",
                log.display()
            ))
            .arg(format!("--property=StandardError=append:{}", log.display()))
            .args(["/usr/bin/env", "-i"])
            .args(environment)
            .args(["/bin/bash", "-c", bootstrap::install_script()])
            .stdin(Stdio::null())
            .status()?;
        anyhow::ensure!(
            status.success(),
            "bootstrap failed: {}",
            runtime_update::log_tail(&log)
        );
        Ok(())
    }
    fn selected(&self, expected: &str) -> Result<bootstrap::Runtime> {
        let root = bootstrap::selected_private_root(&self.prefix)?;
        let runtime = bootstrap::Runtime {
            node: root.join("runtime/node/bin/node"),
            dsh: root.join("harness/node_modules/.bin/dsh"),
            install_root: Some(root.join("harness")),
            version: expected.into(),
        };
        let version = Command::new(&runtime.node)
            .arg(&runtime.dsh)
            .arg("--version")
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", systemd::isolated_path_env(&runtime))
            .output()?;
        anyhow::ensure!(
            version.status.success() && String::from_utf8_lossy(&version.stdout).trim() == expected,
            "selected runtime version mismatch"
        );
        Ok(runtime)
    }
    fn link_and_start(&self, runtime: &bootstrap::Runtime) -> Result<Url> {
        let initial = Url::parse("http://127.0.0.1:0/")?;
        let content = systemd::render_isolated_unit(runtime, &initial, &self.home)?;
        anyhow::ensure!(
            content.contains("ExecStart=/usr/bin/env -i") && content.contains("Restart=no"),
            "unsafe test unit template"
        );
        fs::write(&self.unit_path, content)?;
        fs::set_permissions(&self.unit_path, fs::Permissions::from_mode(0o600))?;
        let validation = Command::new("/usr/bin/systemd-analyze")
            .args(["--user", "verify"])
            .arg(&self.unit_path)
            .output()?;
        anyhow::ensure!(
            validation.status.success(),
            "invalid isolated unit: {}",
            auth::redact(&String::from_utf8_lossy(&validation.stderr))
        );
        self.linked.set(true); // Cleanup remains armed if linking/reloading fails.
        anyhow::ensure!(
            control(&["link", "--runtime", self.unit_path.to_str().unwrap()])
                .status
                .success(),
            "link private test unit"
        );
        anyhow::ensure!(
            control(&["daemon-reload"]).status.success(),
            "reload test unit"
        );
        anyhow::ensure!(
            control(&["start", &self.unit]).status.success(),
            "start isolated test service"
        );
        let base = self.authenticated_origin()?;
        anyhow::ensure!(
            base.port_or_known_default() != Some(3080),
            "test must never use port 3080"
        );
        // Pin the OS-assigned port for subsequent restarts. Reload does not restart
        // the running old process or alter its invocation/auth token.
        fs::write(
            &self.unit_path,
            systemd::render_isolated_unit(runtime, &base, &self.home)?,
        )?;
        anyhow::ensure!(
            control(&["daemon-reload"]).status.success(),
            "pin test service port"
        );
        self.assert_private_environment()?;
        Ok(base)
    }
    fn assert_private_environment(&self) -> Result<()> {
        let pid = property(&self.unit, "MainPID").context("isolated service PID")?;
        anyhow::ensure!(
            !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) && pid != "0",
            "invalid isolated PID"
        );
        // Read only this test's process; never dump its environment to diagnostics.
        let raw = fs::read(format!("/proc/{pid}/environ"))?;
        for (key, value) in [
            ("HOME", self.home.clone()),
            ("DSH_HOME", self.home.join(".dsh")),
            ("XDG_RUNTIME_DIR", self.home.join(".run")),
        ] {
            let expected = format!("{key}={}", value.display());
            anyhow::ensure!(
                raw.split(|b| *b == 0)
                    .any(|item| item == expected.as_bytes()),
                "isolated service directory mismatch for {key}"
            );
        }
        Ok(())
    }
    fn authenticated_origin(&self) -> Result<Url> {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Some(invocation) = property(&self.unit, "InvocationID")
                .filter(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                let output = Command::new("/usr/bin/journalctl")
                    .args([
                        "--user",
                        &format!("--user-unit={}", self.unit),
                        &format!("_SYSTEMD_INVOCATION_ID={invocation}"),
                        "--grep=dsh web:",
                        "-n",
                        "20",
                        "--no-pager",
                        "-o",
                        "cat",
                    ])
                    .output()?;
                if output.status.success() {
                    for line in String::from_utf8_lossy(&output.stdout).lines().rev() {
                        let Some(token) = auth::parse_token_line(line) else {
                            continue;
                        };
                        let Some(raw) = line
                            .split("dsh web: ")
                            .nth(1)
                            .and_then(|s| s.split_whitespace().next())
                        else {
                            continue;
                        };
                        let Ok(mut base) = Url::parse(raw) else {
                            continue;
                        };
                        if base.host_str() != Some("127.0.0.1") || base.scheme() != "http" {
                            continue;
                        }
                        base.set_query(None);
                        base.set_fragment(None);
                        if base.port_or_known_default() == Some(3080) {
                            anyhow::bail!("unsafe test port");
                        }
                        if dsh_ocean::http::status(&base) == Some(401)
                            && auth::validate(&base, &token)
                        {
                            return Ok(base);
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        anyhow::bail!(
            "isolated service did not pass authenticated health; raw journal/token omitted"
        )
    }
    fn update(
        &self,
        runtime: &bootstrap::Runtime,
        target: &str,
        base: &Url,
        reject_candidate: bool,
    ) -> Result<bool> {
        let task = runtime_update::prepare_isolated_at(
            runtime,
            target,
            base,
            &self.prefix,
            &self.unit_path,
            &self.root.join("logs"),
            &self.home,
        )?;
        self.track(&format!("{}.service", task.unit));
        let helper = if reject_candidate {
            let old = bootstrap::selected_private_root(&self.prefix)?;
            format!("const rejectCandidateForTest = process.argv.at(-1) !== {};\n{}\nif (rejectCandidateForTest && process.exitCode !== 1) {{ console.error('[test] injected candidate health rejection'); process.exitCode = 1; }}", serde_json::to_string(&old.display().to_string())?, include_str!("../scripts/check-service.mjs"))
        } else {
            include_str!("../scripts/check-service.mjs").to_owned()
        };
        let mut args = Vec::new();
        for arg in task.command.as_std().get_args() {
            let arg = arg.to_str().context("UTF-8 test command")?;
            if arg.starts_with("--setenv=DSH_OCEAN_SERVICE_HEALTH_JS=") {
                args.push(format!("--setenv=DSH_OCEAN_SERVICE_HEALTH_JS={helper}"));
            } else if arg.starts_with("DSH_OCEAN_SERVICE_HEALTH_JS=") {
                args.push(format!("DSH_OCEAN_SERVICE_HEALTH_JS={helper}"));
            } else {
                args.push(arg.to_owned());
            }
        }
        let result = Command::new("/usr/bin/systemd-run")
            .args(args)
            .stdin(Stdio::null())
            .output()?;
        let tail = runtime_update::log_tail(&task.log);
        if reject_candidate {
            anyhow::ensure!(
                !result.status.success()
                    && tail.contains("rollback complete")
                    && tail.contains("[test] injected candidate health rejection")
                    && !tail.contains("rollback incomplete"),
                "expected live rollback: {tail}"
            );
        } else {
            anyhow::ensure!(
                result.status.success() && tail.contains("update committed"),
                "expected successful live activation: {tail}"
            );
        }
        Ok(result.status.success())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(systemd::test_unit_name(&self.unit));
        let mut safe = true;
        for unit in self.workers.borrow().iter().rev() {
            let stopped = control(&["stop", unit]);
            if !stopped.status.success() {
                let error = String::from_utf8_lossy(&stopped.stderr);
                safe &= error.contains("not loaded") || error.contains("not found");
            }
        }
        if self.linked.get() {
            let stopped = control(&["stop", &self.unit]).status.success();
            safe &= stopped
                || (matches!(
                    property(&self.unit, "ActiveState").as_deref(),
                    Some("inactive" | "failed")
                ) && property(&self.unit, "MainPID").as_deref() == Some("0"));
            safe &= control(&["disable", "--runtime", &self.unit])
                .status
                .success();
            safe &= control(&["daemon-reload"]).status.success();
        }
        if safe {
            assert!(
                self.root.parent() == Some(std::env::temp_dir().as_path())
                    && self
                        .root
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("dsh-ocean-real-update-")
            );
            let _ = fs::remove_dir_all(&self.root);
        } else {
            eprintln!(
                "test cleanup could not confirm stopped units; private fixture retained at {}",
                self.root.display()
            );
        }
    }
}

#[test]
#[ignore = "downloads two DSH versions and requires a real user systemd manager"]
fn real_versions_activate_and_roll_back_on_an_isolated_service() -> Result<()> {
    anyhow::ensure!(systemd::available(), "a user systemd manager is required");
    let protected = production_pid();
    let fixture = Fixture::new()?;
    fixture.bootstrap()?;
    let old = fixture.selected(OLD)?;
    let base = fixture.link_and_start(&old)?;
    let old_invocation = property(&fixture.unit, "InvocationID").context("old invocation")?;
    let marker = fixture.home.join(".dsh/ocean-isolation-marker");
    fs::write(&marker, "isolated test data only\n")?;
    anyhow::ensure!(
        fixture.update(&old, NEW, &base, false)?,
        "update did not commit"
    );
    let updated = fixture.selected(NEW)?;
    let new_root = bootstrap::selected_private_root(&fixture.prefix)?;
    let new_unit = fs::read(&fixture.unit_path)?;
    anyhow::ensure!(
        property(&fixture.unit, "InvocationID").as_ref() != Some(&old_invocation),
        "service did not restart"
    );
    anyhow::ensure!(
        fixture.authenticated_origin()? == base,
        "new version changed test origin"
    );
    fixture.assert_private_environment()?;
    // An actual older-version candidate forces a fresh installation; reject only
    // its live health after restart, then prove the real NEW service is restored.
    anyhow::ensure!(
        !fixture.update(&updated, OLD, &base, true)?,
        "fault-injected candidate committed"
    );
    fixture.selected(NEW)?;
    anyhow::ensure!(
        bootstrap::selected_private_root(&fixture.prefix)? == new_root
            && fs::read(&fixture.unit_path)? == new_unit,
        "rollback did not restore selector and exact unit"
    );
    anyhow::ensure!(
        fixture.authenticated_origin()? == base,
        "restored service failed auth"
    );
    fixture.assert_private_environment()?;
    anyhow::ensure!(
        fs::read_to_string(marker)? == "isolated test data only\n",
        "isolated marker changed"
    );
    anyhow::ensure!(
        production_pid() == protected,
        "production service PID changed during isolated test"
    );
    println!("real {OLD} -> {NEW} activation and live rollback passed; production PID unchanged");
    Ok(())
}

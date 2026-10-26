//! Operator maintenance entry points for an installed runtime.
//!
//! These are thin wrappers around the embedded `bootstrap-runtime.sh` so a
//! packaged user (who does not have a source checkout) can run the same
//! maintenance modes the script has always supported. Nothing here downloads,
//! installs or restarts a service by itself: `--check` and `--gc` never touch a
//! running server, and `--rollback` keeps the script's own health checks and
//! requires an explicit confirmation flag.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

/// Maintenance modes exposed to the CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Report prerequisites and the selected runtime. Read-only.
    Check,
    /// Remove stale scratch and unreferenced runtime generations, keeping the
    /// selected generation and one rollback hop.
    Collect,
    /// Health-check and select the previous generation.
    Rollback,
}

impl Mode {
    pub fn script_argument(self) -> &'static str {
        match self {
            Self::Check => "--check",
            Self::Collect => "--gc",
            Self::Rollback => "--rollback",
        }
    }

    fn requires_confirmation(self) -> bool {
        matches!(self, Self::Rollback)
    }
}

/// Parses `dsh-ocean --runtime-maintenance <mode> [--confirm-maintenance]`.
pub fn parse(args: &[String]) -> Result<Mode> {
    let mut mode = None;
    let mut confirmed = false;
    for argument in args.iter().skip(1) {
        match argument.as_str() {
            "--confirm-maintenance" => confirmed = true,
            other => {
                if mode.is_some() {
                    bail!("expected exactly one maintenance mode");
                }
                mode = Some(match other {
                    "check" => Mode::Check,
                    "gc" | "collect" => Mode::Collect,
                    "rollback" => Mode::Rollback,
                    _ => bail!("expected check, gc or rollback"),
                });
            }
        }
    }
    let mode = mode.ok_or_else(|| anyhow::anyhow!("expected check, gc or rollback"))?;
    if mode.requires_confirmation() && !confirmed {
        bail!("rollback switches the active runtime; pass --confirm-maintenance");
    }
    if !mode.requires_confirmation() && confirmed {
        bail!("--confirm-maintenance only applies to rollback");
    }
    Ok(mode)
}

/// Builds the script invocation with the same private prefix the app uses.
///
/// The installer text is fed on stdin, so the script body never appears in the
/// process list and no temporary file is created.
pub fn command(mode: Mode, prefix: &Path) -> Command {
    let mut command = Command::new("bash");
    command
        .arg("-s")
        // `--` ends bash option parsing so the mode reaches the script as `$1`.
        .arg("--")
        .arg(mode.script_argument())
        .env("DSH_OCEAN_DATA_DIR", prefix)
        // Without this the script falls back to `${BASH_SOURCE[0]}`, which is empty
        // under `bash -s`, and the health gate would look in the current directory.
        .env("DSH_OCEAN_HEALTH_JS", crate::bootstrap::health_script())
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command
}

/// Runs one maintenance mode and returns the installer's exit status.
pub fn run(mode: Mode) -> Result<i32> {
    match std::fs::symlink_metadata(crate::runtime_install::pending_path(&prefix())) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error).context("inspect first-install recovery gate"),
        Ok(_) => {
            anyhow::bail!("incomplete first installation; retry installation before maintenance")
        }
    }
    if mode != Mode::Check {
        crate::runtime_switch::ensure_no_pending(&prefix())?;
    }
    // A rollback may refresh the unit: reject ambiguous configuration before
    // moving the runtime selector or launching any maintenance helper.
    let rollback_base = if mode == Mode::Rollback {
        Some(crate::config::checked_server_url()?)
    } else {
        None
    };
    let mut child = command(mode, &prefix())
        .spawn()
        .context("start runtime maintenance helper")?;
    let mut stdin = child.stdin.take().context("attach maintenance stdin")?;
    stdin
        .write_all(crate::bootstrap::install_script().as_bytes())
        .context("feed the runtime installer")?;
    drop(stdin);
    let status = child.wait().context("wait for runtime maintenance")?;
    let code = status.code().unwrap_or(1);
    if code == 0 && mode == Mode::Rollback {
        let base = rollback_base
            .as_ref()
            .expect("rollback origin validated before mutation");
        if !crate::service::permits_local_start(base) {
            eprintln!("dsh-ocean: rolled back the private runtime; remote origin configured, local service unit left unchanged");
            return Ok(code);
        }
        // The managed unit pins a concrete generation, so a rollback that only moved
        // `current` would leave the service on the old target. Refresh the unit now,
        // while it is stopped; an active unit is left alone and reported.
        match crate::bootstrap::detect() {
            Some(runtime) if !crate::systemd::is_active() => {
                if let Err(error) = crate::systemd::ensure_unit(&runtime, base) {
                    eprintln!(
                        "dsh-ocean: rolled back, but the service unit could not be refreshed: {error:#}"
                    );
                } else {
                    eprintln!(
                        "dsh-ocean: rolled back and refreshed the service unit to {}",
                        runtime.version
                    );
                }
            }
            Some(runtime) => eprintln!(
                "dsh-ocean: selected runtime {}, but {} is still running its pinned generation; review --print-unit and explicitly replace the installed unit before reloading/restarting",
                runtime.version,
                crate::config::UNIT_NAME
            ),
            None => eprintln!(
                "dsh-ocean: rolled back, but no runtime was detected afterwards; run --runtime-maintenance check"
            ),
        }
    }
    Ok(code)
}

/// Prefix used for maintenance, resolved the same way the app resolves it.
pub fn prefix() -> PathBuf {
    crate::config::data_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_only_known_modes_and_confirmation_rules() {
        assert_eq!(
            parse(&args(&["--runtime-maintenance", "check"])).unwrap(),
            Mode::Check
        );
        assert_eq!(
            parse(&args(&["--runtime-maintenance", "gc"])).unwrap(),
            Mode::Collect
        );
        assert_eq!(
            parse(&args(&["--runtime-maintenance", "collect"])).unwrap(),
            Mode::Collect
        );
        assert!(parse(&args(&["--runtime-maintenance", "rollback"])).is_err());
        assert_eq!(
            parse(&args(&[
                "--runtime-maintenance",
                "rollback",
                "--confirm-maintenance"
            ]))
            .unwrap(),
            Mode::Rollback
        );
        for invalid in [
            vec!["--runtime-maintenance"],
            vec!["--runtime-maintenance", "install"],
            vec!["--runtime-maintenance", "check", "gc"],
            vec!["--runtime-maintenance", "check", "--confirm-maintenance"],
            vec!["--runtime-maintenance", "gc", "--confirm-maintenance"],
            vec![
                "--runtime-maintenance",
                "rollback",
                "--confirm-maintenance",
                "check",
            ],
        ] {
            assert!(parse(&args(&invalid)).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn script_arguments_match_the_embedded_installer_modes() {
        assert_eq!(Mode::Check.script_argument(), "--check");
        assert_eq!(Mode::Collect.script_argument(), "--gc");
        assert_eq!(Mode::Rollback.script_argument(), "--rollback");
        // The embedded installer must advertise every mode we expose.
        for flag in ["--check", "--gc", "--rollback"] {
            assert!(
                crate::bootstrap::install_script().contains(flag),
                "installer help/support is missing {flag}"
            );
        }
    }

    #[test]
    fn command_uses_the_private_prefix_and_no_extra_shell() {
        let built = command(Mode::Check, Path::new("/tmp/synthetic-prefix"));
        assert_eq!(built.get_program(), "bash");
        let arguments: Vec<_> = built
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        assert_eq!(arguments, vec!["-s", "--", "--check"]);
        let prefix = built
            .get_envs()
            .find(|(name, _)| *name == "DSH_OCEAN_DATA_DIR")
            .and_then(|(_, value)| value)
            .expect("prefix environment");
        assert_eq!(prefix, "/tmp/synthetic-prefix");
    }
}

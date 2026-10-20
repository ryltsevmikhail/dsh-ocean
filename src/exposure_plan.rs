//! Pairing/exposure preparation.
//!
//! The client never rebinds the service, edits the installed unit or restarts the
//! user manager by itself. This module stages a **candidate** unit file plus the
//! exact apply/verify/revoke commands for a human to run, so the decision stays
//! explicit. Nothing here invokes `systemctl`, opens a firewall port or writes to
//! the installed unit path.
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

/// Non-loopback, non-unspecified addresses a phone on the same network could use.
pub fn acceptable_lan(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => v4.is_private() && !v4.is_loopback() && !v4.is_link_local(),
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            // Unique local addresses (fc00::/7) only.
            !v6.is_loopback() && (segments[0] & 0xfe00) == 0xfc00
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Staged {
    /// Candidate unit file written under the private state directory.
    pub staged_unit: PathBuf,
    pub trusted_host: String,
    pub candidate_url: String,
    pub apply_commands: Vec<String>,
    pub verify_commands: Vec<String>,
    pub revoke_commands: Vec<String>,
    pub warnings: Vec<String>,
    pub changed_anything: bool,
}

fn command_lines(
    port: u16,
    trusted_host: &str,
    unit: &Path,
    staged: &Path,
    backup: &Path,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let apply = vec![
        format!("cp -f -- {} {}", shell_quote(staged), shell_quote(unit)),
        "systemctl --user daemon-reload".to_owned(),
        "systemctl --user restart dsh-web.service".to_owned(),
    ];
    let verify = vec![
        format!("curl -fsS \"http://127.0.0.1:{port}/\" >/dev/null && echo loopback-ok"),
        format!("curl -fsS \"http://{trusted_host}/\" >/dev/null && echo lan-ok"),
        "systemctl --user --no-pager status dsh-web.service".to_owned(),
    ];
    let revoke = vec![
        format!("cp -f -- {} {}", shell_quote(backup), shell_quote(unit)),
        "systemctl --user daemon-reload".to_owned(),
        "systemctl --user restart dsh-web.service".to_owned(),
    ];
    (apply, verify, revoke)
}

/// Single-quote a path for a POSIX shell, which is what the printed commands use.
pub fn shell_quote(path: &Path) -> String {
    let text = path.display().to_string();
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Copies the currently installed unit next to the candidate so revoking is one
/// command, and stages the candidate itself.
pub fn prepare(
    state_dir: &Path,
    port: u16,
    address: IpAddr,
    installed_unit: &Path,
) -> Result<Staged> {
    anyhow::ensure!(acceptable_lan(address), "a private LAN address is required");
    anyhow::ensure!((1024..=65535).contains(&port), "unexpected port");
    let source = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(installed_unit)
        .context("read the installed service unit; install the service first")?;
    let metadata = source.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= 64 * 1024,
        "unexpected installed unit file"
    );
    let mut unit = String::new();
    source.take(64 * 1024 + 1).read_to_string(&mut unit)?;
    anyhow::ensure!(
        !unit.contains('\u{0}') && unit.len() <= 64 * 1024,
        "unexpected unit content"
    );
    let trusted_host = SocketAddr::new(address, port).to_string();
    let exec_lines: Vec<_> = unit
        .lines()
        .filter(|line| line.trim_start().starts_with("ExecStart="))
        .collect();
    anyhow::ensure!(
        exec_lines.len() == 1,
        "the installed unit must have exactly one ExecStart"
    );
    let exec = exec_lines[0];
    let needle = format!("--host 127.0.0.1 --port {port}");
    let offset = exec
        .find(&needle)
        .context("the installed unit has an unexpected bind address or port")?;
    anyhow::ensure!(
        exec.matches("--host ").count() == 1
            && exec.matches("--port ").count() == 1
            && !exec.contains("--trusted-host")
            && exec[..offset].ends_with(' ')
            && exec[offset + needle.len()..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace),
        "the installed unit has ambiguous bind arguments"
    );
    let bind = if address.is_ipv6() { "::" } else { "0.0.0.0" };
    let exposed_exec = exec.replacen(
        &needle,
        &format!("--host {bind} --port {port} --trusted-host {trusted_host}"),
        1,
    );
    let start = unit
        .match_indices(exec)
        .find(|(index, _)| *index == 0 || unit.as_bytes()[index - 1] == b'\n')
        .map(|(index, _)| index)
        .context("could not locate ExecStart")?;
    let mut candidate = unit.clone();
    candidate.replace_range(start..start + exec.len(), &exposed_exec);
    let exposure_dir = state_dir.join("exposure");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&exposure_dir)
        .context("create the exposure staging directory")?;
    anyhow::ensure!(
        fs::symlink_metadata(&exposure_dir)?.file_type().is_dir(),
        "the exposure directory must not be a symlink"
    );
    let plan_dir = exposure_dir.join(crate::downloads::unique_id());
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&plan_dir)
        .context("create a private exposure plan")?;
    let staged_unit = plan_dir.join("dsh-web.exposed.service");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&staged_unit)
        .context("write the staged unit")?;
    file.write_all(candidate.as_bytes())?;
    file.sync_all()?;
    fs::set_permissions(&staged_unit, fs::Permissions::from_mode(0o600))?;
    let backup_unit = plan_dir.join("dsh-web.installed.service");
    let mut backup = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&backup_unit)
        .context("stage a copy of the installed unit")?;
    backup.write_all(unit.as_bytes())?;
    backup.sync_all()?;
    fs::set_permissions(&backup_unit, fs::Permissions::from_mode(0o600))?;
    let (apply_commands, verify_commands, revoke_commands) = command_lines(
        port,
        &trusted_host,
        installed_unit,
        &staged_unit,
        &backup_unit,
    );
    Ok(Staged {
        staged_unit,
        trusted_host: trusted_host.clone(),
        candidate_url: format!("http://{trusted_host}/"),
        apply_commands,
        verify_commands,
        revoke_commands,
        warnings: vec![
            "Anyone on this network who reaches the port sees the same UI; the Harness session token is the only access control."
                .to_owned(),
            "Exposure is a deliberate, reversible change you apply yourself; Ocean does not restart the service."
                .to_owned(),
            "Keep the pre-change unit so revoking is one copy away; the staged directory is private (0700/0600)."
                .to_owned(),
            "Mobile pairing beyond a browser URL (QR code, one-click trust) is not implemented; this only prepares the bind address."
                .to_owned(),
        ],
        changed_anything: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-exposure-{}-{}-{}",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    const UNIT: &str = "[Service]\nExecStart=\"/usr/bin/node\" \"/x/dsh\" --profile web --host 127.0.0.1 --port 3080 --no-open\nRestart=on-failure\n";

    #[test]
    fn private_addresses_are_accepted_and_public_ones_refused() {
        for accepted in ["192.168.1.50", "10.0.0.7", "172.16.5.4", "fd00::5"] {
            let address: IpAddr = accepted.parse().unwrap();
            assert!(
                acceptable_lan(address),
                "{accepted} should be a LAN address"
            );
        }
        for refused in [
            "8.8.8.8",
            "192.0.2.10",
            "127.0.0.1",
            "169.254.1.1",
            "::1",
            "0.0.0.0",
        ] {
            let address: IpAddr = refused.parse().unwrap();
            assert!(
                !acceptable_lan(address),
                "{refused} must not be treated as LAN"
            );
        }
    }

    #[test]
    fn staging_rewrites_only_the_bind_address_and_keeps_the_file_private() {
        let root = temp("stage");
        let installed = root.join("dsh-web.service");
        fs::write(&installed, UNIT).unwrap();
        let staged = prepare(&root, 3080, "192.168.1.50".parse().unwrap(), &installed).unwrap();
        let text = fs::read_to_string(&staged.staged_unit).unwrap();
        assert!(text.contains("--host 0.0.0.0 --port 3080 --trusted-host 192.168.1.50:3080"));
        assert!(!text.contains("--host 127.0.0.1"));
        assert!(text.contains("Restart=on-failure"), "{text}");
        let mode = fs::metadata(&staged.staged_unit)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(staged.trusted_host, "192.168.1.50:3080");
        assert_eq!(staged.candidate_url, "http://192.168.1.50:3080/");
        assert!(!staged.changed_anything);
        // Only the candidate and a private copy of the installed unit are written.
        let mut entries: Vec<_> = fs::read_dir(staged.staged_unit.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        assert_eq!(
            entries,
            vec!["dsh-web.exposed.service", "dsh-web.installed.service"]
        );
        let backup_path = staged
            .staged_unit
            .parent()
            .unwrap()
            .join("dsh-web.installed.service");
        assert_eq!(fs::read(&backup_path).unwrap(), UNIT.as_bytes());
        let backup_mode = fs::metadata(backup_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(backup_mode, 0o600, "the unit copy must be private");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refused_inputs_never_write_a_candidate() {
        let root = temp("refuse");
        for address in ["8.8.8.8", "127.0.0.1"] {
            let installed = root.join("dsh-web.service");
            let outcome = prepare(&root, 3080, address.parse().unwrap(), &installed);
            assert!(outcome.is_err(), "{address} must be refused");
        }
        let installed = root.join("dsh-web.service");
        fs::write(&installed, "no host here").unwrap();
        assert!(prepare(&root, 3080, "192.168.1.50".parse().unwrap(), &installed).is_err());
        assert!(!root.join("exposure").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ipv6_plans_use_bracketed_authorities_and_do_not_replace_previous_plans() {
        let root = temp("ipv6");
        let installed = root.join("dsh-web.service");
        fs::write(&installed, UNIT).unwrap();
        let first = prepare(&root, 3080, "fd00::5".parse().unwrap(), &installed).unwrap();
        assert_eq!(first.candidate_url, "http://[fd00::5]:3080/");
        assert_eq!(first.trusted_host, "[fd00::5]:3080");
        assert!(fs::read_to_string(&first.staged_unit)
            .unwrap()
            .contains("--host :: --port 3080 --trusted-host [fd00::5]:3080"));
        assert!(first
            .verify_commands
            .iter()
            .any(|line| line.contains("http://[fd00::5]:3080/")));
        let second = prepare(&root, 3080, "10.0.0.7".parse().unwrap(), &installed).unwrap();
        assert_ne!(first.staged_unit, second.staged_unit);
        assert!(first.staged_unit.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinked_installed_units_and_staging_directories_are_refused() {
        use std::os::unix::fs::symlink;
        let root = temp("symlinks");
        let installed = root.join("dsh-web.service");
        let target = root.join("real.service");
        fs::write(&target, UNIT).unwrap();
        symlink(&target, &installed).unwrap();
        assert!(prepare(&root, 3080, "10.0.0.7".parse().unwrap(), &installed).is_err());
        assert_eq!(installed, root.join("dsh-web.service"));
        fs::remove_file(&installed).unwrap();
        fs::write(&installed, UNIT).unwrap();
        let other = root.join("other");
        fs::create_dir(&other).unwrap();
        symlink(&other, root.join("exposure")).unwrap();
        assert!(prepare(&root, 3080, "10.0.0.7".parse().unwrap(), &installed).is_err());
        assert_eq!(fs::read_dir(other).unwrap().count(), 0);
        assert_eq!(fs::read_to_string(target).unwrap(), UNIT);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commands_are_explicit_and_never_run_by_the_client() {
        let root = temp("commands");
        let installed = root.join("dsh-web.service");
        fs::write(&installed, UNIT).unwrap();
        let staged = prepare(&root, 3080, "10.0.0.7".parse().unwrap(), &installed).unwrap();
        assert!(staged
            .apply_commands
            .iter()
            .any(|line| line.contains("systemctl --user daemon-reload")));
        assert!(staged
            .verify_commands
            .iter()
            .any(|line| line.contains("http://10.0.0.7:3080/")));
        // The printed commands must be runnable as printed: literal quoted paths,
        // no undefined shell variables.
        let apply = staged.apply_commands[0].clone();
        assert!(apply.starts_with("cp -f -- '"), "{apply}");
        assert!(
            apply.contains(&staged.staged_unit.display().to_string()),
            "{apply}"
        );
        assert!(apply.contains("dsh-web.service'"), "{apply}");
        assert!(!staged.apply_commands.iter().any(|line| line.contains('$')));
        let revoke = staged.revoke_commands[0].clone();
        assert!(
            revoke.contains("dsh-web.installed.service"),
            "revoke must use the staged copy of the installed unit: {revoke}"
        );
        assert!(!staged.revoke_commands.iter().any(|line| line.contains('$')));
        assert!(staged
            .warnings
            .iter()
            .any(|warning| warning.contains("does not restart")));
        assert!(staged
            .warnings
            .iter()
            .any(|warning| warning.contains("not implemented")));
        fs::remove_dir_all(root).unwrap();
    }
}

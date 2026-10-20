//! Offline installer fixtures: PATH deliberately contains no system Node/npm.
//! No network, live DSH profile, service or port is used.
#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    prefix: PathBuf,
    checksum: String,
}

fn executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-install-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Exclusive root creation must precede create_dir_all below: wall-clock
        // timestamps can collide across parallel test threads.
        fs::create_dir(&root).expect("create exclusive installer fixture root");
        let tools = root.join("system tools");
        let node = root.join("archive/node/bin");
        let prefix = root.join("private runtime");
        for path in [
            &tools,
            &node,
            &prefix,
            &root.join("home"),
            &root.join("tmp"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        for tool in [
            "uname",
            "tar",
            "gzip",
            "sha256sum",
            "flock",
            "mkdir",
            "awk",
            "sed",
            "tail",
            "grep",
            "head",
            "mktemp",
            "rm",
            "mv",
            "cp",
            "dirname",
            "readlink",
            "ln",
        ] {
            symlink(format!("/usr/bin/{tool}"), tools.join(tool)).unwrap();
        }
        // These tiny offline archives do not need the production installer's disk
        // budget. Control df explicitly so branch coverage does not depend on the
        // runner's remaining space; low-space tests exercise the real guard below.
        executable(
            &tools.join("df"),
            r#"#!/bin/bash
printf 'Filesystem 1024-blocks Used Available Capacity Mounted on\n'
printf 'fixture 8000000 0 %s 0%% /\n' "${MOCK_FREE_KB-4000000}"
"#,
        );
        executable(
            &node.join("node"),
            r#"#!/bin/bash
case "$1" in
  --version) echo v26.12.0 ;;
  -e) exit "${MOCK_NATIVE_FAIL:-0}" ;;
  --input-type=module) exec /bin/bash "$FIXTURE/health" "${@: -1}" ;;
  */check-runtime.mjs) exec /bin/bash "$FIXTURE/health" "$2" ;;
  *) exec /bin/bash "$@" ;;
esac
"#,
        );
        executable(
            &node.join("npm"),
            r#"#!/usr/bin/env node
case "$1" in
  install)
    [ "${MOCK_INSTALL_FAIL:-0}" = 0 ] || exit 40
    while [ "$#" -gt 0 ]; do
      if [ "$1" = --prefix ]; then prefix="$2"; shift; fi
      package="$1"
      shift
    done
    mkdir -p "$prefix/node_modules/.bin"
    case "$package" in
      pnpm@*) cp "$FIXTURE/pnpm" "$prefix/node_modules/.bin/pnpm" ;;
      *) cp "$FIXTURE/dsh" "$prefix/node_modules/.bin/dsh" ;;
    esac
    ;;
  install-scripts)
    if [ "$2" = approve ]; then exit "${MOCK_APPROVE_FAIL:-0}"; fi
    ;;
  rebuild) exit "${MOCK_REBUILD_FAIL:-0}" ;;
  *) exit 99 ;;
esac
"#,
        );
        executable(
            &root.join("dsh"),
            "#!/bin/bash\necho \"${MOCK_DSH_VERSION:-0.2.0-rc.2}\"\n",
        );
        executable(
            &root.join("pnpm"),
            "#!/bin/bash\necho \"${MOCK_PNPM_VERSION:-10.34.7}\"\n",
        );
        executable(
            &root.join("health"),
            r#"#!/bin/bash
printf '%s\n' "$1" >> "$FIXTURE/health-calls"
count="$(awk 'END {print NR}' "$FIXTURE/health-calls")"
if [ "$count" = "${MOCK_HEALTH_FAIL_AT:-never}" ]; then exit 1; fi
if [ -L "$DSH_OCEAN_DATA_DIR/current" ]; then
  for file in runtime/node/bin/node harness/node_modules/.bin/dsh tools/node_modules/.bin/pnpm; do
    [ -x "$DSH_OCEAN_DATA_DIR/current/$file" ] || exit 98
  done
fi
"#,
        );
        executable(
            &tools.join("curl"),
            r#"#!/bin/bash
printf 'download\n' >> "$FIXTURE/downloads"
while [ "$#" -gt 0 ]; do
  if [ "$1" = -o ]; then output="$2"; break; fi
  shift
done
cp "$FIXTURE/node.tar.gz" "$output"
"#,
        );
        let status = Command::new("/usr/bin/tar")
            .args(["-czf"])
            .arg(root.join("node.tar.gz"))
            .arg("-C")
            .arg(root.join("archive"))
            .arg("node")
            .status()
            .unwrap();
        assert!(status.success());
        let hash = Command::new("/usr/bin/sha256sum")
            .arg(root.join("node.tar.gz"))
            .output()
            .unwrap();
        assert!(hash.status.success());
        let checksum = String::from_utf8(hash.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned();
        Self {
            root,
            prefix,
            checksum,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new("/bin/bash");
        cmd.arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/bootstrap-runtime.sh"))
            .env("PATH", self.root.join("system tools"))
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("FIXTURE", &self.root)
            .env("DSH_OCEAN_DATA_DIR", &self.prefix)
            .env("DSH_NODE_VERSION", "26.12.0")
            .env("DSH_NODE_SHA256", &self.checksum)
            .env("DSH_VERSION", "0.2.0-rc.2")
            .env("DSH_PNPM_VERSION", "10.34.7");
        cmd
    }

    fn seed_old_harness(&self) {
        let bin = self.prefix.join("runtime/node/bin");
        fs::create_dir_all(&bin).unwrap();
        for name in ["node", "npm"] {
            fs::copy(
                self.root.join("archive/node/bin").join(name),
                bin.join(name),
            )
            .unwrap();
        }
        let harness = self.prefix.join("harness/node_modules/.bin");
        fs::create_dir_all(&harness).unwrap();
        executable(&harness.join("dsh"), "#!/bin/bash\necho 0.1.0\n");
        fs::write(self.prefix.join("harness/keep-me"), "old runtime").unwrap();
        let tools = self.prefix.join("tools/node_modules/.bin");
        fs::create_dir_all(&tools).unwrap();
        fs::copy(self.root.join("pnpm"), tools.join("pnpm")).unwrap();
    }

    fn assert_clean_staging(&self) {
        for parent in [
            &self.prefix,
            &self.prefix.join("runtime"),
            &self.prefix.join("generations"),
        ] {
            if !parent.exists() {
                continue;
            }
            for entry in fs::read_dir(parent).unwrap() {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                assert!(
                    !name.starts_with(".harness-stage.")
                        && !name.starts_with(".node-stage.")
                        && !name.starts_with(".download.")
                        && !name.starts_with(".pointer."),
                    "leftover: {name}"
                );
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // root is the unique temporary directory created by this fixture.
        assert!(self
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-install-"));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn describe(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn installs_without_system_node_and_is_idempotent() {
    let f = Fixture::new();
    let no_node = Command::new("/bin/bash")
        .args(["-c", "command -v node"])
        .env("PATH", f.root.join("system tools"))
        .output()
        .unwrap();
    assert!(!no_node.status.success());
    let first = f.command().env("MOCK_FREE_KB", "1500000").output().unwrap();
    assert!(first.status.success(), "{}", describe(&first));
    assert!(f
        .prefix
        .join("current/harness/node_modules/.bin/dsh")
        .is_file());
    assert!(f
        .prefix
        .join("current/tools/node_modules/.bin/pnpm")
        .is_file());
    let selected = fs::read_link(f.prefix.join("current")).unwrap();
    let second = f.command().output().unwrap();
    assert!(second.status.success(), "{}", describe(&second));
    assert_eq!(fs::read_link(f.prefix.join("current")).unwrap(), selected);
    assert_eq!(
        fs::read_to_string(f.root.join("downloads"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    f.assert_clean_staging();
}

#[test]
fn insufficient_space_refuses_before_download_or_runtime_changes() {
    for free_kb in ["0", "1499999"] {
        let f = Fixture::new();
        f.seed_old_harness();
        let output = f.command().env("MOCK_FREE_KB", free_kb).output().unwrap();
        assert!(
            !output.status.success(),
            "accepted {free_kb}: {}",
            describe(&output)
        );
        assert!(
            describe(&output).contains("at least 1.5 GiB free is required"),
            "{}",
            describe(&output)
        );
        assert!(!f.root.join("downloads").exists());
        assert!(!f.prefix.join("current").exists());
        assert_eq!(
            fs::read_to_string(f.prefix.join("harness/keep-me")).unwrap(),
            "old runtime"
        );
        f.assert_clean_staging();
    }
}

#[test]
fn failures_never_replace_the_previous_harness() {
    for (variable, value) in [
        ("MOCK_INSTALL_FAIL", "1"),
        ("MOCK_APPROVE_FAIL", "1"),
        ("MOCK_REBUILD_FAIL", "1"),
        ("MOCK_NATIVE_FAIL", "1"),
        ("MOCK_DSH_VERSION", "9.9.9"),
        ("MOCK_PNPM_VERSION", "0.0.0"),
        ("MOCK_HEALTH_FAIL_AT", "1"),
        ("MOCK_HEALTH_FAIL_AT", "2"),
    ] {
        let f = Fixture::new();
        f.seed_old_harness();
        let old_node = fs::read(f.prefix.join("runtime/node/bin/node")).unwrap();
        let output = f.command().env(variable, value).output().unwrap();
        assert!(
            !output.status.success(),
            "accepted {variable}: {}",
            describe(&output)
        );
        assert_eq!(
            fs::read_to_string(f.prefix.join("harness/keep-me")).unwrap(),
            "old runtime"
        );
        assert!(
            fs::read_to_string(f.prefix.join("harness/node_modules/.bin/dsh"))
                .unwrap()
                .contains("0.1.0")
        );
        assert_eq!(
            fs::read(f.prefix.join("runtime/node/bin/node")).unwrap(),
            old_node
        );
        assert!(!f.prefix.join("current").exists());
        assert_eq!(
            fs::read_dir(f.prefix.join("generations")).unwrap().count(),
            0
        );
        f.assert_clean_staging();
    }
}

#[test]
fn migrates_legacy_without_modifying_it_and_can_roll_back() {
    let f = Fixture::new();
    f.seed_old_harness();
    let install = f.command().output().unwrap();
    assert!(install.status.success(), "{}", describe(&install));
    assert_eq!(
        fs::read_to_string(f.prefix.join("harness/keep-me")).unwrap(),
        "old runtime"
    );
    let selected = fs::read_link(f.prefix.join("current")).unwrap();
    assert_eq!(
        fs::read_to_string(f.prefix.join(&selected).join(".previous"))
            .unwrap()
            .trim(),
        "legacy"
    );
    let rollback = f.command().arg("--rollback").output().unwrap();
    assert!(rollback.status.success(), "{}", describe(&rollback));
    assert!(fs::symlink_metadata(f.prefix.join("current")).is_err());
    assert!(
        f.prefix.join(&selected).is_dir(),
        "retain generation for running processes"
    );
}

#[test]
fn failed_post_activation_health_restores_previous_generation() {
    let f = Fixture::new();
    let initial = f.command().output().unwrap();
    assert!(initial.status.success(), "{}", describe(&initial));
    let before = fs::read_link(f.prefix.join("current")).unwrap();
    fs::write(f.root.join("health-calls"), "").unwrap();
    let failed = f
        .command()
        .env("DSH_VERSION", "0.2.1-rc.2")
        .env("MOCK_DSH_VERSION", "0.2.1-rc.2")
        .env("MOCK_HEALTH_FAIL_AT", "2")
        .output()
        .unwrap();
    assert!(!failed.status.success(), "{}", describe(&failed));
    assert!(describe(&failed).contains("restoring previous runtime selector"));
    assert_eq!(fs::read_link(f.prefix.join("current")).unwrap(), before);
    assert_eq!(
        fs::read_dir(f.prefix.join("generations")).unwrap().count(),
        1
    );
    f.assert_clean_staging();
}

#[test]
fn rollback_checks_health_and_retains_both_generations() {
    let f = Fixture::new();
    assert!(f.command().output().unwrap().status.success());
    let first = fs::read_link(f.prefix.join("current")).unwrap();
    let upgrade = f
        .command()
        .env("DSH_VERSION", "0.2.1-rc.2")
        .env("MOCK_DSH_VERSION", "0.2.1-rc.2")
        .output()
        .unwrap();
    assert!(upgrade.status.success(), "{}", describe(&upgrade));
    let second = fs::read_link(f.prefix.join("current")).unwrap();
    assert_ne!(first, second);
    for failure_at in ["1", "2"] {
        fs::write(f.root.join("health-calls"), "").unwrap();
        let refused = f
            .command()
            .arg("--rollback")
            .env("MOCK_HEALTH_FAIL_AT", failure_at)
            .output()
            .unwrap();
        assert!(!refused.status.success());
        assert_eq!(fs::read_link(f.prefix.join("current")).unwrap(), second);
    }
    let rollback = f.command().arg("--rollback").output().unwrap();
    assert!(rollback.status.success(), "{}", describe(&rollback));
    assert_eq!(fs::read_link(f.prefix.join("current")).unwrap(), first);
    assert!(f.prefix.join(second).is_dir());
}

#[test]
fn rejects_bad_checksums_before_extracting() {
    let f = Fixture::new();
    let output = f
        .command()
        .env("DSH_NODE_SHA256", "0".repeat(64))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(describe(&output).contains("checksum mismatch"));
    assert!(!f.prefix.join("runtime/node").exists());
    f.assert_clean_staging();
}

#[test]
fn inherited_transaction_lock_is_verified_and_retained_by_parent() {
    let f = Fixture::new();
    let invalid = f
        .command()
        .env("DSH_OCEAN_INHERITED_LOCK", "9")
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(describe(&invalid).contains("invalid inherited runtime lock"));
    let base = f.command();
    let mut wrapper = Command::new("/bin/bash");
    wrapper.args([
        "-c",
        r#"
set -eu
exec 9>"$DSH_OCEAN_DATA_DIR/.bootstrap.lock"
flock -n 9
export DSH_OCEAN_INHERITED_LOCK=9
/bin/bash "$1"
if (exec 8>"$DSH_OCEAN_DATA_DIR/.bootstrap.lock"; flock -n 8); then exit 99; fi
"#,
        "transaction",
    ]);
    wrapper.args(base.get_args());
    for (key, value) in base.get_envs() {
        if let Some(value) = value {
            wrapper.env(key, value);
        } else {
            wrapper.env_remove(key);
        }
    }
    let installed = wrapper.output().unwrap();
    assert!(installed.status.success(), "{}", describe(&installed));
    assert!(f.prefix.join("current").is_symlink());
}

#[test]
fn refuses_a_concurrent_installer() {
    let f = Fixture::new();
    let mut holder = Command::new("/usr/bin/flock")
        .arg(f.prefix.join(".bootstrap.lock"))
        .args(["/bin/bash", "-c", "echo ready; read -r release"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(holder.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    let output = f.command().output().unwrap();
    // Closing stdin releases the holder even when an assertion below fails.
    drop(holder.stdin.take());
    let _ = holder.wait();
    assert!(!output.status.success());
    assert!(describe(&output).contains("another bootstrap is already running"));
    assert!(!f.prefix.join("runtime/node").exists());
}

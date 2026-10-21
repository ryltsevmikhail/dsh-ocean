//! Update transactions with a fake user manager; never invokes real systemctl.
#![cfg(unix)]
use std::fs;
use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    prefix: PathBuf,
    unit: PathBuf,
    before: String,
    template: String,
}
fn executable(path: &std::path::Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-update-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // A clock value is not a unique reservation under parallel execution.
        // Reserve the root exclusively before any nested create_dir_all/write.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap_or_else(|error| panic!("reserve isolated update fixture {root:?}: {error}"));
        let prefix = root.join("runtime prefix");
        let unit = root.join("config/systemd/user/dsh-web.service");
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        // Never fall back to the real systemctl, even during failure cleanup.
        for tool in [
            "flock", "cp", "chmod", "mv", "ln", "mktemp", "rm", "readlink", "dirname",
        ] {
            symlink(format!("/usr/bin/{tool}"), root.join("bin").join(tool))
                .unwrap_or_else(|error| panic!("create isolated tool {tool} in {root:?}: {error}"));
        }
        for name in ["gen.old", "gen.new"] {
            let generation = prefix.join("generations").join(name);
            fs::create_dir_all(generation.join("runtime/node/bin")).unwrap();
            fs::write(generation.join(".versions"), "fixture\n").unwrap();
            fs::write(generation.join(".previous"), "generations/gen.old\n").unwrap();
            executable(
                &generation.join("runtime/node/bin/node"),
                "#!/bin/bash\nexec /bin/bash \"$FIXTURE/health\" \"${@: -1}\"\n",
            );
        }
        symlink("generations/gen.old", prefix.join("current")).unwrap();
        let before = format!("[Service]\nExecStart={}/generations/gen.old/runtime/node/bin/node {}/generations/gen.old/harness/node_modules/.bin/dsh\n", prefix.display(), prefix.display());
        let template = before.replace("gen.old", "@@DSH_OCEAN_GENERATION@@");
        fs::write(&unit, &before).unwrap();
        fs::set_permissions(&unit, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(root.join("running-unit"), &before).unwrap();
        executable(
            &root.join("bin/systemctl"),
            r#"#!/bin/bash
set -eu
[ "$1" = --user ] || exit 99
case "$2" in
  show)
    if [[ "$*" == *--property=DropInPaths* ]]; then
      if [ "${MOCK_DROP_IN:-0}" = 1 ]; then echo /custom/drop-in.conf; fi
    elif [ "${MOCK_BAD_FRAGMENT:-0}" = 1 ]; then echo /wrong/unit; else echo "$DSH_OCEAN_UNIT_PATH"; fi ;;
  is-active) exit 0 ;;
  daemon-reload)
    echo reload >> "$FIXTURE/actions"
    if [[ "$(<"$DSH_OCEAN_UNIT_PATH")" == *gen.new* ]] && [ "${MOCK_RELOAD_FAIL:-0}" = 1 ]; then exit 43; fi ;;
  restart)
    echo restart >> "$FIXTURE/actions"
    if [[ "$(<"$DSH_OCEAN_UNIT_PATH")" == *gen.new* ]]; then
      : > "$FIXTURE/new-attempted"
      [ "${MOCK_RESTART_FAIL:-0}" = 0 ] || exit 44
    fi
    cp "$DSH_OCEAN_UNIT_PATH" "$FIXTURE/running-unit"
    if [ "${MOCK_EDIT_DURING_RESTART:-0}" = 1 ]; then echo '# preserved user edit' >> "$DSH_OCEAN_UNIT_PATH"; fi
    if [[ "$(<"$DSH_OCEAN_UNIT_PATH")" == *gen.new* ]] && [ "${MOCK_CANCEL:-0}" = 1 ]; then kill -TERM "$PPID"; fi ;;
  *) exit 99 ;;
esac
"#,
        );
        executable(
            &root.join("health"),
            r#"#!/bin/bash
set -eu
root="$1"
echo "health $root" >> "$FIXTURE/actions"
# Verify the installer lock remains held during both commit and rollback checks.
if (exec 8>"$DSH_OCEAN_DATA_DIR/.bootstrap.lock"; flock -n 8); then exit 97; fi
[[ "$(<"$FIXTURE/running-unit")" == *"$root/runtime/node/bin/node"* ]] || exit 45
if [[ "$root" == *gen.new ]] && [ "${MOCK_HEALTH_FAIL:-0}" = 1 ]; then exit 46; fi
if [[ "$root" == *gen.old ]] && [ -e "$FIXTURE/new-attempted" ] && [ "${MOCK_ROLLBACK_FAIL:-0}" = 1 ]; then exit 47; fi
"#,
        );
        executable(
            &root.join("installer"),
            r#"#!/bin/bash
set -eu
: > "$FIXTURE/installer-called"
[ "$(readlink /proc/self/fd/9)" = "$DSH_OCEAN_DATA_DIR/.bootstrap.lock" ]
flock -n 9
if (exec 8>"$DSH_OCEAN_DATA_DIR/.bootstrap.lock"; flock -n 8); then exit 96; fi
[ "${MOCK_INSTALL_FAIL:-0}" != before ] || exit 41
ln -s generations/gen.new "$DSH_OCEAN_DATA_DIR/candidate-link"
mv -Tf "$DSH_OCEAN_DATA_DIR/candidate-link" "$DSH_OCEAN_DATA_DIR/current"
[ "${MOCK_INSTALL_FAIL:-0}" != after ] || exit 42
if [ "${MOCK_EDIT_DURING_INSTALL:-0}" = 1 ]; then echo '# preserved user edit' >> "$DSH_OCEAN_UNIT_PATH"; fi
"#,
        );
        Self {
            root,
            prefix,
            unit,
            before,
            template,
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new("/bin/bash");
        cmd.arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/update-runtime.sh"))
            .env("PATH", self.root.join("bin"))
            .env("FIXTURE", &self.root)
            .env("DSH_OCEAN_DATA_DIR", &self.prefix)
            .env("DSH_OCEAN_UNIT_PATH", &self.unit)
            .env(
                "DSH_OCEAN_EXPECTED_ROOT",
                self.prefix.join("generations/gen.old"),
            )
            .env("DSH_OCEAN_OLD_UNIT", &self.before)
            .env("DSH_OCEAN_NEW_UNIT", &self.template)
            .env(
                "DSH_OCEAN_INSTALL_SCRIPT",
                fs::read_to_string(self.root.join("installer")).unwrap(),
            )
            .env("DSH_OCEAN_SERVICE_HEALTH_JS", "fixture");
        cmd
    }
    fn assert_old(&self) {
        assert_eq!(
            fs::read_link(self.prefix.join("current")).unwrap(),
            PathBuf::from("generations/gen.old")
        );
        assert_eq!(fs::read_to_string(&self.unit).unwrap(), self.before);
        assert_eq!(
            fs::read_to_string(self.root.join("running-unit")).unwrap(),
            self.before
        );
    }
    fn assert_no_transactions(&self) {
        for parent in [&self.prefix, self.unit.parent().unwrap()] {
            assert!(!fs::read_dir(parent).unwrap().any(|entry| {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                name.starts_with(".update.") || name.starts_with(".dsh-ocean-update.")
            }));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-ocean-update-"));
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn describe(result: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    )
}

#[test]
fn commits_selected_generation_and_unit_together() {
    let f = Fixture::new();
    let result = f.command().output().unwrap();
    assert!(result.status.success(), "{}", describe(&result));
    assert!(describe(&result).contains("update committed"));
    assert_eq!(
        fs::read_link(f.prefix.join("current")).unwrap(),
        PathBuf::from("generations/gen.new")
    );
    assert_eq!(
        fs::read_to_string(&f.unit).unwrap(),
        f.before.replace("gen.old", "gen.new")
    );
    assert_eq!(
        fs::metadata(&f.unit).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(f.prefix.join("generations/gen.old").is_dir());
    f.assert_no_transactions();
}

#[test]
fn only_a_single_generated_path_difference_is_allowed() {
    for extra in [
        "Environment=\"TOKEN=custom\"\n",
        "Restart=always\n",
        "Environment=\"PATH=/custom\" \"TOKEN=custom\"\n",
        "Environment=\"PATH=/one\"\nEnvironment=\"PATH=/two\"\n",
    ] {
        let f = Fixture::new();
        let custom = format!("{}{extra}", f.before);
        fs::write(&f.unit, &custom).unwrap();
        let result = f.command().output().unwrap();
        assert!(!result.status.success(), "{}", describe(&result));
        assert!(describe(&result).contains("custom changes"));
        assert!(!f.root.join("installer-called").exists());
        assert_eq!(fs::read_to_string(&f.unit).unwrap(), custom);
    }
    let mut f = Fixture::new();
    f.before.push_str("Environment=\"PATH=/expected\"\n");
    f.template = f.before.replace("gen.old", "@@DSH_OCEAN_GENERATION@@");
    fs::write(&f.unit, f.before.replace("/expected", "/different")).unwrap();
    let result = f.command().output().unwrap();
    assert!(result.status.success(), "{}", describe(&result));
}

#[test]
fn every_activation_failure_restores_old_unit_and_selector() {
    for (variable, value) in [
        ("MOCK_INSTALL_FAIL", "before"),
        ("MOCK_INSTALL_FAIL", "after"),
        ("MOCK_RELOAD_FAIL", "1"),
        ("MOCK_RESTART_FAIL", "1"),
        ("MOCK_HEALTH_FAIL", "1"),
        ("MOCK_CANCEL", "1"),
    ] {
        let f = Fixture::new();
        let result = f.command().env(variable, value).output().unwrap();
        assert!(
            !result.status.success(),
            "accepted {variable}: {}",
            describe(&result)
        );
        assert!(
            describe(&result).contains("rollback complete"),
            "{}",
            describe(&result)
        );
        f.assert_old();
        f.assert_no_transactions();
        if variable == "MOCK_INSTALL_FAIL" {
            assert!(!fs::read_to_string(f.root.join("actions"))
                .unwrap()
                .contains("restart"));
        }
    }
}

#[test]
fn refuses_custom_units_and_stale_selection_before_mutation() {
    for (variable, value) in [
        ("DSH_OCEAN_OLD_UNIT", "custom"),
        ("MOCK_BAD_FRAGMENT", "1"),
        ("MOCK_DROP_IN", "1"),
        ("DSH_OCEAN_EXPECTED_ROOT", "/stale"),
    ] {
        let f = Fixture::new();
        let result = f.command().env(variable, value).output().unwrap();
        assert!(
            !result.status.success(),
            "case {variable} unexpectedly succeeded: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!f.root.join("installer-called").exists());
        f.assert_old();
        f.assert_no_transactions();
    }
}

#[test]
fn preserves_unit_edits_made_during_installation_or_failed_activation() {
    for variable in ["MOCK_EDIT_DURING_INSTALL", "MOCK_EDIT_DURING_RESTART"] {
        let f = Fixture::new();
        let result = f
            .command()
            .env(variable, "1")
            .env("MOCK_HEALTH_FAIL", "1")
            .output()
            .unwrap();
        assert!(!result.status.success(), "{}", describe(&result));
        assert_eq!(
            fs::read_link(f.prefix.join("current")).unwrap(),
            PathBuf::from("generations/gen.old")
        );
        assert!(fs::read_to_string(&f.unit)
            .unwrap()
            .contains("# preserved user edit"));
        if variable == "MOCK_EDIT_DURING_RESTART" {
            assert!(describe(&result).contains("rollback incomplete"));
        } else {
            assert!(!fs::read_to_string(f.root.join("actions"))
                .unwrap()
                .contains("restart"));
        }
    }
}

#[test]
fn update_task_uses_detached_manager_unit_and_private_logs() {
    use dsh_ocean::bootstrap::Runtime;
    let f = Fixture::new();
    let root = f.prefix.join("generations/gen.old");
    let runtime = Runtime {
        node: root.join("runtime/node/bin/node"),
        dsh: root.join("harness/node_modules/.bin/dsh"),
        version: "0.2.0-rc.2".into(),
        install_root: Some(root.join("harness")),
    };
    let task = dsh_ocean::runtime_update::prepare_at(
        &runtime,
        "0.2.1-rc.2",
        &url::Url::parse("http://127.0.0.1:39081/").unwrap(),
        &f.prefix,
        &f.unit,
        &f.root.join("logs"),
    )
    .unwrap();
    let args: Vec<_> = task
        .command
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(task.command.as_std().get_program(), "systemd-run");
    assert!(args.iter().any(|arg| arg == "--expand-environment=no"));
    assert!(args.iter().any(|arg| arg == "--wait"));
    assert!(!args.iter().any(|arg| arg == "--pipe"));
    assert!(args
        .iter()
        .any(|arg| arg.starts_with("--property=StandardOutput=append:")));
    assert!(args
        .iter()
        .any(|arg| arg.starts_with("--setenv=DSH_OCEAN_NEW_UNIT=")
            && arg.contains("@@DSH_OCEAN_GENERATION@@")));
    assert_eq!(
        fs::metadata(task.log).unwrap().permissions().mode() & 0o777,
        0o600
    );
    f.assert_old();
    assert!(dsh_ocean::runtime_update::prepare_at(
        &runtime,
        "next",
        &url::Url::parse("http://127.0.0.1:39081/").unwrap(),
        &f.prefix,
        &f.unit,
        &f.root.join("logs")
    )
    .is_err());
    assert!(dsh_ocean::runtime_update::prepare_at(
        &runtime,
        "0.2.1-rc.2",
        &url::Url::parse("https://example.com/").unwrap(),
        &f.prefix,
        &f.unit,
        &f.root.join("logs")
    )
    .is_err());
}

#[test]
fn isolated_tasks_require_a_test_namespace_and_private_home() {
    use dsh_ocean::{bootstrap::Runtime, runtime_update, systemd};
    let f = Fixture::new();
    let root = f.prefix.join("generations/gen.old");
    let runtime = Runtime {
        node: root.join("runtime/node/bin/node"),
        dsh: root.join("harness/node_modules/.bin/dsh"),
        version: "0.2.0-rc.2".into(),
        install_root: Some(root.join("harness")),
    };
    let base = url::Url::parse("http://127.0.0.1:39081/").unwrap();
    let home = f.prefix.join("service home%literal\\raw");
    fs::create_dir(&home).unwrap();
    let unit = f.unit.with_file_name("dsh-ocean-test-isolated.service");
    assert!(runtime_update::prepare_at(
        &runtime,
        "0.2.1-rc.2",
        &base,
        &f.prefix,
        &unit,
        &f.root.join("logs")
    )
    .is_err());
    assert!(runtime_update::prepare_isolated_at(
        &runtime,
        "0.2.1-rc.2",
        &base,
        &f.prefix,
        &f.unit,
        &f.root.join("logs"),
        &home
    )
    .is_err());
    assert!(runtime_update::prepare_isolated_at(
        &runtime,
        "0.2.1-rc.2",
        &base,
        &f.prefix,
        &unit,
        &f.root.join("logs"),
        &f.root
    )
    .is_err());
    let task = runtime_update::prepare_isolated_at(
        &runtime,
        "0.2.1-rc.2",
        &base,
        &f.prefix,
        &unit,
        &f.root.join("logs"),
        &home,
    )
    .unwrap();
    let args: Vec<_> = task
        .command
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(args.iter().any(|arg| arg == "/usr/bin/env"));
    assert!(args.iter().any(|arg| arg == "-i"));
    assert!(args
        .iter()
        .any(|arg| arg == "--setenv=DSH_OCEAN_SERVICE_UNIT=dsh-ocean-test-isolated.service"));
    let unit = systemd::render_isolated_unit(&runtime, &base, &home).unwrap();
    assert!(unit.contains("ExecStart=/usr/bin/env -i"));
    assert!(unit.contains("Restart=no"));
    assert!(unit.contains("RuntimeMaxSec=20min"));
    assert!(unit.contains("home%%literal\\\\raw"));
    assert!(!unit.contains("WorkingDirectory=%h"));
    if std::path::Path::new("/usr/bin/systemd-analyze").is_file() {
        let unit_path = f.unit.with_file_name("dsh-ocean-test-isolated.service");
        fs::write(&unit_path, &unit).unwrap();
        let result = Command::new("/usr/bin/systemd-analyze")
            .arg("verify")
            .arg(&unit_path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "invalid isolated unit: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    for name in [
        "dbus.service",
        "dsh-web.service",
        "dsh-ocean-test-.service",
        "dsh-ocean-test-../evil.service",
    ] {
        assert!(!systemd::test_unit_name(name));
    }
}

#[test]
fn refuses_unexpected_service_namespace_before_any_manager_action() {
    for name in [
        "dbus.service",
        "dsh-ocean-test-other.service",
        "../dsh-web.service",
    ] {
        let f = Fixture::new();
        let output = f
            .command()
            .env("DSH_OCEAN_SERVICE_UNIT", name)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!f.root.join("installer-called").exists());
        f.assert_old();
    }
}

#[test]
fn log_tail_redacts_complete_lines_and_discards_partial_first_line() {
    let f = Fixture::new();
    let file = f.root.join("update.log");
    fs::write(&file, "launch token=synthetic-secret\nupdate done\n").unwrap();
    let tail = dsh_ocean::runtime_update::log_tail(&file);
    assert!(!tail.contains("synthetic-secret"));
    assert!(tail.contains("token=***"));
    fs::write(
        &file,
        format!("{}token=synthetic-secret\nupdate done\n", "x".repeat(70000)),
    )
    .unwrap();
    assert_eq!(dsh_ocean::runtime_update::log_tail(&file), "update done");
}

#[test]
#[ignore = "requires a user systemd manager; worker uses mock service commands only"]
fn detached_manager_worker_survives_waiter_exit() {
    use dsh_ocean::bootstrap::Runtime;
    use std::time::{Duration, Instant};
    struct ManagerGuard(String);
    impl Drop for ManagerGuard {
        fn drop(&mut self) {
            assert!(self.0.starts_with("dsh-ocean-update-"));
            let _ = Command::new("/usr/bin/systemctl")
                .args(["--user", "stop", &self.0])
                .output();
        }
    }
    let mut f = Fixture::new();
    let root = f.prefix.join("generations/gen.old");
    let runtime = Runtime {
        node: root.join("runtime/node/bin/node"),
        dsh: root.join("harness/node_modules/.bin/dsh"),
        version: "0.2.0-rc.2".into(),
        install_root: Some(root.join("harness")),
    };
    let base = url::Url::parse("http://127.0.0.1:39081/").unwrap();
    f.before = dsh_ocean::systemd::render_unit(&runtime, &base);
    fs::write(&f.unit, &f.before).unwrap();
    fs::write(f.root.join("running-unit"), &f.before).unwrap();
    let task = dsh_ocean::runtime_update::prepare_at(
        &runtime,
        "0.2.1-rc.2",
        &base,
        &f.prefix,
        &f.unit,
        &f.root.join("logs"),
    )
    .unwrap();
    // Declared after f: stop this unique test unit before deleting its fixture.
    let _manager = ManagerGuard(format!("{}.service", task.unit));
    let installer = format!(
        "{}\n/usr/bin/sleep 0.7\n",
        fs::read_to_string(f.root.join("installer")).unwrap()
    );
    let mut args = Vec::new();
    for arg in task.command.as_std().get_args() {
        let arg = arg.to_string_lossy();
        if arg.starts_with("--setenv=PATH=") {
            args.push(format!("--setenv=PATH={}", f.root.join("bin").display()));
        } else if arg.starts_with("--setenv=DSH_OCEAN_INSTALL_SCRIPT=") {
            args.push(format!("--setenv=DSH_OCEAN_INSTALL_SCRIPT={installer}"));
        } else {
            if arg == "/bin/bash" {
                args.push(format!("--setenv=FIXTURE={}", f.root.display()));
            }
            args.push(arg.into_owned());
        }
    }
    let mut waiter = Command::new("/usr/bin/systemd-run")
        .args(args)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f.root.join("installer-called").exists() && Instant::now() < deadline {
        if waiter.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !f.root.join("installer-called").exists() {
        let _ = waiter.kill();
        let _ = waiter.wait();
        panic!(
            "worker did not start: {}",
            dsh_ocean::runtime_update::log_tail(&task.log)
        );
    }
    waiter.kill().unwrap();
    let _ = waiter.wait();
    while !dsh_ocean::runtime_update::log_tail(&task.log).contains("update committed")
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(dsh_ocean::runtime_update::log_tail(&task.log).contains("update committed"));
    assert_eq!(
        fs::read_link(f.prefix.join("current")).unwrap(),
        PathBuf::from("generations/gen.new")
    );
    assert!(fs::read_to_string(&f.unit).unwrap().contains("gen.new"));
}

#[test]
fn retains_recovery_snapshot_if_old_service_cannot_be_restored() {
    let f = Fixture::new();
    let result = f
        .command()
        .env("MOCK_HEALTH_FAIL", "1")
        .env("MOCK_ROLLBACK_FAIL", "1")
        .output()
        .unwrap();
    assert!(!result.status.success());
    let message = describe(&result);
    assert!(message.contains("rollback incomplete"));
    assert!(!message.contains("rollback complete;"));
    f.assert_old();
    assert!(fs::read_dir(&f.prefix).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".update.")));
}

//! Owned runtime Controller adapters: reachable authority and lifecycle.
//!
//! Systemd cases drive a fake `systemctl`/`journalctl` manager in an isolated
//! HOME/XDG/prefix; child cases drive a real private `/bin/bash` fixture and a
//! throwaway loopback token gate. Every env-sensitive case runs the current test
//! binary as a subprocess so parallel tests never share HOME/XDG/PATH.
#![cfg(target_os = "linux")]

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dsh_ocean::bootstrap::Runtime;
use dsh_ocean::runtime_controller::OwnedController;
use dsh_ocean::runtime_switch::Controller;
use dsh_ocean::service::{Server, ServerMode};
use dsh_ocean::state::AppState;
use url::Url;

static NEXT: AtomicU64 = AtomicU64::new(0);

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn generation_runtime(root: &Path) -> Runtime {
    Runtime {
        node: root.join("runtime/node/bin/node"),
        dsh: root.join("harness/node_modules/.bin/dsh"),
        version: "0.2.0-rc.2".into(),
        install_root: Some(root.join("harness")),
    }
}

/// The login home `%h` resolves to for the current account, independent of the
/// `$HOME` env override the fixture injects. Mirrors the production unit's
/// `WorkingDirectory=%h` scope.
fn account_home() -> PathBuf {
    let mut size = 16 * 1024;
    while size <= 1024 * 1024 {
        let mut buffer = vec![0u8; size];
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE {
            size *= 2;
            continue;
        }
        assert_eq!(status, 0, "getpwuid_r failed for the test account");
        assert!(!result.is_null(), "no passwd record for the test account");
        let record = unsafe { record.assume_init() };
        assert!(!record.pw_dir.is_null(), "passwd record has no home");
        let bytes = unsafe { std::ffi::CStr::from_ptr(record.pw_dir) }.to_bytes();
        return PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()));
    }
    panic!("account home lookup did not converge");
}

fn expected_home() -> PathBuf {
    account_home().join(".dsh")
}

fn server(base: Url, mode: ServerMode) -> Server {
    Server {
        base_url: base,
        token: "pre-switch-token".into(),
        mode,
        locale_patch: None,
        unit: match mode {
            ServerMode::Systemd => Some("dsh-web.service".into()),
            _ => None,
        },
    }
}

/// A throwaway loopback token gate: `GET /` returns 401 (anonymous gate) and
/// `GET /?token=<accepted>` returns 303; anything else is 401.
struct TokenGate {
    port: u16,
    stop: std::sync::Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
#[derive(Clone, Copy)]
enum AnonymousGate {
    Fixed(&'static str),
    OpensAfterToken,
}
fn serve_once(
    stream: &mut std::net::TcpStream,
    accepted: &str,
    anonymous: AnonymousGate,
    exchanged: &mut bool,
) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut raw = Vec::new();
    let mut byte = [0u8];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                raw.push(byte[0]);
                if raw.ends_with(b"\r\n\r\n") || raw.len() >= 8192 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let request = String::from_utf8_lossy(&raw);
    let status = if request.starts_with(&format!("GET /?token={accepted} HTTP/1.1")) {
        *exchanged = true;
        "303 See Other"
    } else if request.starts_with("GET / HTTP/1.1") {
        match anonymous {
            AnonymousGate::Fixed(status) => status,
            AnonymousGate::OpensAfterToken if *exchanged => "200 OK",
            AnonymousGate::OpensAfterToken => "401 Unauthorized",
        }
    } else {
        "401 Unauthorized"
    };
    let _ = stream.write_all(
        format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
    );
}
impl TokenGate {
    fn spawn(accepted: &'static str) -> Self {
        Self::spawn_with_anonymous(accepted, "401 Unauthorized")
    }
    fn spawn_with_anonymous(accepted: &'static str, anonymous: &'static str) -> Self {
        Self::spawn_policy(accepted, AnonymousGate::Fixed(anonymous))
    }
    fn spawn_policy(accepted: &'static str, anonymous: AnonymousGate) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut exchanged = false;
            while !stop_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        serve_once(&mut stream, accepted, anonymous, &mut exchanged)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            stop,
            thread: Some(thread),
        }
    }
    fn base(&self) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}/", self.port)).unwrap()
    }
    fn join(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}

struct SysFixture {
    root: PathBuf,
    prefix: PathBuf,
    unit: PathBuf,
}
impl SysFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-rc-sys-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let prefix = root.join("prefix");
        let unit = root.join("config/systemd/user/dsh-web.service");
        for name in ["home", "state", "data", "bin"] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(name))
                .unwrap();
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(unit.parent().unwrap())
            .unwrap();
        for name in ["gen.old", "gen.new"] {
            let generation = prefix.join("generations").join(name);
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(generation.join("runtime/node/bin"))
                .unwrap();
            fs::write(generation.join(".versions"), "fixture\n").unwrap();
        }
        std::os::unix::fs::symlink("generations/gen.old", prefix.join("current")).unwrap();
        executable(
            &root.join("bin/systemctl"),
            r#"#!/bin/bash
set -eu
[ "$1" = --user ] || exit 99
cmd="$2"
case "$cmd" in
  show)
    if [[ "$*" == *--property=FragmentPath* ]]; then
      if [ "${MOCK_BAD_FRAGMENT:-0}" = 1 ]; then echo /wrong/unit.service; elif [ -f "$DSH_OCEAN_UNIT_PATH" ]; then echo "$DSH_OCEAN_UNIT_PATH"; else echo; fi
    elif [[ "$*" == *--property=DropInPaths* ]]; then
      if [ "${MOCK_DROP_IN:-0}" = 1 ]; then echo "$DSH_OCEAN_UNIT_PATH.d/custom.conf"; fi
    elif [[ "$*" == *--property=ActiveState* ]]; then
      echo "is-active" >> "$FIXTURE/actions"
      if [ "${MOCK_IS_ACTIVE_UNKNOWN:-0}" = 1 ]; then exit 1; fi
      if [ "${MOCK_IS_ACTIVE_FAILED:-0}" = 1 ]; then echo "failed"; exit 0; fi
      if [ -e "$FIXTURE/active" ]; then echo "active"; else echo "inactive"; fi
    else exit 99; fi ;;
  show-environment)
    if [ "${MOCK_SHOW_ENV_FAIL:-0}" = 1 ]; then exit 1; fi
    if [ -n "${DSH_OCEAN_MANAGER_ENV_FILE:-}" ]; then
      cat "$DSH_OCEAN_MANAGER_ENV_FILE"
    else
      printf 'HOME=%s\n' "$DSH_OCEAN_LOGIN_HOME"
    fi ;;
  is-active)
    echo "is-active" >> "$FIXTURE/actions"
    if [ "${MOCK_IS_ACTIVE_UNKNOWN:-0}" = 1 ]; then exit 1; fi
    if [ -e "$FIXTURE/active" ]; then exit 0; else exit 3; fi ;;
  stop)
    echo "stop" >> "$FIXTURE/actions"
    rm -f "$FIXTURE/active"
    if [ "${MOCK_STOP_FAIL:-0}" = 1 ]; then exit 1; fi ;;
  start)
    echo "start" >> "$FIXTURE/actions"
    touch "$FIXTURE/active"
    if [ "${MOCK_START_FAIL:-0}" = 1 ]; then exit 1; fi ;;
  enable) echo "enable" >> "$FIXTURE/actions" ;;
  daemon-reload)
    echo "reload" >> "$FIXTURE/actions"
    if [ "${MOCK_RELOAD_FAIL:-0}" = 1 ]; then exit 1; fi ;;
  *) exit 99 ;;
esac
"#,
        );
        executable(
            &root.join("bin/journalctl"),
            r#"#!/bin/bash
if [ "${MOCK_DELAY_JOURNAL:-0}" = 1 ]; then
  n=0; [ ! -f "$FIXTURE/journal-count" ] || read -r n < "$FIXTURE/journal-count"
  n=$((n+1)); printf '%s\n' "$n" > "$FIXTURE/journal-count"
  [ "$n" -ge 4 ] || exit 0
fi
if [ -n "${DSH_JOURNAL_TOKEN:-}" ]; then
  printf 'dsh web: http://127.0.0.1:%s/?token=%s\n' "${DSH_AUTH_PORT:-0}" "${DSH_JOURNAL_TOKEN}"
fi
exit 0
"#,
        );
        Self { root, prefix, unit }
    }
    fn command(&self, case: &str) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "systemd_case", "--nocapture"])
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("DSH_OCEAN_DATA_DIR", &self.prefix)
            .env("DSH_OCEAN_UNIT_PATH", &self.unit)
            .env("DSH_OCEAN_LOGIN_HOME", account_home())
            .env("FIXTURE", &self.root)
            .env("DSH_OCEAN_RUNTIME_CONTROLLER_CASE", case);
        cmd
    }
}
impl Drop for SysFixture {
    fn drop(&mut self) {
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

/// ---- Systemd inner case (runs in an isolated subprocess) ----

#[test]
fn systemd_case() {
    let Some(case) = std::env::var_os("DSH_OCEAN_RUNTIME_CONTROLLER_CASE") else {
        return;
    };
    let case = case.to_string_lossy().into_owned();
    let root = PathBuf::from(std::env::var_os("FIXTURE").unwrap());
    let prefix = PathBuf::from(std::env::var_os("DSH_OCEAN_DATA_DIR").unwrap());
    let unit_path = PathBuf::from(std::env::var_os("DSH_OCEAN_UNIT_PATH").unwrap());
    let old_root = prefix.join("generations/gen.old");
    let candidate = prefix.join("generations/gen.new");
    let base = Url::parse(
        &std::env::var("DSH_OCEAN_BASE").unwrap_or_else(|_| "http://127.0.0.1:39081/".to_string()),
    )
    .unwrap();
    let old = generation_runtime(&old_root);
    let old_unit = if case == "recover-locale-health" {
        dsh_ocean::systemd::render_unit_with_patch(
            &old,
            &base,
            &root.join("saved-resource/overlay.yml"),
        )
        .unwrap()
    } else {
        dsh_ocean::systemd::render_unit(&old, &base)
    };

    // The service starts active.
    fs::write(root.join("active"), b"").unwrap();

    match case.as_str() {
        "cold-locale-refusal"
        | "cold-locale-active"
        | "cold-locale-corrupt"
        | "cold-locale-pending" => {
            let bytes = if case == "cold-locale-corrupt" {
                b"custom unit".to_vec()
            } else {
                old_unit.as_bytes().to_vec()
            };
            fs::write(&unit_path, &bytes).unwrap();
            if case != "cold-locale-active" {
                dsh_ocean::systemd::stop().unwrap();
            }
            if case == "cold-locale-pending" {
                fs::write(prefix.join(".runtime-pending.json"), "pending").unwrap();
            }
            let patch = root.join("known-resource/overlay.yml");
            assert!(dsh_ocean::systemd::ensure_unit_for_launch(
                &old,
                &base,
                Some(&patch),
                Some(&patch)
            )
            .is_err());
            assert_eq!(fs::read(&unit_path).unwrap(), bytes);
            let actions = fs::read_to_string(root.join("actions")).unwrap_or_default();
            assert!(!actions
                .lines()
                .any(|line| matches!(line, "reload" | "enable" | "start")));
        }
        "cold-locale-missing" => {
            dsh_ocean::systemd::stop().unwrap();
            assert!(!unit_path.exists());
            let patch = root.join("known-resource/overlay.yml");
            dsh_ocean::systemd::prove_cold_launch(&old, &base, Some(&patch)).unwrap();
            assert!(!unit_path.exists());
            dsh_ocean::systemd::ensure_unit_for_launch(&old, &base, Some(&patch), Some(&patch))
                .unwrap();
            assert_eq!(
                fs::read(&unit_path).unwrap(),
                dsh_ocean::systemd::render_unit_with_patch(&old, &base, &patch)
                    .unwrap()
                    .as_bytes()
            );
        }
        "cold-locale" => {
            let launch = dsh_ocean::runtime_controller::systemd_launch_scope().unwrap();
            assert_eq!(launch.cwd, account_home());
            assert_eq!(launch.dsh_home, account_home().join(".dsh"));
            fs::write(&unit_path, &old_unit).unwrap();
            dsh_ocean::systemd::stop().unwrap();
            let patch = root.join("known-resource/overlay.yml");
            dsh_ocean::systemd::ensure_unit_for_launch(&old, &base, Some(&patch), Some(&patch))
                .unwrap();
            assert_eq!(
                fs::read(&unit_path).unwrap(),
                dsh_ocean::systemd::render_unit_with_patch(&old, &base, &patch)
                    .unwrap()
                    .as_bytes()
            );
            dsh_ocean::systemd::ensure_unit_for_launch(&old, &base, None, Some(&patch)).unwrap();
            assert_eq!(fs::read(&unit_path).unwrap(), old_unit.as_bytes());
            dsh_ocean::systemd::start().unwrap();
            assert!(dsh_ocean::systemd::ensure_unit_for_launch(
                &old,
                &base,
                Some(&patch),
                Some(&patch)
            )
            .is_err());
            assert_eq!(fs::read(&unit_path).unwrap(), old_unit.as_bytes());
        }
        "locale-unit" | "locale-change" => {
            let patch = root.join("old-resource/overlay.yml");
            let captured = dsh_ocean::systemd::render_unit_with_patch(&old, &base, &patch).unwrap();
            fs::write(&unit_path, &captured).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            let mut launched = server(base.clone(), ServerMode::Systemd);
            launched.locale_patch = Some(patch.clone());
            permit.publish_server(launched).unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            if case == "locale-change" {
                c = c.with_candidate_locale_patch(None).unwrap();
            }
            c.ownership(&old_root).unwrap();
            let image = c.unit().unwrap().unwrap();
            assert_eq!(image.before, captured.as_bytes());
            assert_eq!(
                String::from_utf8_lossy(&image.after).contains(patch.to_str().unwrap()),
                case != "locale-change"
            );
            c.switch_unit(&Some(image.clone()), true).unwrap();
            c.start(&candidate).unwrap();
            c.switch_unit(&Some(image), false).unwrap();
            c.start(&old_root).unwrap();
            assert_eq!(fs::read(&unit_path).unwrap(), captured.as_bytes());
        }
        "legacy-unit" | "unsafe-umask-unit" => {
            let legacy = old_unit.replace("UMask=0077\n", "");
            let captured = if case == "legacy-unit" {
                legacy
            } else {
                legacy.replace("Type=simple\n", "Type=simple\nUMask=0000\n")
            };
            fs::write(&unit_path, &captured).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            if case == "unsafe-umask-unit" {
                assert!(c.ownership(&old_root).is_err());
                assert_eq!(fs::read(&unit_path).unwrap(), captured.as_bytes());
            } else {
                c.ownership(&old_root).unwrap();
                let image = c.unit().unwrap().unwrap();
                assert_eq!(
                    image.before,
                    captured.as_bytes(),
                    "capture exact historical bytes"
                );
                assert!(
                    String::from_utf8_lossy(&image.after)
                        .lines()
                        .any(|l| l == "UMask=0077"),
                    "new generation must create private files"
                );
                c.switch_unit(&Some(image.clone()), true).unwrap();
                c.start(&candidate).unwrap();
                c.switch_unit(&Some(image.clone()), false).unwrap();
                c.start(&old_root).unwrap();
                assert_eq!(
                    fs::read(&unit_path).unwrap(),
                    captured.as_bytes(),
                    "rollback must preserve exact legacy before-image"
                );
            }
        }
        "health-delayed-journal" | "recover-health-delayed-journal" | "recover-locale-health" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            let mut c = if case.starts_with("recover-") {
                assert!(permit.server().unwrap().is_none());
                OwnedController::headless_recover(
                    permit,
                    old_root.clone(),
                    candidate.clone(),
                    expected_home(),
                    Some(dsh_ocean::runtime_switch::UnitImage {
                        before: old_unit.as_bytes().to_vec(),
                        after: dsh_ocean::systemd::render_unit(
                            &generation_runtime(&candidate),
                            &base,
                        )
                        .into_bytes(),
                    }),
                )
                .unwrap()
            } else {
                permit
                    .publish_server(server(base.clone(), ServerMode::Systemd))
                    .unwrap();
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap()
            };
            if case == "recover-locale-health" {
                c.restore_locale_patches(Some(root.join("saved-resource/overlay.yml")), None)
                    .unwrap();
            }
            c.ownership(&old_root).unwrap();
            c.stop().unwrap();
            c.stopped().unwrap();
            c.start(&old_root).unwrap();
            c.health()
                .expect("must await a fresh authenticated journal token, not just systemctl start");
            assert_eq!(c.permit().server().unwrap().unwrap().token, "good-token");
            if case == "recover-locale-health" {
                assert_eq!(
                    c.permit().server().unwrap().unwrap().locale_patch,
                    Some(root.join("saved-resource/overlay.yml"))
                );
            }
            c.stop().unwrap();
            c.stopped().unwrap();
        }
        "attached-refused" => {
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Attached))
                .unwrap();
            let error =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .err()
                    .expect("attached servers must be refused");
            assert!(error.to_string().contains("attach"), "{error:#}");
        }
        "ownership-and-roundtrip" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            c.ownership(&old_root).unwrap();
            let image = c
                .unit()
                .unwrap()
                .expect("systemd controller has a unit image");
            assert_eq!(image.before, old_unit.as_bytes());
            assert_eq!(
                image.after,
                dsh_ocean::systemd::render_unit(&generation_runtime(&candidate), &base).as_bytes()
            );
            c.check_unit(&Some(image.clone())).unwrap();
            c.switch_unit(&Some(image.clone()), true).unwrap();
            assert_eq!(fs::read(&unit_path).unwrap(), image.after);
            c.start(&candidate).unwrap();
            c.switch_unit(&Some(image.clone()), false).unwrap();
            assert_eq!(fs::read(&unit_path).unwrap(), image.before);
            c.start(&old_root).unwrap();
            let actions = fs::read_to_string(root.join("actions")).unwrap();
            assert_eq!(actions.matches("start").count(), 2, "{actions}");
            assert_eq!(actions.matches("reload").count(), 2, "{actions}");
        }
        "external-edit" => {
            let mut edited = old_unit.clone();
            edited.push_str("# preserved user edit\n");
            fs::write(&unit_path, edited).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            let error = c
                .ownership(&old_root)
                .expect_err("an edited unit is not owned");
            assert!(error.to_string().contains("unit"), "{error:#}");
        }
        "root-mismatch" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let stale_root = prefix.join("generations/gen.other");
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(stale_root.join("runtime/node/bin"))
                .unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            let error = c
                .ownership(&stale_root)
                .expect_err("stale selection must be refused");
            assert!(error.to_string().contains("root"), "{error:#}");
        }
        "stop-before-inactive" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            c.ownership(&old_root).unwrap();
            assert!(
                c.stopped().is_err(),
                "active service must fail the inactive proof"
            );
            c.stop().unwrap();
            c.stopped().unwrap();
            let actions = fs::read_to_string(root.join("actions")).unwrap();
            let stop = actions.find("stop").unwrap();
            let inactive = actions.rfind("is-active").unwrap();
            assert!(
                stop < inactive,
                "stop must precede the inactive proof: {actions}"
            );
        }
        "health-requires-token" | "health-requires-anonymous-gate" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            c.ownership(&old_root).unwrap();
            c.stop().unwrap();
            c.stopped().unwrap();
            c.start(&old_root).unwrap();
            dsh_ocean::auth::cache_token("pre-switch-token");
            let began = std::time::Instant::now();
            let error = c
                .health()
                .expect_err("an unauthenticated service must not be healthy");
            assert!(
                began.elapsed() < Duration::from_secs(40),
                "readiness must remain bounded"
            );
            assert!(!format!("{error:#}").contains("good-token"));
            assert_eq!(
                c.permit().server().unwrap().unwrap().token,
                "pre-switch-token",
                "failed readiness must not publish a new token"
            );
            assert_eq!(
                dsh_ocean::auth::cached_token().as_deref(),
                Some("pre-switch-token"),
                "failed readiness must not overwrite the previous cache"
            );
        }
        "dropin-refused" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            let error = c
                .ownership(&old_root)
                .expect_err("a unit drop-in must refuse ownership");
            assert!(error.to_string().contains("drop-in"), "{error:#}");
        }
        "fragment-refused" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            let error = c
                .ownership(&old_root)
                .expect_err("a wrong fragment path must refuse ownership");
            assert!(error.to_string().contains("unit"), "{error:#}");
        }
        "stop-failure" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            c.ownership(&old_root).unwrap();
            let error = c
                .stop()
                .expect_err("a failed stop must refuse the transaction");
            assert!(error.to_string().contains("stop"), "{error:#}");
            // No replacement: the on-disk unit is untouched by the failed stop.
            assert_eq!(fs::read(&unit_path).unwrap(), old_unit.as_bytes());
        }
        "is-active-unknown" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            c.ownership(&old_root).unwrap();
            c.stop().unwrap();
            let error = c
                .stopped()
                .expect_err("an unknown service state must not pass the inactive proof");
            assert!(error.to_string().contains("unknown"), "{error:#}");
        }
        "health-fresh-token" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            c.ownership(&old_root).unwrap();
            c.stop().unwrap();
            c.stopped().unwrap();
            c.start(&old_root).unwrap();
            c.health().unwrap();
            let published = c.permit().server().unwrap().unwrap();
            assert_eq!(published.token, "good-token");
            assert_eq!(published.mode, ServerMode::Systemd);
        }
        "scope-mismatch" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let wrong = root.join("somewhere-else");
            fs::DirBuilder::new()
                .recursive(true)
                .create(&wrong)
                .unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let error = OwnedController::headless(permit, old.clone(), candidate.clone(), wrong)
                .err()
                .expect("a snapshot scope that differs from the service data home must be refused");
            assert!(error.to_string().contains("scope"), "{error:#}");
        }
        "dsh-home-override" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let error =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .err()
                    .expect("a DSH_HOME override must refuse an owned systemd switch");
            assert!(error.to_string().contains("DSH_HOME"), "{error:#}");
        }
        "manager-dsh-home-differs" => {
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let error =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .err()
                    .expect(
                        "a manager DSH_HOME that differs from the snapshot scope must be refused",
                    );
            assert!(error.to_string().contains("scope"), "{error:#}");
            assert!(
                !error.to_string().contains("/other"),
                "must not reflect the manager value: {error:#}"
            );
        }
        "manager-home-differs" => {
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let error =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .err()
                    .expect("a manager HOME that differs from the snapshot scope must be refused");
            assert!(error.to_string().contains("scope"), "{error:#}");
            assert!(
                !error.to_string().contains("/other"),
                "must not reflect the manager value: {error:#}"
            );
        }
        "manager-plain-matching" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .expect("a plain matching manager HOME/DSH_HOME must be accepted");
            // Ownership re-runs the manager-env proof alongside the unit check.
            c.ownership(&old_root).unwrap();
        }
        "manager-quoted" => {
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let error =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .err()
                    .expect("a quoted/escaped manager value must be refused");
            assert!(
                !error.to_string().contains("super-secret"),
                "must not reflect the synthetic value: {error:#}"
            );
        }
        "manager-query-error" => {
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let error =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .err()
                    .expect("a manager environment query failure must be refused");
            assert!(error.to_string().contains("environment"), "{error:#}");
        }
        "manager-oversize" => {
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let error =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .err()
                    .expect("an oversized manager environment must be refused");
            assert!(error.to_string().contains("too large"), "{error:#}");
        }
        "is-active-failed" => {
            fs::write(&unit_path, &old_unit).unwrap();
            let state = AppState::default();
            let permit = state.begin_update().unwrap();
            permit
                .publish_server(server(base.clone(), ServerMode::Systemd))
                .unwrap();
            let mut c =
                OwnedController::headless(permit, old.clone(), candidate.clone(), expected_home())
                    .unwrap();
            c.ownership(&old_root).unwrap();
            c.stop().unwrap();
            let error = c
                .stopped()
                .expect_err("a failed service state must not pass the inactive proof");
            assert!(error.to_string().contains("inactive"), "{error:#}");
        }
        other => panic!("unknown systemd case {other}"),
    }
}

#[test]
fn systemd_attached_is_refused() {
    let f = SysFixture::new();
    let output = f.command("attached-refused").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_ownership_unit_switch_and_start_roundtrip() {
    let f = SysFixture::new();
    let output = f.command("ownership-and-roundtrip").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_patch_recipe_keeps_exact_before_image_through_candidate_and_rollback() {
    let f = SysFixture::new();
    let output = f.command("locale-unit").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_plain_candidate_preserves_exact_patched_rollback_unit() {
    let f = SysFixture::new();
    let output = f.command("locale-change").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn cold_systemd_native_recipe_updates_only_exact_inactive_owned_unit() {
    let f = SysFixture::new();
    let output = f.command("cold-locale").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn cold_native_systemd_refusals_leave_unit_bytes_and_service_actions_untouched() {
    for flag in [
        "MOCK_DROP_IN",
        "MOCK_BAD_FRAGMENT",
        "MOCK_IS_ACTIVE_UNKNOWN",
        "MOCK_IS_ACTIVE_FAILED",
    ] {
        let f = SysFixture::new();
        let output = f
            .command("cold-locale-refusal")
            .env(flag, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}: {}", flag, describe(&output));
    }
    for case in [
        "cold-locale-active",
        "cold-locale-corrupt",
        "cold-locale-pending",
    ] {
        let f = SysFixture::new();
        let output = f.command(case).output().unwrap();
        assert!(output.status.success(), "{}", describe(&output));
    }
}

#[test]
fn cold_native_systemd_proves_missing_slot_before_creation() {
    let f = SysFixture::new();
    let output = f.command("cold-locale-missing").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_refuses_external_edit() {
    let f = SysFixture::new();
    let output = f.command("external-edit").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_refuses_a_stale_selected_root() {
    let f = SysFixture::new();
    let output = f.command("root-mismatch").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_stop_precedes_the_inactive_proof() {
    let f = SysFixture::new();
    let output = f.command("stop-before-inactive").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_waits_for_delayed_token_in_apply_and_fresh_recovery() {
    for case in [
        "health-delayed-journal",
        "recover-health-delayed-journal",
        "recover-locale-health",
    ] {
        let gate = TokenGate::spawn("good-token");
        let f = SysFixture::new();
        let output = f
            .command(case)
            .env("DSH_OCEAN_BASE", gate.base().as_str())
            .env("DSH_OCEAN_URL", gate.base().as_str())
            .env("DSH_AUTH_PORT", gate.port.to_string())
            .env("DSH_JOURNAL_TOKEN", "good-token")
            .env("MOCK_DELAY_JOURNAL", "1")
            .output()
            .unwrap();
        gate.join();
        assert!(output.status.success(), "{case}: {}", describe(&output));
    }
}

#[test]
fn legacy_generated_unit_upgrades_privacy_but_preserves_exact_rollback_image() {
    for case in ["legacy-unit", "unsafe-umask-unit"] {
        let f = SysFixture::new();
        let output = f.command(case).output().unwrap();
        assert!(output.status.success(), "{case}: {}", describe(&output));
    }
}

#[test]
fn systemd_health_requires_an_authenticated_token() {
    let f = SysFixture::new();
    let output = f.command("health-requires-token").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn token_redirect_without_anonymous_gate_cannot_publish_or_cache_success() {
    let gate = TokenGate::spawn_with_anonymous("good-token", "200 OK");
    let f = SysFixture::new();
    let output = f
        .command("health-requires-anonymous-gate")
        .env("DSH_OCEAN_BASE", gate.base().as_str())
        .env("DSH_AUTH_PORT", gate.port.to_string())
        .env("DSH_JOURNAL_TOKEN", "good-token")
        .output()
        .unwrap();
    gate.join();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn anonymous_gate_opening_after_token_exchange_cannot_publish_or_cache_success() {
    let gate = TokenGate::spawn_policy("good-token", AnonymousGate::OpensAfterToken);
    let f = SysFixture::new();
    let output = f
        .command("health-requires-anonymous-gate")
        .env("DSH_OCEAN_BASE", gate.base().as_str())
        .env("DSH_AUTH_PORT", gate.port.to_string())
        .env("DSH_JOURNAL_TOKEN", "good-token")
        .output()
        .unwrap();
    gate.join();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_health_publishes_the_fresh_token() {
    let gate = TokenGate::spawn("good-token");
    let f = SysFixture::new();
    let output = f
        .command("health-fresh-token")
        .env("DSH_OCEAN_BASE", gate.base().to_string())
        .env("DSH_AUTH_PORT", gate.port.to_string())
        .env("DSH_JOURNAL_TOKEN", "good-token")
        .output()
        .unwrap();
    gate.join();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_refuses_a_drop_in() {
    let f = SysFixture::new();
    let output = f
        .command("dropin-refused")
        .env("MOCK_DROP_IN", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_refuses_a_wrong_fragment_path() {
    let f = SysFixture::new();
    let output = f
        .command("fragment-refused")
        .env("MOCK_BAD_FRAGMENT", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_stop_failure_refuses_and_never_replaces() {
    let f = SysFixture::new();
    let output = f
        .command("stop-failure")
        .env("MOCK_STOP_FAIL", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_unknown_state_fails_the_inactive_proof() {
    let f = SysFixture::new();
    let output = f
        .command("is-active-unknown")
        .env("MOCK_IS_ACTIVE_UNKNOWN", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_refuses_a_wrong_snapshot_scope() {
    let f = SysFixture::new();
    let output = f.command("scope-mismatch").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_refuses_a_dsh_home_override() {
    let f = SysFixture::new();
    let output = f
        .command("dsh-home-override")
        .env("DSH_HOME", "/tmp/dsh-ocean-other-home")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

fn write_manager_env(root: &Path, content: &str) -> PathBuf {
    let path = root.join("manager-env");
    fs::write(&path, content).unwrap();
    path
}

#[test]
fn systemd_refuses_a_divergent_manager_dsh_home() {
    let f = SysFixture::new();
    let login = account_home();
    let content = format!("HOME={}\nDSH_HOME=/other/.dsh\n", login.display());
    let file = write_manager_env(&f.root, &content);
    let output = f
        .command("manager-dsh-home-differs")
        .env("DSH_OCEAN_MANAGER_ENV_FILE", &file)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_refuses_a_divergent_manager_home() {
    let f = SysFixture::new();
    let file = write_manager_env(&f.root, "HOME=/other\n");
    let output = f
        .command("manager-home-differs")
        .env("DSH_OCEAN_MANAGER_ENV_FILE", &file)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_accepts_a_plain_matching_manager_home_and_dsh_home() {
    let f = SysFixture::new();
    let login = account_home();
    let content = format!(
        "HOME={}\nDSH_HOME={}\n",
        login.display(),
        expected_home().display()
    );
    let file = write_manager_env(&f.root, &content);
    let output = f
        .command("manager-plain-matching")
        .env("DSH_OCEAN_MANAGER_ENV_FILE", &file)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_refuses_a_quoted_manager_value_without_reflecting_it() {
    let f = SysFixture::new();
    let login = account_home();
    // A backslash-escaped space makes this unparseable without shell evaluation;
    // the synthetic "super-secret" token must never appear in the error.
    let content = format!(
        "HOME={}\nDSH_HOME=/home/super-secret\\ path/.dsh\n",
        login.display()
    );
    let file = write_manager_env(&f.root, &content);
    let output = f
        .command("manager-quoted")
        .env("DSH_OCEAN_MANAGER_ENV_FILE", &file)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_refuses_a_manager_environment_query_failure() {
    let f = SysFixture::new();
    let output = f
        .command("manager-query-error")
        .env("MOCK_SHOW_ENV_FAIL", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_refuses_an_oversized_manager_environment() {
    let f = SysFixture::new();
    let mut big = String::new();
    while big.len() < 70 * 1024 {
        big.push_str("X=yyyyyyyyyyyyyyyyyyyyyyyyyyy\n");
    }
    let file = write_manager_env(&f.root, &big);
    let output = f
        .command("manager-oversize")
        .env("DSH_OCEAN_MANAGER_ENV_FILE", &file)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_failed_state_fails_the_inactive_proof() {
    let f = SysFixture::new();
    let output = f
        .command("is-active-failed")
        .env("MOCK_IS_ACTIVE_FAILED", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

/// ---- Child inner case (runs in an isolated subprocess) ----

#[test]
fn child_case() {
    let Some(case) = std::env::var_os("DSH_OCEAN_RUNTIME_CONTROLLER_CASE") else {
        return;
    };
    let case = case.to_string_lossy().into_owned();
    let root = PathBuf::from(std::env::var_os("FIXTURE").unwrap());
    let prefix = PathBuf::from(std::env::var_os("DSH_OCEAN_DATA_DIR").unwrap());
    let old_root = prefix.join("generations/gen.old");
    let candidate = prefix.join("generations/gen.new");
    let port: u16 = std::env::var("DSH_OCEAN_PORT").unwrap().parse().unwrap();
    let marker = root.join("marker");
    let old = generation_runtime(&old_root);

    let state = AppState::default();
    let permit = state.begin_update().unwrap();
    let old_node = old_root.join("runtime/node/bin/node");
    let old_dsh = old_root.join("harness/node_modules/.bin/dsh");
    let log: dsh_ocean::managed_child::Log = std::sync::Arc::new(|_| {});
    let recipe = if case.starts_with("recipe-") {
        Some(root.join("old-resource/overlay.yml"))
    } else {
        None
    };
    let launch_args = dsh_ocean::service::managed_arguments(recipe.as_deref()).unwrap();
    let launch_args = launch_args.iter().map(String::as_str).collect::<Vec<_>>();
    let started = tauri::async_runtime::handle()
        .block_on(dsh_ocean::managed_child::start(
            &old_node,
            &old_dsh,
            &launch_args,
            "/usr/bin:/bin",
            Duration::from_secs(10),
            log,
        ))
        .expect("old managed child starts");
    let base = started.base_url.clone();
    let child_pid = started.child.id().unwrap();
    let mut launched = server(base, ServerMode::Child);
    launched.locale_patch = recipe.clone();
    permit.publish_child(launched, started.child).unwrap();

    match case.as_str() {
        "recipe-roundtrip" | "recipe-change" | "recipe-recovery" => {
            let mut c = OwnedController::headless(
                permit,
                old.clone(),
                candidate.clone(),
                root.join("home/.dsh"),
            )
            .unwrap();
            if case != "recipe-roundtrip" {
                c = c.with_candidate_locale_patch(None).unwrap();
            }
            c.ownership(&old_root).unwrap();
            c.stop().unwrap();
            c.stopped().unwrap();
            fs::write(&marker, "").unwrap();
            c.start(&candidate).unwrap();
            let expected = if case != "recipe-roundtrip" {
                None
            } else {
                recipe.clone()
            };
            assert_eq!(c.permit().server().unwrap().unwrap().locale_patch, expected);
            assert_eq!(
                fs::read_to_string(&marker)
                    .unwrap()
                    .contains(recipe.as_ref().unwrap().to_str().unwrap()),
                expected.is_some()
            );
            if case == "recipe-recovery" {
                drop(c);
                let permit = state.begin_update().unwrap();
                c = OwnedController::headless_recover(
                    permit,
                    old_root.clone(),
                    candidate.clone(),
                    root.join("home/.dsh"),
                    None,
                )
                .unwrap();
                c.restore_locale_patches(recipe.clone(), None).unwrap();
                c.ownership(&old_root).unwrap();
            }
            c.stop().unwrap();
            c.stopped().unwrap();
            fs::write(&marker, "").unwrap();
            c.start(&old_root).unwrap();
            assert_eq!(c.permit().server().unwrap().unwrap().locale_patch, recipe);
            assert!(fs::read_to_string(&marker)
                .unwrap()
                .contains(recipe.as_ref().unwrap().to_str().unwrap()));
        }
        "stop-reaps" => {
            let mut c = OwnedController::headless(
                permit,
                old.clone(),
                candidate.clone(),
                root.join("home/.dsh"),
            )
            .unwrap();
            c.ownership(&old_root).unwrap();
            assert!(
                c.stopped().is_err(),
                "an owned child must fail the absent proof"
            );
            c.stop().unwrap();
            c.stopped().unwrap();
            c.stop().unwrap();
            c.stopped().unwrap();
        }
        "explicit-root" => {
            let mut c = OwnedController::headless(
                permit,
                old.clone(),
                candidate.clone(),
                root.join("home/.dsh"),
            )
            .unwrap();
            c.ownership(&old_root).unwrap();
            c.stop().unwrap();
            c.stopped().unwrap();
            c.start(&candidate).unwrap();
            let recorded = fs::read_to_string(&marker).unwrap();
            assert!(
                recorded.contains("gen.new/harness/node_modules/.bin/dsh"),
                "start must use the explicit candidate root, got {recorded}"
            );
            let published = c.permit().server().unwrap().unwrap();
            assert_eq!(published.mode, ServerMode::Child);
            assert_eq!(published.base_url.port(), Some(port));
        }
        "fresh-token-health" => {
            let mut c = OwnedController::headless(
                permit,
                old.clone(),
                candidate.clone(),
                root.join("home/.dsh"),
            )
            .unwrap();
            c.ownership(&old_root).unwrap();
            c.stop().unwrap();
            c.stopped().unwrap();
            c.start(&candidate).unwrap();
            c.health().unwrap();
            let published = c.permit().server().unwrap().unwrap();
            assert_eq!(published.token, "accepted");
        }
        "stop-failure-restores" => {
            let mut c = OwnedController::headless(
                permit,
                old.clone(),
                candidate.clone(),
                root.join("home/.dsh"),
            )
            .unwrap();
            c.ownership(&old_root).unwrap();
            // Reap the leader outside the controller so the controller's
            // stop_child can no longer reap it (ECHILD), forcing the failure
            // path that must restore the lease rather than discard it.
            assert_eq!(
                unsafe { libc::kill(child_pid as libc::pid_t, libc::SIGKILL) },
                0
            );
            let mut status: libc::c_int = 0;
            assert_eq!(
                unsafe { libc::waitpid(child_pid as libc::pid_t, &mut status, 0) },
                child_pid as libc::pid_t
            );
            let error = c
                .stop()
                .expect_err("a stop of an already-reaped child must fail");
            assert!(error.to_string().contains("stop"), "{error:#}");
            // The lease restored the child: ownership is retained, nothing discarded.
            assert!(
                c.permit().owned_server().unwrap().is_some(),
                "child must be restored after a failed stop"
            );
            assert!(c.stopped().is_err(), "a restored child is still owned");
        }
        other => panic!("unknown child case {other}"),
    }
}

struct ChildFixture {
    root: PathBuf,
    prefix: PathBuf,
}
impl ChildFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-rc-child-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let prefix = root.join("prefix");
        for name in ["home", "state", "data"] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(name))
                .unwrap();
        }
        for (name, token) in [("gen.old", "old-token"), ("gen.new", "accepted")] {
            let generation = prefix.join("generations").join(name);
            let node_dir = generation.join("runtime/node/bin");
            let dsh_dir = generation.join("harness/node_modules/.bin");
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&node_dir)
                .unwrap();
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dsh_dir)
                .unwrap();
            fs::write(generation.join(".versions"), "fixture\n").unwrap();
            std::os::unix::fs::symlink("/bin/bash", node_dir.join("node")).unwrap();
            executable(
                &dsh_dir.join("dsh"),
                &format!(
                    "#!/bin/bash\nprintf '%s\\n' \"$0\" \"$@\" >> \"$DSH_OCEAN_MARKER\"\nprintf 'dsh web: http://127.0.0.1:%s/?token=%s\\n' \"$DSH_OCEAN_PORT\" \"{token}\"\nexec sleep 30\n"
                ),
            );
        }
        std::os::unix::fs::symlink("generations/gen.old", prefix.join("current")).unwrap();
        Self { root, prefix }
    }
    fn command(&self, case: &str, gate: Option<&TokenGate>) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "child_case", "--nocapture"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("DSH_OCEAN_DATA_DIR", &self.prefix)
            .env("FIXTURE", &self.root)
            .env("DSH_OCEAN_MARKER", self.root.join("marker"))
            .env("DSH_OCEAN_RUNTIME_CONTROLLER_CASE", case);
        let port = gate.map(|gate| gate.port).unwrap_or(39082);
        cmd.env("DSH_OCEAN_PORT", port.to_string());
        cmd
    }
}
impl Drop for ChildFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn child_candidate_and_rollback_retain_captured_launch_patch() {
    let f = ChildFixture::new();
    let output = f.command("recipe-roundtrip", None).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn plain_candidate_does_not_replace_captured_patched_rollback_recipe() {
    let f = ChildFixture::new();
    let output = f.command("recipe-change", None).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn same_session_child_recovery_uses_saved_old_recipe_not_current_plain_candidate() {
    let f = ChildFixture::new();
    let output = f.command("recipe-recovery", None).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn child_stop_reaps_and_proves_absence() {
    let f = ChildFixture::new();
    let output = f.command("stop-reaps", None).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn child_start_uses_the_explicit_root_not_detection() {
    let f = ChildFixture::new();
    let output = f.command("explicit-root", None).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn child_health_validates_the_fresh_token() {
    let f = ChildFixture::new();
    let gate = TokenGate::spawn("accepted");
    let output = f
        .command("fresh-token-health", Some(&gate))
        .output()
        .unwrap();
    gate.join();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn child_stop_failure_restores_the_child() {
    let f = ChildFixture::new();
    let output = f.command("stop-failure-restores", None).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

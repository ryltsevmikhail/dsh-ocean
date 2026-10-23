//! Native signed installed-runtime apply/recovery orchestration, real private
//! fixtures only. Check cases authenticate a fixed-key signed manifest over a
//! fake `curl`; the opt-in apply case drives the full owned switch with a real
//! Node binary and a synthetic DSH HTTP server, asserting rollback restores the
//! original home bytes and the selector/floor. No real user profile, service,
//! npm registry or network is touched.
#![cfg(target_os = "linux")]

use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use dsh_ocean::bootstrap::Runtime;
use dsh_ocean::managed_runtime::{self, ApplyOutcome, Status};
use dsh_ocean::release_feed::hex;
use dsh_ocean::runtime_controller::OwnedController;
use dsh_ocean::runtime_switch::{Controller, UnitImage};
use dsh_ocean::service::{Server, ServerMode};
use dsh_ocean::state::AppState;
use dsh_ocean::update_control::Control;
use ed25519_dalek::{Signer, SigningKey};
use semver::Version;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

static NEXT: AtomicU64 = AtomicU64::new(0);
const DOMAIN: &[u8] = b"DSH Ocean runtime manifest v1\n";
// These fixtures exercise native candidate selection/import, not a cross-target feed.
const ARCH: &str = std::env::consts::ARCH;

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn describe(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Fixture {
    root: PathBuf,
    key: SigningKey,
    manifest: Value,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-ocean-managed-runtime-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for name in ["home", "state", "config", "data", "tools", "prefix"] {
            fs::create_dir(root.join(name)).unwrap();
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let now = Utc::now();
        let manifest = json!({"version":1,"product":"dsh-ocean-runtime","channel":"stable","sequence":9,"release":"0.2.1","publishedAt":(now-ChronoDuration::minutes(1)).to_rfc3339(),"expiresAt":(now+ChronoDuration::days(1)).to_rfc3339(),"components":{"node":"24.14.1","dsh":"0.2.1","pnpm":"10.30.3"},"stateScope":"web-profile-sessions-storages-v1","artifacts":[{"target":format!("linux-{ARCH}"),"kind":"tar.gz","url":format!("https://github.com/ryltsevmikhail/dsh-ocean/releases/download/runtime-stable-v0.2.1-r9/dsh-ocean-runtime-stable-0.2.1-9-{ARCH}.tar.gz"),"size":6,"sha256":"11".repeat(32),"nodeArchiveSha256":"22".repeat(32)}]});
        let fixture = Self {
            root,
            key: SigningKey::from_bytes(&[81; 32]),
            manifest,
        };
        fixture.write();
        fixture
    }

    fn write(&self) {
        let bytes = serde_json::to_vec(&self.manifest).unwrap();
        fs::write(self.root.join("manifest.json"), &bytes).unwrap();
        let mut message = DOMAIN.to_vec();
        message.extend_from_slice(&bytes);
        fs::write(
            self.root.join("signature.json"),
            serde_json::to_vec(&json!({"version":1,"keyId":"runtime-fixture","signatureHex":hex(&self.key.sign(&message).to_bytes())})).unwrap(),
        )
        .unwrap();
        fs::write(
            self.root.join("key.json"),
            serde_json::to_vec(&json!({"version":1,"keyId":"runtime-fixture","publicKeyHex":hex(self.key.verifying_key().as_bytes())})).unwrap(),
        )
        .unwrap();
    }

    fn write_settings(&self) {
        let source = json!({
            "key": {"version":1,"keyId":"runtime-fixture","publicKeyHex":hex(self.key.verifying_key().as_bytes())},
            "stable": {"manifestUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/runtime.json","signatureUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/runtime.sig.json","minimumSequence":1},
            "next": {"manifestUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/runtime-next.json","signatureUrl":"https://github.com/ryltsevmikhail/dsh-ocean/releases/latest/download/runtime-next.sig.json","minimumSequence":1}
        });
        let settings =
            json!({"schema":1,"shellChannel":"stable","runtimeChannel":"stable","runtime":source});
        let config = self.root.join("config/dsh-ocean");
        fs::create_dir_all(&config).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            config.join("updates.json"),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
        fs::set_permissions(
            config.join("updates.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }

    fn write_curl(&self) {
        let script = format!(
            "#!/usr/bin/python3\nfrom pathlib import Path\nimport sys\np=Path({root:?})\nu=sys.argv[sys.argv.index('--url')+1]\nif u.endswith('.sig.json'): f='signature.json'\nelif 'runtime-stable-' in u and u.endswith('.tar.gz'): f='bundle.tar.gz'\nelse: f='manifest.json'\nsys.stdout.buffer.write((p/f).read_bytes())\n",
            root = self.root
        );
        executable(&self.root.join("tools/curl"), &script);
    }

    /// The OLD managed generation: a real Node symlink plus a disposable DSH
    /// HTTP server that always answers 401/303 (so its rollback health passes).
    fn write_old_generation(&self, node: &Path) {
        let old = self.root.join("prefix/generations/gen.old");
        let node_dir = old.join("runtime/node/bin");
        let dsh_dir = old.join("harness/node_modules/.bin");
        fs::create_dir_all(&node_dir).unwrap();
        fs::create_dir_all(&dsh_dir).unwrap();
        std::os::unix::fs::symlink(node, node_dir.join("node")).unwrap();
        executable(
            &dsh_dir.join("dsh"),
            "if(process.argv.includes('--version')){console.log('0.2.0');process.exit(0);}\nconst http=require('node:http');\nconst s=http.createServer((q,r)=>{const token=new URL(q.url,'http://x').searchParams.get('token');r.statusCode=token?303:401;r.end();});\ns.listen(0,'127.0.0.1',()=>console.log('dsh web: http://127.0.0.1:'+s.address().port+'/?token=old-token'));",
        );
        fs::write(old.join(".versions"), "0.2.0\n0.2.0\n10.30.3\n22\n").unwrap();
        std::os::unix::fs::symlink("generations/gen.old", self.root.join("prefix/current"))
            .unwrap();
    }

    /// A complete signed bundle whose node is the real ELF (health-checkable),
    /// whose DSH is a disposable JS HTTP server that serves correctly only in
    /// the isolated health environment (so the owned child's post-switch health
    /// fails and forces the rollback path), and which writes synthetic changed
    /// home bytes before failing.
    fn bundle_real_node(&mut self, node: &Path) {
        let node_bytes = fs::read(node).unwrap();
        let version = Command::new(node)
            .arg("--version")
            .env_clear()
            .output()
            .unwrap();
        assert!(version.status.success());
        let node_version = String::from_utf8(version.stdout).unwrap().trim().to_owned();
        self.manifest["components"]["node"] = json!(node_version.trim_start_matches('v'));
        let node_version = self.manifest["components"]["node"]
            .as_str()
            .unwrap()
            .to_owned();

        let dsh = b"if(process.argv.includes('--version')){console.log('0.2.1');process.exit(0);}\nconst isolated=process.env.HOME&&process.env.HOME.includes('dsh-ocean-health.');\nconst fs=require('node:fs');\nif(!isolated){try{fs.mkdirSync(process.env.DSH_HOME+'/sessions',{recursive:true});}catch(e){}try{fs.writeFileSync(process.env.DSH_HOME+'/config','changed');}catch(e){}try{fs.writeFileSync(process.env.DSH_HOME+'/sessions/session','changed');}catch(e){}}\nconst http=require('node:http');\nconst s=http.createServer((q,r)=>{const token=new URL(q.url,'http://x').searchParams.get('token');r.statusCode=(isolated&&token)?303:401;r.end();});\ns.listen(0,'127.0.0.1',()=>console.log('dsh web: http://127.0.0.1:'+s.address().port+'/?token=new-token'));";
        let pnpm = b"console.log('10.30.3');".to_vec();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gzip);
            let versions = format!("{node_version}\n0.2.1\n10.30.3\n{}\n", "22".repeat(32));
            for (name, body, mode) in [
                ("runtime/node/bin/node", node_bytes.as_slice(), 0o755),
                ("harness/node_modules/.bin/dsh", dsh.as_slice(), 0o755),
                ("tools/node_modules/.bin/pnpm", pnpm.as_slice(), 0o755),
                (
                    "NOTICE",
                    b"complete-runtime-license-evidence".as_slice(),
                    0o644,
                ),
                (".versions", versions.as_bytes(), 0o644),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_mode(mode);
                header.set_size(body.len() as u64);
                header.set_cksum();
                tar.append_data(
                    &mut header,
                    format!("dsh-ocean-runtime-stable-0.2.1-9-linux-{ARCH}/{name}"),
                    body,
                )
                .unwrap();
            }
            for name in ["node-pty", "koffi"] {
                let mut header = tar::Header::new_gnu();
                header.set_mode(0o644);
                header.set_size(18);
                header.set_cksum();
                tar.append_data(&mut header,format!("dsh-ocean-runtime-stable-0.2.1-9-linux-{ARCH}/harness/node_modules/@deepseek-ai/dsh/node_modules/{name}/index.js"),b"module.exports={};".as_slice()).unwrap();
            }
            tar.finish().unwrap();
        }
        let bytes = gzip.finish().unwrap();
        self.manifest["artifacts"][0]["size"] = json!(bytes.len());
        self.manifest["artifacts"][0]["sha256"] = json!(hex(&Sha256::digest(&bytes)));
        fs::write(self.root.join("bundle.tar.gz"), bytes).unwrap();
        self.write();
    }

    /// A synthetic signed bundle (no real Node ELF): executable shim payloads and
    /// a `.versions` record matching the fixture manifest. Import/verify only
    /// require executable files with matching component versions; nothing is run.
    fn bundle_synthetic(&mut self) {
        let node = b"#!/bin/sh\necho v24.14.1\n".to_vec();
        let dsh = b"#!/bin/sh\necho 0.2.1\n".to_vec();
        let pnpm = b"#!/bin/sh\necho 10.30.3\n".to_vec();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gzip);
            let versions = format!("24.14.1\n0.2.1\n10.30.3\n{}\n", "22".repeat(32));
            for (name, body, mode) in [
                ("runtime/node/bin/node", node.as_slice(), 0o755),
                ("harness/node_modules/.bin/dsh", dsh.as_slice(), 0o755),
                ("tools/node_modules/.bin/pnpm", pnpm.as_slice(), 0o755),
                (
                    "NOTICE",
                    b"complete-runtime-license-evidence".as_slice(),
                    0o644,
                ),
                (".versions", versions.as_bytes(), 0o644),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_mode(mode);
                header.set_size(body.len() as u64);
                header.set_cksum();
                tar.append_data(
                    &mut header,
                    format!("dsh-ocean-runtime-stable-0.2.1-9-linux-{ARCH}/{name}"),
                    body,
                )
                .unwrap();
            }
            for name in ["node-pty", "koffi"] {
                let mut header = tar::Header::new_gnu();
                header.set_mode(0o644);
                header.set_size(18);
                header.set_cksum();
                tar.append_data(&mut header, format!("dsh-ocean-runtime-stable-0.2.1-9-linux-{ARCH}/harness/node_modules/@deepseek-ai/dsh/node_modules/{name}/index.js"), b"module.exports={};".as_slice()).unwrap();
            }
            tar.finish().unwrap();
        }
        let bytes = gzip.finish().unwrap();
        self.manifest["artifacts"][0]["size"] = json!(bytes.len());
        self.manifest["artifacts"][0]["sha256"] = json!(hex(&Sha256::digest(&bytes)));
        fs::write(self.root.join("bundle.tar.gz"), bytes).unwrap();
        self.write();
    }

    fn command(&self, case: &str, node: Option<&Path>) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "managed_runtime_case", "--nocapture"])
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("tools").display()),
            )
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("DSH_OCEAN_DATA_DIR", self.root.join("prefix"))
            .env("DSH_HOME", self.root.join("home/.dsh"))
            .env("FIXTURE", &self.root)
            .env("DSH_OCEAN_MANAGED_RUNTIME_CASE", case);
        if let Some(node) = node {
            cmd.env("DSH_OCEAN_TEST_RUNTIME_NODE", node);
        }
        cmd
    }

    fn write_systemctl(&self) {
        let unit = self.root.join("config/systemd/user/dsh-web.service");
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        executable(
            &self.root.join("tools/systemctl"),
            r#"#!/bin/bash
set -eu
[ "$1" = --user ] || exit 99
cmd="$2"
case "$cmd" in
  show)
    if [[ "$*" == *--property=FragmentPath* ]]; then echo "$DSH_OCEAN_UNIT_PATH";
    elif [[ "$*" == *--property=DropInPaths* ]]; then :;
    else exit 99; fi ;;
  show-environment)
    printf 'HOME=%s\n' "$DSH_OCEAN_LOGIN_HOME" ;;
  daemon-reload) : ;;
  *) exit 99 ;;
esac
"#,
        );
        executable(&self.root.join("tools/journalctl"), "#!/bin/bash\nexit 0\n");
    }

    fn command_sys(&self, case: &str) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "managed_runtime_case", "--nocapture"])
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("tools").display()),
            )
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("DSH_OCEAN_DATA_DIR", self.root.join("prefix"))
            .env(
                "DSH_OCEAN_UNIT_PATH",
                self.root.join("config/systemd/user/dsh-web.service"),
            )
            .env("DSH_OCEAN_LOGIN_HOME", account_home())
            .env("FIXTURE", &self.root)
            .env("DSH_OCEAN_MANAGED_RUNTIME_CASE", case);
        cmd
    }

    /// Full fake systemctl for the cross-session systemd recovery E2E. The
    /// manager environment is pinned to the fixture's private HOME (not the
    /// login home), so the scope proof resolves `<private HOME>/.dsh` — the same
    /// directory `config::dsh_home()` returns when `$HOME` is the fixture home.
    fn write_systemctl_full(&self) {
        let unit = self.root.join("config/systemd/user/dsh-web.service");
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        executable(
            &self.root.join("tools/systemctl"),
            r#"#!/bin/bash
set -eu
[ "$1" = --user ] || exit 99
cmd="$2"
case "$cmd" in
  show)
    if [[ "$*" == *--property=FragmentPath* ]]; then echo "$DSH_OCEAN_UNIT_PATH";
    elif [[ "$*" == *--property=DropInPaths* ]]; then :;
    elif [[ "$*" == *--property=ActiveState* ]]; then
      if [ -e "$FIXTURE/active" ]; then echo "active"; else echo "inactive"; fi
    else exit 99; fi ;;
  show-environment)
    printf 'HOME=%s\n' "$DSH_OCEAN_PRIVATE_HOME" ;;
  stop) rm -f "$FIXTURE/active" ;;
  start) touch "$FIXTURE/active" ;;
  daemon-reload) : ;;
  *) exit 99 ;;
esac
"#,
        );
        executable(
            &self.root.join("tools/journalctl"),
            "#!/bin/bash\nprintf 'dsh web: http://127.0.0.1:%s/?token=%s\\n' \"${DSH_AUTH_PORT:-0}\" \"${DSH_JOURNAL_TOKEN}\"\nexit 0\n",
        );
    }

    fn command_sys_full(&self, case: &str) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "managed_runtime_case", "--nocapture"])
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("tools").display()),
            )
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("DSH_OCEAN_DATA_DIR", self.root.join("prefix"))
            .env(
                "DSH_OCEAN_UNIT_PATH",
                self.root.join("config/systemd/user/dsh-web.service"),
            )
            .env("DSH_OCEAN_PRIVATE_HOME", self.root.join("home"))
            .env("FIXTURE", &self.root)
            .env("DSH_OCEAN_MANAGED_RUNTIME_CASE", case);
        cmd
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A throwaway loopback HTTP gate: `GET /` returns 401 (anonymous gate) and
/// `GET /?token=<accepted>` returns 303; anything else is 401.
struct TokenGate {
    port: u16,
    stop: std::sync::Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
fn serve_once(stream: &mut std::net::TcpStream, accepted: &str) {
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
        "303 See Other"
    } else {
        "401 Unauthorized"
    };
    let _ = stream.write_all(
        format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
    );
}
impl TokenGate {
    fn spawn(accepted: &'static str) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => serve_once(&mut stream, accepted),
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
    fn join(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}

/// ---- inner case (runs in an isolated subprocess) ----

#[test]
fn managed_runtime_case() {
    let Some(case) = std::env::var_os("DSH_OCEAN_MANAGED_RUNTIME_CASE") else {
        return;
    };
    let case = case.to_string_lossy().into_owned();
    let root = PathBuf::from(std::env::var_os("FIXTURE").unwrap());
    let prefix = PathBuf::from(std::env::var_os("DSH_OCEAN_DATA_DIR").unwrap());

    match case.as_str() {
        "check-unconfigured" => {
            let current = Version::parse("0.2.0").unwrap();
            let error = managed_runtime::check(&current, Arc::new(Control::default()))
                .err()
                .expect("an unconfigured runtime source must fail");
            assert!(error.to_string().contains("not configured"), "{error:#}");
            assert!(!root.join("state/dsh-ocean").exists());
            assert!(!root.join("home/.dsh").exists());
        }
        "check-signed" => {
            let current = Version::parse("0.2.0").unwrap();
            let check = managed_runtime::check(&current, Arc::new(Control::default()))
                .expect("a signed fixture must authenticate");
            assert_eq!(check.summary["release"], "0.2.1");
            assert_eq!(check.summary["sequence"], 9);
            assert_eq!(check.summary["downloaded"], false);
            assert_eq!(check.summary["keyFingerprint"], fixture_fingerprint());
            assert_eq!(check.source.channel, "stable");
            assert!(!root.join("state/dsh-ocean").exists());
            assert!(!root.join("home/.dsh").exists());
        }
        "apply-child-rollback" => {
            let home = PathBuf::from(std::env::var_os("DSH_HOME").unwrap());
            apply_child_rollback(&prefix, &home);
        }
        "recover-systemd-unit" => {
            recover_systemd_unit(&root, &prefix);
        }
        "recover-systemd-full" => {
            recover_systemd_full(&root, &prefix);
        }
        "apply-refused-attached" => {
            apply_refused_attached(&root, &prefix);
        }
        "status-current-vs-available" => {
            status_current_vs_available(&root, &prefix);
        }
        other => panic!("unknown managed_runtime case {other}"),
    }
}

/// The fixed fixture key's fingerprint, recomputed inside the subprocess so the
/// check result is compared against an independently derived value.
fn fixture_fingerprint() -> String {
    let key = SigningKey::from_bytes(&[81; 32]);
    hex(&Sha256::digest(key.verifying_key().as_bytes()))
}

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
        let bytes = unsafe { std::ffi::CStr::from_ptr(record.pw_dir) }.to_bytes();
        return PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()));
    }
    panic!("account home lookup did not converge");
}

fn generation_runtime(root: &Path) -> Runtime {
    Runtime {
        node: root.join("runtime/node/bin/node"),
        dsh: root.join("harness/node_modules/.bin/dsh"),
        version: "0.2.0".into(),
        install_root: Some(root.join("harness")),
    }
}

/// Proves the recovery constructor accepts a journal whose on-disk unit is
/// already the candidate (after) image — the interrupted-apply state that a
/// normal constructor would refuse — and restores the before image.
fn recover_systemd_unit(root: &Path, prefix: &Path) {
    let unit_path = PathBuf::from(std::env::var_os("DSH_OCEAN_UNIT_PATH").unwrap());
    let old_root = prefix.join("generations/gen.old");
    let new_root = prefix.join("generations/gen.new");
    for generation in [&old_root, &new_root] {
        fs::create_dir_all(generation.join("runtime/node/bin")).unwrap();
        fs::create_dir_all(generation.join("harness/node_modules/.bin")).unwrap();
        fs::write(generation.join(".versions"), "fixture\n").unwrap();
    }
    let base = Url::parse("http://127.0.0.1:3080/").unwrap();
    let before =
        dsh_ocean::systemd::render_unit(&generation_runtime(&old_root), &base).into_bytes();
    let after = dsh_ocean::systemd::render_unit(&generation_runtime(&new_root), &base).into_bytes();
    let image = UnitImage {
        before: before.clone(),
        after: after.clone(),
    };
    // The interrupted apply already switched the unit to the candidate.
    fs::write(&unit_path, &after).unwrap();

    let state = AppState::default();
    let permit = state.begin_update().unwrap();
    permit
        .publish_server(Server {
            base_url: base.clone(),
            token: "test-token".into(),
            mode: ServerMode::Systemd,
            unit: Some("dsh-web.service".into()),
            locale_patch: None,
        })
        .unwrap();

    let mut controller = OwnedController::headless_recover(
        permit,
        old_root.clone(),
        new_root.clone(),
        account_home().join(".dsh"),
        Some(image.clone()),
    )
    .expect("recovery controller constructs from the journal OLD runtime");
    // Ownership must accept either the before or after image, never re-render.
    controller.ownership(&old_root).unwrap();
    controller.switch_unit(&Some(image.clone()), false).unwrap();
    assert_eq!(fs::read(&unit_path).unwrap(), before);
    let _ = root;
}

fn apply_child_rollback(prefix: &Path, home: &Path) {
    let old_root = prefix.join("generations/gen.old");
    let old_node = old_root.join("runtime/node/bin/node");
    let old_dsh = old_root.join("harness/node_modules/.bin/dsh");

    // Original managed-home bytes that the rollback must restore.
    fs::create_dir_all(home.join("sessions")).unwrap();
    fs::create_dir_all(home.join("plugins/node_modules")).unwrap();
    fs::write(home.join("config"), b"original-config").unwrap();
    fs::write(home.join("sessions/session"), b"original-session").unwrap();
    fs::write(home.join("plugins/node_modules/module"), b"original-plugin").unwrap();

    // Owned managed child for the OLD generation.
    let state = AppState::default();
    let permit = state.begin_update().unwrap();
    let log: dsh_ocean::managed_child::Log = Arc::new(|_| {});
    let started = tauri::async_runtime::handle()
        .block_on(dsh_ocean::managed_child::start(
            &old_node,
            &old_dsh,
            &[
                "--profile",
                "web",
                "--host",
                "127.0.0.1",
                "--port",
                "0",
                "--no-open",
            ],
            "/usr/bin:/bin",
            Duration::from_secs(10),
            log,
        ))
        .expect("old managed child starts");
    permit
        .publish_child(
            dsh_ocean::service::Server {
                base_url: started.base_url.clone(),
                token: started.token.clone(),
                mode: dsh_ocean::service::ServerMode::Child,
                unit: None,
                locale_patch: None,
            },
            started.child,
        )
        .unwrap();

    let old = dsh_ocean::bootstrap::Runtime {
        dsh: old_dsh.clone(),
        node: old_node.clone(),
        version: "0.2.0".into(),
        install_root: Some(old_root.join("harness")),
    };

    let current = Version::parse("0.2.0").unwrap();
    let candidate = managed_runtime::check(&current, Arc::new(Control::default()))
        .expect("signed check before apply")
        .candidate;

    let outcome =
        managed_runtime::apply_native(None, permit, old, candidate, Arc::new(Control::default()))
            .expect("apply returns an outcome, not a hard error");

    match outcome {
        ApplyOutcome::RolledBack { error } => {
            assert!(error.contains("rejected"), "{error}");
        }
        ApplyOutcome::PendingRecovery { error } => {
            panic!("expected clean rollback, got pending: {error}")
        }
        ApplyOutcome::Applied { release } => panic!("expected rollback, got applied {release}"),
    }

    // Session/config/plugin bytes restored, selector back to old, no pending.
    assert_eq!(fs::read(home.join("config")).unwrap(), b"original-config");
    assert_eq!(
        fs::read(home.join("sessions/session")).unwrap(),
        b"original-session"
    );
    assert_eq!(
        fs::read(home.join("plugins/node_modules/module")).unwrap(),
        b"original-plugin"
    );
    assert_eq!(
        fs::read_link(prefix.join("current")).unwrap(),
        PathBuf::from("generations/gen.old")
    );
    assert!(!prefix.join(".runtime-pending.json").exists());
    // The floor was not advanced for a rejected candidate.
    let floor = fs::read_to_string(
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap()
            .join("dsh-ocean/runtime-release-state.json"),
    )
    .unwrap_or_default();
    assert!(
        !floor.contains("\"release\": \"0.2.1\""),
        "floor must not record a rejected candidate: {floor}"
    );
}

/// Re-sign the fixture manifest in place with the fixed test key (used to prove
/// that a same-DSH-release candidate with changed components stays available).
fn resign_manifest(root: &Path, manifest: Value) {
    let key = SigningKey::from_bytes(&[81; 32]);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(root.join("manifest.json"), &bytes).unwrap();
    let mut message = DOMAIN.to_vec();
    message.extend_from_slice(&bytes);
    fs::write(
        root.join("signature.json"),
        serde_json::to_vec(&json!({
            "version": 1,
            "keyId": "runtime-fixture",
            "signatureHex": hex(&key.sign(&message).to_bytes())
        }))
        .unwrap(),
    )
    .unwrap();
}

/// Cross-session systemd recovery end-to-end through the real `recover_native`
/// facade, with a fresh `AppState` that has NO published server: the pending
/// gate forbids starting one. Ownership is proven from the journal's captured
/// unit images and the fake manager env (pinned to the private HOME), the home
/// is restored, the selector/unit return to the old generation, and a fresh
/// token is published after the recovered OLD root starts and proves health.
fn recover_systemd_full(root: &Path, prefix: &Path) {
    let unit_path = PathBuf::from(std::env::var_os("DSH_OCEAN_UNIT_PATH").unwrap());
    let base = Url::parse(
        &std::env::var("DSH_OCEAN_URL").unwrap_or_else(|_| "http://127.0.0.1:3080/".to_owned()),
    )
    .unwrap();
    let paths = managed_runtime::runtime_paths().unwrap();
    let home = paths.home.clone();
    let backups = paths.backups.clone();
    let old_root = prefix.join("generations/gen.old");
    let new_root = prefix.join("generations/gen.new");
    for generation in [&old_root, &new_root] {
        fs::create_dir_all(generation.join("runtime/node/bin")).unwrap();
        fs::create_dir_all(generation.join("harness/node_modules/.bin")).unwrap();
        fs::write(generation.join(".versions"), "fixture\n").unwrap();
    }

    // Original private-home bytes the recovery must restore.
    fs::create_dir_all(home.join("sessions")).unwrap();
    fs::create_dir_all(home.join("plugins/node_modules")).unwrap();
    fs::write(home.join("config"), b"original-config").unwrap();
    fs::write(home.join("sessions/session"), b"original-session").unwrap();
    fs::write(home.join("plugins/node_modules/module"), b"original-plugin").unwrap();

    // Durable snapshot of the original home; then the interrupted candidate wrote.
    let snapshot = dsh_ocean::runtime_snapshot::capture(&home, &backups, || Ok(())).unwrap();
    fs::write(home.join("config"), b"changed-config").unwrap();
    fs::write(home.join("sessions/session"), b"changed-session").unwrap();
    fs::write(home.join("plugins/node_modules/module"), b"changed-plugin").unwrap();

    let before =
        dsh_ocean::systemd::render_unit(&generation_runtime(&old_root), &base).into_bytes();
    let after = dsh_ocean::systemd::render_unit(&generation_runtime(&new_root), &base).into_bytes();
    let image = UnitImage {
        before: before.clone(),
        after: after.clone(),
    };

    // Interrupted-apply state: selector on the candidate, unit on disk already
    // the candidate image.
    std::os::unix::fs::symlink("generations/gen.new", prefix.join("current")).unwrap();
    fs::write(&unit_path, &after).unwrap();

    let journal = json!({
        "schema": 1,
        "home": home,
        "backups": backups,
        "old": "generations/gen.old",
        "new": "generations/gen.new",
        "snapshot": snapshot.path(),
        "unit": serde_json::to_value(&image).unwrap(),
    });
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(prefix.join(".runtime-pending.json"))
        .unwrap();
    file.write_all(&serde_json::to_vec(&journal).unwrap())
        .unwrap();
    file.sync_all().unwrap();
    drop(file);

    // Fresh session: no server published at all.
    let state = AppState::default();
    let permit = state.begin_update().unwrap();
    let outcome = managed_runtime::recover_native(None, permit, Arc::new(Control::default()))
        .expect("systemd recovery must succeed without a live server");
    assert!(matches!(
        outcome,
        managed_runtime::RecoverOutcome::Recovered {
            refresh_error: None
        }
    ));

    assert_eq!(fs::read(home.join("config")).unwrap(), b"original-config");
    assert_eq!(
        fs::read(home.join("sessions/session")).unwrap(),
        b"original-session"
    );
    assert_eq!(
        fs::read(home.join("plugins/node_modules/module")).unwrap(),
        b"original-plugin"
    );
    assert_eq!(
        fs::read_link(prefix.join("current")).unwrap(),
        PathBuf::from("generations/gen.old")
    );
    assert_eq!(fs::read(&unit_path).unwrap(), before);
    assert!(!prefix.join(".runtime-pending.json").exists());

    let published = state
        .server()
        .expect("the recovered OLD root published a fresh server");
    assert_eq!(published.mode, ServerMode::Systemd);
    assert_eq!(published.token, "recovered-token");
    let _ = root;
}

/// An attached (generation-managed) server must refuse `apply_native` before any
/// download, so no orphaned generation or private download cache is left behind.
fn apply_refused_attached(root: &Path, prefix: &Path) {
    let old_root = prefix.join("generations/gen.old");
    fs::create_dir_all(old_root.join("runtime/node/bin")).unwrap();
    fs::create_dir_all(old_root.join("harness/node_modules/.bin")).unwrap();
    fs::write(old_root.join(".versions"), "fixture\n").unwrap();
    let old = generation_runtime(&old_root);

    let current = Version::parse("0.2.0").unwrap();
    let candidate = managed_runtime::check(&current, Arc::new(Control::default()))
        .expect("signed check before refused apply")
        .candidate;

    let state = AppState::default();
    let permit = state.begin_update().unwrap();
    permit
        .publish_server(Server {
            base_url: Url::parse("http://127.0.0.1:3080/").unwrap(),
            token: "t".into(),
            mode: ServerMode::Attached,
            unit: None,
            locale_patch: None,
        })
        .unwrap();

    let error =
        managed_runtime::apply_native(None, permit, old, candidate, Arc::new(Control::default()))
            .err()
            .expect("an attached server must refuse apply before download");
    assert!(error.to_string().contains("attach"), "{error:#}");

    let state_dir = PathBuf::from(std::env::var_os("XDG_STATE_HOME").unwrap()).join("dsh-ocean");
    assert!(
        !state_dir.join("runtime-cache").exists(),
        "an attached apply must not download"
    );
    let _ = root;
}

/// A selected generation whose receipt matches the signed candidate is Current;
/// a same-DSH-release candidate with changed components (pnpm) stays Available.
fn status_current_vs_available(root: &Path, prefix: &Path) {
    let current = Version::parse("0.2.0").unwrap();
    let candidate = managed_runtime::check(&current, Arc::new(Control::default()))
        .expect("signed check")
        .candidate;
    let verified = candidate.verified();

    let state_dir = PathBuf::from(std::env::var_os("XDG_STATE_HOME").unwrap()).join("dsh-ocean");
    fs::create_dir_all(&state_dir).unwrap();
    let staged = dsh_ocean::release_download::download_runtime_controlled(
        verified,
        &state_dir.join("artifact.tar.gz"),
        Arc::new(Control::default()),
    )
    .expect("download synthetic bundle");
    let prepared = dsh_ocean::runtime_bundle::import(staged.path(), verified, prefix)
        .expect("import synthetic bundle");
    let generation = prepared
        .root()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    std::os::unix::fs::symlink(format!("generations/{generation}"), prefix.join("current"))
        .unwrap();

    let recheck = managed_runtime::check(&current, Arc::new(Control::default())).unwrap();
    assert_eq!(recheck.status, Status::Current);

    let mut manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    manifest["components"]["pnpm"] = json!("10.30.4");
    resign_manifest(root, manifest);
    let recheck2 = managed_runtime::check(&current, Arc::new(Control::default())).unwrap();
    assert_eq!(recheck2.status, Status::Available);
}

/// ---- outer tests ----

#[test]
fn check_requires_a_configured_runtime_source() {
    let f = Fixture::new();
    let output = f.command("check-unconfigured", None).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn signed_check_reports_candidate_without_install() {
    let f = Fixture::new();
    f.write_settings();
    f.write_curl();
    let output = f.command("check-signed", None).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
fn systemd_recovery_proves_ownership_from_the_journal_unit() {
    let f = Fixture::new();
    f.write_systemctl();
    let output = f.command_sys("recover-systemd-unit").output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

/// The full owned apply with a real Node binary and a synthetic DSH server.
/// Requires `DSH_OCEAN_TEST_RUNTIME_NODE` (a real node) and is opt-in: the
/// fixture signs a complete bundle whose node is the real ELF, the DSH is a
/// disposable JS HTTP server, and the whole switch + rollback is driven through
/// the production headless OwnedController.
#[test]
#[ignore = "requires DSH_OCEAN_TEST_RUNTIME_NODE; synthetic DSH, no user runtime"]
fn signed_apply_switches_and_rolls_back_with_real_node() {
    let node = match std::env::var_os("DSH_OCEAN_TEST_RUNTIME_NODE") {
        Some(node) => PathBuf::from(node),
        None => return,
    };
    let mut f = Fixture::new();
    f.write_old_generation(&node);
    f.bundle_real_node(&node);
    f.write_settings();
    f.write_curl();
    let output = f
        .command("apply-child-rollback", Some(&node))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

/// H1: cross-session systemd recovery through the real `recover_native` facade
/// with a fresh `AppState` (no published server) restores the private home, the
/// old selector/unit and an authenticated fresh token. No real user home.
#[test]
fn systemd_recovery_restores_home_and_selects_the_old_unit() {
    let gate = TokenGate::spawn("recovered-token");
    let f = Fixture::new();
    f.write_systemctl_full();
    let output = f
        .command_sys_full("recover-systemd-full")
        .env("DSH_OCEAN_URL", format!("http://127.0.0.1:{}/", gate.port))
        .env("DSH_AUTH_PORT", gate.port.to_string())
        .env("DSH_JOURNAL_TOKEN", "recovered-token")
        .output()
        .unwrap();
    gate.join();
    assert!(output.status.success(), "{}", describe(&output));
}

/// M1: an attached server is refused before any download/import (no orphaned
/// generation, no private download cache).
#[test]
fn attached_apply_is_refused_before_download() {
    let f = Fixture::new();
    f.write_settings();
    f.write_curl();
    let output = f.command("apply-refused-attached", None).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

/// M2: a selected generation matching the signed candidate is Current; changed
/// components (same DSH release) remain Available.
#[test]
fn status_distinguishes_current_from_available() {
    let mut f = Fixture::new();
    f.write_settings();
    f.write_curl();
    f.bundle_synthetic();
    let output = f
        .command("status-current-vs-available", None)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

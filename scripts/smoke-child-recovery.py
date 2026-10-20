#!/usr/bin/env python3
"""Real Ocean/WebKit child recovery smoke; run under test-native.sh + xvfb-run.

Only the executable argument is accepted. No host session/environment is forwarded.
Diagnostics deliberately exclude application output and fixture token URLs.
"""
import argparse
import ctypes
from dataclasses import dataclass
import http.server
import json
import os
from pathlib import Path
import re
import signal
import stat
import subprocess
import sys
import threading
import time


class SmokeError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise SmokeError(message)


def private_directory(path):
    require(path.is_absolute() and path.resolve() == path, "noncanonical private directory")
    info = path.lstat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid()
            and stat.S_IMODE(info.st_mode) == 0o700, "unsafe private directory")


def validate_context(env):
    runtime = Path(env.get("XDG_RUNTIME_DIR", ""))
    require(runtime.is_absolute() and runtime.name == "runtime"
            and runtime.parent.parent == Path("/tmp")
            and re.fullmatch(r"dsh-ocean-native-test\.[A-Za-z0-9_-]+", runtime.parent.name),
            "requires test-native runtime")
    private_directory(runtime.parent)
    private_directory(runtime)
    require(env.get("DSH_OCEAN_TEST_PRIVATE_BUS") == "1"
            and env.get("DBUS_SESSION_BUS_ADDRESS", "").startswith("unix:"), "requires private bus")
    require(env.get("DSH_OCEAN_TEST_XVFB") == "1"
            and re.fullmatch(r":[1-9][0-9]*(?:\.0)?", env.get("DISPLAY", "")), "requires private Xvfb display")
    authority = Path(env.get("XAUTHORITY", ""))
    require(authority.is_absolute() and authority.resolve() == authority
            and authority.parent.name.startswith("xvfb-run."), "requires xvfb-run authority")
    info = authority.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid()
            and not info.st_mode & 0o077, "unsafe Xauthority")
    private_directory(authority.parent)
    return runtime


def executable(value):
    path = Path(value)
    require(path.is_absolute() and path.is_file() and os.access(path, os.X_OK), "requires absolute executable")
    return path.resolve()


def create_root(runtime):
    root = runtime / "gui-child"
    root.mkdir(mode=0o700)  # Exclusive: never reuse or remove pre-existing state.
    for name in ("home", "dsh", "config", "data", "state", "cache", "ocean-data", "tmp", "tools"):
        (root / name).mkdir(mode=0o700)
    return root


@dataclass(frozen=True)
class Identity:
    pid: int
    ppid: int
    pgid: int
    start: int
    state: str


def proc_identity(pid):
    try:
        path = Path("/proc") / str(pid)
        if path.stat().st_uid != os.getuid():
            return None
        text = (path / "stat").read_text()
        fields = text[text.rindex(")") + 2:].split()
        return Identity(pid, int(fields[1]), int(fields[2]), int(fields[19]), fields[0])
    except (FileNotFoundError, ProcessLookupError):
        return None


def same_process(identity):
    current = proc_identity(identity.pid)
    return current is not None and current.start == identity.start


def live(identity):
    current = proc_identity(identity.pid)
    return current is not None and current.start == identity.start and current.state not in ("Z", "X")


def children(pid):
    # Forks belong to individual tasks: Tokio/WebKit may spawn off the main thread.
    # Enumerate only this owned process's task lists, never host /proc or groups.
    result = set()
    try:
        tasks = list(Path(f"/proc/{pid}/task").iterdir())
    except FileNotFoundError:
        return []
    for task in tasks:
        try:
            result.update(int(value) for value in (task / "children").read_text().split())
        except FileNotFoundError:
            continue  # A task exited while its process was being inspected.
    return sorted(result)


def descendants(pid):
    result = []
    for child in children(pid):
        identity = proc_identity(child)
        if identity is not None and identity.ppid == pid:
            result.append(identity)
            result.extend(descendants(child))
    return result


def reap(exclude=None):
    # Never steal the monitored application's wait status from Popen.
    for pid in children(os.getpid()):
        if pid == exclude:
            continue
        try:
            os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            pass


def subreaper():
    require(sys.platform == "linux", "requires Linux")
    require(ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0, "cannot enable child subreaper")


def owned_signal(identity, sig):
    current = proc_identity(identity.pid)
    require(current is not None and current.start == identity.start
            and current.ppid == os.getpid(), "refusing non-child signal")
    os.kill(identity.pid, sig)


def cleanup(app_identity, timeout=5):
    """Failure-only cleanup of our app group and directly adopted child groups."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        direct = [proc_identity(pid) for pid in children(os.getpid())]
        groups = set()
        for identity in direct:
            if identity is None or identity.ppid != os.getpid():
                continue
            # A live/unreaped direct group leader pins this owned PGID. A
            # non-leader's group is not independently authorized: kill that
            # individual child below, then collect any newly adopted children.
            if identity.pgid == identity.pid and identity.pgid != os.getpgrp():
                groups.add(identity.pgid)
        for group in groups:
            require(group != os.getpgrp(), "refusing controller group signal")
            try:
                os.killpg(group, signal.SIGKILL)
            except ProcessLookupError:
                pass
        # Non-leader adopted children may retain a dead fixture leader's group.
        for identity in direct:
            if identity and same_process(identity) and identity.ppid == os.getpid():
                try:
                    owned_signal(identity, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        reap()
        if not children(os.getpid()):
            return
        time.sleep(0.05)
    raise SmokeError("owned process cleanup timed out")


def window_ids(text):
    return [int(value, 16) for value in re.findall(r'^\s*(0x[0-9a-fA-F]+)\s+"DSH Ocean"(?:\s|:)', text, re.M)]


def visible_windows(env):
    result = subprocess.run(["/usr/bin/xwininfo", "-root", "-tree"], env=env,
                            capture_output=True, timeout=2, check=True)
    visible = []
    for window in window_ids(result.stdout.decode("utf-8", "replace")):
        detail = subprocess.run(["/usr/bin/xwininfo", "-id", hex(window)], env=env,
                                capture_output=True, timeout=2, check=True)
        if re.search(rb"Map State:\s*IsViewable\b", detail.stdout):
            visible.append(window)
    return visible


def read_record(root, kind, generation):
    path = root / kind / f"{generation}.json"
    if not path.exists():
        return None
    require(not path.is_symlink() and path.stat().st_size <= 4096, "unsafe fixture record")
    try:
        return json.loads(path.read_text())
    except json.JSONDecodeError:
        return None  # Atomic fixture writes preferred; tolerate publication races.


def validate_server(record, generation, app):
    require(isinstance(record, dict) and record.get("generation") == generation, "bad server generation")
    for key in ("pid", "ppid", "pgid", "port", "sentinel_pid"):
        require(type(record.get(key)) is int, "bad server metadata")
    require(record["ppid"] == app.pid and record["pgid"] == record["pid"]
            and 1024 <= record["port"] <= 65535 and record["port"] != 3080, "bad server isolation")
    leader = proc_identity(record["pid"])
    sentinel = proc_identity(record["sentinel_pid"])
    require(leader is not None and leader.ppid == app.pid and leader.pgid == leader.pid
            and sentinel is not None and sentinel.ppid == leader.pid and sentinel.pgid == leader.pgid,
            "server process ownership mismatch")
    return leader, sentinel


class Probe(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.queries += 1
        # No raw request data is retained or printed.
        if "?" in self.path or self.headers.get("Authorization") or self.headers.get("Cookie"):
            self.server.token_seen = True
        self.send_response(503)
        self.end_headers()

    do_POST = do_GET

    def log_message(self, *args):
        pass


def app_environment(root, runtime, inherited, probe_port):
    env = {"PATH": str(root / "tools") + ":/usr/bin:/bin", "HOME": str(root / "home"),
           "DSH_HOME": str(root / "dsh"), "TMPDIR": str(root / "tmp"),
           "XDG_RUNTIME_DIR": str(runtime), "DSH_OCEAN_DATA_DIR": str(root / "ocean-data"),
           "DSH_OCEAN_NODE": str(executable(sys.executable)),
           "DSH_OCEAN_DSH_BIN": str(Path(__file__).resolve().with_name("gui-child-fixture.py")),
           "DSH_OCEAN_TEST_GUI_CHILD": "1", "DSH_OCEAN_TEST_GUI_CHILD_ROOT": str(root),
           "DSH_OCEAN_URL": f"http://127.0.0.1:{probe_port}/", "DO_NOT_TRACK": "1",
           "DSH_TELEMETRY": "0", "DSH_TELEMETRY_DISABLED": "1", "NO_AT_BRIDGE": "1",
           "GIO_USE_VFS": "local", "GSETTINGS_BACKEND": "memory", "GDK_BACKEND": "x11",
           "LIBGL_ALWAYS_SOFTWARE": "1", "WEBKIT_DISABLE_DMABUF_RENDERER": "1",
           "WEBKIT_DISABLE_COMPOSITING_MODE": "1", "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS": "1"}
    for key in ("DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS"):
        env[key] = inherited[key]
    for key, name in (("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"),
                      ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache")):
        env[key] = str(root / name)
    return env


def install_tools(root):
    body = (f"#!{executable(sys.executable)}\nimport os,sys\nfrom pathlib import Path\n"
            "name=Path(sys.argv[0]).name\n"
            "if name=='systemctl' and sys.argv[1:]==['--user','is-system-running']:\n"
            " print('Failed to connect to bus: unavailable',file=sys.stderr);sys.exit(1)\n"
            f"with open({str(root / 'violation')!r},'a') as f:f.write('forbidden tool invocation\\n')\n"
            "sys.exit(97)\n")
    for name in ("systemctl", "journalctl", "xdg-open", "npm", "pnpm"):
        path = root / "tools" / name
        path.write_text(body)
        path.chmod(0o700)


def run(binary):
    inherited = dict(os.environ)
    runtime = validate_context(inherited)
    binary = executable(binary)
    fixture = executable(str(Path(__file__).resolve().with_name("gui-child-fixture.py")))
    require(fixture.is_file(), "fixture missing")
    subreaper()  # This script is a dedicated process; never imported runner state.
    root = create_root(runtime)
    install_tools(root)
    config = root / "config" / "dsh-ocean"
    config.mkdir(mode=0o700)
    (config / "config.json").write_text('{"closeToTray":false}\n')
    probe = http.server.HTTPServer(("127.0.0.1", 0), Probe)
    probe.queries = 0
    probe.token_seen = False
    require(probe.server_port != 3080, "unexpected probe port")
    thread = threading.Thread(target=probe.serve_forever, daemon=True)
    thread.start()
    env = app_environment(root, runtime, inherited, probe.server_port)
    deadline = time.monotonic() + 110
    app = None
    app_identity = None
    success = False

    def check():
        require(time.monotonic() < deadline, "smoke deadline exceeded")
        require(not probe.token_seen, "configured probe received credentials")
        require(not (root / "violation").exists(), "forbidden tool invoked")
        require(app.poll() is None, "application exited prematurely")

    def wait_for(predicate):
        while True:
            check()
            value = predicate()
            if value:
                return value
            time.sleep(0.1)

    try:
        print("child recovery: launch", flush=True)
        with (root / "app-output.log").open("xb") as output:
            app = subprocess.Popen([str(binary)], cwd=root, env=env, stdout=output,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        app_identity = proc_identity(app.pid)
        require(app_identity is not None and app_identity.ppid == os.getpid()
                and app_identity.pgid == app.pid, "application ownership mismatch")

        def rendered(generation):
            value = read_record(root, "rendered", generation)
            return isinstance(value, dict) and value.get("generation") == generation and value.get("rendered") is True

        wait_for(lambda: rendered(1))
        first = wait_for(lambda: read_record(root, "servers", 1))
        leader, sentinel = validate_server(first, 1, app)
        def single_window():
            windows = visible_windows(env)
            return windows[0] if len(windows) == 1 else None

        window = wait_for(single_window)
        check()
        print("child recovery: first render; terminate server leader", flush=True)
        require(same_process(app_identity) and same_process(leader), "process identity changed")
        current = proc_identity(leader.pid)
        require(current.ppid == app.pid and current.pgid == leader.pid, "server no longer owned")
        os.kill(leader.pid, signal.SIGKILL)  # Deliberately NOT killpg: sentinel must expose group leaks.
        require(live(sentinel), "sentinel did not survive leader-only kill")
        wait_for(lambda: rendered(2))
        second = wait_for(lambda: read_record(root, "servers", 2))
        new_leader, new_sentinel = validate_server(second, 2, app)
        # Port 0 may reuse a freed port. PID/generation and the authenticated
        # JavaScript beacon, not port inequality, establish replacement/reload.
        require(new_leader.pid != leader.pid and second.get("marker_preserved") is True,
                "recovery state not preserved")
        wait_for(lambda: visible_windows(env) == [window])
        reap(exclude=app.pid)
        check()
        require(not live(leader) and not live(sentinel), "old server group survived recovery")
        # Check only owned/adopted descendants, not global process tables.
        require(not any(item.pgid == leader.pgid and item.state not in ("Z", "X")
                        for item in descendants(os.getpid())), "old server group still live")
        print("child recovery: recovered; close main window", flush=True)
        close_env = dict(env, DSH_OCEAN_TEST_X11="1")
        subprocess.run([sys.executable, str(Path(__file__).with_name("test-x11-close.py")), hex(window)],
                       env=close_env, capture_output=True, check=True, timeout=3)
        remaining = max(0.1, min(15, deadline - time.monotonic()))
        require(app.wait(timeout=remaining) == 0, "application close failed")
        # No success-path killing: give orphan exit/reaping a bounded grace period.
        end = min(deadline, time.monotonic() + 3)
        while time.monotonic() < end:
            reap()
            if not descendants(os.getpid()):
                break
            time.sleep(0.05)
        require(not descendants(os.getpid()) and not live(new_leader) and not live(new_sentinel),
                "application descendants survived close")
        require(not probe.token_seen and not (root / "violation").exists(), "isolation violation")
        require(probe.queries > 0, "configured probe was not exercised")
        success = True
        print("child recovery: PASS", flush=True)
    finally:
        if not success:
            cleanup(app_identity)
        probe.shutdown()
        probe.server_close()
        thread.join(timeout=1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", help="absolute path to real Ocean executable")
    arguments = parser.parse_args()
    os.umask(0o077)

    def interrupted(*_):
        raise SmokeError("interrupted")

    signal.signal(signal.SIGTERM, interrupted)
    try:
        run(arguments.binary)
    except (SmokeError, OSError, subprocess.SubprocessError, ValueError):
        # Exception strings can contain command output or credentials; never print them.
        print("child recovery: FAIL (private diagnostics not printed)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

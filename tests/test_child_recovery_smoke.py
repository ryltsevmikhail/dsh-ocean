"""No-GUI safety checks for the real-binary child recovery controller."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/smoke-child-recovery.py"
SPEC = importlib.util.spec_from_file_location("child_recovery_smoke", SCRIPT)
smoke = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = smoke
SPEC.loader.exec_module(smoke)


class ChildRecoverySmoke(unittest.TestCase):
    def context(self, temporary, authority):
        runtime = Path(temporary) / "runtime"
        runtime.mkdir(mode=0o700)
        auth = Path(authority) / "Xauthority"
        auth.touch(mode=0o600)
        return {"XDG_RUNTIME_DIR": str(runtime), "DSH_OCEAN_TEST_PRIVATE_BUS": "1",
                "DBUS_SESSION_BUS_ADDRESS": "unix:path=/private/bus", "DSH_OCEAN_TEST_XVFB": "1",
                "DISPLAY": ":99", "XAUTHORITY": str(auth)}

    def test_context_and_root_are_exclusive(self):
        # Python tempfile names may contain underscores; keep this deterministic.
        with tempfile.TemporaryDirectory(prefix="dsh-ocean-native-test.valid_", dir="/tmp") as temporary, \
                tempfile.TemporaryDirectory(prefix="xvfb-run.", dir="/tmp") as authority:
            env = self.context(temporary, authority)
            runtime = smoke.validate_context(env)
            root = smoke.create_root(runtime)
            self.assertEqual(root.stat().st_mode & 0o777, 0o700)
            self.assertTrue((root / "ocean-data").is_dir())
            with self.assertRaises(FileExistsError):
                smoke.create_root(runtime)
            for key, value in (("DISPLAY", ":0"), ("DISPLAY", "host:99"),
                               ("DSH_OCEAN_TEST_PRIVATE_BUS", "0"), ("DSH_OCEAN_TEST_XVFB", "0"),
                               ("DBUS_SESSION_BUS_ADDRESS", "tcp:host=remote")):
                with self.subTest(key=key, value=value), self.assertRaises(smoke.SmokeError):
                    smoke.validate_context(dict(env, **{key: value}))

    def test_rejects_symlink_and_public_runtime_or_authority(self):
        with tempfile.TemporaryDirectory(prefix="dsh-ocean-native-test.", dir="/tmp") as temporary, \
                tempfile.TemporaryDirectory(prefix="xvfb-run.", dir="/tmp") as authority:
            env = self.context(temporary, authority)
            runtime = Path(env["XDG_RUNTIME_DIR"])
            runtime.chmod(0o755)
            with self.assertRaises(smoke.SmokeError):
                smoke.validate_context(env)
            runtime.chmod(0o700)
            auth = Path(env["XAUTHORITY"])
            auth.chmod(0o644)
            with self.assertRaises(smoke.SmokeError):
                smoke.validate_context(env)
            auth.chmod(0o600)
            link = Path(authority) / "link"
            link.symlink_to(auth)
            with self.assertRaises(smoke.SmokeError):
                smoke.validate_context(dict(env, XAUTHORITY=str(link)))

    def test_binary_validator(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "binary"
            path.write_text("not executable")
            for value in ("relative", temporary, str(path)):
                with self.assertRaises(smoke.SmokeError):
                    smoke.executable(value)
            path.chmod(0o700)
            self.assertEqual(smoke.executable(str(path)), path)

    def test_children_collects_all_task_lists(self):
        with tempfile.TemporaryDirectory() as temporary:
            task_root = Path(temporary)
            for name, contents in (("101", "501 502"), ("102", "502 503")):
                (task_root / name).mkdir()
                (task_root / name / "children").write_text(contents)
            with patch.object(smoke, "Path", return_value=task_root):
                self.assertEqual(smoke.children(100), [501, 502, 503])

    def test_cleanup_never_signals_a_nonleader_childs_group(self):
        identity = smoke.Identity(123, os.getpid(), os.getpgrp() + 100000, 1, "S")
        with patch.object(smoke, "children", side_effect=[[identity.pid], []]), \
                patch.object(smoke, "proc_identity", return_value=identity), \
                patch.object(smoke, "reap"), \
                patch.object(smoke, "owned_signal") as send, \
                patch.object(smoke.os, "killpg") as group:
            smoke.cleanup(None, timeout=1)
        group.assert_not_called()
        send.assert_called_once_with(identity, smoke.signal.SIGKILL)

    def test_window_parser_is_exact(self):
        tree = '''  0x123 "DSH Ocean": ("dsh-ocean" "DSH Ocean") 800x600+0+0
  0x456 "Other DSH Ocean": ()
  0x789 "DSH Ocean": ()
  0x999 "DSH Ocean impostor": ()
'''
        self.assertEqual(smoke.window_ids(tree), [0x123, 0x789])

    def test_environment_is_allowlisted(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inherited = {"DISPLAY": ":99", "XAUTHORITY": "/private/auth",
                         "DBUS_SESSION_BUS_ADDRESS": "unix:path=/private/bus",
                         "OPENAI_API_KEY": "secret", "DSH_TOKEN": "secret", "PATH": "/host/tools"}
            env = smoke.app_environment(root, root / "runtime", inherited, 54321)
            self.assertNotIn("OPENAI_API_KEY", env)
            self.assertNotIn("DSH_TOKEN", env)
            self.assertNotIn("/host/tools", env["PATH"])
            self.assertEqual(env["DSH_OCEAN_URL"], "http://127.0.0.1:54321/")
            self.assertEqual(env["DSH_OCEAN_NODE"], str(Path(sys.executable).resolve()))
            self.assertEqual(env["HOME"], str(root / "home"))

    def test_process_identity_and_refusal_to_signal_nonchild(self):
        identity = smoke.proc_identity(os.getpid())
        self.assertEqual(identity.pid, os.getpid())
        self.assertTrue(smoke.same_process(identity))
        stale = smoke.Identity(identity.pid, identity.ppid, identity.pgid, identity.start + 1, identity.state)
        self.assertFalse(smoke.same_process(stale))
        with patch.object(smoke.os, "kill") as kill:
            with self.assertRaises(smoke.SmokeError):
                smoke.owned_signal(identity, 9)
            kill.assert_not_called()

    def test_server_ownership_validation(self):
        app = type("App", (), {"pid": 20})()
        record = dict(generation=1, pid=30, ppid=20, pgid=30, port=23456, sentinel_pid=31)
        identities = {30: smoke.Identity(30, 20, 30, 1, "S"), 31: smoke.Identity(31, 30, 30, 2, "S")}
        with patch.object(smoke, "proc_identity", side_effect=identities.get):
            self.assertEqual(smoke.validate_server(record, 1, app), (identities[30], identities[31]))
            for change in ({"ppid": 99}, {"port": 3080}, {"pid": True}, {"generation": 2}, {"pgid": 99}):
                with self.subTest(change=change), self.assertRaises(smoke.SmokeError):
                    smoke.validate_server(dict(record, **change), 1, app)
            identities[31] = smoke.Identity(31, 99, 30, 2, "S")
            with self.assertRaises(smoke.SmokeError):
                smoke.validate_server(record, 1, app)

    @unittest.skipUnless(sys.platform == "linux", "Linux child subreaper")
    def test_subreaper_cleanup_own_synthetic_group_in_dedicated_process(self):
        # Subreaper state must not alter the unittest runner. The synthetic leader
        # exits first, leaving the sleep adopted by this dedicated controller.
        code = '''
import importlib.util, os, subprocess, sys, time
spec=importlib.util.spec_from_file_location('smoke',sys.argv[1])
m=importlib.util.module_from_spec(spec);sys.modules['smoke']=m;spec.loader.exec_module(m)
m.subreaper()
p=subprocess.Popen([sys.executable,'-c', 'import subprocess; subprocess.Popen(["/bin/sleep","120"])'], start_new_session=True)
identity=m.proc_identity(p.pid)
p.wait(timeout=3)
end=time.monotonic()+3
while not m.children(os.getpid()) and time.monotonic()<end: time.sleep(.01)
assert m.children(os.getpid()), 'sentinel not adopted'
m.cleanup(identity, timeout=3)
assert not m.children(os.getpid()), 'owned children survived'
'''
        result = subprocess.run([sys.executable, "-B", "-c", code, str(SCRIPT)], capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_guard_failure_never_launches(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(smoke.subprocess, "Popen") as launch:
            with self.assertRaises(smoke.SmokeError):
                smoke.run("/bin/true")
            launch.assert_not_called()


if __name__ == "__main__":
    unittest.main()

"""Synthetic tool only: output is a fixture header, NOT real squashfs acceptance."""
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import runpy
import shutil
import subprocess
import sys
import tomllib
import tempfile
import unittest
from unittest.mock import patch
import urllib.request
from test_package_tar import PackagingFixture, ROOT

TOOLS = runpy.run_path(str(ROOT / "scripts/appimagetool.py"))
TOOL = '''#!/usr/bin/env python3
import hashlib,json,os,pathlib,platform,sys
mode=os.environ.get("FAKE_IMAGE_MODE", "ok")
with open(os.environ["FAKE_TOOL_LOG"],"a") as log: log.write(json.dumps({"entry":__file__,"args":sys.argv[1:]})+"\\n")
if sys.argv[1:]==["--appimage-extract"]:
 if mode=="extract-fail": sys.exit(4)
 root=pathlib.Path("squashfs-root");root.mkdir()
 if mode=="outside-entry": (root/"AppRun").symlink_to(pathlib.Path(__file__).resolve())
 else:
  entry=root/"AppRun";entry.write_bytes(pathlib.Path(__file__).read_bytes());entry.chmod(0o700)
else:
 assert sys.argv[1]=="--no-appstream"
 if mode=="build-fail": sys.exit(5)
 appdir=pathlib.Path(sys.argv[2]);output=pathlib.Path(sys.argv[3])
 binary=(appdir/"usr/bin/dsh-ocean").read_bytes()
 assert b"fresh-cargo-output" in binary and b"stale-should-not-ship" not in binary
 assert (appdir/"dsh-ocean.png").is_file() and (appdir/"dsh-ocean.desktop").is_file()
 for desktop in [appdir/"dsh-ocean.desktop", appdir/"usr/share/applications/dsh-ocean.desktop"]:
  assert "Exec=dsh-ocean" in desktop.read_text() and "%U" not in desktop.read_text()
 assert (appdir/"usr/share/doc/dsh-ocean/LICENSE").is_file()
 assert not (appdir/"install.sh").exists() and not (appdir/"install.py").exists()
 assert '"$@"' in (appdir/"AppRun").read_text()
 for path in appdir.rglob("*"):
  expected=0o755 if path.is_dir() or path in [appdir/"AppRun",appdir/"usr/bin/dsh-ocean"] else 0o644
  assert path.stat().st_mode&0o777==expected
  assert path.stat().st_mtime==int(os.environ["SOURCE_DATE_EPOCH"])
 header=bytearray(64);header[:7]=b"\\x7fELF\\x02\\x01\\x01";header[8:11]=b"AI\\x02";header[16:18]=(2).to_bytes(2,"little");header[18:20]=(62 if platform.machine()=="x86_64" else 183).to_bytes(2,"little")
 if mode=="bad-header": header[8:11]=b"bad"
 output.write_bytes(header+b"SYNTHETIC-NOT-SQUASHFS fresh-cargo-output "+hashlib.sha256(binary).digest());output.chmod(0o755)
 if mode=="symlink-output":
  held=output.with_name("held-image");output.rename(held);output.symlink_to(held)
'''


class AppImagePackaging(PackagingFixture):
    def setUp(self):
        super().setUp()
        for name in ("package-appimage.sh", "package-appimage.py", "appimagetool.py"):
            shutil.copyfile(ROOT / "scripts" / name, self.root / "scripts" / name)
        (self.root / "packaging/dsh-ocean.desktop").write_text("[Desktop Entry]\nType=Application\nExec=dsh-ocean %U\nIcon=dsh-ocean\n")
        self.tool = self.root / "synthetic-tool"
        self.tool.write_text(TOOL)
        self.tool.chmod(0o600)
        self.sha = hashlib.sha256(self.tool.read_bytes()).hexdigest()
        self.log = self.root / "tool-log.jsonl"

    def build_image(self, output="dist", sha=None, tool=None, **environment):
        directory = self.root / output
        result = subprocess.run(["bash", str(self.root / "scripts/package-appimage.sh"), "--tool", str(tool or self.tool), "--tool-sha256", sha or self.sha, "--output-dir", str(directory)], cwd=self.root, env={**self.env, "FAKE_TOOL_LOG": str(self.log), **environment}, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)
        return result, directory / f"dsh-ocean-0.1.0-{platform.machine()}.AppImage"

    def test_private_verified_snapshot_fresh_staging_header_and_old_stage_preservation(self):
        old = self.root / "dist/appimage/old/keep"
        old.parent.mkdir(parents=True)
        old.write_bytes(b"leave old stage alone")
        result, output = self.build_image()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output.read_bytes()[8:11], b"AI\x02")
        self.assertIn(b"SYNTHETIC-NOT-SQUASHFS", output.read_bytes())
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)
        self.assertEqual(output.stat().st_nlink, 1)
        self.assertEqual(self.tool.stat().st_mode & 0o777, 0o600)
        self.assertEqual(old.read_bytes(), b"leave old stage alone")
        events = [json.loads(row) for row in self.log.read_text().splitlines()]
        self.assertEqual(len(events), 2)
        self.assertTrue(all(row["entry"] != str(self.tool) for row in events))
        self.assertIn("--appimage-extract", events[0]["args"])
        self.assertFalse(list(output.parent.glob(".dsh-ocean-package-*")))

    def test_digest_missing_wrong_or_symlinked_tool_never_executes_or_builds(self):
        result = subprocess.run(["bash", str(self.root / "scripts/package-appimage.sh")], env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertNotEqual(result.returncode, 0)
        refused, output = self.build_image(sha="0" * 64)
        self.assertNotEqual(refused.returncode, 0)
        link = self.root / "tool-alias"
        link.symlink_to(self.tool)
        refused, _ = self.build_image(tool=link)
        self.assertNotEqual(refused.returncode, 0)
        self.tool.chmod(0o666)
        refused, _ = self.build_image()
        self.assertNotEqual(refused.returncode, 0)
        self.tool.chmod(0o600)
        os.link(self.tool, self.root / "tool-hardlink")
        refused, _ = self.build_image()
        self.assertNotEqual(refused.returncode, 0)
        self.assertFalse(self.log.exists())
        self.assertFalse((self.root / "cargo-argv.jsonl").exists())
        self.assertFalse(output.parent.exists())

    def test_extraction_build_and_header_failures_never_publish(self):
        for mode in ("extract-fail", "outside-entry", "build-fail", "bad-header", "symlink-output"):
            result, output = self.build_image(mode, FAKE_IMAGE_MODE=mode)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(output.exists())
            self.assertFalse(list(output.parent.glob(".dsh-ocean-package-*")))

    def test_existing_and_racing_outputs_survive(self):
        built, output = self.build_image()
        self.assertEqual(built.returncode, 0, built.stderr)
        before = output.read_bytes()
        events = self.log.read_bytes()
        refused, _ = self.build_image()
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(output.read_bytes(), before)
        self.assertEqual(self.log.read_bytes(), events)
        directory = self.root / "race"
        directory.mkdir()
        collision = directory / output.name
        refused, _ = self.build_image("race", FAKE_RACE_OUTPUT=str(collision))
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(collision.read_bytes(), b"raced-other-output")
        self.assertFalse(list(directory.glob(".dsh-ocean-package-*")))


class Response(io.BytesIO):
    def __init__(self, value, url="https://example.invalid/pinned-tool"):
        super().__init__(value)
        self.url = url

    def geturl(self):
        return self.url


class ToolIntake(unittest.TestCase):
    def test_default_opener_rejects_each_http_redirect_before_following_it(self):
        handler = TOOLS["HTTPSRedirect"]()
        request = urllib.request.Request("https://example.invalid/start")
        request = handler.redirect_request(request, None, 302, "Found", {}, "https://example.invalid/second")
        self.assertEqual(request.full_url, "https://example.invalid/second")
        for target in ("http://example.invalid/plain", "ftp://example.invalid/plain", "https://user:pass@example.invalid/plain"):
            with self.assertRaises(ValueError):
                handler.redirect_request(request, None, 302, "Found", {}, target)
        # No transport exists in this test: ensure production default actually
        # installs this before-follow guard, not only final-response validation.
        seen = []
        class Opener:
            def open(self, url, timeout):
                seen.append((url, timeout))
                return "synthetic-response"
        with patch.object(urllib.request, "build_opener", side_effect=lambda installed: (seen.append(isinstance(installed, TOOLS["HTTPSRedirect"])), Opener())[1]):
            self.assertEqual(TOOLS["open_https"]("https://example.invalid/start", 30), "synthetic-response")
        self.assertEqual(seen, [True, ("https://example.invalid/start", 30)])

    def test_https_digest_private_cache_no_execution_no_overwrite(self):
        with tempfile.TemporaryDirectory(prefix="dsh-ocean-tool-test-") as temporary:
            root = Path(temporary)
            payload = b"synthetic-tool-not-executed"
            sha = hashlib.sha256(payload).hexdigest()
            calls = []
            def opener(url, timeout):
                calls.append((url, timeout))
                return Response(payload)
            cached = TOOLS["fetch"]("https://example.invalid/pinned-tool", sha, root, opener)
            self.assertEqual(cached.read_bytes(), payload)
            self.assertEqual(cached.stat().st_mode & 0o777, 0o600)
            self.assertEqual(cached.stat().st_nlink, 1)
            with self.assertRaises(FileExistsError):
                TOOLS["fetch"]("https://example.invalid/pinned-tool", sha, root, opener)
            self.assertEqual(len(calls), 1)
            self.assertEqual(list(root.iterdir()), [cached])
            race = root / "race"
            race.mkdir()
            collided = race / cached.name
            def collision_opener(url, timeout):
                collided.write_bytes(b"existing-racing-cache")
                return Response(payload)
            with self.assertRaises(FileExistsError):
                TOOLS["fetch"]("https://example.invalid/pinned-tool", sha, race, collision_opener)
            self.assertEqual(collided.read_bytes(), b"existing-racing-cache")
            self.assertEqual(list(race.iterdir()), [collided])

    def test_bad_digest_redirect_url_and_bound_do_not_leave_cache(self):
        with tempfile.TemporaryDirectory(prefix="dsh-ocean-tool-test-") as temporary:
            root = Path(temporary)
            for url in ("http://example.invalid/tool", "https://user:pass@example.invalid/tool"):
                with self.assertRaises(ValueError):
                    TOOLS["fetch"](url, "0" * 64, root, lambda *a, **kw: self.fail("network must not start"))
            with self.assertRaises(ValueError):
                TOOLS["fetch"]("https://example.invalid/tool", "0" * 64, root, lambda *a, **kw: Response(b"mismatch"))
            with self.assertRaises(ValueError):
                TOOLS["fetch"]("https://example.invalid/tool", "0" * 64, root, lambda *a, **kw: Response(b"x", "http://example.invalid/redirect"))
            globals_ = TOOLS["copy_verified"].__globals__
            previous = globals_["MAX_TOOL"]
            try:
                globals_["MAX_TOOL"] = 4
                with self.assertRaises(ValueError):
                    TOOLS["fetch"]("https://example.invalid/tool", hashlib.sha256(b"oversized").hexdigest(), root, lambda *a, **kw: Response(b"oversized"))
            finally:
                globals_["MAX_TOOL"] = previous
            self.assertEqual(list(root.iterdir()), [])


def real_appdir_smoke():
    """Real native AppRun --version only; no external appimagetool/image build."""
    backend = runpy.run_path(str(ROOT / "scripts/package-tar.py"))
    image = runpy.run_path(str(ROOT / "scripts/package-appimage.py"))
    expected = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    with tempfile.TemporaryDirectory(prefix="dsh-ocean appdir smoke-") as temporary:
        workspace = Path(temporary)
        def check(output, version, arch, opened, epoch):
            appdir = workspace / "AppDir with spaces"
            image["stage_inputs"](appdir, opened, epoch)
            home = workspace / "private-home"
            home.mkdir()
            environment = {**os.environ, "HOME": str(home), "DSH_HOME": str(home / "dsh"), "DSH_OCEAN_DATA_DIR": str(home / "ocean"), "XDG_CONFIG_HOME": str(home / "config"), "XDG_DATA_HOME": str(home / "data"), "XDG_STATE_HOME": str(home / "state"), "DBUS_SESSION_BUS_ADDRESS": "unix:path=/nonexistent"}
            result = subprocess.run([str(appdir / "AppRun"), "--version"], env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10, check=True)
            assert result.stdout.strip() == f"dsh-ocean {expected}"
            assert not list(home.iterdir())
            output.write(b"AppDir-only smoke, NOT an AppImage")
        backend["package"](ROOT, workspace / "output", 0, archive_writer=check, archive_name=lambda v,a: "appdir-smoke.bin")
        print("Real AppDir/AppRun headless --version passed in a spaced private path; no third-party tool/image/install executed.")


if __name__ == "__main__":
    if sys.argv[1:] == ["--real-appdir-smoke"]:
        real_appdir_smoke()
    else:
        unittest.main()

"""Run the source installer only against disposable prefixes and mocked helpers."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from test_package_install import desktop_arguments
from license_payload_fixture import populate as license_fixture, NAMES as LICENSE_NAMES

ROOT = Path(__file__).resolve().parents[1]


class SourceInstall(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="dsh-source-install-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'source space % \\ " $ `'
        for directory in ("scripts", "packaging", "icons", "target/release"):
            (self.source / directory).mkdir(parents=True)
        shutil.copyfile(ROOT / "scripts/install.sh", self.source / "scripts/install.sh")
        shutil.copyfile(ROOT / "packaging/dsh-ocean.desktop", self.source / "packaging/dsh-ocean.desktop")
        for name in ("32x32.png", "64x64.png", "128x128.png", "128x128@2x.png", "icon.png"):
            (self.source / "icons" / name).write_bytes(b"fixture icon")
        license_fixture(self.source)
        self.binary = self.source / "target/release/dsh-ocean"
        self.binary.write_text("#!/bin/sh\nexit 99\n")
        self.binary.chmod(0o755)
        self.tools = self.root / "tools"
        self.tools.mkdir()
        # A closed PATH prevents any real cache, cargo, or service operation.
        for name in ("bash", "dirname", "install", "cat"):
            (self.tools / name).symlink_to(shutil.which(name))
        mock = (f"#!{sys.executable}\nimport json, os, sys\n"
                "with open(os.environ['TOOL_LOG'], 'a') as out:\n"
                "    out.write(json.dumps([os.path.basename(sys.argv[0]), *sys.argv[1:]]) + '\\n')\n"
                "if os.path.basename(sys.argv[0]) not in ('cargo', 'update-desktop-database', 'gtk-update-icon-cache'):\n"
                "    sys.exit(98)\n")
        for name in ("cargo", "update-desktop-database", "gtk-update-icon-cache", "systemctl", "dsh-ocean"):
            tool = self.tools / name
            tool.write_text(mock)
            tool.chmod(0o755)
        self.log = self.root / "tools.jsonl"
        self.home = self.root / "home"
        self.home.mkdir()
        self.data = self.root / 'data space % \\ " $ `'
        self.bin = self.root / "bin"
        self.env = {"PATH": str(self.tools), "HOME": str(self.home),
                    "XDG_DATA_HOME": str(self.data), "DSH_OCEAN_BIN_DIR": str(self.bin),
                    "TOOL_LOG": str(self.log), "LC_ALL": "C"}

    def run_install(self, *args):
        return subprocess.run([str(self.tools / "bash"), str(self.source / "scripts/install.sh"), *args],
                              env=self.env, cwd=self.root, text=True, capture_output=True, timeout=10)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def assert_install(self):
        result = self.run_install("--no-build")
        self.assertEqual(result.returncode, 0, result.stderr)
        binary = Path(self.env.get("DSH_OCEAN_BIN_DIR", str(self.home / ".local/bin"))) / "dsh-ocean"
        self.assertEqual(binary.read_bytes(), self.binary.read_bytes())
        self.assertTrue(os.access(binary, os.X_OK))
        desktop = self.data / "applications/dev.dsh.ocean.desktop"
        text = desktop.read_text()
        self.assertNotIn("%U", text.replace("%%", ""))
        # GLib independently decodes desktop string escaping and argv quoting;
        # desktop field-code expansion then maps %% to a literal percent.
        self.assertEqual(desktop_arguments(desktop), [str(binary).replace("%", "%%")])
        original = (self.source / "packaging/dsh-ocean.desktop").read_text()
        self.assertEqual([line for line in text.splitlines() if not line.startswith("Exec=")],
                         [line for line in original.splitlines() if not line.startswith("Exec=")])
        for size in (32, 64, 128, 256, 512):
            self.assertEqual((self.data / f"icons/hicolor/{size}x{size}/apps/dsh-ocean.png").read_bytes(), b"fixture icon")
        for name in LICENSE_NAMES:
            self.assertEqual((self.data / "doc/dsh-ocean" / name).read_bytes(), (self.source / name).read_bytes())
        self.assertEqual((self.data / "doc/dsh-ocean/packaging/license-inputs.txt").read_bytes(),
                         (self.source / "packaging/license-inputs.txt").read_bytes())
        self.assertEqual(self.calls(), [["update-desktop-database", str(desktop.parent)],
                                       ["gtk-update-icon-cache", "-f", "-t", str(self.data / "icons/hicolor")]])
        self.assertFalse((self.home / ".config").exists())

    def test_embedded_dictionary_license_is_mandatory_before_install(self):
        manifest = self.source / 'packaging/license-inputs.txt'
        manifest.write_text('\n'.join(line for line in manifest.read_text().splitlines() if line != 'locales/dsh/LICENSE') + '\n')
        result = self.run_install('--no-build')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.bin / 'dsh-ocean').exists())

    def test_covered_source_entries_are_mandatory(self):
        manifest = self.source / "packaging/license-inputs.txt"
        rows = manifest.read_text().splitlines()
        manifest.write_text("\n".join(row for row in rows if not row.startswith("docs/third-party/sources/")) + "\n")
        result = self.run_install("--no-build")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.bin / "dsh-ocean").exists())

    def test_missing_or_linked_notice_fails_before_replacing_binary(self):
        notice = self.source / "THIRD_PARTY_NOTICES.txt"
        self.assertEqual(notice.parent, self.source)
        saved = notice.read_bytes()
        notice.unlink()
        result = self.run_install("--no-build")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.bin / "dsh-ocean").exists())
        outside = self.root / "outside-notice"
        outside.write_bytes(saved)
        notice.symlink_to(outside)
        result = self.run_install("--no-build")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.bin / "dsh-ocean").exists())
        self.assertEqual(outside.read_bytes(), saved)

    def test_payload_manifest_cannot_name_private_paths(self):
        with (self.source / "packaging/license-inputs.txt").open("a") as out:
            out.write("../../private-key\n")
        result = self.run_install("--no-build")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.bin / "dsh-ocean").exists())

    def test_no_build_default_prefix(self):
        del self.env["DSH_OCEAN_BIN_DIR"]
        self.assert_install()

    def test_quoted_paths_and_injection_payloads(self):
        for suffix in ('space dir', 'percent %f %U %%', 'back\\slash', 'double"quote',
                       'dollar$(touch INJECTED)', '`touch INJECTED`', 'all % \\ " $ ` & | ;'):
            with self.subTest(suffix=suffix):
                self.env["DSH_OCEAN_BIN_DIR"] = str(self.root / suffix)
                self.log.write_text("")
                self.assert_install()
                self.assertFalse((self.root / "INJECTED").exists())

    def test_build_is_locked(self):
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls()[0], ["cargo", "build", "--release", "--locked"])

    def test_no_build_missing_binary_still_builds_locked(self):
        self.binary.chmod(0o644)
        result = self.run_install("--no-build")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.calls(), [["cargo", "build", "--release", "--locked"]])

    def test_unrepresentable_binary_prefix_refused_before_changes(self):
        for value in ("relative/bin", str(self.root / "line\nbreak"), str(self.root / "tab\tname")):
            with self.subTest(value=value):
                self.env["DSH_OCEAN_BIN_DIR"] = value
                result = self.run_install("--no-build")
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertFalse(self.data.exists())
                self.assertEqual(self.calls(), [])


if __name__ == "__main__":
    unittest.main()

"""Exercise only the Arch template's staging function in a disposable prefix."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest
from license_payload_fixture import ROOT, NAMES, populate


class ArchLicensePayload(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="dsh-ocean-arch-license-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.srcdir = self.root / "src"
        version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
        self.source = self.srcdir / f"dsh-ocean-{version}"
        populate(self.source)
        for relative in ("target/release/dsh-ocean", "packaging/dsh-ocean.desktop",
                         "icons/32x32.png", "icons/64x64.png", "icons/128x128.png",
                         "icons/128x128@2x.png", "icons/icon.png"):
            path = self.source / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"synthetic payload, never executed\n")
        self.tools = self.root / "tools"
        self.tools.mkdir()
        for name in ("install", "sed"):
            (self.tools / name).symlink_to(shutil.which(name))
        self.output = self.root / "package"

    def stage(self):
        # No build(), package manager, service helper or client is invoked.
        return subprocess.run([shutil.which("bash"), "-c",
                               'set -e; source "$1"; srcdir="$2"; pkgdir="$3"; package',
                               "fixture", str(ROOT / "packaging/PKGBUILD"),
                               str(self.srcdir), str(self.output)],
                              env={"PATH": str(self.tools), "HOME": str(self.root)},
                              capture_output=True, text=True, timeout=10)

    def test_stages_full_notice_and_source_payload(self):
        result = self.stage()
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in NAMES:
            self.assertEqual((self.output / "usr/share/doc/dsh-ocean" / name).read_bytes(),
                             (self.source / name).read_bytes())
        self.assertEqual((self.output / "usr/share/licenses/dsh-ocean/THIRD_PARTY_NOTICES.txt").read_bytes(),
                         (self.source / "THIRD_PARTY_NOTICES.txt").read_bytes())

    def test_fifo_manifest_is_refused_without_opening_it(self):
        manifest = self.source / "packaging/license-inputs.txt"
        self.assertTrue(manifest.resolve().is_relative_to(self.root.resolve()))
        manifest.unlink()
        os.mkfifo(manifest)
        result = self.stage()  # A timeout-isolated child, never the test reader.
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.output / "usr/bin/dsh-ocean").exists())

    def test_omitted_source_entry_fails_before_staging_binary(self):
        manifest = self.source / "packaging/license-inputs.txt"
        manifest.write_text("\n".join(row for row in manifest.read_text().splitlines()
                                      if not row.startswith("docs/third-party/sources/")) + "\n")
        result = self.stage()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.output / "usr/bin/dsh-ocean").exists())


if __name__ == "__main__":
    unittest.main()

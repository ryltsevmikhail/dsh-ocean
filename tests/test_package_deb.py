"""Read-only Debian archive inspection; never dpkg -i, apt or maintainer scripts."""
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
import tarfile
import tempfile
import tomllib
import unittest
from test_package_tar import PackagingFixture, ROOT, INPUTS

DEB = runpy.run_path(str(ROOT / "scripts/package-deb.py"))


def read_ar(raw):
    assert raw.startswith(b"!<arch>\n")
    offset, members, headers = 8, {}, []
    while offset < len(raw):
        header = raw[offset:offset + 60]
        assert len(header) == 60 and header[58:] == b"`\n"
        name = header[:16].decode().strip().removesuffix("/")
        assert name not in members
        size = int(header[48:58])
        headers.append((name, int(header[16:28]), int(header[28:34]), int(header[34:40]), int(header[40:48], 8)))
        start = offset + 60
        assert start + size <= len(raw)
        members[name] = raw[start:start + size]
        offset = start + size
        if size % 2:
            assert raw[offset:offset + 1] == b"\n"
            offset += 1
    assert offset == len(raw)
    assert list(members) == ["debian-binary", "control.tar.gz", "data.tar.gz"]
    assert members["debian-binary"] == b"2.0\n"
    return members, headers


def expected_payload():
    files = {"usr/bin/dsh-ocean", "usr/share/applications/dev.dsh.ocean.desktop", "usr/share/doc/dsh-ocean/copyright"}
    files.update("usr/share/doc/dsh-ocean/" + key for key in INPUTS if key == "assets/ocean-ice.svg" or key in ("README.md", "CHANGELOG.md", "LICENSE", "NOTICE", "CONTRIBUTING.md", "SECURITY.md", "SUPPORT.md", "CODE_OF_CONDUCT.md", "GOVERNANCE.md") or key.startswith("docs/"))
    files.update(f"usr/share/icons/hicolor/{size}x{size}/apps/dsh-ocean.png" for size in (32, 64, 128, 256, 512))
    return files


def inspect_package(path):
    members, headers = read_ar(path.read_bytes())
    epoch = headers[0][1]
    assert all(header[1:] == (epoch, 0, 0, 0o100644) for header in headers)
    for name in ("control.tar.gz", "data.tar.gz"):
        assert members[name][:4] == b"\x1f\x8b\x08\x00"
        assert int.from_bytes(members[name][4:8], "little") == epoch
    with tarfile.open(fileobj=io.BytesIO(members["control.tar.gz"])) as archive:
        assert [entry.name for entry in archive.getmembers()] == ["control", "md5sums"]
        assert all(entry.isfile() and entry.uid == entry.gid == 0 and entry.mode == 0o644 and entry.mtime == epoch for entry in archive.getmembers())
        control = archive.extractfile("control").read().decode()
        checksums = dict(line.split("  ", 1)[::-1] for line in archive.extractfile("md5sums").read().decode().splitlines())
    with tarfile.open(fileobj=io.BytesIO(members["data.tar.gz"])) as archive:
        entries = archive.getmembers()
        assert all(entry.isfile() or entry.isdir() for entry in entries)
        assert all(entry.uid == entry.gid == 0 and entry.mtime == epoch and entry.mode == (0o755 if entry.isdir() or entry.name == "usr/bin/dsh-ocean" else 0o644) for entry in entries)
        files = {entry.name: archive.extractfile(entry).read() for entry in entries if entry.isfile()}
        assert set(files) == expected_payload()
        assert set(checksums) == set(files)
        for name, content in files.items():
            assert hashlib.md5(content, usedforsecurity=False).hexdigest() == checksums[name]
        assert b"Exec=/usr/bin/dsh-ocean\n" in files["usr/share/applications/dev.dsh.ocean.desktop"]
        assert not any("install.sh" in name or "install.py" in name or "postinst" in name or "systemd" in name for name in files)
    return control, files, entries, headers


class DebianPackaging(PackagingFixture):
    def setUp(self):
        super().setUp()
        for name in ("package-deb.sh", "package-deb.py"):
            shutil.copyfile(ROOT / "scripts" / name, self.root / "scripts" / name)
        (self.root / "packaging/dsh-ocean.desktop").write_text("[Desktop Entry]\nType=Application\nExec=dsh-ocean %U\nIcon=dsh-ocean\n")
        arch = "amd64" if platform.machine() == "x86_64" else "arm64"
        self.deb_name = f"dsh-ocean_0.1.0-1_{arch}.deb"

    def build(self, output="dist", **environment):
        directory = self.root / output
        result = subprocess.run(["bash", str(self.root / "scripts/package-deb.sh"), "--output-dir", str(directory)], cwd=self.root, env={**self.env, **environment}, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15)
        return result, directory / self.deb_name

    def test_fresh_payload_control_md5_modes_epochs_and_no_maintainer_scripts(self):
        legacy = self.root / "dist/deb/legacy/keep"
        legacy.parent.mkdir(parents=True)
        legacy.write_bytes(b"old staging must survive")
        result, output = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        control, files, entries, headers = inspect_package(output)
        self.assertIn("Version: 0.1.0-1\n", control)
        self.assertIn("Architecture: " + ("amd64" if platform.machine() == "x86_64" else "arm64"), control)
        self.assertIn("libwebkit2gtk-4.1-0", control)
        self.assertIn(b"fresh-cargo-output", files["usr/bin/dsh-ocean"])
        self.assertNotIn(b"stale-should-not-ship", files["usr/bin/dsh-ocean"])
        self.assertIn(b"fixture LICENSE", files["usr/share/doc/dsh-ocean/copyright"])
        self.assertIn(b"fixture NOTICE", files["usr/share/doc/dsh-ocean/copyright"])
        self.assertTrue(all(row[1:] == (17, 0, 0, 0o100644) for row in headers))
        self.assertTrue(all(entry.uid == entry.gid == 0 and entry.mtime == 17 for entry in entries))
        self.assertEqual(next(entry.mode for entry in entries if entry.name == "usr/bin/dsh-ocean"), 0o755)
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)
        self.assertEqual(output.stat().st_nlink, 1)
        self.assertEqual(legacy.read_bytes(), b"old staging must survive")
        args = json.loads((self.root / "cargo-argv.jsonl").read_text().strip())
        self.assertIn("--locked", args)
        self.assertFalse(list(output.parent.glob(".dsh-ocean-package-*")))

    def test_same_inputs_reproduce_bytes_and_existing_or_racing_output_survives(self):
        first, a = self.build("one")
        second, b = self.build("two")
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(a.read_bytes(), b.read_bytes())
        original = a.read_bytes()
        refused, _ = self.build("one")
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(a.read_bytes(), original)
        race = self.root / "race"
        race.mkdir()
        collided = race / self.deb_name
        refused, _ = self.build("race", FAKE_RACE_OUTPUT=str(collided))
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(collided.read_bytes(), b"raced-other-output")
        self.assertFalse(list(race.glob(".dsh-ocean-package-*")))

    def test_failed_build_and_invalid_desktop_never_publish(self):
        result, output = self.build("failed", FAKE_CARGO_MODE="fail")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.parent.exists())
        (self.root / "packaging/dsh-ocean.desktop").write_text("[Desktop Entry]\nExec=one\nExec=two\n")
        result, output = self.build("desktop")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())
        self.assertFalse(list(output.parent.glob(".dsh-ocean-package-*")))

    def test_prerelease_version_and_arm_metadata_mapping(self):
        self.assertEqual(DEB["deb_version"]("1.2.3-rc-linux+build-2"), "1.2.3~rc-linux+build-2-1")
        self.assertEqual(DEB["filename"]("1.2.3", "aarch64"), "dsh-ocean_1.2.3-1_arm64.deb")
        if shutil.which("dpkg"):
            subprocess.run(["dpkg", "--compare-versions", DEB["deb_version"]("1.2.3-rc-linux+build-2"), "lt", DEB["deb_version"]("1.2.3")], check=True)
        manifest = self.root / "Cargo.toml"
        manifest.write_text(manifest.read_text().replace("0.1.0", "1.2.3-rc-linux+build-2"))
        result, _ = self.build("pre")
        self.assertEqual(result.returncode, 0, result.stderr)
        outputs = list((self.root / "pre").glob("*.deb"))
        self.assertEqual(len(outputs), 1)
        control, *_ = inspect_package(outputs[0])
        self.assertIn("Version: 1.2.3~rc-linux+build-2-1\n", control)

    def test_output_symlink_is_not_followed(self):
        referent = self.root / "synthetic-keep"
        referent.write_bytes(b"unchanged")
        directory = self.root / "link"
        directory.mkdir()
        (directory / self.deb_name).symlink_to(referent)
        result, _ = self.build("link")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(referent.read_bytes(), b"unchanged")
        self.assertFalse((self.root / "cargo-argv.jsonl").exists())


def real_smoke():
    with tempfile.TemporaryDirectory(prefix="dsh-ocean-deb-smoke-") as temporary:
        directory = Path(temporary)
        subprocess.run(["bash", str(ROOT / "scripts/package-deb.sh"), "--output-dir", str(directory)], cwd=ROOT, check=True)
        outputs = list(directory.glob("*.deb"))
        assert len(outputs) == 1
        control, files, _, _ = inspect_package(outputs[0])
        expected = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
        assert f"Version: {DEB['deb_version'](expected)}\n" in control
        # Independent standard readers inspect only; never install or execute hooks.
        if shutil.which("ar"):
            result = subprocess.run(["ar", "t", str(outputs[0])], text=True, stdout=subprocess.PIPE, check=True)
            assert result.stdout.splitlines() == ["debian-binary", "control.tar.gz", "data.tar.gz"]
            print("Independent ar reader passed.")
        if shutil.which("dpkg-deb"):
            subprocess.run(["dpkg-deb", "--info", str(outputs[0])], check=True, stdout=subprocess.PIPE)
            subprocess.run(["dpkg-deb", "--contents", str(outputs[0])], check=True, stdout=subprocess.PIPE)
            print("Independent dpkg-deb info/contents passed (no installation).")
        binary = directory / "headless-version-client"
        binary.write_bytes(files["usr/bin/dsh-ocean"])
        binary.chmod(0o700)
        home = directory / "private-home"
        home.mkdir()
        environment = {**os.environ, "HOME": str(home), "DSH_HOME": str(home / "dsh"), "DSH_OCEAN_DATA_DIR": str(home / "ocean"), "XDG_CONFIG_HOME": str(home / "config"), "XDG_DATA_HOME": str(home / "data"), "XDG_STATE_HOME": str(home / "state"), "DBUS_SESSION_BUS_ADDRESS": "unix:path=/nonexistent"}
        version = subprocess.run([str(binary), "--version"], env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10, check=True)
        assert version.stdout.strip() == f"dsh-ocean {expected}"
        assert not list(home.iterdir())
        print("Real Debian ar/control/data/checksums and headless version passed; no package installed.")


if __name__ == "__main__":
    if sys.argv[1:] == ["--real-smoke"]:
        real_smoke()
    else:
        unittest.main()

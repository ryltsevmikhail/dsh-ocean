"""Portable new-install tests: only owned temporary HOME/XDG/prefixes, no services."""
import ctypes
import ctypes.util
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
from unittest.mock import patch
from test_package_tar import ROOT
from license_payload_fixture import populate as license_fixture, NAMES as LICENSE_NAMES

INSTALL = runpy.run_path(str(ROOT / "packaging/install-from-package.py"))


def desktop_arguments(path):
    """Independent GLib readers only: parse strings/argv, never launch a desktop entry."""
    library = ctypes.util.find_library("glib-2.0")
    if not library:
        raise unittest.SkipTest("GLib reader is provided by the native SDK in CI")
    glib = ctypes.CDLL(library)
    glib.g_key_file_new.restype = ctypes.c_void_p
    glib.g_key_file_load_from_data.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_void_p]
    glib.g_key_file_load_from_data.restype = ctypes.c_int
    glib.g_key_file_get_string.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p]
    glib.g_key_file_get_string.restype = ctypes.c_void_p
    glib.g_key_file_unref.argtypes = [ctypes.c_void_p]
    glib.g_free.argtypes = [ctypes.c_void_p]
    glib.g_strfreev.argtypes = [ctypes.POINTER(ctypes.c_char_p)]
    glib.g_shell_parse_argv.argtypes = [ctypes.c_char_p, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.POINTER(ctypes.c_char_p)), ctypes.c_void_p]
    glib.g_shell_parse_argv.restype = ctypes.c_int
    key = glib.g_key_file_new()
    try:
        content = path.read_bytes()
        assert glib.g_key_file_load_from_data(key, content, len(content), 0, None)
        command = glib.g_key_file_get_string(key, b"Desktop Entry", b"Exec", None)
        assert command
        try:
            argc = ctypes.c_int()
            argv = ctypes.POINTER(ctypes.c_char_p)()
            assert glib.g_shell_parse_argv(ctypes.string_at(command), ctypes.byref(argc), ctypes.byref(argv), None)
            try:
                return [argv[index].decode() for index in range(argc.value)]
            finally:
                glib.g_strfreev(argv)
        finally:
            glib.g_free(command)
    finally:
        glib.g_key_file_unref(key)


class PortableInstall(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="dsh-ocean-install-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.package = self.root / "package"
        self.package.mkdir()
        shutil.copyfile(ROOT / "packaging/install-from-package.py", self.package / "install.py")
        shutil.copyfile(ROOT / "packaging/install-from-package.sh", self.package / "install.sh")
        self.header = bytearray(64)
        self.header[:7] = b"\x7fELF\x02\x01\x01"
        self.header[16:18] = (3).to_bytes(2, "little")
        self.header[18:20] = (62 if platform.machine() == "x86_64" else 183).to_bytes(2, "little")
        (self.package / "dsh-ocean").write_bytes(self.header + b"synthetic-native-not-executed")
        for size in (32, 64, 128, 256, 512):
            path = self.package / f"icons/{size}x{size}.png"
            path.parent.mkdir(exist_ok=True)
            path.write_bytes(b"synthetic-icon")
        (self.package / "packaging").mkdir()
        (self.package / "packaging/dsh-ocean.desktop").write_text("[Desktop Entry]\nType=Application\nName=Fixture\nExec=dsh-ocean %U\nIcon=dsh-ocean\n")
        license_fixture(self.package)
        self.home = self.root / "private-home"
        self.home.mkdir()
        self.bin = self.home / "bin odd & quote\" dollar$ tick` percent% slash\\ apostrophe'"
        self.data = self.home / "data"
        self.state = self.home / "state"
        self.desktop = self.data / "applications/dev.dsh.ocean.desktop"
        tools = self.root / "forbidden-tools"
        tools.mkdir()
        for name in ("systemctl", "update-desktop-database", "gtk-update-icon-cache", "xdg-open"):
            path = tools / name
            path.write_text('#!/bin/sh\nprintf forbidden >> "$FORBIDDEN_LOG"\nexit 90\n')
            path.chmod(0o755)
        self.env = {**os.environ, "HOME": str(self.home), "DSH_OCEAN_BIN_DIR": str(self.bin), "XDG_DATA_HOME": str(self.data), "XDG_STATE_HOME": str(self.state), "FORBIDDEN_LOG": str(self.root / "forbidden-log"), "PATH": str(tools) + os.pathsep + os.environ["PATH"]}

    def cli(self, *arguments, **environment):
        return subprocess.run(["bash", str(self.package / "install.sh"), *arguments], env={**self.env, **environment}, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)

    def assert_no_install(self):
        self.assertFalse((self.bin / "dsh-ocean").exists())
        self.assertFalse(list(self.home.rglob(".dsh-ocean-install-*")))
        self.assertFalse((self.root / "forbidden-log").exists())

    def test_read_only_check_then_new_install_and_glib_exec_quoting(self):
        result = self.cli("--check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(list(self.home.iterdir()), [])
        result = self.cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        binary = self.bin / "dsh-ocean"
        self.assertEqual(binary.read_bytes(), (self.package / "dsh-ocean").read_bytes())
        self.assertEqual(binary.stat().st_mode & 0o777, 0o755)
        self.assertEqual(binary.stat().st_nlink, 1)
        self.assertEqual(self.desktop.stat().st_mode & 0o777, 0o644)
        for name in LICENSE_NAMES:
            installed = self.data / "doc/dsh-ocean" / name
            self.assertEqual(installed.read_bytes(), (self.package / name).read_bytes())
            self.assertEqual(installed.stat().st_mode & 0o777, 0o644)
        arguments = desktop_arguments(self.desktop)
        self.assertEqual(arguments, [str(binary).replace("%", "%%")])
        self.assertEqual(arguments[0].replace("%%", "%"), str(binary))
        self.assertFalse((self.root / "forbidden-log").exists())
        self.assertFalse(list(self.home.rglob(".dsh-ocean-install-*")))
        before = binary.read_bytes()
        refused = self.cli()
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(binary.read_bytes(), before)

    def test_covered_source_entries_are_mandatory_even_if_files_exist(self):
        manifest = self.package / "packaging/license-inputs.txt"
        rows = manifest.read_text().splitlines()
        manifest.write_text("\n".join(row for row in rows if not row.startswith("docs/third-party/sources/")) + "\n")
        self.assertNotEqual(self.cli().returncode, 0)
        self.assert_no_install()

    def test_modified_notice_survives_uninstall(self):
        result = self.cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        notice = self.data / "doc/dsh-ocean/THIRD_PARTY_NOTICES.txt"
        notice.write_bytes(b"operator annotation retained")
        result = self.cli("--uninstall")
        # Preserve the existing all-or-nothing refusal, not a partial uninstall.
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(notice.read_bytes(), b"operator annotation retained")
        self.assertTrue((self.bin / "dsh-ocean").exists())
        self.assertTrue((self.data / "doc/dsh-ocean/LICENSE").exists())
        self.assertTrue(self.ledger().exists())

    def test_license_manifest_rejects_private_paths_and_missing_text(self):
        manifest = self.package / "packaging/license-inputs.txt"
        original = manifest.read_bytes()
        manifest.write_bytes(original + b"../../private-key\n")
        self.assertNotEqual(self.cli().returncode, 0)
        self.assert_no_install()
        manifest.write_bytes(original)
        notice = self.package / "THIRD_PARTY_NOTICES.txt"
        self.assertEqual(notice.parent, self.package)
        notice.unlink()
        self.assertNotEqual(self.cli().returncode, 0)
        self.assert_no_install()

    def test_empty_overrides_use_private_home_defaults(self):
        result = self.cli(DSH_OCEAN_BIN_DIR="", XDG_DATA_HOME="")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.home / ".local/bin/dsh-ocean").is_file())
        self.assertTrue((self.home / ".local/share/applications/dev.dsh.ocean.desktop").is_file())
        self.assertFalse(self.bin.exists())
        self.assertFalse(self.data.exists())
        self.assertFalse((self.root / "forbidden-log").exists())

    def test_existing_late_target_fails_before_any_new_directories(self):
        self.desktop.parent.mkdir(parents=True)
        self.desktop.write_bytes(b"existing desktop must survive")
        result = self.cli()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.bin.exists())
        self.assertFalse((self.data / "icons").exists())
        self.assertFalse((self.data / "icons/hicolor").exists())
        # `applications` is a shared XDG directory and must not be removed.
        self.assertTrue((self.data / "applications").is_dir())
        self.assertEqual(self.desktop.read_bytes(), b"existing desktop must survive")
        self.assert_no_install()

    def test_symlink_target_parent_and_unsafe_paths_are_not_followed(self):
        self.bin.mkdir()
        note = self.root / "synthetic-keep"
        note.write_bytes(b"do not touch")
        (self.bin / "dsh-ocean").symlink_to(note)
        refused = self.cli()
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(note.read_bytes(), b"do not touch")
        alias = self.root / "aliased-bin"
        alias.symlink_to(self.bin, target_is_directory=True)
        refused = self.cli(DSH_OCEAN_BIN_DIR=str(alias))
        self.assertNotEqual(refused.returncode, 0)
        for unsafe in ("relative", str(self.root / "bad\npath"), str(self.root / "alias/../bin")):
            self.assertNotEqual(self.cli(DSH_OCEAN_BIN_DIR=unsafe).returncode, 0)
        self.assertFalse((self.data).exists())

    def test_source_symlink_and_wrong_machine_fail_before_destination_writes(self):
        icon = self.package / "icons/32x32.png"
        self.assertTrue(icon.is_absolute() and icon.parent.parent == self.package)
        icon.unlink()
        icon.symlink_to(self.root / "nonexistent-note")
        self.assertNotEqual(self.cli().returncode, 0)
        self.assertEqual(list(self.home.iterdir()), [])
        self.assertTrue(icon.is_symlink() and icon.parent.parent == self.package)
        icon.unlink()
        icon.write_bytes(b"restored fixture icon")
        header = self.header.copy()
        header[18:20] = (183 if platform.machine() == "x86_64" else 62).to_bytes(2, "little")
        (self.package / "dsh-ocean").write_bytes(header)
        self.assertNotEqual(self.cli().returncode, 0)
        self.assertEqual(list(self.home.iterdir()), [])

    def test_package_root_ancestor_symlink_is_rejected_without_resolve_hiding_it(self):
        alias = self.root / "source-alias"
        alias.symlink_to(self.root, target_is_directory=True)
        logical_package = alias / "package"
        with self.assertRaises(OSError):
            INSTALL["install"](logical_package, self.bin, self.data, self.state)
        result = subprocess.run(["bash", str(logical_package / "install.sh")], env=self.env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(self.home.iterdir()), [])

    def ledger(self):
        return self.state / "dsh-ocean/installed-portable.json"

    def test_install_records_private_ledger_and_uninstall_removes_only_verified_files(self):
        self.assertEqual(self.cli().returncode, 0)
        ledger = self.ledger()
        self.assertEqual(ledger.stat().st_mode & 0o777, 0o600)
        document = json.loads(ledger.read_text())
        self.assertEqual(document["schema"], 1)
        paths = {entry["path"] for entry in document["files"]}
        self.assertEqual(len(paths), 8 + len(LICENSE_NAMES))
        self.assertIn(str(self.bin / "dsh-ocean"), paths)
        for entry in document["files"]:
            self.assertEqual(set(entry), {"path", "mode", "size", "sha256"})
            self.assertEqual(hashlib.sha256(Path(entry["path"]).read_bytes()).hexdigest(), entry["sha256"])
        unrelated = self.bin / "operator-note"
        unrelated.write_bytes(b"not installed by this ledger")
        result = self.cli("--uninstall")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.bin / "dsh-ocean").exists())
        self.assertFalse(self.desktop.exists())
        self.assertEqual(unrelated.read_bytes(), b"not installed by this ledger")
        self.assertFalse(ledger.exists())
        self.assertFalse((self.state / "dsh-ocean").exists())
        self.assertFalse((self.data / "icons").exists())
        self.assertFalse(list(self.home.rglob(".dsh-ocean-install-*")))

    def test_uninstall_keeps_modified_files_and_whole_install_when_any_check_fails(self):
        self.assertEqual(self.cli().returncode, 0)
        binary = self.bin / "dsh-ocean"
        binary.write_bytes(b"operator-modified-client")
        result = self.cli("--uninstall")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("kept with the ledger intact", result.stderr)
        self.assertEqual(binary.read_bytes(), b"operator-modified-client")
        self.assertTrue(self.desktop.exists())
        self.assertTrue(self.ledger().exists())
        binary.unlink()
        binary.write_bytes((self.package / "dsh-ocean").read_bytes())
        binary.chmod(0o755)
        note = self.root / "synthetic-outside-target"
        note.write_bytes(b"outside must survive")
        self.desktop.unlink()
        self.desktop.symlink_to(note)
        result = self.cli("--uninstall")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(note.read_bytes(), b"outside must survive")
        self.assertTrue(self.ledger().exists())
        self.assertTrue(binary.exists())

    def test_uninstall_requires_ledger_and_rejects_tampered_paths(self):
        self.assertNotEqual(self.cli("--uninstall").returncode, 0)
        self.assertEqual(self.cli().returncode, 0)
        outer = self.root / "operator-file"
        outer.write_bytes(b"do not delete")
        ledger = self.ledger()
        document = json.loads(ledger.read_text())
        document["files"][0]["path"] = str(outer)
        ledger.write_text(json.dumps(document))
        result = self.cli("--uninstall")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("outside the owned prefixes", result.stderr)
        self.assertEqual(outer.read_bytes(), b"do not delete")
        self.assertTrue((self.bin / "dsh-ocean").exists())
        self.assertTrue(ledger.exists())
        document["files"][0]["path"] = str(self.bin / "dsh-ocean")
        document["files"][0]["sha256"] = "0" * 64
        ledger.write_text(json.dumps(document))
        self.assertNotEqual(self.cli("--uninstall").returncode, 0)
        self.assertTrue((self.bin / "dsh-ocean").exists())
        self.assertNotEqual(self.cli("--uninstall", "--check").returncode, 0)
        ledger_document = json.loads(ledger.read_text())
        ledger_document["files"][0]["sha256"] = hashlib.sha256((self.bin / "dsh-ocean").read_bytes()).hexdigest()
        ledger.write_text(json.dumps(ledger_document))
        binary = self.bin / "dsh-ocean"
        original = binary.stat().st_mode & 0o7777
        binary.chmod(0o700)
        self.assertNotEqual(self.cli("--uninstall").returncode, 0)
        self.assertTrue(binary.exists() and ledger.exists())
        binary.chmod(original)
        self.assertEqual(self.cli("--uninstall").returncode, 0)
        self.assertFalse(binary.exists())
        self.assertFalse(ledger.exists())

    def test_collision_after_first_publication_rolls_back_only_owned_files(self):
        real_link = os.link
        calls = []
        def race(source, target, **keywords):
            calls.append(target)
            if len(calls) == 2:
                fd = os.open(target, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600, dir_fd=keywords["dst_dir_fd"])
                with os.fdopen(fd, "wb") as existing:
                    existing.write(b"racing-other-output")
            return real_link(source, target, **keywords)
        with patch.object(os, "link", side_effect=race):
            with self.assertRaises(FileExistsError):
                INSTALL["install"](self.package, self.bin, self.data, self.state)
        self.assert_no_install()
        self.assertEqual((self.data / "icons/hicolor/32x32/apps/dsh-ocean.png").read_bytes(), b"racing-other-output")
        self.assertFalse(self.desktop.exists())

    def test_ledger_records_prunable_directories_and_exact_modes(self):
        pre_existing = self.data / "icons/hicolor"
        pre_existing.mkdir(parents=True)
        shared = self.data / "applications"
        shared.mkdir(parents=True)
        self.assertEqual(self.cli().returncode, 0)
        document = json.loads(self.ledger().read_text())
        recorded = set(document["directories"])
        self.assertIn(str(self.state / "dsh-ocean"), recorded)
        self.assertNotIn(str(pre_existing), recorded)
        self.assertNotIn(str(self.data / "applications"), recorded)
        for entry in document["files"]:
            target = Path(entry["path"])
            info = target.stat()
            self.assertEqual(entry["mode"], info.st_mode & 0o7777)
            self.assertEqual(entry["size"], info.st_size)
        self.assertEqual(self.cli("--uninstall").returncode, 0)
        # Pre-existing (adopted) directories are never pruned; directories this
        # install created are pruned only once they are empty.
        self.assertTrue(pre_existing.is_dir())
        self.assertTrue(shared.is_dir())
        self.assertFalse((pre_existing / "32x32").exists())
        self.assertFalse((self.state / "dsh-ocean").exists())

    def test_uninstall_skips_absent_files_and_cleans_crash_scratch_links(self):
        self.assertEqual(self.cli().returncode, 0)
        binary = self.bin / "dsh-ocean"
        scratch = self.bin / ".dsh-ocean-install-deadbeef"
        os.link(binary, scratch)
        self.assertEqual(binary.stat().st_nlink, 2)
        icon = self.data / "icons/hicolor/32x32/apps/dsh-ocean.png"
        icon.unlink()
        result = self.cli("--uninstall")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(binary.exists())
        self.assertFalse(scratch.exists())
        self.assertFalse(self.ledger().exists())

    def test_existing_ledger_blocks_reinstall_even_without_target_files(self):
        self.assertEqual(self.cli().returncode, 0)
        for entry in json.loads(self.ledger().read_text())["files"]:
            Path(entry["path"]).unlink()
        refused = self.cli()
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("run --uninstall first", refused.stderr)
        self.assertEqual(self.cli("--uninstall").returncode, 0)
        self.assertEqual(self.cli().returncode, 0)

    def test_ledger_publication_failure_rolls_back_every_target(self):
        real_link = os.link
        def fail_ledger(source, target, **keywords):
            if target == "installed-portable.json":
                raise OSError("synthetic ledger publication failure")
            return real_link(source, target, **keywords)
        with patch.object(os, "link", side_effect=fail_ledger):
            with self.assertRaises(OSError):
                INSTALL["install"](self.package, self.bin, self.data, self.state)
        self.assertFalse((self.bin / "dsh-ocean").exists())
        self.assertFalse(self.desktop.exists())
        self.assertFalse(self.ledger().exists())
        self.assertFalse(list(self.home.rglob(".dsh-ocean-*")))

    def test_replaced_owned_output_is_preserved_during_failed_cleanup(self):
        real_link = os.link
        calls = []
        def replace(source, target, **keywords):
            calls.append(target)
            if len(calls) == 2:
                binary = self.bin / "dsh-ocean"
                self.assertTrue(binary.is_absolute() and binary.parent == self.bin and binary.is_file())
                binary.unlink()
                binary.write_bytes(b"replacement-must-survive")
                raise OSError("synthetic later publication failure")
            return real_link(source, target, **keywords)
        with patch.object(os, "link", side_effect=replace):
            with self.assertRaisesRegex(ValueError, "cleanup incomplete"):
                INSTALL["install"](self.package, self.bin, self.data, self.state)
        self.assertEqual((self.bin / "dsh-ocean").read_bytes(), b"replacement-must-survive")
        self.assertFalse(list(self.home.rglob(".dsh-ocean-install-*")))
        self.assertFalse(self.desktop.exists())


def real_smoke():
    with tempfile.TemporaryDirectory(prefix="dsh-ocean-private-install-smoke-") as temporary:
        root = Path(temporary)
        output = root / "output"
        subprocess.run(["bash", str(ROOT / "scripts/package.sh"), "--output-dir", str(output)], cwd=ROOT, check=True)
        archives = list(output.glob("*.tar.gz"))
        assert len(archives) == 1
        prefix = archives[0].name.removesuffix(".tar.gz")
        package = root / "trusted-package"
        package.mkdir()
        paths = ["dsh-ocean", "install.sh", "install.py", "packaging/dsh-ocean.desktop", "packaging/license-inputs.txt", *LICENSE_NAMES] + [f"icons/{size}x{size}.png" for size in (32,64,128,256,512)]
        with tarfile.open(archives[0]) as archive:
            for relative in paths:
                target = package / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                with target.open("xb") as destination:
                    shutil.copyfileobj(archive.extractfile(prefix + "/" + relative), destination)
        home = root / "private-home"
        home.mkdir()
        tools = root / "forbidden-tools"
        tools.mkdir()
        for name in ("systemctl", "update-desktop-database", "gtk-update-icon-cache", "xdg-open"):
            helper = tools / name
            helper.write_text('#!/bin/sh\nprintf forbidden >> "$FORBIDDEN_LOG"\nexit 90\n')
            helper.chmod(0o755)
        environment = {**os.environ, "PATH": str(tools) + os.pathsep + os.environ["PATH"], "FORBIDDEN_LOG": str(root / "forbidden-log"), "HOME": str(home), "DSH_OCEAN_BIN_DIR": str(home / "bin space & percent%"), "XDG_DATA_HOME": str(home / "data"), "XDG_STATE_HOME": str(home / "state"), "DSH_HOME": str(home / "runtime-not-created"), "DSH_OCEAN_DATA_DIR": str(home / "ocean-not-created"), "DBUS_SESSION_BUS_ADDRESS": "unix:path=/nonexistent"}
        subprocess.run(["bash", str(package / "install.sh"), "--check"], env=environment, check=True)
        assert not list(home.iterdir())
        subprocess.run(["bash", str(package / "install.sh")], env=environment, check=True)
        binary = Path(environment["DSH_OCEAN_BIN_DIR"]) / "dsh-ocean"
        result = subprocess.run([str(binary), "--version"], env=environment, check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        expected = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
        assert result.stdout.strip() == f"dsh-ocean {expected}"
        assert not (root / "forbidden-log").exists()
        assert not (home / "runtime-not-created").exists() and not (home / "ocean-not-created").exists()
        assert desktop_arguments(home / "data/applications/dev.dsh.ocean.desktop")[0].replace("%%", "%") == str(binary)
        ledger = home / "state/dsh-ocean/installed-portable.json"
        assert ledger.is_file() and (ledger.stat().st_mode & 0o777) == 0o600
        for relative in LICENSE_NAMES:
            installed_notice = home / "data/doc/dsh-ocean" / relative
            assert installed_notice.read_bytes() == (package / relative).read_bytes()
        assert len(list((home / "data/doc/dsh-ocean/docs/third-party/sources").glob("*.crate"))) == 5
        operator_note = Path(environment["DSH_OCEAN_BIN_DIR"]) / "operator-note"
        operator_note.write_bytes(b"unrelated operator file")
        subprocess.run(["bash", str(package / "install.sh"), "--uninstall"], env=environment, check=True)
        assert not binary.exists() and not ledger.exists()
        assert operator_note.read_bytes() == b"unrelated operator file"
        assert not (home / "data/applications/dev.dsh.ocean.desktop").exists()
        print("Trusted local tar private-prefix fresh install/preflight/GLib Exec/version/uninstall passed; real HOME, services and cache helpers untouched.")


if __name__ == "__main__":
    if sys.argv[1:] == ["--real-smoke"]:
        real_smoke()
    else:
        unittest.main()

"""Portable packaging tests in synthetic repos, with a fake trusted Cargo stream."""
import json
import os
from pathlib import Path
import platform
import runpy
import shutil
import subprocess
import stat
import sys
import tomllib
import tarfile
import tempfile
import unittest
from license_payload_fixture import populate as license_fixture

ROOT = Path(__file__).resolve().parents[1]
INPUTS = runpy.run_path(str(ROOT / "scripts/package-tar.py"))["INPUTS"]
FAKE = '''#!/usr/bin/env python3
import json,os,pathlib,platform,sys
root=pathlib.Path.cwd()
with (root/"cargo-argv.jsonl").open("a") as log: log.write(json.dumps(sys.argv[1:])+"\\n")
assert sys.argv[1]=="build" and "--release" in sys.argv and "--locked" in sys.argv
mode=os.environ.get("FAKE_CARGO_MODE","ok")
if mode=="fail": sys.exit(7)
if mode=="missing": print(json.dumps({"reason":"build-finished","success":True})); sys.exit(0)
if mode=="bad-json": print("not a Cargo artifact stream"); sys.exit(0)
if mode=="bad-shape": print("[]"); sys.exit(0)
out=root/"alternate target/release/dsh-ocean"
out.parent.mkdir(parents=True,exist_ok=True)
header=bytearray(64);header[:7]=b"\\x7fELF\\x02\\x01\\x01";header[16:18]=(3).to_bytes(2,"little")
machine=62 if platform.machine()=="x86_64" else 183
if mode=="wrong": machine=183 if machine==62 else 62
header[18:20]=machine.to_bytes(2,"little")
out.write_bytes(header+b"fresh-cargo-output");out.chmod(0o755)
if mode=="link":
 target=out.with_name("real-binary");out.rename(target);out.symlink_to(target)
if os.environ.get("FAKE_RACE_OUTPUT"): pathlib.Path(os.environ["FAKE_RACE_OUTPUT"]).write_bytes(b"raced-other-output")
if mode=="manifest-change":
 manifest=root/"Cargo.toml";manifest.write_text(manifest.read_text().replace("0.1.0","0.22.0"))
print(json.dumps({"reason":"compiler-artifact","manifest_path":str(root/"Cargo.toml"),"target":{"name":"dsh-ocean","kind":["bin"]},"executable":str(out),"fresh":True}))
'''


class PackagingFixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="dsh-ocean-package-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / "repo"
        (self.root / "scripts").mkdir(parents=True)
        for name in ("package.sh", "package-tar.py"):
            shutil.copyfile(ROOT / "scripts" / name, self.root / "scripts" / name)
        (self.root / "Cargo.toml").write_text('[package]\nname="dsh-ocean"\nversion="0.1.0"\n')
        for source in INPUTS.values():
            path = self.root / source
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(("fixture " + source + "\n").encode())
        license_fixture(self.root)
        stale = self.root / "target/release/dsh-ocean"
        stale.parent.mkdir(parents=True)
        stale.write_bytes(b"stale-should-not-ship")
        stale.chmod(0o755)
        tools = self.root / "fake-tools"
        tools.mkdir()
        cargo = tools / "cargo"
        cargo.write_text(FAKE)
        cargo.chmod(0o755)
        self.env = {**os.environ, "PATH": str(tools) + os.pathsep + os.environ["PATH"], "SOURCE_DATE_EPOCH": "17", "CARGO_TARGET_DIR": "alternate target"}
        self.name = f"dsh-ocean-0.1.0-linux-{platform.machine()}"

    def build(self, output="dist", **environment):
        directory = self.root / output
        result = subprocess.run(["bash", str(self.root / "scripts/package.sh"), "--output-dir", str(directory)], cwd=self.root, env={**self.env, **environment}, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15)
        return result, directory / (self.name + ".tar.gz")


class Packaging(PackagingFixture):
    def test_public_readme_guides_and_policies_are_shipped(self):
        required = {"CONTRIBUTING.md", "SECURITY.md", "SUPPORT.md", "CODE_OF_CONDUCT.md",
                    "GOVERNANCE.md", "docs/README.md", "docs/USAGE.md", "docs/OPEN_SOURCE_CHECKLIST.md",
                    "docs/BRANDING.md", "assets/ocean-ice.svg"}
        self.assertTrue(required.issubset(INPUTS))
        self.assertTrue(all((ROOT / INPUTS[name]).is_file() for name in required))

    def test_reported_fresh_executable_allowlist_private_archive_and_normalized_metadata(self):
        for directory in ("audit", "design", "plugins"):
            path = self.root / directory / "untracked-secret"
            path.parent.mkdir()
            path.write_text("untracked-must-not-ship")
        result, output = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        args = json.loads((self.root / "cargo-argv.jsonl").read_text().strip())
        self.assertIn("--locked", args)
        self.assertEqual(args[args.index("--manifest-path") + 1], str(self.root / "Cargo.toml"))
        self.assertEqual(args[args.index("--bin") + 1], "dsh-ocean")
        self.assertIn("--message-format=json-render-diagnostics", args)
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)
        self.assertEqual(output.stat().st_nlink, 1)
        self.assertEqual(int.from_bytes(output.read_bytes()[4:8], "little"), 17)
        with tarfile.open(output) as archive:
            members = archive.getmembers()
            files = {entry.name.removeprefix(self.name + "/") for entry in members if entry.isfile()}
            self.assertEqual(files, set(INPUTS) | {"dsh-ocean"})
            self.assertTrue(all(entry.isfile() or entry.isdir() for entry in members))
            self.assertTrue(all(entry.uid == entry.gid == 0 and entry.mtime == 17 for entry in members))
            binary = archive.extractfile(self.name + "/dsh-ocean").read()
            self.assertIn(b"fresh-cargo-output", binary)
            self.assertNotIn(b"stale-should-not-ship", binary)
            self.assertEqual(archive.getmember(self.name + "/install.sh").mode, 0o755)
            self.assertFalse(any("untracked-secret" in entry.name for entry in members))
        self.assertFalse(list(output.parent.glob(".dsh-ocean-package-*")))

    def test_covered_source_entries_cannot_be_removed_from_manifest(self):
        manifest = self.root / "packaging/license-inputs.txt"
        rows = manifest.read_text().splitlines()
        manifest.write_text("\n".join(row for row in rows if not row.startswith("docs/third-party/sources/")) + "\n")
        self.assertTrue((self.root / "docs/third-party/sources/cssparser-0.37.0.crate").is_file())
        result, output = self.build()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())

    def test_missing_linked_or_oversized_notices_fail_closed(self):
        notice = self.root / "THIRD_PARTY_NOTICES.txt"
        self.assertEqual(notice.parent, self.root)
        original = notice.read_bytes()
        notice.unlink()
        result, output = self.build()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())
        outside = Path(self.temporary.name) / "outside-notice"
        outside.write_bytes(original)
        notice.symlink_to(outside)
        result, output = self.build()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())
        self.assertEqual(outside.read_bytes(), original)
        notice.unlink()
        notice.write_bytes(original)
        with notice.open("r+b") as stream:
            stream.truncate(16 * 1024 * 1024 + 1)
        result, output = self.build()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())

    def test_archive_bytes_are_reproducible_for_identical_inputs_not_source_mtimes(self):
        first, a = self.build("one")
        self.assertEqual(first.returncode, 0, first.stderr)
        for source in INPUTS.values():
            os.utime(self.root / source, (12345, 12345))
        second, b = self.build("two")
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(a.read_bytes(), b.read_bytes())

    def test_existing_and_racing_outputs_are_never_overwritten(self):
        result, output = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        before = output.read_bytes()
        refused, _ = self.build()
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(output.read_bytes(), before)
        self.assertEqual(len((self.root / "cargo-argv.jsonl").read_text().splitlines()), 1)
        race_dir = self.root / "race"
        race_dir.mkdir()
        raced = race_dir / output.name
        refused, _ = self.build("race", FAKE_RACE_OUTPUT=str(raced))
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(raced.read_bytes(), b"raced-other-output")
        self.assertFalse(list(race_dir.glob(".dsh-ocean-package-*")))
        linked_dir = self.root / "linked-file"
        linked_dir.mkdir()
        referent = self.root / "synthetic-keep-file"
        referent.write_bytes(b"untouched referent")
        (linked_dir / output.name).symlink_to(referent)
        refused, _ = self.build("linked-file")
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(referent.read_bytes(), b"untouched referent")

    def test_failed_missing_invalid_cross_arch_or_linked_cargo_artifacts_do_not_publish(self):
        for mode in ("fail", "missing", "bad-json", "bad-shape", "wrong", "link"):
            with self.subTest(mode=mode):
                # The link case runs last, so it cannot redirect later writes.
                result, output = self.build(mode, FAKE_CARGO_MODE=mode)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertFalse(output.exists())
                self.assertFalse(output.parent.exists())

    def test_symlinked_source_and_output_directory_fail_without_following_them(self):
        outside = self.root / "synthetic-private-note"
        outside.write_text("synthetic-do-not-package")
        readme = self.root / "README.md"
        self.assertTrue(readme.is_absolute() and readme.parent == self.root)
        readme.unlink()
        readme.symlink_to(outside)
        result, output = self.build("source-link")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())
        self.assertEqual(outside.read_text(), "synthetic-do-not-package")
        real = self.root / "real-output"
        real.mkdir()
        linked = self.root / "linked-output"
        linked.symlink_to(real, target_is_directory=True)
        result, _ = self.build("linked-output")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(real.iterdir()), [])

    def test_manifest_change_during_build_is_not_mislabeled_as_old_version(self):
        result, output = self.build("manifest-change", FAKE_CARGO_MODE="manifest-change")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.parent.exists())
        self.assertIn("0.22.0", (self.root / "Cargo.toml").read_text())

    def test_repository_input_parent_symlinks_do_not_package_external_names(self):
        original = self.root / "docs"
        retained = self.root / "retained-docs"
        self.assertTrue(original.is_absolute() and retained.is_absolute() and original.parent == retained.parent == self.root)
        original.rename(retained)
        external = self.root / "external-docs"
        external.mkdir()
        for name in retained.iterdir():
            (external / name.name).write_text("synthetic-private-do-not-ship")
        original.symlink_to(external, target_is_directory=True)
        result, output = self.build("parent-link")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.parent.exists())
        self.assertTrue(all(path.read_text() == "synthetic-private-do-not-ship" for path in external.iterdir()))

    def test_special_oversized_inputs_and_writable_output_fail_without_cleanup(self):
        readme = self.root / "README.md"
        with readme.open("wb") as file:
            file.truncate(16 * 1024 * 1024 + 1)
        result, output = self.build("oversized")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.parent.exists())
        self.assertTrue(readme.is_absolute() and readme.parent == self.root and readme.is_file())
        readme.unlink()
        os.mkfifo(readme)
        result, output = self.build("fifo")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.parent.exists())
        self.assertTrue(readme.is_absolute() and readme.parent == self.root and stat.S_ISFIFO(readme.lstat().st_mode))
        readme.unlink()
        readme.write_text("fixture README")
        directory = self.root / "writable"
        directory.mkdir()
        directory.chmod(0o777)
        result, output = self.build("writable")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(directory.iterdir()), [])
        self.assertEqual(directory.stat().st_mode & 0o777, 0o777)

    def test_invalid_epoch_and_unknown_flags_do_not_build(self):
        for value in ("-1", "not-a-number", "4294967296", "9" * 5000):
            result, output = self.build(SOURCE_DATE_EPOCH=value)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(output.exists())
        result = subprocess.run(["bash", str(self.root / "scripts/package.sh"), "--overwrite"], cwd=self.root, env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "cargo-argv.jsonl").exists())


def real_smoke():
    """Build/inspect actual tar and run only its extracted client's --version."""
    with tempfile.TemporaryDirectory(prefix="dsh-ocean-package-smoke-") as temporary:
        directory = Path(temporary)
        subprocess.run(["bash", str(ROOT / "scripts/package.sh"), "--output-dir", str(directory)], cwd=ROOT, check=True)
        outputs = list(directory.glob("*.tar.gz"))
        assert len(outputs) == 1
        prefix = outputs[0].name.removesuffix(".tar.gz")
        binary = directory / "headless-version-client"
        with tarfile.open(outputs[0]) as archive:
            files = {entry.name.removeprefix(prefix + "/") for entry in archive.getmembers() if entry.isfile()}
            assert files == set(INPUTS) | {"dsh-ocean"}
            with binary.open("xb") as target:
                shutil.copyfileobj(archive.extractfile(prefix + "/dsh-ocean"), target)
        binary.chmod(0o700)
        home = directory / "private-home"
        home.mkdir()
        environment = {**os.environ, "HOME": str(home), "DSH_HOME": str(home / "dsh"), "DSH_OCEAN_DATA_DIR": str(home / "ocean"), "XDG_CONFIG_HOME": str(home / "config"), "XDG_DATA_HOME": str(home / "data"), "XDG_STATE_HOME": str(home / "state"), "DBUS_SESSION_BUS_ADDRESS": "unix:path=/nonexistent"}
        version = subprocess.run([str(binary), "--version"], env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10, check=True)
        expected = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
        assert version.stdout.strip() == f"dsh-ocean {expected}", version.stdout
        assert not list(home.iterdir()), "headless --version must not initialize profiles/runtime"
        print("Real tar allowlist and headless client --version passed; no installer ran.")


if __name__ == "__main__":
    if sys.argv[1:] == ["--real-smoke"]:
        real_smoke()
    else:
        unittest.main()

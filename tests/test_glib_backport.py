"""Mutation controls for the offline backport verifier; never modify real source."""
import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("check_glib_backport", ROOT / "scripts/check-glib-backport.py")
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class GlibBackportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="dsh-ocean-glib-check-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in ("Cargo.toml", "Cargo.lock"):
            shutil.copyfile(ROOT / name, self.root / name)
        shutil.copytree(ROOT / "vendor", self.root / "vendor")

    def reject(self):
        with self.assertRaises((ValueError, OSError)):
            CHECK.validate(self.root)

    def test_current_exact_backport_is_accepted(self):
        self.assertEqual(CHECK.validate(self.root), 121)

    def test_reverting_the_security_fix_is_rejected(self):
        path = self.root / "vendor/glib/src/variant_iter.rs"
        data = path.read_bytes()
        for old, new in CHECK.CHANGES:
            data = data.replace(new, old)
        path.write_bytes(data)
        self.reject()

    def test_unrelated_upstream_edits_are_rejected(self):
        path = self.root / "vendor/glib/src/lib.rs"
        path.write_bytes(path.read_bytes() + b"\n// unauthorized source change\n")
        self.reject()

    def test_license_removal_is_rejected(self):
        (self.root / "vendor/glib/LICENSE").unlink()
        self.reject()

    def test_extra_source_is_rejected(self):
        (self.root / "vendor/glib/src/extra.rs").write_text("// not upstream\n")
        self.reject()

    def test_symlinked_source_is_rejected(self):
        path = self.root / "vendor/glib/src/variant_iter.rs"
        outside = self.root / "outside.rs"
        path.rename(outside)
        path.symlink_to(outside)
        self.reject()

    def test_unrelated_override_is_rejected(self):
        path = self.root / "Cargo.toml"
        path.write_text(path.read_text().replace('path = "vendor/glib"', 'path = "other/glib"'))
        self.reject()

    def test_registry_fallback_is_rejected(self):
        path = self.root / "Cargo.lock"
        path.write_text(path.read_text().replace('name = "glib"\nversion = "0.18.5"',
            'name = "glib"\nversion = "0.18.5"\nsource = "registry+https://github.com/rust-lang/crates.io-index"'))
        self.reject()

    def test_registry_publication_without_patch_propagation_is_rejected(self):
        path = self.root / "Cargo.toml"
        path.write_text(path.read_text().replace("publish = false", "publish = true"))
        self.reject()

    def test_wrong_pristine_archive_is_rejected_before_tar_processing(self):
        archive = self.root / "invalid.crate"
        archive.write_bytes(b"not a tar archive")
        with self.assertRaisesRegex(ValueError, "archive checksum mismatch"):
            CHECK.validate(self.root, archive)


if __name__ == "__main__":
    unittest.main()

"""Isolated stdlib fixtures: no network, builds, or user data."""
import copy
import importlib.util
import io
import json
from pathlib import Path
import shutil
import tarfile
import tempfile
import unittest
from unittest import mock
import contextlib
import os
import subprocess
import sys

REPO = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('license_inventory', REPO / 'scripts/license-inventory.py')
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class InventoryTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / 'Cargo.toml').write_text('[package]\nname="fixture"\nversion="1"\n')
        (self.root / 'packaging').mkdir()
        self.payload_manifest = self.root / 'packaging/license-inputs.txt'
        self.write_payload_manifest()
        shutil.copytree(REPO / 'vendor/glib', self.root / 'vendor/glib')
        shutil.copyfile(REPO / 'vendor/glib-0.18.5.provenance.json', self.root / 'vendor/glib-0.18.5.provenance.json')
        shutil.copytree(REPO / 'docs/third-party/evidence', self.root / 'docs/third-party/evidence')
        sources = self.root / 'docs/third-party/sources'
        sources.mkdir()
        self.base = self.root / 'registry/src/index/fixture-1.0.0'
        self.base.mkdir(parents=True)
        (self.base / 'Cargo.toml').write_text('[package]\nname="fixture"\nversion="1.0.0"\n')
        (self.base / 'LICENSE').write_text('Fixture license text\n')
        self.archive = self.root / 'registry/cache/index/fixture-1.0.0.crate'
        self.archive.parent.mkdir(parents=True)
        with tarfile.open(self.archive, 'w:gz') as archive:
            for name in ['Cargo.toml', 'LICENSE']:
                data = (self.base / name).read_bytes()
                info = tarfile.TarInfo('fixture-1.0.0/' + name)
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
        self.checksum = audit.sha(self.archive.read_bytes())
        self.lock = self.root / 'Cargo.lock'
        self.lock.write_text(f'version=4\n[[package]]\nname="fixture"\nversion="1.0.0"\nsource="{audit.REGISTRY}"\nchecksum="{self.checksum}"\n')
        for name, version in audit.MPL.items():
            (sources / f'{name}-{version}.crate').write_bytes(b'fixture unused archive')
        p = dict(id='fixture-id', name='fixture', version='1.0.0', source=audit.REGISTRY,
                 manifest_path=str(self.base / 'Cargo.toml'), license='MIT OR Apache-2.0', license_file=None,
                 targets=[dict(name='fixture', kind=['lib'], crate_types=['lib'])])
        root = dict(id='root', manifest_path=str(self.root / 'Cargo.toml'), targets=[])
        metadata = dict(packages=[root, p], resolve=dict(root='root', nodes=[
            dict(id='root', features=['default'], deps=[dict(pkg='fixture-id', dep_kinds=[dict(kind=None, target=None)])]),
            dict(id='fixture-id', features=[], deps=[])]))
        self.metadata = {t: copy.deepcopy(metadata) for t in audit.TARGETS}

    def write_payload_manifest(self):
        paths = sorted(audit.BASE_LICENSE_INPUTS) + [f'docs/third-party/sources/{name}-{version}.crate' for name, version in audit.MPL.items()]
        self.payload_manifest.write_text('\n'.join(paths) + '\n')

    def generate(self):
        return audit.generate(self.root, self.metadata)

    def test_determinism_and_full_notices(self):
        a = self.generate()
        self.assertEqual(a, self.generate())
        self.assertIn('Fixture license text', a['THIRD_PARTY_NOTICES.txt'])
        inventory = json.loads(a['docs/third-party/inventory.json'])
        self.assertTrue(inventory['packages'][0]['manual_review'])
        self.assertNotIn(str(self.root), json.dumps(a))

    def test_corrupt_license(self):
        (self.base / 'LICENSE').write_text('corruption')
        with self.assertRaisesRegex(ValueError, 'license content'):
            self.generate()

    def test_missing_license(self):
        (self.base / 'LICENSE').unlink()
        with self.assertRaisesRegex(ValueError, 'missing published license'):
            self.generate()

    def test_corrupt_archive(self):
        self.archive.write_bytes(b'corrupt')
        with self.assertRaisesRegex(ValueError, 'archive checksum'):
            self.generate()

    def test_unsupported_source(self):
        for m in self.metadata.values():
            m['packages'][1]['source'] = 'git+https://invalid.test/repo'
        with self.assertRaises(ValueError):
            self.generate()

    def test_bad_registry_path(self):
        for m in self.metadata.values():
            m['packages'][1]['manifest_path'] = str(self.root / 'Cargo.toml')
        with self.assertRaisesRegex(ValueError, 'source path'):
            self.generate()

    def test_symlink(self):
        (self.base / 'LICENSE').unlink()
        (self.base / 'LICENSE').symlink_to(self.base / 'Cargo.toml')
        with self.assertRaisesRegex(ValueError, 'symlink'):
            self.generate()

    def test_stale_lock(self):
        self.lock.write_text(self.lock.read_text().replace('1.0.0', '9.0.0'))
        with self.assertRaisesRegex(ValueError, 'stale metadata'):
            self.generate()

    def test_extra_evidence(self):
        (self.root / 'docs/third-party/evidence/extra').write_text('unexpected')
        with self.assertRaisesRegex(ValueError, 'evidence file'):
            self.generate()

    def test_corrupt_evidence(self):
        (self.root / 'docs/third-party/evidence/qrcodegen-header').write_text('wrong')
        with self.assertRaisesRegex(ValueError, 'evidence checksum'):
            self.generate()

    def run_check(self):
        args = ['license-inventory.py', '--check']
        for target, metadata in self.metadata.items():
            path = self.root / (target + '.json')
            path.write_text(json.dumps(metadata))
            args.extend(['--metadata', target + '=' + str(path)])
        with mock.patch.object(audit, '__file__', str(self.root / 'scripts/license-inventory.py')), mock.patch('sys.argv', args), contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
            return audit.main()

    def test_check_notices_and_lock_changes(self):
        for path, content in self.generate().items():
            (self.root / path).write_text(content)
        self.assertEqual(self.run_check(), 0)
        notices = self.root / 'THIRD_PARTY_NOTICES.txt'
        original = notices.read_text()
        notices.write_text(original + 'corrupt')
        self.assertEqual(self.run_check(), 1)
        notices.write_text(original)
        self.lock.write_text(self.lock.read_text() + '\n# changed lock bytes\n')
        self.assertEqual(self.run_check(), 1)

    def test_check_rejects_removed_mpl_manifest_entry_even_when_archive_exists(self):
        for path, content in self.generate().items():
            (self.root / path).write_text(content)
        self.assertEqual(self.run_check(), 0)
        entry = f'docs/third-party/sources/cssparser-{audit.MPL["cssparser"]}.crate'
        self.assertTrue((self.root / entry).is_file())
        lines = self.payload_manifest.read_text().splitlines()
        self.payload_manifest.write_text('\n'.join(line for line in lines if line != entry) + '\n')
        self.assertEqual(self.run_check(), 1)

    def test_manifest_requires_exact_reviewed_set_without_duplicates_or_private_paths(self):
        original = self.payload_manifest.read_text().splitlines()
        variants = [original + [original[0]], original[1:],
                    original + ['/private/not-for-distribution'],
                    original + ['docs/third-party/unreviewed.txt']]
        for paths in variants:
            with self.subTest(paths=paths):
                self.payload_manifest.write_text('\n'.join(paths) + '\n')
                with self.assertRaisesRegex(ValueError, 'license payload manifest'):
                    self.generate()

    def test_ordinary_glib_source_edit_is_rejected(self):
        path = self.root / 'vendor/glib/src/object.rs'
        path.write_bytes(path.read_bytes() + b'\n// unreviewed source change\n')
        with self.assertRaisesRegex(ValueError, 'glib source changed'):
            self.generate()

    def test_extra_glib_source_is_rejected(self):
        (self.root / 'vendor/glib/src/unreviewed.rs').write_text('// extra source\n')
        with self.assertRaisesRegex(ValueError, 'glib source extra/missing'):
            self.generate()

    def test_extra_glib_directory_is_rejected(self):
        (self.root / 'vendor/glib/unreviewed').mkdir()
        with self.assertRaisesRegex(ValueError, 'glib source extra/missing'):
            self.generate()

    def test_hardlinked_glib_source_is_rejected(self):
        (self.root / 'object-alias.rs').hardlink_to(self.root / 'vendor/glib/src/object.rs')
        with self.assertRaisesRegex(ValueError, 'glib source must be regular'):
            self.generate()

    def test_missing_glib_source_is_rejected(self):
        (self.root / 'vendor/glib/src/object.rs').unlink()
        with self.assertRaisesRegex(ValueError, 'glib source extra/missing'):
            self.generate()

    def test_symlinked_glib_source_is_rejected(self):
        path = self.root / 'vendor/glib/src/object.rs'
        path.unlink()
        path.symlink_to(self.root / 'vendor/glib/src/lib.rs')
        with self.assertRaisesRegex(ValueError, 'glib source must be regular'):
            self.generate()

    def test_modified_glib_provenance_cannot_authorize_modified_source(self):
        path = self.root / 'vendor/glib/src/object.rs'
        path.write_bytes(path.read_bytes() + b'\n// unreviewed source change\n')
        provenance_path = self.root / 'vendor/glib-0.18.5.provenance.json'
        provenance = json.loads(provenance_path.read_text())
        provenance['original_files']['src/object.rs'] = audit.sha(path.read_bytes())
        provenance_path.write_text(json.dumps(provenance))
        with self.assertRaisesRegex(ValueError, 'glib provenance differs from trusted public tree'):
            self.generate()

    def test_missing_supplier_evidence(self):
        (self.root / 'docs/third-party/evidence/qrcodegen-header').unlink()
        with self.assertRaisesRegex(ValueError, 'evidence file'):
            self.generate()

    def test_corrupt_mpl_payload(self):
        (self.root / 'docs/third-party/sources/fixture-1.0.0.crate').write_bytes(b'corrupt')
        with mock.patch.dict(audit.MPL, {'fixture': '1.0.0'}, clear=True):
            self.write_payload_manifest()
            with self.assertRaisesRegex(ValueError, 'MPL source archive checksum'):
                self.generate()

    def test_modified_mpl_source(self):
        (self.base / 'Cargo.toml').write_text('changed covered source')
        with mock.patch.dict(audit.MPL, {'fixture': '1.0.0'}, clear=True):
            self.write_payload_manifest()
            with self.assertRaisesRegex(ValueError, 'MPL cached source modified'):
                self.generate()

    def test_extra_source(self):
        (self.root / 'docs/third-party/sources/extra.crate').write_text('unexpected')
        with self.assertRaisesRegex(ValueError, 'source archive'):
            self.generate()


class BoundedReaderTests(unittest.TestCase):
    def test_generation_refuses_hardlink_before_truncation(self):
        with tempfile.TemporaryDirectory(prefix='dsh-license-output-') as directory:
            output = Path(directory) / 'generated.txt'
            alias = Path(directory) / 'other-file'
            output.write_bytes(b'original alias must survive')
            os.link(output, alias)
            with self.assertRaisesRegex(ValueError, 'single-link'):
                audit.write_generated(output, 'replacement')
            self.assertEqual(alias.read_bytes(), b'original alias must survive')
            self.assertEqual(output.read_bytes(), alias.read_bytes())

    def test_generation_fifo_is_refused_without_waiting_for_a_reader(self):
        with tempfile.TemporaryDirectory(prefix='dsh-license-output-fifo-') as directory:
            output = Path(directory) / 'generated-fifo'
            os.mkfifo(output)
            program = """import runpy,sys
writer=runpy.run_path(sys.argv[1])['write_generated']
try:
 writer(sys.argv[2], 'must not be written')
except (OSError,ValueError):
 sys.exit(0)
sys.exit('accepted FIFO')
"""
            result = subprocess.run([sys.executable, '-B', '-c', program,
                                     str(REPO / 'scripts/license-inventory.py'), str(output)],
                                    capture_output=True, text=True, timeout=3)
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_fifo_is_rejected_without_waiting_for_a_writer(self):
        with tempfile.TemporaryDirectory(prefix='dsh-license-fifo-') as directory:
            path = Path(directory) / 'input-fifo'
            os.mkfifo(path)
            # Isolate the negative control: an accidentally blocking open must
            # time out only this subprocess, never the suite/CI worker.
            program = """import runpy,sys
reader=runpy.run_path(sys.argv[1])['bounded']
try:
 reader(sys.argv[2])
except ValueError:
 sys.exit(0)
sys.exit('accepted FIFO')
"""
            result = subprocess.run([sys.executable, '-B', '-c', program,
                                     str(REPO / 'scripts/license-inventory.py'), str(path)],
                                    capture_output=True, text=True, timeout=3)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()

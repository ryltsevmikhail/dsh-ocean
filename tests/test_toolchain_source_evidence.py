"""Private mutation controls for pinned compiler source terms, not legal approval."""
import importlib.util
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('toolchain_sources', ROOT / 'scripts/check-toolchain-source-evidence.py')
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class SourceProfile(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='dsh-toolchain-source-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.profile = self.root / audit.PROFILE
        shutil.copytree(ROOT / audit.PROFILE, self.profile)

    def test_checked_in_profile_is_valid(self):
        self.assertEqual(audit.validate(self.root)['compiler_source_commit'], audit.COMMIT)

    def test_unicode_terms_are_pinned_and_mutations_rejected(self):
        path = self.profile / 'LICENSE-RUST-UNICODE'
        self.assertTrue(path.is_file(), 'retain the exact upstream Unicode-3.0 text')
        self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(),
                         'f5062c9a188d81dfe66b56db4182dcf9e4b17c0d9b0d311a8e20b3a1b075c443')
        index = audit.validate(self.root)
        row = next(row for row in index['files'] if row['file'] == path.name)
        self.assertEqual(row['upstream_path'], 'LICENSES/Unicode-3.0.txt')
        path.write_bytes(path.read_bytes() + b'changed')
        with self.assertRaisesRegex(ValueError, 'hash mismatch'):
            audit.validate(self.root)

    def test_license_mutation_is_not_accepted(self):
        with (self.profile / 'LICENSE-COMPILER-BUILTINS').open('ab') as out:
            out.write(b'changed')
        with self.assertRaisesRegex(ValueError, 'hash mismatch'):
            audit.validate(self.root)

    def test_index_cannot_bless_different_terms(self):
        (self.profile / 'index.json').write_text('{}')
        with self.assertRaisesRegex(ValueError, 'unreviewed'):
            audit.validate(self.root)

    def test_extra_source_is_not_silently_ignored(self):
        (self.profile / 'unreviewed.txt').write_text('extra')
        with self.assertRaisesRegex(ValueError, 'file set'):
            audit.validate(self.root)

    def test_hardlink_does_not_count_as_immutable_evidence(self):
        os.link(self.profile / 'LICENSE-RUST-MIT', self.root / 'alias')
        with self.assertRaisesRegex(ValueError, 'single-link'):
            audit.validate(self.root)

    def test_fifo_reader_is_time_bounded(self):
        fifo = self.root / 'COPYRIGHT-fifo'
        os.mkfifo(fifo)
        code = """import runpy,sys
reader=runpy.run_path(sys.argv[1])['read_regular']
try: reader(sys.argv[2])
except ValueError: sys.exit(0)
sys.exit('accepted FIFO')
"""
        result = subprocess.run([sys.executable, '-B', '-c', code,
                                 str(ROOT / 'scripts/check-toolchain-source-evidence.py'), str(fifo)],
                                capture_output=True, text=True, timeout=3)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()

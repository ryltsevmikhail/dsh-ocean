"""Exact bytes and target architecture must survive into the executable, not an addon."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/check-embedded-locales.py'
FILES = [ROOT / 'locales/ocean' / (language + '.json') for language in ('en', 'ru', 'es', 'pt-BR', 'de', 'fr', 'ja', 'ko')]
FILES += [ROOT / 'locales/dsh' / name for name in ('LICENSE', 'builtin-locale-preflight.mjs', 'builtin-locale-sdk-preflight.mjs', 'lib/client.js', 'lib/index.js', 'overlay.yml', 'package.json')]

class RetentionTests(unittest.TestCase):
    def check(self, machine=62, omitted=None, architecture='x86_64', elf=True):
        with tempfile.TemporaryDirectory(prefix='ocean-embedded-test-') as temporary:
            binary = Path(temporary) / 'candidate'
            header = bytearray(64)
            header[:7] = b'\x7fELF\x02\x01\x01' if elf else b'not-elf'
            header[16:18] = (3).to_bytes(2, 'little')
            header[18:20] = machine.to_bytes(2, 'little')
            payload = bytes(header) + b'\0'.join(file.read_bytes() for file in FILES if file != omitted)
            binary.write_bytes(payload)
            result = subprocess.run([sys.executable, '-B', str(SCRIPT), '--binary', str(binary), '--arch', architecture], capture_output=True, text=True)
            return result, payload

    def test_both_native_elf_targets_report_exact_all_language_resource_bytes(self):
        for machine, architecture in [(62, 'x86_64'), (183, 'aarch64')]:
            with self.subTest(architecture=architecture):
                result, payload = self.check(machine=machine, architecture=architecture)
                self.assertEqual(result.returncode, 0, result.stderr)
                report = json.loads(result.stdout)
                self.assertEqual(report['elfSha256'], hashlib.sha256(payload).hexdigest())
                self.assertEqual(report['architecture'], architecture)
                self.assertEqual(len(report['resources']), 15)
                self.assertEqual({entry['path'] for entry in report['resources']}, {str(file.relative_to(ROOT)) for file in FILES})
                self.assertFalse(report['releaseApproval'])

    def test_missing_dictionary_worker_or_license_and_wrong_elf_fail_closed(self):
        for omitted in [FILES[1], ROOT / 'locales/dsh/lib/client.js', ROOT / 'locales/dsh/LICENSE']:
            with self.subTest(omitted=omitted):
                result, _ = self.check(omitted=omitted)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, '')
        for kwargs in [{'machine': 183}, {'elf': False}, {'architecture': 'riscv64'}]:
            result, _ = self.check(**kwargs)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, '')

if __name__ == '__main__':
    unittest.main()

"""Isolated synthetic artifacts only; never invokes the user's compiler."""
import hashlib
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/artifact-license-evidence.py'
spec = importlib.util.spec_from_file_location('artifact_evidence', SCRIPT)
evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evidence)


def fixture(machine=62, rpath=b'/home/private-person/cache:$ORIGIN/../lib:relative-secret', needed=b'libc.so.6'):
    strings = b'\0' + needed + b'\0' + rpath + b'\0'
    tags = [(5, 0x400200), (10, len(strings)), (1, 1), (29, len(needed) + 2), (0, 0)]
    data = bytearray(0x200 + len(strings))
    data[:16] = b'\x7fELF\x02\x01\x01' + b'\0' * 9
    struct.pack_into('<HHIQQQIHHHHHH', data, 16, 3, machine, 1, 0, 64, 0, 0, 64, 56, 3, 0, 0, 0)
    struct.pack_into('<IIQQQQQQ', data, 64, 1, 5, 0, 0x400000, 0, len(data), len(data), 4096)
    struct.pack_into('<IIQQQQQQ', data, 120, 2, 4, 0x100, 0x400100, 0, len(tags) * 16, len(tags) * 16, 8)
    interp = b'/lib64/ld-linux-x86-64.so.2\0'
    struct.pack_into('<IIQQQQQQ', data, 176, 3, 4, 0x1d0, 0x4001d0, 0, len(interp), len(interp), 1)
    data[0x1d0:0x1d0 + len(interp)] = interp
    for i, pair in enumerate(tags):
        struct.pack_into('<qQ', data, 0x100 + 16 * i, *pair)
    data[0x200:] = strings
    return data


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.binary = self.root / 'private-binary'
        self.binary.write_bytes(fixture())
        self.compiler = self.root / 'fake-rustc'
        self.compiler.write_text('#!' + sys.executable + '\nimport sys\nassert sys.argv[1:] == ["-vV"]\nprint("rustc 1.80.0")\nprint("release: 1.80.0")\nprint("host: x86_64-unknown-linux-gnu")\nprint("commit-hash: ' + 'a' * 40 + '")\nprint("LLVM version: 18.1.7")\nprint("private: /home/private-person")\n')
        self.compiler.chmod(0o700)

    def test_known_bytes_direct_dependencies_and_privacy(self):
        result = evidence.inspect_elf(self.binary)
        self.assertEqual(result['sha256'], hashlib.sha256(self.binary.read_bytes()).hexdigest())
        self.assertEqual(result['size'], self.binary.stat().st_size)
        self.assertEqual(result['elf']['direct_needed'][0]['name'], 'libc.so.6')
        self.assertNotIn('private-person', json.dumps(result))
        self.assertEqual(result['elf']['search_paths'][0]['entries'][0]['classification'], 'absolute-redacted')
        self.binary.write_bytes(fixture(183, needed=b'/home/secret/lib.so'))
        result = evidence.inspect_elf(self.binary)
        self.assertEqual(result['elf']['machine'], 'aarch64')
        self.assertEqual(result['elf']['direct_needed'][0]['classification'], 'suspicious-redacted')
        self.assertNotIn('secret', json.dumps(result))

    def test_malformed_headers_and_bounds(self):
        cases = []
        cases.append(fixture(3))
        x = fixture(); x[4] = 1; cases.append(x)
        x = fixture(); struct.pack_into('<Q', x, 32, 2**63); cases.append(x)
        x = fixture(); struct.pack_into('<Q', x, 64 + 32, len(x) + 1); cases.append(x)
        x = fixture(); struct.pack_into('<H', x, 56, 65535); cases.append(x)
        x = fixture(); x[-1] = 65; cases.append(x)
        x = fixture(); struct.pack_into('<Q', x, 0x128, 999999); cases.append(x)
        x = fixture(); struct.pack_into('<qQ', x, 0x140, 1, 1); cases.append(x)
        x = fixture(); struct.pack_into('<qQ', x, 0x130, 5, 0x400200); cases.append(x)
        x = fixture(); struct.pack_into('<I', x, 176, 1); cases.append(x)
        x = fixture(); struct.pack_into('<Q', x, 0x108, 0xfffffff); cases.append(x)
        x = fixture(); struct.pack_into('<Q', x, 120 + 16, 0x400110); cases.append(x)
        x = fixture(); struct.pack_into('<qQ', x, 0x130, 1, 1); cases.append(x)
        x = fixture(); x[0x200] = 65; cases.append(x)
        x = fixture(); x[0x1d0 + 3] = 0; cases.append(x)
        for index, data in enumerate(cases):
            with self.subTest(index=index):
                self.binary.write_bytes(data)
                with self.assertRaises(evidence.EvidenceError):
                    evidence.inspect_elf(self.binary)

    def test_symlink_fifo_and_notice_hardlinks_refused_but_cargo_alias_readonly(self):
        link = self.root / 'link'
        link.symlink_to(self.binary)
        with self.assertRaises(OSError):
            evidence.inspect_elf(link)
        link.unlink()
        os.link(self.binary, link)
        original = self.binary.read_bytes()
        self.assertEqual(evidence.inspect_elf(self.binary)['sha256'], hashlib.sha256(original).hexdigest())
        self.assertEqual(link.read_bytes(), original)  # No alias is modified.
        with self.assertRaises(evidence.EvidenceError):
            evidence.read_regular(self.binary)  # Non-artifact inputs stay single-link.
        fifo = self.root / 'fifo'
        os.mkfifo(fifo)
        start = time.monotonic()
        process = subprocess.run([sys.executable, str(SCRIPT), '--binary', str(fifo), '--output-dir', str(self.root / 'out'), '--rustc', str(self.compiler)], capture_output=True, timeout=3)
        self.assertEqual(process.returncode, 1)
        self.assertLess(time.monotonic() - start, 3)
        with self.assertRaises(evidence.EvidenceError):
            evidence.read_regular(fifo)

    def test_compiler_limits_and_whitelist(self):
        result = evidence.observe_compiler(self.compiler)
        self.assertEqual(result['release'], '1.80.0')
        self.assertEqual(result['rust_source_url'], 'https://github.com/rust-lang/rust/tree/' + 'a' * 40)
        self.assertNotIn('private', json.dumps(result))
        self.compiler.write_text('#!' + sys.executable + '\nimport os\nwhile True: os.write(2, b"x"*8192)\n')
        with self.assertRaises(evidence.EvidenceError):
            evidence.observe_compiler(self.compiler, timeout=.5, limit=10000)
        self.compiler.write_text('#!' + sys.executable + '\nimport time\ntime.sleep(30)\n')
        start = time.monotonic()
        with self.assertRaises(evidence.EvidenceError):
            evidence.observe_compiler(self.compiler, timeout=.2)
        self.assertLess(time.monotonic() - start, 2)

    def test_output_determinism_notices_inputs_and_collision(self):
        notice = self.root / 'COPYRIGHT-library'
        notice.write_bytes(b'Public copyright https://example.org/license\n')
        (self.root / 'Cargo.lock').write_bytes(b'lock bytes')
        inv = self.root / 'docs/third-party'
        inv.mkdir(parents=True)
        (inv / 'inventory.json').write_bytes(b'{}')
        a, b = self.root / 'a', self.root / 'b'
        one = evidence.create_evidence(self.binary, a, self.compiler, [('std', notice)], root=self.root)
        two = evidence.create_evidence(self.binary, b, self.compiler, [('std', notice)], root=self.root)
        self.assertEqual(one, two)
        self.assertEqual((a / 'report.json').read_bytes(), (b / 'report.json').read_bytes())
        self.assertEqual((a / 'notices/std-COPYRIGHT-library').read_bytes(), notice.read_bytes())
        self.assertEqual(len(one['observed_inputs']), 2)
        self.assertEqual(a.stat().st_mode & 0o777, 0o700)
        self.assertEqual((a / 'report.json').stat().st_mode & 0o777, 0o600)
        self.assertNotIn(str(self.root), json.dumps(one))
        self.assertTrue(all(type(v) is bool for v in one['environment_present'].values()))
        before = (a / 'report.json').read_bytes()
        with self.assertRaises(FileExistsError):
            evidence.create_evidence(self.binary, a, self.compiler, root=self.root)
        self.assertEqual(before, (a / 'report.json').read_bytes())
        with self.assertRaises(FileExistsError):
            evidence.create_evidence(self.binary, self.binary, self.compiler, root=self.root)
        alias = self.root / 'alias'
        alias.symlink_to(a, target_is_directory=True)
        with self.assertRaises(OSError):
            evidence.create_evidence(self.binary, alias / 'child', self.compiler, root=self.root)

    def test_relative_notice_from_hidden_cwd_is_refused(self):
        hidden = self.root / '.profile'
        hidden.mkdir()
        (hidden / 'LICENSE').write_bytes(b'not an approved public source')
        with contextlib.chdir(hidden):
            with self.assertRaisesRegex(evidence.EvidenceError, 'hidden'):
                evidence.create_evidence(self.binary, self.root / 'hidden-out', str(self.compiler), [('relative', 'LICENSE')], self.root)
        self.assertFalse((self.root / 'hidden-out').exists())

    def test_notice_count_and_aggregate_bounds_fail_before_output(self):
        source = self.root / 'LICENSE-candidate'
        source.write_bytes(b'12345')
        with mock.patch.object(evidence, 'MAX_NOTICE_COUNT', 1):
            with self.assertRaisesRegex(evidence.EvidenceError, 'count'):
                evidence.create_evidence(self.binary, self.root / 'count-out', str(self.compiler), [('one', source), ('two', source)], self.root)
        self.assertFalse((self.root / 'count-out').exists())
        with mock.patch.object(evidence, 'MAX_TOTAL_NOTICE', 9):
            with self.assertRaisesRegex(evidence.EvidenceError, 'total notice'):
                evidence.create_evidence(self.binary, self.root / 'bytes-out', str(self.compiler), [('one', source), ('two', source)], self.root)
        self.assertFalse((self.root / 'bytes-out').exists())

    def test_notice_validation(self):
        notice = self.root / 'NOTICE'
        notice.write_bytes(b'notice')
        for notices in ([('bad/label', notice)], [('x', notice), ('x', notice)], [('x', self.binary)]):
            with self.assertRaises(evidence.EvidenceError):
                evidence.create_evidence(self.binary, self.root / 'out', self.compiler, notices, root=self.root)
        hidden = self.root / '.profile'
        hidden.write_bytes(b'secret')
        with self.assertRaises(evidence.EvidenceError):
            evidence.create_evidence(self.binary, self.root / 'out', self.compiler, [('x', hidden)], root=self.root)


if __name__ == '__main__':
    unittest.main()

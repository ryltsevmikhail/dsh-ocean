"""Release toolchain notices must be real retained inputs, not a report-only claim."""
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT/'scripts/check-release-toolchain.py'
PROFILE = ROOT/'docs/toolchain/rust-1.90.0'

class ReleaseToolchain(unittest.TestCase):
    def test_reviewed_profile_is_complete_and_packaged(self):
        result = subprocess.run(['python3', str(SCRIPT)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        index = json.loads((PROFILE/'index.json').read_text())
        self.assertEqual(index['rustcCommit'], '1159e78c4747b02ef996e55082b704c09b970588')
        self.assertEqual(set(index['targets']), {'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu'})
        manifest = set(line for line in (ROOT/'packaging/license-inputs.txt').read_text().splitlines()
                       if line and not line.startswith('#'))
        required = {'docs/toolchain/rust-1.90.0/index.json'} | {
            'docs/toolchain/rust-1.90.0/'+f['path'] for f in index['files']}
        self.assertTrue(required <= manifest, required-manifest)
        self.assertIn('docs/toolchain/rust-1.90.0/COPYRIGHT-library.html', manifest)

    def test_observed_native_arm_members_have_exact_retained_source_and_license(self):
        import hashlib
        import tarfile
        manifest=json.loads((PROFILE/'COMPILER-RT-AARCH64-index.json').read_text())
        expected={e['path']:e for e in manifest['files']}
        self.assertEqual(len(expected),11)
        self.assertIn('compiler-rt/LICENSE.TXT',expected)
        self.assertIn('compiler-rt/lib/builtins/cpu_model/aarch64/fmv/getauxval.inc',expected)
        with tarfile.open(PROFILE/'COMPILER-RT-AARCH64-source.tar.gz') as archive:
            actual={}
            for m in archive.getmembers():
                self.assertTrue(m.isfile())
                name=m.name.split('/',1)[1]
                self.assertIn(name,expected)
                self.assertNotIn(name,actual)
                actual[name]=hashlib.sha256(archive.extractfile(m).read()).hexdigest()
            self.assertEqual(actual,{p:e['sha256'] for p,e in expected.items()})
        with tarfile.open(PROFILE/'rust-src-1.90.0.tar.xz') as archive:
            script=archive.extractfile('rust-src-1.90.0/rust-src/lib/rustlib/src/rust/library/compiler-builtins/compiler-builtins/build.rs').read()
            self.assertEqual(hashlib.sha256(script).hexdigest(),manifest['rustBuildScript']['sha256'])

    def test_native_startup_notices_preserve_the_actual_linking_exceptions(self):
        for arch in ('x86_64','aarch64'):
            for name in ('start.S','crti.S','crtn.S'):
                text=' '.join((PROFILE/f'GNU-glibc-{arch}-{name}').read_text().split())
                self.assertIn('unlimited permission to link the compiled version',text)
                self.assertIn('without any restriction coming from the use of this file',text)
        self.assertIn('GNU LESSER GENERAL PUBLIC LICENSE',(PROFILE/'GNU-glibc-COPYING.LIB').read_text())
        self.assertIn('GCC Runtime Library Exception',(PROFILE/'GNU-gcc-crtstuff.c').read_text())

    def test_actual_compiler_and_installed_notice_identity_are_required(self):
        with tempfile.TemporaryDirectory(prefix='ocean-installed-toolchain-') as tmp:
            tmp=Path(tmp);sysroot=tmp/'sysroot';installed=sysroot/'share/doc/rust'
            shutil.copytree(PROFILE, installed)
            compiler=tmp/'rustc'
            def write_compiler(version):
                compiler.write_text('#!/bin/sh\nprintf "%s\\n" "release: '+version+'" "commit-hash: 1159e78c4747b02ef996e55082b704c09b970588" "host: x86_64-unknown-linux-gnu"\n')
                compiler.chmod(0o700)
            command=['python3',str(SCRIPT),'--sysroot',str(sysroot),'--rustc',str(compiler),'--target','x86_64-unknown-linux-gnu']
            write_compiler('1.99.1')
            wrong=subprocess.run(command,capture_output=True,text=True)
            self.assertNotEqual(wrong.returncode,0)
            self.assertIn('compiler',wrong.stderr)
            write_compiler('1.90.0')
            good=subprocess.run(command,capture_output=True,text=True)
            self.assertEqual(good.returncode,0,good.stderr)
            (installed/'COPYRIGHT-library.html').write_bytes(b'changed sysroot notice')
            wrong=subprocess.run(command,capture_output=True,text=True)
            self.assertNotEqual(wrong.returncode,0)
            self.assertIn('installed toolchain notice mismatch',wrong.stderr)

    def test_changed_public_notice_is_refused_not_repaired(self):
        with tempfile.TemporaryDirectory(prefix='ocean-toolchain-notices-') as tmp:
            profile = Path(tmp)/'profile'
            shutil.copytree(PROFILE, profile)
            notice = profile/'COPYRIGHT-library.html'
            notice.write_bytes(notice.read_bytes()+b'changed')
            result = subprocess.run(['python3', str(SCRIPT), '--profile', str(profile)], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('notice', result.stderr.lower())
            self.assertTrue(notice.read_bytes().endswith(b'changed'))

if __name__ == '__main__': unittest.main()

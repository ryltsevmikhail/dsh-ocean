import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import subprocess
import sys
import os
import textwrap

SPEC = importlib.util.spec_from_file_location('prebuilt', Path(__file__).resolve().parents[1] / 'scripts/check-prebuilt-release.py')
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class PrebuiltReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.archive = self.root / 'candidate.tar.gz'
        self.archive.write_bytes(b'exact accepted bytes')
        self.commit = 'a' * 40
        self.lock = dict(product='runtime', channel='next', target='linux-x86_64',
                         release='1.0.0-rc.1', sequence=1, sourceCommit=self.commit,
                         filename=self.archive.name, size=self.archive.stat().st_size,
                         sha256=hashlib.sha256(self.archive.read_bytes()).hexdigest())

    def test_accepts_exact_bytes_and_source(self):
        self.assertEqual(module.validate(self.lock, self.root, self.commit), self.archive)

    def test_rejects_mutated_archive(self):
        self.archive.write_bytes(b'substituted payload!!')
        with self.assertRaises(ValueError):
            module.validate(self.lock, self.root, self.commit)

    def test_rejects_tag_at_workflow_commit_instead_of_build_commit(self):
        with self.assertRaises(ValueError):
            module.validate(self.lock, self.root, 'b' * 40)

    def test_rejects_path_traversal(self):
        self.lock['filename'] = '../candidate.tar.gz'
        with self.assertRaises(ValueError):
            module.validate(self.lock, self.root, self.commit)

    def test_rejects_symlink(self):
        original = self.archive.rename(self.root / 'actual.tar.gz')
        self.archive.symlink_to(original)
        with self.assertRaises(ValueError):
            module.validate(self.lock, self.root, self.commit)

    def test_shell_requires_matching_native_evidence(self):
        self.lock['product'] = 'shell'
        with self.assertRaises(ValueError):
            module.validate(self.lock, self.root, self.commit)
        evidence = dict(sourceCommit=self.commit, archive=dict(archiveSha256=self.lock['sha256'], archiveSize=self.lock['size']))
        (self.root / 'native-evidence.json').write_text(json.dumps(evidence))
        self.assertEqual(module.validate(self.lock, self.root, self.commit), self.archive)
        evidence['sourceCommit'] = 'b' * 40
        (self.root / 'native-evidence.json').write_text(json.dumps(evidence))
        with self.assertRaises(ValueError):
            module.validate(self.lock, self.root, self.commit)

    def test_workflow_metadata_layout_generates_manifest(self):
        repo = Path(__file__).resolve().parents[1]
        workflow = (repo / '.github/workflows/prebuilt-rc.yml').read_text()
        # Resolve the actual workflow's output location, then run both metadata
        # commands with real canonical archive names. This catches placing the
        # descriptor outside the directory whose bytes manifest() rehashes.
        artifact_line = next(line.strip() for line in workflow.splitlines()
                             if 'scripts/release-artifacts.py artifact ' in line)
        output = artifact_line.split(' > ', 1)[1].strip('"')
        output = Path(output.replace('$RUNNER_TEMP', str(self.root)))
        for product, version, sequence, filename in [
            ('shell', '0.1.2-rc.2', '2', 'dsh-ocean-0.1.2-rc.2-x86_64.tar.gz'),
            ('runtime', '0.2.0-rc.2.ocean.2', '1', 'dsh-ocean-runtime-next-0.2.0-rc.2.ocean.2-1-x86_64.tar.gz')]:
            directory = self.root / 'prebuilt'
            directory.mkdir(exist_ok=True)
            archive = directory / filename
            archive.write_bytes(b'bounded metadata test payload')
            base = [sys.executable, str(repo / 'scripts/release-artifacts.py')]
            flags = ['--product', product, '--channel', 'next', '--release', version, '--sequence', sequence]
            description = subprocess.check_output(base + ['artifact'] + flags + ['--target', 'linux-x86_64', '--file', str(archive)])
            output.write_bytes(description)
            result = subprocess.run(base + ['manifest'] + flags + ['--publication-target', 'linux-x86_64', '--artifact', str(output)], capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr.decode())

    def run_remote_publication_fixture(self, scenario):
        repo = Path(__file__).resolve().parents[1]
        workflow = (repo / '.github/workflows/prebuilt-rc.yml').read_text()
        section = workflow.split('      - name: Upload signatures and optionally publish next prerelease\n', 1)[1]
        script = textwrap.dedent(section.split('        run: |\n', 1)[1])
        self.lock['tag'] = 'runtime-next-v1.0.0-rc.1-r1'
        (self.root / 'lock.json').write_text(json.dumps(self.lock))
        prebuilt = self.root / 'prebuilt'
        prebuilt.mkdir()
        self.archive.rename(prebuilt / self.archive.name)
        signature = self.root / 'signature'
        signature.mkdir()
        (self.root / 'manifest.json').write_bytes(b'manifest')
        (signature / 'signature.json').write_bytes(b'signature')
        (signature / 'public-key.json').write_bytes(b'public-key')
        snapshot = dict(id=42, draft=True, assets=[dict(id=1, name=self.archive.name)])
        (self.root / 'draft.json').write_text(json.dumps(snapshot))
        (self.root / 'scenario').write_text(scenario)
        fake = self.root / 'fake-bin'
        fake.mkdir()
        gh = fake / 'gh'
        gh.write_text('''#!/usr/bin/python3
import json,os,pathlib,sys
r=pathlib.Path(os.environ['RUNNER_TEMP']); args=sys.argv[1:]; scenario=(r/'scenario').read_text()
if args[:2] == ['release','upload']: sys.exit(0)
if '--method' in args:
 (r/'published').write_text('yes'); sys.exit(0)
endpoint=next(x for x in args if x.startswith('repos/'))
if '/commits/' in endpoint:
 print('a'*40); sys.exit(0)
if '/releases/tags/' in endpoint:
 data=dict(id=43 if scenario=='recreated' else 42,draft=True,assets=[
  dict(id=9 if scenario=='replaced-id' else 1,name='candidate.tar.gz'),
  dict(id=2,name='manifest.json'),dict(id=3,name='signature.json'),dict(id=4,name='public-key.json')])
 print(json.dumps(data)); sys.exit(0)
if '/releases/assets/' in endpoint:
 identity=endpoint.rsplit('/',1)[1]
 paths={'1':r/'prebuilt/candidate.tar.gz','2':r/'manifest.json','3':r/'signature/signature.json','4':r/'signature/public-key.json'}
 sys.stdout.buffer.write(b'changed remote bytes' if scenario=='changed-bytes' and identity=='1' else paths[identity].read_bytes()); sys.exit(0)
raise SystemExit('unexpected network command: '+repr(args))
''')
        gh.chmod(0o700)
        env = dict(os.environ, RUNNER_TEMP=str(self.root), PRODUCT='runtime',
                   GITHUB_REPOSITORY='fixture/repo', PUBLISH='true',
                   PATH=str(fake) + ':/usr/bin:/bin')
        result = subprocess.run(['bash', '-c', script], cwd=repo, env=env, capture_output=True)
        return result, (self.root / 'published').exists()

    def test_remote_exact_assets_publish_captured_release_id(self):
        result, published = self.run_remote_publication_fixture('exact')
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertTrue(published)

    def test_remote_replaced_asset_id_refuses_publication(self):
        result, published = self.run_remote_publication_fixture('replaced-id')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(published)

    def test_remote_recreated_draft_refuses_publication(self):
        result, published = self.run_remote_publication_fixture('recreated')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(published)

    def test_remote_changed_bytes_refuse_publication(self):
        result, published = self.run_remote_publication_fixture('changed-bytes')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(published)


if __name__ == '__main__':
    unittest.main()

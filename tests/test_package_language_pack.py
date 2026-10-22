"""Optional release asset: build from sources; reject tampering and profile leaks."""
import hashlib
import io
import tarfile
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class LanguagePackRelease(unittest.TestCase):
    def test_build_and_verify_exact_asset_and_reject_tampered_bytes(self):
        with tempfile.TemporaryDirectory(prefix='ocean-locale-release-') as tmp:
            command = ['python3', str(ROOT/'scripts/package-language-pack.py')]
            build = subprocess.run(command + ['--output-dir', tmp], capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            metadata = json.loads((Path(tmp)/'language-pack-artifact.json').read_text())
            self.assertEqual(metadata['version'], '0.4.0')
            self.assertEqual(metadata['filename'], 'dsh-ocean-desktop-language-pack-0.4.0.tgz')
            verify = subprocess.run(command + ['--verify', tmp], capture_output=True, text=True)
            self.assertEqual(verify.returncode, 0, verify.stderr)
            archive = Path(tmp)/metadata['filename']
            original = archive.read_bytes()
            with tarfile.open(fileobj=io.BytesIO(original), mode='r:gz') as source:
                items = [(m, source.extractfile(m).read()) for m in source.getmembers()]
            with tarfile.open(archive, 'w:gz') as target:
                for member, data in items:
                    target.addfile(member, io.BytesIO(data))
                leak = tarfile.TarInfo('package/artifacts/private-profile.json')
                leak.size = 2
                target.addfile(leak, io.BytesIO(b'{}'))
            data = archive.read_bytes()
            metadata.update(size=len(data), sha256=hashlib.sha256(data).hexdigest())
            (Path(tmp)/'language-pack-artifact.json').write_text(json.dumps(metadata))
            refused = subprocess.run(command + ['--verify', tmp], capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn('packed paths', refused.stderr.lower())
            archive.write_bytes(data + b'tampered')
            refused = subprocess.run(command + ['--verify', tmp], capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn('checksum', refused.stderr.lower())

if __name__ == '__main__': unittest.main()

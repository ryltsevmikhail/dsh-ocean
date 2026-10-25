"""The standalone language pack tests its declared peer, not a shell release pin."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class LanguagePackRuntimePin(unittest.TestCase):
    def test_shell_runtime_version_does_not_replace_language_pack_contract(self):
        script = (ROOT / 'scripts/test-language-pack.sh').read_text()
        assignment = next(line for line in script.splitlines() if line.startswith('DSH_VERSION='))
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'language-pack').mkdir()
            (root / 'runtime-manifest.json').write_text(json.dumps(dict(dsh=dict(version='0.2.0-rc.2.ocean.2'))))
            (root / 'language-pack/package.json').write_text(json.dumps(dict(engines=dict(dsh='0.2.0-rc.2'))))
            result = subprocess.check_output(['bash', '-eu', '-c', assignment + '\nprintf %s "$DSH_VERSION"'], env=dict(ROOT=str(root), PATH='/usr/bin:/bin'), text=True)
            self.assertEqual(result, '0.2.0-rc.2')


if __name__ == '__main__':
    unittest.main()

"""Exercise CI environment generation: wrong native checksum must fail before bootstrap."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/ci-sdk-env.py'

class EnvironmentTests(unittest.TestCase):
    def invoke(self, arch, mutate=None, root=None):
        with tempfile.TemporaryDirectory(prefix='ocean-ci-env-test-') as temporary:
            directory = Path(temporary)
            manifest = {'schema': 1, 'node': {'version': '26.12.0', 'archives': {'linux-x64': {'sha256': 'a'*64}, 'linux-arm64': {'sha256': 'b'*64}}}, 'dsh': {'package': '@deepseek-ai/dsh', 'version': '0.2.0-rc.2'}, 'pnpm': {'version': '10.34.7'}}
            if mutate:
                mutate(manifest)
            file = directory / 'manifest.json'
            file.write_text(json.dumps(manifest))
            return subprocess.run([sys.executable, '-B', str(SCRIPT), '--manifest', str(file), '--arch', arch, '--fixture-root', root or str(directory / 'fixture')], capture_output=True, text=True)

    def test_native_checksum_and_fixture_sources_are_architecture_specific(self):
        for arch, checksum in [('x86_64', 'a'*64), ('aarch64', 'b'*64)]:
            with self.subTest(arch=arch):
                result = self.invoke(arch)
                self.assertEqual(result.returncode, 0, result.stderr)
                values = dict(line.split('=', 1) for line in result.stdout.splitlines())
                self.assertEqual(values['DSH_NODE_SHA256'], checksum)
                self.assertEqual(values['DSH_VERSION'], '0.2.0-rc.2')
                self.assertTrue(values['DSH_OCEAN_SDK_FIXTURE'].endswith('/fixture/ocean-data/current'))
                self.assertTrue(values['DSH_LOCALE_CLIENT_SOURCE'].endswith('/@deepseek-ai/dsh-client-locale/lib/client.js'))
                self.assertTrue(values['DSH_CLIENT_MODULES_SOURCE'].endswith('/@deepseek-ai/dsh-client-modules/lib/client.js'))
                self.assertNotIn('DSH_OCEAN_TEST_RUNTIME_ROOT', values, 'canonical generation must be resolved after installation')
                self.assertNotIn('DSH_HOME', values)
                self.assertNotIn('DSH_OCEAN_DATA_DIR', values)

    def test_invalid_architecture_pins_or_path_emit_no_partial_environment(self):
        for arch, mutate, root in [('riscv64', None, None), ('aarch64', lambda m: m['node']['archives']['linux-arm64'].update(sha256='bad'), None), ('x86_64', lambda m: m['dsh'].update(version='latest'), None), ('x86_64', None, 'relative'), ('x86_64', None, '/tmp/fixture\nINJECTED=yes')]:
            with self.subTest(arch=arch, root=root):
                result = self.invoke(arch, mutate, root)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, '')

if __name__ == '__main__':
    unittest.main()

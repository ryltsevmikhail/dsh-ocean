"""The protected `test` context must aggregate, not bypass, both native runners."""
import os
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]

class NativeGateTests(unittest.TestCase):
    def gate(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        match = re.search(r'^  native-gate:\n(.*?)(?=^  [a-z][a-z0-9_-]*:\n|\Z)', workflow, re.M | re.S)
        self.assertIsNotNone(match, 'protected test context requires a fail-closed native matrix aggregate')
        return match.group(1)

    def test_protected_context_depends_on_complete_native_matrix(self):
        gate = self.gate()
        for contract in ['name: test', 'needs: test', 'if: always()', 'NATIVE_RESULT: ${{ needs.test.result }}']:
            self.assertIn(contract, gate)
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertIn('runner: [ubuntu-24.04, ubuntu-24.04-arm]', workflow)

    def test_actual_shell_gate_rejects_failure_cancellation_skip_and_missing_result(self):
        commands = re.findall(r'^\s+run: (.+)$', self.gate(), re.M)
        self.assertEqual(len(commands), 1)
        for outcome in ['success', 'failure', 'cancelled', 'skipped', '']:
            with self.subTest(outcome=outcome):
                result = subprocess.run(['bash', '-ec', commands[0]], env={**os.environ, 'NATIVE_RESULT': outcome}, capture_output=True)
                self.assertEqual(result.returncode == 0, outcome == 'success')

if __name__ == '__main__':
    unittest.main()

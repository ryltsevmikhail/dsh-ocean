"""Make front doors must fail before building or fetching without explicit trust."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class MakeTargets(unittest.TestCase):
    def test_appimage_requires_explicit_local_tool_and_preserves_arguments(self):
        with tempfile.TemporaryDirectory(prefix="dsh-make-test-") as temp:
            root = Path(temp)
            shutil.copyfile(ROOT / "Makefile", root / "Makefile")
            (root / "scripts").mkdir()
            log = root / "args.json"
            (root / "scripts/package-appimage.sh").write_text(
                'exec "$PYTHON" -c \'import json, os, sys; '
                'open(os.environ["ARG_LOG"], "w").write(json.dumps(sys.argv[1:]))\' "$@"\n')
            env = {**os.environ, "PYTHON": sys.executable, "ARG_LOG": str(log)}
            env.pop("APPIMAGE_TOOL", None)
            env.pop("APPIMAGE_TOOL_SHA256", None)
            result = subprocess.run(["make", "package-appimage"], cwd=root, env=env,
                                    capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Set APPIMAGE_TOOL", result.stderr)
            self.assertFalse(log.exists())
            # Shell metacharacters are data, not Make-expanded shell source.
            tool = str(root / 'tool " $(touch INJECTED) `touch INJECTED` ; image')
            env.update(APPIMAGE_TOOL=tool, APPIMAGE_TOOL_SHA256="a" * 64)
            result = subprocess.run(["make", "package-appimage"], cwd=root, env=env,
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(log.read_text()),
                             ["--tool", tool, "--tool-sha256", "a" * 64])
            self.assertFalse((root / "INJECTED").exists())


if __name__ == "__main__":
    unittest.main()

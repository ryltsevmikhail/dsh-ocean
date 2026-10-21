"""Publisher archive tests: private synthetic payloads, no npm/network/user profile."""
import hashlib
import json
from pathlib import Path
import subprocess
import shutil
import sys
import tarfile
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/package-runtime.py"

class RuntimePackage(unittest.TestCase):
    def test_complete_payload_and_regular_exec_shims_are_packaged_without_source_mutation(self):
        node=shutil.which("node")
        if not node:
            self.skipTest("in-place shim acceptance needs pinned Node with process.execve")
        capability=subprocess.run([node,"-p","typeof process.execve"],capture_output=True,text=True,env={"PATH":"/usr/bin:/bin"})
        if capability.returncode or capability.stdout.strip()!="function":
            self.skipTest("in-place shim acceptance needs Node 22.15+/pinned Node 26; archive refusal tests still run")
        with tempfile.TemporaryDirectory(prefix="ocean-runtime-producer-") as directory:
            root = Path(directory); source = root / "runtime"
            for name, body in {
                "runtime/node/bin/node": b"synthetic-native-node",
                "harness/node_modules/@deepseek-ai/dsh/package.json": b'{"type":"module"}',
                "harness/node_modules/@deepseek-ai/dsh/bin/cli.js": b"import {value} from '../relative.js';console.log(JSON.stringify({value,args:process.argv.slice(2),entry:process.argv[1],pid:process.pid}));\n",
                "harness/node_modules/@deepseek-ai/dsh/relative.js": b"export const value='resolved-relative-dependency';",
                "tools/node_modules/pnpm/bin/pnpm.cjs": b"synthetic pnpm entry",
                "harness/node_modules/native/build/addon.node": b"complete native payload",
                f"harness/node_modules/long/{'a'*90}/{'b'*90}/dependency.js": b"bounded long dependency path",
                "NOTICE": b"aggregate runtime license evidence",
                "runtime/node/LICENSE": b"complete node notice",
                "runtime/node/lib/node_modules/npm/bin/npm-cli.js": b"console.log('synthetic npm');",
                ".versions": b"26.12.0\n0.2.1\n10.34.7\n" + b"ab" * 32 + b"\n",
            }.items():
                path = source / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(body)
            for name, target in [("harness/node_modules/.bin/dsh", "../@deepseek-ai/dsh/bin/cli.js"), ("tools/node_modules/.bin/pnpm", "../pnpm/bin/pnpm.cjs"), ("runtime/node/bin/npm", "../lib/node_modules/npm/bin/npm-cli.js")]:
                path = source / name; path.parent.mkdir(parents=True, exist_ok=True); path.symlink_to(target)
            result = subprocess.run([sys.executable, "-B", str(SCRIPT), "--root", str(source), "--output-dir", str(root / "dist"), "--channel", "stable", "--release", "0.2.1", "--sequence", "9", "--target", "linux-x86_64"], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            archive = root / "dist/dsh-ocean-runtime-stable-0.2.1-9-x86_64.tar.gz"
            self.assertTrue(archive.is_file())
            prefix = "dsh-ocean-runtime-stable-0.2.1-9-linux-x86_64/"
            with tarfile.open(archive) as package:
                members = {entry.name: entry for entry in package.getmembers()}
                self.assertTrue(all(entry.isfile() or entry.isdir() for entry in members.values()))
                self.assertEqual(package.extractfile(prefix + "harness/node_modules/native/build/addon.node").read(), b"complete native payload")
                extracted=root / "extracted"
                for relative in ["harness/node_modules/.bin/dsh", "harness/node_modules/@deepseek-ai/dsh/package.json", "harness/node_modules/@deepseek-ai/dsh/bin/cli.js", "harness/node_modules/@deepseek-ai/dsh/relative.js"]:
                    target=extracted/relative;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(package.extractfile(prefix+relative).read())
                node=shutil.which("node");self.assertIsNotNone(node,"publisher fixture requires Node with execve support")
                process=subprocess.Popen([node,str(extracted/"harness/node_modules/.bin/dsh"),"probe"],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,env={"HOME":str(root),"PATH":"/usr/bin:/bin"})
                stdout,stderr=process.communicate(timeout=5);self.assertEqual(process.returncode,0,stderr)
                actual=json.loads(stdout);self.assertEqual(actual["value"],"resolved-relative-dependency");self.assertEqual(actual["args"],["probe"]);self.assertEqual(actual["pid"],process.pid,"bin shim must exec in-place, not orphan a nested server subprocess");self.assertEqual(Path(actual["entry"]),extracted/"harness/node_modules/@deepseek-ai/dsh/bin/cli.js")
                self.assertEqual(package.extractfile(prefix + "runtime/node/LICENSE").read(), b"complete node notice")
                self.assertEqual(package.extractfile(prefix + "NOTICE").read(), b"aggregate runtime license evidence")
            self.assertTrue((source / "harness/node_modules/.bin/dsh").is_symlink())
            metadata = json.loads((root / "dist/artifact.json").read_text())
            self.assertEqual(metadata["sha256"], hashlib.sha256(archive.read_bytes()).hexdigest())
            self.assertEqual(metadata["size"], archive.stat().st_size)
            self.assertEqual(metadata["nodeArchiveSha256"], "ab" * 32)
            alias=root/"alias";alias.symlink_to(root,target_is_directory=True)
            aliased=subprocess.run([sys.executable,"-B",str(SCRIPT),"--root",str(alias/"runtime"),"--output-dir",str(root/"aliased-dist"),"--channel","stable","--release","0.2.1","--sequence","9","--target","linux-x86_64"],capture_output=True,text=True)
            self.assertEqual(aliased.returncode,0,aliased.stderr)
            self.assertEqual(json.loads((root/"aliased-dist/artifact.json").read_text())["sha256"],metadata["sha256"],"symlinked ancestor must not change shim targets or reproducible archive bytes")

    def test_unsafe_or_incomplete_inputs_never_publish_an_archive(self):
        for case in ["escape", "credential", "missing", "versions", "record-newline", "noncanonical-semver"]:
            with self.subTest(case=case), tempfile.TemporaryDirectory(prefix="ocean-runtime-producer-negative-") as directory:
                root=Path(directory);source=root/"runtime"
                for name in ["runtime/node/bin/node", "harness/node_modules/.bin/dsh", "tools/node_modules/.bin/pnpm"]:
                    path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(b"synthetic complete entry")
                (source/".versions").write_text("26.12.0\n0.2.1\n10.34.7\n"+"ab"*32+"\n")
                def run(output):
                    return subprocess.run([sys.executable,"-B",str(SCRIPT),"--root",str(source),"--output-dir",str(output),"--channel","stable","--release","0.2.1","--sequence","9","--target","linux-x86_64"],capture_output=True,text=True)
                control=run(root/"control");self.assertEqual(control.returncode,0,control.stderr)
                if case=="escape":
                    (root/"outside").write_bytes(b"synthetic-outside-data");(source/"harness/escape").symlink_to(root/"outside")
                elif case=="credential":(source/"harness/.npmrc").write_text("synthetic-token-never-publish")
                elif case=="missing":(source/"tools/node_modules/.bin/pnpm").unlink()
                elif case=="versions":(source/".versions").write_text("26.12.0\n9.9.9\n10.34.7\n"+"ab"*32+"\n")
                elif case=="record-newline":(source/".versions").write_text("26.12.0\n0.2.1\n10.34.7\n"+"ab"*32)
                else:(source/".versions").write_text("026.12.0\n0.2.1\n10.34.7\n"+"ab"*32+"\n")
                result=run(root/"dist")
                self.assertNotEqual(result.returncode,0);self.assertFalse(list((root/"dist").glob("*.tar.gz")));self.assertNotIn("synthetic-token-never-publish",result.stdout+result.stderr)

if __name__ == "__main__":
    unittest.main()

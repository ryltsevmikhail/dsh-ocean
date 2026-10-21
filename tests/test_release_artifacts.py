"""Release metadata uses exact built bytes; no network/signing/real keys."""
import hashlib
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/release-artifacts.py"

class ReleaseArtifacts(unittest.TestCase):
    def run_tool(self, *args):
        return subprocess.run([sys.executable,"-B",str(SCRIPT),*map(str,args)],capture_output=True,text=True)

    def test_runtime_manifest_binds_two_actual_archives_and_independent_product(self):
        with tempfile.TemporaryDirectory(prefix="ocean-release-metadata-") as directory:
            root=Path(directory);components=root/"components.json"
            components.write_text(json.dumps({"schema":1,"node":{"version":"26.12.0","archives":{"linux-x64":{"sha256":"ab"*32},"linux-arm64":{"sha256":"cd"*32}}},"dsh":{"package":"@deepseek-ai/dsh","version":"0.2.1"},"pnpm":{"version":"10.34.7"}}))
            artifacts=[]
            for target,arch,source_hash in [("linux-x86_64","x86_64","ab"*32),("linux-aarch64","aarch64","cd"*32)]:
                archive=root/f"dsh-ocean-runtime-stable-0.2.1-9-{arch}.tar.gz";archive.write_bytes(b"synthetic-"+arch.encode())
                result=self.run_tool("artifact","--product","runtime","--channel","stable","--release","0.2.1","--sequence","9","--target",target,"--file",archive,"--components",components)
                self.assertEqual(result.returncode,0,result.stderr);meta=json.loads(result.stdout)
                self.assertEqual(meta["sha256"],hashlib.sha256(archive.read_bytes()).hexdigest());self.assertEqual(meta["size"],archive.stat().st_size);self.assertEqual(meta["nodeArchiveSha256"],source_hash)
                item=root/f"{arch}.json";item.write_text(result.stdout);artifacts.extend(["--artifact",item])
            result=self.run_tool("manifest","--product","runtime","--channel","stable","--release","0.2.1","--sequence","9","--components",components,*artifacts)
            self.assertEqual(result.returncode,0,result.stderr);manifest=json.loads(result.stdout)
            self.assertEqual(manifest["product"],"dsh-ocean-runtime");self.assertEqual(manifest["components"],{"node":"26.12.0","dsh":"0.2.1","pnpm":"10.34.7"});self.assertEqual(manifest["stateScope"],"web-profile-sessions-storages-v1");self.assertEqual(len(manifest["artifacts"]),2)
            if verifier:=os.environ.get("DSH_OCEAN_TEST_VERIFIER"):
                metadata_file=root/"manifest.json";metadata_file.write_text(result.stdout)
                signing=subprocess.run([shutil.which("node"),str(SCRIPT.with_name("sign-publisher-release.mjs")),"--manifest",str(metadata_file),"--product","runtime","--output-dir",str(root/"signature")],capture_output=True,text=True,env={"HOME":str(root),"TMPDIR":str(root),"PATH":"/usr/bin:/bin"})
                self.assertEqual(signing.returncode,0,signing.stderr)
                verified=subprocess.run([verifier,"--verify-runtime-release",str(metadata_file),str(root/"signature/signature.json"),str(root/"signature/public-key.json"),"--channel","stable","--target","linux-x86_64","--min-sequence","1","--current-runtime-version","0.2.0"],capture_output=True,text=True,env={"HOME":str(root),"XDG_STATE_HOME":str(root/"state"),"PATH":"/nonexistent"})
                self.assertEqual(verified.returncode,0,verified.stderr);self.assertFalse((root/"state").exists())
            changed=root/"dsh-ocean-runtime-stable-0.2.1-9-aarch64.tar.gz";original=changed.read_bytes();changed.write_bytes(b"changed-after-description")
            tampered=self.run_tool("manifest","--product","runtime","--channel","stable","--release","0.2.1","--sequence","9","--components",components,*artifacts)
            self.assertNotEqual(tampered.returncode,0,"signing metadata must not reuse stale hashes after actual archive changes");changed.write_bytes(original)
            corrupted=json.loads(Path(artifacts[1]).read_text());corrupted["url"]="https://untrusted.invalid/asset";Path(artifacts[1]).write_text(json.dumps(corrupted))
            refused=self.run_tool("manifest","--product","runtime","--channel","stable","--release","0.2.1","--sequence","9","--components",components,*artifacts)
            self.assertNotEqual(refused.returncode,0);self.assertEqual(refused.stdout,"")

    def test_release_plan_validates_inputs_without_building_signing_or_publishing(self):
        result=self.run_tool("plan","--product","shell","--channel","next","--release","0.3.0","--sequence","10")
        self.assertEqual(result.returncode,0,result.stderr);plan=json.loads(result.stdout);self.assertEqual(plan["tag"],"v0.3.0");self.assertEqual(plan["targets"],["linux-aarch64","linux-x86_64"]);self.assertFalse(plan["published"])
        bad=self.run_tool("plan","--product","shell","--channel","next","--release","0.3.0","--sequence","0");self.assertNotEqual(bad.returncode,0);self.assertEqual(bad.stdout,"")

    def test_publication_source_guard_requires_tag_to_match_exact_built_commit(self):
        args=["plan","--product","shell","--channel","stable","--release","0.3.0","--sequence","10","--source-commit","11"*20]
        matching=self.run_tool(*args,"--tag-commit","11"*20)
        self.assertEqual(matching.returncode,0,matching.stderr);self.assertEqual(json.loads(matching.stdout)["sourceCommit"],"11"*20)
        different=self.run_tool(*args,"--tag-commit","22"*20);self.assertNotEqual(different.returncode,0);self.assertEqual(different.stdout,"")
        absent=self.run_tool(*args);self.assertNotEqual(absent.returncode,0)

    def test_stable_prereleases_fail_before_building_or_signing(self):
        stable=self.run_tool("plan","--product","shell","--channel","stable","--release","0.3.0-rc.1","--sequence","10")
        self.assertNotEqual(stable.returncode,0,"stable channel must refuse prereleases before building")
        next_channel=self.run_tool("plan","--product","shell","--channel","next","--release","0.3.0-rc.1","--sequence","10")
        self.assertEqual(next_channel.returncode,0,next_channel.stderr)

    def test_multiply_linked_archive_is_refused_like_the_independent_client(self):
        with tempfile.TemporaryDirectory(prefix="ocean-release-linked-") as directory:
            root=Path(directory);archive=root/"dsh-ocean-0.3.0-x86_64.tar.gz";archive.write_bytes(b"synthetic-shell")
            os.link(archive,root/"alias.tar.gz")
            linked=self.run_tool("artifact","--product","shell","--channel","stable","--release","0.3.0","--sequence","10","--target","linux-x86_64","--file",archive)
            self.assertNotEqual(linked.returncode,0,"client verification refuses multiple-link artifact inputs")

    def test_single_arch_validation_is_explicit_and_never_the_default_publish_manifest(self):
        with tempfile.TemporaryDirectory(prefix="ocean-release-metadata-") as directory:
            root=Path(directory);archive=root/"dsh-ocean-0.3.0-x86_64.tar.gz";archive.write_bytes(b"synthetic-shell")
            result=self.run_tool("artifact","--product","shell","--channel","next","--release","0.3.0","--sequence","10","--target","linux-x86_64","--file",archive)
            self.assertEqual(result.returncode,0,result.stderr);item=root/"artifact.json";item.write_text(result.stdout)
            args=["manifest","--product","shell","--channel","next","--release","0.3.0","--sequence","10","--artifact",item]
            refused=self.run_tool(*args);self.assertNotEqual(refused.returncode,0)
            validation=self.run_tool(*args,"--single-target-validation");self.assertEqual(validation.returncode,0,validation.stderr)
            self.assertEqual(json.loads(validation.stdout)["product"],"dsh-ocean")

if __name__ == "__main__":
    unittest.main()

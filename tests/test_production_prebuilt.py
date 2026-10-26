import copy
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import json
import os
import sys

spec=importlib.util.spec_from_file_location('prebuilt',Path(__file__).resolve().parents[1]/'scripts/production-prebuilt.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)

class Guards(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=Path(self.tmp.name)
        self.commit='a'*40
        self.lock=dict(product='shell',channel='stable',release='0.1.5',sequence=4,tag='v0.1.5',sourceCommit=self.commit,artifacts=[])
        for arch in ('x86_64','aarch64'):
            name=f'dsh-ocean-0.1.5-{arch}.tar.gz'; data=arch.encode();(self.root/name).write_bytes(data)
            self.lock['artifacts'].append(dict(target='linux-'+arch,filename=name,size=len(data),sha256=hashlib.sha256(data).hexdigest()))
    def validate(self,lock=None,commit=None):m.validate(lock or self.lock,self.root,commit or self.commit)
    def test_accepted(self):self.validate()
    def test_wrong_source(self):
        with self.assertRaises(ValueError):self.validate(commit='b'*40)
    def test_channel_drift(self):
        self.lock['channel']='next'
        with self.assertRaises(ValueError):self.validate()
    def test_duplicate_target(self):
        self.lock['artifacts'][1]['target']='linux-x86_64'
        with self.assertRaises(ValueError):self.validate()
    def test_missing_architecture(self):
        self.lock['artifacts'].pop()
        with self.assertRaises(ValueError):self.validate()
    def test_changed_bytes_same_size(self):
        item=self.lock['artifacts'][0];(self.root/item['filename']).write_bytes(b'X'*item['size'])
        with self.assertRaises(ValueError):self.validate()
    def test_symlink(self):
        item=self.lock['artifacts'][0];path=self.root/item['filename'];path.rename(self.root/'other');path.symlink_to('other')
        with self.assertRaises(ValueError):self.validate()
    def test_hardlink(self):
        path=self.root/self.lock['artifacts'][0]['filename'];(self.root/'other').hardlink_to(path)
        with self.assertRaises(ValueError):self.validate()
    def test_traversal(self):
        self.lock['artifacts'][0]['filename']='../archive.tar.gz'
        with self.assertRaises(ValueError):self.validate()
    def test_release_selection_rejects_published_duplicate_missing(self):
        valid=dict(tag_name='v0.1.5',draft=True,id=1)
        self.assertEqual(m.select_release([[valid]],'v0.1.5')['id'],1)
        for pages in ([],[[dict(valid,draft=False)]],[[valid,valid]]):
            with self.assertRaises(ValueError):m.select_release(pages,'v0.1.5')
    def test_asset_identity_requires_unique(self):
        for assets in ([],[dict(name='a',id=1),dict(name='a',id=2)]):
            with self.assertRaises(ValueError):m.asset(dict(assets=assets),'a')

class Orchestration(unittest.TestCase):
    def test_runtime_verifier_imports_both_archives_into_distinct_private_prefixes(self):
        lock=json.loads((m.ROOT/'docs/releases/production-prebuilt-lock.json').read_text())['runtime']
        with tempfile.TemporaryDirectory() as tmp, patch.dict(os.environ,PRODUCT='runtime',GITHUB_REPOSITORY='ryltsevmikhail/dsh-ocean',RUNNER_TEMP=tmp), patch.object(sys,'argv',['test','verify','--verifier','/native/verifier']), patch.object(m,'api',return_value={'sha':lock['sourceCommit']}), patch.object(m,'validate'), patch.object(m.subprocess,'run') as called:
            m.main()
            calls=[x.args[0] for x in called.call_args_list]
            self.assertEqual(len(calls),2)
            prefixes=[]
            for command in calls:
                self.assertEqual(command[1],'--import-runtime-release')
                self.assertIn('--artifact',command)
                self.assertEqual(command[command.index('--confirm-import')+1],'yes')
                prefixes.append(command[command.index('--prefix')+1])
            self.assertEqual(len(set(prefixes)),2)
    def test_upload_uses_captured_id_and_refuses_newly_published_draft(self):
        lock=json.loads((m.ROOT/'docs/releases/production-prebuilt-lock.json').read_text())['shell']
        draft=dict(id=123,draft=True,tag_name=lock['tag'],assets=[dict(name=x['filename'],id=i) for i,x in enumerate(lock['artifacts'])])
        for publish_before_upload in (False,True):
            with self.subTest(publish_before_upload=publish_before_upload), tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);(root/'draft.json').write_text(json.dumps(draft));(root/'signature').mkdir()
                for path in (root/'manifest.json',root/'signature/signature.json',root/'signature/public-key.json'):path.write_text('{}')
                def api(path):
                    if '/commits/' in path:return {'sha':lock['sourceCommit']}
                    return dict(draft,draft=not publish_before_upload)
                def download(repo,release,name,destination):
                    destination.write_bytes('{}'.encode());return 1
                with patch.dict(os.environ,PRODUCT='shell',GITHUB_REPOSITORY='ryltsevmikhail/dsh-ocean',RUNNER_TEMP=tmp), patch.object(sys,'argv',['test','upload']), patch.object(m,'api',side_effect=api), patch.object(m,'release',return_value=draft), patch.object(m,'validate'), patch.object(m,'download',side_effect=download), patch.object(m.subprocess,'run') as called:
                    if publish_before_upload:
                        with self.assertRaisesRegex(ValueError,'no longer an unpublished draft'):m.main()
                        called.assert_not_called()
                    else:
                        m.main()
                        self.assertEqual(called.call_count,3)
                        for call in called.call_args_list:
                            command=call.args[0]
                            self.assertIn('https://uploads.github.com/repos/ryltsevmikhail/dsh-ocean/releases/123/assets?name=', ' '.join(command))
                            self.assertNotIn('release',command)

if __name__=='__main__':unittest.main()


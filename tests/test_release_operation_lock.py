import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('operation_lock',Path(__file__).resolve().parents[1]/'scripts/release-operation-lock.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class Lock(unittest.TestCase):
 def test_competing_owner_refuses_before_mutation(self):
  with patch.object(m,'api',return_value={'object':{'sha':'other'}}) as api:
   with self.assertRaisesRegex(RuntimeError,'another production operation'):m.acquire('mine')
   self.assertEqual(api.call_count,1)
 def test_atomic_creation_conflict_is_not_force_updated(self):
  def api(path,method='GET',data=None,absent=False):
   if absent:return None
   if path.endswith('/git/ref/heads/main'):return {'object':{'sha':'main'}}
   if path.endswith('/git/commits/main'):return {'tree':{'sha':'tree'}}
   if path.endswith('/git/commits'):return {'sha':'uniqueowner'}
   self.assertEqual(method,'POST');self.assertEqual(data['ref'],'refs/'+m.REF);raise RuntimeError('conflict')
  with patch.object(m,'api',side_effect=api):
   with self.assertRaisesRegex(RuntimeError,'conflict'):m.acquire('mine')
 def test_changed_owner_never_deleted(self):
  with patch.object(m,'api',return_value={'object':{'sha':'other'}}) as api:
   with self.assertRaisesRegex(RuntimeError,'owner changed'):m.release({'commit':'mine'})
   self.assertEqual(api.call_count,1)
 def test_cleanup_on_operation_error(self):
  token={'commit':'mine'}
  with patch.object(m,'acquire',return_value=token),patch.object(m,'release') as release:
   with self.assertRaisesRegex(ValueError,'operation failed'):
    with m.hold('owner'):raise ValueError('operation failed')
   release.assert_called_once_with(token)
if __name__=='__main__':unittest.main()

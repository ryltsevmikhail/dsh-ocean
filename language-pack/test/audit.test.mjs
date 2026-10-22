import {test} from 'node:test';
import assert from 'node:assert/strict';
import {existsSync} from 'node:fs';
const file=new URL('../scripts/audit-catalogs.mjs',import.meta.url);
test('coverage audit enumerates every unchanged entry instead of hiding it behind fallback',async()=>{
 assert.ok(existsSync(file),'catalog audit missing');
 const {auditCatalog}=await import(file);
 assert.deepEqual(auditCatalog({common:{cancel:'Cancel',json:'JSON',empty:''}},{common:{cancel:'Отмена',json:'JSON',empty:''}}),{
  namespaces:{common:3},total:3,changed:1,unchanged:[{namespace:'common',key:'json',value:'JSON'},{namespace:'common',key:'empty',value:''}],
 });
});

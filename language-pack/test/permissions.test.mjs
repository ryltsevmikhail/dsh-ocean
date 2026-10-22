import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {runInNewContext} from 'node:vm';
import {languages} from '../src/register.mjs';
import {renderClient} from '../scripts/build.mjs';
import {validateCatalogs} from '../src/validate.mjs';
import {extractCatalogs} from '../scripts/extract-catalog.mjs';
test('permission settings, current-session access and approval catalogs are complete',()=>{
 const {catalogs}=extractCatalogs(process.env.DSH_INSTALL||'/home/ryltsevmg/.local/opt/dsh');
 assert.equal(Object.keys(catalogs['settings.permission']||{}).length,12,'missing settings.permission namespace');
 assert.equal(Object.keys(catalogs['permission.access']||{}).length,17,'missing permission.access namespace');
 assert.equal(Object.keys(catalogs.approval||{}).length,5,'missing approval namespace');
});

const load=p=>JSON.parse(readFileSync(new URL(p,import.meta.url)));
test('Japanese full-access warnings cover sensitive operations beyond confidentiality',()=>{
 const ja=load('../data/ja.json');
 for(const ns of ['settings.permission','permission.access']){
  assert.ok(ja[ns]['confirm.description'].includes('慎重な扱いを要する操作'),`${ns}: broad sensitive-operation warning`);
  assert.ok(!ja[ns]['confirm.description'].includes('機密性の高い操作'));
 }
});
test('all 8568 archived 0.3.0 translations remain byte-equivalent by namespace',()=>{
 for(const [id,namespaces] of Object.entries(load('./baseline-0.3.0.json'))){
  const current=load(`../data/${id}.json`);
  for(const [ns,hash] of Object.entries(namespaces))assert.equal(createHash('sha256').update(JSON.stringify(current[ns])).digest('hex'),hash,`${id}/${ns}: prior value changed`);
 }
});
test('permission safety copy preserves PTC and rejects changed placeholder multiplicity',()=>{
 const source={'permission.access':{'auto.description':'PTC PTC {name}'}};
 const fresh=()=>Object.fromEntries(languages.map(({id})=>[id,structuredClone(source)]));
 const data=fresh();data.ru['permission.access']['auto.description']='PTC {name}';
 assert.throws(()=>validateCatalogs(data,source),/embedded technical literal/);
 const duplicated=fresh();duplicated.ko['permission.access']['auto.description']+=' {name}';
 assert.throws(()=>validateCatalogs(duplicated,source),/placeholder/);
});

test('every new key resolves through actual installed LocaleRuntime with exact translated values',()=>{
 const install=process.env.DSH_INSTALL||'/home/ryltsevmg/.local/opt/dsh';let official,pack;
 runInNewContext(readFileSync(`${install}/node_modules/@deepseek-ai/dsh-client-locale/lib/client.js`,'utf8'),{navigator:{languages:['en'],language:'en'},window:{__ModuleLoader__:{load(d){official=d;}}}});
 const {LocaleRuntime}=official.factory(()=>({}));const effects=[],ctx={effect(fn){effects.push(fn());},emit(){}};const locale=new LocaleRuntime(ctx);
 const catalogs=Object.fromEntries(languages.map(({id})=>[id,load(`../data/${id}.json`)]));
 runInNewContext(renderClient(catalogs),{window:{__ModuleLoader__:{load(d){pack=d;}}}});pack.factory().apply({...ctx,locale});
 const canonical=extractCatalogs(install).catalogs;
 for(const ns of ['settings.permission','permission.access','approval']){
  locale.register(ns,'en',canonical[ns]);const t=locale.bind(ns);
  for(const {id} of languages){locale.setLocale(id);for(const key of Object.keys(canonical[ns])){
   assert.ok(catalogs[id][ns]?.[key],`${id}/${ns}/${key}: missing`);
   assert.equal(t(key,{name:'/exact/path',toolName:'bash'}),catalogs[id][ns][key].replaceAll('{name}','/exact/path').replaceAll('{toolName}','bash'));
  }}
 }
 for(const dispose of effects.reverse())if(typeof dispose==='function')dispose();
});

test('pinned official labels retain exact verdict and mode bindings; pack never supplies policy',()=>{
 const install=process.env.DSH_INSTALL||'/home/ryltsevmg/.local/opt/dsh';
 const approval=readFileSync(`${install}/node_modules/@deepseek-ai/dsh-client-ui-approval/lib/client.js`,'utf8');
 assert.match(approval,/answer\("rejected"\);\s*\},\s*children: t\("reject"\)/);
 assert.match(approval,/answer\("allowed-once"\);\s*\},\s*children: t\("allowOnce"\)/);
 assert.match(approval,/answer\(event.key === "Enter" \? "allowed-once" : "rejected"\)/);
 const permission=readFileSync(`${install}/node_modules/@deepseek-ai/dsh-client-ui-permission-presets/lib/client.js`,'utf8');
 for(const pair of ['["read-only", "preset.readOnly"]','["workspace-write", "preset.workspaceWrite"]','[FULL_ACCESS_PRESET, "preset.fullAccess"]'])assert.ok(permission.includes(pair));
 assert.match(permission,/const FULL_ACCESS_PRESET = "danger-full-access"/);
 assert.match(permission,/if \(id === "danger-full-access"\) \{\s*setAcknowledged\(false\);\s*setConfirmingFullAccess\(true\);\s*return;/);
 assert.match(permission,/onConfirm: \(\) => \{\s*setAcknowledged\(false\);\s*setConfirmingFullAccess\(false\);\s*select\(FULL_ACCESS_PRESET\)/);
 assert.match(readFileSync(new URL('../src/register.mjs',import.meta.url),'utf8'),/ctx\.locale\.register\(namespace, language.id, dictionary\)/);
});

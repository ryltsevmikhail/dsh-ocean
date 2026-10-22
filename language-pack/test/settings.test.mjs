import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {runInNewContext} from 'node:vm';
import {extractCatalogs} from '../scripts/extract-catalog.mjs';
import {renderClient} from '../scripts/build.mjs';
import {languages} from '../src/register.mjs';
const load=name=>JSON.parse(readFileSync(new URL(name,import.meta.url)));
test('complete theme and shortcuts owned namespaces are extracted from pinned artifacts',()=>{
 const {catalogs,inventory}=extractCatalogs(process.env.DSH_INSTALL||'/home/ryltsevmg/.local/opt/dsh');
 assert.equal(Object.keys(catalogs['settings.theme']||{}).length,9,'missing settings.theme namespace');
 assert.equal(Object.keys(catalogs.shortcuts||{}).length,56,'missing shortcuts namespace');
 assert.equal(inventory.totalKeys,1258);assert.equal(inventory.sources.length,18);
});
test('all 0.2.0 translations remain byte-for-byte equivalent by namespace',()=>{
 for(const [id,baseline] of Object.entries(load('./baseline-0.2.0.json'))){
  const current=load(`../data/${id}.json`);
  for(const [ns,hash] of Object.entries(baseline))assert.equal(createHash('sha256').update(JSON.stringify(current[ns])).digest('hex'),hash,`${id}/${ns}: old translation changed`);
 }
});
test('actual locale callers do not fall back to English for settings feature copy',()=>{
 let official,pack;const runtime=process.env.DSH_INSTALL||'/home/ryltsevmg/.local/opt/dsh';
 runInNewContext(readFileSync(`${runtime}/node_modules/@deepseek-ai/dsh-client-locale/lib/client.js`,'utf8'),{navigator:{languages:['en'],language:'en'},window:{__ModuleLoader__:{load(d){official=d;}}}});
 const {LocaleRuntime}=official.factory(()=>({}));const effects=[],ctx={effect(fn){effects.push(fn());},emit(){}};const locale=new LocaleRuntime(ctx);
 const canonical=extractCatalogs(runtime).catalogs;
 const catalogs=Object.fromEntries(languages.map(({id})=>[id,load(`../data/${id}.json`)]));
 runInNewContext(renderClient(catalogs),{window:{__ModuleLoader__:{load(d){pack=d;}}}});pack.factory().apply({...ctx,locale});
 for(const ns of ['settings.theme','shortcuts']){
  assert.ok(canonical[ns],`${ns}: namespace missing`);locale.register(ns,'en',canonical[ns]);const t=locale.bind(ns);
  for(const {id} of languages){locale.setLocale(id);for(const [key,en] of Object.entries(canonical[ns])){
   const text=t(key,{command:'Example',commands:'Example',count:2,location:'Example',reload:'Example'});
   assert.ok(catalogs[id][ns]?.[key],`${id}/${ns}/${key}: missing translation`);
   if(!['fontSize.unit','desktop-document'].includes(key) && !(id==='de' && key==='appearance.system') && !(id==='fr' && key==='application'))assert.notEqual(text,en,`${id}/${ns}/${key}: English fallback`);
   assert.notEqual(text,key);
   const intended=catalogs[id][ns][key].replaceAll('{command}','Example').replaceAll('{commands}','Example').replaceAll('{count}','2').replaceAll('{location}','Example').replaceAll('{reload}','Example');
   assert.equal(text,intended,`${id}/${ns}/${key}: actual interpolated locale value`);
  }}
 }
 for(const dispose of effects.reverse())if(typeof dispose==='function')dispose();
});

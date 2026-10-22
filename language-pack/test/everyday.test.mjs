import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { createHash } from 'node:crypto';

test('all original 0.1.0 dictionary values remain exactly unchanged',()=>{
 const baseline=JSON.parse(readFileSync(new URL('./baseline-0.1.0.json',import.meta.url)));
 for(const [id,namespaces] of Object.entries(baseline)){
  const current=JSON.parse(readFileSync(new URL(`../data/${id}.json`,import.meta.url)));
  for(const [ns,hash] of Object.entries(namespaces)) assert.equal(createHash('sha256').update(JSON.stringify(current[ns])).digest('hex'),hash,`${id}/${ns}: original translations changed`);
 }
});
import { renderClient } from '../scripts/build.mjs';
import { languages } from '../src/register.mjs';

test('everyday sidebar and preset callers resolve translated owned namespaces in every language', () => {
 const runtime=process.env.DSH_INSTALL || '/home/ryltsevmg/.local/opt/dsh';
 let official,pack;
 runInNewContext(readFileSync(`${runtime}/node_modules/@deepseek-ai/dsh-client-locale/lib/client.js`,'utf8'),{navigator:{languages:['en'],language:'en'},window:{__ModuleLoader__:{load(d){official=d;}}}});
 const {LocaleRuntime}=official.factory(()=>({}));
 const effects=[],ctx={effect(fn){effects.push(fn());},emit(){}};
 const locale=new LocaleRuntime(ctx);
 const canonical=JSON.parse(readFileSync(new URL('../data/en.json',import.meta.url)));
 for(const ns of ['sidebar','settings.agentPreset'])locale.register(ns,'en',canonical[ns]);
 const catalogs=Object.fromEntries(languages.map(({id})=>[id,JSON.parse(readFileSync(new URL(`../data/${id}.json`,import.meta.url)))]));
 runInNewContext(renderClient(catalogs),{window:{__ModuleLoader__:{load(d){pack=d;}}}});
 pack.factory().apply({...ctx,locale});
 const bound={sidebar:locale.bind('sidebar'),'settings.agentPreset':locale.bind('settings.agentPreset')};
 for(const {id} of languages){
  locale.setLocale(id);
  for(const [ns,key,english] of [['sidebar','session.new','New Session'],['sidebar','session.new.label','New session'],['sidebar','toggle.open','Open sidebar'],['sidebar','toggle.collapse','Collapse sidebar'],['sidebar','panels.label','Global panels'],['settings.agentPreset','nav','Agent presets'],['settings.agentPreset','presetStandardName','Standard mode']]){
   const text=bound[ns](key);
   assert.notEqual(text,english,`${id}/${ns}/${key}: visible English fallback`);
   assert.notEqual(text,key,`${id}/${ns}/${key}: missing namespace`);
  }
  for(const ns of ['sidebar','settings.agentPreset'])for(const key of Object.keys(canonical[ns])){
   assert.notEqual(catalogs[id][ns][key],canonical[ns][key],`${id}/${ns}/${key}: untranslated everyday copy`);
   const expected=catalogs[id][ns][key].replaceAll('{name}','Example preset').replaceAll('{reason}','Example reason');
   assert.equal(bound[ns](key,{name:'Example preset',reason:'Example reason'}),expected,`${id}/${ns}/${key}: actual caller resolution`);
  }
 }
 for(const dispose of effects.splice(0).reverse())if(typeof dispose==='function')dispose();
 assert.deepEqual(Array.from(locale.getLocale().locales,l=>l.id),['zh','en']);
 pack.factory().apply({...ctx,locale});
 assert.deepEqual(Array.from(locale.getLocale().locales,l=>l.id),['zh','en',...languages.map(l=>l.id)]);
 locale.setLocale('ru');
 assert.equal(bound.sidebar('session.new'),catalogs.ru.sidebar['session.new']);
 for(const dispose of effects.splice(0).reverse())if(typeof dispose==='function')dispose();
 assert.deepEqual(Array.from(locale.getLocale().locales,l=>l.id),['zh','en']);
});

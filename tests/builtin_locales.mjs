import test from 'node:test';
import assert from 'node:assert/strict';
import {existsSync,readFileSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import vm from 'node:vm';
const root=new URL('../',import.meta.url);
test('built-in client resolves with pinned public LocaleRuntime and unloads its effects', {skip:!process.env.DSH_LOCALE_CLIENT_SOURCE},()=>{
 let official;vm.runInNewContext(readFileSync(process.env.DSH_LOCALE_CLIENT_SOURCE,'utf8'),{navigator:{languages:['en'],language:'en'},window:{__ModuleLoader__:{load(value){official=value;}}}});
 const {LocaleRuntime}=official.factory(()=>({}));const disposers=[];
 const ctx={effect(fn){disposers.push(fn());},emit(){}};const locale=new LocaleRuntime(ctx);
 locale.register('common','en',{builtinFallback:'Upstream English fallback'});
 let builtin;vm.runInNewContext(readFileSync(new URL('locales/dsh/lib/client.js',root),'utf8'),{window:{__ModuleLoader__:{load(value){builtin=value;}}}});
 builtin.factory().apply({...ctx,locale});
 for(const id of ['ru','es','pt-BR','de','fr','ja','ko']) {locale.setLocale(id);assert.equal(locale.bind('missing')('builtinFallback'),'Upstream English fallback');}
 locale.setLocale('ru');assert.equal(locale.bind('common')('cancel'),'Отмена');
 for(const dispose of disposers.reverse())if(typeof dispose==='function')dispose();
 assert.deepEqual(Array.from(locale.getLocale().locales,x=>x.id),['zh','en']);
});

test('module identity is a closed package token, not executable source',async()=>{
 const {renderClient}=await import('../language-pack/scripts/build.mjs');
 assert.throws(()=>renderClient({},"@ocean/pkg';alert(1)//"),/invalid built-in module id/);
});

test('main native payload retains the built-in dictionary copyright and licence',()=>{
 const paths=readFileSync(new URL('packaging/license-inputs.txt',root),'utf8').trim().split('\n');
 assert(paths.includes('locales/dsh/LICENSE'),'native package omits built-in dictionary licence');
 assert(readFileSync(new URL('packaging/install-from-package.py',root),'utf8').includes('"locales/dsh/LICENSE"'));
 assert(readFileSync(new URL('NOTICE',root),'utf8').includes('Built-in DSH dictionaries'));
});

test('main-package DSH locale resource is generated deterministically without an optional package',()=>{
 const generator=new URL('scripts/build-builtin-locales.mjs',root);
 assert(existsSync(generator),'no main-package DSH locale resource builder');
 const run=spawnSync(process.execPath,[generator.pathname,'--check'],{cwd:root,encoding:'utf8'});
 assert.equal(run.status,0,run.stdout+run.stderr);
 const manifest=JSON.parse(readFileSync(new URL('locales/dsh/package.json',root)));
 assert.equal(manifest.name,'@dsh-ocean/builtin-locales');
 assert.equal(manifest.dsh.client.platform,'web');assert.equal(manifest.exports['./client'],'./lib/client.js');
 assert.equal(manifest.scripts,undefined);assert.equal(manifest.dependencies,undefined);
 assert.match(readFileSync(new URL('locales/dsh/overlay.yml',root),'utf8'),/name: \.\/lib\/index\.js/);
 let definition;vm.runInNewContext(readFileSync(new URL('locales/dsh/lib/client.js',root),'utf8'),{window:{__ModuleLoader__:{load(value){definition=value;}}}});
 assert.equal(definition.id,manifest.name);const plugin=definition.factory();
 const ids=[],registered=[],disposers=[];
 const ctx={effect(fn){disposers.push(fn());},locale:{addLanguage(value){ids.push(value.id);return()=>{};},register(namespace,language,dictionary){registered.push({namespace,language,dictionary});return()=>{};}}};
 plugin.apply(ctx);assert.deepEqual(ids,['ru','es','pt-BR','de','fr','ja','ko']);
 assert(registered.some(x=>x.language==='ru'&&x.namespace==='chat'));
 assert.equal(disposers.length,7+registered.length);
});

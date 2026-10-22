import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
const build = new URL('../scripts/build.mjs', import.meta.url);
test('browser artifact follows official lazy module loader contract without side effects', async () => {
 assert.ok(existsSync(build), 'browser bundle generator missing');
 const { renderClient } = await import(build);
 const { languages } = await import('../src/register.mjs');
 const dictionaries = Object.fromEntries(languages.map(({id}) => [id,{common:{cancel:`${id}:cancel`}}]));
 const text = renderClient(dictionaries);
 let definition;
 runInNewContext(text, { window: { __ModuleLoader__: { load(d) { definition = d; } } } });
 assert.equal(definition.id, '@dsh-ocean/desktop-language-pack');
 const module = definition.factory(() => assert.fail('language pack must have no runtime imports'));
 assert.deepEqual(Array.from(module.inject), ['locale']);
 let effects = 0;
 module.apply({ effect(fn) { effects++; return fn(); }, locale: { addLanguage(){ return ()=>{}; },register(){return ()=>{};} } });
 assert.equal(effects, 14);
});

test('real installed LocaleRuntime resolves packed dictionaries, placeholders, fallback and unload', async () => {
 assert.ok(existsSync(build), 'browser bundle generator missing');
 const { renderClient } = await import(build);
 const { languages } = await import('../src/register.mjs');
 const runtime = process.env.DSH_INSTALL || '/home/ryltsevmg/.local/opt/dsh';
 let official;
 runInNewContext(readFileSync(`${runtime}/node_modules/@deepseek-ai/dsh-client-locale/lib/client.js`, 'utf8'), { navigator: { languages: ['en'], language: 'en' }, window: {__ModuleLoader__: {load(d){official=d;}}} });
 // Imported UI components are not rendered by LocaleRuntime. No service behavior is stubbed.
 const { LocaleRuntime } = official.factory(() => ({}));
 const effects = [], ctx = { effect(fn){ effects.push(fn()); }, emit(){} };
 const locale = new LocaleRuntime(ctx);
 const disposeEn = locale.register('common','en',{fallback:'English fallback'});
 const data = Object.fromEntries(languages.map(({id})=>[id,{common:{cancel:`${id}:cancel`},chat:{greeting:`${id}: {name}`}}]));
 let definition;
 runInNewContext(renderClient(data),{window:{__ModuleLoader__:{load(d){definition=d;}}}});
 definition.factory().apply({...ctx,locale});
 assert.deepEqual(Array.from(locale.getLocale().locales,l=>l.id), ['zh','en',...languages.map(l=>l.id)]);
 for (const {id} of languages) {
  locale.setLocale(id);
  assert.equal(locale.bind('chat')('greeting',{name:'User content'}),`${id}: User content`);
  assert.equal(locale.bind('chat')('cancel'),`${id}:cancel`);
  assert.equal(locale.bind('missing')('fallback'),'English fallback');
 }
 for (const dispose of effects.splice(0).reverse()) if (typeof dispose === 'function') dispose();
 assert.deepEqual(Array.from(locale.getLocale().locales,l=>l.id), ['zh','en']);
 definition.factory().apply({...ctx,locale});
 locale.setLocale('ru');
 assert.equal(locale.bind('chat')('greeting',{name:'Re-enabled'}),'ru: Re-enabled');
 for (const dispose of effects.splice(0).reverse()) if (typeof dispose === 'function') dispose();
 assert.deepEqual(Array.from(locale.getLocale().locales,l=>l.id), ['zh','en']);
 disposeEn();
});

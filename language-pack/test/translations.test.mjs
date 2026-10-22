import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
const data = new URL('../data/', import.meta.url);
const en = JSON.parse(readFileSync(new URL('en.json', data)));
const languages = (process.env.LANGUAGES || 'ru,es,pt-BR,de,fr,ja,ko').split(',');
const placeholders = s => [...s.matchAll(/\{([^{}]+)\}/g)].map(x => x[1]).sort();
for (const language of languages) test(`${language}: complete canonical keys, placeholders and genuine translated copy`, () => {
 const url = new URL(`${language}.json`, data);
 assert.ok(existsSync(url), `${language} translation is missing`);
 const dict = JSON.parse(readFileSync(url));
 assert.deepEqual(Object.keys(dict).sort(), Object.keys(en).sort());
 let translated = 0, count = 0;
 for (const [ns, source] of Object.entries(en)) {
  assert.deepEqual(Object.keys(dict[ns]).sort(), Object.keys(source).sort(), ns);
  for (const [key, original] of Object.entries(source)) {
   const value = dict[ns][key];
   assert.equal(typeof value, 'string', `${ns}.${key}`);
   assert.ok(value.length || !original.length, `${ns}.${key}: empty translation`);
   assert.deepEqual(placeholders(value), placeholders(original), `${ns}.${key}: placeholders`);
   if (value !== original) translated++;
   count++;
  }
 }
 assert.ok(translated / count > 0.94, `${language}: ${translated}/${count} changed; English-copy packs are prohibited`);
});

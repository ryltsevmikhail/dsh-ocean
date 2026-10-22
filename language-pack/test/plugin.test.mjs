import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
const source = new URL('../src/register.mjs', import.meta.url);
test('registration is owned, ordered, and entirely reversible', async () => {
 assert.ok(existsSync(source), 'locale plugin registration implementation missing');
 const { registerLanguagePack, languages } = await import(source);
 const calls = [], disposers = [], active = new Set();
 const ctx = {
  effect(fn, label) { assert.ok(label); disposers.push(fn()); },
  locale: {
   addLanguage(def) { calls.push(['language',def]); active.add(def.id); return () => active.delete(def.id); },
   register(ns, id, dict) { assert.ok(active.has(id)); calls.push(['dict',ns,id,dict]); active.add(`${id}:${ns}`); return () => active.delete(`${id}:${ns}`); },
  },
 };
 const dictionaries = Object.fromEntries(languages.map(({id}) => [id, {common:{cancel:'test'}, chat:{send:'test'}}]));
 registerLanguagePack(ctx, dictionaries);
 assert.deepEqual(languages.map(l => l.id), ['ru','es','pt-BR','de','fr','ja','ko']);
 assert.equal(calls.length, 21);
 assert.ok(languages.every(l => l.fallback === 'en'));
 for (const dispose of disposers.reverse()) { assert.equal(typeof dispose, 'function'); dispose(); }
 assert.equal(active.size, 0);
});
test('package declares a real bundle and only pinned, compatible host peers', () => {
 const path = new URL('../package.json',import.meta.url);
 assert.ok(existsSync(path), 'bundle manifest missing');
 const pkg = JSON.parse(readFileSync(path));
 assert.equal(pkg.dsh.bundle.patch, './cordis.patch.yml');
 assert.equal(pkg.dsh.client.platform, 'web');
 assert.deepEqual(pkg.dsh.client.inject, ['@deepseek-ai/dsh-client-locale']);
 assert.equal(pkg.peerDependencies['@deepseek-ai/dsh-client-locale'], '0.2.0-rc.2');
 assert.equal(pkg.dependencies, undefined);
 for (const hook of ['preinstall','install','postinstall','prepare','prepack']) assert.equal(pkg.scripts?.[hook], undefined);
});

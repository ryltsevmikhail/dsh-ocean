import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
test('extractor reads complete pinned preset guide spread and joined Markdown without evaluating plugin',async()=>{
 const {extractCatalogs}=await import('../scripts/extract-catalog.mjs');
 assert.equal(typeof extractCatalogs,'function','read-only extractor missing');
 const result=extractCatalogs(process.env.DSH_INSTALL || '/home/ryltsevmg/.local/opt/dsh');
 const en=result.catalogs;
 assert.deepEqual(en,JSON.parse(readFileSync(root+'data/en.json')));
 assert.equal(Object.keys(en.sidebar).length,5);
 assert.equal(Object.keys(en['settings.agentPreset']).length,41);
 assert.equal(Object.keys(en['settings.agentPreset']).filter(k=>k.startsWith('guide')||['modeExplanation','howToUse'].includes(k)).length,19);
 assert.ok(en['settings.agentPreset'].guidePtcExplanation.includes('### What reaches the model\n\nTool results first reach the program'));
 assert.equal(result.inventory.totalKeys,1258);
 assert.equal(result.inventory.sources.length,18);
});

test('canonical inventory records all eighteen scoped namespaces and provenance', () => {
  assert.ok(existsSync(root + 'data/en.json'), 'canonical English inventory is missing');
  const en = JSON.parse(readFileSync(root + 'data/en.json'));
  assert.deepEqual(Object.keys(en).sort(), ['common','settings.locale','settings','chat','conversation','workspace','model','settings.models','settings.plugins','settings.pluginInventory','pluginManager','sidebar','settings.agentPreset','settings.theme','shortcuts','settings.permission','permission.access','approval'].sort());
  assert.ok(Object.values(en).every(d => Object.values(d).every(v => typeof v === 'string')));
  const inventory = JSON.parse(readFileSync(root + 'data/inventory.json'));
  assert.equal(inventory.runtime.version, '0.2.0-rc.2');
  assert.match(inventory.runtime.sha256, /^[a-f0-9]{64}$/);
});

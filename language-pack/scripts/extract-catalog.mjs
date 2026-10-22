// Reads pinned, installed official artifacts, never evaluates a full plugin.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { runInNewContext } from 'node:vm';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = new URL('../', import.meta.url);
const hash = text => createHash('sha256').update(text).digest('hex');
const spec = [
 ['locale','en$1','common'], ['locale','en','settings.locale'],
 ['ui-settings-general','en','settings'], ['ui-chat','en','chat'],
 ['ui-conversation','en','conversation'], ['ui-workspace','en','workspace'],
 ['ui-model-selection','en','model'], ['ui-settings-models','en','settings.models'],
 ['ui-settings-plugins','en','settings.plugins'], ['ui-settings-plugin-inventory','en','settings.pluginInventory'],
 ['ui-plugin-manager','en','pluginManager'],
 ['ui-sidebar','en','sidebar'], ['ui-agent-preset','en','settings.agentPreset'],
 ['ui-theme','en','settings.theme'], ['ui-shortcuts','en','shortcuts'],
 ['ui-permission-presets','en','settings.permission'],
 ['ui-permission-presets','accessEn','permission.access'], ['ui-approval','en','approval'],
];
// Balanced object scan: braces and semicolons inside quoted copy are not syntax.
function literalAt(source, start) {
 let depth = 0, quote, escaped = false;
 for (let i = start; i < source.length; i++) {
  const c = source[i];
  if (quote) { if (escaped) escaped = false; else if (c === '\\') escaped = true; else if (c === quote) quote = undefined; continue; }
  if (c === '"' || c === "'") { quote = c; continue; }
  if (c === '{') depth++;
  if (c === '}' && --depth === 0) return source.slice(start, i + 1);
 }
 throw Error('Unterminated catalog');
}
export function extractCatalogs(install) {
const catalogs = {}, sources = [];
for (const [suffix, variable, namespace] of spec) {
 const pkg = '@deepseek-ai/dsh-client-' + suffix;
 const artifact = `${install}/node_modules/${pkg}/lib/client.js`;
 const source = readFileSync(artifact, 'utf8');
 const marker = `const ${variable} = {`;
 const start = source.indexOf(marker);
 if (start < 0 || source.indexOf(marker, start + 1) >= 0) throw Error(`Ambiguous catalog ${pkg}:${variable}`);
 const literal = literalAt(source, start + marker.length - 1);
 // Only a catalog expression runs in an empty VM with a known upstream constant.
 const constants = { PLAN_NEXT_ACTION_EN: 'describe your task to generate plan' };
 // The preset catalog composes its owned curated guide literal. Evaluate only
 // these two known literals, not the plugin, imports, or any runtime code.
 if (namespace === 'settings.agentPreset') {
  const guideMarker = 'const guideEn = {';
  const guideStart = source.indexOf(guideMarker);
  if (guideStart < 0 || source.indexOf(guideMarker, guideStart + 1) >= 0) throw Error('Ambiguous preset guide');
  constants.guideEn = runInNewContext('(' + literalAt(source, guideStart + guideMarker.length - 1) + ')', {}, { timeout: 1000 });
 }
 const dictionary = runInNewContext('(' + literal + ')', constants, { timeout: 1000 });
 if (!Object.values(dictionary).every(value => typeof value === 'string')) throw Error(`Non-string catalog ${pkg}`);
 catalogs[namespace] = dictionary;
 const manifest = JSON.parse(readFileSync(`${install}/node_modules/${pkg}/package.json`));
 sources.push({ namespace, package: pkg, version: manifest.version, artifact: 'lib/client.js', variable, sha256: hash(source), keys: Object.keys(dictionary).length });
}
const runtimeFile = readFileSync(`${install}/node_modules/@deepseek-ai/dsh/package.json`);
const runtime = { package: '@deepseek-ai/dsh', version: JSON.parse(runtimeFile).version, sha256: hash(runtimeFile) };
// Normalize VM-owned objects before returning a plain serializable snapshot.
return JSON.parse(JSON.stringify({catalogs,inventory:{runtime,sources,totalKeys:Object.values(catalogs).reduce((n,d)=>n+Object.keys(d).length,0)}}));
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
 const {catalogs,inventory}=extractCatalogs(resolve(process.argv[2] || '/home/ryltsevmg/.local/opt/dsh'));
 mkdirSync(new URL('data/',root), { recursive: true });
 writeFileSync(new URL('data/en.json',root), JSON.stringify(catalogs,null,2)+'\n');
 writeFileSync(new URL('data/inventory.json',root), JSON.stringify(inventory,null,2)+'\n');
 console.log(inventory.sources.map(x=>`${x.namespace}: ${x.keys}`).join('\n'));
}

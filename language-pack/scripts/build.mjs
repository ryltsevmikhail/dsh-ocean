import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { languages, registerLanguagePack } from '../src/register.mjs';
import { validateCatalogs } from '../src/validate.mjs';
import { auditCatalog } from './audit-catalogs.mjs';
const root = new URL('../', import.meta.url);

/** Emit the official no-import, lazy browser factory (see installed UI template). */
export function renderClient(catalogs) {
  return `// Generated from src/register.mjs and data/*.json; do not edit.\nwindow.__ModuleLoader__.load({\n  id: '@dsh-ocean/desktop-language-pack',\n  factory() {\n    const languages = ${JSON.stringify(languages)};\n    const catalogs = ${JSON.stringify(catalogs)};\n    ${registerLanguagePack.toString()}\n    return { inject: ['locale'], apply(ctx) { registerLanguagePack(ctx, catalogs); } };\n  },\n});\n`;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const catalogs = Object.fromEntries(languages.map(({id}) => [id, JSON.parse(readFileSync(new URL(`data/${id}.json`,root)))]));
  const reference=JSON.parse(readFileSync(new URL('data/en.json',root)));
  validateCatalogs(catalogs,reference);
  const audits=Object.fromEntries(languages.map(({id})=>[id,auditCatalog(reference,catalogs[id])]));
  writeFileSync(new URL('data/unchanged.json',root),JSON.stringify(audits,null,2)+'\n');
  mkdirSync(new URL('lib/', root), {recursive:true});
  writeFileSync(new URL('lib/client.js',root), renderClient(catalogs));
  writeFileSync(new URL('lib/index.js',root), '/** Host half: all behavior belongs to the owned Client locale effects. */\nexport function apply() {}\n');
  console.log(`Built ${languages.length} languages, ${Object.keys(catalogs.ru).length} namespaces each`);
}

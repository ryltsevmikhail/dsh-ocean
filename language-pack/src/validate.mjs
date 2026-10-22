import assert from 'node:assert/strict';
import { languages } from './register.mjs';

/** These are wire identifiers/examples, not English prose to be translated. */
export const technicalKeys = {
 'settings.models': ['deepSeekBaseUrl','customBaseUrlPlaceholder','customAnthropicBaseUrlPlaceholder','protocolOpenAiCompletions','protocolOpenAiResponses','protocolAnthropicMessages'],
 'chat': ['message.compaction.commandTitle'],
 'conversation': ['tool.title.bash','tool.title.pwsh','tool.title.grep','tool.title.glob','web.http'],
 'pluginManager': ['installGuideIdExample','registryCustomPlaceholder'],
};
export const shortcutTechnicalLiterals = ['Mod','Shift','Command','Option','Ctrl','Alt','Control','Esc','Tab','Backslash','Backquote','localStorage','dsh.keybindings.v1','userData/keybindings.json'];
export const presetTechnicalLiterals = ['Programmatic Tool Calling','run_code','TypeScript','SDK','workflow','PTC','DSH','Cordis','Skills','Markdown','JSON','configs/','schema.json','CSV','logs/'];
// ASCII identifier boundaries allow natural Japanese/Korean suffixes while not
// confusing prose "workflows" with the actual tool identifier "workflow".
const occurrences = (value, literal) => [...value.matchAll(new RegExp('(?<![A-Za-z0-9_])'+literal.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')+'(?![A-Za-z0-9_])','g'))].length;
export const placeholders = value => [...value.matchAll(/\{([^{}]+)\}/g)].map(match=>match[1]).sort();

/** Refuse incomplete or incompatible catalogs before generating a distributable. */
export function validateCatalogs(catalogs, reference) {
 assert.deepEqual(Object.keys(catalogs).sort(),languages.map(({id})=>id).sort(),'catalog languages');
 for(const {id} of languages) {
  const translated=catalogs[id];
  assert.deepEqual(Object.keys(translated).sort(),Object.keys(reference).sort(),`${id}: namespaces`);
  for(const [namespace, source] of Object.entries(reference)) {
   const dictionary=translated[namespace];
   assert.deepEqual(Object.keys(dictionary).sort(),Object.keys(source).sort(),`${id}/${namespace}: keys`);
   for(const [key, original] of Object.entries(source)) {
    const value=dictionary[key], location=`${id}/${namespace}/${key}`;
    assert.equal(typeof value,'string',`${location}: string required`);
    assert.ok(value.length || !original.length,`${location}: empty translation`);
    assert.deepEqual(placeholders(value),placeholders(original),`${location}: placeholder mismatch`);
    if(technicalKeys[namespace]?.includes(key)) assert.equal(value,original,`${location}: technical literal changed`);
    if(namespace === 'shortcuts') {
      const combinations=original.match(/(?:Mod|Command|Ctrl|Control)(?:\+(?:Shift|Option|Alt))*\+(?:Backslash|Backquote|key|[/,.])/g)||[];
      for(const combination of new Set(combinations)) {
       const pattern=combination.replace(/[.*+?^${}()|[\]\\]/g,'\\$&').replace(/key$/, '[\\p{L}]+');
       const matches=[...value.matchAll(new RegExp(pattern,'gu'))].length;
       assert.equal(matches,combinations.filter(c=>c===combination).length,`${location}: shortcut combination ${combination} changed`);
      }
     }
     if(namespace === 'shortcuts') for(const literal of shortcutTechnicalLiterals) {
      assert.equal(occurrences(value,literal),occurrences(original,literal),`${location}: embedded technical literal ${literal} changed`);
     }
     if(namespace === 'permission.access') for(const literal of ['PTC']) {
      assert.equal(occurrences(value,literal),occurrences(original,literal),`${location}: embedded technical literal ${literal} changed`);
     }
     if(namespace === 'settings.agentPreset') for(const literal of presetTechnicalLiterals) {
     assert.equal(occurrences(value,literal),occurrences(original,literal),`${location}: embedded technical literal ${literal} changed`);
    }
   }
  }
 }
}

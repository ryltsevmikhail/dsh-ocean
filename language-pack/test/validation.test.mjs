import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { languages } from '../src/register.mjs';
const file=new URL('../src/validate.mjs',import.meta.url);
test('build validation rejects missing keys, placeholders and altered protocol/URL literals',async()=>{
 assert.ok(existsSync(file),'strict catalog validator missing');
 const { validateCatalogs }=await import(file);
 const source={common:{cancel:'Cancel'},chat:{message:'Hello {name}'},'settings.models':{deepSeekBaseUrl:'https://api.deepseek.com/anthropic',protocolAnthropicMessages:'Anthropic Messages'}};
 const fresh=()=>Object.fromEntries(languages.map(({id})=>[id,{common:{cancel:'translated'},chat:{message:'translated {name}'},'settings.models':{...source['settings.models']}}]));
 assert.doesNotThrow(()=>validateCatalogs(fresh(),source));
 let data=fresh();delete data.ru.common.cancel;assert.throws(()=>validateCatalogs(data,source),/keys/);
 data=fresh();data.ru.chat.message='translated {wrong}';assert.throws(()=>validateCatalogs(data,source),/placeholder/);
 data=fresh();data.ru['settings.models'].deepSeekBaseUrl='https://other.example';assert.throws(()=>validateCatalogs(data,source),/technical literal/);
 data=fresh();data.ru['settings.models'].protocolAnthropicMessages='translated protocol';assert.throws(()=>validateCatalogs(data,source),/technical literal/);
 data=fresh();delete data.ko;assert.throws(()=>validateCatalogs(data,source),/languages/);
 data=fresh();data.ru.extra={};assert.throws(()=>validateCatalogs(data,source),/namespaces/);
 data=fresh();data.ru.common.cancel='';assert.throws(()=>validateCatalogs(data,source),/empty/);
 data=fresh();data.ru.chat.message='translated {name} {name}';assert.throws(()=>validateCatalogs(data,source),/placeholder/);
});

test('preset guides protect embedded tool, format and path literals including multiplicity',async()=>{
 const {validateCatalogs}=await import(file);
 const source={'settings.agentPreset':{guidePtcExplanation:'PTC Programmatic Tool Calling run_code TypeScript SDK workflow PTC',guidePtcUsage:'JSON configs/ schema.json CSV logs/ CSV',guideStandardExplanation:'Skills'}};
 const fresh=()=>Object.fromEntries(languages.map(({id})=>[id,structuredClone(source)]));
 assert.doesNotThrow(()=>validateCatalogs(fresh(),source));
 for(const literal of ['run_code','TypeScript','SDK','workflow','PTC','Programmatic Tool Calling','JSON','configs/','schema.json','CSV','logs/','Skills']){
  const data=fresh();const key=Object.keys(source['settings.agentPreset']).find(k=>source['settings.agentPreset'][k].includes(literal));
  data.ru['settings.agentPreset'][key]=source['settings.agentPreset'][key].replace(literal,'translated');
  assert.throws(()=>validateCatalogs(data,source),/embedded technical literal/,literal);
 }
 const data=fresh();data.ko['settings.agentPreset'].guidePtcUsage+=' CSV';
 assert.throws(()=>validateCatalogs(data,source),/embedded technical literal/);
});

test('shortcut help preserves physical key tokens and storage paths',async()=>{
 const {validateCatalogs}=await import(file);
 const source={shortcuts:{help:'Mod+Shift+, Command+Option+key Ctrl+Alt+key Esc Tab Backslash Backquote localStorage dsh.keybindings.v1 userData/keybindings.json'}};
 const fresh=()=>Object.fromEntries(languages.map(({id})=>[id,structuredClone(source)]));
 const changed=fresh();changed.es.shortcuts.help=changed.es.shortcuts.help.replace('Mod+Shift+,','Mod Shift+,');
 assert.throws(()=>validateCatalogs(changed,source),/shortcut combination/);
 for(const token of ['Mod','Shift','Command','Option','Ctrl','Alt','Esc','Tab','Backslash','Backquote','localStorage','dsh.keybindings.v1','userData/keybindings.json']){
  const data=fresh();data.ru.shortcuts.help=data.ru.shortcuts.help.replace(token,'translated');
  assert.throws(()=>validateCatalogs(data,source),/embedded technical literal|shortcut combination/,token);
 }
});

test('actual CLI labels, package-id example and registry URL remain literal',async()=>{
 const {validateCatalogs}=await import(file);
 const source={chat:{'message.compaction.commandTitle':'compact'},conversation:{'tool.title.bash':'Bash','web.http':'HTTP'},pluginManager:{installGuideIdExample:'dsh-plugin-whale-pet',registryCustomPlaceholder:'https://npm.example.com/'}};
 const data=Object.fromEntries(languages.map(({id})=>[id,structuredClone(source)]));
 assert.doesNotThrow(()=>validateCatalogs(data,source));
 data.ru.chat['message.compaction.commandTitle']='translated';
 assert.throws(()=>validateCatalogs(data,source),/technical literal/);
 data.ru.chat['message.compaction.commandTitle']='compact';data.es.pluginManager.installGuideIdExample='translated-package-name';
 assert.throws(()=>validateCatalogs(data,source),/technical literal/);
});

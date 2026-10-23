import { readFileSync,existsSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import assert from 'node:assert/strict';
const source=new URL('../ui/i18n.js',import.meta.url);
const en=JSON.parse(readFileSync(new URL('../locales/ocean/en.json',import.meta.url)));
const ru=JSON.parse(readFileSync(new URL('../locales/ocean/ru.json',import.meta.url)));
function factory(){
 assert(existsSync(source),'owned pages have no built-in localization helper');
 const window={};vm.runInNewContext(readFileSync(source,'utf8'),{window});return window.OceanI18n;
}
test('owned UI localizer accepts native catalogs and preserves plain argument text once',()=>{
 const i18n=factory().create();
 assert.equal(i18n.t('common.cancel',{},'Cancel'),'Cancel');
 assert(i18n.update({language:'ru',messages:ru}));assert.equal(i18n.language,'ru');
 assert.equal(i18n.t('common.cancel',{},'Cancel'),'Отмена');
 const value='<img src=x onerror=alert(1)> {count}';
 assert.equal(i18n.t('setup.log_lines',{count:value},'Log lines: {count}'),'Строк журнала: '+value);
 assert.equal(i18n.t('missing.key',{},'English fallback'),'English fallback');
});
test('owned pairing page inserts localized placeholders as text and keeps proxy actions explicit', async()=>{
 const html=readFileSync(new URL('../ui/remote.html',import.meta.url),'utf8');const script=html.match(/<script>([\s\S]*?)<\/script>/)[1];
 const elements=new Map(),calls=[],requests=[];const payload={language:'ru',messages:{...ru,'remote.loading':'<img src=x onerror=alert(1)>'}};
 const document={hidden:false,documentElement:{lang:'en'},addEventListener(){},createElement(){return {textContent:'',className:''};},getElementById(id){
  if(!elements.has(id)) elements.set(id,{textContent:'',disabled:false,classList:{add(){},remove(){}},children:[],replaceChildren(...nodes){this.children=nodes;},addEventListener(name,fn){this[name]=fn;},set innerHTML(value){assert.equal(value,'<svg>trusted-native-qr</svg>','catalog or placeholder must not reach HTML');}});
  return elements.get(id);
 }};
 const context=vm.createContext({document,console,setInterval(){return 1;},clearInterval(){},window:{addEventListener(){},__TAURI_INTERNALS__:{invoke(cmd,args){calls.push([cmd,args]);if(cmd==='get_native_locale')return Promise.resolve(payload);if(cmd==='get_remote_state')return new Promise(resolve=>requests.push(resolve));return Promise.resolve(null);}}}});
 vm.runInContext(readFileSync(source,'utf8'),context);vm.runInContext(script,context);
 for(let i=0;i<12;i++)await Promise.resolve();
 assert.equal(document.documentElement.lang,'ru');assert.equal(elements.get('qr').children[0].textContent,'<img src=x onerror=alert(1)>');
 assert.equal(calls.some(([cmd])=>cmd==='start_pairing_proxy'),false);
 requests.shift()({qrSvg:null,note:'<b>raw diagnostic</b>',pairingUrl:null});for(let i=0;i<12;i++)await Promise.resolve();
 assert.equal(elements.get('note').textContent,'<b>raw diagnostic</b>');
 elements.get('proxy-5').click();for(let i=0;i<12;i++)await Promise.resolve();
 assert.equal(calls.filter(([cmd,args])=>cmd==='start_pairing_proxy'&&args.ttlSeconds===300).length,1);
});

test('invalid catalog metadata cannot select arbitrary language or replace valid catalog',()=>{
 const i18n=factory().create();assert(i18n.update({language:'en',messages:en}));
 for(const value of [null,{}, {language:'../../ru',messages:ru},{language:'ru',messages:{key:42}}]) {
  assert.equal(i18n.update(value),false);assert.equal(i18n.language,'en');
 }
 assert.equal(i18n.t('setup.log_lines',{},'Log lines: {count}'),'Log lines: {count}');
 assert.equal(i18n.t('setup.log_lines',{count:'2',extra:'bad'},'Log lines: {count}'),'Log lines: {count}');
});

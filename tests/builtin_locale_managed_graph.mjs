import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const native='@dsh-ocean/builtin-locales',legacy='@dsh-ocean/desktop-language-pack';
function definition(path){let value;vm.runInNewContext(readFileSync(path,'utf8'),{URL,console,document:{querySelector:()=>({}),querySelectorAll:()=>[]},navigator:{languages:['en'],language:'en'},window:{__ModuleLoader__:{load(registration){value=registration;}}}});return value;}
test('actual pinned ClientEntries yields native before live legacy creation and restores after removal',{skip:!process.env.DSH_LOCALE_CLIENT_SOURCE||!process.env.DSH_CLIENT_MODULES_SOURCE},async()=>{
 const nativeDefinition=definition(new URL('../locales/dsh/lib/client.js',import.meta.url));
 const {LocaleRuntime}=definition(process.env.DSH_LOCALE_CLIENT_SOURCE).factory(()=>({}));
 const clientModules=definition(process.env.DSH_CLIENT_MODULES_SOURCE).factory(()=>({}));
 const legacyDefinition={id:legacy,factory:()=>({apply(ctx){for(const id of ['ru','es','pt-BR','de','fr','ja','ko']){ctx.effect(()=>ctx.locale.addLanguage({id,label:'Manual '+id,fallback:'en'}));ctx.effect(()=>ctx.locale.register('manual',id,{owner:'manual'}));}}})};
 function wire(ids,revision){return {rev:revision,batches:ids.map(id=>({phase:'application',url:'https://fixture.invalid/'+(id===native?'native':'legacy')+'.js',rev:'same-code',entries:[id]})),entries:ids.map(id=>({id,url:'https://fixture.invalid/'+(id===native?'native':'legacy')+'.js',rev:'same-code',inject:[],external:[],immediately:true}))};}
 const first=wire([native],'first');const target={mode:'queue',pendingQueue:[]};
 const modules=new clientModules.ClientModuleSystem({manifest:clientModules.parseBootManifest(first),staticModules:{},bootstrapModule:{id:'@deepseek-ai/dsh-client-modules',exports:clientModules},registrationTarget:target,loadBundle:async url=>{target.load(url.endsWith('/native.js')?nativeDefinition:legacyDefinition);}});
 const locale=new LocaleRuntime({effect(fn){fn();},emit(){}});const entries=new Map();const rootEffects=[];const activeIds=()=>Array.from(locale.getLocale().locales,x=>x.id.toLowerCase()).sort();
 const loader={ctx:{effect(fn){rootEffects.push(fn());}},ensureId:({name})=>name,resolve:id=>entries.get(id),entries:()=>entries.values(),async create({name}){
   const effects=[];const entry={id:name,options:{name,config:{}},fiber:{state:2,await:async()=>{}},dispose(){for(const fn of effects.splice(0).reverse())if(typeof fn==='function')fn();}};entries.set(name,entry);
   const plugin=await modules.import(name,'',{});plugin.apply({modules,locale,effect(fn){effects.push(fn());}});
 },remove(id){entries.get(id)?.dispose();entries.delete(id);},await:async()=>{}};
 await modules.entries.start(loader,modules.manifest);assert.equal(activeIds().length,9);
 await modules.entries.sync(wire([native,legacy],'add-legacy'));
 assert.equal(modules.entries.state.getSnapshot().failures.length,0);assert.equal(activeIds().length,9);assert.equal(locale.bind('manual')('owner'),'owner');
 locale.setLocale('ru');assert.equal(locale.bind('manual')('owner'),'manual');
 await modules.entries.sync(wire([native],'remove-legacy'));
 assert.equal(modules.entries.state.getSnapshot().failures.length,0);assert.equal(activeIds().length,9);assert.equal(locale.bind('common')('cancel'),'Отмена');
 loader.remove(native);assert.deepEqual(activeIds(),['en','zh']);for(const dispose of rootEffects)await dispose();
});

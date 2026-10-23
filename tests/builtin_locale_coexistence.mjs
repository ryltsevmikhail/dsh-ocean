import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const native='@dsh-ocean/builtin-locales',legacy='@dsh-ocean/desktop-language-pack';
const ids=['ru','es','pt-BR','de','fr','ja','ko'];
class StrictLocale {
 constructor(){this.languages=new Map(['en','zh'].map(id=>[id,{id}]));this.dictionaries=new Map();}
 addLanguage(language){const id=language.id.toLowerCase();if(this.languages.has(id))throw Error('duplicate locale '+id);this.languages.set(id,language);return()=>{if(this.languages.get(id)===language)this.languages.delete(id);};}
 register(namespace,id,dictionary){const key=id.toLowerCase()+':'+namespace;if(this.dictionaries.has(key))throw Error('duplicate dictionary '+key);this.dictionaries.set(key,dictionary);return()=>{if(this.dictionaries.get(key)===dictionary)this.dictionaries.delete(key);};}
 getLocale(){return {locales:[...this.languages.values()]};}
}
function fixture(locale=new StrictLocale()){
 let definition;vm.runInNewContext(readFileSync(new URL('../locales/dsh/lib/client.js',import.meta.url),'utf8'),{window:{__ModuleLoader__:{load(value){definition=value;}}}});
 let snapshot={syncing:false,failures:[]};const listeners=new Set();
 const modules={manifest:{plugins:[{id:native}]},entries:{state:{getSnapshot:()=>snapshot,subscribe(fn){listeners.add(fn);return()=>listeners.delete(fn);}}}};
 const effects=[];const ctx={locale,modules,effect(fn){effects.push(fn());}};
 return {locale,modules,mount(){definition.factory().apply(ctx);},dispose(){for(const dispose of effects.splice(0).reverse())if(typeof dispose==='function')dispose();},syncing(value){snapshot={syncing:value,failures:[]};for(const fn of [...listeners])fn();},codes(){return Array.from(locale.getLocale().locales,x=>x.id.toLowerCase()).sort();}};
}
function mountLegacy(locale){const disposers=[];for(const id of ids){disposers.push(locale.addLanguage({id,label:'Manual '+id,fallback:'en'}));disposers.push(locale.register('manual',id,{owner:'manual'}));}return()=>{for(const dispose of disposers.splice(0).reverse())dispose();};}
function assertions(factory){
 for(const nativeFirst of [true,false]){
  const f=factory();f.modules.manifest.plugins.push({id:legacy});let remove;
  if(nativeFirst){f.mount();remove=mountLegacy(f.locale);}else{remove=mountLegacy(f.locale);f.mount();}
  f.dispose();assert.equal(f.codes().length,9,'native disposal must retain every manual language');remove();assert.deepEqual(f.codes(),['en','zh']);
 }
 const f=factory();f.mount();assert.equal(f.codes().length,9);
 // Real entries publish syncing=true synchronously BEFORE replacing manifest.
 f.syncing(true);assert.deepEqual(f.codes(),['en','zh'],'yield before legacy can activate, while manifest still contains only native');
 f.modules.manifest.plugins.push({id:legacy});const remove=mountLegacy(f.locale);f.syncing(false);assert.equal(f.codes().length,9);
 f.syncing(true);remove();f.modules.manifest.plugins=[{id:native}];f.syncing(false);assert.equal(f.codes().length,9,'legacy removal restores native');
 f.dispose();f.dispose();assert.deepEqual(f.codes(),['en','zh']);
}
test('native locales yield to managed manual graph regardless of activation order and live reconciliation',()=>assertions(()=>fixture()));
test('existing untracked language is preserved without a duplicate error or partial native registration',()=>{
 const f=fixture();const remove=f.locale.addLanguage({id:'fr',label:'Foreign',fallback:'en'});assert.doesNotThrow(()=>f.mount());assert.deepEqual(f.codes(),['en','fr','zh']);assert.equal(f.locale.dictionaries.size,0);f.dispose();assert.deepEqual(f.codes(),['en','fr','zh']);remove();
});
test('a registration failure disposes every partial native dictionary and language',()=>{
 const f=fixture();const register=f.locale.register.bind(f.locale);let calls=0;f.locale.register=(...args)=>{if(++calls===5)throw Error('fixture registration failure');return register(...args);};assert.throws(()=>f.mount(),/fixture registration failure/);assert.deepEqual(f.codes(),['en','zh']);assert.equal(f.locale.dictionaries.size,0);f.dispose();
});
test('coexistence also uses the actual pinned public LocaleRuntime',{skip:!process.env.DSH_LOCALE_CLIENT_SOURCE},()=>{
 let definition;vm.runInNewContext(readFileSync(process.env.DSH_LOCALE_CLIENT_SOURCE,'utf8'),{navigator:{languages:['en'],language:'en'},window:{__ModuleLoader__:{load(value){definition=value;}}}});
 const {LocaleRuntime}=definition.factory(()=>({}));assertions(()=>fixture(new LocaleRuntime({effect(fn){fn();},emit(){}})));
});

import test from 'node:test';
import assert from 'node:assert/strict';
const source=new URL('../scripts/builtin-locale-preflight.mjs',import.meta.url);
test('SDK adapter refuses incomplete scope before touching SDK or ambient home',async()=>{
 const {inspectConfigured}=await import(source);let calls=0;
 const sdk=new Proxy({}, {get(){calls++;throw new Error('must not access SDK');}});
 assert.equal((await inspectConfigured({sdk,profile:{name:'web',dir:'/private/profiles/web',patchPath:'/private/profiles/web/cordis.patch.yml',layers:[],skippedBundles:[]},installAnchor:'/private/sdk/package.json',cwd:'/private',resolveIdentity(){}})).state,'unknown');
 assert.equal(calls,0);
});

test('SDK adapter uses one detached composition with explicit scope and treats warnings as unknown',async()=>{
 const {inspectConfigured}=await import(source);let seen;let warn=false;
 const profile={name:'web',dir:'/private/profiles/web',patchPath:'/private/profiles/web/cordis.patch.yml',layers:[{packageName:'core'}],skippedBundles:[]};
 const sdk={readProfilePatches(_bin,context,initial){seen=context;assert.equal(initial,profile);return [{fixture:true}];},composeEntries(layers,warning){assert.equal(layers.length,1);if(warn)warning('unsupported');return [{id:'core',name:'core'}];},async createRuntimeResolution(options){assert.equal(options.profile,profile);assert.equal(options.home,'/private');return {};}};
 const options={sdk,profile,installAnchor:'/sdk/package.json',home:'/private',cwd:'/work',resolveIdentity:()=>({name:'core'})};
 assert.equal((await inspectConfigured(options)).state,'absent');assert.equal(seen.home,'/private');assert.deepEqual(seen.overlays,[]);
 warn=true;assert.equal((await inspectConfigured(options)).state,'unknown');
});

test('configured legacy identity follows resolved manifests, not names or installed dependencies',async()=>{
 const {classify}=await import(source);
 const resolve=name=>({name:name==='alias'?'@dsh-ocean/desktop-language-pack':name});
 assert.equal((await classify([{id:'core',name:'@deepseek-ai/dsh-client-locale'}],resolve)).state,'absent');
 assert.equal((await classify([{id:'manual',name:'alias'}],resolve)).state,'enabled');
 assert.equal((await classify([{id:'manual',name:'alias',disabled:true}],resolve)).state,'disabled');
});
test('literal group ancestry matches Loader disable semantics; unknown composition cannot authorize',async()=>{
 const {classify}=await import(source);const resolve=()=>({name:'@dsh-ocean/desktop-language-pack'});
 assert.equal((await classify([{id:'g',name:'group',group:true,disabled:true,config:[{id:'manual',name:'file:///private/alias.js'}]}],resolve)).state,'disabled');
 for(const rows of [[{id:'x',name:{__jsExpr:'dynamic'}}],[{id:'x',name:'cordis:include',config:{path:'./extra.yml'}}],[{id:'x',name:'group',group:true,config:'dynamic'}],[{id:'x',name:'alias',disabled:{__jsExpr:'dynamic'}}]]) {
  assert.equal((await classify(rows,resolve)).state,'unknown');
 }
 assert.equal((await classify([{id:'x',name:'unresolved'}],()=>undefined)).state,'unknown');
 assert.equal((await classify([{id:'x',name:'include-alias'}],()=>({name:'@deepseek-ai/cordis-plugin-include'}))).state,'unknown');
});

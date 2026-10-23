import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,mkdirSync,writeFileSync,existsSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve,dirname} from 'node:path';
import {pathToFileURL} from 'node:url';
import {owningManifest,readOnlyProfile,preflight,ordinaryModuleTarget} from '../scripts/builtin-locale-sdk-preflight.mjs';
function fixture(t){const root=mkdtempSync(join(tmpdir(),'ocean-preflight-test-'));t.after(()=>{const path=resolve(root);assert.equal(dirname(path),resolve(tmpdir()));assert(path.split('/').at(-1).startsWith('ocean-preflight-test-'));rmSync(path,{recursive:true});});return root;}
test('published SDK types/default export is ordinary but custom conditions stay unknown',()=>{
 assert.equal(ordinaryModuleTarget({exports:{'.':{types:'./lib/types/index.d.ts',default:'./lib/index.js'}}}),'./lib/index.js');
 assert.equal(ordinaryModuleTarget({main:'lib/index.js'}),'./lib/index.js');
 assert.equal(ordinaryModuleTarget({exports:{'.':{node:'./node.js',default:'./fallback.js'}}}),undefined);
 assert.equal(ordinaryModuleTarget({exports:'../foreign/index.js'}),undefined);
});
test('file identity is obtained from metadata without importing plugin code',t=>{
 const root=fixture(t);mkdirSync(join(root,'lib'));writeFileSync(join(root,'package.json'),JSON.stringify({name:'@dsh-ocean/desktop-language-pack'}));
 writeFileSync(join(root,'lib/anything.js'),"throw new Error('configured plugin must never execute');");
 assert.equal(owningManifest(pathToFileURL(join(root,'lib/anything.js')).href).name,'@dsh-ocean/desktop-language-pack');
 assert.equal(owningManifest('https://foreign/plugin.js'),undefined);
});
test('fresh profile synthesis reads shipped patches and orphaned user patch without creating profile',t=>{
 const root=fixture(t),profileDir=join(root,'profiles/web');let optional;
 const sdk={PROFILE_TEMPLATES:{web:{bundles:['core','web']}},resolveBundleDir(_bin,name){return '/sdk/'+name;},readProfileManifest(){return {dsh:{bundle:{patch:'./patch.yml'}}};},bundlePatchPaths(dir){return [dir+'/patch.yml'];},loadOverlayPatches(_bin,path){return [{source:path}];},loadOptionalPatches(_bin,path){optional=path;return [{orphan:true}];}};
 const profile=readOnlyProfile(sdk,profileDir,'/sdk/package.json');
 assert.equal(profile.layers.length,2);assert.equal(profile.patches[0].orphan,true);assert.equal(optional,join(profileDir,'cordis.patch.yml'));assert(!existsSync(profileDir));
});
test('missing explicit SDK/home scope never falls back to a host installation',async()=>{
 assert.equal((await preflight({profileDir:'/private/profiles/web'})).state,'unknown');
});

import test from 'node:test';import assert from 'node:assert/strict';import {spawnSync} from 'node:child_process';
import {mkdtempSync,existsSync,rmSync,mkdirSync,writeFileSync,readFileSync} from 'node:fs';import {tmpdir} from 'node:os';import {resolve,join,dirname} from 'node:path';import {fileURLToPath} from 'node:url';
const fixture=process.env.DSH_OCEAN_SDK_FIXTURE;
function removeFixture(home){const path=resolve(home);assert.equal(dirname(path),resolve(tmpdir()));assert(path.split('/').at(-1).startsWith('ocean-real-sdk-preflight-'));rmSync(path,{recursive:true});}
function inspectFixture(home){const sdk=resolve(fixture),dshHome=join(home,'.dsh');const request={installAnchor:join(sdk,'harness/node_modules/@deepseek-ai/dsh/package.json'),profileDir:join(dshHome,'profiles/web'),home:dshHome,cwd:home,telemetryDisabledEnv:'1'};const result=spawnSync(join(sdk,'runtime/node/bin/node'),['--experimental-import-meta-resolve',fileURLToPath(new URL('../scripts/builtin-locale-sdk-preflight.mjs',import.meta.url)),JSON.stringify(request)],{env:{HOME:home,DSH_HOME:dshHome,PATH:'/usr/bin:/bin'},encoding:'utf8',timeout:30000,maxBuffer:8192});assert.equal(result.error,undefined);assert.equal(result.status,0,result.stderr);return JSON.parse(result.stdout);}

test('complete SDK detects configured legacy file entry and preserves explicit disabled policy',{skip:!fixture},()=>{
 const home=mkdtempSync(join(tmpdir(),'ocean-real-sdk-preflight-'));try{
 const profile=join(home,'.dsh/profiles/web'),pack=join(profile,'node_modules/@dsh-ocean/desktop-language-pack');mkdirSync(join(pack,'lib'),{recursive:true});
 const manifest={name:'ocean-private-profile',private:true,version:'1.0.0',dependencies:{'@dsh-ocean/desktop-language-pack':'file:'+pack},dsh:{profile:{bundles:['@deepseek-ai/dsh-base','@deepseek-ai/dsh-web-app','@dsh-ocean/desktop-language-pack']}}};
 const profileManifest=JSON.stringify(manifest);writeFileSync(join(profile,'package.json'),profileManifest);
 writeFileSync(join(pack,'package.json'),JSON.stringify({name:'@dsh-ocean/desktop-language-pack',version:'0.4.0',type:'module',exports:{'.':'./lib/index.js'},dsh:{bundle:{patch:'./cordis.patch.yml'}}}));
 writeFileSync(join(pack,'lib/index.js'),"throw new Error('preflight imported configured plugin');");writeFileSync(join(pack,'cordis.patch.yml'),'- insert:\n    - id: legacy-language\n      name: ./lib/index.js\n');
 assert.deepEqual(inspectFixture(home),{state:'enabled'});
 const userPatch='- id: legacy-language\n  disabled: true\n';writeFileSync(join(profile,'cordis.patch.yml'),userPatch);
 assert.deepEqual(inspectFixture(home),{state:'disabled'});assert.equal(readFileSync(join(profile,'package.json'),'utf8'),profileManifest);assert.equal(readFileSync(join(profile,'cordis.patch.yml'),'utf8'),userPatch);assert(!existsSync(join(profile,'cordis.yml')));
 writeFileSync(join(profile,'cordis.patch.yml'),'- id: legacy-language\n  disabled: !!js runtimeFlag\n');assert.deepEqual(inspectFixture(home),{state:'unknown'});
 writeFileSync(join(profile,'cordis.patch.yml'),'- insert:\n    - id: nested\n      name: cordis:include\n      config:\n        path: ./extra.yml\n');assert.deepEqual(inspectFixture(home),{state:'unknown'});assert(!existsSync(join(profile,'extra.yml')));
 }finally{removeFixture(home);}
});
test('complete pinned SDK fresh web preflight is absent and creates no profile',{skip:!fixture},()=>{
 const sdk=resolve(fixture);const home=mkdtempSync(join(tmpdir(),'ocean-real-sdk-preflight-'));try{
  const dshHome=join(home,'.dsh'),profileDir=join(dshHome,'profiles/web');
  const request={installAnchor:join(sdk,'harness/node_modules/@deepseek-ai/dsh/package.json'),profileDir,home:dshHome,cwd:home,telemetryDisabledEnv:'1'};
  const result=spawnSync(join(sdk,'runtime/node/bin/node'),['--experimental-import-meta-resolve',fileURLToPath(new URL('../scripts/builtin-locale-sdk-preflight.mjs',import.meta.url)),JSON.stringify(request)],{env:{HOME:home,DSH_HOME:dshHome,PATH:'/usr/bin:/bin'},encoding:'utf8',timeout:30000,maxBuffer:8192});
  assert.equal(result.error,undefined);assert.equal(result.status,0,result.stderr);assert.deepEqual(JSON.parse(result.stdout),{state:'absent'});assert(!existsSync(profileDir));assert(!existsSync(dshHome));
 }finally{const path=resolve(home);assert.equal(dirname(path),resolve(tmpdir()));assert(path.split('/').at(-1).startsWith('ocean-real-sdk-preflight-'));rmSync(path,{recursive:true});}
});

// Real tarball installation in an owned HOME; never uses the live web profile.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, realpathSync } from 'node:fs';
import { resolve, basename, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { verifyBrowser } from './verify-browser.mjs';
import { languages } from '../src/register.mjs';
import { runOwned, startOwned, installSignalCleanup } from './owned-process.mjs';
const root = fileURLToPath(new URL('../',import.meta.url));

export function fixtureEnvironment(fixtureRoot, base = process.env) {
 if (!fixtureRoot.startsWith('/') || !/^language-pack-(?:fixture|test)-[\w-]+$/.test(basename(fixtureRoot))) throw Error('unsafe fixture root');
 const home = join(fixtureRoot,'home');
 return {
  PATH: base.PATH, LANG: base.LANG || 'C.UTF-8', HOME: home, DSH_HOME: join(home,'.dsh'),
  XDG_CONFIG_HOME: join(home,'.config'), XDG_DATA_HOME: join(home,'.local/share'),
  XDG_STATE_HOME: join(home,'.local/state'), XDG_CACHE_HOME: join(home,'.cache'),
  COREPACK_HOME: join(fixtureRoot,'corepack'), PNPM_HOME: join(fixtureRoot,'pnpm'),
  npm_config_cache: join(fixtureRoot,'npm-cache'), npm_config_prefix: join(fixtureRoot,'npm-prefix'),
  npm_config_store_dir: join(fixtureRoot,'pnpm-store'), npm_config_userconfig: join(fixtureRoot,'npmrc'),
  npm_config_ignore_scripts: 'true', CI: 'true',
 };
}

export function parsePackOutput(text) {
 const parsed=JSON.parse(text);
 const entries=Array.isArray(parsed)?parsed:Object.values(parsed);
 assert.equal(entries.length,1,'npm pack must describe exactly one artifact');
 const entry=entries[0];
 assert.ok(entry && typeof entry.filename==='string' && entry.filename===basename(entry.filename) && entry.filename.endsWith('.tgz'),'unsafe npm pack filename');
 assert.ok(Array.isArray(entry.files) && entry.files.every(file=>typeof file.path==='string'),'npm pack file inventory missing');
 return entry;
}

export function parseFixtureLaunch(text) {
 const match = text.match(/dsh web: (http:\/\/127\.0\.0\.1:(\d+)\/\?token=[^\s\u001b)]+)/);
 if (!match || match[2] === '3080' || match[2] === '0') return undefined;
 const url = new URL(match[1]);
 return { address: url.origin, launchUrl: url.href };
}

async function main() {
 const removeSignals=installSignalCleanup();
 try {
  const install = realpathSync(process.env.DSH_INSTALL || '/home/ryltsevmg/.local/opt/dsh');
  const cli = join(install,'node_modules/@deepseek-ai/dsh/lib/bin.js');
  const runtime=JSON.parse(readFileSync(join(install,'node_modules/@deepseek-ai/dsh/package.json')));
  assert.equal(runtime.version,'0.2.0-rc.2');
  const inventory=JSON.parse(readFileSync(join(root,'data/inventory.json')));
  const sha256=bytes=>createHash('sha256').update(bytes).digest('hex');
  assert.equal(sha256(readFileSync(join(install,'node_modules/@deepseek-ai/dsh/package.json'))),inventory.runtime.sha256,'runtime manifest differs from canonical inventory');
  for(const source of inventory.sources) assert.equal(sha256(readFileSync(join(install,'node_modules',source.package,source.artifact))),source.sha256,`canonical artifact changed: ${source.package}`);
  const artifacts = join(root,'artifacts');
  mkdirSync(artifacts,{recursive:true});
  const fixture = realpathSync(mkdtempSync(join(artifacts,'language-pack-fixture-')));
  const env = fixtureEnvironment(fixture);
  mkdirSync(env.HOME,{recursive:true});writeFileSync(env.npm_config_userconfig,'ignore-scripts=true\n');
  const profile = 'language-pack-fixture';
  async function run(label, executable, args, cwd = root) {
   const result = await runOwned(executable,args,{cwd,env,timeoutMs:180000});
   writeFileSync(join(fixture,label+'.log'),result.stdout+result.stderr);
   assert.ok(!result.timedOut && result.status===0 && result.stopped,`${label} failed; inspect ${join(fixture,label+'.log')}`);
   return result.stdout;
  }
  const pack = parsePackOutput(await run('pack','npm',['pack','--ignore-scripts','--json','--pack-destination',artifacts]));
  const tarball = join(artifacts,pack.filename), members = pack.files.map(x=>x.path);
  assert.ok(members.includes('lib/client.js'));assert.ok(members.includes('cordis.patch.yml'));
  assert.ok(!members.some(x=>x.startsWith('artifacts/')||x.startsWith('node_modules/')));
  const freshProfile=profile+'-fresh';
  await run('init-fresh',process.execPath,[cli,'--profile',freshProfile,'--from-default-profile','web','--dump-config']);
  await run('install-fresh',process.execPath,[cli,'plugin','--profile',freshProfile,'add',tarball,'--ignore-scripts']);
  const freshConfig=await run('dump-config-fresh',process.execPath,[cli,'--profile',freshProfile,'--dump-config']);
  assert.ok(freshConfig.includes('@dsh-ocean/desktop-language-pack'));
  assert.equal(readFileSync(join(env.DSH_HOME,'profiles',freshProfile,'node_modules/@dsh-ocean/desktop-language-pack/lib/client.js'),'utf8'),readFileSync(join(root,'lib/client.js'),'utf8'));
  await run('init',process.execPath,[cli,'--profile',profile,'--from-default-profile','web','--dump-config']);
  const rollbackTarball=join(artifacts,'dsh-ocean-desktop-language-pack-0.1.0.tgz');
  assert.equal(sha256(readFileSync(rollbackTarball)),'e2451d3b2b725dfea5b201c488c91695469d5d72596be4d30e39b4fb5b052b2e','rollback tarball changed');
  assert.equal(sha256(readFileSync(join(artifacts,'dsh-ocean-desktop-language-pack-0.2.0.tgz'))),'e65ecd11afb60815c2968111f78ef86a8b5cde8aa09a3920d3e612ac6846c117','0.2.0 archive changed');
   const previousTarball=join(artifacts,'dsh-ocean-desktop-language-pack-0.3.0.tgz');
   assert.equal(sha256(readFileSync(previousTarball)),'4f3002b454f7acaa7018656733661bea01cf9fda9bb8859edbe4593ad11eb354','previous tarball changed');
   await run('install-previous',process.execPath,[cli,'plugin','--profile',profile,'add',previousTarball,'--ignore-scripts']);
  const installedPath=join(env.DSH_HOME,'profiles',profile,'node_modules/@dsh-ocean/desktop-language-pack');
  assert.equal(JSON.parse(readFileSync(join(installedPath,'package.json'))).version,'0.3.0');
  await run('install',process.execPath,[cli,'plugin','--profile',profile,'add',tarball,'--ignore-scripts']);
  assert.equal(JSON.parse(readFileSync(join(installedPath,'package.json'))).version,'0.4.0');
  const composed = await run('dump-config',process.execPath,[cli,'--profile',profile,'--dump-config']);
  assert.ok(composed.includes('@dsh-ocean/desktop-language-pack'));
  const installed = join(env.DSH_HOME,'profiles',profile,'node_modules/@dsh-ocean/desktop-language-pack');
  const builtClient=readFileSync(join(root,'lib/client.js'),'utf8');
  assert.equal(readFileSync(join(installed,'lib/client.js'),'utf8'),builtClient);
  const browserProfile=process.env.VERIFY_FRESH_BROWSER==='1'?freshProfile:profile;
   let log='', address, launchUrl, server, timeout, settled=false, resolveReady,rejectReady;
  const ready=new Promise((resolve,reject)=>{resolveReady=resolve;rejectReady=reject;});
  // Ensure an early startup error never becomes an unhandled rejection.
  ready.catch(()=>{});
  const redact = text => text.replace(/([?&](?:token|auth)=)[^\s&]+/g,'$1<redacted>');
  let response,serverCleanup;
  try {
   timeout=setTimeout(()=>rejectReady(Error('fixture startup timed out')),45000);
   server=await startOwned(process.execPath,[cli,'--profile',browserProfile,'--host','127.0.0.1','--port','0','--no-open'],{cwd:fixture,env,onOutput(chunk){
    log+=chunk;const found=parseFixtureLaunch(log);
    if(found && !settled){settled=true;({address,launchUrl}=found);clearTimeout(timeout);resolveReady();}
   }});
   server.wait().then(()=>{if(!settled)rejectReady(Error('fixture exited before readiness'));});
   await ready;assert.notEqual(new URL(address).port,'3080');
   const exchange=await fetch(launchUrl,{redirect:'manual',signal:AbortSignal.timeout(15000)});
   assert.equal(exchange.status,303);
   const cookie=exchange.headers.get('set-cookie')?.split(';')[0];assert.ok(cookie,'fixture launch cookie missing');
   const headers={cookie};
   const htmlResponse=await fetch(address,{headers,signal:AbortSignal.timeout(15000)});assert.equal(htmlResponse.status,200);
   const html=await htmlResponse.text();writeFileSync(join(fixture,'index.html'),html);
   assert.ok(html.includes('@dsh-ocean/desktop-language-pack'),'pack missing from actual browser boot graph');
   const urls=[...html.matchAll(/(?:src|href)="([^"]*plugins\/[^\"]+)"/g)].map(m=>m[1].replaceAll('&amp;','&'));
   const packUrl=urls.find(url=>url.includes('@dsh-ocean/desktop-language-pack'));assert.ok(packUrl,'pack browser script/preload missing');
   const scriptResponse=await fetch(new URL(packUrl,address),{headers,signal:AbortSignal.timeout(15000)});assert.equal(scriptResponse.status,200);
   const script=await scriptResponse.text();assert.ok(script.includes(builtClient.trim()),'served pack bytes differ from installed tarball artifact');
   writeFileSync(join(fixture,'served-client.js'),script);
   const catalogs=Object.fromEntries(languages.map(({id})=>[id,JSON.parse(readFileSync(join(root,'data',id+'.json')))]));
   const browser=await verifyBrowser(launchUrl,fixture,env,catalogs);
   response={fixture,home:env.HOME,profile,browserProfile,address,tarball,sha256:createHash('sha256').update(readFileSync(tarball)).digest('hex'),installed:true,freshInstall:true,updatedFrom:'0.3.0',previousTarball,rollbackTarball,composed:true,bootGraph:true,servedClient:true,browser,members};
  } finally {
   clearTimeout(timeout);
   if(server)serverCleanup=await server.stop();
   writeFileSync(join(fixture,'server.log'),redact(log));
  }
  assert.equal(serverCleanup?.stopped,true,'server process group not confirmed stopped');
  response.serverStopped=serverCleanup.stopped;
  writeFileSync(join(artifacts,process.env.VERIFY_FRESH_BROWSER==='1'?'fixture-result-fresh.json':'fixture-result.json'),JSON.stringify(response,null,2)+'\n');
  console.log(JSON.stringify(response,null,2));
 } finally {removeSignals();}
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
 try {await main();}
 catch(error) {console.error(String(error.stack || error).replace(/([?&](?:token|auth)=)[^\s&]+/g,'$1<redacted>'));process.exitCode=1;}
}

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
const file=new URL('../scripts/owned-process.mjs',import.meta.url);
test('owned process supervisor refuses invalid process group ids and handles spawn failure',async()=>{
 assert.ok(existsSync(file),'owned process supervisor missing');
 const {assertOwnedPid,startOwned}=await import(file);
 for(const pid of [undefined,0,-1,1,NaN,Infinity,'123'])assert.throws(()=>assertOwnedPid(pid),/process group/);
 await assert.rejects(startOwned('/definitely/no-such-language-pack-executable',[],{env:{PATH:'/bin'}}),/spawn/);
});
test('owned process cleanup kills a surviving TERM-resistant descendant after leader exits',async()=>{
 assert.ok(existsSync(file),'owned process supervisor missing');
 const {startOwned,liveGroupMembers}=await import(file);
 const script=`const {spawn}=require('node:child_process');const child=spawn(process.execPath,['-e',"process.on('SIGTERM',()=>{});setInterval(()=>{},1000)"],{stdio:'ignore'});child.on('spawn',()=>setTimeout(()=>{console.log('ready');process.exit(0)},150));`;
 const child=await startOwned(process.execPath,['-e',script],{env:{PATH:process.env.PATH},graceMs:150});
 await child.wait();
 assert.ok(liveGroupMembers(child.pid).length>0,'fixture descendant should survive its leader before cleanup');
 assert.deepEqual(await child.stop(),{stopped:true,escalated:true});
 assert.deepEqual(liveGroupMembers(child.pid),[]);
 assert.deepEqual(await child.stop(),{stopped:true,escalated:true});
});
test('bounded commands stop their whole group on timeout and preserve successful output',async()=>{
 assert.ok(existsSync(file),'owned process supervisor missing');
 const {runOwned,liveGroupMembers}=await import(file);
 const result=await runOwned(process.execPath,['-e',"console.log('fixture');setInterval(()=>{},1000)"],{env:{PATH:process.env.PATH},timeoutMs:150,graceMs:100});
 assert.equal(result.timedOut,true);assert.ok(result.stdout.includes('fixture'));assert.equal(result.stopped,true);assert.deepEqual(liveGroupMembers(result.pid),[]);
 const success=await runOwned(process.execPath,['-e',"console.log('ok')"],{env:{PATH:process.env.PATH},timeoutMs:1000});
 assert.equal(success.status,0);assert.equal(success.stdout.trim(),'ok');assert.equal(success.stopped,true);
});

test('large command output is drained after leader exit before returning results',async()=>{
 const {startOwned,runOwned}=await import(file);
 const owner=await startOwned(process.execPath,['-e',"process.stdout.write('x'.repeat(4*1024*1024),()=>process.exit(0))"],{env:{PATH:process.env.PATH}});
 try {
  assert.equal(typeof owner.drained,'function','stdio drain completion is missing');
  await owner.wait();await owner.drained();
  assert.equal(owner.stdout.length,4*1024*1024);
 } finally {await owner.stop();}
 const result=await runOwned(process.execPath,['-e',"process.stdout.write(JSON.stringify({data:'x'.repeat(4*1024*1024)}),()=>process.exit(0))"],{env:{PATH:process.env.PATH},timeoutMs:5000});
 assert.equal(JSON.parse(result.stdout).data.length,4*1024*1024);
});

test('interrupt handlers also stop separately detached managed children',async()=>{
 const {runOwned,liveGroupMembers}=await import(file);
 const script=`import {startOwned,installSignalCleanup} from ${JSON.stringify(file.href)};installSignalCleanup();const child=await startOwned(process.execPath,['-e','setInterval(()=>{},1000)'],{env:{PATH:process.env.PATH}});console.log('owned-child:'+child.pid);setInterval(()=>{},1000);`;
 const result=await runOwned(process.execPath,['--input-type=module','-e',script],{env:{PATH:process.env.PATH},timeoutMs:350,graceMs:2000});
 assert.equal(result.timedOut,true);
 const pid=Number(result.stdout.match(/owned-child:(\d+)/)?.[1]);assert.ok(pid>1);
 assert.deepEqual(liveGroupMembers(pid),[]);
});

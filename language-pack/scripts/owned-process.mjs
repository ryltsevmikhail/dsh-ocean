// Linux-only integration-test process ownership, including orphaned descendants.
import { spawn } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';
const active=new Set();

export function assertOwnedPid(pid) {
 if(!Number.isSafeInteger(pid)||pid<=1)throw Error('invalid owned process group');
}

/** Zombies execute no code and hold no sockets; report only live group members. */
export function liveGroupMembers(pgid) {
 assertOwnedPid(pgid);
 const result=[];
 for(const name of readdirSync('/proc')) {
  if(!/^\d+$/.test(name))continue;
  try {
   const stat=readFileSync(`/proc/${name}/stat`,'utf8');
   const fields=stat.slice(stat.lastIndexOf(')')+2).split(' ');
   if(Number(fields[2])===pgid && fields[0]!=='Z' && fields[0]!=='X')result.push(Number(name));
  } catch(error) {if(!['ENOENT','ESRCH','EACCES'].includes(error.code))throw error;}
 }
 return result;
}

async function waitGroup(pgid,timeoutMs) {
 const deadline=Date.now()+timeoutMs;
 while(liveGroupMembers(pgid).length) {
  if(Date.now()>=deadline)return false;
  await delay(25);
 }
 return true;
}

export async function startOwned(command,args,{cwd,env,onOutput,graceMs=1500}={}) {
 const child=spawn(command,args,{cwd,env,stdio:['ignore','pipe','pipe'],detached:true});
 let stdout='',stderr='',stopPromise;
 const ready=new Promise((resolve,reject)=>{child.once('spawn',resolve);child.once('error',reject);});
 const finished=new Promise(resolve=>{child.once('exit',(status,signal)=>resolve({status,signal}));child.once('error',error=>resolve({status:null,signal:null,error}));});
 const drained=new Promise(resolve=>child.once('close',resolve));
 const owner={
  get pid(){return child.pid;},
  get stdout(){return stdout;},get stderr(){return stderr;},
  wait(){return finished;},
  drained(){return drained;},
  stop(){
   if(stopPromise)return stopPromise;
   stopPromise=(async()=>{
    try {await ready;}catch{active.delete(owner);return {stopped:true,escalated:false};}
    assertOwnedPid(child.pid);
    let escalated=false;
    if(liveGroupMembers(child.pid).length) {
     try{process.kill(-child.pid,'SIGTERM');}catch(error){if(error.code!=='ESRCH')throw error;}
     if(!await waitGroup(child.pid,graceMs)) {
      escalated=true;
      try{process.kill(-child.pid,'SIGKILL');}catch(error){if(error.code!=='ESRCH')throw error;}
     }
    }
    const stopped=await waitGroup(child.pid,3000);
    if(!stopped)throw Error(`owned process group ${child.pid} did not stop`);
    await finished;
    let drainTimeout;
    try {await Promise.race([drained,new Promise((_,reject)=>{drainTimeout=setTimeout(()=>reject(Error('owned process stdio failed to close')),3000);})]);}
    finally {clearTimeout(drainTimeout);}
    active.delete(owner);
    return {stopped,escalated};
   })();
   return stopPromise;
  },
 };
 active.add(owner);
 child.stdout.on('data',chunk=>{stdout+=chunk;if(stdout.length>16*1024*1024)stdout=stdout.slice(-16*1024*1024);onOutput?.(String(chunk),'stdout');});
 child.stderr.on('data',chunk=>{stderr+=chunk;if(stderr.length>16*1024*1024)stderr=stderr.slice(-16*1024*1024);onOutput?.(String(chunk),'stderr');});
 try {await ready;assertOwnedPid(child.pid);return owner;}catch(error){await owner.stop();throw error;}
}

export async function runOwned(command,args,{timeoutMs=180000,...options}={}) {
 const owner=await startOwned(command,args,options);
 let timer,timedOut=false;
 try {
  const result=await Promise.race([owner.wait(),new Promise(resolve=>{timer=setTimeout(()=>{timedOut=true;resolve({status:null,signal:'timeout'});},timeoutMs);})]);
  const cleanup=await owner.stop();
  return {...result,...cleanup,timedOut,pid:owner.pid,stdout:owner.stdout,stderr:owner.stderr};
 } finally {clearTimeout(timer);await owner.stop();}
}

/** Install only in the executable fixture runner, never as an import side effect. */
export function installSignalCleanup() {
 let stopping=false;
 const handlers=new Map();
 for(const [signal,code] of [['SIGINT',130],['SIGTERM',143]]) {
  const handler=async()=>{
   if(stopping)return;stopping=true;
   try{await Promise.all([...active].map(owner=>owner.stop()));process.exit(code);}
   catch{process.exit(1);}
  };
  handlers.set(signal,handler);process.on(signal,handler);
 }
 return ()=>{for(const [signal,handler]of handlers)process.off(signal,handler);};
}

const {readFileSync}=require('node:fs');
const vm=require('node:vm');
const assert=require('node:assert/strict');
const test=require('node:test');
const path=process.env.VISIBILITY_FILE || require('node:path').join(require('node:os').homedir(), '.dsh/profiles/web/node_modules/dsh-plugin-tool-management/lib/mcp/tool-visibility.js');
const source=readFileSync(path,'utf8').replace(/^import .*;$/gm,'').replace('export function createToolVisibility','function createToolVisibility');
function fixture(){
 const timers=[]; let map={}; let resolver=async a=>a.hidden; let changes=0;
 const context={MCP_TOOL_PREFIX:'mcp__',noteRuntime(){},clearRuntimeNote(){},setTimeout(fn){timers.push(fn);return fn},clearTimeout(fn){const i=timers.indexOf(fn);if(i>=0)timers.splice(i,1)}};
 vm.createContext(context);vm.runInContext(source+';globalThis.factory=createToolVisibility',context);
 const v=context.factory({SIDECAR_TTL_MS:0,ensurePaths:async()=>({}),readJsonFile:async()=>map,mcpSidecar:x=>x,MCP_DISABLED_TOOLS_FILE:'disabled',tools:{restrict(){},schemas:async()=>['mcp__s__x','mcp__s__y'].map(name=>({name}))},carrierHiddenTools:a=>resolver(a)});
 function agent(hidden=[]){const a={hidden,active:new Set(),ctx:{tools:{restrict({deny}){const layer=[...deny];a.active.add(layer);changes++;v.scheduleToolRestrictions();return ()=>{a.active.delete(layer);changes++;v.scheduleToolRestrictions()}}}}};v.attachAgent(a);return a}
 const flush=async()=>{for(let i=0;i<20;i++)await Promise.resolve()};
 async function drain(){await flush();for(let i=0;i<8&&timers.length;i++){timers.shift()();await flush()}assert.equal(timers.length,0,'tools/change must converge');}
 const names=a=>[...a.active].flat().sort();
 return {v,agent,flush,drain,names,setMap:x=>map=x,setResolver:x=>resolver=x,get changes(){return changes}};
}
test('tools/change converges and retains live layers',async()=>{const f=fixture();const a=f.agent(['carrier']);await f.drain();const count=f.changes;await f.v.applyToolRestrictions();await f.drain();assert.equal(f.changes,count);assert.deepEqual(f.names(a),['carrier'])});
test('deny changes, clears, and differs between agents',async()=>{const f=fixture();const a=f.agent(['a']),b=f.agent(['b']);await f.drain();f.setMap({s:['x']});await f.v.applyToolRestrictions();await f.drain();assert.deepEqual(f.names(a),['a','mcp__s__x']);assert.deepEqual(f.names(b),['b','mcp__s__x']);f.setMap({s:['y']});await f.v.applyToolRestrictions();await f.drain();assert.deepEqual(f.names(a),['a','mcp__s__y']);f.setMap({});a.hidden=[];await f.v.applyToolRestrictions();await f.drain();assert.deepEqual(f.names(a),[]);assert.deepEqual(f.names(b),['b'])});
for(const action of ['replay','detach','dispose'])test('stale carrier result after '+action,async()=>{const f=fixture();const a=f.agent(['initial']);await f.flush();let resolve;f.setResolver(()=>new Promise(r=>resolve=r));await f.v.applyToolRestrictions();await f.flush();const old=resolve;if(action==='replay'){f.setResolver(async()=>['new']);await f.v.applyToolRestrictions();await f.flush()}else if(action==='detach')f.v.detachAgent(a);else f.v.disposeVisibility();old(['stale']);await f.flush();assert.deepEqual(f.names(a),action==='replay'?['new']:[]);f.v.disposeVisibility()});

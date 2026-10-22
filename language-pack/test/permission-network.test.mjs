import {test} from 'node:test';
import assert from 'node:assert/strict';
import * as browser from '../scripts/verify-browser.mjs';
test('policy read reply must match its fresh request correlation id',()=>{
 assert.equal(typeof browser.assertPermissionReadReply,'function');
 assert.doesNotThrow(()=>browser.assertPermissionReadReply({rpcId:'fresh-id',result:{ok:true,value:{}}},'fresh-id'));
 assert.throws(()=>browser.assertPermissionReadReply({rpcId:'old-id',result:{ok:true,value:{}}},'fresh-id'),/correlation/);
 assert.throws(()=>browser.assertPermissionReadReply({rpcId:'fresh-id',result:{ok:false}},'fresh-id'),/rejected/);
});
test('policy read replay is confined to the exact owned origin and official read methods',()=>{
 assert.equal(typeof browser.assertPermissionReadReplay,'function');
 const address='http://127.0.0.1:41234/?token=owned';
 const good={url:'http://127.0.0.1:41234/api/settings/describe',request:{method:'settings/describe'}};
 assert.doesNotThrow(()=>browser.assertPermissionReadReplay(address,good));
 for(const url of ['https://example.com/api/settings/describe','http://127.0.0.1:3080/api/settings/describe','http://127.0.0.1:41235/api/settings/describe'])assert.throws(()=>browser.assertPermissionReadReplay(address,{...good,url}));
 assert.throws(()=>browser.assertPermissionReadReplay(address,{...good,request:{method:'settings/mutate'}}));
});
test('policy snapshot selects authoritative permission values and catalog defaults only',()=>{
 assert.equal(typeof browser.permissionStateOf,'function');
 const describe={namespaces:[{ns:'theme',value:{mode:'dark'}},{ns:'permission',value:{defaultPreset:'workspace-write'}}]};
 assert.deepEqual(browser.permissionStateOf(describe,{defaultPreset:'read-only'}),{permission:{defaultPreset:'workspace-write'},catalogDefault:'read-only',effectiveDefault:'workspace-write'});
 assert.throws(()=>browser.permissionStateOf({namespaces:[]},{defaultPreset:'read-only'}),/permission namespace/);
});
test('fixture identifies permission writes and approval/task commands without blocking safe reads',()=>{
 assert.equal(typeof browser.isForbiddenPermissionRequest,'function');
 for(const method of ['settings/mutate','settings/update','settings/replace'])assert.equal(browser.isForbiddenPermissionRequest({method,payload:{ns:'permission'}}),true);
 for(const method of ['approval/answer','session/prompt','agent/command'])assert.equal(browser.isForbiddenPermissionRequest({method,payload:{}}),true);
 for(const request of [{method:'settings/describe',payload:{}},{method:'permissionPresets/catalog',payload:{}},{method:'settings/mutate',payload:{ns:'theme'}}])assert.equal(browser.isForbiddenPermissionRequest(request),false);
});

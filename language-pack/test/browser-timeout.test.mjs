import {test}from'node:test';
import assert from'node:assert/strict';
import{startOwned}from'../scripts/owned-process.mjs';
import*as browser from'../scripts/verify-browser.mjs';
test('browser discovery and socket-open work are bounded and race owned-browser death',async()=>{
 assert.equal(typeof browser.boundedBrowserStep,'function','bounded browser startup step missing');
 let child=await startOwned(process.execPath,['-e','setInterval(()=>{},1000)'],{env:{PATH:process.env.PATH}});
 try{await assert.rejects(browser.boundedBrowserStep(new Promise(()=>{}),child,{timeoutMs:50,label:'socket open'}),/socket open timed out/);}finally{await child.stop();}
 child=await startOwned(process.execPath,['-e','process.exit(0)'],{env:{PATH:process.env.PATH}});
 try{await assert.rejects(browser.boundedBrowserStep(new Promise(()=>{}),child,{timeoutMs:1000,label:'discovery'}),/browser exited during discovery/);}finally{await child.stop();}
});

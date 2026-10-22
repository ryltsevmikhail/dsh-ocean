import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import { startOwned } from './owned-process.mjs';
import { basename, join } from 'node:path';
import { writeFileSync } from 'node:fs';
import { languages } from '../src/register.mjs';

export function assertBrowserFixture(address, fixture) {
 const url = new URL(address);
 assert.equal(url.protocol,'http:'); assert.equal(url.hostname,'127.0.0.1');
 assert.ok(url.port && url.port !== '3080'); assert.equal(url.pathname,'/');
 assert.ok(fixture.startsWith('/') && /^language-pack-fixture-[\w-]+$/.test(basename(fixture)),'unsafe browser fixture root');
}

export async function boundedBrowserStep(work,owner,{timeoutMs=10000,label='startup step'}={}) {
 let timer;
 try {
  return await Promise.race([work,owner.wait().then(()=>{throw Error(`browser exited during ${label}`);}),new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error(`${label} timed out`)),timeoutMs);})]);
 } finally {clearTimeout(timer);}
}

// Long labels may intentionally ellipsize, but must retain a readable prefix.
// Short native labels need only fit themselves; DOM integer scrollWidth rounds.
export function isPresetTitleReadable(title) {
 return !!title && typeof title.text==='string' && title.text.trim().length>0 && Number.isFinite(title.width) && Number.isFinite(title.scrollWidth) && title.width>0 && title.width>=Math.min(60,title.scrollWidth)-1;
}

// Flush layout, await finite UI transitions, then capture the fully painted state.
export async function waitForVisualStability(cdp) {
 return cdp.evaluate(`(async()=>{
  const frames=()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
  let timer;
  try {
   await Promise.race([(async()=>{
    await frames();
    const animations=document.getAnimations().filter(a=>Number.isFinite(a.effect?.getTiming().iterations ?? 1) && a.playState!=='finished');
    await Promise.all(animations.map(a=>a.finished.catch(()=>{})));
    await frames();
   })(),new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error('UI animations did not settle')),5000);})]);
   return true;
  } finally {clearTimeout(timer);}
 })()`);
}

export function assertPermissionReadReply(response,rpcId) {
 assert.equal(response.rpcId,rpcId,'policy read correlation mismatch');
 assert.ok(response.result,'policy read has no RPC result');
 assert.notEqual(response.result.ok,false,'policy read rejected');
}

export function assertPermissionReadReplay(address,{url,request}) {
 const fixture=new URL(address),target=new URL(url);
 assert.equal(fixture.protocol,'http:');assert.equal(fixture.hostname,'127.0.0.1');assert.ok(fixture.port && fixture.port!=='3080');
 assert.equal(target.origin,fixture.origin,'policy read outside owned fixture origin');
 assert.ok(['settings/describe','permissionPresets/catalog'].includes(request?.method),'not an official policy read method');
 assert.equal(target.pathname,`/api/${request.method}`,'policy read endpoint mismatch');
}

export function permissionStateOf(describe,catalog) {
 const permission=describe.namespaces?.find(n=>n.ns==='permission');
 assert.ok(permission,'authoritative permission namespace missing');
 assert.equal(typeof catalog.defaultPreset,'string');
 return {permission:permission.value,catalogDefault:catalog.defaultPreset,effectiveDefault:permission.value.defaultPreset??catalog.defaultPreset};
}

export function isForbiddenPermissionRequest(request) {
 const method=request?.method||'';
 return /^(?:approval\/|session\/(?:prompt|command)|agent\/(?:prompt|command))/.test(method) || (/^settings\/(?:mutate|replace|update)$/.test(method) && (request.payload?.ns==='permission' || request.payload?.[0]==='permission'));
}

function socketOpen(socket) {
 return new Promise((resolve,reject)=>{
  const cleanup=()=>{socket.removeEventListener('open',opened);socket.removeEventListener('error',failed);socket.removeEventListener('close',closed);};
  const opened=()=>{cleanup();resolve();};
  const failed=()=>{cleanup();reject(Error('browser websocket failed before open'));};
  const closed=()=>{cleanup();reject(Error('browser websocket closed before open'));};
  socket.addEventListener('open',opened,{once:true});socket.addEventListener('error',failed,{once:true});socket.addEventListener('close',closed,{once:true});
 });
}

class CDP {
 constructor(socket) {
  this.socket=socket; this.next=0; this.pending=new Map(); this.errors=[]; this.permissionRequests=[]; this.safeReads=new Map();
  const disconnected=()=>{for(const pending of this.pending.values()){clearTimeout(pending.timer);pending.reject(Error('browser CDP socket disconnected'));}this.pending.clear();};
  socket.addEventListener('close',disconnected);socket.addEventListener('error',disconnected);
  socket.addEventListener('message',({data})=>{
   const result=JSON.parse(data);
   if(result.method==='Network.requestWillBeSent' && result.params.request.postData){
     try{const request=JSON.parse(result.params.request.postData);if(/^(settings\/describe|permissionPresets\/catalog|permission-presets\/catalog)$/.test(request.method))this.safeReads.set(request.method,{url:result.params.request.url,request});if(isForbiddenPermissionRequest(request))this.permissionRequests.push({method:request.method,payload:request.payload});}catch{}
    }
    if(result.method==='Runtime.exceptionThrown') this.errors.push(result.params.exceptionDetails.text);
   if(!result.id)return;
   const pending=this.pending.get(result.id); if(!pending)return;
   this.pending.delete(result.id); clearTimeout(pending.timer);
   if(result.error)pending.reject(Error(JSON.stringify(result.error))); else pending.resolve(result.result);
  });
 }
 async send(method,params={}) {
  if(method==='Page.captureScreenshot')await waitForVisualStability(this);
   const id=++this.next;
  return new Promise((resolve,reject)=>{
   const timer=setTimeout(()=>{this.pending.delete(id);reject(Error(`CDP timeout: ${method}`));},20000);
   this.pending.set(id,{resolve,reject,timer}); this.socket.send(JSON.stringify({id,method,params}));
  });
 }
 async evaluate(expression) {
  const result=await this.send('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true}).catch(error=>{throw Error(`Browser evaluation failed: ${expression}`,{cause:error});});
  if(result.exceptionDetails)throw Error(JSON.stringify(result.exceptionDetails));
  return result.result.value;
 }
 async wait(expression) {
  const deadline=Date.now()+30000;
  do {
   // Predicates may end in querySelector(); never serialize a React-owned DOM node.
   if(await this.evaluate(`Boolean(${expression})`))return;
   await new Promise(resolve=>setTimeout(resolve,100));
  } while(Date.now()<deadline);
  throw Error(`Browser condition timed out: ${expression}; visible: ${await this.evaluate('document.body.innerText')}`);
 }
}

async function verifySettingsFeature(cdp,fixture,id,expected) {
 const theme=expected['settings.theme'],shortcuts=expected.shortcuts;
 const originalBindings=await cdp.evaluate(`localStorage.getItem('dsh.keybindings.v1')`);
 const button=text=>`Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()===${JSON.stringify(text)})`;
 const aria=value=>'[aria-label='+JSON.stringify(value)+']';
 for(const key of ['appearance.title','fontSize.title','fontSize.description'])await cdp.wait(`document.body?.innerText.includes(${JSON.stringify(theme[key])})`);
 const originalTheme=await cdp.evaluate(`(()=>{const labels=${JSON.stringify(['appearance.light','appearance.dark','appearance.system'].map(k=>theme[k]))};return Array.from(document.querySelectorAll('button[aria-pressed="true"]')).find(b=>labels.includes(b.textContent.trim()))?.textContent.trim();})()`);
 assert.ok(originalTheme,'selected fixture theme missing');
 for(const key of ['appearance.light','appearance.dark','appearance.system']){
  await cdp.evaluate(`${button(theme[key])}.click()`);
  await cdp.wait(`${button(theme[key])}?.getAttribute('aria-pressed')==='true'`);
 }
 await cdp.evaluate(`${button(originalTheme)}.click()`);
 await cdp.wait(`${button(originalTheme)}?.getAttribute('aria-pressed')==='true'`);
 const increase='button'+aria(theme['fontSize.increase']),decrease='button'+aria(theme['fontSize.decrease']);
 const fontValue=`document.querySelector(${JSON.stringify(increase)})?.parentElement.previousElementSibling.textContent`;
 await cdp.wait(`document.querySelector(${JSON.stringify(increase)}) && document.querySelector(${JSON.stringify(decrease)})`);
 const originalFont=Number(await cdp.evaluate(fontValue));assert.ok(originalFont>=10&&originalFont<=22);
 const first=originalFont<22?increase:decrease,back=originalFont<22?decrease:increase;
 await cdp.evaluate(`document.querySelector(${JSON.stringify(first)}).click()`);
 await cdp.wait(`Number(${fontValue})===${originalFont+(originalFont<22?1:-1)}`);
 await cdp.evaluate(`document.querySelector(${JSON.stringify(back)}).click()`);
 await cdp.wait(`Number(${fontValue})===${originalFont}`);
 for(const key of ['settings','description'])await cdp.wait(`document.body?.innerText.includes(${JSON.stringify(shortcuts[key])})`);
 await cdp.evaluate(`document.querySelector(${JSON.stringify('button'+aria(shortcuts.view))}).click()`);
 await cdp.wait(`document.querySelector(${JSON.stringify('input'+aria(shortcuts.search))})`);
 for(const key of ['title','input','menus','move','select','dismiss','reset-all'])await cdp.wait(`document.body?.innerText.includes(${JSON.stringify(shortcuts[key])})`);
 await cdp.wait(`document.querySelector(${JSON.stringify('button'+aria(shortcuts.close))})`);
 await cdp.wait(`document.querySelector(${JSON.stringify('[aria-label='+JSON.stringify(shortcuts.application)+']')})`);
 const editLabel=shortcuts['edit-label'].replace('{command}',shortcuts.open);
 await cdp.wait(`document.querySelector(${JSON.stringify('button'+aria(editLabel))})`);
 await cdp.evaluate(`document.querySelector(${JSON.stringify('button'+aria(editLabel))}).click()`);
 await cdp.wait(`document.querySelector('[data-shortcut-modal="shortcut-edit"]') && document.querySelector(${JSON.stringify('button'+aria(shortcuts.record))})`);
 const commandTitle=await cdp.evaluate(`(()=>{const editor=document.querySelector('[data-shortcut-modal="shortcut-edit"]');const element=Array.from(editor.closest('li').querySelectorAll('span')).find(e=>e.textContent===${JSON.stringify(shortcuts.open)});return element?{width:element.getBoundingClientRect().width,scrollWidth:element.scrollWidth,text:element.textContent}:null;})()`);
 assert.ok(isPresetTitleReadable(commandTitle),`${id}: inline shortcut command label is unreadable (${JSON.stringify(commandTitle)})`);
 if(id==='ja')assert.ok(commandTitle.width>=commandTitle.scrollWidth-1,`ja: inline command must expose its action verb (${JSON.stringify(commandTitle)})`);
 const editor=await cdp.evaluate(`document.querySelector('[data-shortcut-modal="shortcut-edit"]').innerText`);
 assert.ok(editor.includes(shortcuts.reset));assert.ok(editor.includes(shortcuts.clear));
 writeFileSync(join(fixture,`browser-${id}-shortcut-editor.png`),Buffer.from((await cdp.send('Page.captureScreenshot',{format:'png'})).data,'base64'));
 // The owner replaces the edit button while open; its title pointer handler closes the inline editor.
 await cdp.evaluate(`Array.from(document.querySelectorAll('h2')).find(e=>e.textContent===${JSON.stringify(shortcuts.title)}).dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}))`);
 await cdp.wait(`!document.querySelector('[data-shortcut-modal="shortcut-edit"]')`);
 const nonmatchingEdit=shortcuts['edit-label'].replace('{command}',expected.sidebar['session.new']);
 await cdp.wait(`document.querySelector(${JSON.stringify('button'+aria(nonmatchingEdit))})`);
 const search='input'+aria(shortcuts.search);
 await cdp.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(search)});Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,${JSON.stringify(shortcuts.open)});e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
 await cdp.wait(`document.querySelector(${JSON.stringify('button'+aria(shortcuts['clear-search']))})`);
 await cdp.wait(`document.querySelector(${JSON.stringify('button'+aria(editLabel))}) && !document.querySelector(${JSON.stringify('button'+aria(nonmatchingEdit))})`);
 await cdp.evaluate(`document.querySelector(${JSON.stringify('button'+aria(shortcuts['clear-search']))}).click()`);
 await cdp.wait(`document.querySelector(${JSON.stringify(search)})?.value===''`);
 // With unchanged bindings the owner deliberately disables reset-all. Do not manufacture changes just to enable it.
 assert.equal(await cdp.evaluate(`${button(shortcuts['reset-all'])}?.disabled`),true,'unmodified fixture reset-all should be disabled');
 writeFileSync(join(fixture,`browser-${id}-shortcuts.png`),Buffer.from((await cdp.send('Page.captureScreenshot',{format:'png'})).data,'base64'));
 await cdp.evaluate(`document.querySelector(${JSON.stringify('button'+aria(shortcuts.close))}).click()`);
 await cdp.wait(`!document.querySelector(${JSON.stringify(search)})`);
 assert.equal(await cdp.evaluate(`localStorage.getItem('dsh.keybindings.v1')`),originalBindings,'fixture shortcut bindings changed');
 const result={appearanceOptions:true,themeRestored:originalTheme,fontSizeLabelsAndHint:true,fontSizeRestored:originalFont,shortcutsRow:true,editor:true,searchAndClear:true,actionAccessibleLabels:true,resetAllDisabledWithoutChanges:true,shortcutBindingsUnchanged:true};
 writeFileSync(join(fixture,`browser-${id}-settings-feature.json`),JSON.stringify(result,null,2)+'\n');return result;
}

async function readPermissionState(cdp) {
 const describe=cdp.safeReads.get('settings/describe');
 const catalog=cdp.safeReads.get('permissionPresets/catalog');
 assert.ok(describe&&catalog,`missing observed safe read endpoints: ${[...cdp.safeReads.keys()]}`);
 const read=async ({url,request})=>{
  assertPermissionReadReplay(cdp.fixtureAddress,{url,request});
  const rpcId=randomUUID();
  const response=await cdp.evaluate(`(async()=>{const r=await fetch(${JSON.stringify(url)},{method:'POST',redirect:'error',headers:{'content-type':'application/json'},body:${JSON.stringify(JSON.stringify({...request,rpcId}))}});if(!r.ok)throw Error('policy read failed');return r.json();})()`);
  assertPermissionReadReply(response,rpcId);
  return response.result.value??response.result;
 };
 return permissionStateOf(await read(describe),await read(catalog));
}

/** Read only the actual General permission menu. Never select any mode or confirmation action. */
export async function verifyPermissionMenu(cdp,fixture,id,expected,initial) {
 const copy=expected['settings.permission'];
 const row=`Array.from(document.querySelectorAll('div')).find(e=>e.textContent===${JSON.stringify(copy.title)})?.parentElement?.parentElement`;
 await cdp.wait(`${row}?.querySelector('button[aria-haspopup="menu"]:not(:disabled)')`);
 const current=await cdp.evaluate(`${row}.querySelector('button').textContent.trim()`);
 const policyBefore=await readPermissionState(cdp);
 if(initial){assert.equal(current,initial.defaultLabel,`${id}: permission label changed after reload`);assert.deepEqual(policyBefore,initial.policyBefore,`${id}: authoritative permission state changed after reload`);}
 await cdp.evaluate(`${row}.querySelector('button').click()`);
 await cdp.wait(`document.querySelector('[role="menu"]')`);
 const items=await cdp.evaluate(`Array.from(document.querySelectorAll('[role="menuitemradio"],[role="menuitem"]')).map(e=>({text:e.textContent.trim(),width:e.getBoundingClientRect().width,scrollWidth:e.scrollWidth}))`);
 for(const key of ['preset.readOnly','preset.workspaceWrite','preset.fullAccess']){
  const item=items.find(e=>e.text===copy[key]);assert.ok(item,`${id}: missing permission mode ${key}`);
  assert.ok(item.width>=item.scrollWidth-1,`${id}: permission name clipped: ${JSON.stringify(item)}`);
 }
 writeFileSync(join(fixture,`browser-${id}-permissions.png`),Buffer.from((await cdp.send('Page.captureScreenshot',{format:'png'})).data,'base64'));
 // Escape here closes only the menu (there is no approval or risk dialog).
 assert.equal(await cdp.evaluate(`!!document.querySelector('[data-approval-key],[role="alertdialog"]')`),false);
 await cdp.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
 await cdp.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
 await cdp.wait(`!document.querySelector('[role="menu"]')`);
 assert.equal(await cdp.evaluate(`${row}.querySelector('button').textContent.trim()`),current);
 const policyAfter=await readPermissionState(cdp);assert.deepEqual(policyAfter,policyBefore,`${id}: authoritative permission state changed after menu inspection`);
 const result={defaultLabel:current,policyBefore,policyAfter,authoritativePolicyUnchanged:true,items,openedReadOnly:true,selectedNoOptions:true,unchangedAfterClose:true,confirmationNotOpened:true,approvalStatesNotExercised:true};
 writeFileSync(join(fixture,`browser-${id}-permissions.json`),JSON.stringify(result,null,2)+'\n');return result;
}

/** Exercise Settings → General → Language in a separate, temporary Chromium profile. */
export async function verifyBrowser(address, fixture, env, catalogs) {
 assertBrowserFixture(address,fixture);
 let log='', socket, browserSocket, control, contextId, cdp, browser, timeout, readyResolve, readyReject;
 const ready=new Promise((resolve,reject)=>{readyResolve=resolve;readyReject=reject;});
 ready.catch(()=>{});
 const results=[];
 try {
  timeout=setTimeout(()=>readyReject(Error('Chromium startup timed out')),20000);
  browser=await startOwned(process.env.CHROMIUM || '/usr/bin/chromium',[
   '--headless=new','--disable-gpu','--disable-background-networking','--disable-sync',
   '--disable-extensions','--no-first-run','--no-default-browser-check','--password-store=basic',
   '--lang=en-US','--window-size=1440,1000','--remote-debugging-address=127.0.0.1','--remote-debugging-port=0',
   `--user-data-dir=${join(fixture,'chromium-profile')}`,'about:blank',
  ],{env,onOutput(chunk){log+=chunk;const match=log.match(/DevTools listening on (ws:\/\/127\.0\.0\.1:\d+\/devtools\/browser\/[^\s]+)/);if(match){clearTimeout(timeout);readyResolve(match[1]);}}});
  browser.wait().then(()=>readyReject(Error('Chromium exited before readiness')));
  const endpoint=await ready;
  const debug=new URL(endpoint);
  browserSocket=new WebSocket(endpoint);await boundedBrowserStep(socketOpen(browserSocket),browser,{label:'browser socket open'});control=new CDP(browserSocket);
  ({browserContextId:contextId}=await control.send('Target.createBrowserContext'));
  const {targetId}=await control.send('Target.createTarget',{url:'about:blank',browserContextId:contextId});
  const targets=await boundedBrowserStep(fetch(`http://${debug.host}/json/list`,{signal:AbortSignal.timeout(10000)}).then(response=>response.json()),browser,{label:'target discovery'});
  const target=targets.find(item=>item.id===targetId);assert.ok(target?.webSocketDebuggerUrl);
  socket=new WebSocket(target.webSocketDebuggerUrl);await boundedBrowserStep(socketOpen(socket),browser,{label:'page socket open'});cdp=new CDP(socket);cdp.fixtureAddress=address;
  await cdp.send('Runtime.enable');await cdp.send('Page.enable');await cdp.send('Network.enable');
  // Linux Chromium can inherit OS Accept-Language despite --lang. Configure
   // only this disposable browser before exercising explicit UI locale choices.
   await cdp.send('Network.setUserAgentOverride',{userAgent:await cdp.evaluate('navigator.userAgent'),acceptLanguage:'en-US,en;q=0.9'});
   await cdp.send('Page.navigate',{url:address});
   await cdp.wait(`navigator.language==='en-US'`);
  // A fresh official Web profile shows its preview notice, then optional provider setup.
  // Acknowledge only in this owned fixture; no credential is entered or provider called.
  await cdp.wait(`Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim()==='Continue')`);
  await cdp.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==='Continue').click()`);
  await cdp.wait(`Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim()==='Configure later')`);
  await cdp.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==='Configure later').click()`);
  await cdp.wait(`Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim()==='Settings')`);
  const graphEntry=await cdp.evaluate(`window.__DSH_BOOT__.entries.find(entry=>entry.id==='@dsh-ocean/desktop-language-pack')`);
  assert.ok(graphEntry?.url,'pack missing from parsed browser boot manifest');
  assert.deepEqual(graphEntry.inject,['@deepseek-ai/dsh-client-locale']);
  assert.equal(graphEntry.immediately,true);
  const initial=await cdp.evaluate(`({lang:document.documentElement.lang,buttons:Array.from(document.querySelectorAll('button')).map(b=>({text:b.textContent.trim(),aria:b.getAttribute('aria-label')}))})`);
  writeFileSync(join(fixture,'browser-initial.json'),JSON.stringify(initial,null,2)+'\n');
  await cdp.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==='Settings').click()`);
  await cdp.wait(`document.body?.innerText.includes('Language')`);
  // This changes only visibility in the disposable profile, never permissions.
  const codingView=await cdp.evaluate(`document.querySelector('[role="switch"][aria-label="Show coding view"]')?.getAttribute('aria-checked')`);
  assert.ok(codingView==='true'||codingView==='false','coding view toggle missing');
  if(codingView==='false')await cdp.evaluate(`document.querySelector('[role="switch"][aria-label="Show coding view"]').click()`);
  await cdp.wait(`document.querySelector('[role="switch"][aria-label="Show coding view"]')?.getAttribute('aria-checked')==='true'`);
  let active='English';
  for(const language of languages) {
   // Self-described language labels remain stable when the surrounding UI changes.
   await cdp.wait(`Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim()===${JSON.stringify(active)})`);
   await cdp.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()===${JSON.stringify(active)}).click()`);
   await cdp.wait(`Array.from(document.querySelectorAll('[role="menuitem"],[role="menuitemradio"],[role="option"],button')).some(b=>b.textContent.trim()===${JSON.stringify(language.label)})`);
   await cdp.evaluate(`Array.from(document.querySelectorAll('[role="menuitem"],[role="menuitemradio"],[role="option"],button')).find(b=>b.textContent.trim()===${JSON.stringify(language.label)}).click()`);
   await cdp.wait(`document.documentElement?.lang===${JSON.stringify(language.id)}`);
   const expected=catalogs[language.id];
   await cdp.wait(`document.body?.innerText.includes(${JSON.stringify(expected['settings.locale']['language.title'])}) && document.body?.innerText.includes(${JSON.stringify(expected.settings.trigger)})`);
   const settingsFeature=await verifySettingsFeature(cdp,fixture,language.id,expected);
    const permissionFeature=await verifyPermissionMenu(cdp,fixture,language.id,expected);
    const state=await cdp.evaluate(`({lang:document.documentElement.lang,text:document.body.innerText,placeholders:Array.from(document.querySelectorAll('textarea,input,[contenteditable]')).map(e=>e.getAttribute('placeholder')||e.getAttribute('data-placeholder'))})`);
   assert.ok(state.text.includes(language.label));
   writeFileSync(join(fixture,`browser-${language.id}.json`),JSON.stringify(state,null,2)+'\n');
   const screenshot=await cdp.send('Page.captureScreenshot',{format:'png'});
   writeFileSync(join(fixture,`browser-${language.id}.png`),Buffer.from(screenshot.data,'base64'));
   await cdp.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()===${JSON.stringify(expected['settings.models'].nav)}).click()`);
   await cdp.wait(`document.body?.innerText.includes(${JSON.stringify(expected['settings.models'].intro)}) && Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim()===${JSON.stringify(expected['settings.models'].add)})`);
   const modelsScreenshot=await cdp.send('Page.captureScreenshot',{format:'png'});
   writeFileSync(join(fixture,`browser-${language.id}-models.png`),Buffer.from(modelsScreenshot.data,'base64'));
   const preset=expected['settings.agentPreset'];
   await cdp.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()===${JSON.stringify(preset.nav)}).click()`);
   await cdp.wait(`document.body?.innerText.includes(${JSON.stringify(preset.sectionIntro)}) && document.querySelector('[data-agent-preset-id="standard"]')`);
   const cards=await cdp.evaluate(`Array.from(document.querySelectorAll('[data-agent-preset-id]')).map(e=>({id:e.getAttribute('data-agent-preset-id'),text:e.innerText}))`);
   const modeKeys={standard:'presetStandard',ptc:'presetPtc',minimal:'presetMinimal',cordis:'presetCordis'};
   for(const card of cards)if(modeKeys[card.id]){
    assert.ok(card.text.includes(preset[modeKeys[card.id]+'Name']),`${language.id}/${card.id}: preset name`);
    assert.ok(card.text.includes(preset[modeKeys[card.id]+'Description']),`${language.id}/${card.id}: preset description`);
   }
   assert.ok(cards.some(card=>card.id==='standard'));
    const standardTitle=await cdp.evaluate(`(()=>{const element=Array.from(document.querySelector('[data-agent-preset-id="standard"]').querySelectorAll('span[title]')).find(e=>e.title===${JSON.stringify(preset.presetStandardName)});return element?{width:element.getBoundingClientRect().width,scrollWidth:element.scrollWidth,text:element.textContent}:null;})()`);
    assert.ok(isPresetTitleReadable(standardTitle),`${language.id}: default preset title is hidden/truncated beyond readability (${JSON.stringify(standardTitle)})`);
   writeFileSync(join(fixture,`browser-${language.id}-presets.png`),Buffer.from((await cdp.send('Page.captureScreenshot',{format:'png'})).data,'base64'));
   await cdp.evaluate(`Array.from(document.querySelector('[data-agent-preset-id="standard"]').querySelectorAll('button')).find(b=>b.getAttribute('aria-label')===${JSON.stringify(preset.modeExplanation+': '+preset.presetStandardName)}).click()`);
   await cdp.wait(`document.body?.innerText.includes(${JSON.stringify(preset.guideStandardIntro)}) && document.querySelector('[data-guide-page="explanation"]:not([hidden])')`);
   const explanation=await cdp.evaluate(`document.querySelector('[data-guide-page="explanation"]').innerText`);
   assert.ok(explanation.includes(preset.guideStandardExplanation.split('\n\n')[1]));
   await cdp.evaluate(`Array.from(document.querySelectorAll('[role="tab"]')).find(b=>b.textContent.trim()===${JSON.stringify(preset.howToUse)}).click()`);
   await cdp.wait(`document.querySelector('[data-guide-page="usage"]:not([hidden])')`);
   const usage=await cdp.evaluate(`document.querySelector('[data-guide-page="usage"]').innerText`);
   assert.ok(usage.includes(preset.guideStandardUsage.split('\n\n')[1].replace(/^> /,'')));
   writeFileSync(join(fixture,`browser-${language.id}-guide.png`),Buffer.from((await cdp.send('Page.captureScreenshot',{format:'png'})).data,'base64'));
   await cdp.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.getAttribute('aria-label')===${JSON.stringify(preset.close)}).click()`);
   await cdp.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()===${JSON.stringify(expected.settings['general.nav'])}).click()`);
   await cdp.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
   await cdp.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
   const composerCopy=['placeholder.workspace','placeholder.hero','placeholder.default','placeholder.unavailable'].map(key=>expected.conversation[key]);
   await cdp.wait(`Array.from(document.querySelectorAll('[data-placeholder]')).some(e=>${JSON.stringify(composerCopy)}.includes(e.getAttribute('data-placeholder')))`);
   const sidebar=expected.sidebar;
   const ariaSelector=value=>'[aria-label='+JSON.stringify(value)+']';
   await cdp.wait(`document.querySelector(${JSON.stringify(ariaSelector(sidebar['session.new.label']))}) && document.body?.innerText.includes(${JSON.stringify(sidebar['session.new'])})`);
   await cdp.wait(`document.querySelector(${JSON.stringify(ariaSelector(sidebar['panels.label']))})`);
   await cdp.evaluate(`document.querySelector(${JSON.stringify('button'+ariaSelector(sidebar['toggle.collapse']))}).click()`);
   await cdp.wait(`document.querySelector(${JSON.stringify('button'+ariaSelector(sidebar['toggle.open']))})`);
   writeFileSync(join(fixture,`browser-${language.id}-sidebar-collapsed.png`),Buffer.from((await cdp.send('Page.captureScreenshot',{format:'png'})).data,'base64'));
   await cdp.evaluate(`document.querySelector(${JSON.stringify('button'+ariaSelector(sidebar['toggle.open']))}).click()`);
   await cdp.wait(`document.querySelector(${JSON.stringify('button'+ariaSelector(sidebar['toggle.collapse']))}) && document.body?.innerText.includes(${JSON.stringify(sidebar['session.new'])})`);
   await cdp.wait(`Array.from(document.querySelectorAll('button')).some(b=>b.title===${JSON.stringify(preset.seatHint)} && b.textContent.includes(${JSON.stringify(preset.presetStandardName)}))`);
   await cdp.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.title===${JSON.stringify(preset.seatHint)}).click()`);
   await cdp.wait(`document.body?.innerText.includes(${JSON.stringify(preset.presetStandardDescription)})`);
   const picker=await cdp.evaluate(`({text:document.body.innerText,items:Array.from(document.querySelectorAll('[role="menuitem"],[role="menuitemradio"]')).map(e=>e.innerText)})`);
   assert.ok(picker.items.length>0,'mode picker options missing');
   for(const card of cards)if(modeKeys[card.id]){
    assert.ok(picker.items.some(text=>text.includes(preset[modeKeys[card.id]+'Name']) && text.includes(preset[modeKeys[card.id]+'Description'])),`${language.id}/${card.id}: picker name and description`);
   }
   writeFileSync(join(fixture,`browser-${language.id}-picker.png`),Buffer.from((await cdp.send('Page.captureScreenshot',{format:'png'})).data,'base64'));
   // Close the picker only; never stage another preset or start a task.
   await cdp.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
   await cdp.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
   const chatScreenshot=await cdp.send('Page.captureScreenshot',{format:'png'});
   writeFileSync(join(fixture,`browser-${language.id}-chat.png`),Buffer.from(chatScreenshot.data,'base64'));
   cdp.safeReads.clear();
    await cdp.send('Page.reload',{ignoreCache:true});
   await cdp.wait(`document.documentElement?.lang===${JSON.stringify(language.id)} && document.body?.innerText.includes(${JSON.stringify(sidebar['session.new'])}) && Array.from(document.querySelectorAll('button')).some(b=>b.title===${JSON.stringify(preset.seatHint)} && b.textContent.includes(${JSON.stringify(preset.presetStandardName)}))`);
   const everyday={sidebar,renderedCards:cards,picker,persistedAfterReload:true};
   writeFileSync(join(fixture,`browser-${language.id}-everyday.json`),JSON.stringify(everyday,null,2)+'\n');
   await cdp.evaluate(`Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()===${JSON.stringify(expected.settings.trigger)}).click()`);
   await cdp.wait(`Array.from(document.querySelectorAll('button[aria-pressed="true"]')).some(b=>b.textContent.trim()===${JSON.stringify(settingsFeature.themeRestored)})`);
    await cdp.wait(`Number(document.querySelector(${JSON.stringify('[aria-label='+JSON.stringify(expected['settings.theme']['fontSize.increase'])+']')})?.parentElement.previousElementSibling.textContent)===${settingsFeature.fontSizeRestored}`);
    settingsFeature.restoredPreferencesPersistedAfterReload=true;
     await verifyPermissionMenu(cdp,fixture,language.id,expected,permissionFeature);
     permissionFeature.unchangedAfterReload=true;
    results.push({id:language.id,htmlLang:state.lang,languageRow:true,settingsCopy:true,settingsFeature,permissionFeature,modelProviderControls:true,chatComposer:true,newSessionTextAndAria:true,globalPanelsAria:true,sidebarCollapseExpand:true,modePicker:true,agentPresetSettings:true,standardGuideDetailsAndUsage:true,persistedAfterReload:true,availablePresetIds:cards.map(card=>card.id)});
   active=language.label;
  }
  if(codingView==='false'){
   const label=catalogs.ko.settings['developerTools.title'];
   const selector='[role="switch"][aria-label='+JSON.stringify(label)+']';
   await cdp.wait(`document.querySelector(${JSON.stringify(selector)})`);
   await cdp.evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
   await cdp.wait(`document.querySelector(${JSON.stringify(selector)})?.getAttribute('aria-checked')==='false'`);
  }
  cdp.safeReads.clear();
    await cdp.send('Page.reload',{ignoreCache:true});
  await cdp.wait(`document.documentElement?.lang==='ko' && document.body?.innerText.includes(${JSON.stringify(catalogs.ko.settings.trigger)})`);
  assert.deepEqual(cdp.errors,[],'uncaught browser exceptions');
   assert.deepEqual(cdp.permissionRequests,[],'fixture issued a permission write, approval answer or task command');
   writeFileSync(join(fixture,'permission-request-audit.json'),JSON.stringify({forbiddenRequests:cdp.permissionRequests,scope:'Captured HTTP JSON RPC permission writes, approval methods, task prompts/commands; no interception or policy patching.'},null,2)+'\n');
  return {languages:results,graphEntry,persistedAfterReload:true,uncaughtExceptions:cdp.errors,browserProfile:join(fixture,'chromium-profile')};
 } finally {
  clearTimeout(timeout);
  if(control && contextId)await control.send('Target.disposeBrowserContext',{browserContextId:contextId}).catch(()=>{});
  try{socket?.close();}catch{};try{browserSocket?.close();}catch{};
  if(browser)assert.equal((await browser.stop()).stopped,true,'browser process group not confirmed stopped');
  writeFileSync(join(fixture,'chromium.log'),log.replace(/([?&]token=)[^\s&]+/g,'$1<redacted>'));
 }
}

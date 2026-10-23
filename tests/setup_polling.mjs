// Run with node --test tests/setup_polling.mjs; no GUI or dependencies.
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import assert from 'node:assert/strict';

const html = readFileSync(new URL('../ui/index.html', import.meta.url), 'utf8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
function fixture(hidden = false, locale = 'en') {
  const timers = new Map(), elements = new Map(), listeners = new Map();
  const calls = [], requests = [];
  let nextTimer = 0;
  const state = { phase: 'ready', logs: [], canInstall: false };
  const document = {
    hidden, documentElement: {lang:'en'},
    addEventListener: (name, fn) => listeners.set(name, fn),
    getElementById(id) {
      if (!elements.has(id)) {
        const classes = new Set();
        elements.set(id, { textContent: '', disabled: false,
          classList: { add: x => classes.add(x), contains: x => classes.has(x),
            toggle(x, on) { if (on) classes.add(x); else classes.delete(x); } },
          addEventListener(name, fn) { this[name] = fn; },
        });
      }
      return elements.get(id);
    },
  };
  const sandbox = {
    document, console,
    setTimeout(fn, delay) { const id = ++nextTimer; timers.set(id, { fn, delay }); return id; },
    clearTimeout: id => timers.delete(id),
    window: { __TAURI_INTERNALS__: { invoke(command) {
      calls.push(command);
      if (command === 'get_native_locale') return Promise.resolve({language:locale,messages:JSON.parse(readFileSync(new URL(`../locales/ocean/${locale}.json`, import.meta.url),'utf8'))});
      if (command === 'get_setup_state') return new Promise((resolve, reject) => requests.push({ resolve, reject }));
      if (command === 'start_bootstrap') return new Promise(resolve => { state.installing = true; state.finish = resolve; });
      if (command === 'cancel_bootstrap') { state.cancelRequested = true; return Promise.resolve(true); }
      return Promise.resolve();
    } } },
  };
  const context=vm.createContext(sandbox);
  vm.runInContext(readFileSync(new URL('../ui/i18n.js', import.meta.url),'utf8'),context);
  vm.runInContext(script,context);
  return { document, elements, calls, requests, timers, state,
    visible(on) { document.hidden = !on; listeners.get('visibilitychange')(); },
    async respond() { requests.shift().resolve({ ...state }); await settle(); },
    async tick() { const entries = [...timers.values()]; timers.clear(); for (const {fn} of entries) fn(); await settle(); },
  };
}

test('Russian setup keeps recovery gates and renders native catalog text without HTML interpretation', async()=>{
 const f=fixture(false,'ru');
 Object.assign(f.state,{phase:'failed',recoveryRequired:true,updating:true,message:'<img src=x onerror=alert(1)>'});
 await f.respond();
 assert.equal(f.document.documentElement.lang,'ru');
 assert.equal(f.elements.get('recover').textContent,'Восстановить runtime…');
 assert.equal(f.elements.get('recover').disabled,true);
 f.elements.get('recover').click();await settle();assert.equal(f.calls.includes('recover_runtime'),false);
 f.elements.get('language').click();await settle();assert.equal(f.calls.filter(x=>x==='open_language_settings').length,1);
 Object.assign(f.state,{recoveryRequired:false});await f.tick();await f.respond();
 assert.equal(f.elements.get('status-text').textContent,'<img src=x onerror=alert(1)>');
 assert.equal(f.calls.includes('prepare_service'),false);
});

test('pending recovery replaces startup controls and opens only explicit recovery', async () => {
  const f = fixture();
  Object.assign(f.state, { phase: 'idle', canInstall: true, recoveryRequired: true });
  await f.respond();
  const recover = f.elements.get('recover');
  assert.ok(recover, 'pending recovery must have a dedicated control');
  assert.equal(recover.classList.contains('hidden'), false);
  assert.equal(f.elements.get('retry').classList.contains('hidden'), true);
  assert.equal(f.elements.get('install').classList.contains('hidden'), true);
  await f.tick();
  assert.equal(f.calls.includes('prepare_service'), false);
  recover.click();
  await settle();
  assert.equal(f.calls.filter(x => x === 'recover_runtime').length, 1);
  assert.equal(f.calls.includes('start_bootstrap'), false);
  Object.assign(f.state, { phase: 'failed', canInstall: false, recoveryRequired: false });
  await f.respond();
  assert.equal(recover.classList.contains('hidden'), true);
  assert.equal(f.elements.get('retry').classList.contains('hidden'), false);
});

test('recovery presentation uses structured state, not an error substring', async () => {
  const f = fixture();
  Object.assign(f.state, { phase: 'failed', message: 'pending runtime transaction', recoveryRequired: false });
  await f.respond();
  const recover = f.elements.get('recover');
  assert.ok(recover);
  assert.equal(recover.classList.contains('hidden'), true);
  assert.equal(f.elements.get('retry').classList.contains('hidden'), false);
});

test('ongoing native update disables recovery even for a dispatched click', async () => {
  const f = fixture();
  Object.assign(f.state, { phase: 'failed', recoveryRequired: true, updating: true });
  await f.respond();
  const recover = f.elements.get('recover');
  assert.ok(recover);
  assert.equal(recover.disabled, true);
  recover.click();
  await settle();
  assert.equal(f.calls.includes('recover_runtime'), false);
});

test('ready page pauses while hidden and refreshes immediately when shown', async () => {
  const f = fixture();
  await f.respond();
  assert.equal(f.timers.size, 1);
  f.visible(false);
  assert.equal(f.timers.size, 0);
  await f.tick();
  assert.equal(f.calls.filter(x => x === 'get_setup_state').length, 1);
  assert.equal(f.calls.filter(x => x === 'get_native_locale').length, 1);
  f.visible(true);
  assert.equal(f.calls.filter(x => x === 'get_setup_state').length, 2);
  assert.equal(f.calls.filter(x => x === 'get_native_locale').length, 1);
  await f.respond();
  assert.equal(f.timers.size, 1); // ready alone must not disable visible updates
});

test('slow requests never overlap, including hide/show and action refreshes', async () => {
  const f = fixture();
  f.visible(false);
  f.visible(true);
  f.elements.get('retry').click();
  await settle();
  await f.tick();
  assert.equal(f.calls.filter(x => x === 'get_setup_state').length, 1);
  assert.equal(f.timers.size, 0);
  f.visible(false);
  await f.respond();
  assert.equal(f.timers.size, 0); // late completion cannot restart hidden polling
  f.visible(true);
  await f.respond();
  await f.tick();
  assert.equal(f.requests.length, 1);
  assert.equal(f.timers.size, 0);
});

test('initially hidden page waits; failed IPC retries on the next timer', async () => {
  const f = fixture(true);
  assert.equal(f.calls.length, 0);
  f.visible(true);
  f.requests.shift().reject(new Error('offline'));
  await settle();
  assert.equal(f.elements.get('phase').textContent, 'Error');
  assert.equal(f.timers.size, 1);
  await f.tick();
  await f.respond();
  assert.equal(f.elements.get('phase').textContent, 'Ready');
});

test('pending installer and cancellation do not block state polling', async () => {
  const f = fixture();
  await f.respond();
  f.elements.get('install').click();
  await f.tick();
  await f.respond();
  const cancel = f.elements.get('cancel-install');
  assert.equal(cancel.classList.contains('hidden'), false);
  assert.equal(cancel.disabled, false);
  cancel.click();
  await settle();
  await f.respond();
  assert.equal(cancel.disabled, true);
  assert.match(cancel.textContent, /Cancelling/);
  f.state.installing = false;
  f.state.finish();
  await settle();
  await f.respond();
  assert.equal(cancel.classList.contains('hidden'), true);
  assert.equal(f.timers.size, 1);
});

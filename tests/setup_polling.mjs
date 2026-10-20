// Run with node --test tests/setup_polling.mjs; no GUI or dependencies.
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import assert from 'node:assert/strict';

const html = readFileSync(new URL('../ui/index.html', import.meta.url), 'utf8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
function fixture(hidden = false) {
  const timers = new Map(), elements = new Map(), listeners = new Map();
  const calls = [], requests = [];
  let nextTimer = 0;
  const state = { phase: 'ready', logs: [], canInstall: false };
  const document = {
    hidden,
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
  vm.runInNewContext(script, {
    document, console,
    setTimeout(fn, delay) { const id = ++nextTimer; timers.set(id, { fn, delay }); return id; },
    clearTimeout: id => timers.delete(id),
    window: { __TAURI_INTERNALS__: { invoke(command) {
      calls.push(command);
      if (command === 'get_setup_state') return new Promise((resolve, reject) => requests.push({ resolve, reject }));
      if (command === 'start_bootstrap') return new Promise(resolve => { state.installing = true; state.finish = resolve; });
      if (command === 'cancel_bootstrap') { state.cancelRequested = true; return Promise.resolve(true); }
      return Promise.resolve();
    } } },
  });
  return { document, elements, calls, requests, timers, state,
    visible(on) { document.hidden = !on; listeners.get('visibilitychange')(); },
    async respond() { requests.shift().resolve({ ...state }); await settle(); },
    async tick() { const entries = [...timers.values()]; timers.clear(); for (const {fn} of entries) fn(); await settle(); },
  };
}

test('ready page pauses while hidden and refreshes immediately when shown', async () => {
  const f = fixture();
  await f.respond();
  assert.equal(f.timers.size, 1);
  f.visible(false);
  assert.equal(f.timers.size, 0);
  await f.tick();
  assert.equal(f.calls.length, 1);
  f.visible(true);
  assert.equal(f.calls.length, 2);
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

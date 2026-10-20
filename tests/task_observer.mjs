import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const script = fs.readFileSync(new URL('../src/task_observer.js', import.meta.url), 'utf8');
const ACTIVITY = 'dsh-ocean:task-activity';
function realm({ origin = 'http://127.0.0.1:3080', frame = false, ready = true } = {}) {
  const window = new EventTarget(); window.top = frame ? {} : window; window.location = { origin };
  let created = 0, count = 0;
  class Socket extends EventTarget {
    static OPEN = 1; static CLOSED = 3;
    constructor(url) { super(); this.url = url; this.sent = []; this.ready = true; created++; }
    send(data) { if (!this.ready) throw new Error('native send failed'); this.sent.push(data); return 'native-result'; }
    message(value) { this.dispatchEvent(new MessageEvent('message', { data: typeof value === 'string' ? value : JSON.stringify(value) })); }
  }
  window.WebSocket = Socket;
  window.addEventListener(ACTIVITY, event => { assert.equal('detail' in event, false); assert.equal('data' in event, false); count++; });
  const context = vm.createContext({ window, Event, URL }); vm.runInContext(script, context);
  if (ready) window.dispatchEvent(new Event('dsh-ocean:task-bridge-ready'));
  return { window, Socket, context, get count() { return count; }, get created() { return created; } };
}
function open(socket, stream = 'events', generation = 'generation-1') {
  socket.send(JSON.stringify({ type: 'open', streamId: stream, endpoint: '$events', payload: { args: {} } }));
  if (generation) socket.message({ type: 'item', streamId: stream, value: { type: 'ready', clientId: generation, host: { home: 'synthetic-private-home' } } });
}
function status(socket, agent, running, stream = 'events') {
  socket.message({ type: 'item', streamId: stream, value: { type: 'emit', event: 'api-session/status', args: [agent, running] } });
}
const endpoint = 'ws://127.0.0.1:3080/api/remote.mux';

test('observes only a real true -> false; never initial or duplicate false', () => {
  const r = realm(); const ws = new r.Socket(endpoint); open(ws);
  status(ws, 'agent-private-id', false); assert.equal(r.count, 0);
  status(ws, 'agent-private-id', true); status(ws, 'agent-private-id', false); status(ws, 'agent-private-id', false);
  assert.equal(r.count, 1); assert.equal(r.created, 1);
});
test('preserves constructor, subclasses, statics, messages, return values and errors', () => {
  const r = realm(); assert.equal(r.window.WebSocket, r.Socket); assert.equal(r.window.WebSocket.OPEN, 1);
  class Child extends r.window.WebSocket {} const ws = new Child(endpoint);
  assert.ok(ws instanceof Child && ws instanceof r.Socket); assert.equal(ws.constructor, Child);
  const raw = '{"private":"not a status"}'; assert.equal(ws.send(raw), 'native-result'); assert.equal(ws.sent[0], raw);
  const bytes = new Uint8Array([1,2]); ws.send(bytes); assert.equal(ws.sent[1], bytes);
  ws.ready = false; assert.throws(() => ws.send('invalid state'), /native send failed/);
});
test('requires known event stream, readiness and strict positional boolean arguments', () => {
  const r = realm(); const ws = new r.Socket(endpoint);
  status(ws, 'x', true); status(ws, 'x', false); open(ws, 'events', null);
  status(ws, 'x', true); status(ws, 'x', false); assert.equal(r.count, 0);
  ws.message({ type: 'item', streamId: 'events', value: { type: 'ready', clientId: 'g', host: { home: '/synthetic-private-home' } } });
  for (const args of [[{ agentId: 'x', running: true }], ['x', 'true'], ['x', 0], ['x', false, 'extra'], ['', true]]) {
    ws.message({ type: 'item', streamId: 'events', value: { type: 'emit', event: 'api-session/status', args } });
  }
  status(ws, 'x', false); assert.equal(r.count, 0);
  status(ws, 'x', true); status(ws, 'x', false, 'other'); assert.equal(r.count, 0);
  status(ws, 'x', false); assert.equal(r.count, 1);
});
test('supports secure/base-relative endpoints; ignores foreign origin and unrelated sockets', () => {
  const r = realm({ origin: 'https://ocean.example:8443' });
  for (const address of ['wss://foreign.example/app/api/remote.mux', 'wss://ocean.example:8443/other', 'ws://ocean.example:8443/api/remote.mux']) {
    const ws = new r.Socket(address); open(ws); status(ws, 'x', true); status(ws, 'x', false);
  }
  assert.equal(r.count, 0);
  const ws = new r.Socket('wss://ocean.example:8443/prefix/api/remote.mux'); open(ws); status(ws, 'x', true); status(ws, 'x', false); assert.equal(r.count, 1);
});
test('socket close/error and logical end/error/cancel cannot look like completion', () => {
  for (const ending of ['close', 'socket-error', 'end', 'error', 'cancel', 'outgoing-cancel']) {
    const r = realm(); const ws = new r.Socket(endpoint); open(ws); status(ws, 'x', true);
    if (ending === 'close' || ending === 'socket-error') ws.dispatchEvent(new Event(ending === 'close' ? 'close' : 'error'));
    else if (ending === 'outgoing-cancel') ws.send(JSON.stringify({ type: 'cancel', streamId: 'events' }));
    else ws.message({ type: ending, streamId: 'events' });
    status(ws, 'x', false); assert.equal(r.count, 0, ending);
  }
});
test('generation replacement clears old transitions', () => {
  const r = realm(); const ws = new r.Socket(endpoint); open(ws); status(ws, 'x', true);
  ws.message({ type: 'item', streamId: 'events', value: { type: 'ready', clientId: 'replacement', host: { home: '/synthetic-private-home' } } });
  status(ws, 'x', false); assert.equal(r.count, 0);
  status(ws, 'x', true); status(ws, 'x', false); assert.equal(r.count, 1);
});
test('malformed ready frames reset observation; event streams are bounded', () => {
  const r = realm(); const ws = new r.Socket(endpoint); open(ws); status(ws, 'x', true);
  ws.message({ type: 'item', streamId: 'events', value: { type: 'ready', clientId: 'bad-without-host' } });
  status(ws, 'x', false); assert.equal(r.count, 0);
  for (let i=0; i<9; i++) open(ws, `stream-${i}`);
  status(ws, 'x', true, 'stream-0'); status(ws, 'x', false, 'stream-0'); assert.equal(r.count, 0);
  status(ws, 'x', true, 'stream-8'); status(ws, 'x', false, 'stream-8'); assert.equal(r.count, 1);
});
test('bounded state evicts old IDs instead of inventing their completion', () => {
  const r = realm(); const ws = new r.Socket(endpoint); open(ws);
  for (let i=0; i<1025; i++) status(ws, `agent-${i}`, true);
  status(ws, 'agent-0', false); assert.equal(r.count, 0);
  status(ws, 'agent-1024', false); assert.equal(r.count, 1);
});
test('malformed, binary, oversized or non-status frames never interrupt original delivery', () => {
  const r = realm(); const ws = new r.Socket(endpoint); open(ws); status(ws, 'x', true);
  for (const raw of ['broken', 'x'.repeat(8193), { type: 'item', streamId: 'events', value: { type: 'emit', event: 'api-session/status', args: ['x', false] }, extra: true }, { type: 'item', streamId: 'events', value: { type: 'emit', event: 'private-message', args: ['synthetic-secret'] } }]) ws.message(raw);
  ws.dispatchEvent(new MessageEvent('message', { data: new Uint8Array([1]) })); assert.equal(r.count, 0);
  status(ws, 'x', false); assert.equal(r.count, 1);
});
test('late isolated bridge coalesces pending activity without IDs; reinjection is idempotent', () => {
  const r = realm({ ready: false }); const ws = new r.Socket(endpoint); open(ws);
  for (const id of ['private-a', 'private-b']) { status(ws, id, true); status(ws, id, false); }
  assert.equal(r.count, 0); r.window.dispatchEvent(new Event('dsh-ocean:task-bridge-ready')); assert.equal(r.count, 1);
  vm.runInContext(script, r.context); status(ws, 'private-a', true); status(ws, 'private-a', false); assert.equal(r.count, 2);
});
test('never patches child frames', () => {
  const r = realm({ frame: true }); const ws = new r.Socket(endpoint); open(ws); status(ws, 'x', true); status(ws, 'x', false); assert.equal(r.count, 0);
});

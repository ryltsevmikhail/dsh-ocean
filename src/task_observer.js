// Observe the inspected DSH 0.2.0-rc.2 / 0.2.1-rc.2 text mux; never open another connection.
// No content/ID leaves this world. Unknown contracts fail quiet and pass through.
(() => {
  if (window.top !== window || typeof window.WebSocket !== 'function') return;
  const guard = Symbol.for('dev.dsh.ocean.task-observer');
  if (window[guard]) return;
  const NativeSocket = window.WebSocket;
  const descriptor = Object.getOwnPropertyDescriptor(NativeSocket.prototype, 'send');
  if (!descriptor || typeof descriptor.value !== 'function') return;
  const originalSend = descriptor.value;
  const sockets = new WeakMap();
  const ACTIVITY = 'dsh-ocean:task-activity';
  let bridgeReady = false, pending = false;
  function activity() {
    if (!bridgeReady) { pending = true; return; }
    window.dispatchEvent(new Event(ACTIVITY));
  }
  window.addEventListener('dsh-ocean:task-bridge-ready', () => {
    bridgeReady = true;
    if (pending) { pending = false; activity(); }
  });
  function keys(value, allowed) {
    return value && typeof value === 'object' && !Array.isArray(value)
      && Object.keys(value).length === allowed.length && allowed.every(key => Object.hasOwn(value, key));
  }
  const id = value => typeof value === 'string' && value.length > 0 && value.length <= 256;
  function parse(data) {
    if (typeof data !== 'string' || data.length > 8192) return null;
    try { return JSON.parse(data); } catch { return null; }
  }
  function stateFor(socket) {
    if (sockets.has(socket)) return sockets.get(socket);
    let state = null;
    try {
      const url = new URL(socket.url);
      if (!['ws:', 'wss:'].includes(url.protocol)) return null;
      url.protocol = url.protocol === 'wss:' ? 'https:' : 'http:';
      if (url.origin !== window.location.origin || !url.pathname.endsWith('/api/remote.mux')) return null;
      state = { streams: new Map(), agents: new Map(), generation: null, dead: false };
      const reset = () => { state.streams.clear(); state.agents.clear(); state.generation = null; };
      socket.addEventListener('close', () => { reset(); state.dead = true; });
      socket.addEventListener('error', () => { reset(); state.dead = true; });
      socket.addEventListener('message', event => {
        try {
          if (state.dead) return;
          const frame = parse(event.data);
          if (!frame || !id(frame.streamId) || !state.streams.has(frame.streamId)) return;
          if (frame.type === 'end' || frame.type === 'error' || frame.type === 'cancel') {
            state.streams.delete(frame.streamId); state.agents.clear(); return;
          }
          if (!keys(frame, ['type', 'streamId', 'value']) || frame.type !== 'item') return;
          const value = frame.value;
          if (value?.type === 'ready') {
            if (!keys(value, ['type', 'clientId', 'host']) || !id(value.clientId)
                || !keys(value.host, ['home']) || typeof value.host.home !== 'string') {
              state.streams.set(frame.streamId, false); state.agents.clear(); return;
            }
            if (state.generation !== value.clientId) {
              state.agents.clear();
              for (const stream of state.streams.keys()) state.streams.set(stream, false);
            }
            state.generation = value.clientId; state.streams.set(frame.streamId, true); return;
          }
          if (state.streams.get(frame.streamId) !== true) return;
          if (!keys(value, ['type', 'event', 'args']) || value.type !== 'emit'
              || value.event !== 'api-session/status' || !Array.isArray(value.args)
              || value.args.length !== 2 || !id(value.args[0]) || typeof value.args[1] !== 'boolean') return;
          const [agent, running] = value.args;
          const previous = state.agents.get(agent);
          if (!state.agents.has(agent) && state.agents.size >= 1024) state.agents.delete(state.agents.keys().next().value);
          state.agents.set(agent, running);
          if (previous === true && running === false) activity();
        } catch { /* Never break delivery or log private frames. */ }
      });
    } catch { state = null; }
    sockets.set(socket, state);
    return state;
  }
  try {
    Object.defineProperty(NativeSocket.prototype, 'send', { ...descriptor, value: function send(data) {
      let state;
      try { state = stateFor(this); } catch { /* Native send retains its own errors. */ }
      const result = Reflect.apply(originalSend, this, arguments);
      try {
        if (state && !state.dead) {
          const frame = parse(data);
          if (frame?.type === 'open' && keys(frame, ['type', 'streamId', 'endpoint', 'payload'])
              && id(frame.streamId) && frame.endpoint === '$events'
              && keys(frame.payload, ['args']) && keys(frame.payload.args, [])) {
            if (!state.streams.has(frame.streamId) && state.streams.size >= 8) {
              state.streams.delete(state.streams.keys().next().value); state.agents.clear();
            }
            state.streams.set(frame.streamId, false);
          } else if (frame?.type === 'cancel' && state.streams.has(frame.streamId)) {
            state.streams.delete(frame.streamId); state.agents.clear();
          }
        }
      } catch { /* Pass original frames untouched. */ }
      return result;
    }});
    Object.defineProperty(window, guard, { value: true });
    window.dispatchEvent(new Event('dsh-ocean:task-adapter-ready'));
  } catch { /* Unsupported/frozen transports are not modified further. */ }
})();

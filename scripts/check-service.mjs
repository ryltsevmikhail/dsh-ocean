// Verify an already-running user unit. Read launch tokens only from its current
// invocation; never print journal output, tokens, or token-bearing URLs.
import { spawnSync } from 'node:child_process';
import http from 'node:http';
import { setTimeout as delay } from 'node:timers/promises';

const root = process.argv.at(-1);
const unit = process.env.DSH_OCEAN_SERVICE_UNIT || 'dsh-web.service';
if (unit.length > 128 || (unit !== 'dsh-web.service' && !/^dsh-ocean-test-[A-Za-z0-9-]+\.service$/.test(unit))) {
  throw new Error('unsupported service unit namespace');
}
let base;
try { base = new URL(process.env.DSH_OCEAN_SERVICE_URL || 'http://127.0.0.1:3080/'); }
catch { throw new Error('invalid service health origin'); }
if (base.protocol !== 'http:' || base.hostname !== '127.0.0.1' || base.username || base.password) {
  throw new Error('service health requires an unauthenticated loopback HTTP origin');
}
const duration = Number(process.env.DSH_OCEAN_SERVICE_HEALTH_TIMEOUT_MS || 45000);
if (!Number.isInteger(duration) || duration < 100 || duration > 60000) throw new Error('invalid health timeout');
if (!root?.startsWith('/')) throw new Error('absolute generation root required');

function command(binary, args) {
  const result = spawnSync(binary, args, { encoding: 'utf8', timeout: 2000, maxBuffer: 512 * 1024 });
  return result.status === 0 ? result.stdout.trim() : null;
}
function property(name) {
  return command('systemctl', ['--user', 'show', unit, `--property=${name}`, '--value']);
}
function request(url) {
  return new Promise((resolve) => {
    const req = http.get(url, { agent: false, timeout: 1500 }, (res) => { res.resume(); resolve(res.statusCode); });
    req.on('timeout', () => req.destroy());
    req.on('error', () => resolve(null));
  });
}

const deadline = performance.now() + duration;
let healthy = false;
while (performance.now() < deadline) {
  const invocation = property('InvocationID');
  const pid = property('MainPID');
  const exec = property('ExecStart');
  if (invocation && /^[a-f0-9]{32}$/i.test(invocation) && /^[1-9][0-9]*$/.test(pid || '')
      && exec?.includes(`${root}/runtime/node/bin/node`)
      && exec.includes(`${root}/harness/node_modules/.bin/dsh`)) {
    const text = command('journalctl', ['--user', `--user-unit=${unit}`, `_SYSTEMD_INVOCATION_ID=${invocation}`, '--grep=dsh web:', '-n', '20', '--no-pager', '-o', 'cat']);
    for (const line of (text || '').split('\n').reverse()) {
      const match = line.match(/dsh web:\s+(http:\/\/\S+)/);
      if (!match) continue;
      try {
        const url = new URL(match[1]);
        if (url.origin !== base.origin || !url.searchParams.get('token')) continue;
        if (await request(new URL('/', base)) === 401 && await request(url) === 303
            && property('InvocationID') === invocation && property('MainPID') === pid) {
          healthy = true;
          break;
        }
      } catch { /* Untrusted/malformed log lines are not diagnostics. */ }
    }
  }
  if (healthy) break;
  await delay(100);
}
if (!healthy) {
  console.error('[service-health] selected runtime did not pass authenticated health in the current invocation');
  process.exitCode = 1;
} else {
  console.log('[service-health] selected generation is serving authenticated HTTP');
}

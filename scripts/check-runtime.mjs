// Check a candidate runtime using a disposable HOME/profile and an OS-assigned
// loopback port. Never reads the user's credentials or reuses the live server.
import { spawn } from 'node:child_process';
import { mkdtemp, rm, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import http from 'node:http';
import { once } from 'node:events';

const root = process.argv.at(-1);
if (!root || !path.isAbsolute(root)) throw new Error('absolute runtime root required');
const home = await mkdtemp(path.join(tmpdir(), 'dsh-ocean-health.'));
const node = path.join(root, 'runtime/node/bin/node');
const dsh = path.join(root, 'harness/node_modules/.bin/dsh');
let child;
let timer;
let stopping = false;
const diagnostics = [];

function request(url) {
  return new Promise((resolve, reject) => {
    const req = http.get(url, { agent: false, timeout: 3000 }, (res) => {
      res.resume();
      resolve(res.statusCode);
    });
    req.on('timeout', () => req.destroy(new Error('health request timed out')));
    // Do not propagate the URL/token in network exception text.
    req.on('error', () => reject(new Error('health request failed')));
  });
}

function signalGroup(signal) {
  if (!child?.pid) return;
  try { process.kill(-child.pid, signal); }
  catch (error) { if (error.code !== 'ESRCH') throw error; }
}

async function stop() {
  if (stopping) return;
  stopping = true;
  clearTimeout(timer);
  if (child?.pid) {
    const exited = child.exitCode !== null || child.signalCode !== null;
    const done = exited ? Promise.resolve() : once(child, 'exit').catch(() => {});
    signalGroup('SIGTERM');
    let grace;
    await Promise.race([done, new Promise((resolve) => { grace = setTimeout(resolve, 1500); })]);
    clearTimeout(grace);
    // Also terminate descendants that survived their parent.
    signalGroup('SIGKILL');
    await done;
  }
  await rm(home, { recursive: true, force: true });
}

let failHealth;
const interrupted = () => failHealth?.(new Error('health check interrupted'));
process.once('SIGINT', interrupted);
process.once('SIGTERM', interrupted);
try {
  await mkdir(path.join(home, 'tmp'));
  const ready = new Promise((resolve, reject) => {
    failHealth = reject;
    timer = setTimeout(() => reject(new Error('DSH health check timed out')), 60000);
    child = spawn(node, [dsh, '--profile', 'web', '--host', '127.0.0.1', '--port', '0', '--no-open'], {
      cwd: home,
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: {
        HOME: home,
        DSH_HOME: path.join(home, '.dsh'),
        DSH_TELEMETRY_DISABLED: '1',
        XDG_CONFIG_HOME: path.join(home, '.config'),
        XDG_DATA_HOME: path.join(home, '.local/share'),
        XDG_STATE_HOME: path.join(home, '.local/state'),
        XDG_CACHE_HOME: path.join(home, '.cache'),
        TMPDIR: path.join(home, 'tmp'),
        LANG: 'C.UTF-8',
        PATH: `${path.dirname(node)}:${path.join(root, 'tools/node_modules/.bin')}:/usr/bin:/bin`,
      },
    });
    child.once('error', () => reject(new Error('could not start candidate DSH')));
    child.once('exit', () => reject(new Error('candidate DSH exited during health check')));
    let probing = false;
    const buffers = new Map();
    for (const stream of [child.stdout, child.stderr]) {
      buffers.set(stream, { text: '', discard: false });
      stream.setEncoding('utf8');
      stream.on('data', (chunk) => {
        const buffer = buffers.get(stream);
        if (buffer.discard) {
          const end = chunk.indexOf('\n');
          if (end === -1) return;
          chunk = chunk.slice(end + 1);
          buffer.discard = false;
        }
        const lines = (buffer.text + chunk).split('\n');
        buffer.text = lines.pop();
        if (buffer.text.length > 16384) {
          buffer.text = '';
          buffer.discard = true;
        }
        for (const line of lines) {
          if (line.length > 16384) continue;
          const safe = line.replace(/https?:\/\/\S+/g, '[URL omitted]').replace(/token=[^\s&]+/g, 'token=***').slice(0, 1000);
          if (safe.trim()) diagnostics.push(safe);
          if (diagnostics.length > 24) diagnostics.shift();
          const match = line.match(/dsh web:\s+(http:\/\/\S+)/);
          if (!match || probing) continue;
          probing = true;
          void (async () => {
            const url = new URL(match[1]);
            if (url.hostname !== '127.0.0.1' || !url.port || url.port === '3080' || !url.searchParams.get('token')) {
              throw new Error('candidate reported an unsafe health URL');
            }
            const origin = new URL('/', url);
            if (await request(origin) !== 401 || await request(url) !== 303) {
              throw new Error('candidate DSH failed authenticated HTTP health check');
            }
            if (child.exitCode !== null || child.signalCode !== null) throw new Error('candidate exited before health completion');
            resolve();
          })().catch(() => reject(new Error('candidate DSH failed authenticated HTTP health check')));
        }
      });
    }
  });
  await ready;
  console.log('[health] isolated DSH returned 401 and authenticated 303');
} catch (error) {
  console.error(`[health] ${error.message}`);
  // Only bounded, whole, redacted lines from an empty isolated profile.
  for (const line of diagnostics) console.error(`[health:diagnostic] ${line}`);
  process.exitCode = 1;
} finally {
  await stop();
}

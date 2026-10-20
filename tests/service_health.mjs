import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const helper = fileURLToPath(new URL('../scripts/check-service.mjs', import.meta.url));
const token = 'synthetic-service-token-never-print';
const invocation = '1'.repeat(32);
function executable(file, source) { fs.writeFileSync(file, source, { mode: 0o755 }); }
async function run(mode, embedded = false, unit = 'dsh-web.service') {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dsh-ocean-service-test.'));
  fs.mkdirSync(path.join(dir, 'bin'));
  fs.writeFileSync(path.join(dir, 'invocation'), invocation);
  const requests = [];
  const server = http.createServer((req, res) => {
    requests.push(req.url);
    if (mode === 'invocation-change' && req.url.includes('token=')) {
      fs.writeFileSync(path.join(dir, 'invocation'), '2'.repeat(32));
    }
    res.statusCode = req.url.includes(`token=${token}`) ? 303 : 401;
    res.end();
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${server.address().port}/`;
  executable(path.join(dir, 'bin/systemctl'), `#!/bin/bash
[ "$3" = "$SERVICE_UNIT" ] || exit 99
for arg in "$@"; do
  case "$arg" in
    --property=InvocationID) /bin/cat "$FIXTURE/invocation" ;;
    --property=MainPID) echo 12345 ;;
    --property=ExecStart)
      if [ "$MODE" = wrong-runtime ]; then echo /wrong/runtime; else
        printf '%s/runtime/node/bin/node %s/harness/node_modules/.bin/dsh' "$SERVICE_ROOT" "$SERVICE_ROOT"
      fi ;;
  esac
done
`);
  executable(path.join(dir, 'bin/journalctl'), `#!/bin/bash
printf '%s\\n' "$@" > "$FIXTURE/journal-args"
[[ " $* " == *" --user-unit=$SERVICE_UNIT "* ]] || exit 99
[[ " $* " == *" _SYSTEMD_INVOCATION_ID=${invocation} "* ]] || exit 0
if [ "$MODE" = stale ]; then
  echo "dsh web: $SERVICE_URL?token=stale-token"
else
  echo "dsh web: $SERVICE_URL?token=${token}"
fi
`);
  try {
    const args = embedded ? ['--input-type=module', '-e', fs.readFileSync(helper, 'utf8'), dir] : [helper, dir];
    const child = spawn(process.execPath, args, {
      env: { ...process.env, PATH: `${dir}/bin:/usr/bin:/bin`, FIXTURE: dir, MODE: mode, SERVICE_ROOT: dir, SERVICE_URL: url,
        SERVICE_UNIT: unit, DSH_OCEAN_SERVICE_UNIT: unit, DSH_OCEAN_SERVICE_URL: url, DSH_OCEAN_SERVICE_HEALTH_TIMEOUT_MS: mode === 'ok' ? '3000' : '300' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '', stderr = '';
    child.stdout.on('data', (part) => { stdout += part; });
    child.stderr.on('data', (part) => { stderr += part; });
    const status = await new Promise((resolve, reject) => { child.on('error', reject); child.on('exit', resolve); });
    assert.ok(!`${stdout}${stderr}`.includes(token), 'service token must not enter diagnostics');
    if (fs.existsSync(path.join(dir, 'journal-args'))) {
      assert.ok(fs.readFileSync(path.join(dir, 'journal-args'), 'utf8').includes('_SYSTEMD_INVOCATION_ID='));
    }
    return { status, stdout, stderr, requests };
  } finally {
    await new Promise((resolve) => server.close(resolve));
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

for (const embedded of [false, true]) {
  test(`accepts current invocation and selected runtime (embedded=${embedded})`, async () => {
    const result = await run('ok', embedded);
    assert.equal(result.status, 0, result.stderr);
    assert.ok(result.requests.includes('/'));
    assert.ok(result.requests.some((url) => url.includes(`token=${token}`)));
  });
}
test('uses an explicit isolated unit for all manager and journal queries', async () => {
  const result = await run('ok', true, 'dsh-ocean-test-explicit.service');
  assert.equal(result.status, 0, result.stderr);
});
test('rejects unexpected service namespaces without HTTP requests', async () => {
  for (const unit of ['dbus.service', '../dsh-web.service', 'dsh-ocean-test-.service']) {
    const result = await run('ok', false, unit);
    assert.equal(result.status, 1);
    assert.deepEqual(result.requests, []);
    assert.ok(result.stderr.includes('unsupported service unit namespace'));
  }
});
for (const mode of ['stale', 'wrong-runtime', 'invocation-change']) {
  test(`rejects ${mode} rather than reporting update success`, async () => {
    const result = await run(mode);
    assert.equal(result.status, 1, result.stderr);
    assert.ok(result.stderr.includes('did not pass authenticated health'));
    if (mode === 'wrong-runtime') assert.deepEqual(result.requests, []);
  });
}

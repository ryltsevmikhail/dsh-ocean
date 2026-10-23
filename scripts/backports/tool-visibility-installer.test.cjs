const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { spawnSync } = require('node:child_process');
const { before, after, transform } = require('./tool-visibility.cjs');
const installed = process.env.VISIBILITY_FILE || path.join(os.homedir(), '.dsh/profiles/web/node_modules/dsh-plugin-tool-management/lib/mcp/tool-visibility.js');
const original = fs.readFileSync(installed, 'utf8').replace(after, before);
const patched = transform(original);
function fixture(version = '0.20.0', source = original) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dsh-backport-test-'));
  fs.mkdirSync(path.join(root, 'lib/mcp'), { recursive: true });
  fs.writeFileSync(path.join(root, 'package.json'), JSON.stringify({ name: 'dsh-plugin-tool-management', version }));
  const target = path.join(root, 'lib/mcp/tool-visibility.js');
  fs.writeFileSync(target, source);
  const run = mode => spawnSync(process.execPath, [path.join(__dirname, 'tool-visibility.cjs'), mode, root], { encoding: 'utf8' });
  return { root, target, run };
}
test('apply, backup, idempotence and rollback without touching live service', () => {
  const f = fixture();
  assert.equal(f.run('--check').status, 0);
  assert.equal(f.run('--apply').status, 0);
  assert.equal(fs.readFileSync(f.target, 'utf8'), patched);
  assert.equal(fs.readFileSync(f.target + '.cpu-backport.original', 'utf8'), original);
  assert.equal(f.run('--apply').status, 0);
  assert.equal(f.run('--rollback').status, 0);
  assert.equal(fs.readFileSync(f.target, 'utf8'), original);
});
test('refuses unknown version or modified original', () => {
  for (const f of [fixture('0.21.0'), fixture('0.20.0', original + '\n// different\n')]) {
    const saved = fs.readFileSync(f.target, 'utf8');
    assert.notEqual(f.run('--apply').status, 0);
    assert.equal(fs.readFileSync(f.target, 'utf8'), saved);
  }
});
test('refuses damaged backup for apply and rollback', () => {
  const f = fixture();
  fs.writeFileSync(f.target + '.cpu-backport.original', 'wrong');
  assert.notEqual(f.run('--apply').status, 0);
  assert.equal(fs.readFileSync(f.target, 'utf8'), original);
  fs.writeFileSync(f.target, patched);
  assert.notEqual(f.run('--rollback').status, 0);
  assert.equal(fs.readFileSync(f.target, 'utf8'), patched);
});

#!/usr/bin/env node
// Explicit, opt-in backport. Never restarts DSH.
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const ORIGINAL = 'a8aacdd24b069c2e10aaee82ea9cb57d2da1521b7759ac0c1805e1bbc5086b34';
const before = `        // 每次重放先撤掉每个 agent 上的旧层：名单变短（重新启用某个工具）时，只有撤掉这层限制
        // 它才会重新可见；名单变了（含逐 agent 那份）也必须重装而不是叠加。
        epoch += 1;
        for (const agent of [...agentRestrictions.keys()])
            liftRestriction(agent);`;
const after = `        // Keep live agents' layers and comparison keys: restrictAgent replaces a
        // changed deny list (including an empty one). Unconditional teardown here
        // erases those keys and makes tools/change continuously retrigger itself.
        epoch += 1;
        for (const agent of [...agentRestrictions.keys()]) {
            if (!seenAgents.has(agent))
                liftRestriction(agent);
        }`;
const hash = x => crypto.createHash('sha256').update(x).digest('hex');
function transform(source) {
  if (hash(source) !== ORIGINAL) throw Error('Unknown source hash; refusing to patch');
  if (source.split(before).length !== 2) throw Error('Patch context must occur exactly once');
  return source.replace(before, after);
}
function main() {
  const [mode = '--check', root = path.join(os.homedir(), '.dsh/profiles/web/node_modules/dsh-plugin-tool-management')] = process.argv.slice(2);
  if (!['--check', '--apply', '--rollback'].includes(mode)) throw Error('Usage: tool-visibility.cjs [--check|--apply|--rollback] [package-directory]');
  const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
  if (pkg.name !== 'dsh-plugin-tool-management' || pkg.version !== '0.20.0') throw Error('Expected dsh-plugin-tool-management 0.20.0; review upstream instead');
  const target = path.join(root, 'lib/mcp/tool-visibility.js');
  const backup = target + '.cpu-backport.original';
  const source = fs.readFileSync(target, 'utf8');
  const isOriginal = hash(source) === ORIGINAL;
  const original = isOriginal ? source : source.replace(after, before);
  const patched = transform(original); // validates complete original, not just a marker
  if (source !== original && source !== patched) throw Error('Unknown installed content');
  if (mode === '--check') { console.log(isOriginal ? 'Original: backport needed' : 'Patched: verified'); return; }
  if (mode === '--rollback') {
    if (isOriginal) { console.log('Already original'); return; }
    const saved = fs.readFileSync(backup, 'utf8');
    if (hash(saved) !== ORIGINAL) throw Error('Backup hash mismatch');
    fs.writeFileSync(target, saved);
    console.log('Restored original on disk. Running DSH is unchanged.'); return;
  }
  if (!isOriginal) { console.log('Already patched'); return; }
  if (fs.existsSync(backup)) {
    if (hash(fs.readFileSync(backup)) !== ORIGINAL) throw Error('Existing backup hash mismatch');
  } else fs.copyFileSync(target, backup, fs.constants.COPYFILE_EXCL);
  // Leave the original untouched unless syntax validation succeeds.
  // Validate as an ES module via stdin.
  const syntax = spawnSync(process.execPath, ['--input-type=module', '--check'], { input: patched, encoding: 'utf8' });
  if (syntax.status !== 0) throw Error(syntax.stderr);
  if (fs.readFileSync(target, 'utf8') !== source) throw Error('Target changed while patching');
  fs.writeFileSync(target, patched);
  console.log('Backport applied; backup: ' + backup + '. Running DSH is unchanged.');
}
module.exports = { transform, ORIGINAL, before, after };
if (require.main === module) { try { main(); } catch (e) { console.error(e.message); process.exitCode = 1; } }

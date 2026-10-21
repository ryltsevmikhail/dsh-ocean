// Ephemeral TEST-ONLY keys; no production key/trust root is generated or read.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { spawn, spawnSync } from 'node:child_process';
import { signRelease } from '../scripts/sign-release.mjs';
const script = fileURLToPath(new URL('../scripts/sign-release.mjs', import.meta.url));
function fixture(t, algorithm = 'ed25519') {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dsh-ocean-signer-test-'));
  fs.chmodSync(root, 0o700);
  t.after(() => { const actual = fs.realpathSync(root); assert.equal(actual, root); assert.ok(path.basename(actual).startsWith('dsh-ocean-signer-test-')); fs.rmSync(actual, { recursive: true }); });
  const { privateKey, publicKey } = crypto.generateKeyPairSync(algorithm, algorithm === 'rsa' ? { modulusLength: 2048 } : {});
  const pem = privateKey.export({ format: 'pem', type: 'pkcs8' });
  const key = path.join(root, 'private.pem'); const manifest = path.join(root, 'manifest.json');
  fs.writeFileSync(key, pem, { mode: 0o600 }); fs.writeFileSync(manifest, '{"synthetic":"release fixture"}\n');
  return { root, key, manifest, pem, publicKey, args: ['--manifest', manifest, '--private-key', key, '--key-id', 'test-only'] };
}
test('signs exact domain plus raw bytes and exposes only public material', t => {
  const f = fixture(t); const before = fs.readFileSync(f.key); const bundle = signRelease(f.args);
  const signature = Buffer.from(bundle.signature.signatureHex, 'hex');
  assert.equal(signature.length, 64); assert.equal(bundle.publicKey.publicKeyHex.length, 64);
  const bytes = fs.readFileSync(f.manifest); const domain = Buffer.from('DSH Ocean release manifest v1\n');
  assert.ok(crypto.verify(null, Buffer.concat([domain, bytes]), f.publicKey, signature));
  assert.equal(crypto.verify(null, bytes, f.publicKey, signature), false);
  assert.equal(crypto.verify(null, Buffer.concat([domain, bytes, Buffer.from(' ')]), f.publicKey, signature), false);
  assert.equal(bundle.manifestSha256, crypto.createHash('sha256').update(bytes).digest('hex'));
  assert.equal(bundle.publicKeySha256, crypto.createHash('sha256').update(Buffer.from(bundle.publicKey.publicKeyHex, 'hex')).digest('hex'));
  const output = JSON.stringify(bundle); assert.ok(!output.includes('PRIVATE KEY') && !output.includes(f.pem.toString().split('\n')[1]));
  assert.deepEqual(fs.readFileSync(f.key), before); assert.equal(fs.statSync(f.key).mode & 0o777, 0o600); assert.equal(fs.readdirSync(f.root).length, 2);
});
test('runtime signing is an explicit separate domain and cannot authorize shell bytes',t=>{
  const f=fixture(t);const bytes=fs.readFileSync(f.manifest);
  const bundle=signRelease([...f.args,'--product','runtime']);const signature=Buffer.from(bundle.signature.signatureHex,'hex');
  assert.ok(crypto.verify(null,Buffer.concat([Buffer.from('DSH Ocean runtime manifest v1\n'),bytes]),f.publicKey,signature));
  assert.equal(crypto.verify(null,Buffer.concat([Buffer.from('DSH Ocean release manifest v1\n'),bytes]),f.publicKey,signature),false);
  const shell=Buffer.from(signRelease(f.args).signature.signatureHex,'hex');
  assert.equal(crypto.verify(null,Buffer.concat([Buffer.from('DSH Ocean runtime manifest v1\n'),bytes]),f.publicKey,shell),false);
  const cli=spawnSync(process.execPath,[script,...f.args,'--product','runtime'],{encoding:'utf8',env:{HOME:f.root,PATH:'/nonexistent'}});assert.equal(cli.status,0,cli.stderr);
  assert.ok(crypto.verify(null,Buffer.concat([Buffer.from('DSH Ocean runtime manifest v1\n'),bytes]),f.publicKey,Buffer.from(JSON.parse(cli.stdout).signature.signatureHex,'hex')));
  assert.throws(()=>signRelease([...f.args,'--product','unknown']),/product/);
  assert.throws(()=>signRelease([...f.args,'--product','runtime','--product','shell']),/duplicate/);
});
test('runtime publisher signature passes the independent Rust verifier offline', {skip:!process.env.DSH_OCEAN_TEST_VERIFIER}, t=>{
  const f=fixture(t);const now=Date.now();
  fs.writeFileSync(f.manifest,JSON.stringify({version:1,product:'dsh-ocean-runtime',channel:'stable',sequence:7,release:'0.2.1',publishedAt:new Date(now-60000).toISOString(),expiresAt:new Date(now+3600000).toISOString(),components:{node:'26.12.0',dsh:'0.2.1',pnpm:'10.34.7'},stateScope:'web-profile-sessions-storages-v1',artifacts:[{target:'linux-x86_64',kind:'tar.gz',url:'https://github.com/ryltsevmikhail/dsh-ocean/releases/download/runtime-stable-v0.2.1-r7/dsh-ocean-runtime-stable-0.2.1-7-x86_64.tar.gz',size:8,sha256:'ab'.repeat(32),nodeArchiveSha256:'cd'.repeat(32)}]}));
  const signature=path.join(f.root,'signature.json');const publicKey=path.join(f.root,'public.json');
  const verify=bundle=>{fs.writeFileSync(signature,JSON.stringify(bundle.signature));fs.writeFileSync(publicKey,JSON.stringify(bundle.publicKey));return spawnSync(process.env.DSH_OCEAN_TEST_VERIFIER,['--verify-runtime-release',f.manifest,signature,publicKey,'--channel','stable','--target','linux-x86_64','--min-sequence','1','--current-runtime-version','0.2.0'],{encoding:'utf8',timeout:5000,env:{HOME:f.root,XDG_STATE_HOME:path.join(f.root,'state'),PATH:'/nonexistent'}});};
  const result=verify(signRelease([...f.args,'--product','runtime']));assert.equal(result.status,0,result.stderr);
  assert.notEqual(verify(signRelease(f.args)).status,0,'shell-domain signing must fail independent Runtime verifier');assert.ok(!fs.existsSync(path.join(f.root,'state')),'offline verification must not remember a floor');
});
test('supports an owner-read-only PEM without altering it', t => {
  const f = fixture(t); fs.chmodSync(f.key, 0o400); assert.equal(signRelease(f.args).signature.keyId, 'test-only'); assert.equal(fs.statSync(f.key).mode & 0o777, 0o400);
});
test('rejects loose/executable key modes before signing', t => {
  const f = fixture(t); for (const mode of [0o644, 0o660, 0o700]) { fs.chmodSync(f.key, mode); assert.throws(() => signRelease(f.args), /0600 or 0400/); }
});
test('rejects symlinked and multiply-linked private keys', t => {
  const f = fixture(t); const link = path.join(f.root, 'link.pem'); fs.symlinkSync(f.key, link);
  const args = [...f.args]; args[3] = link; assert.throws(() => signRelease(args), /cannot read signing input/);
  fs.linkSync(f.key, path.join(f.root, 'hard.pem')); assert.throws(() => signRelease(f.args), /links or size/);
});
test('rejects non-Ed25519 keys and does not echo native crypto errors', t => {
  const f = fixture(t, 'rsa'); assert.throws(() => signRelease(f.args), /^Error: only Ed25519/);
  fs.writeFileSync(f.key, 'synthetic-private-corrupt-PEM');
  assert.throws(() => signRelease(f.args), /^Error: invalid or encrypted private key PEM$/);
});
test('rejects bounded/empty inputs and invalid/duplicate option values', t => {
  const f = fixture(t); fs.writeFileSync(f.manifest, ''); assert.throws(() => signRelease(f.args), /empty manifest/);
  fs.writeFileSync(f.manifest, Buffer.alloc(65537)); assert.throws(() => signRelease(f.args), /size refused/);
  const id = [...f.args]; id[5] = '../../synthetic-private'; assert.throws(() => signRelease(id), /key identity/);
  const duplicate = [...f.args]; duplicate[4] = '--manifest'; assert.throws(() => signRelease(duplicate), /duplicate signing option/);
});
test('rejects oversized PEMs and FIFO inputs without blocking', t => {
  const f = fixture(t); fs.writeFileSync(f.key, Buffer.alloc(8193)); assert.throws(() => signRelease(f.args), /size refused/);
  const fifo = path.join(f.root, 'private-fifo'); const made = spawnSync('/usr/bin/mkfifo', [fifo]); assert.equal(made.status, 0);
  const args = [...f.args]; args[3] = fifo;
  const rejected = spawnSync(process.execPath, [script, ...args], { encoding: 'utf8', timeout: 2000, env: { HOME: f.root, PATH: '/nonexistent' } });
  assert.equal(rejected.status, 1); assert.equal(rejected.stdout, ''); assert.ok(rejected.stderr.includes('input type'));
});
test('broken output pipes fail without an unhandled Node stack', async t => {
  const f = fixture(t); const child = spawn(process.execPath, [script, ...f.args], { stdio: ['ignore', 'pipe', 'pipe'], env: { HOME: f.root, PATH: '/nonexistent' } });
  child.stdout.destroy(); let error = ''; child.stderr.setEncoding('utf8'); child.stderr.on('data', value => { error += value; });
  const code = await new Promise((resolve, reject) => { child.once('close', resolve); child.once('error', reject); });
  assert.equal(code, 1); assert.ok(!error.includes('Unhandled') && !error.includes(' at ') && !error.includes(f.root));
});
test('CLI works offline/headless and prints no private bytes on failures', t => {
  const f = fixture(t); const env = { HOME: f.root, PATH: '/nonexistent', XDG_CONFIG_HOME: path.join(f.root, 'must-not-create') };
  const success = spawnSync(process.execPath, [script, ...f.args], { encoding: 'utf8', env }); assert.equal(success.status, 0, success.stderr);
  assert.equal(JSON.parse(success.stdout).signature.keyId, 'test-only'); assert.equal(fs.existsSync(env.XDG_CONFIG_HOME), false);
  fs.writeFileSync(f.key, 'synthetic-private-malformed-data');
  const failure = spawnSync(process.execPath, [script, ...f.args], { encoding: 'utf8', env }); assert.equal(failure.status, 1); assert.equal(failure.stdout, '');
  assert.ok(!failure.stderr.includes('synthetic-private') && !failure.stderr.includes(f.root) && !failure.stderr.includes(' at '));
  assert.equal(fs.readdirSync(f.root).length, 2);
});

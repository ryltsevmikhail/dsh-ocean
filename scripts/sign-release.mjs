#!/usr/bin/env node
// Explicit offline signing only: no key generation, file output, network,
// profile/service access, or trust-root installation.
import fs from 'node:fs';
import crypto from 'node:crypto';
import { pathToFileURL } from 'node:url';

const DOMAIN = Buffer.from('DSH Ocean release manifest v1\n');
function fail(message) { throw new Error(message); }
function readRegular(path, limit, secret = false) {
  let fd;
  try {
    fd = fs.openSync(path, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
    const before = fs.fstatSync(fd, { bigint: true });
    if (!before.isFile() || before.nlink !== 1n || before.size > BigInt(limit)) fail('input type, links or size refused');
    if (secret) {
      if (typeof process.getuid !== 'function' || before.uid !== BigInt(process.getuid())) fail('private key owner refused');
      const mode = Number(before.mode & 0o7777n);
      if (mode !== 0o600 && mode !== 0o400) fail('private key requires mode 0600 or 0400');
    }
    const buffer = Buffer.alloc(limit + 1);
    let size = 0;
    try {
      while (size < buffer.length) {
        const n = fs.readSync(fd, buffer, size, buffer.length - size, null);
        if (!n) break;
        size += n;
      }
      if (size > limit) fail('input grew beyond limit');
      const after = fs.fstatSync(fd, { bigint: true });
      if (before.size !== after.size || before.mtimeNs !== after.mtimeNs || before.ctimeNs !== after.ctimeNs || after.nlink !== 1n) fail('input changed while reading');
      return Buffer.from(buffer.subarray(0, size));
    } finally { if (secret) buffer.fill(0); }
  } catch (error) {
    // Never echo native OpenSSL/fs errors, paths, PEM data, or argument values.
    const allowed = ['input type, links or size refused', 'private key owner refused', 'private key requires mode 0600 or 0400', 'input grew beyond limit', 'input changed while reading'];
    fail(allowed.includes(error.message) ? error.message : 'cannot read signing input');
  } finally { if (fd !== undefined) fs.closeSync(fd); }
}
export function signRelease(args) {
  if (args.length !== 6) fail('usage: --manifest FILE --private-key PEM --key-id ID');
  const flags = new Map();
  for (let i = 0; i < args.length; i += 2) {
    if (!['--manifest', '--private-key', '--key-id'].includes(args[i]) || flags.has(args[i])) fail('unknown or duplicate signing option');
    flags.set(args[i], args[i + 1]);
  }
  const id = flags.get('--key-id');
  if (typeof id !== 'string' || !/^[a-z0-9_-]{1,64}$/.test(id)) fail('invalid signing key identity');
  const manifest = readRegular(flags.get('--manifest'), 65536);
  if (!manifest.length) fail('empty manifest refused');
  let privateBytes;
  try {
    privateBytes = readRegular(flags.get('--private-key'), 8192, true);
    let key;
    try { key = crypto.createPrivateKey({ key: privateBytes, format: 'pem' }); }
    catch { fail('invalid or encrypted private key PEM'); }
    if (key.asymmetricKeyType !== 'ed25519') fail('only Ed25519 private keys are accepted');
    const publicDer = crypto.createPublicKey(key).export({ format: 'der', type: 'spki' });
    const prefix = Buffer.from('302a300506032b6570032100', 'hex');
    if (publicDer.length !== 44 || !publicDer.subarray(0, 12).equals(prefix)) fail('unexpected Ed25519 public key encoding');
    const signature = crypto.sign(null, Buffer.concat([DOMAIN, manifest]), key);
    return {
      version: 1,
      signature: { version: 1, keyId: id, signatureHex: signature.toString('hex') },
      publicKey: { version: 1, keyId: id, publicKeyHex: publicDer.subarray(12).toString('hex') },
      manifestSha256: crypto.createHash('sha256').update(manifest).digest('hex'),
      publicKeySha256: crypto.createHash('sha256').update(publicDer.subarray(12)).digest('hex'),
      warning: 'Offline signature only; manifest policy was not validated. Authenticate the public key independently and run the Rust verifier before publishing. No keys, trust roots, files, releases or updates were installed.'
    };
  } finally { privateBytes?.fill(0); }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  // Broken output pipes must not escape as an unhandled Node stack trace.
  process.stdout.on('error', () => { process.exitCode = 1; });
  process.stderr.on('error', () => { process.exitCode = 1; });
  try { process.stdout.write(`${JSON.stringify(signRelease(process.argv.slice(2)), null, 2)}\n`); }
  catch (error) {
    // All expected parser/crypto errors above are coarse. Unknown failures are
    // intentionally not logged with a stack or native detail.
    const safe = new Set(['usage: --manifest FILE --private-key PEM --key-id ID', 'unknown or duplicate signing option', 'invalid signing key identity', 'empty manifest refused', 'input type, links or size refused', 'private key owner refused', 'private key requires mode 0600 or 0400', 'input grew beyond limit', 'input changed while reading', 'cannot read signing input', 'invalid or encrypted private key PEM', 'only Ed25519 private keys are accepted', 'unexpected Ed25519 public key encoding']);
    process.stderr.write(`dsh-ocean: ${safe.has(error.message) ? error.message : 'release signing failed'}\n`);
    process.exitCode = 1;
  }
}

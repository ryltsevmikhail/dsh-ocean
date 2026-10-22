import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
const file = new URL('../scripts/verify-browser.mjs',import.meta.url);
test('browser fixture refuses live service URLs and non-fixture profile roots', async () => {
 assert.ok(existsSync(file),'isolated browser verifier missing');
 const { assertBrowserFixture } = await import(file);
 assert.doesNotThrow(()=>assertBrowserFixture('http://127.0.0.1:45000','/tmp/language-pack-fixture-abc'));
 for (const address of ['http://127.0.0.1:3080','https://127.0.0.1:45000','http://example.com:45000','http://127.0.0.1:45000/path']) assert.throws(()=>assertBrowserFixture(address,'/tmp/language-pack-fixture-abc'));
 assert.throws(()=>assertBrowserFixture('http://127.0.0.1:45000','/home/user/.config/chromium'));
});

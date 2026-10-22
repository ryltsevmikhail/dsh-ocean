import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
const script = new URL('../scripts/verify-fixture.mjs',import.meta.url);
test('fixture environment isolates every known DSH and package-manager root', async () => {
 assert.ok(existsSync(script), 'isolated fixture runner missing');
 const { fixtureEnvironment } = await import(script);
 const env = fixtureEnvironment('/tmp/language-pack-test-123', { HOME:'/home/live', DSH_HOME:'/home/live/.dsh', DSH_PROFILE_DIR:'/home/live/.dsh/profiles/web', DSH_PROFILE:'web', PATH:'/bin', DSH_LAUNCH_TOKEN:'secret', XDG_CONFIG_HOME:'/home/live/.config', npm_config_prefix:'/home/live/npm' });
 assert.equal(env.HOME, '/tmp/language-pack-test-123/home');
 assert.equal(env.DSH_HOME, '/tmp/language-pack-test-123/home/.dsh');
 assert.equal(env.DSH_PROFILE, undefined);
 assert.equal(env.DSH_PROFILE_DIR, undefined);
 assert.equal(env.DSH_LAUNCH_TOKEN, undefined);
 assert.equal(env.XDG_CONFIG_HOME, '/tmp/language-pack-test-123/home/.config');
 assert.equal(env.npm_config_prefix, '/tmp/language-pack-test-123/npm-prefix');
 assert.equal(env.PATH,'/bin');
 assert.throws(()=>fixtureEnvironment('/home/live',{}), /fixture root/);
 assert.throws(()=>fixtureEnvironment('/',{}), /fixture root/);
});

test('npm pack metadata accepts array and keyed-object output with one safe artifact', async () => {
  const { parsePackOutput } = await import(script);
  assert.equal(typeof parsePackOutput, 'function');
  const entry = { filename: 'pack-0.1.0.tgz', files: [{path:'lib/client.js'}] };
  assert.deepEqual(parsePackOutput(JSON.stringify([entry])), entry);
  assert.deepEqual(parsePackOutput(JSON.stringify({'@scope/pack':entry})), entry);
  assert.throws(() => parsePackOutput('[]'));
  assert.throws(() => parsePackOutput(JSON.stringify([entry,entry])));
  assert.throws(() => parsePackOutput(JSON.stringify([{...entry,filename:'../pack.tgz'}])));
});

test('fixture startup retains its authenticated launch URL without adopting another service', async () => {
 const { parseFixtureLaunch } = await import(script);
 assert.equal(typeof parseFixtureLaunch, 'function', 'authenticated fixture startup parser missing');
 assert.equal(parseFixtureLaunch('unrelated http://127.0.0.1:3080/?token=live'), undefined);
 assert.equal(parseFixtureLaunch('dsh web: http://127.0.0.1:45000/'), undefined);
 assert.deepEqual(parseFixtureLaunch('dsh web: http://127.0.0.1:45000/?token=fixture-secret (LAN: http://10.0.0.1:45000/?token=fixture-secret)'), {address:'http://127.0.0.1:45000', launchUrl:'http://127.0.0.1:45000/?token=fixture-secret'});
});

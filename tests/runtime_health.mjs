// Run using the freshly installed Node: node --test tests/runtime_health.mjs.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const helper = fileURLToPath(new URL('../scripts/check-runtime.mjs', import.meta.url));
const source = fs.readFileSync(helper, 'utf8');
const token = 'synthetic-health-token-do-not-print';
function fixture(mode) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dsh-ocean-health-test.'));
  fs.mkdirSync(path.join(root, 'runtime/node/bin'), { recursive: true });
  fs.mkdirSync(path.join(root, 'harness/node_modules/.bin'), { recursive: true });
  fs.symlinkSync(process.execPath, path.join(root, 'runtime/node/bin/node'));
  fs.writeFileSync(path.join(root, 'harness/node_modules/.bin/dsh'), `
    const fs = require('node:fs');
    const http = require('node:http');
    const root = ${JSON.stringify(root)};
    const mode = ${JSON.stringify(mode)};
    if (process.argv.includes('--version')) { console.log('0.2.1'); process.exit(0); }
    const probe = {pid: process.pid, home: process.env.HOME, dshHome: process.env.DSH_HOME, cwd: process.cwd(), args: process.argv.slice(2), telemetryDisabled: process.env.DSH_TELEMETRY_DISABLED, requests: []};
    const save = () => fs.writeFileSync(root + '/probe.json', JSON.stringify(probe));
    save();
    if (mode === 'exit') process.exit(2);
    if (mode === 'unsafe') {
      console.log('dsh web: http://127.0.0.1:3080/?token=${token}');
      setInterval(() => {}, 1000);
    } else {
      const server = http.createServer((req, res) => {
        probe.requests.push(req.url); save();
        res.statusCode = mode === 'wrong-status' ? 200 : (req.url.includes('token=') ? 303 : 401);
        res.end();
      });
      server.listen(0, '127.0.0.1', () => {
        // Split the launch line to exercise incremental parsing.
        process.stdout.write('dsh we');
        setTimeout(() => console.log('b: http://127.0.0.1:' + server.address().port + '/?token=${token}'), 10);
      });
    }
  `);
  fs.mkdirSync(path.join(root, 'tools/node_modules/.bin'), {recursive:true});
  fs.writeFileSync(path.join(root, 'tools/node_modules/.bin/pnpm'), "console.log('10.34.7');\n");
  for (const name of ['node-pty','koffi']) {
    const directory=path.join(root,'harness/node_modules/@deepseek-ai/dsh/node_modules',name);
    fs.mkdirSync(directory,{recursive:true});
    fs.writeFileSync(path.join(directory,'index.js'), `const fs=require('node:fs');fs.writeFileSync(${JSON.stringify(path.join(root,`${name}-home`))},process.env.HOME);module.exports={};`);
  }
  return {
    root,
    run(embedded = false, expected = null) {
      const args = expected ? ['--expected-components', JSON.stringify(expected), root] : [root];
      return spawnSync(process.execPath, embedded
        ? ['--input-type=module', '-e', source, '--', ...args] : [helper, ...args],
      { encoding: 'utf8', timeout: 12000 });
    },
    probe() { return JSON.parse(fs.readFileSync(path.join(root, 'probe.json'), 'utf8')); },
    cleanup() {
      // Defensive cleanup if the helper regresses and leaves a detached child.
      if (fs.existsSync(path.join(root, 'probe.json'))) {
        try { process.kill(-this.probe().pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
      }
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

for (const embedded of [false, true]) {
  test(`authenticated health check is isolated, cleaned up and redacted (embedded=${embedded})`, () => {
    const f = fixture('ok');
    try {
      const result = f.run(embedded);
      assert.equal(result.status, 0, result.stderr);
      assert.ok(!`${result.stdout}${result.stderr}`.includes(token));
      const probe = f.probe();
      assert.equal(probe.home, probe.cwd);
      assert.deepEqual(probe.args, ['--profile', 'web', '--host', '127.0.0.1', '--port', '0', '--no-open']);
      assert.equal(probe.telemetryDisabled, '1');
      assert.equal(probe.dshHome, path.join(probe.home, '.dsh'));
      assert.notEqual(probe.home, os.homedir());
      assert.ok(!fs.existsSync(probe.home), 'temporary profile removed');
      assert.throws(() => process.kill(probe.pid, 0), { code: 'ESRCH' }, 'candidate process reaped');
      assert.equal(probe.requests[0], '/');
      assert.ok(probe.requests[1].includes('token='));
    } finally { f.cleanup(); }
  });
}

for (const mode of ['unsafe', 'wrong-status', 'exit']) {
  test(`rejects ${mode} and cleans up the candidate`, () => {
    const f = fixture(mode);
    try {
      const result = f.run();
      assert.equal(result.status, 1, result.stderr);
      assert.ok(!`${result.stdout}${result.stderr}`.includes(token));
      const probe = f.probe();
      assert.ok(!fs.existsSync(probe.home));
      assert.throws(() => process.kill(probe.pid, 0), { code: 'ESRCH' });
    } finally { f.cleanup(); }
  });
}

for (const embedded of [false,true]) {
  test(`signed component checks run in disposable home before auth health (embedded=${embedded})`,()=>{
    const f=fixture('ok');try{
      const result=f.run(embedded,{node:process.versions.node,dsh:'0.2.1',pnpm:'10.34.7'});
      assert.equal(result.status,0,result.stderr);
      for(const name of ['node-pty','koffi']) {
        const home=fs.readFileSync(path.join(f.root,`${name}-home`),'utf8');
        assert.notEqual(home,os.homedir());assert.equal(home,f.probe().home);assert.ok(!fs.existsSync(home));
      }
    }finally{f.cleanup();}
  });
}
for (const bad of ['node','dsh','pnpm','native']) {
  test(`rejects signed component/native mismatch ${bad} before server launch`,()=>{
    const f=fixture('ok');try{
      const expected={node:process.versions.node,dsh:'0.2.1',pnpm:'10.34.7'};
      if(bad==='native')fs.writeFileSync(path.join(f.root,'harness/node_modules/@deepseek-ai/dsh/node_modules/koffi/index.js'),"throw new Error('synthetic-secret-do-not-print');");
      else expected[bad]='99.99.99';
      const result=f.run(false,expected);assert.equal(result.status,1,result.stderr);
      assert.ok(!fs.existsSync(path.join(f.root,'probe.json')),'incompatible candidate must not start real server health');
      assert.ok(!`${result.stdout}${result.stderr}`.includes('synthetic-secret-do-not-print'));
    }finally{f.cleanup();}
  });
}

test('missing Node fails promptly', () => {
  const f = fixture('ok');
  try {
    fs.unlinkSync(path.join(f.root, 'runtime/node/bin/node'));
    const result = f.run();
    assert.equal(result.status, 1, result.stderr);
    assert.ok(result.stderr.includes('could not start candidate DSH'));
  } finally { f.cleanup(); }
});

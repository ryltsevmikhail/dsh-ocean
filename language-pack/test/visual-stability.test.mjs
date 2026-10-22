import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runInNewContext } from 'node:vm';

test('title readability rejects hidden text but accepts short labels and CSS subpixel rounding', async () => {
  const { isPresetTitleReadable } = await import('../scripts/verify-browser.mjs');
  assert.equal(typeof isPresetTitleReadable, 'function');
  assert.equal(isPresetTitleReadable(null), false);
  assert.equal(isPresetTitleReadable({width:0,scrollWidth:150,text:'Standard mode'}), false);
  assert.equal(isPresetTitleReadable({width:20,scrollWidth:150,text:'Standard mode'}), false);
  assert.equal(isPresetTitleReadable({width:59.375,scrollWidth:60,text:'표준 모드'}), true);
  assert.equal(isPresetTitleReadable({width:45,scrollWidth:45,text:'Mode'}), true);
  assert.equal(isPresetTitleReadable({width:90,scrollWidth:150,text:'Standard mode'}), true);
});

test('screenshots await finite enter animations and final paint, not infinite spinners', async () => {
  const { waitForVisualStability } = await import('../scripts/verify-browser.mjs');
  assert.equal(typeof waitForVisualStability, 'function');
  let finish, frames = 0, completed = false;
  const animation = { playState: 'running', effect: { getTiming: () => ({iterations:1}) }, finished: new Promise(resolve => {finish=resolve;}) };
  const context = {
    document: { getAnimations: () => [animation, {playState:'running',effect:{getTiming:()=>({iterations:Infinity})},finished:new Promise(()=>{})}] },
    requestAnimationFrame: callback => {frames++; queueMicrotask(() => callback(0));},
    setTimeout, clearTimeout,
  };
  const pending = waitForVisualStability({evaluate: expression => runInNewContext(expression, context)}).then(() => {completed=true;});
  await new Promise(setImmediate);
  assert.equal(completed, false, 'must not capture while guide enter animation is active');
  animation.playState='finished'; finish(); await pending;
  assert.equal(completed,true);
  assert.ok(frames>=4,'wait for layout before and final paint after animations');
});

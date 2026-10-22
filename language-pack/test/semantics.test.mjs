import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const catalog = id => JSON.parse(readFileSync(new URL(`../data/${id}.json`, import.meta.url)));

test('Russian task write scopes describe planned changes, not enforced permissions', () => {
  assert.equal(catalog('ru').conversation['detail.field.writeScopes'], 'Планируемые области изменений');
});

test('Brazilian Portuguese uses integer grouping and invariant Portuguese time abbreviations', () => {
  const pt = catalog('pt-BR');
  assert.equal(pt.chat['number.groupSeparator'], '.');
  assert.equal(pt.workspace['time.months'], '{n} m.');
  assert.equal(pt.workspace['time.years'], '{n} a.');
});

test('German inactive runtime state does not imply a plugin never started', () => {
  const de = catalog('de');
  assert.equal(de.pluginManager.rowStateIdle, 'Wird nicht ausgeführt');
  assert.equal(de['settings.pluginInventory'].unobserved, 'Wird nicht ausgeführt');
});

test('Korean distinguishes persisted script permission from history and ended from successful work', () => {
  const ko = catalog('ko');
  assert.equal(ko.conversation['detail.receipt.alreadyFinished'], '이미 종료됨');
  assert.equal(ko.pluginManager.installApprovalConsequence, '허용하면 스크립트가 이 환경에서 사용자 권한으로 실행되며, 실행 허용 권한은 이 프로필에 저장됩니다.');
});

test('Spanish hierarchy and counters do not imply a main session or fixed plural counts', () => {
  const es = catalog('es');
  assert.equal(es.conversation['detail.field.parent'], 'Sesión padre');
  assert.equal(es.conversation['queue.count'], 'Mensajes en cola: {n}');
  assert.equal(es.chat['stats.counts'], 'Turnos: {turns} · pasos: {steps}');
});

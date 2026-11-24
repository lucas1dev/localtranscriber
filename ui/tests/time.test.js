import { test } from 'node:test';
import assert from 'node:assert/strict';
import { shiftTimestamp } from '../js/time.js';

test('soma segundos simples', () => {
  assert.equal(shiftTimestamp('00:00:01,230', 12), '00:00:13,230');
});
test('vira a hora', () => {
  assert.equal(shiftTimestamp('00:59:59,500', 1), '01:00:00,500');
});
test('deslocamento zero mantém', () => {
  assert.equal(shiftTimestamp('00:00:00,000', 0), '00:00:00,000');
});

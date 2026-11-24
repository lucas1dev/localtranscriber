import test from 'node:test';
import assert from 'node:assert/strict';
import { formatBytes } from '../js/format.js';

test('formatBytes usa MB abaixo de 1 GB e GB acima, em pt-BR', () => {
  assert.equal(formatBytes(147951465), '148 MB');
  assert.equal(formatBytes(487601967), '488 MB');
  assert.equal(formatBytes(1533763059), '1,5 GB');
  assert.equal(formatBytes(1624555275), '1,6 GB');
});

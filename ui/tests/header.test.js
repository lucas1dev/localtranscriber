import test from 'node:test';
import assert from 'node:assert/strict';
import { JOB_LABEL } from '../js/views/header.js';

test('todo tipo de trabalho tem rótulo no cabeçalho', () => {
  for (const kind of ['file', 'mic', 'summary', 'compare']) assert.ok(JOB_LABEL[kind], kind);
  assert.equal(JOB_LABEL.compare, 'Comparando…');
});

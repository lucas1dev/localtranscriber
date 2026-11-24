import test from 'node:test';
import assert from 'node:assert/strict';
import { JOB_LABEL, jobLabel } from '../js/views/header.js';

test('todo tipo de trabalho tem rótulo no cabeçalho', () => {
  for (const kind of ['file', 'mic', 'summary', 'compare']) assert.ok(JOB_LABEL[kind], kind);
  assert.equal(JOB_LABEL.compare, 'Comparando…');
});

test('jobLabel: gravação parando mostra Finalizando…', () => {
  assert.equal(jobLabel({ kind: 'mic', percent: null }), 'Gravando…');
  assert.equal(jobLabel({ kind: 'mic', percent: null, stopping: true }), 'Finalizando…');
  assert.equal(jobLabel({ kind: 'file', percent: 42 }), 'Transcrevendo… 42%');
  assert.equal(jobLabel({ kind: 'file', percent: null }), 'Transcrevendo…');
  assert.equal(jobLabel({ kind: 'summary', percent: 10 }), 'Resumindo…');
  assert.equal(jobLabel({ kind: 'x' }), 'Processando…');
});

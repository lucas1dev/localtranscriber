import test from 'node:test';
import assert from 'node:assert/strict';
import { ollamaDefaultModel, pickStyle } from '../js/summary-options.js';
import { mergeSettings } from '../js/flow.js';

test('pickStyle usa o formato salvo, válido', () => {
  assert.equal(pickStyle(null, { summary_style: 'meeting' }, false), 'meeting');
  assert.equal(pickStyle(null, { summary_style: 'inexistente' }, false), 'executive');
  assert.equal(pickStyle('podcast', { summary_style: 'meeting' }, true), 'podcast');
});

test('pickStyle sem configurações: espera enquanto oculto, executivo ao aparecer', () => {
  assert.equal(pickStyle(null, null, false), null);
  assert.equal(pickStyle(null, null, true), 'executive');
});

test('ollamaDefaultModel: padrão instalado fica; senão o primeiro instalado; nenhum = null', () => {
  assert.equal(ollamaDefaultModel(['llama3.2:3b', 'qwen3:8b']), null);
  assert.equal(ollamaDefaultModel(['llama3.2:3b', 'gemma3:4b']), 'llama3.2:3b');
  assert.equal(ollamaDefaultModel([]), null);
});

test('mergeSettings não salva por cima de configurações que não carregaram', () => {
  assert.deepEqual(mergeSettings({ a: 1, b: 2 }, { b: 3 }), { a: 1, b: 3 });
  assert.equal(mergeSettings(null, { b: 3 }), null);
  assert.equal(mergeSettings(undefined, { b: 3 }), null);
});

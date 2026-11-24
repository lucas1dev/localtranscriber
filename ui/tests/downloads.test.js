import test from 'node:test';
import assert from 'node:assert/strict';
import { createStore } from '../js/store.js';
import { clearEntry, initDownloadEvents, startEntry } from '../js/downloads.js';

function setup() {
  const store = createStore({ downloads: {}, pulls: {}, ollamaTick: 0, settings: null, models: [] });
  const handlers = {};
  initDownloadEvents(store, async (event, cb) => { handlers[event] = cb; });
  return { store, emit: (e, p) => handlers[e](p) };
}

test('progress atualiza a entrada', () => {
  const { store, emit } = setup();
  emit('model://progress', { id: 'base', done: 5, total: 10 });
  assert.deepEqual(store.get().downloads.base, { done: 5, total: 10, error: null });
});

test('done cancelled remove sem erro', () => {
  const { store, emit } = setup();
  emit('model://progress', { id: 'base', done: 5, total: 10 });
  emit('model://done', { id: 'base', ok: false, error: { kind: 'cancelled', message: 'Cancelado' } });
  assert.deepEqual(store.get().downloads, {});
});

test('done com erro mantém a mensagem', () => {
  const { store, emit } = setup();
  emit('model://progress', { id: 'base', done: 5, total: 10 });
  emit('model://done', { id: 'base', ok: false, error: { kind: 'network', message: 'Sem rede' } });
  assert.equal(store.get().downloads.base.error, 'Sem rede');
});

test('done ok remove a entrada (e tenta atualizar os modelos)', () => {
  const { store, emit } = setup();
  emit('model://progress', { id: 'base', done: 10, total: 10 });
  emit('model://done', { id: 'base', ok: true, error: null });
  assert.deepEqual(store.get().downloads, {});
});

test('done que chega antes do start não é sobrescrito', () => {
  const { store, emit } = setup();
  assert.equal(startEntry(store, 'downloads', 'base'), true); // escrito antes do invoke
  emit('model://done', { id: 'base', ok: false, error: { kind: 'network', message: 'Sem rede' } });
  assert.equal(startEntry(store, 'downloads', 'base'), true); // erro antigo pode ser refeito
  emit('model://done', { id: 'base', ok: false, error: { kind: 'network', message: 'Sem rede' } });
  assert.equal(store.get().downloads.base.error, 'Sem rede');
  // um download ativo não é sobrescrito
  emit('model://progress', { id: 'small', done: 3, total: 9 });
  assert.equal(startEntry(store, 'downloads', 'small'), false);
  assert.equal(store.get().downloads.small.done, 3);
});

test('ollama: done ok remove e incrementa ollamaTick; erro fica', () => {
  const { store, emit } = setup();
  emit('ollama://progress', { name: 'q', status: 'pulling', completed: 1, total: 2 });
  emit('ollama://done', { name: 'q', ok: true, error: null });
  assert.deepEqual(store.get().pulls, {});
  assert.equal(store.get().ollamaTick, 1);
  emit('ollama://done', { name: 'z', ok: false, error: { kind: 'x', message: 'falhou' } });
  assert.equal(store.get().pulls.z.error, 'falhou');
});

test('clearEntry remove só a chave pedida', () => {
  const { store } = setup();
  startEntry(store, 'pulls', 'a');
  startEntry(store, 'pulls', 'b');
  clearEntry(store, 'pulls', 'a');
  assert.deepEqual(Object.keys(store.get().pulls), ['b']);
});

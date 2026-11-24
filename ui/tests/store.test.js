import test from 'node:test';
import assert from 'node:assert/strict';
import { createStore } from '../js/store.js';
import { errorMessage, isCancelled } from '../js/api.js';

test('store notifica e faz merge raso', () => {
  const s = createStore({ a: 1, b: 2 });
  let seen;
  s.subscribe((v) => { seen = v; });
  s.set({ b: 3 });
  assert.deepEqual(seen, { a: 1, b: 3 });
  assert.deepEqual(s.get(), { a: 1, b: 3 });
});

test('unsubscribe para de notificar', () => {
  const s = createStore({ a: 1 });
  let calls = 0;
  const off = s.subscribe(() => { calls += 1; });
  s.set({ a: 2 });
  off();
  s.set({ a: 3 });
  assert.equal(calls, 1);
});

test('errorMessage cobre string, objeto e outros', () => {
  assert.equal(errorMessage('falhou'), 'falhou');
  assert.equal(errorMessage({ kind: 'user', message: 'Arquivo inválido' }), 'Arquivo inválido');
  assert.equal(errorMessage(42), '42');
});

test('isCancelled', () => {
  assert.equal(isCancelled({ kind: 'cancelled', message: '' }), true);
  assert.equal(isCancelled({ kind: 'user', message: 'x' }), false);
  assert.equal(isCancelled(undefined), false);
});

import test from 'node:test';
import assert from 'node:assert/strict';
import { configuredProviders, initialProvider, usableProviders } from '../js/providers.js';

const p = (id, configured, enabled = true, kind = 'api_key') => ({ id, label: id, kind, configured, enabled, detail: '' });

test('claude_code desabilitado some, mesmo com kind subscription', () => {
  const list = [p('codex', true, true, 'subscription'), p('claude_code', true, false, 'subscription')];
  assert.deepEqual(usableProviders(list).map((x) => x.id), ['codex']);
});

test('claude_code habilitado e configurado aparece', () => {
  const list = [p('claude_code', true, true, 'subscription'), p('openai', false)];
  assert.deepEqual(usableProviders(list).map((x) => x.id), ['claude_code', 'openai']);
  assert.deepEqual(configuredProviders(list).map((x) => x.id), ['claude_code']);
});

test('initialProvider: salvo configurado vence', () => {
  assert.equal(initialProvider([p('openai', true), p('gemini', true)], 'gemini'), 'gemini');
});

test('initialProvider: salvo não configurado cai no primeiro configurado', () => {
  assert.equal(initialProvider([p('openai', false), p('gemini', true)], 'openai'), 'gemini');
});

test('initialProvider: nenhum configurado dá null', () => {
  assert.equal(initialProvider([p('openai', false)], 'openai'), null);
});

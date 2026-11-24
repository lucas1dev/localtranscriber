import test from 'node:test';
import assert from 'node:assert/strict';
import { buildCompareRequest, buildSummaryRequest } from '../js/requests.js';

const state = { transcript: { text: 'texto da reunião', segments: [] } };

test('buildSummaryRequest: formato snake_case, modelo vazio vira null', () => {
  assert.deepEqual(
    buildSummaryRequest(state, { provider: 'openai', style: 'executive', model: '  ', customInstructions: 'ignorado' }),
    { text: 'texto da reunião', provider: 'openai', style: 'executive', model: null, custom_instructions: '' },
  );
});

test('buildSummaryRequest: modelo aparado e instruções só no formato custom', () => {
  assert.deepEqual(
    buildSummaryRequest(state, { provider: 'ollama', style: 'custom', model: ' qwen3:8b ', customInstructions: 'foco em prazos' }),
    { text: 'texto da reunião', provider: 'ollama', style: 'custom', model: 'qwen3:8b', custom_instructions: 'foco em prazos' },
  );
});

test('buildCompareRequest: models inclui só o provedor do seletor com modelo preenchido', () => {
  assert.deepEqual(
    buildCompareRequest(state, {
      providers: ['openai', 'gemini'], provider: 'gemini', style: 'detailed', model: ' gemini-2.5-pro ', customInstructions: '',
    }),
    {
      text: 'texto da reunião', providers: ['openai', 'gemini'], style: 'detailed',
      models: { gemini: 'gemini-2.5-pro' }, custom_instructions: '',
    },
  );
});

test('buildCompareRequest: sem modelo ou provedor do seletor fora da seleção, models é vazio', () => {
  const base = { providers: ['openai', 'gemini'], style: 'executive', customInstructions: '' };
  assert.deepEqual(buildCompareRequest(state, { ...base, provider: 'gemini', model: '' }).models, {});
  assert.deepEqual(buildCompareRequest(state, { ...base, provider: 'ollama', model: 'x' }).models, {});
});

test('buildCompareRequest: modelo padrão alternativo do Ollama entra quando nada foi digitado para ele', () => {
  const base = { providers: ['openai', 'ollama'], style: 'executive', customInstructions: '', defaults: { ollama: 'llama3.2:3b' } };
  assert.deepEqual(buildCompareRequest(state, { ...base, provider: 'openai', model: '' }).models, { ollama: 'llama3.2:3b' });
  assert.deepEqual(buildCompareRequest(state, { ...base, provider: 'ollama', model: 'gemma3:4b' }).models, { ollama: 'gemma3:4b' });
  assert.deepEqual(buildCompareRequest(state, { ...base, providers: ['openai', 'gemini'], provider: 'openai', model: '' }).models, {});
});

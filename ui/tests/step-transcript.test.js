import test from 'node:test';
import assert from 'node:assert/strict';
import { segmentText } from '../js/views/step-transcript.js';

test('segmentos da gravação dupla mostram Você/Reunião', () => {
  assert.equal(segmentText({ start: '00:00:01,000', text: 'oi', speaker: 'voce' }), 'Você: oi');
  assert.equal(segmentText({ start: '00:00:02,000', text: 'tudo bem', speaker: 'reuniao' }), 'Reunião: tudo bem');
  assert.equal(segmentText({ start: '00:00:03,000', text: 'sem etiqueta', speaker: null }), 'sem etiqueta');
  assert.equal(segmentText({ start: '00:00:04,000', text: 'arquivo' }), 'arquivo');
});

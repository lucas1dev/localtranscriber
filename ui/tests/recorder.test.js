import test from 'node:test';
import assert from 'node:assert/strict';
import {
  SPEAKER_LABEL, liveLine, levelPercent, pickSource, showMicPicker, showHeadphoneHint, pickMicId,
} from '../js/recorder.js';

test('SPEAKER_LABEL', () => assert.deepEqual(SPEAKER_LABEL, { voce: 'Você', reuniao: 'Reunião' }));

test('liveLine com e sem etiqueta', () => {
  assert.equal(liveLine({ speaker: 'voce', text: 'texto' }), 'Você: texto');
  assert.equal(liveLine({ speaker: 'reuniao', text: 'oi' }), 'Reunião: oi');
  assert.equal(liveLine({ speaker: null, text: 'só texto' }), 'só texto');
});

test('levelPercent faz clamp e arredonda', () => {
  assert.deepEqual([-1, 0, 0.5, 2].map(levelPercent), [0, 0, 50, 100]);
  assert.equal(levelPercent(null), 0);
  assert.equal(levelPercent(0.333), 33);
});

test('pickSource', () => {
  assert.equal(pickSource(null), 'mic');
  assert.equal(pickSource({ record_source: 'x' }), 'mic');
  assert.equal(pickSource({ record_source: 'both' }), 'both');
  assert.equal(pickSource({ record_source: 'system' }), 'system');
});

test('showMicPicker e showHeadphoneHint', () => {
  assert.equal(showMicPicker('system'), false);
  assert.equal(showMicPicker('mic'), true);
  assert.equal(showMicPicker('both'), true);
  assert.equal(showHeadphoneHint('both'), true);
  assert.equal(showHeadphoneHint('mic'), false);
  assert.equal(showHeadphoneHint('system'), false);
});

test('pickMicId usa o salvo se existir, senão o padrão', () => {
  const list = [{ id: 'a', is_default: true }, { id: 'b', is_default: false }];
  assert.equal(pickMicId(list, 'b'), 'b');
  assert.equal(pickMicId(list, 'zzz'), 'a');
  assert.equal(pickMicId(list, null), 'a');
  assert.equal(pickMicId([{ id: 'x', is_default: false }], null), 'x');
  assert.equal(pickMicId([], 'b'), null);
});

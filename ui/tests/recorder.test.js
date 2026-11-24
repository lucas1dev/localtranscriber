import test from 'node:test';
import assert from 'node:assert/strict';
import {
  SPEAKER_LABEL, liveLine, levelPercent, pickSource, showMicPicker, showHeadphoneHint, pickMicId, createRecorder,
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

function fakeDeps() {
  const handlers = {};
  const calls = [];
  const off = [];
  let releaseStart;
  const deps = {
    on: async (event, cb) => { handlers[event] = cb; return () => off.push(event); },
    api: {
      startRecording: (...a) => { calls.push(['start', ...a]); return new Promise((r) => { releaseStart = r; }); },
      stopRecording: async () => { calls.push(['stop']); return { text: 'x' }; },
    },
  };
  return { deps, handlers, calls, off, release: () => releaseStart() };
}

test('erro durante a abertura fica na fila: aviso já, coleta só depois do start', async () => {
  const f = fakeDeps();
  const seen = [];
  const rec = createRecorder({ onText() {}, onLevel() {}, onError: (m, deferred) => seen.push([m, deferred]) }, f.deps);
  const started = rec.start({ source: 'mic', micId: null, model: 'm', language: 'pt' });
  await new Promise((r) => setImmediate(r));
  assert.equal(rec.starting, true);
  f.handlers['record://error']({ message: 'mic caiu' });
  assert.deepEqual(seen, [['mic caiu', true]]);
  assert.deepEqual(f.calls.filter((c) => c[0] === 'stop'), []);
  f.release();
  assert.equal(await started, 'mic caiu');
  assert.equal(rec.starting, false);
  await rec.stop();
  assert.deepEqual(f.calls.map((c) => c[0]), ['start', 'stop']);
});

test('erro depois da abertura não é adiado', async () => {
  const f = fakeDeps();
  const seen = [];
  const rec = createRecorder({ onText() {}, onLevel() {}, onError: (m, d) => seen.push([m, d]) }, f.deps);
  const started = rec.start({ source: 'both', micId: 'a', model: 'm', language: 'pt' });
  await new Promise((r) => setImmediate(r));
  f.release();
  assert.equal(await started, null);
  f.handlers['record://error']({ message: 'caiu' });
  assert.deepEqual(seen, [['caiu', false]]);
});

test('eventos fora da gravação são ignorados e os ouvintes saem no stop', async () => {
  const f = fakeDeps();
  const got = [];
  const rec = createRecorder({ onText: (p) => got.push(p), onLevel: (p) => got.push(p), onError: (m) => got.push(m) }, f.deps);
  const started = rec.start({ source: 'mic', micId: null, model: 'm', language: 'pt' });
  await new Promise((r) => setImmediate(r));
  f.release();
  await started;
  f.handlers['record://text']({ speaker: null, text: 'a' });
  await rec.stop();
  assert.deepEqual(f.off.sort(), ['record://error', 'record://level', 'record://text']);
  f.handlers['record://text']({ speaker: null, text: 'b' });
  f.handlers['record://level']({ mic: 1 });
  f.handlers['record://error']({ message: 'z' });
  assert.deepEqual(got, [{ speaker: null, text: 'a' }]);
});

test('start que falha solta os ouvintes e repassa o erro', async () => {
  const f = fakeDeps();
  f.deps.api.startRecording = async () => { throw new Error('boom'); };
  const rec = createRecorder({ onText() {}, onLevel() {}, onError() {} }, f.deps);
  await assert.rejects(rec.start({ source: 'mic', micId: null, model: 'm', language: 'pt' }), /boom/);
  assert.equal(f.off.length, 3);
  assert.equal(rec.starting, false);
});

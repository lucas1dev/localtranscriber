import test from 'node:test';
import assert from 'node:assert/strict';
import {
  SPEAKER_LABEL, liveLine, levelPercent, pickSource, showMicPicker, showHeadphoneHint, pickMicId, createRecorder,
  isMacUserAgent, silenceHint, showSilenceHint, SILENCE_HINT, SILENCE_HINT_MS, afterRecording, recordingResult, recordingFailure,
} from '../js/recorder.js';
import { afterTranscribe, initialState } from '../js/flow.js';

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

test('isMacUserAgent reconhece o WebKit do macOS', () => {
  assert.equal(isMacUserAgent('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15'), true);
  assert.equal(isMacUserAgent('Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/120'), false);
  assert.equal(isMacUserAgent(undefined), false);
});

test('silenceHint: Reunião em zero por 12 s desde o começo', () => {
  assert.equal(SILENCE_HINT_MS, 12_000);
  assert.equal(silenceHint({ peak: 0, elapsedMs: 11_999 }), false);
  assert.equal(silenceHint({ peak: 0, elapsedMs: 12_000 }), true);
  assert.equal(silenceHint({ peak: 0.009, elapsedMs: 30_000 }), true);
  // Some assim que o nível sobe, e não volta.
  assert.equal(silenceHint({ peak: 0.01, elapsedMs: 30_000 }), false);
  assert.equal(silenceHint({ peak: 0.5, elapsedMs: 12_000 }), false);
});

test('silenceHint só no macOS e com o som do computador', () => {
  assert.equal(showSilenceHint({ mac: true, source: 'system', peak: 0, elapsedMs: 13_000 }), true);
  assert.equal(showSilenceHint({ mac: true, source: 'both', peak: 0, elapsedMs: 13_000 }), true);
  assert.equal(showSilenceHint({ mac: true, source: 'mic', peak: 0, elapsedMs: 13_000 }), false);
  assert.equal(showSilenceHint({ mac: false, source: 'system', peak: 0, elapsedMs: 13_000 }), false);
  assert.equal(SILENCE_HINT, 'Nada do som do computador até agora. Se algo está tocando, confira Ajustes do Sistema → Privacidade e Segurança → Gravação de áudio do sistema.');
});

test('afterRecording: sem fala no macOS com som do computador lembra a permissão', () => {
  const empty = { text: '', segments: [] };
  const job = { ...initialState, job: { kind: 'mic' } };
  const hint = 'Nenhuma fala detectada no áudio. Se o som do computador não foi captado, confira a permissão em Ajustes do Sistema → Privacidade e Segurança → Gravação de áudio do sistema.';
  assert.equal(afterRecording(job, empty, { mac: true, source: 'system' }).error, hint);
  assert.equal(afterRecording(job, empty, { mac: true, source: 'both' }).error, hint);
  assert.equal(afterRecording(job, empty, { mac: true, source: 'mic' }).error, 'Nenhuma fala detectada no áudio.');
  assert.equal(afterRecording(job, empty, { mac: false, source: 'both' }).error, 'Nenhuma fala detectada no áudio.');
  // Com fala, igual ao arquivo.
  const t = { text: 'oi', segments: [] };
  assert.deepEqual(afterRecording(job, t, { mac: true, source: 'both' }), afterTranscribe(job, t));
});

test('erro da sessão durante o stop da pessoa chega ao onError e sobrevive ao resultado', async () => {
  const f = fakeDeps();
  let release;
  f.deps.api.stopRecording = () => new Promise((r) => { release = r; });
  let sessionError = null;
  const rec = createRecorder({ onText() {}, onLevel() {}, onError: (m) => { sessionError = m; } }, f.deps);
  const started = rec.start({ source: 'mic', micId: null, model: 'm', language: 'pt' });
  await new Promise((r) => setImmediate(r));
  f.release();
  await started;
  const stopping = rec.stop();
  f.handlers['record://error']({ message: 'O microfone foi desconectado.' });
  release({ text: 'oi', segments: [] });
  const result = await stopping;
  const job = { ...initialState, job: { kind: 'mic' } };
  const patch = recordingResult(job, result, { mac: false, source: 'mic', sessionError });
  assert.equal(patch.error, 'O microfone foi desconectado.');
  assert.equal(patch.step, 2);
  // Sem erro da sessão, o resultado normal.
  assert.deepEqual(recordingResult(job, result, { mac: false, source: 'mic', sessionError: null }), afterTranscribe(job, result));
});

test('recordingFailure junta o erro da sessão e o do stop', () => {
  assert.equal(recordingFailure(null, 'falhou'), 'falhou');
  assert.equal(recordingFailure('mic caiu', 'falhou'), 'mic caiu (falhou)');
});

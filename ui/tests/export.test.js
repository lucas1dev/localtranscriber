import test from 'node:test';
import assert from 'node:assert/strict';
import { saveText, toSrt } from '../js/export.js';

test('toSrt formata índice, intervalo e texto', () => assert.equal(
  toSrt([
    { start: '00:00:00,000', end: '00:00:02,500', text: 'Olá' },
    { start: '00:00:02,500', end: '00:00:04,000', text: 'mundo' },
  ]),
  '1\n00:00:00,000 --> 00:00:02,500\nOlá\n\n2\n00:00:02,500 --> 00:00:04,000\nmundo',
));

test('toSrt vazio', () => assert.equal(toSrt([]), ''));

function fakeTauri(savePath) {
  const calls = { save: [], invoke: [] };
  globalThis.window = {
    __TAURI__: {
      dialog: { save: async (opts) => { calls.save.push(opts); return savePath; } },
      core: { invoke: async (cmd, args) => { calls.invoke.push([cmd, args]); } },
    },
  };
  return calls;
}

test('saveText pergunta onde salvar e grava o conteúdo', async () => {
  const calls = fakeTauri('/tmp/x/transcricao.txt');
  assert.equal(await saveText('transcricao.txt', 'Olá'), true);
  assert.deepEqual(calls.save, [{ defaultPath: 'transcricao.txt', filters: [{ name: 'Texto', extensions: ['txt'] }] }]);
  assert.deepEqual(calls.invoke, [['save_text_file', { path: '/tmp/x/transcricao.txt', contents: 'Olá' }]]);
});

test('saveText usa o filtro da extensão (srt, md)', async () => {
  const calls = fakeTauri('/tmp/a');
  await saveText('transcricao.srt', '1');
  await saveText('resumo-openai.md', '# r');
  assert.deepEqual(calls.save.map((o) => o.filters), [
    [{ name: 'Legendas SRT', extensions: ['srt'] }],
    [{ name: 'Markdown', extensions: ['md'] }],
  ]);
});

test('saveText cancelado não grava nem dá erro', async () => {
  const calls = fakeTauri(null);
  assert.equal(await saveText('transcricao.txt', 'Olá'), false);
  assert.deepEqual(calls.invoke, []);
});

test('toSrt etiqueta a fala quando há speaker', () => {
  assert.equal(
    toSrt([
      { start: '00:00:00,000', end: '00:00:01,000', text: 'Oi.', speaker: 'reuniao' },
      { start: '00:00:01,000', end: '00:00:02,000', text: 'Olá.', speaker: 'voce' },
      { start: '00:00:02,000', end: '00:00:03,000', text: 'Sem.', speaker: null },
    ]),
    '1\n00:00:00,000 --> 00:00:01,000\nReunião: Oi.\n\n2\n00:00:01,000 --> 00:00:02,000\nVocê: Olá.\n\n3\n00:00:02,000 --> 00:00:03,000\nSem.',
  );
});

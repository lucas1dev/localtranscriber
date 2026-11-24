import test from 'node:test';
import assert from 'node:assert/strict';
import {
  initialState, goTo, openSettings, closeSettings, canStartJob, startJob,
  afterTranscribe, pasteText, needsReplaceConfirm, failJob,
} from '../js/flow.js';

test('não pula para passo não alcançado', () => assert.deepEqual(goTo({...initialState}, 3), {}));
test('silêncio mantém no passo 1 com erro', () => assert.deepEqual(afterTranscribe({...initialState, job:{kind:'transcribe'}}, {text:'', segments:[]}), {job:null, step:1, error:'Nenhuma fala detectada no áudio.'}));
test('transcrição avança e libera passo 2', () => { const p = afterTranscribe({...initialState}, {text:'oi', segments:[{start:'',end:'',text:'oi'}]}); assert.equal(p.step, 2); assert.equal(p.maxStep, 2); });
test('cannot_start_second_job', () => { const s = {...initialState, job:{kind:'transcribe', percent:10}}; assert.equal(canStartJob(s), false); assert.equal(startJob(s,'summary').error, 'Aguarde o processamento atual terminar.'); });
test('navigation_keeps_job', () => { const s = {...initialState, step:2, maxStep:3, job:{kind:'summary', percent:null}}; const p1 = openSettings(s); assert.equal(p1.job, undefined); assert.deepEqual(goTo({...s, ...p1}, 1), {step:1, error:null}); });
test('texto editado pede confirmação antes de substituir', () => assert.equal(needsReplaceConfirm({...initialState, transcript:{text:'x'}, edited:true}), true));
test('colar texto vai ao passo 2', () => assert.equal(pasteText(initialState, 'abc').step, 2));

test('openSettings e closeSettings mudam só view', () => {
  const s = {...initialState, step:2, maxStep:2, job:{kind:'summary', percent:null}};
  assert.deepEqual(openSettings(s), {view:'settings'});
  assert.deepEqual(closeSettings({...s, view:'settings'}), {view:'steps'});
});

test('failJob limpa o job e mostra a mensagem', () => {
  const s = {...initialState, job:{kind:'compare', percent:50}};
  assert.deepEqual(failJob(s, 'Falhou'), {job:null, error:'Falhou'});
});

test('startJob libera quando não há job', () => {
  assert.deepEqual(startJob({...initialState}, 'mic'), {job:{kind:'mic', percent:null}, error:null});
});

test('goTo avança para passo já alcançado diferente do atual', () => {
  assert.deepEqual(goTo({...initialState, step:2, maxStep:2}, 1), {step:1, error:null});
  assert.deepEqual(goTo({...initialState, step:1, maxStep:2}, 1), {});
});

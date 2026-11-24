// Lógica pura de navegação dos passos. Nenhuma função toca no DOM ou em api.js.
// Cada função devolve um patch (merge raso feito pelo store) e não muta o estado.

export const initialState = {
  view: 'steps',
  settingsSection: 'transcription',
  step: 1,
  maxStep: 1,
  transcript: null,
  edited: false,
  job: null,
  error: null,
  summaries: [],
};

export function goTo(state, step) {
  if (step <= state.maxStep && step !== state.step) return { step, error: null };
  return {};
}

export function openSettings(state, section = 'transcription') {
  return { view: 'settings', settingsSection: section };
}

export function closeSettings(state) {
  return { view: 'steps' };
}

export function canStartJob(state) {
  return state.job === null;
}

export function startJob(state, kind) {
  if (!canStartJob(state)) {
    return { error: 'Aguarde o processamento atual terminar.' };
  }
  return { job: { kind, percent: null }, error: null };
}

function isEmptyTranscript(transcript) {
  return transcript.segments.length === 0 && transcript.text.trim() === '';
}

export function afterTranscribe(state, transcript) {
  if (isEmptyTranscript(transcript)) {
    return { job: null, step: 1, error: 'Nenhuma fala detectada no áudio.' };
  }
  return {
    job: null,
    transcript,
    edited: false,
    step: 2,
    maxStep: Math.max(state.maxStep, 2),
    summaries: [],
    error: null,
  };
}

export function pasteText(state, text) {
  const transcript = {
    text,
    segments: [],
    model: null,
    language: null,
    sourceName: 'Texto colado',
  };
  return {
    transcript,
    edited: false,
    step: 2,
    maxStep: Math.max(state.maxStep, 2),
    summaries: [],
    error: null,
  };
}

export function needsReplaceConfirm(state) {
  return state.transcript !== null && state.edited;
}

export function failJob(state, message) {
  return { job: null, error: message };
}

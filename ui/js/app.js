// Ponto de entrada da interface: cria o estado, carrega dados e monta as views.
import { api, errorMessage } from './api.js';
import { h } from './dom.js';
import { initialState } from './flow.js';
import { initDownloadEvents } from './downloads.js';
import { createStore } from './store.js';
import { mountErrorBanner } from './views/error-banner.js';
import { mountHeader } from './views/header.js';
import { mountSettings } from './views/settings.js';
import { mountStepAudio } from './views/step-audio.js';
import { mountStepSummary } from './views/step-summary.js';
import { mountStepTranscript } from './views/step-transcript.js';

const store = createStore({ ...initialState, settings: null, models: undefined, downloads: {}, pulls: {}, ollamaTick: 0 });

mountHeader(document.getElementById('header'), store);
mountErrorBanner(document.getElementById('banner'), store);

const main = document.getElementById('main');
const audio = h('section', { class: 'step' });
const transcript = h('section', { class: 'step', hidden: true });
const summary = h('section', { class: 'step', hidden: true });
const settings = h('section', { class: 'step', hidden: true });
main.append(audio, transcript, summary, settings);
mountStepAudio(audio, store);
mountStepTranscript(transcript, store);
mountStepSummary(summary, store);
mountSettings(settings, store);
initDownloadEvents(store);

async function load() {
  try {
    const [settingsData, models] = await Promise.all([api.getSettings(), api.listModels()]);
    store.set({ settings: settingsData, models });
  } catch (e) {
    store.set({ error: errorMessage(e), models: [] });
  }
}
load();

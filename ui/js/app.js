// Ponto de entrada da interface: cria o estado, carrega dados e monta as views.
import { api, errorMessage } from './api.js';
import { h } from './dom.js';
import { closeSettings, initialState } from './flow.js';
import { createStore } from './store.js';
import { mountErrorBanner } from './views/error-banner.js';
import { mountHeader } from './views/header.js';
import { mountStepAudio } from './views/step-audio.js';

const store = createStore({ ...initialState, settings: null, models: undefined });

mountHeader(document.getElementById('header'), store);
mountErrorBanner(document.getElementById('banner'), store);

const main = document.getElementById('main');
const audio = h('section', { class: 'step' });
const later = h('section', { class: 'step', hidden: true }, h('p', { class: 'muted', text: 'Passo em construção.' }));
const settings = h('section', { class: 'step', hidden: true },
  h('p', { text: 'Configurações (em construção)' }),
  h('button', { class: 'btn', type: 'button', text: '← Voltar', onClick: () => store.set(closeSettings(store.get())) }));
main.append(audio, later, settings);
mountStepAudio(audio, store);

// Placeholders até as Tasks 13 e 14.
store.subscribe((s) => {
  later.hidden = !(s.view === 'steps' && s.step !== 1);
  settings.hidden = s.view !== 'settings';
});

async function load() {
  try {
    const [settingsData, models] = await Promise.all([api.getSettings(), api.listModels()]);
    store.set({ settings: settingsData, models });
  } catch (e) {
    store.set({ error: errorMessage(e), models: [] });
  }
}
load();

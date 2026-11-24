// Página de Configurações: voltar, menu lateral e uma seção por vez.
import { closeSettings } from '../flow.js';
import { h } from '../dom.js';
import { mountSettingsAbout } from './settings-about.js';
import { mountSettingsAccounts } from './settings-accounts.js';
import { mountSettingsOllama } from './settings-ollama.js';
import { mountSettingsTranscription } from './settings-transcription.js';

const SECTIONS = [
  ['transcription', 'Transcrição', mountSettingsTranscription],
  ['accounts', 'Contas de IA', mountSettingsAccounts],
  ['ollama', 'Ollama', mountSettingsOllama],
  ['about', 'Sobre', mountSettingsAbout],
];

export function mountSettings(root, store) {
  const back = h('button', { class: 'btn', type: 'button', text: '← Voltar', onClick: () => store.set(closeSettings(store.get())) });
  const buttons = SECTIONS.map(([id, label]) => h('button', {
    class: 'side-btn', type: 'button', text: label, onClick: () => store.set({ settingsSection: id }),
  }));
  const nav = h('nav', { class: 'side', 'aria-label': 'Seções' }, buttons);
  const content = h('div', { class: 'settings-content' });
  root.append(h('div', { class: 'actions' }, back), h('div', { class: 'settings-layout' }, nav, content));

  for (const [, , mount] of SECTIONS) {
    const pane = h('div', { class: 'pane-section' });
    content.append(pane);
    mount(pane, store);
  }

  const render = (s) => {
    root.hidden = s.view !== 'settings';
    SECTIONS.forEach(([id], i) => {
      const current = s.settingsSection === id;
      buttons[i].classList.toggle('current', current);
      if (current) buttons[i].setAttribute('aria-current', 'page'); else buttons[i].removeAttribute('aria-current');
    });
  };
  store.subscribe(render);
  render(store.get());
}

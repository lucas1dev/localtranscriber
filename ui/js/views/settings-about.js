// Configurações › Sobre — versões e pasta de dados.
import { api, errorMessage } from '../api.js';
import { h, showText } from '../dom.js';

const THIRD_PARTY = [
  ['whisper.cpp', 'MIT', 'https://github.com/ggml-org/whisper.cpp'],
  ['FFmpeg', 'LGPL-2.1+ (macOS) / LGPL-3.0+ (Windows)', 'https://ffmpeg.org/download.html#releases'],
  ['Codex CLI', 'Apache-2.0', 'https://github.com/openai/codex'],
  ['marked', 'MIT', 'https://github.com/markedjs/marked'],
  ['DOMPurify', 'Apache-2.0 / MPL-2.0', 'https://github.com/cure53/DOMPurify'],
];

export function mountSettingsAbout(root, store) {
  let wasVisible = false;
  const error = h('p', { class: 'form-error', role: 'alert', hidden: true });
  const versions = h('dl', { class: 'facts' });
  const dir = h('span', { class: 'mono' });
  const openBtn = h('button', {
    class: 'btn', type: 'button', text: 'Abrir pasta',
    onClick: async () => {
      try { await api.openDataDir(); } catch (e) { showText(error, errorMessage(e)); }
    },
  });
  const licenses = h('ul', { class: 'license-list' }, ...THIRD_PARTY.map(([name, license, url]) =>
    h('li', {}, h('a', {
      href: url, text: name,
      onClick: async (e) => {
        e.preventDefault();
        try { await api.openUrl(url); } catch (err) { showText(error, errorMessage(err)); }
      },
    }), ` — ${license}`)));
  root.append(h('h2', { text: 'Sobre' }), error, versions,
    h('div', { class: 'row-main' }, h('div', { class: 'row-info' }, h('strong', { text: 'Pasta de dados' }), dir), openBtn),
    h('h3', { text: 'Licenças de terceiros' }), licenses,
    h('p', { class: 'muted', text: 'Os textos completos das licenças estão dentro do aplicativo (pasta licenses, nos recursos do pacote).' }));

  async function load() {
    showText(error, '');
    try {
      const s = await api.getStatus();
      const v = s.versions || {};
      const rows = [['Local Transcriber', s.version], ['whisper', v.whisper], ['FFmpeg', v.ffmpeg], ['Codex', v.codex]];
      versions.replaceChildren(...rows.flatMap(([k, val]) => [h('dt', { text: k }), h('dd', { text: val || '—' })]));
      dir.textContent = s.data_dir;
    } catch (e) {
      showText(error, errorMessage(e));
    }
  }

  store.subscribe((s) => {
    const visible = s.view === 'settings' && s.settingsSection === 'about';
    root.hidden = !visible;
    if (visible && !wasVisible) load();
    wasVisible = visible;
  });
  root.hidden = true;
}

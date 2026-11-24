// Configurações › Transcrição — modelos Whisper e idioma padrão.
import { api, errorMessage } from '../api.js';
import { h, showText } from '../dom.js';
import { clearEntry, percentOf, refreshModels, startEntry } from '../downloads.js';
import { mergeSettings, SETTINGS_NOT_LOADED } from '../flow.js';
import { formatBytes } from '../format.js';
import { LANGUAGES } from './step-audio.js';

export function mountSettingsTranscription(root, store) {
  const ui = { confirming: null, rows: new Map(), wasVisible: false };

  const error = h('p', { class: 'form-error', role: 'alert', hidden: true });
  const list = h('div', { class: 'rows' });
  const importBtn = h('button', { class: 'btn', type: 'button', text: 'Importar modelo existente…', onClick: () => importModel() });
  const langSel = h('select', { id: 'default-lang', onChange: () => saveLanguage() },
    LANGUAGES.map(([v, l]) => h('option', { value: v, text: l })));
  root.append(
    h('h2', { text: 'Transcrição' }),
    h('p', { class: 'muted', text: 'Modelos Whisper que rodam neste computador, sem enviar o áudio para a internet.' }),
    error, list,
    h('div', { class: 'actions' }, importBtn),
    h('div', { class: 'options' }, h('label', { for: 'default-lang', text: 'Idioma padrão' }), langSel));

  async function run(action) {
    showText(error, '');
    try {
      await action();
    } catch (e) {
      showText(error, errorMessage(e));
      ui.rows.clear(); // refaz as linhas (ex.: o rádio marcado que não valeu)
      render(store.get());
    }
  }

  const download = (id) => run(async () => {
    const created = startEntry(store, 'downloads', id);
    try {
      await api.downloadModel(id);
    } catch (e) {
      if (created) clearEntry(store, 'downloads', id);
      throw e;
    }
  });
  const cancel = (id) => run(async () => {
    clearEntry(store, 'downloads', id); // sai da linha mesmo que nenhum `done` chegue
    await api.cancelDownload(id);
  });
  const makeDefault = (id) => run(async () => {
    await api.setDefaultModel(id);
    await refreshModels(store);
  });
  const remove = (id) => run(async () => {
    ui.confirming = null;
    try {
      await api.deleteModel(id);
    } finally {
      await refreshModels(store);
    }
  });
  const importModel = () => run(async () => {
    const path = await api.pickModelFile();
    if (!path) return;
    await api.importModel(path);
    await refreshModels(store);
  });
  const saveLanguage = () => run(async () => {
    const next = mergeSettings(store.get().settings, { default_language: langSel.value });
    if (!next) throw new Error(SETTINGS_NOT_LOADED);
    const saved = await api.setSettings(next);
    store.set({ settings: saved });
  });

  function buildRow(m, entry, installedCount) {
    const el = h('div', { class: 'row' });
    const info = h('div', { class: 'row-info' },
      h('strong', { text: m.label }), h('span', { class: 'muted', text: formatBytes(m.size_bytes) }));
    const side = h('div', { class: 'row-actions' });
    const extra = [];
    let bar = null;
    let barText = null;
    if (entry && !entry.error) {
      bar = h('progress', { max: '100', 'aria-label': `Baixando ${m.label}` });
      barText = h('span', { class: 'bar-text' });
      extra.push(h('div', { class: 'progress' }, bar, barText));
      side.append(h('button', { class: 'btn', type: 'button', text: 'Cancelar', onClick: () => cancel(m.id) }));
    } else if (m.installed) {
      info.append(h('span', { class: 'ok', text: '✓ instalado' }));
      side.append(
        h('label', { class: 'check' },
          h('input', { type: 'radio', name: 'default-model', checked: m.is_default, onChange: () => makeDefault(m.id) }),
          h('span', { text: 'padrão' })));
      const onlyOne = m.is_default && installedCount === 1;
      side.append(h('button', {
        class: 'btn', type: 'button', text: 'Remover', disabled: onlyOne,
        title: onlyOne ? 'É o único modelo instalado' : null,
        onClick: () => { ui.confirming = m.id; render(store.get()); },
      }));
      if (ui.confirming === m.id) {
        extra.push(h('div', { class: 'confirm', role: 'alert' },
          h('span', { text: `Remover o modelo ${m.label}? O arquivo é apagado deste computador.` }),
          h('button', { class: 'btn danger', type: 'button', text: 'Remover', onClick: () => remove(m.id) }),
          h('button', { class: 'btn', type: 'button', text: 'Cancelar', onClick: () => { ui.confirming = null; render(store.get()); } })));
      }
    } else if (entry?.error) {
      extra.push(h('p', { class: 'form-error', text: entry.error }));
      side.append(h('button', {
        class: 'btn', type: 'button', text: 'Tentar de novo',
        onClick: () => { clearEntry(store, 'downloads', m.id); download(m.id); },
      }));
    } else {
      side.append(h('button', { class: 'btn', type: 'button', text: 'Baixar', onClick: () => download(m.id) }));
    }
    el.append(h('div', { class: 'row-main' }, info, side), ...extra);
    return { el, bar, barText };
  }

  function updateProgress(row, m, entry) {
    if (!row.bar) return;
    const pct = percentOf(entry);
    if (pct === null) row.bar.removeAttribute('value'); else row.bar.value = pct;
    row.barText.textContent = pct === null ? '…' : `${pct}%`;
  }

  function render(s) {
    const visible = s.view === 'settings' && s.settingsSection === 'transcription';
    root.hidden = !visible;
    if (visible && !ui.wasVisible) { showText(error, ''); ui.confirming = null; }
    ui.wasVisible = visible;
    if (!visible) return;

    if (s.settings) langSel.value = s.settings.default_language;
    const models = s.models || [];
    const installedCount = models.filter((m) => m.installed).length;
    const next = [];
    for (const m of models) {
      const entry = s.downloads[m.id];
      const sig = JSON.stringify([m.label, m.size_bytes, m.installed, m.is_default, installedCount,
        !!entry, entry?.error ?? null, ui.confirming === m.id]);
      let row = ui.rows.get(m.id);
      if (!row || row.sig !== sig) {
        row = { ...buildRow(m, entry, installedCount), sig };
        ui.rows.set(m.id, row);
      }
      if (entry) updateProgress(row, m, entry);
      next.push(row.el);
    }
    list.replaceChildren(...next);
    if (s.models === undefined) list.append(h('p', { class: 'muted', text: 'Carregando…' }));
  }
  store.subscribe(render);
  render(store.get());
}

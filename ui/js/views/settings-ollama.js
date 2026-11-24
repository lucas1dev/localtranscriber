// Configurações › Ollama — estado, modelos instalados e download de modelos.
import { api, errorMessage } from '../api.js';
import { h, showText } from '../dom.js';
import { clearEntry, percentOf } from '../downloads.js';

const DOWNLOAD_URL = 'https://ollama.com/download';

export function mountSettingsOllama(root, store) {
  const ui = { running: false, models: [], loaded: false, confirming: null, wasVisible: false, tick: 0, pullKey: null };

  const error = h('p', { class: 'form-error', role: 'alert', hidden: true });
  const status = h('p', { role: 'status' });
  const getLink = h('button', {
    class: 'link', type: 'button', text: 'Baixar o Ollama (ollama.com/download)', hidden: true,
    onClick: () => run(() => api.openUrl(DOWNLOAD_URL)),
  });
  const list = h('div', { class: 'rows' });
  const nameInput = h('input', {
    id: 'pull-name', type: 'text', placeholder: 'qwen3:8b', autocomplete: 'off', spellcheck: 'false',
    onKeydown: (ev) => { if (ev.key === 'Enter') pull(); },
  });
  const pullBtn = h('button', { class: 'btn', type: 'button', text: 'Baixar', onClick: () => pull() });
  const pulls = h('div', { class: 'rows' });

  root.append(
    h('h2', { text: 'Ollama' }),
    h('p', { class: 'muted', text: 'Roda modelos de linguagem neste computador, sem enviar a transcrição para a internet.' }),
    error, status, getLink,
    h('h3', { text: 'Modelos instalados' }), list,
    h('h3', { text: 'Baixar modelo' }),
    h('div', { class: 'options' }, h('label', { for: 'pull-name', text: 'Baixar modelo' }), nameInput, pullBtn),
    pulls);

  async function run(action) {
    showText(error, '');
    try {
      await action();
    } catch (e) {
      showText(error, errorMessage(e));
    }
  }

  async function load() {
    try {
      const info = await api.ollamaModels();
      ui.running = info.running;
      ui.models = info.models || [];
    } catch (e) {
      ui.running = false;
      ui.models = [];
      showText(error, errorMessage(e));
    }
    ui.loaded = true;
    render(store.get());
  }

  const pull = () => run(async () => {
    const name = nameInput.value.trim();
    if (!name) return;
    clearEntry(store, 'pulls', name);
    await api.ollamaPull(name);
    store.set({ pulls: { ...store.get().pulls, [name]: { status: '', done: 0, total: 0, error: null } } });
    nameInput.value = '';
  });

  const remove = (name) => run(async () => {
    ui.confirming = null;
    try {
      await api.ollamaDelete(name);
    } finally {
      await load();
    }
  });

  function modelRow(name) {
    const el = h('div', { class: 'row' }, h('div', { class: 'row-main' },
      h('div', { class: 'row-info' }, h('strong', { class: 'mono', text: name })),
      h('div', { class: 'row-actions' }, h('button', {
        class: 'btn', type: 'button', text: 'Remover', onClick: () => { ui.confirming = name; render(store.get()); },
      }))));
    if (ui.confirming === name) {
      el.append(h('div', { class: 'confirm', role: 'alert' },
        h('span', { text: `Remover ${name} do Ollama?` }),
        h('button', { class: 'btn danger', type: 'button', text: 'Remover', onClick: () => remove(name) }),
        h('button', { class: 'btn', type: 'button', text: 'Cancelar', onClick: () => { ui.confirming = null; render(store.get()); } })));
    }
    return el;
  }

  // Linhas de download: reconstruídas só quando o conjunto/estado muda; o progresso é atualizado no lugar.
  const pullRows = new Map();
  function renderPulls(entries) {
    const key = JSON.stringify(Object.entries(entries).map(([n, e]) => [n, !!e.error, e.error]));
    if (key !== ui.pullKey) {
      ui.pullKey = key;
      pullRows.clear();
      pulls.replaceChildren(...Object.entries(entries).map(([name, e]) => {
        if (e.error) {
          return h('div', { class: 'row' },
            h('div', { class: 'row-main' }, h('strong', { class: 'mono', text: name }),
              h('button', { class: 'btn', type: 'button', text: 'Dispensar', onClick: () => clearEntry(store, 'pulls', name) })),
            h('p', { class: 'form-error', text: e.error }));
        }
        const bar = h('progress', { max: '100', 'aria-label': `Baixando ${name}` });
        const text = h('span', { class: 'bar-text' });
        pullRows.set(name, { bar, text });
        return h('div', { class: 'row' }, h('div', { class: 'row-main' }, h('strong', { class: 'mono', text: name }),
          h('button', { class: 'btn', type: 'button', text: 'Cancelar', onClick: () => run(() => api.cancelOllamaPull(name)) })),
        h('div', { class: 'progress' }, bar, text));
      }));
    }
    for (const [name, row] of pullRows) {
      const e = entries[name];
      const pct = percentOf(e);
      if (pct === null) row.bar.removeAttribute('value'); else row.bar.value = pct;
      row.text.textContent = pct === null ? (e.status || '…') : `${pct}%`;
    }
  }

  function render(s) {
    const visible = s.view === 'settings' && s.settingsSection === 'ollama';
    root.hidden = !visible;
    if (visible && !ui.wasVisible) { showText(error, ''); ui.confirming = null; load(); }
    ui.wasVisible = visible;
    if (!visible) return;
    if (s.ollamaTick !== ui.tick) { ui.tick = s.ollamaTick; load(); }

    status.textContent = !ui.loaded ? 'Verificando…' : (ui.running ? 'Ollama rodando' : 'Ollama não encontrado');
    getLink.hidden = !ui.loaded || ui.running;
    nameInput.disabled = pullBtn.disabled = !ui.running;
    list.replaceChildren(...(ui.models.length
      ? ui.models.map(modelRow)
      : [h('p', { class: 'muted', text: ui.running ? 'Nenhum modelo baixado.' : '—' })]));
    renderPulls(s.pulls);
  }
  store.subscribe(render);
  render(store.get());
}

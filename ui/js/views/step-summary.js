// Passo ③ — gerar um resumo com um provedor ou comparar de 2 a 4 provedores.
import { api, errorMessage } from '../api.js';
import { h } from '../dom.js';
import { saveText } from '../export.js';
import { failJob, goTo, mergeSettings, openSettings, startJob } from '../flow.js';
import { interceptLinks, renderMarkdown } from '../markdown.js';
import { configuredProviders, initialProvider, usableProviders } from '../providers.js';
import { buildCompareRequest, buildSummaryRequest } from '../requests.js';
import { OLLAMA_DEFAULT, STYLES, ollamaDefaultModel, pickStyle } from '../summary-options.js';

const DEFAULT_MODEL = {
  openai: 'gpt-5.1', anthropic: 'claude-sonnet-4-5', gemini: 'gemini-2.5-flash', ollama: OLLAMA_DEFAULT,
};
const ACCOUNT_DEFAULT = 'padrão da sua conta';
const MAX_COMPARE = 4;

export function mountStepSummary(root, store) {
  const ui = { providers: [], provider: '', style: null, model: '', custom: '', compare: new Set(), ollama: [] };
  let wasVisible = false;
  let providersKey = null;
  let shownSummaries = null;

  const label = (id) => ui.providers.find((p) => p.id === id)?.label || id;
  const usable = () => usableProviders(ui.providers);
  const configured = () => configuredProviders(ui.providers);

  // --- elementos ---
  const providerSel = h('select', { id: 'prov-sel', onChange: () => { ui.provider = providerSel.value; ui.model = ''; modelInput.value = ''; render(store.get()); } });
  const configLink = h('button', { class: 'link', type: 'button', text: 'configurar', onClick: () => store.set(openSettings(store.get(), 'accounts')) });
  const styleSel = h('select', { id: 'style-sel', onChange: () => { ui.style = styleSel.value; render(store.get()); } },
    STYLES.map(([v, l]) => h('option', { value: v, text: l })));
  const modelInput = h('input', { id: 'model-in', type: 'text', onInput: () => { ui.model = modelInput.value; } });
  const customArea = h('textarea', {
    class: 'custom-area', rows: '3', 'aria-label': 'Instruções personalizadas', placeholder: 'Foco, público, tamanho, estrutura…',
    onInput: () => { ui.custom = customArea.value; },
  });
  const options = h('div', { class: 'options' },
    h('label', { for: 'prov-sel', text: 'Provedor' }), providerSel, configLink,
    h('label', { for: 'style-sel', text: 'Formato' }), styleSel,
    h('label', { for: 'model-in', text: 'Modelo' }), modelInput);

  const genBtn = h('button', { class: 'btn primary', type: 'button', text: 'Gerar resumo', onClick: generate });
  const status = h('span', { class: 'muted', role: 'status' });
  const compareBox = h('fieldset', { class: 'compare' }, h('legend', { text: 'Comparar provedores (2 a 4)' }));
  const compareList = h('div', { class: 'compare-list' });
  const compareBtn = h('button', { class: 'btn', type: 'button', text: 'Comparar', onClick: compare });
  compareBox.append(compareList, compareBtn);

  const results = h('div', { class: 'summary-results' });
  results.addEventListener('click', (ev) => interceptLinks(ev, (url) => {
    api.openUrl(url).catch((e) => store.set({ error: errorMessage(e) }));
  }));
  const back = h('button', { class: 'btn', type: 'button', text: '← Voltar', onClick: () => store.set(goTo(store.get(), 2)) });

  root.append(options, customArea, h('div', { class: 'actions' }, genBtn, status), compareBox, results, h('div', { class: 'actions' }, back));

  // --- ações ---
  function form() {
    return { provider: ui.provider, style: ui.style, model: ui.model, customInstructions: ui.custom };
  }

  async function saveChoice() {
    const next = mergeSettings(store.get().settings, { summary_provider: ui.provider, summary_style: ui.style });
    if (!next) return; // configurações não carregaram: não grava por cima
    try {
      const saved = await api.setSettings(next);
      store.set({ settings: saved });
    } catch (e) {
      store.set({ error: errorMessage(e) });
    }
  }

  async function generate() {
    const s = store.get();
    if (!s.transcript || !ui.provider) return;
    const patch = startJob(s, 'summary');
    store.set(patch);
    if (!patch.job) return;
    let r;
    try {
      r = await api.summarize(buildSummaryRequest(store.get(), form()));
    } catch (e) {
      store.set(failJob(store.get(), errorMessage(e)));
      return;
    }
    store.set({ job: null, summaries: [{ provider: r.provider, model: r.model, summary: r.summary, chunks_processed: r.chunks_processed }] });
    await saveChoice();
  }

  async function compare() {
    const s = store.get();
    if (!s.transcript || ui.compare.size < 2 || ui.compare.size > MAX_COMPARE) return;
    const patch = startJob(s, 'compare');
    store.set(patch);
    if (!patch.job) return;
    let items;
    try {
      const defaults = { ollama: ollamaDefaultModel(ui.ollama) };
      items = await api.compare(buildCompareRequest(store.get(), { ...form(), providers: [...ui.compare], defaults }));
    } catch (e) {
      store.set(failJob(store.get(), errorMessage(e)));
      return;
    }
    store.set({
      job: null,
      summaries: items.map((i) => ({ provider: i.provider, model: i.model, summary: i.summary, error: i.error ? i.error.message : null })),
    });
    await saveChoice();
  }

  async function refreshProviders() {
    try {
      ui.providers = await api.providersStatus();
    } catch (e) {
      store.set({ error: errorMessage(e) });
      return;
    }
    if (configured().some((p) => p.id === 'ollama')) {
      try { ui.ollama = (await api.ollamaModels()).models || []; } catch (e) { console.error(e); ui.ollama = []; }
    } else {
      ui.ollama = [];
    }
    render(store.get());
  }

  // --- render ---
  function syncProviders(s) {
    const key = JSON.stringify(usable().map((p) => [p.id, p.label, p.configured]));
    if (key !== providersKey) {
      providersKey = key;
      providerSel.replaceChildren(...usable().map((p) => h('option', {
        value: p.id, disabled: !p.configured, text: p.configured ? p.label : `${p.label} — configurar`,
      })));
      compareList.replaceChildren(...configured().map((p) => h('label', { class: 'check' },
        h('input', {
          type: 'checkbox', value: p.id,
          onChange: (ev) => { if (ev.target.checked) ui.compare.add(p.id); else ui.compare.delete(p.id); render(store.get()); },
        }), h('span', { text: p.label }))));
    }
    const ids = configured().map((p) => p.id);
    ui.compare = new Set([...ui.compare].filter((id) => ids.includes(id)));
    if (!ids.includes(ui.provider)) {
      ui.provider = initialProvider(ui.providers, s.settings?.summary_provider) || '';
      ui.model = '';
      modelInput.value = '';
    }
    providerSel.value = ui.provider;
    for (const box of compareList.querySelectorAll('input')) {
      box.checked = ui.compare.has(box.value);
      box.disabled = s.job !== null || (!box.checked && ui.compare.size >= MAX_COMPARE);
    }
  }

  // Sem o modelo padrão instalado no Ollama, já preenche o primeiro modelo instalado.
  function preselectOllamaModel() {
    if (ui.provider !== 'ollama' || ui.model) return;
    const model = ollamaDefaultModel(ui.ollama);
    if (model) { ui.model = model; modelInput.value = model; }
  }

  function placeholder() {
    if (ui.provider === 'ollama' && ui.ollama.length) return `Instalados: ${ui.ollama.slice(0, 4).join(', ')}`;
    return DEFAULT_MODEL[ui.provider] || ACCOUNT_DEFAULT;
  }

  function renderCard(item) {
    const meta = [item.model, item.chunks_processed > 1 ? `${item.chunks_processed} blocos processados` : null].filter(Boolean).join(' · ');
    const body = h('div', { class: 'summary-body' });
    if (item.error) body.textContent = item.error;
    else body.innerHTML = renderMarkdown(item.summary || ''); // único innerHTML do app: Markdown sanitizado
    const card = h('article', { class: item.error ? 'summary-card error' : 'summary-card' },
      h('h3', { text: label(item.provider) }), h('div', { class: 'muted', text: meta }), body);
    if (item.summary && !item.error) {
      const copy = h('button', { class: 'btn', type: 'button', text: 'Copiar' });
      copy.addEventListener('click', async () => {
        try {
          await navigator.clipboard.writeText(item.summary);
        } catch (e) { store.set({ error: errorMessage(e) }); return; }
        copy.textContent = 'Copiado ✓';
        setTimeout(() => { copy.textContent = 'Copiar'; }, 1400);
      });
      card.append(h('div', { class: 'actions' }, copy,
        h('button', { class: 'btn', type: 'button', text: 'Salvar .md', onClick: () => saveText(`resumo-${item.provider}.md`, item.summary).catch((e) => store.set({ error: errorMessage(e) })) })));
    }
    return card;
  }

  function render(s) {
    const visible = s.view === 'steps' && s.step === 3;
    root.hidden = !visible;
    if (visible && !wasVisible) refreshProviders();
    wasVisible = visible;

    ui.style = pickStyle(ui.style, s.settings, visible);
    syncProviders(s);
    preselectOllamaModel();
    if (ui.style !== null) styleSel.value = ui.style;
    modelInput.placeholder = placeholder();
    customArea.hidden = ui.style !== 'custom';
    configLink.hidden = usable().every((p) => p.configured);

    const busy = s.job !== null;
    const summarizing = busy && (s.job.kind === 'summary' || s.job.kind === 'compare');
    status.textContent = summarizing ? (s.job.kind === 'compare' ? 'Comparando…' : 'Resumindo…') : '';
    genBtn.disabled = busy || !ui.provider || !s.transcript;
    compareBtn.disabled = busy || ui.compare.size < 2 || ui.compare.size > MAX_COMPARE;
    compareBox.hidden = configured().length < 2;
    for (const el of [providerSel, styleSel, modelInput, customArea]) el.disabled = busy;

    if (s.summaries !== shownSummaries) {
      shownSummaries = s.summaries;
      results.replaceChildren(...s.summaries.map(renderCard));
      results.classList.toggle('compare-grid', s.summaries.length > 1);
    }
  }

  render(store.get());
  store.subscribe(render);
}

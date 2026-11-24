// Passo ① — escolher arquivo ou gravar do microfone, modelo e idioma, e transcrever.
import { api, on, errorMessage, isCancelled } from '../api.js';
import { h } from '../dom.js';
import {
  afterTranscribe, failJob, needsReplaceConfirm, openSettings, pasteText, startJob,
} from '../flow.js';
import { createMic } from '../mic.js';

const LANGUAGES = [
  ['auto', 'Detectar automaticamente'], ['pt', 'Português'], ['en', 'Inglês'],
  ['es', 'Espanhol'], ['fr', 'Francês'], ['de', 'Alemão'],
];

const baseName = (path) => path.split(/[\\/]/).pop();
const clock = (secs) => `${String(Math.floor(secs / 60)).padStart(2, '0')}:${String(secs % 60).padStart(2, '0')}`;

// Backend devolve snake_case; o estado usa sourceName.
const toTranscript = (t) => ({
  text: t.text, segments: t.segments, model: t.model, language: t.language, sourceName: t.source_name,
});

export function mountStepAudio(root, store) {
  const ui = { tab: 'file', path: null, model: '', language: null, confirming: false, micRunning: false, micStopping: false, elapsed: 0 };
  let mic = null;
  let clockTimer = null;
  let optionsKey = null;

  // --- elementos ---
  const noModelWarn = h('div', { class: 'notice' },
    h('span', { text: 'Nenhum modelo instalado. Baixe um modelo para transcrever.' }),
    h('button', { class: 'btn', type: 'button', text: 'Abrir Configurações', onClick: () => store.set(openSettings(store.get())) }));

  const tabFile = h('button', { class: 'tab', type: 'button', text: 'Arquivo', onClick: () => setTab('file') });
  const tabMic = h('button', { class: 'tab', type: 'button', text: 'Microfone', onClick: () => setTab('mic') });

  const dropName = h('span', { class: 'drop-name' });
  const drop = h('button', { class: 'drop', type: 'button', onClick: pickFile },
    h('span', { class: 'drop-title', text: 'Solte um arquivo aqui ou clique para escolher' }), dropName);
  const filePane = h('div', { class: 'pane' }, drop);

  const micTime = h('div', { class: 'mic-time', text: '00:00' });
  const micLive = h('div', { class: 'mic-live', 'aria-live': 'polite' });
  const micBtn = h('button', { class: 'btn primary', type: 'button', onClick: toggleMic });
  const micPane = h('div', { class: 'pane' }, micTime, micLive, micBtn);

  const modelSel = h('select', { id: 'model-sel', onChange: () => { ui.model = modelSel.value; render(); } });
  const langSel = h('select', { id: 'lang-sel', onChange: () => { ui.language = langSel.value; } },
    LANGUAGES.map(([v, l]) => h('option', { value: v, text: l })));
  const getModels = h('button', { class: 'link', type: 'button', text: 'Baixar modelos', onClick: () => store.set(openSettings(store.get())) });
  const options = h('div', { class: 'options' },
    h('label', { for: 'model-sel', text: 'Modelo' }), modelSel, getModels,
    h('label', { for: 'lang-sel', text: 'Idioma' }), langSel);

  const goBtn = h('button', { class: 'btn primary', type: 'button', text: 'Transcrever', onClick: onTranscribe });

  const confirmPanel = h('div', { class: 'notice confirm', role: 'alertdialog', hidden: true },
    h('span', { text: 'Substituir a transcrição editada?' }),
    h('button', { class: 'btn primary', type: 'button', text: 'Substituir', onClick: () => { ui.confirming = false; if (ui.tab === 'mic') startMic(); else runTranscribe(); } }),
    h('button', { class: 'btn', type: 'button', text: 'Cancelar', onClick: () => { ui.confirming = false; render(); } }));

  const bar = h('progress', { max: '100' });
  const barText = h('span', { class: 'bar-text' });
  const cancelBtn = h('button', { class: 'btn', type: 'button', text: 'Cancelar', onClick: () => api.cancelTranscription().catch((e) => store.set({ error: errorMessage(e) })) });
  const progress = h('div', { class: 'progress', hidden: true }, bar, barText, cancelBtn);

  const pasteLink = h('button', { class: 'link', type: 'button', text: 'Já tenho o texto', onClick: () => { pasteBox.hidden = !pasteBox.hidden; } });
  const pasteArea = h('textarea', { rows: '6', placeholder: 'Cole o texto aqui', 'aria-label': 'Texto' });
  const pasteBtn = h('button', {
    class: 'btn', type: 'button', text: 'Usar este texto',
    onClick: () => {
      const text = pasteArea.value.trim();
      if (!text || store.get().job) return;
      store.set(pasteText(store.get(), text));
      pasteArea.value = '';
      pasteBox.hidden = true;
    },
  });
  const pasteBox = h('div', { class: 'paste', hidden: true }, pasteArea, pasteBtn);

  root.append(noModelWarn, h('div', { class: 'tabs', role: 'tablist' }, tabFile, tabMic),
    filePane, micPane, options, confirmPanel, h('div', { class: 'actions' }, goBtn), progress, pasteLink, pasteBox);

  // --- ações ---
  function setTab(tab) {
    if (store.get().job) return;
    ui.tab = tab;
    render();
  }

  async function pickFile() {
    if (store.get().job) return;
    try {
      const path = await api.pickAudioFile();
      if (path) { ui.path = Array.isArray(path) ? path[0] : path; render(); }
    } catch (e) {
      store.set({ error: errorMessage(e) });
    }
  }

  function selectedInstalled() {
    return (store.get().models || []).some((m) => m.id === ui.model && m.installed);
  }

  function canGo() {
    const s = store.get();
    if (s.job !== null || !selectedInstalled()) return false;
    return ui.tab === 'mic' || ui.path !== null;
  }

  function onTranscribe() {
    if (!canGo() || ui.tab !== 'file') return;
    if (needsReplaceConfirm(store.get())) { ui.confirming = true; render(); return; }
    runTranscribe();
  }

  async function runTranscribe() {
    const patch = startJob(store.get(), 'file');
    store.set(patch);
    if (!patch.job) return;
    try {
      const t = await api.transcribeFile(ui.path, ui.model, ui.language);
      store.set(afterTranscribe(store.get(), toTranscript(t)));
    } catch (e) {
      store.set(isCancelled(e) ? { job: null } : failJob(store.get(), errorMessage(e)));
    }
  }

  function stopClock() { clearInterval(clockTimer); clockTimer = null; }

  async function toggleMic() {
    if (ui.micRunning) return stopMic();
    if (!canGo()) return;
    if (needsReplaceConfirm(store.get())) { ui.confirming = true; render(); return; }
    startMic();
  }

  async function startMic() {
    const patch = startJob(store.get(), 'mic');
    store.set(patch);
    if (!patch.job) return;
    micLive.textContent = '';
    mic = createMic({
      onChunkText: (t) => { micLive.textContent += (micLive.textContent ? ' ' : '') + t; },
      onError: (msg) => store.set({ error: msg }),
    });
    try {
      await mic.start(ui.model, ui.language);
    } catch (e) {
      mic = null;
      store.set(failJob(store.get(), errorMessage(e)));
      return;
    }
    ui.micRunning = true;
    ui.elapsed = 0;
    clockTimer = setInterval(() => { ui.elapsed += 1; micTime.textContent = clock(ui.elapsed); }, 1000);
    render();
  }

  async function stopMic() {
    if (ui.micStopping) return;
    ui.micStopping = true;
    stopClock();
    render();
    const m = mic;
    try {
      const result = await m.stop();
      ui.micRunning = ui.micStopping = false;
      mic = null;
      store.set(afterTranscribe(store.get(), result));
    } catch (e) {
      ui.micRunning = ui.micStopping = false;
      mic = null;
      store.set(failJob(store.get(), errorMessage(e)));
    }
  }

  // --- render ---
  function syncOptions(s) {
    const models = s.models || [];
    const key = JSON.stringify(models.map((m) => [m.id, m.installed]));
    if (key !== optionsKey) {
      optionsKey = key;
      modelSel.replaceChildren(...models.map((m) => h('option', {
        value: m.id, disabled: !m.installed, text: m.installed ? m.label : `${m.label} — não instalado`,
      })));
    }
    if (!selectedInstalled()) {
      const pref = models.find((m) => m.id === s.settings?.default_model && m.installed) || models.find((m) => m.installed);
      ui.model = pref ? pref.id : '';
    }
    modelSel.value = ui.model;
    if (ui.language === null && s.settings) ui.language = s.settings.default_language || 'auto';
    if (ui.language !== null) langSel.value = ui.language;
  }

  function render() {
    const s = store.get();
    root.hidden = !(s.view === 'steps' && s.step === 1);
    syncOptions(s);
    const busy = s.job !== null;
    const anyInstalled = (s.models || []).some((m) => m.installed);
    noModelWarn.hidden = anyInstalled || s.models === undefined;

    tabFile.classList.toggle('active', ui.tab === 'file');
    tabMic.classList.toggle('active', ui.tab === 'mic');
    tabFile.disabled = tabMic.disabled = busy;
    filePane.hidden = ui.tab !== 'file';
    micPane.hidden = ui.tab !== 'mic';
    dropName.textContent = ui.path ? baseName(ui.path) : '';
    drop.disabled = busy;
    if (!ui.micRunning && !(busy && s.job.kind === 'mic')) {
      micBtn.textContent = 'Gravar';
      micBtn.disabled = !canGo();
    } else if (ui.micRunning) {
      micBtn.textContent = ui.micStopping ? 'Finalizando…' : 'Finalizar';
      micBtn.disabled = ui.micStopping;
    }
    micTime.textContent = clock(ui.elapsed);

    modelSel.disabled = langSel.disabled = busy;
    goBtn.hidden = ui.tab !== 'file';
    goBtn.disabled = !canGo();
    pasteBtn.disabled = busy;
    confirmPanel.hidden = !ui.confirming;
    if (ui.confirming) { goBtn.disabled = true; micBtn.disabled = true; }

    const fileJob = busy && s.job.kind === 'file';
    progress.hidden = !fileJob;
    if (fileJob) {
      if (s.job.percent == null) { bar.removeAttribute('value'); barText.textContent = ''; }
      else { bar.value = s.job.percent; barText.textContent = `${s.job.percent}%`; }
    }
  }

  // --- eventos ---
  on('transcribe://progress', (p) => {
    const job = store.get().job;
    if (job && job.kind === 'file' && typeof p?.percent === 'number') {
      store.set({ job: { ...job, percent: p.percent } });
    }
  }).catch((e) => { console.error(e); store.set({ error: errorMessage(e) }); });

  on('tauri://drag-drop', (p) => {
    const s = store.get();
    const path = p?.paths?.[0];
    if (!path || s.view !== 'steps' || s.step !== 1 || s.job !== null) return;
    ui.tab = 'file';
    ui.path = path;
    render();
  }).catch((e) => { console.error(e); store.set({ error: errorMessage(e) }); });

  render();
  store.subscribe(render);
}

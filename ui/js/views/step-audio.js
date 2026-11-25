// Passo ① — escolher arquivo ou gravar (microfone e/ou som do computador), modelo e idioma, e transcrever.
import { api, on, errorMessage, isCancelled } from '../api.js';
import { h } from '../dom.js';
import {
  afterTranscribe, failJob, needsReplaceConfirm, openSettings, pasteText, startJob,
} from '../flow.js';
import {
  SILENCE_HINT, SPEAKER_LABEL, createRecorder, isMacUserAgent, levelPercent, liveLine, outputOptions, pickMicId,
  pickOutputId, pickSource, recordingFailure, recordingResult, showHeadphoneHint, showMicPicker, showOutputNote, showOutputPicker,
  showSilenceHint, silenceHintText,
} from '../recorder.js';

export const LANGUAGES = [
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
  const mac = isMacUserAgent(navigator.userAgent);
  const ui = {
    tab: 'file', path: null, model: '', language: null, confirming: false, micRunning: false, micStopping: false, starting: false, elapsed: 0,
    source: null, inputs: [], micId: null, inputsLoaded: false, outputs: [], outputsLoaded: false, outputId: null, outputNotice: '',
    // Gravação em andamento: fonte usada, quando começou e o maior nível de Reunião.
    recSource: null, recOutputId: null, startedAt: 0, systemPeak: 0,
  };
  let recorder = null;
  // Erro que a sessão avisou na gravação atual; sobrevive ao resultado de stop().
  let sessionError = null;
  let clockTimer = null;
  let optionsKey = null;

  // --- elementos ---
  const noModelWarn = h('div', { class: 'notice' },
    h('span', { text: 'Nenhum modelo instalado. Baixe um modelo para transcrever.' }),
    h('button', { class: 'btn', type: 'button', text: 'Abrir Configurações', onClick: () => store.set(openSettings(store.get(), 'transcription')) }));

  const tabFile = h('button', { class: 'tab', type: 'button', text: 'Arquivo', onClick: () => setTab('file') });
  const tabMic = h('button', { class: 'tab', type: 'button', text: 'Gravar', onClick: () => setTab('mic') });

  const dropName = h('span', { class: 'drop-name' });
  const drop = h('button', { class: 'drop', type: 'button', onClick: pickFile },
    h('span', { class: 'drop-title', text: 'Solte um arquivo aqui ou clique para escolher' }), dropName);
  const filePane = h('div', { class: 'pane' }, drop);

  const SOURCE_LABELS = [['mic', 'Microfone'], ['system', 'Som do computador'], ['both', 'Microfone + som do computador']];
  const sourceRadios = SOURCE_LABELS.map(([value, label]) => {
    const input = h('input', { type: 'radio', name: 'record-source', value, onChange: () => { ui.source = value; render(); } });
    return { value, input, node: h('label', { class: 'radio' }, input, h('span', { text: label })) };
  });
  const sourceRow = h('div', { class: 'source-row', role: 'radiogroup', 'aria-label': 'Fonte' },
    h('span', { class: 'field-label', text: 'Fonte' }), sourceRadios.map((r) => r.node));
  const micSel = h('select', { id: 'mic-sel', onChange: () => { ui.micId = micSel.value || null; } });
  const refreshBtn = h('button', { class: 'btn', type: 'button', text: '↻', title: 'Recarregar microfones', 'aria-label': 'Recarregar microfones', onClick: () => loadInputs() });
  const micRow = h('div', { class: 'mic-row' }, h('label', { for: 'mic-sel', text: 'Microfone' }), micSel, refreshBtn);
  const outputSel = h('select', { id: 'output-sel', onChange: () => { ui.outputId = outputSel.value || null; } });
  const outputRefreshBtn = h('button', { class: 'btn', type: 'button', text: '↻', title: 'Recarregar saídas de som', 'aria-label': 'Recarregar saídas de som', onClick: () => loadOutputs() });
  const outputRow = h('div', { class: 'mic-row output-row' }, h('label', { for: 'output-sel', text: 'Saída de som' }), outputSel, outputRefreshBtn);
  const hint = h('div', { class: 'hint', text: 'Use fone de ouvido para a fala dos outros não ser gravada duas vezes.' });
  const setupBox = h('div', { class: 'record-setup' }, sourceRow, micRow, outputRow, hint);

  const recDot = h('span', { class: 'rec-dot', 'aria-hidden': 'true' });
  const recText = h('span', { class: 'rec-text', text: 'Gravando' });
  const micTime = h('span', { class: 'mic-time', text: '00:00' });
  const recStatus = h('div', { class: 'rec-status', role: 'status' }, recDot, recText, micTime);
  const levelFills = { voce: h('div', { class: 'level-fill' }), reuniao: h('div', { class: 'level-fill' }) };
  const levelRows = {
    voce: h('div', { class: 'level-row' }, h('span', { text: SPEAKER_LABEL.voce }), h('div', { class: 'level-track' }, levelFills.voce)),
    reuniao: h('div', { class: 'level-row' }, h('span', { text: SPEAKER_LABEL.reuniao }), h('div', { class: 'level-track' }, levelFills.reuniao)),
  };
  const levels = h('div', { class: 'levels' }, levelRows.voce, levelRows.reuniao);
  const silenceNote = h('div', { class: 'hint', text: SILENCE_HINT, hidden: true });
  const outputNote = h('div', { class: 'hint', 'aria-live': 'polite', hidden: true });
  const micLive = h('div', { class: 'mic-live', 'aria-live': 'polite' });
  const micBtn = h('button', { class: 'btn primary', type: 'button', onClick: toggleMic });
  const micPane = h('div', { class: 'pane' }, setupBox, recStatus, levels, silenceNote, outputNote, micLive, micBtn);

  const modelSel = h('select', { id: 'model-sel', onChange: () => { ui.model = modelSel.value; render(); } });
  const langSel = h('select', { id: 'lang-sel', onChange: () => { ui.language = langSel.value; } },
    LANGUAGES.map(([v, l]) => h('option', { value: v, text: l })));
  const getModels = h('button', { class: 'link', type: 'button', text: 'Baixar modelos', onClick: () => store.set(openSettings(store.get(), 'transcription')) });
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
    if (tab === 'mic' && !ui.inputsLoaded) { loadInputs(); loadOutputs(); }
    render();
  }

  // Carrega a lista de microfones; erros vão para o banner.
  async function loadInputs() {
    try {
      ui.inputs = await api.listAudioInputs();
      ui.inputsLoaded = true;
      ui.micId = pickMicId(ui.inputs, ui.micId ?? store.get().settings?.record_mic_id);
      micSel.replaceChildren(...ui.inputs.map((i) => h('option', { value: i.id, text: i.is_default ? `${i.name} (padrão)` : i.name })));
      if (ui.micId) micSel.value = ui.micId;
      store.set({ error: null });
    } catch (e) {
      store.set({ error: errorMessage(e) });
    }
  }

  // Carrega as saídas de som; se falhar, o seletor fica só com a primeira opção.
  async function loadOutputs() {
    try {
      ui.outputs = await api.listAudioOutputs();
    } catch (e) {
      console.error(e);
      ui.outputs = [];
    }
    ui.outputId = pickOutputId(ui.outputs, currentOutputId({
      loaded: ui.outputsLoaded, current: ui.outputId, saved: store.get().settings?.record_output_id,
    }));
    ui.outputsLoaded = true;
    outputSel.replaceChildren(...outputOptions(ui.outputs, mac).map((o) => h('option', { value: o.value, text: o.text })));
    outputSel.value = ui.outputId || '';
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

  function setLevels(p) {
    levelFills.voce.style.width = `${levelPercent(p?.mic)}%`;
    levelFills.reuniao.style.width = `${levelPercent(p?.system)}%`;
    if (typeof p?.system === 'number') ui.systemPeak = Math.max(ui.systemPeak, p.system);
    syncSilenceNote();
  }

  function syncSilenceNote() {
    const pinned = isPinned({ outputId: ui.recOutputId, notice: ui.outputNotice });
    silenceNote.textContent = silenceHintText({ pinned, mac });
    silenceNote.hidden = !(ui.micRunning && !ui.micStopping && showSilenceHint({
      mac, source: ui.recSource, peak: ui.systemPeak, elapsedMs: Date.now() - ui.startedAt, pinned,
    }));
  }

  function appendLive(p) {
    micLive.append(h('div', { text: liveLine(p) }));
    micLive.scrollTop = micLive.scrollHeight;
  }

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
    micLive.replaceChildren();
    setLevels(null);
    sessionError = null;
    recorder = createRecorder({
      onText: appendLive,
      onLevel: setLevels,
      onNotice: (message) => { ui.outputNotice = message; render(); },
      // O backend já encerrou a sessão: mostra o aviso e recolhe o resultado parcial.
      // Durante a abertura (`deferred`) a coleta espera start() terminar.
      onError: (message, deferred) => {
        sessionError = message;
        store.set({ error: message });
        if (!deferred) stopMic();
      },
    });
    const source = ui.source || pickSource(store.get().settings);
    ui.recSource = source;
    const outputId = showOutputPicker(source) ? ui.outputId : null;
    ui.recOutputId = outputId;
    ui.outputNotice = '';
    ui.systemPeak = 0;
    ui.starting = true;
    render();
    let pending;
    try {
      pending = await recorder.start({ source, micId: showMicPicker(source) ? ui.micId : null, outputId, model: ui.model, language: ui.language });
    } catch (e) {
      recorder = null;
      ui.starting = false;
      ui.outputNotice = '';
      store.set(failJob(store.get(), errorMessage(e)));
      return;
    }
    ui.starting = false;
    // Lembra a escolha (o backend já salvou).
    const cur = store.get().settings;
    if (cur) {
      store.set({ settings: { ...cur, record_source: source, record_mic_id: showMicPicker(source) ? ui.micId : cur.record_mic_id,
        record_output_id: showOutputPicker(source) ? ui.outputId : cur.record_output_id } });
    }
    ui.micRunning = true;
    ui.startedAt = Date.now();
    ui.elapsed = 0;
    clockTimer = setInterval(() => { ui.elapsed += 1; micTime.textContent = clock(ui.elapsed); }, 1000);
    render();
    if (pending) stopMic();
  }

  // Todo caminho (Finalizar, erro da sessão, erro na abertura) lê `sessionError`
  // depois do stop: um erro que chega durante a parada também fica no aviso.
  async function stopMic() {
    if (ui.micStopping || !recorder) return;
    ui.micStopping = true;
    stopClock();
    const job = store.get().job;
    if (job?.kind === 'mic') store.set({ job: { ...job, stopping: true } });
    render();
    const r = recorder;
    const finish = () => { ui.micRunning = ui.micStopping = false; recorder = null; ui.outputNotice = ''; };
    try {
      const result = toTranscript(await r.stop());
      finish();
      store.set(recordingResult(store.get(), result, { mac, source: ui.recSource, sessionError }));
    } catch (e) {
      finish();
      store.set(failJob(store.get(), recordingFailure(sessionError, errorMessage(e))));
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
    if (ui.starting) {
      micBtn.textContent = 'Iniciando…';
      micBtn.disabled = true;
    } else if (!ui.micRunning && !(busy && s.job.kind === 'mic')) {
      micBtn.textContent = 'Gravar';
      micBtn.disabled = !canGo();
    } else if (ui.micRunning) {
      micBtn.textContent = ui.micStopping ? 'Finalizando…' : 'Finalizar';
      micBtn.disabled = ui.micStopping;
    }
    micTime.textContent = clock(ui.elapsed);

    // Escolha de fonte e microfone
    if (!ui.source && s.settings) ui.source = pickSource(s.settings);
    const source = ui.source || 'mic';
    for (const r of sourceRadios) { r.input.checked = r.value === source; r.input.disabled = busy; }
    micRow.hidden = !showMicPicker(source);
    micSel.disabled = refreshBtn.disabled = busy;
    outputRow.hidden = !showOutputPicker(source);
    outputSel.disabled = outputRefreshBtn.disabled = busy;
    hint.hidden = !showHeadphoneHint(source);
    setupBox.hidden = ui.micRunning;
    // Indicador: some quando parado; pulsa gravando; parado e sem barras em "Finalizando…"
    recStatus.hidden = !ui.micRunning;
    recStatus.classList.toggle('finishing', ui.micStopping);
    recText.textContent = ui.micStopping ? 'Finalizando…' : 'Gravando';
    levels.hidden = !ui.micRunning || ui.micStopping;
    levelRows.voce.hidden = !showMicPicker(source);
    levelRows.reuniao.hidden = source === 'mic';
    outputNote.textContent = ui.outputNotice;
    outputNote.hidden = !showOutputNote({ running: ui.micRunning, message: ui.outputNotice });
    syncSilenceNote();

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

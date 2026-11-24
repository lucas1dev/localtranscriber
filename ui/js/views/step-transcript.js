// Passo ② — ler e editar a transcrição, copiar/exportar e seguir para o resumo.
import { errorMessage } from '../api.js';
import { h } from '../dom.js';
import { saveText, toSrt } from '../export.js';
import { goTo } from '../flow.js';
import { liveLine } from '../recorder.js';

// Texto do segmento na lista; na gravação dupla, com a etiqueta (Você/Reunião).
export const segmentText = (seg) => liveLine(seg);

export function mountStepTranscript(root, store) {
  let shown = null; // transcrição cujo texto está no textarea
  let shownSegments = null;
  let copyTimer = null;

  const meta = h('p', { class: 'muted transcript-meta' });
  const area = h('textarea', {
    class: 'transcript-area', 'aria-label': 'Transcrição', spellcheck: 'false',
    onInput: () => {
      const s = store.get();
      if (!s.transcript) return;
      const next = { ...s.transcript, text: area.value };
      shown = next;
      store.set({ transcript: next, edited: true });
    },
  });
  const segList = h('div', { class: 'segments' });
  const segTitle = h('summary', {});
  const segments = h('details', { class: 'segments-box' }, segTitle, segList);

  const copyBtn = h('button', { class: 'btn', type: 'button', text: 'Copiar', onClick: onCopy });
  const txtBtn = h('button', { class: 'btn', type: 'button', text: 'TXT', onClick: () => exportText('transcricao.txt', store.get().transcript.text) });
  const srtBtn = h('button', { class: 'btn', type: 'button', text: 'SRT', onClick: () => exportText('transcricao.srt', toSrt(store.get().transcript.segments)) });
  const back = h('button', { class: 'btn', type: 'button', text: '← Voltar', onClick: () => store.set(goTo(store.get(), 1)) });
  const next = h('button', {
    class: 'btn primary', type: 'button', text: 'Resumir →',
    onClick: () => {
      const s = store.get();
      store.set({ step: 3, maxStep: Math.max(s.maxStep, 3), error: null });
    },
  });
  const tools = h('div', { class: 'actions' }, copyBtn, txtBtn, srtBtn);
  const nav = h('div', { class: 'actions spread' }, back, next);

  root.append(meta, area, segments, tools, nav);

  async function exportText(name, contents) {
    try {
      await saveText(name, contents);
    } catch (e) {
      store.set({ error: errorMessage(e) });
    }
  }

  async function onCopy() {
    try {
      await navigator.clipboard.writeText(store.get().transcript.text);
    } catch (e) {
      store.set({ error: errorMessage(e) });
      return;
    }
    copyBtn.textContent = 'Copiado ✓';
    clearTimeout(copyTimer);
    copyTimer = setTimeout(() => { copyBtn.textContent = 'Copiar'; }, 1400);
  }

  function render(s) {
    root.hidden = !(s.view === 'steps' && s.step === 2);
    const t = s.transcript;
    if (!t) return;
    meta.textContent = [t.sourceName, t.model, t.language].filter(Boolean).join(' · ');
    if (t !== shown) { shown = t; area.value = t.text; }
    if (t.segments !== shownSegments) {
      shownSegments = t.segments;
      segList.replaceChildren(...t.segments.map((seg) => h('div', { class: 'segment' },
        h('span', { class: 'seg-time', text: seg.start }), h('span', { text: segmentText(seg) }))));
      segTitle.textContent = `Segmentos (${t.segments.length})`;
    }
    segments.hidden = t.segments.length === 0;
    srtBtn.disabled = t.segments.length === 0;
  }

  render(store.get());
  store.subscribe(render);
}

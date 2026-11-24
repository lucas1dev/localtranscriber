// Gravação do microfone em blocos de 12 s. Cada bloco usa um MediaRecorder próprio
// (arquivo completo e decodificável) e é transcrito em fila, um por vez.
import { api, errorMessage } from './api.js';
import { shiftTimestamp } from './time.js';

export const CHUNK_SECONDS = 12;

// onChunkText(texto) a cada bloco transcrito; onError(mensagem) em falhas.
export function createMic({ onChunkText, onError }) {
  let stream = null;
  let recording = false;
  let current = null;
  let timer = null;
  let index = 0;
  let queue = Promise.resolve();
  let model = '';
  let language = 'auto';
  let text = '';
  let segments = [];
  let sourceLanguage = null;

  function enqueue(blob, mime, chunkIndex) {
    queue = queue.then(async () => {
      try {
        const data = Array.from(new Uint8Array(await blob.arrayBuffer()));
        const t = await api.transcribeChunk(data, mime, model, language);
        const offset = chunkIndex * CHUNK_SECONDS;
        for (const s of t.segments || []) {
          segments.push({ ...s, start: shiftTimestamp(s.start, offset), end: shiftTimestamp(s.end, offset) });
        }
        const piece = (t.text || '').trim();
        if (piece) {
          text = text ? `${text} ${piece}` : piece;
          onChunkText(piece);
        }
        sourceLanguage = t.language ?? sourceLanguage;
      } catch (e) {
        onError(errorMessage(e));
      }
    });
  }

  function recordOne() {
    const rec = new MediaRecorder(stream);
    const parts = [];
    const chunkIndex = index++;
    current = rec;
    rec.ondataavailable = (ev) => { if (ev.data.size) parts.push(ev.data); };
    rec.onstop = () => {
      clearTimeout(timer);
      const mime = rec.mimeType || 'audio/webm';
      if (parts.length) enqueue(new Blob(parts, { type: mime }), mime, chunkIndex);
      if (recording) {
        recordOne();
      } else {
        stream?.getTracks().forEach((t) => t.stop());
        stream = null;
        current = null;
        finishStop?.();
      }
    };
    rec.start();
    timer = setTimeout(() => { if (rec.state !== 'inactive') rec.stop(); }, CHUNK_SECONDS * 1000);
  }

  let finishStop = null;

  return {
    async start(modelId, lang) {
      model = modelId;
      language = lang;
      text = '';
      segments = [];
      sourceLanguage = null;
      index = 0;
      queue = Promise.resolve();
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      recording = true;
      recordOne();
    },
    // Resolve com o resultado acumulado depois que a fila esvaziar.
    async stop() {
      if (recording) {
        recording = false;
        await new Promise((resolve) => {
          finishStop = resolve;
          if (current && current.state !== 'inactive') current.stop();
          else resolve();
        });
        finishStop = null;
      }
      await queue;
      return { text, segments, model, language: sourceLanguage ?? language, sourceName: 'Microfone' };
    },
  };
}

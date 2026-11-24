// Exportação de transcrições e resumos.
import { api } from './api.js';
import { SPEAKER_LABEL } from './recorder.js';

export function toSrt(segments) {
  return segments
    .map((s, i) => `${i + 1}\n${s.start} --> ${s.end}\n${s.speaker && SPEAKER_LABEL[s.speaker] ? `${SPEAKER_LABEL[s.speaker]}: ` : ''}${s.text}`)
    .join('\n\n');
}

const FILTERS = {
  txt: { name: 'Texto', extensions: ['txt'] },
  srt: { name: 'Legendas SRT', extensions: ['srt'] },
  md: { name: 'Markdown', extensions: ['md'] },
};

// Pergunta onde salvar (nome sugerido `name`) e grava `contents`.
// Devolve false se a pessoa cancelar o diálogo; erros de gravação são lançados.
export async function saveText(name, contents) {
  const filter = FILTERS[name.split('.').pop()];
  const path = await api.pickSavePath(name, filter ? [filter] : []);
  if (!path) return false;
  await api.saveTextFile(path, contents);
  return true;
}

// Escolhas iniciais do passo ③ (funções puras).
export const STYLES = [
  ['executive', 'Executivo'], ['detailed', 'Detalhado'], ['academic', 'Acadêmico'],
  ['meeting', 'Reunião / ata'], ['class', 'Aula / estudo'], ['podcast', 'Podcast'], ['custom', 'Personalizado'],
];
export const OLLAMA_DEFAULT = 'qwen3:8b';

// Formato inicial: o atual, senão o salvo (se válido) ou "executive". Sem configurações
// (ainda carregando, ou falharam), espera até o passo aparecer e usa "executive".
export function pickStyle(current, settings, visible) {
  if (current !== null) return current;
  if (settings) return STYLES.some(([v]) => v === settings.summary_style) ? settings.summary_style : 'executive';
  return visible ? 'executive' : null;
}

// Modelo do Ollama a pré-selecionar: null se o padrão está instalado (ou nada está);
// senão o primeiro modelo instalado.
export function ollamaDefaultModel(installed) {
  if (!installed.length || installed.includes(OLLAMA_DEFAULT)) return null;
  return installed[0];
}

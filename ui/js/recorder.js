// Gravação nativa: funções puras (testáveis) e ligação com os comandos e eventos do backend.
import { api, on } from './api.js';
import { afterTranscribe, NO_SPEECH } from './flow.js';

export const SPEAKER_LABEL = { voce: 'Você', reuniao: 'Reunião' };
export const SOURCES = ['mic', 'system', 'both'];

export function liveLine({ speaker, text }) {
  const label = SPEAKER_LABEL[speaker];
  return label ? `${label}: ${text}` : text;
}

// Nível de 0 a 1 vira porcentagem inteira de 0 a 100.
export function levelPercent(x) {
  if (typeof x !== 'number' || Number.isNaN(x)) return 0;
  return Math.round(Math.min(1, Math.max(0, x)) * 100);
}

export function pickSource(settings) {
  const s = settings?.record_source;
  return SOURCES.includes(s) ? s : 'mic';
}

export const showMicPicker = (source) => source !== 'system';
export const showHeadphoneHint = (source) => source === 'both';
const usesSystem = (source) => source === 'system' || source === 'both';

// Mesmo critério de settings-accounts.js: o WebKit do macOS se apresenta como Macintosh.
export const isMacUserAgent = (ua) => typeof ua === 'string' && ua.includes('Macintosh');

// No macOS, sem a permissão de gravar o som do computador, o tap entrega silêncio
// em vez de erro. Se Reunião ficar em zero desde o começo, um aviso discreto sugere
// conferir a permissão; some (para sempre) assim que algum som chega.
export const SILENCE_HINT_MS = 12_000;
export const SILENCE_HINT = 'Nada do som do computador até agora. Se algo está tocando, confira Ajustes do Sistema → Privacidade e Segurança → Gravação de áudio do sistema.';
const SILENT_LEVEL = 0.01;

// `peak`: maior nível de Reunião desde o começo; `elapsedMs`: tempo gravando.
export function silenceHint({ peak, elapsedMs }) {
  return elapsedMs >= SILENCE_HINT_MS && !(peak >= SILENT_LEVEL);
}

export function showSilenceHint({ mac, source, peak, elapsedMs }) {
  return mac && usesSystem(source) && silenceHint({ peak, elapsedMs });
}

const NO_SYSTEM_AUDIO = ' Se o som do computador não foi captado, confira a permissão em Ajustes do Sistema → Privacidade e Segurança → Gravação de áudio do sistema.';

// Como afterTranscribe; uma gravação sem fala no macOS com o som do computador
// também lembra a permissão (negada, ela vira silêncio, não erro).
export function afterRecording(state, transcript, { mac, source }) {
  const patch = afterTranscribe(state, transcript);
  if (patch.error === NO_SPEECH && mac && usesSystem(source)) {
    return { ...patch, error: NO_SPEECH + NO_SYSTEM_AUDIO };
  }
  return patch;
}

// Microfone salvo, se ainda existir; senão o padrão do sistema; senão o primeiro.
export function pickMicId(inputs, savedId) {
  if (inputs.length === 0) return null;
  if (savedId && inputs.some((i) => i.id === savedId)) return savedId;
  return (inputs.find((i) => i.is_default) || inputs[0]).id;
}

// Uma gravação por vez. Os ouvintes só existem entre start() e o fim de stop(),
// e eventos que chegam fora desse intervalo são ignorados.
// `deps` permite trocar api/on nos testes.
export function createRecorder({ onText, onLevel, onError }, deps = { api, on }) {
  const { api: backend, on: listen } = deps;
  let active = false;
  let starting = false;
  let pendingError = null;
  let unlisten = [];

  async function subscribe() {
    const subs = [
      listen('record://text', (p) => { if (active) onText(p); }),
      listen('record://level', (p) => { if (active) onLevel(p); }),
      listen('record://error', (p) => {
        if (!active) return;
        const message = p?.message || 'A gravação foi interrompida.';
        // O backend só guarda a gravação quando start_recording termina: durante a
        // abertura o erro fica na fila (start() devolve a mensagem) e o aviso aparece já.
        if (starting) pendingError = message;
        onError(message, starting);
      }),
    ];
    unlisten = await Promise.all(subs);
  }

  function unsubscribe() {
    active = false;
    for (const off of unlisten) off();
    unlisten = [];
  }

  return {
    get starting() { return starting; },
    // Devolve a mensagem de um erro de sessão ocorrido durante a abertura (ou null).
    async start({ source, micId, model, language }) {
      starting = true;
      pendingError = null;
      try {
        await subscribe();
        active = true;
        await backend.startRecording(source, micId, model, language);
      } catch (e) {
        unsubscribe();
        throw e;
      } finally {
        starting = false;
      }
      return pendingError;
    },
    // Devolve o resultado do backend (snake_case); sempre solta os ouvintes.
    async stop() {
      try {
        return await backend.stopRecording();
      } finally {
        unsubscribe();
      }
    },
  };
}

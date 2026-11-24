// Estado dos downloads (modelos Whisper e Ollama) mantido no store, fora das views:
// continua valendo ao sair da página de Configurações.
import { api, errorMessage, isCancelled, on } from './api.js';

export async function refreshModels(store) {
  try {
    const [settings, models] = await Promise.all([api.getSettings(), api.listModels()]);
    store.set({ settings, models });
  } catch (e) {
    store.set({ error: errorMessage(e) });
  }
}

const without = (map, key) => Object.fromEntries(Object.entries(map).filter(([k]) => k !== key));

// Registra os listeners globais uma única vez, na partida do app.
export function initDownloadEvents(store) {
  const listen = (event, cb) => on(event, cb).catch((e) => console.error(event, e));

  listen('model://progress', ({ id, done, total }) => {
    store.set({ downloads: { ...store.get().downloads, [id]: { done, total, error: null } } });
  });
  listen('model://done', ({ id, ok, error }) => {
    const { downloads } = store.get();
    if (ok || isCancelled(error)) store.set({ downloads: without(downloads, id) });
    else store.set({ downloads: { ...downloads, [id]: { done: 0, total: 0, error: errorMessage(error) } } });
    if (ok) refreshModels(store);
  });

  listen('ollama://progress', ({ name, status, completed, total }) => {
    store.set({ pulls: { ...store.get().pulls, [name]: { status, done: completed, total, error: null } } });
  });
  listen('ollama://done', ({ name, ok, error }) => {
    const { pulls, ollamaTick } = store.get();
    if (ok || isCancelled(error)) store.set({ pulls: without(pulls, name), ollamaTick: ollamaTick + 1 });
    else store.set({ pulls: { ...pulls, [name]: { status: '', done: 0, total: 0, error: errorMessage(error) } } });
  });
}

export function clearEntry(store, key, name) {
  store.set({ [key]: without(store.get()[key], name) });
}

export function percentOf(entry) {
  return entry.total > 0 ? Math.min(100, Math.round((entry.done / entry.total) * 100)) : null;
}

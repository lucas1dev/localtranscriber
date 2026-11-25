// Ponte para o backend Tauri. window.__TAURI__ é lido só dentro das funções (importável em testes no Node).
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);

export const api = {
  getStatus: () => invoke('get_status'),
  getSettings: () => invoke('get_settings'),
  setSettings: (settings) => invoke('set_settings', { settings }),
  openDataDir: () => invoke('open_data_dir'),
  saveTextFile: (path, contents) => invoke('save_text_file', { path, contents }),

  listModels: () => invoke('list_models'),
  downloadModel: (id) => invoke('download_model', { id }),
  cancelDownload: (id) => invoke('cancel_download', { id }),
  deleteModel: (id) => invoke('delete_model', { id }),
  setDefaultModel: (id) => invoke('set_default_model', { id }),
  importModel: (path) => invoke('import_model', { path }),

  transcribeFile: (path, model, language) => invoke('transcribe_file', { path, model, language }),
  listAudioInputs: () => invoke('list_audio_inputs'),
  listAudioOutputs: () => invoke('list_audio_outputs'),
  startRecording: (source, micId, outputId, model, language) => invoke('start_recording', { source, micId, outputId, model, language }),
  stopRecording: () => invoke('stop_recording'),
  confirmClose: () => invoke('confirm_close'),
  cancelTranscription: () => invoke('cancel_transcription'),

  providersStatus: () => invoke('providers_status'),
  saveApiKey: (provider, key) => invoke('save_api_key', { provider, key }),
  deleteApiKey: (provider) => invoke('delete_api_key', { provider }),
  codexLogin: () => invoke('codex_login'),
  codexLogout: () => invoke('codex_logout'),
  ollamaModels: () => invoke('ollama_models'),
  ollamaPull: (name) => invoke('ollama_pull', { name }),
  cancelOllamaPull: (name) => invoke('cancel_ollama_pull', { name }),
  ollamaDelete: (name) => invoke('ollama_delete', { name }),

  summarize: (req) => invoke('summarize', { req }),
  compare: (req) => invoke('compare', { req }),

  openUrl: (url) => window.__TAURI__.opener.openUrl(url),

  pickAudioFile: () => window.__TAURI__.dialog.open({
    multiple: false,
    directory: false,
    filters: [{ name: 'Áudio e vídeo', extensions: ['mp3', 'wav', 'm4a', 'mp4', 'mov', 'webm', 'ogg', 'flac', 'aac', 'aif', 'aiff'] }],
  }),
  // Diálogo "Salvar como"; devolve o caminho escolhido ou null se cancelado.
  pickSavePath: (defaultPath, filters) => window.__TAURI__.dialog.save({ defaultPath, filters }),
  pickModelFile: () => window.__TAURI__.dialog.open({
    multiple: false,
    directory: false,
    filters: [{ name: 'Modelo Whisper', extensions: ['bin'] }],
  }),
};

// Assina um evento do backend; devolve Promise<unlisten>. O callback recebe o payload.
export function on(event, cb) {
  return window.__TAURI__.event.listen(event, (ev) => cb(ev.payload));
}

export function errorMessage(e) {
  if (typeof e === 'string') return e;
  if (e && typeof e === 'object' && e.message) return e.message;
  return String(e);
}

export function isCancelled(e) {
  return e?.kind === 'cancelled';
}

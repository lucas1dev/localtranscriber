// Configurações › Contas de IA — assinatura, chave de API e local.
import { api, errorMessage } from '../api.js';
import { h, showText } from '../dom.js';
import { connectedLabel } from '../providers.js';

const API_KEY_PROVIDERS = ['openai', 'anthropic', 'gemini'];
const keychainName = () => (navigator.userAgent.includes('Windows')
  ? 'Gerenciador de Credenciais do Windows'
  : 'Keychain do macOS');

export function mountSettingsAccounts(root, store) {
  const ui = { providers: [], loggingIn: false, editing: new Set(), inputs: new Map(), wasVisible: false };
  const byId = (id) => ui.providers.find((p) => p.id === id);

  const error = h('p', { class: 'form-error', role: 'alert', hidden: true });

  // --- ChatGPT ---
  const codexState = h('span', { class: 'muted' });
  const codexWait = h('span', { class: 'muted', role: 'status', text: 'Conclua o login no navegador…', hidden: true });
  const codexLoginBtn = h('button', { class: 'btn', type: 'button', text: 'Entrar com ChatGPT', onClick: () => codexLogin() });
  const codexLogoutBtn = h('button', { class: 'btn', type: 'button', text: 'Sair', onClick: () => run(async () => { await api.codexLogout(); await refresh(); }) });
  const codexBox = h('div', { class: 'row' },
    h('div', { class: 'row-main' }, h('div', { class: 'row-info' }, h('strong', { text: 'ChatGPT' }), codexState),
      h('div', { class: 'row-actions' }, codexLoginBtn, codexLogoutBtn)), codexWait);

  // --- Claude Code ---
  const claudeToggle = h('input', { type: 'checkbox', role: 'switch', onChange: () => toggleClaude() });
  const claudeState = h('span', { class: 'muted' });
  const claudeBox = h('div', { class: 'row' },
    h('div', { class: 'row-main' }, h('div', { class: 'row-info' }, h('strong', { text: 'Claude Code' }), claudeState),
      h('label', { class: 'check' }, claudeToggle, h('span', { text: 'Usar o Claude Code instalado neste computador' }))),
    h('p', { class: 'muted small', text: 'Usa o Claude Code que você instalou e logou neste computador. O consumo conta no seu plano Claude. O app não faz login nem guarda credenciais do Claude.' }));

  // --- chaves de API ---
  const keyRows = new Map(API_KEY_PROVIDERS.map((id) => [id, buildKeyRow(id)]));

  function buildKeyRow(id) {
    const input = h('input', {
      type: 'password', autocomplete: 'off', spellcheck: 'false', placeholder: 'Cole a chave aqui',
      'aria-label': 'Chave de API', onKeydown: (ev) => { if (ev.key === 'Enter') saveKey(id); },
    });
    ui.inputs.set(id, input);
    const state = h('span', { class: 'muted' });
    const name = h('strong');
    const actions = h('div', { class: 'row-actions' });
    const form = h('div', { class: 'key-form' }, input,
      h('button', { class: 'btn', type: 'button', text: 'Salvar', onClick: () => saveKey(id) }),
      h('button', { class: 'btn', type: 'button', text: 'Cancelar', onClick: () => { input.value = ''; ui.editing.delete(id); render(); } }));
    const el = h('div', { class: 'row' }, h('div', { class: 'row-main' }, h('div', { class: 'row-info' }, name, state), actions), form);
    return { el, name, state, actions, form, cancel: form.lastChild };
  }

  root.append(
    h('h2', { text: 'Contas de IA' }), error,
    h('h3', { text: 'Pela sua assinatura' }), codexBox, claudeBox,
    h('h3', { text: 'Por chave de API' }),
    h('p', { class: 'muted small', text: 'Chaves de API são cobradas por uso na conta do provedor.' }),
    h('p', { class: 'muted small', text: `A chave fica guardada no ${keychainName()} e nunca volta a aparecer aqui.` }),
    ...[...keyRows.values()].map((r) => r.el),
    h('h3', { text: 'Local' }),
    h('p', { class: 'muted small', text: 'O Ollama roda modelos neste computador. Configure na seção Ollama.' }));

  // --- ações ---
  async function run(action) {
    showText(error, '');
    try {
      await action();
    } catch (e) {
      showText(error, errorMessage(e));
    }
  }

  async function refresh() {
    ui.providers = await api.providersStatus();
    render();
  }

  const codexLogin = () => run(async () => {
    ui.loggingIn = true;
    render();
    try {
      const status = await api.codexLogin();
      ui.providers = ui.providers.map((p) => (p.id === status.id ? status : p));
    } finally {
      ui.loggingIn = false;
      render();
    }
  });

  const toggleClaude = () => run(async () => {
    try {
      const saved = await api.setSettings({ ...store.get().settings, claude_code_enabled: claudeToggle.checked });
      store.set({ settings: saved });
    } finally {
      await refresh();
    }
  });

  const saveKey = (id) => run(async () => {
    const input = ui.inputs.get(id);
    const key = input.value;
    input.value = ''; // a chave não fica no DOM, nem se o salvamento falhar
    await api.saveApiKey(id, key);
    ui.editing.delete(id);
    await refresh();
  });

  // --- render ---
  function render() {
    const codex = byId('codex');
    const claude = byId('claude_code');
    if (codex) {
      const missing = !codex.configured && codex.detail.startsWith('Codex embutido');
      codexState.textContent = codex.configured ? connectedLabel(codex.detail) : codex.detail;
      codexLoginBtn.hidden = codex.configured;
      codexLoginBtn.disabled = ui.loggingIn || missing;
      codexLogoutBtn.hidden = !codex.configured;
    }
    codexWait.hidden = !ui.loggingIn;
    if (claude) {
      claudeState.textContent = claude.configured ? connectedLabel(claude.detail) : claude.detail;
      claudeToggle.checked = claude.enabled;
    }
    for (const id of API_KEY_PROVIDERS) {
      const p = byId(id);
      const row = keyRows.get(id);
      if (!p) continue;
      row.name.textContent = p.label;
      row.state.textContent = p.detail;
      const editing = !p.configured || ui.editing.has(id);
      row.form.hidden = !editing;
      row.cancel.hidden = !p.configured;
      row.actions.replaceChildren(...(p.configured && !editing ? [
        h('button', { class: 'btn', type: 'button', text: 'Trocar', onClick: () => { ui.editing.add(id); render(); ui.inputs.get(id).focus(); } }),
        h('button', { class: 'btn', type: 'button', text: 'Remover', onClick: () => run(async () => { await api.deleteApiKey(id); await refresh(); }) }),
      ] : []));
    }
  }

  store.subscribe((s) => {
    const visible = s.view === 'settings' && s.settingsSection === 'accounts';
    root.hidden = !visible;
    if (visible && !ui.wasVisible) { showText(error, ''); run(refresh); }
    if (!visible) for (const input of ui.inputs.values()) input.value = '';
    ui.wasVisible = visible;
  });
  root.hidden = true;
}

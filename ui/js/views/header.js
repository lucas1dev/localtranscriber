import { h } from '../dom.js';
import { closeSettings, goTo, openSettings } from '../flow.js';

const STEPS = ['① Áudio', '② Transcrição', '③ Resumo'];
const JOB_LABEL = { file: 'Transcrevendo…', mic: 'Gravando…', summary: 'Resumindo…' };

export function mountHeader(root, store) {
  const title = h('span', { class: 'brand', text: 'Local Transcriber' });
  const nav = h('nav', { class: 'steps', 'aria-label': 'Passos' });
  const jobBadge = h('span', { class: 'job-badge', hidden: true });
  const gear = h('button', {
    class: 'icon-btn', type: 'button', 'aria-label': 'Configurações', title: 'Configurações', text: '⚙',
    onClick: () => store.set(openSettings(store.get(), store.get().settingsSection)),
  });
  root.append(h('div', { class: 'header-inner' }, title, nav, jobBadge, gear));

  const buttons = STEPS.map((label, i) => {
    const b = h('button', {
      class: 'step-btn', type: 'button', text: label,
      onClick: () => store.set({ ...closeSettings(store.get()), ...goTo(store.get(), i + 1) }),
    });
    return b;
  });
  buttons.forEach((b, i) => {
    if (i > 0) nav.append(h('span', { class: 'step-sep', 'aria-hidden': 'true', text: '─' }));
    nav.append(b);
  });

  const render = (s) => {
    buttons.forEach((b, i) => {
      const n = i + 1;
      b.disabled = n > s.maxStep;
      b.classList.toggle('current', s.view === 'steps' && n === s.step);
      if (s.view === 'steps' && n === s.step) b.setAttribute('aria-current', 'step');
      else b.removeAttribute('aria-current');
    });
    jobBadge.hidden = s.job === null;
    if (s.job) {
      const base = JOB_LABEL[s.job.kind] || 'Processando…';
      jobBadge.textContent = s.job.percent != null && s.job.kind === 'file' ? `${base} ${s.job.percent}%` : base;
    }
  };
  render(store.get());
  store.subscribe(render);
}

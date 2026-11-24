// Fechar a janela com uma gravação em andamento: o backend segura o fechamento e
// avisa (app://close-requested); aqui a pessoa confirma ou continua gravando.
import { api, errorMessage, on } from '../api.js';
import { h } from '../dom.js';

export const CLOSE_EVENT = 'app://close-requested';
export const CLOSE_CONFIRM = 'Uma gravação está em andamento. Fechar agora descarta a gravação. Fechar mesmo assim?';

export function mountCloseGuard(root, store) {
  const box = h('div', { class: 'confirm close-guard', role: 'alertdialog', hidden: true },
    h('span', { text: CLOSE_CONFIRM }),
    h('button', {
      class: 'btn danger', type: 'button', text: 'Fechar',
      onClick: () => api.confirmClose().catch((e) => { box.hidden = true; store.set({ error: errorMessage(e) }); }),
    }),
    h('button', { class: 'btn', type: 'button', text: 'Continuar gravando', onClick: () => { box.hidden = true; } }));
  root.append(box);
  on(CLOSE_EVENT, () => { box.hidden = false; })
    .catch((e) => { console.error(e); store.set({ error: errorMessage(e) }); });
}

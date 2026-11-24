import { h } from '../dom.js';

export function mountErrorBanner(root, store) {
  const msg = h('span', { class: 'banner-text' });
  const close = h('button', {
    class: 'banner-close', type: 'button', 'aria-label': 'Fechar',
    text: '×', onClick: () => store.set({ error: null }),
  });
  const box = h('div', { class: 'banner', role: 'alert', hidden: true }, msg, close);
  root.append(box);
  const render = (s) => {
    box.hidden = !s.error;
    msg.textContent = s.error || '';
  };
  render(store.get());
  store.subscribe(render);
}

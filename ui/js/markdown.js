// Markdown -> HTML sanitizado. Lê window.marked/window.DOMPurify (ui/vendor/) só na hora do uso.
const SANITIZE = { FORBID_TAGS: ['form', 'input', 'button', 'textarea', 'select'] };

export function renderMarkdown(md) {
  return window.DOMPurify.sanitize(window.marked.parse(md), SANITIZE);
}

// Clique num resumo renderizado: nenhum link navega a janela do app (a sessão fica só
// na memória). Links http(s) abrem no navegador por `openUrl`; os demais são ignorados.
export function interceptLinks(ev, openUrl) {
  const link = ev.target?.closest?.('a');
  if (!link) return;
  ev.preventDefault();
  let url;
  try {
    url = new URL(link.getAttribute('href') ?? '');
  } catch {
    return;
  }
  if (url.protocol === 'http:' || url.protocol === 'https:') openUrl(url.href);
}

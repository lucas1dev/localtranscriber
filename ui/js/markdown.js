// Markdown -> HTML sanitizado. Lê window.marked/window.DOMPurify (ui/vendor/) só na hora do uso.
export function renderMarkdown(md) {
  return window.DOMPurify.sanitize(window.marked.parse(md));
}

// Exportação de transcrições.
export function toSrt(segments) {
  return segments
    .map((s, i) => `${i + 1}\n${s.start} --> ${s.end}\n${s.text}`)
    .join('\n\n');
}

export function downloadText(name, content) {
  const url = URL.createObjectURL(new Blob([content], { type: 'text/plain;charset=utf-8' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

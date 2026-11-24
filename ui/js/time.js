// Desloca um timestamp "HH:MM:SS,mmm" por um número de segundos.
export function shiftTimestamp(ts, seconds) {
  const m = /^(\d+):(\d{2}):(\d{2}),(\d{3})$/.exec(ts);
  if (!m) return ts;
  const total = ((+m[1] * 60 + +m[2]) * 60 + +m[3]) * 1000 + +m[4] + Math.round(seconds * 1000);
  const pad = (n, w) => String(n).padStart(w, '0');
  const ms = total % 1000;
  const s = Math.floor(total / 1000);
  return `${pad(Math.floor(s / 3600), 2)}:${pad(Math.floor(s / 60) % 60, 2)}:${pad(s % 60, 2)},${pad(ms, 3)}`;
}

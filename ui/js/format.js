// Formatação de tamanhos em pt-BR.
const mb = new Intl.NumberFormat('pt-BR', { maximumFractionDigits: 0 });
const gb = new Intl.NumberFormat('pt-BR', { maximumFractionDigits: 1 });

export function formatBytes(n) {
  return n < 1e9 ? `${mb.format(n / 1e6)} MB` : `${gb.format(n / 1e9)} GB`;
}

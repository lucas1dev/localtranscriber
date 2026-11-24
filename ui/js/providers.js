// Filtros puros sobre ProviderStatusDto {id, label, kind, configured, enabled, detail}.
export function usableProviders(list) {
  return list.filter((p) => !(p.id === 'claude_code' && !p.enabled));
}

export function configuredProviders(list) {
  return usableProviders(list).filter((p) => p.configured);
}

// Provedor salvo, se configurado; senão o primeiro configurado; senão null.
export function initialProvider(list, saved) {
  const ids = configuredProviders(list).map((p) => p.id);
  return ids.includes(saved) ? saved : (ids[0] ?? null);
}

// Texto de uma conta conectada: "Conectado: p@x.com", ou só "Conectado" quando o
// detalhe é vazio ou já diz "conectado".
export function connectedLabel(detail) {
  const d = (detail ?? '').trim();
  return d && d.toLowerCase() !== 'conectado' ? `Conectado: ${d}` : 'Conectado';
}

// Monta os pedidos de resumo/comparação (snake_case, como o backend espera).
const instructions = (form) => (form.style === 'custom' ? form.customInstructions : '');

export function buildSummaryRequest(state, form) {
  return {
    text: state.transcript.text,
    provider: form.provider,
    style: form.style,
    model: form.model.trim() || null,
    custom_instructions: instructions(form),
  };
}

// O modelo digitado vale só para o provedor do seletor, se ele estiver entre os comparados.
export function buildCompareRequest(state, form) {
  const model = form.model.trim();
  const models = model && form.providers.includes(form.provider) ? { [form.provider]: model } : {};
  return {
    text: state.transcript.text,
    providers: form.providers,
    style: form.style,
    models,
    custom_instructions: instructions(form),
  };
}

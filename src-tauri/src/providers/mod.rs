use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Codex,
    Openai,
    Anthropic,
    Gemini,
    Ollama,
    ClaudeCode,
}

pub fn label(id: ProviderId) -> &'static str {
    match id {
        ProviderId::Codex => "ChatGPT (assinatura)",
        ProviderId::Openai => "OpenAI (API)",
        ProviderId::Anthropic => "Claude (API)",
        ProviderId::Gemini => "Gemini (API)",
        ProviderId::Ollama => "Ollama (local)",
        ProviderId::ClaudeCode => "Claude Code (assinatura)",
    }
}

/// `None` para Codex e Claude Code: cada CLI usa o próprio padrão.
pub fn default_model(id: ProviderId) -> Option<&'static str> {
    match id {
        ProviderId::Openai => Some("gpt-5.1"),
        ProviderId::Anthropic => Some("claude-sonnet-4-5"),
        ProviderId::Gemini => Some("gemini-2.5-flash"),
        ProviderId::Ollama => Some("qwen3:8b"),
        ProviderId::Codex | ProviderId::ClaudeCode => None,
    }
}

pub fn keychain_account(id: ProviderId) -> Option<&'static str> {
    match id {
        ProviderId::Openai => Some("openai"),
        ProviderId::Anthropic => Some("anthropic"),
        ProviderId::Gemini => Some("gemini"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_id_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&ProviderId::ClaudeCode).unwrap(), "\"claude_code\"");
    }

    #[test]
    fn keychain_accounts_match_python_app() {
        assert_eq!(keychain_account(ProviderId::Openai), Some("openai"));
        assert_eq!(keychain_account(ProviderId::Anthropic), Some("anthropic"));
        assert_eq!(keychain_account(ProviderId::Gemini), Some("gemini"));
        assert_eq!(keychain_account(ProviderId::Codex), None);
        assert_eq!(keychain_account(ProviderId::Ollama), None);
        assert_eq!(keychain_account(ProviderId::ClaudeCode), None);
    }

    #[test]
    fn labels_and_default_models() {
        assert_eq!(label(ProviderId::Codex), "ChatGPT (assinatura)");
        assert_eq!(label(ProviderId::ClaudeCode), "Claude Code (assinatura)");
        assert_eq!(default_model(ProviderId::Openai), Some("gpt-5.1"));
        assert_eq!(default_model(ProviderId::Ollama), Some("qwen3:8b"));
        assert_eq!(default_model(ProviderId::Codex), None);
        assert_eq!(default_model(ProviderId::ClaudeCode), None);
    }
}

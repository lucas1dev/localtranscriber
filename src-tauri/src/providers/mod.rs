pub mod claude_code;
pub mod cli;
pub mod codex;
pub mod http;
pub mod ollama;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::credentials::SecretStore;
use crate::error::AppError;
use crate::summarize::Llm;

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

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub openai: String,
    pub anthropic: String,
    pub gemini: String,
    pub ollama: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            openai: "https://api.openai.com".into(),
            anthropic: "https://api.anthropic.com".into(),
            gemini: "https://generativelanguage.googleapis.com".into(),
            ollama: "http://127.0.0.1:11434".into(),
        }
    }
}

pub struct ProviderCtx {
    pub http: reqwest::Client,
    pub endpoints: Endpoints,
    pub secrets: Arc<dyn SecretStore>,
    pub codex: codex::Codex,
    pub claude: Option<claude_code::ClaudeCode>,
}

/// Nome do provedor de API na mensagem de chave ausente.
fn api_name(id: ProviderId) -> &'static str {
    match id {
        ProviderId::Openai => "OpenAI",
        ProviderId::Anthropic => "Anthropic",
        ProviderId::Gemini => "Gemini",
        _ => label(id),
    }
}

fn api_key(id: ProviderId, ctx: &ProviderCtx) -> Result<String, AppError> {
    let stored = match keychain_account(id) {
        Some(account) => ctx.secrets.get(account)?,
        None => None,
    };
    stored.ok_or_else(|| {
        AppError::User(format!(
            "Adicione uma chave de API da {} em Configurações → Contas de IA.",
            api_name(id)
        ))
    })
}

/// Monta o provedor `id`; usa `default_model(id)` quando `model` é `None`.
pub fn build(
    id: ProviderId,
    model: Option<String>,
    ctx: &ProviderCtx,
) -> Result<Box<dyn Llm>, AppError> {
    let model = model.or_else(|| default_model(id).map(String::from));
    let http = ctx.http.clone();
    let e = &ctx.endpoints;
    Ok(match id {
        ProviderId::Openai => Box::new(http::OpenAi {
            http,
            base: e.openai.clone(),
            key: api_key(id, ctx)?,
            model: model.unwrap_or_default(),
        }),
        ProviderId::Anthropic => Box::new(http::Anthropic {
            http,
            base: e.anthropic.clone(),
            key: api_key(id, ctx)?,
            model: model.unwrap_or_default(),
        }),
        ProviderId::Gemini => Box::new(http::Gemini {
            http,
            base: e.gemini.clone(),
            key: api_key(id, ctx)?,
            model: model.unwrap_or_default(),
        }),
        ProviderId::Ollama => Box::new(ollama::Ollama {
            http,
            base: e.ollama.clone(),
            model: model.unwrap_or_default(),
        }),
        ProviderId::Codex => Box::new(ctx.codex.llm(model)),
        ProviderId::ClaudeCode => match &ctx.claude {
            Some(claude) => Box::new(claude.llm(model)),
            None => {
                return Err(AppError::User(
                    "Claude Code não encontrado neste computador.".into(),
                ))
            }
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::MemoryStore;

    fn ctx(secrets: MemoryStore, claude: Option<claude_code::ClaudeCode>) -> ProviderCtx {
        ProviderCtx {
            http: reqwest::Client::new(),
            endpoints: Endpoints::default(),
            secrets: Arc::new(secrets),
            codex: codex::Codex {
                bin: "codex".into(),
                home: "codex-home".into(),
            },
            claude,
        }
    }

    fn user_err(r: Result<Box<dyn Llm>, AppError>) -> String {
        match r {
            Err(AppError::User(m)) => m,
            Err(e) => panic!("esperava User, veio {e:?}"),
            Ok(_) => panic!("esperava erro"),
        }
    }

    #[tokio::test]
    async fn build_without_key_is_user_error() {
        let c = ctx(MemoryStore::new(), None);
        let m = user_err(build(ProviderId::Openai, None, &c));
        assert!(m.contains("Configurações → Contas de IA"), "{m}");
        assert_eq!(
            m,
            "Adicione uma chave de API da OpenAI em Configurações → Contas de IA."
        );
        assert_eq!(
            user_err(build(ProviderId::Anthropic, None, &c)),
            "Adicione uma chave de API da Anthropic em Configurações → Contas de IA."
        );
        assert_eq!(
            user_err(build(ProviderId::Gemini, None, &c)),
            "Adicione uma chave de API da Gemini em Configurações → Contas de IA."
        );
    }

    #[tokio::test]
    async fn build_with_key_uses_default_model_and_endpoint() {
        use wiremock::matchers::{body_partial_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "sk-ant-123"))
            .and(body_partial_json(
                serde_json::json!({"model":"claude-sonnet-4-5"}),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"content":[{"type":"text","text":"ok"}]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let store = MemoryStore::new();
        store.set("anthropic", "sk-ant-123").unwrap();
        let mut c = ctx(store, None);
        c.endpoints.anthropic = server.uri();
        let llm = build(ProviderId::Anthropic, None, &c).unwrap();
        assert_eq!(llm.complete("S", "U").await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn build_claude_code_requires_installation() {
        let c = ctx(MemoryStore::new(), None);
        assert_eq!(
            user_err(build(ProviderId::ClaudeCode, None, &c)),
            "Claude Code não encontrado neste computador."
        );
        let c = ctx(
            MemoryStore::new(),
            Some(claude_code::ClaudeCode {
                bin: "claude".into(),
            }),
        );
        assert!(build(ProviderId::ClaudeCode, None, &c).is_ok());
        assert!(build(ProviderId::Codex, None, &c).is_ok());
        assert!(build(ProviderId::Ollama, None, &c).is_ok());
    }

    #[test]
    fn endpoints_default() {
        let e = Endpoints::default();
        assert_eq!(e.openai, "https://api.openai.com");
        assert_eq!(e.anthropic, "https://api.anthropic.com");
        assert_eq!(e.gemini, "https://generativelanguage.googleapis.com");
        assert_eq!(e.ollama, "http://127.0.0.1:11434");
    }

    #[test]
    fn provider_id_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&ProviderId::ClaudeCode).unwrap(),
            "\"claude_code\""
        );
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

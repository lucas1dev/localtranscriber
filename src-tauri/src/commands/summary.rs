use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::providers::provider_ctx;
use crate::error::AppError;
use crate::providers::{self, ProviderId};
use crate::state::AppState;
use crate::summarize::{run_summary, Llm};

#[derive(Deserialize)]
pub struct SummaryRequest {
    pub text: String,
    pub provider: ProviderId,
    pub style: String,
    pub model: Option<String>,
    #[serde(default)]
    pub custom_instructions: String,
}

#[derive(Serialize)]
pub struct SummaryResult {
    pub provider: ProviderId,
    pub model: Option<String>,
    pub style: String,
    pub summary: String,
    pub chunks_processed: usize,
}

#[derive(Deserialize)]
pub struct CompareRequest {
    pub text: String,
    pub providers: Vec<ProviderId>,
    pub style: String,
    #[serde(default)]
    pub models: HashMap<ProviderId, String>,
    #[serde(default)]
    pub custom_instructions: String,
}

#[derive(Serialize)]
pub struct CompareItem {
    pub provider: ProviderId,
    pub model: Option<String>,
    pub summary: Option<String>,
    pub error: Option<AppError>,
}

/// Remove duplicados preservando a ordem.
fn dedupe(providers: &[ProviderId]) -> Vec<ProviderId> {
    let mut seen = HashSet::new();
    providers.iter().copied().filter(|p| seen.insert(*p)).collect()
}

pub fn validate_compare(providers: &[ProviderId]) -> Result<(), AppError> {
    if (2..=4).contains(&dedupe(providers).len()) {
        Ok(())
    } else {
        Err(AppError::User(
            "Escolha de 2 a 4 provedores para comparar.".into(),
        ))
    }
}

fn effective_model(id: ProviderId, requested: Option<String>) -> Option<String> {
    requested.or_else(|| providers::default_model(id).map(String::from))
}

pub async fn run_compare(
    req_providers: &[ProviderId],
    models: &HashMap<ProviderId, String>,
    text: &str,
    style: &str,
    custom_instructions: &str,
    builder: impl Fn(ProviderId) -> Result<Box<dyn Llm>, AppError>,
) -> Vec<CompareItem> {
    let ids = dedupe(req_providers);
    let tasks = ids.into_iter().map(|id| {
        let model = effective_model(id, models.get(&id).cloned());
        let llm = builder(id);
        async move {
            let result = match llm {
                Ok(llm) => run_summary(llm.as_ref(), text, style, custom_instructions)
                    .await
                    .map(|(summary, _)| summary),
                Err(e) => Err(e),
            };
            match result {
                Ok(summary) => CompareItem {
                    provider: id,
                    model,
                    summary: Some(summary),
                    error: None,
                },
                Err(e) => CompareItem {
                    provider: id,
                    model,
                    summary: None,
                    error: Some(e),
                },
            }
        }
    });
    futures_util::future::join_all(tasks).await
}

const CLAUDE_DISABLED: &str = "Ative o Claude Code em Configurações → Contas de IA.";

fn ensure_enabled(state: &AppState, id: ProviderId) -> Result<(), AppError> {
    let enabled = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .claude_code_enabled;
    if id == ProviderId::ClaudeCode && !enabled {
        return Err(AppError::User(CLAUDE_DISABLED.into()));
    }
    Ok(())
}

#[tauri::command]
pub async fn summarize(
    state: State<'_, Arc<AppState>>,
    req: SummaryRequest,
) -> Result<SummaryResult, AppError> {
    ensure_enabled(&state, req.provider)?;
    let ctx = provider_ctx(&state);
    let llm = providers::build(req.provider, req.model.clone(), &ctx)?;
    let (summary, chunks_processed) =
        run_summary(llm.as_ref(), &req.text, &req.style, &req.custom_instructions).await?;
    Ok(SummaryResult {
        provider: req.provider,
        model: effective_model(req.provider, req.model),
        style: req.style,
        summary,
        chunks_processed,
    })
}

#[tauri::command]
pub async fn compare(
    state: State<'_, Arc<AppState>>,
    req: CompareRequest,
) -> Result<Vec<CompareItem>, AppError> {
    validate_compare(&req.providers)?;
    for id in &req.providers {
        ensure_enabled(&state, *id)?;
    }
    let ctx = provider_ctx(&state);
    Ok(run_compare(
        &req.providers,
        &req.models,
        &req.text,
        &req.style,
        &req.custom_instructions,
        |id| providers::build(id, req.models.get(&id).cloned(), &ctx),
    )
    .await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct FakeLlm(Result<String, String>);

    #[async_trait]
    impl Llm for FakeLlm {
        async fn complete(&self, _s: &str, _u: &str) -> Result<String, AppError> {
            self.0.clone().map_err(AppError::User)
        }
    }

    #[test]
    fn compare_requires_2_to_4() {
        use ProviderId::*;
        assert!(validate_compare(&[Codex]).is_err());
        assert!(validate_compare(&[Codex, Codex]).is_err());
        assert!(validate_compare(&[Codex, Openai]).is_ok());
        assert!(validate_compare(&[Codex, Openai, Codex]).is_ok());
        assert!(validate_compare(&[Codex, Openai, Anthropic, Gemini]).is_ok());
        assert!(validate_compare(&[Codex, Openai, Anthropic, Gemini, Ollama]).is_err());
        match validate_compare(&[]) {
            Err(AppError::User(m)) => {
                assert_eq!(m, "Escolha de 2 a 4 provedores para comparar.")
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn compare_isolates_errors() {
        let items = run_compare(
            &[ProviderId::Openai, ProviderId::Gemini, ProviderId::Openai],
            &HashMap::new(),
            "texto",
            "executive",
            "",
            |id| {
                Ok(Box::new(match id {
                    ProviderId::Openai => FakeLlm(Ok("resumo".into())),
                    _ => FakeLlm(Err("falhou".into())),
                }) as Box<dyn Llm>)
            },
        )
        .await;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].provider, ProviderId::Openai);
        assert_eq!(items[0].summary.as_deref(), Some("resumo"));
        assert!(items[0].error.is_none());
        assert_eq!(items[0].model.as_deref(), Some("gpt-5.1"));
        assert_eq!(items[1].provider, ProviderId::Gemini);
        assert!(items[1].summary.is_none());
        assert!(matches!(&items[1].error, Some(AppError::User(m)) if m == "falhou"));
    }

    #[tokio::test]
    async fn compare_builder_error_is_isolated() {
        let items = run_compare(
            &[ProviderId::Openai, ProviderId::Gemini],
            &HashMap::new(),
            "texto",
            "executive",
            "",
            |id| match id {
                ProviderId::Openai => Err(AppError::User("sem chave".into())),
                _ => Ok(Box::new(FakeLlm(Ok("ok".into()))) as Box<dyn Llm>),
            },
        )
        .await;
        assert!(items[0].error.is_some());
        assert_eq!(items[1].summary.as_deref(), Some("ok"));
    }
}

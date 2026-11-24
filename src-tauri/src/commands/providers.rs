use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, State};

use super::models::{claim, release};
use crate::credentials;
use crate::error::AppError;
use crate::providers::codex::{AuthStatus, Codex};
use crate::providers::{self, claude_code, ollama, ProviderCtx, ProviderId};
use crate::state::AppState;

const ORDER: [ProviderId; 6] = [
    ProviderId::Codex,
    ProviderId::ClaudeCode,
    ProviderId::Openai,
    ProviderId::Anthropic,
    ProviderId::Gemini,
    ProviderId::Ollama,
];

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ProviderStatusDto {
    pub id: ProviderId,
    pub label: String,
    pub kind: &'static str,
    pub configured: bool,
    pub enabled: bool,
    pub detail: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct OllamaDto {
    pub running: bool,
    pub models: Vec<String>,
}

fn kind(id: ProviderId) -> &'static str {
    match id {
        ProviderId::Codex | ProviderId::ClaudeCode => "subscription",
        ProviderId::Ollama => "local",
        _ => "api_key",
    }
}

/// Monta o status de todos os provedores a partir dos dados já coletados.
pub fn assemble_status(
    codex: &AuthStatus,
    claude: Option<&AuthStatus>,
    claude_enabled: bool,
    keys: &HashSet<ProviderId>,
    ollama: &OllamaDto,
) -> Vec<ProviderStatusDto> {
    ORDER
        .iter()
        .map(|&id| {
            let (configured, detail) = match id {
                ProviderId::Codex => match codex {
                    AuthStatus::LoggedIn(d) => (true, d.clone()),
                    AuthStatus::LoggedOut => (false, "Não conectado".into()),
                    AuthStatus::Missing => (
                        false,
                        "Codex embutido não encontrado — reinstale o app".into(),
                    ),
                },
                ProviderId::ClaudeCode => match (claude_enabled, claude) {
                    (false, _) => (false, "Desativado".into()),
                    (true, None | Some(AuthStatus::Missing)) => {
                        (false, "Claude Code não encontrado neste computador".into())
                    }
                    (true, Some(AuthStatus::LoggedOut)) => {
                        (false, "Claude Code não está logado".into())
                    }
                    (true, Some(AuthStatus::LoggedIn(d))) => (true, d.clone()),
                },
                ProviderId::Ollama => {
                    if !ollama.running {
                        (false, "Ollama não encontrado".into())
                    } else if ollama.models.is_empty() {
                        (false, "Ollama rodando, nenhum modelo baixado".into())
                    } else {
                        (
                            true,
                            format!("Ollama rodando · {} modelo(s)", ollama.models.len()),
                        )
                    }
                }
                _ => {
                    if keys.contains(&id) {
                        (true, "Chave salva no keychain".into())
                    } else {
                        (false, "Sem chave".into())
                    }
                }
            };
            ProviderStatusDto {
                id,
                label: providers::label(id).to_string(),
                kind: kind(id),
                configured,
                enabled: id != ProviderId::ClaudeCode || claude_enabled,
                detail,
            }
        })
        .collect()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn claude_enabled(state: &AppState) -> bool {
    state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .claude_code_enabled
}

pub fn provider_ctx(state: &AppState) -> ProviderCtx {
    ProviderCtx {
        http: state.http.clone(),
        endpoints: state.endpoints.clone(),
        secrets: state.secrets.clone(),
        codex: Codex {
            bin: state.sidecars.codex.clone(),
            home: state.paths.codex_home.clone(),
        },
        claude: if claude_enabled(state) {
            claude_code::locate(&home_dir(), std::env::var_os("PATH"))
        } else {
            None
        },
    }
}

async fn ollama_info(ctx: &ProviderCtx) -> OllamaDto {
    let base = &ctx.endpoints.ollama;
    if !ollama::is_running(&ctx.http, base).await {
        return OllamaDto {
            running: false,
            models: vec![],
        };
    }
    OllamaDto {
        running: true,
        models: ollama::list_models(&ctx.http, base).await.unwrap_or_default(),
    }
}

fn stored_keys(ctx: &ProviderCtx) -> HashSet<ProviderId> {
    ORDER
        .iter()
        .copied()
        .filter(|&id| {
            providers::keychain_account(id)
                .map(|acc| matches!(ctx.secrets.get(acc), Ok(Some(_))))
                .unwrap_or(false)
        })
        .collect()
}

async fn collect_status(state: &AppState) -> Vec<ProviderStatusDto> {
    let ctx = provider_ctx(state);
    let enabled = claude_enabled(state);
    let claude_status = async {
        match &ctx.claude {
            Some(c) => Some(c.status().await),
            None => None,
        }
    };
    let (codex, claude, ollama) =
        tokio::join!(ctx.codex.status(), claude_status, ollama_info(&ctx));
    assemble_status(&codex, claude.as_ref(), enabled, &stored_keys(&ctx), &ollama)
}

fn pick(all: Vec<ProviderStatusDto>, id: ProviderId) -> Result<ProviderStatusDto, AppError> {
    all.into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| AppError::Internal("Provedor desconhecido.".into()))
}

#[tauri::command]
pub async fn providers_status(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<ProviderStatusDto>, AppError> {
    Ok(collect_status(&state).await)
}

#[tauri::command]
pub async fn save_api_key(
    state: State<'_, Arc<AppState>>,
    provider: ProviderId,
    key: String,
) -> Result<ProviderStatusDto, AppError> {
    let account = providers::keychain_account(provider)
        .ok_or_else(|| AppError::User("Este provedor não usa chave de API.".into()))?;
    state.secrets.set(account, &credentials::normalize_key(&key)?)?;
    pick(collect_status(&state).await, provider)
}

#[tauri::command]
pub fn delete_api_key(state: State<'_, Arc<AppState>>, provider: ProviderId) -> Result<(), AppError> {
    let account = providers::keychain_account(provider)
        .ok_or_else(|| AppError::User("Este provedor não usa chave de API.".into()))?;
    state.secrets.delete(account)
}

#[tauri::command]
pub async fn codex_login(state: State<'_, Arc<AppState>>) -> Result<ProviderStatusDto, AppError> {
    provider_ctx(&state).codex.login().await?;
    pick(collect_status(&state).await, ProviderId::Codex)
}

#[tauri::command]
pub async fn codex_logout(state: State<'_, Arc<AppState>>) -> Result<(), AppError> {
    provider_ctx(&state).codex.logout().await
}

#[tauri::command]
pub async fn ollama_models(state: State<'_, Arc<AppState>>) -> Result<OllamaDto, AppError> {
    Ok(ollama_info(&provider_ctx(&state)).await)
}

#[tauri::command]
pub async fn ollama_pull(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<(), AppError> {
    let token = claim(
        &state.ollama_pulls,
        &name,
        "Este modelo já está sendo baixado no Ollama.",
    )?;
    let state = Arc::clone(&state);
    tokio::spawn(async move {
        let progress_app = app.clone();
        let progress_name = name.clone();
        let result = ollama::pull(
            &state.http,
            &state.endpoints.ollama,
            &name,
            token,
            move |status, completed, total| {
                let _ = progress_app.emit(
                    "ollama://progress",
                    json!({ "name": progress_name, "status": status, "completed": completed, "total": total }),
                );
            },
        )
        .await;
        release(&state.ollama_pulls, &name);
        let (ok, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e)),
        };
        let _ = app.emit("ollama://done", json!({ "name": name, "ok": ok, "error": error }));
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_ollama_pull(state: State<'_, Arc<AppState>>, name: String) {
    if let Some(t) = state
        .ollama_pulls
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&name)
    {
        t.cancel();
    }
}

#[tauri::command]
pub async fn ollama_delete(state: State<'_, Arc<AppState>>, name: String) -> Result<(), AppError> {
    ollama::delete(&state.http, &state.endpoints.ollama, &name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dto(keys: &[ProviderId], claude: Option<&AuthStatus>, enabled: bool, o: &OllamaDto) -> Vec<ProviderStatusDto> {
        assemble_status(
            &AuthStatus::LoggedOut,
            claude,
            enabled,
            &keys.iter().copied().collect(),
            o,
        )
    }

    fn off() -> OllamaDto {
        OllamaDto { running: false, models: vec![] }
    }

    #[test]
    fn status_order_and_kinds() {
        let s = dto(&[ProviderId::Openai], None, false, &off());
        let ids: Vec<_> = s.iter().map(|x| x.id).collect();
        assert_eq!(ids, ORDER.to_vec());
        assert_eq!(s[0].kind, "subscription");
        assert_eq!(s[1].kind, "subscription");
        assert_eq!(s[2].kind, "api_key");
        assert_eq!(s[5].kind, "local");
        assert!(s[2].configured && s[2].detail == "Chave salva no keychain");
        assert!(!s[3].configured && s[3].detail == "Sem chave");
        assert_eq!(s[0].detail, "Não conectado");
        assert_eq!(s[5].detail, "Ollama não encontrado");
        assert!(s.iter().filter(|x| x.id != ProviderId::ClaudeCode).all(|x| x.enabled));
    }

    #[test]
    fn claude_code_disabled_is_not_configured_even_if_logged_in() {
        let li = AuthStatus::LoggedIn("a@b".into());
        let s = dto(&[], Some(&li), false, &off());
        assert!(!s[1].configured && !s[1].enabled);
        assert_eq!(s[1].detail, "Desativado");
        let s = dto(&[], Some(&li), true, &off());
        assert!(s[1].configured && s[1].enabled);
        assert_eq!(s[1].detail, "a@b");
        let s = dto(&[], None, true, &off());
        assert_eq!(s[1].detail, "Claude Code não encontrado neste computador");
        let s = dto(&[], Some(&AuthStatus::LoggedOut), true, &off());
        assert_eq!(s[1].detail, "Claude Code não está logado");
    }

    #[test]
    fn ollama_running_without_models_is_not_configured() {
        let s = dto(&[], None, false, &OllamaDto { running: true, models: vec![] });
        assert!(!s[5].configured);
        assert_eq!(s[5].detail, "Ollama rodando, nenhum modelo baixado");
        let s = dto(&[], None, false, &OllamaDto { running: true, models: vec!["a".into(), "b".into()] });
        assert!(s[5].configured);
        assert_eq!(s[5].detail, "Ollama rodando · 2 modelo(s)");
    }

    #[test]
    fn codex_details() {
        let s = assemble_status(&AuthStatus::LoggedIn("x".into()), None, false, &HashSet::new(), &off());
        assert!(s[0].configured && s[0].detail == "x");
        let s = assemble_status(&AuthStatus::Missing, None, false, &HashSet::new(), &off());
        assert_eq!(s[0].detail, "Codex embutido não encontrado — reinstale o app");
    }
}

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use crate::error::AppError;
use crate::models;
use crate::state::AppState;
use crate::transcribe::{decode_to_pcm, join_text, Transcript};

/// Valida modelo e idioma e devolve o caminho do arquivo do modelo.
fn validate_request(
    models_dir: &Path,
    model: &str,
    language: &str,
) -> Result<PathBuf, AppError> {
    let info = models::find(model).ok_or_else(|| AppError::User("Modelo inválido.".into()))?;
    let lang_ok = language == "auto"
        || ((2..=3).contains(&language.len()) && language.bytes().all(|b| b.is_ascii_lowercase()));
    if !lang_ok {
        return Err(AppError::User("Idioma inválido.".into()));
    }
    let path = models_dir.join(models::file_name(model));
    if !path.is_file() {
        return Err(AppError::User(format!(
            "Modelo {} não instalado. Baixe em Configurações → Transcrição.",
            info.label
        )));
    }
    Ok(path)
}

/// Decodifica e transcreve (bloqueante). Chamar dentro de `spawn_blocking`.
fn run_transcription(
    state: &AppState,
    input: &Path,
    model: &str,
    language: &str,
    source_name: String,
    cancel: Arc<AtomicBool>,
    on_progress: Box<dyn FnMut(i32) + Send>,
) -> Result<Transcript, AppError> {
    let model_path = validate_request(&state.paths.models_dir, model, language)?;
    let pcm = decode_to_pcm(&state.sidecars.ffmpeg, input)?;
    let segments =
        state
            .engine
            .transcribe(&model_path, model, &pcm, language, cancel, on_progress)?;
    Ok(Transcript {
        text: join_text(&segments),
        segments,
        model: model.to_string(),
        language: language.to_string(),
        source_name,
    })
}

#[tauri::command]
pub async fn transcribe_file(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: String,
    model: String,
    language: String,
) -> Result<Transcript, AppError> {
    let state = Arc::clone(&state);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = state
            .transcription_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err(AppError::User(
                "Já existe uma transcrição em andamento.".into(),
            ));
        }
        *slot = Some(Arc::clone(&cancel));
    }

    let input = PathBuf::from(&path);
    let source_name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let worker_state = Arc::clone(&state);
    let result = tokio::task::spawn_blocking(move || {
        run_transcription(
            &worker_state,
            &input,
            &model,
            &language,
            source_name,
            cancel,
            Box::new(move |p| {
                let _ = app.emit("transcribe://progress", serde_json::json!({ "percent": p }));
            }),
        )
    })
    .await;

    *state
        .transcription_cancel
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;

    result.map_err(|e| AppError::Internal(format!("Falha na tarefa de transcrição: {e}")))?
}

#[tauri::command]
pub async fn transcribe_chunk(
    state: State<'_, Arc<AppState>>,
    data: Vec<u8>,
    mime: String,
    model: String,
    language: String,
) -> Result<Transcript, AppError> {
    let state = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let suffix = if mime.contains("mp4") { ".m4a" } else { ".webm" };
        let mut file = tempfile::Builder::new().suffix(suffix).tempfile()?;
        file.write_all(&data)?;
        file.flush()?;
        run_transcription(
            &state,
            file.path(),
            &model,
            &language,
            "Microfone".into(),
            Arc::new(AtomicBool::new(false)),
            Box::new(|_| {}),
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("Falha na tarefa de transcrição: {e}")))?
}

#[tauri::command]
pub fn cancel_transcription(state: State<'_, Arc<AppState>>) {
    let slot = state
        .transcription_cancel
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(flag) = slot.as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(r: Result<PathBuf, AppError>) -> String {
        match r {
            Err(AppError::User(m)) => m,
            other => panic!("esperava User, veio {other:?}"),
        }
    }

    #[test]
    fn unknown_model_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            user_msg(validate_request(dir.path(), "nope", "auto")),
            "Modelo inválido."
        );
    }

    #[test]
    fn bad_languages_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        for lang in ["", "e", "english", "PT", "p1", "../"] {
            assert_eq!(
                user_msg(validate_request(dir.path(), "large-v3-turbo", lang)),
                "Idioma inválido.",
                "{lang}"
            );
        }
    }

    #[test]
    fn missing_model_file_uses_label() {
        let dir = tempfile::tempdir().unwrap();
        let label = models::find("large-v3-turbo").unwrap().label;
        assert_eq!(
            user_msg(validate_request(dir.path(), "large-v3-turbo", "pt")),
            format!("Modelo {label} não instalado. Baixe em Configurações → Transcrição.")
        );
    }

    #[test]
    fn installed_model_returns_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(models::file_name("large-v3-turbo"));
        std::fs::write(&file, b"x").unwrap();
        for lang in ["auto", "pt", "yue"] {
            assert_eq!(
                validate_request(dir.path(), "large-v3-turbo", lang).unwrap(),
                file
            );
        }
    }
}

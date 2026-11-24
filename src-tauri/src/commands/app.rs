use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::error::AppError;
use crate::models::{self, ModelState};
use crate::providers::cli;
use crate::settings::{self, Settings};
use crate::state::AppState;

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const WHISPER_VERSION: &str = "whisper-rs 0.16.0";

#[derive(Serialize)]
pub struct StatusDto {
    pub ffmpeg_ok: bool,
    pub codex_ok: bool,
    pub models: Vec<ModelState>,
    pub version: String,
    pub data_dir: String,
    pub versions: HashMap<String, String>,
}

/// Primeira linha não vazia da saída de `program arg`, ou "?" em qualquer falha.
async fn tool_version(program: &Path, arg: &str) -> String {
    let args = [OsString::from(arg)];
    match cli::run("", program, &args, &[], None, VERSION_TIMEOUT).await {
        Ok(out) if out.code == 0 => out
            .stdout
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(String::from)
            .unwrap_or_else(|| "?".into()),
        _ => "?".into(),
    }
}

#[tauri::command]
pub async fn get_status(state: State<'_, Arc<AppState>>) -> Result<StatusDto, AppError> {
    let default_id = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .default_model
        .clone();
    let (ffmpeg, codex) = tokio::join!(
        tool_version(&state.sidecars.ffmpeg, "-version"),
        tool_version(&state.sidecars.codex, "--version"),
    );
    let versions = HashMap::from([
        ("whisper".to_string(), WHISPER_VERSION.to_string()),
        ("ffmpeg".to_string(), ffmpeg),
        ("codex".to_string(), codex),
    ]);
    Ok(StatusDto {
        ffmpeg_ok: state.sidecars.ffmpeg.is_file(),
        codex_ok: state.sidecars.codex.is_file(),
        models: models::list(&state.paths.models_dir, &default_id),
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: state.paths.data_dir.display().to_string(),
        versions,
    })
}

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> Settings {
    state.settings.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[tauri::command]
pub fn set_settings(
    state: State<'_, Arc<AppState>>,
    settings: Settings,
) -> Result<Settings, AppError> {
    let mut current = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    settings::save(&state.paths.settings_file, &settings)?;
    *current = settings.clone();
    Ok(settings)
}

#[tauri::command]
pub fn open_data_dir(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), AppError> {
    app.opener()
        .open_path(state.paths.data_dir.display().to_string(), None::<&str>)
        .map_err(|e| AppError::Internal(format!("Não foi possível abrir a pasta: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_tool_gives_question_mark() {
        assert_eq!(tool_version(Path::new("/nonexistent/tool"), "-v").await, "?");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn first_line_of_output() {
        assert_eq!(tool_version(Path::new("/bin/echo"), "oi").await, "oi");
    }
}

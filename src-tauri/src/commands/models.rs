use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;
use std::collections::HashMap;

use crate::error::AppError;
use crate::models::{self, ModelState};
use crate::settings::{self, Settings};
use crate::state::AppState;

/// Registra `key` no mapa e devolve o token; erro `User(busy)` se já existir.
pub fn claim(
    map: &Mutex<HashMap<String, CancellationToken>>,
    key: &str,
    busy: &str,
) -> Result<CancellationToken, AppError> {
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    if map.contains_key(key) {
        return Err(AppError::User(busy.into()));
    }
    let token = CancellationToken::new();
    map.insert(key.to_string(), token.clone());
    Ok(token)
}

pub fn release(map: &Mutex<HashMap<String, CancellationToken>>, key: &str) {
    map.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
}

fn default_id(state: &AppState) -> String {
    state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .default_model
        .clone()
}

#[tauri::command]
pub fn list_models(state: State<'_, Arc<AppState>>) -> Vec<ModelState> {
    models::list(&state.paths.models_dir, &default_id(&state))
}

#[tauri::command]
pub async fn download_model(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), AppError> {
    if models::find(&id).is_none() {
        return Err(AppError::User(format!("Modelo desconhecido: {id}")));
    }
    let token = claim(&state.downloads, &id, "Este modelo já está sendo baixado.")?;
    let state = Arc::clone(&state);
    tokio::spawn(async move {
        let progress_app = app.clone();
        let progress_id = id.clone();
        let result = models::download(
            &state.http,
            models::HF_BASE,
            &state.paths.models_dir,
            &id,
            token,
            move |done, total| {
                let _ = progress_app.emit(
                    "model://progress",
                    json!({ "id": progress_id, "done": done, "total": total }),
                );
            },
        )
        .await;
        release(&state.downloads, &id);
        let (ok, error) = match result {
            Ok(_) => (true, None),
            Err(e) => (false, Some(e)),
        };
        let _ = app.emit("model://done", json!({ "id": id, "ok": ok, "error": error }));
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_download(state: State<'_, Arc<AppState>>, id: String) {
    if let Some(t) = state
        .downloads
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
    {
        t.cancel();
    }
}

#[tauri::command]
pub fn delete_model(state: State<'_, Arc<AppState>>, id: String) -> Result<(), AppError> {
    if state
        .downloads
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&id)
    {
        return Err(AppError::User(
            "Aguarde o download terminar ou cancele antes de remover.".into(),
        ));
    }
    state.engine.evict();
    models::delete(&state.paths.models_dir, &id)
}

#[tauri::command]
pub fn set_default_model(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<Settings, AppError> {
    let installed = models::find(&id).is_some()
        && state.paths.models_dir.join(models::file_name(&id)).is_file();
    if !installed {
        return Err(AppError::User("Modelo não instalado.".into()));
    }
    let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    let mut updated = settings.clone();
    updated.default_model = id;
    settings::save(&state.paths.settings_file, &updated)?;
    *settings = updated.clone();
    Ok(updated)
}

#[tauri::command]
pub async fn import_model(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<String, AppError> {
    let state = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let id = models::import_file(&PathBuf::from(path), &state.paths.models_dir)?;
        state.engine.evict();
        Ok(id)
    })
    .await
    .map_err(|e| AppError::Internal(format!("Falha ao importar o modelo: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_rejects_duplicates_and_release_frees() {
        let map = Mutex::new(HashMap::new());
        claim(&map, "a", "ocupado").unwrap();
        match claim(&map, "a", "ocupado") {
            Err(AppError::User(m)) => assert_eq!(m, "ocupado"),
            other => panic!("{other:?}"),
        }
        claim(&map, "b", "ocupado").unwrap();
        release(&map, "a");
        claim(&map, "a", "ocupado").unwrap();
    }
}

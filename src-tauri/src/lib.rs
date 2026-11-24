mod commands;
mod credentials;
pub mod error;
mod models;
mod navigation;
mod paths;
pub mod providers;
mod record;
pub mod summarize;
mod settings;
mod state;
pub mod transcribe;

use std::sync::Arc;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(navigation::guard())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let paths = paths::AppPaths::from_data_dir(data_dir)?;
            app.manage(Arc::new(state::AppState::new(paths)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::transcription::transcribe_file,
            commands::transcription::transcribe_chunk,
            commands::transcription::cancel_transcription,
            commands::app::get_status,
            commands::app::get_settings,
            commands::app::set_settings,
            commands::app::open_data_dir,
            commands::app::save_text_file,
            commands::models::list_models,
            commands::models::download_model,
            commands::models::cancel_download,
            commands::models::delete_model,
            commands::models::set_default_model,
            commands::models::import_model,
            commands::providers::providers_status,
            commands::providers::save_api_key,
            commands::providers::delete_api_key,
            commands::providers::codex_login,
            commands::providers::codex_logout,
            commands::providers::ollama_models,
            commands::providers::ollama_pull,
            commands::providers::cancel_ollama_pull,
            commands::providers::ollama_delete,
            commands::summary::summarize,
            commands::summary::compare,
        ])
        .run(tauri::generate_context!())
        .expect("erro ao executar o Local Transcriber");
}

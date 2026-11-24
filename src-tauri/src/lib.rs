mod commands;
#[allow(dead_code)]
mod credentials;
pub mod error;
#[allow(dead_code)]
mod models;
#[allow(dead_code)]
mod paths;
#[allow(dead_code)]
pub mod providers;
#[allow(dead_code)]
mod settings;
#[allow(dead_code)]
mod state;
pub mod transcribe;

use std::sync::Arc;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
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
        ])
        .run(tauri::generate_context!())
        .expect("erro ao executar o Local Transcriber");
}

pub mod error;
#[allow(dead_code)]
mod models;
#[allow(dead_code)]
mod paths;
#[allow(dead_code)]
mod settings;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .run(tauri::generate_context!())
        .expect("erro ao executar o Local Transcriber");
}

//! Testes com FFmpeg e modelo whisper reais. Ignorados por padrão.
//!
//! Rode `scripts/fetch-test-fixtures.sh` e depois, dentro de `src-tauri/`:
//! `LT_FFMPEG=$(which ffmpeg) LT_MODEL=/caminho/ggml-<id>.bin cargo test --test transcribe_real -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use local_transcriber_lib::error::AppError;
use local_transcriber_lib::transcribe::{decode_to_pcm, join_text, Engine};

fn env_path(var: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => {
            eprintln!("pulando: defina {var} para rodar este teste");
            None
        }
    }
}

fn jfk() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jfk.wav");
    assert!(
        p.exists(),
        "{} não existe; rode scripts/fetch-test-fixtures.sh",
        p.display()
    );
    p
}

fn no_progress() -> Box<dyn FnMut(i32) + Send> {
    Box::new(|_| {})
}

#[test]
#[ignore]
fn transcribes_jfk() {
    let (Some(ffmpeg), Some(model)) = (env_path("LT_FFMPEG"), env_path("LT_MODEL")) else {
        return;
    };
    let pcm = decode_to_pcm(&ffmpeg, &jfk(), &AtomicBool::new(false)).unwrap();
    let progress = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = progress.clone();
    let segments = Engine::new()
        .transcribe(
            &model,
            "test",
            &pcm,
            "auto",
            Arc::new(AtomicBool::new(false)),
            Box::new(move |p| seen.lock().unwrap().push(p)),
        )
        .unwrap();
    let text = join_text(&segments);
    println!("texto: {text}");
    println!("segmentos: {segments:?}");
    println!("progresso: {:?}", progress.lock().unwrap());
    assert!(
        text.to_lowercase()
            .contains("ask not what your country can do for you"),
        "texto inesperado: {text}"
    );
}

#[test]
#[ignore]
fn decodes_file_with_accented_name() {
    let Some(ffmpeg) = env_path("LT_FFMPEG") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("Reunião ação 1.wav");
    std::fs::copy(jfk(), &copy).unwrap();
    let pcm = decode_to_pcm(&ffmpeg, &copy, &AtomicBool::new(false)).unwrap();
    assert!(pcm.len() > 16000, "pcm curto demais: {}", pcm.len());
}

#[test]
#[ignore]
fn streaming_decode_matches_full_output() {
    let Some(ffmpeg) = env_path("LT_FFMPEG") else {
        return;
    };
    let out = std::process::Command::new(&ffmpeg)
        .args(local_transcriber_lib::transcribe::ffmpeg_args(&jfk()))
        .output()
        .unwrap();
    assert!(out.status.success());
    let want: Vec<f32> = out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let got = decode_to_pcm(&ffmpeg, &jfk(), &AtomicBool::new(false)).unwrap();
    assert!(got.len() > 16000);
    assert_eq!(got, want);
}

#[test]
#[ignore]
fn silence_gives_no_segments() {
    let Some(model) = env_path("LT_MODEL") else {
        return;
    };
    let pcm = vec![0.0f32; 16000 * 5];
    let segments = Engine::new()
        .transcribe(
            &model,
            "test",
            &pcm,
            "auto",
            Arc::new(AtomicBool::new(false)),
            no_progress(),
        )
        .unwrap();
    assert_eq!(join_text(&segments), "", "segmentos: {segments:?}");
}

#[test]
#[ignore]
fn cancel_aborts() {
    let (Some(ffmpeg), Some(model)) = (env_path("LT_FFMPEG"), env_path("LT_MODEL")) else {
        return;
    };
    let pcm = decode_to_pcm(&ffmpeg, &jfk(), &AtomicBool::new(false)).unwrap();
    let result = Engine::new().transcribe(
        &model,
        "test",
        &pcm,
        "auto",
        Arc::new(AtomicBool::new(true)),
        no_progress(),
    );
    assert!(
        matches!(result, Err(AppError::Cancelled)),
        "esperado Cancelled, veio {result:?}"
    );
}

#[test]
#[ignore]
fn cancel_during_run_aborts() {
    let (Some(ffmpeg), Some(model)) = (env_path("LT_FFMPEG"), env_path("LT_MODEL")) else {
        return;
    };
    let pcm = decode_to_pcm(&ffmpeg, &jfk(), &AtomicBool::new(false)).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    // Cancela no primeiro aviso de progresso, já com o whisper rodando.
    let result = Engine::new().transcribe(
        &model,
        "test",
        &pcm,
        "auto",
        cancel,
        Box::new(move |_| flag.store(true, std::sync::atomic::Ordering::Relaxed)),
    );
    assert!(
        matches!(result, Err(AppError::Cancelled)),
        "esperado Cancelled, veio {result:?}"
    );
}

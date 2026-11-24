// Gravação nativa: lista microfones, começa e para a sessão de `record::session`.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use crate::commands::transcription::{claim_slot, validate_request, SlotGuard};
use crate::error::AppError;
use crate::record::devices::{self, AudioInput, OpenSource, SourceKind, SourceMsg};
use crate::record::session::{
    Env, Opened, Reopen, Session, SessionEvent, Sink, SourceSpec, Transcriber,
};
use crate::settings::{self, Settings};
use crate::state::AppState;
use crate::transcribe::{Engine, Transcript};

/// Mensagens entre a thread de conversão de uma fonte e a sessão. Cada uma cobre
/// de 10 a 300 ms de áudio e a sessão drena a cada 100 ms: sobra folga.
const SOURCE_QUEUE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Mic,
    System,
    Both,
}

impl Mode {
    fn parse(source: &str) -> Result<Mode, AppError> {
        match source {
            "mic" => Ok(Mode::Mic),
            "system" => Ok(Mode::System),
            "both" => Ok(Mode::Both),
            _ => Err(AppError::User("Fonte de gravação inválida.".into())),
        }
    }

    /// Fontes abertas e o falante de cada uma (só no modo duplo).
    fn sources(self) -> &'static [(SourceKind, Option<&'static str>)] {
        match self {
            Mode::Mic => &[(SourceKind::Mic, None)],
            Mode::System => &[(SourceKind::System, None)],
            Mode::Both => &[
                (SourceKind::Mic, Some("voce")),
                (SourceKind::System, Some("reuniao")),
            ],
        }
    }

    fn source_name(self, mic_name: &str) -> String {
        match self {
            Mode::Mic => format!("Microfone — {mic_name}"),
            Mode::System => "Som do computador".into(),
            Mode::Both => "Microfone + som do computador".into(),
        }
    }
}

/// Sessão em andamento. Guarda o espaço de transcrição até `stop_recording`,
/// mesmo que a sessão tenha parado sozinha por erro.
pub struct RecordingHandle {
    session: Session,
    model: String,
    language: String,
    source_name: String,
    slot: SlotGuard,
}

impl RecordingHandle {
    /// Bloqueante: espera a fila esvaziar.
    fn finish(self) -> Result<Transcript, AppError> {
        let RecordingHandle {
            session,
            model,
            language,
            source_name,
            slot,
        } = self;
        let recorded = session.stop()?;
        if slot.cancel().load(Ordering::Relaxed) {
            return Err(AppError::Cancelled);
        }
        Ok(Transcript {
            text: recorded.text,
            segments: recorded.segments,
            model,
            language,
            source_name,
        })
    }
}

fn open_source(
    kind: SourceKind,
    mic_id: Option<&str>,
) -> Result<(OpenSource, Receiver<SourceMsg>), AppError> {
    let (tx, rx) = mpsc::sync_channel(SOURCE_QUEUE);
    Ok((devices::open(kind, mic_id, tx)?, rx))
}

/// Abre as fontes e começa a sessão (bloqueante). Devolve a sessão e o `sourceName`.
#[allow(clippy::too_many_arguments)]
fn begin(
    engine: Arc<Engine>,
    model_path: PathBuf,
    model: String,
    language: String,
    mode: Mode,
    mic_id: Option<String>,
    cancel: Arc<AtomicBool>,
    sink: Sink,
) -> Result<(Session, String), AppError> {
    // Microfone salvo que não existe mais: usa o padrão. Reabrir volta ao mesmo
    // dispositivo; se ele sumiu, a sessão desiste com "O microfone foi desconectado.".
    let mic_id = if mode == Mode::System {
        None
    } else {
        let inputs = devices::list_inputs()?;
        let chosen = devices::resolve_mic(&inputs, mic_id.as_deref())
            .ok_or_else(|| AppError::User("Nenhum microfone encontrado.".into()))?;
        Some(chosen.id.clone())
    };
    let mut specs = Vec::new();
    let mut mic_name = String::new();
    for &(kind, speaker) in mode.sources() {
        // Se a segunda fonte falhar, a primeira é solta junto com `specs`.
        let (source, rx) = open_source(kind, mic_id.as_deref())?;
        if kind == SourceKind::Mic {
            mic_name = source.device_name().to_string();
        }
        let reopen_id = mic_id.clone();
        let reopen: Reopen = Box::new(move || {
            let (source, rx) = open_source(kind, reopen_id.as_deref())?;
            Ok(Opened { capture: Box::new(source), rx })
        });
        specs.push(SourceSpec {
            kind,
            speaker,
            opened: Opened { capture: Box::new(source), rx },
            reopen,
        });
    }
    let transcriber: Transcriber = Arc::new(move |pcm: &[f32]| {
        engine.transcribe(
            &model_path,
            &model,
            pcm,
            &language,
            Arc::clone(&cancel),
            Box::new(|_| {}),
        )
    });
    let session = Session::new(specs, transcriber, sink, Env::system())?;
    Ok((session, mode.source_name(&mic_name)))
}

/// Nome e corpo do evento da interface.
fn event_payload(event: SessionEvent) -> (&'static str, Value) {
    match event {
        SessionEvent::Text { speaker, text } => {
            ("record://text", json!({ "speaker": speaker, "text": text }))
        }
        SessionEvent::Level { mic, system } => {
            ("record://level", json!({ "mic": mic, "system": system }))
        }
        SessionEvent::Error { message } => ("record://error", json!({ "message": message })),
    }
}

/// Guarda a fonte escolhida; o microfone só muda quando ele é usado.
fn remember_choice(current: &Settings, source: &str, mic_id: Option<&str>) -> Settings {
    let mut next = current.clone();
    next.record_source = source.to_string();
    if source != "system" {
        next.record_mic_id = mic_id.map(str::to_string);
    }
    next
}

fn save_choice(state: &AppState, source: &str, mic_id: Option<&str>) {
    let mut current = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    let next = remember_choice(&current, source, mic_id);
    // Não salvar a preferência não impede de gravar.
    match settings::save(&state.paths.settings_file, &next) {
        Ok(()) => *current = next,
        Err(e) => eprintln!("gravação: não foi possível salvar a fonte escolhida: {e}"),
    }
}

#[tauri::command]
pub async fn list_audio_inputs() -> Result<Vec<AudioInput>, AppError> {
    tokio::task::spawn_blocking(devices::list_inputs)
        .await
        .map_err(|e| AppError::Internal(format!("Falha ao listar os microfones: {e}")))?
}

#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    source: String,
    mic_id: Option<String>,
    model: String,
    language: String,
) -> Result<(), AppError> {
    let state = Arc::clone(&state);
    let mode = Mode::parse(&source)?;
    let model_path = validate_request(&state.paths.models_dir, &model, &language)?;
    // Erro daqui em diante solta `slot` e libera o espaço.
    let slot = claim_slot(&state.transcription_cancel)?;
    save_choice(&state, &source, mic_id.as_deref());

    let engine = Arc::clone(&state.engine);
    let cancel = slot.cancel();
    let sink: Sink = Arc::new(move |event| {
        let (name, payload) = event_payload(event);
        let _ = app.emit(name, payload);
    });
    let (m, l) = (model.clone(), language.clone());
    let (session, source_name) = tokio::task::spawn_blocking(move || {
        begin(engine, model_path, m, l, mode, mic_id, cancel, sink)
    })
    .await
    .map_err(|e| AppError::Internal(format!("Falha ao começar a gravação: {e}")))??;

    *state.recording.lock().unwrap_or_else(|e| e.into_inner()) = Some(RecordingHandle {
        session,
        model,
        language,
        source_name,
        slot,
    });
    Ok(())
}

#[tauri::command]
pub async fn stop_recording(state: State<'_, Arc<AppState>>) -> Result<Transcript, AppError> {
    let handle = state
        .recording
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or_else(|| AppError::User("Nenhuma gravação em andamento.".into()))?;
    // Esperar a fila esvaziar bloqueia: fora da thread do runtime. Se a tarefa
    // entrar em pânico, o `SlotGuard` dentro do handle libera o espaço mesmo assim.
    tokio::task::spawn_blocking(move || handle.finish())
        .await
        .map_err(|e| AppError::Internal(format!("Falha ao terminar a gravação: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::transcription::CancelSlot;

    #[test]
    fn recording_slot_blocks_file_transcription() {
        let slot = CancelSlot::default();
        // `start_recording` ocupa o espaço e o guarda no `RecordingHandle`.
        let recording = claim_slot(&slot).unwrap();
        // `transcribe_file` passa pela mesma função.
        match claim_slot(&slot) {
            Err(AppError::User(m)) => assert_eq!(m, "Já existe uma transcrição em andamento."),
            other => panic!("esperava erro de exclusividade, veio {:?}", other.map(|_| ())),
        }
        drop(recording);
        assert!(slot.lock().unwrap().is_none());
        assert!(claim_slot(&slot).is_ok());
    }

    #[test]
    fn modes_parse_and_name_the_source() {
        assert!(matches!(Mode::parse("xyz"), Err(AppError::User(m)) if m == "Fonte de gravação inválida."));
        assert_eq!(Mode::parse("mic").unwrap().source_name("MacBook Pro Microfone"), "Microfone — MacBook Pro Microfone");
        assert_eq!(Mode::parse("system").unwrap().source_name(""), "Som do computador");
        assert_eq!(Mode::parse("both").unwrap().source_name("X"), "Microfone + som do computador");
        assert_eq!(
            Mode::Both.sources(),
            [(SourceKind::Mic, Some("voce")), (SourceKind::System, Some("reuniao"))]
        );
        assert_eq!(Mode::System.sources(), [(SourceKind::System, None)]);
    }

    #[test]
    fn events_follow_the_ui_contract() {
        assert_eq!(
            event_payload(SessionEvent::Text { speaker: Some("voce"), text: "oi".into() }),
            ("record://text", json!({ "speaker": "voce", "text": "oi" }))
        );
        assert_eq!(
            event_payload(SessionEvent::Text { speaker: None, text: "oi".into() }),
            ("record://text", json!({ "speaker": null, "text": "oi" }))
        );
        assert_eq!(
            event_payload(SessionEvent::Level { mic: Some(0.5), system: None }),
            ("record://level", json!({ "mic": 0.5, "system": null }))
        );
        assert_eq!(
            event_payload(SessionEvent::Error { message: "x".into() }),
            ("record://error", json!({ "message": "x" }))
        );
    }

    #[test]
    fn choice_keeps_mic_when_recording_only_the_computer() {
        let current = Settings { record_mic_id: Some("m1".into()), ..Settings::default() };
        let s = remember_choice(&current, "system", None);
        assert_eq!((s.record_source.as_str(), s.record_mic_id.as_deref()), ("system", Some("m1")));
        let s = remember_choice(&current, "both", Some("m2"));
        assert_eq!((s.record_source.as_str(), s.record_mic_id.as_deref()), ("both", Some("m2")));
        let s = remember_choice(&current, "mic", None);
        assert_eq!(s.record_mic_id, None);
    }

    fn smoke_models_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("LT_MODELS_DIR") {
            return dir.into();
        }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let app_dir = home
            .join("Library/Application Support/io.github.lucas1dev.localtranscriber/models");
        if app_dir.join("ggml-large-v3-turbo.bin").is_file() {
            return app_dir;
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../models")
    }

    /// Grava 15 s do microfone padrão com o modelo instalado e imprime o resultado.
    /// Precisa de microfone real e do modelo large-v3-turbo.
    #[test]
    #[ignore]
    fn real_recording_smoke() {
        use std::sync::atomic::AtomicU32;
        use std::time::{Duration, Instant};

        let dir = smoke_models_dir();
        let model = "large-v3-turbo";
        let model_path = validate_request(&dir, model, "auto").expect("modelo instalado");
        println!("modelo: {}", model_path.display());

        let levels = Arc::new(AtomicU32::new(0));
        let peak = Arc::new(AtomicU32::new(0));
        let sink: Sink = {
            let (levels, peak) = (levels.clone(), peak.clone());
            Arc::new(move |event| match event {
                SessionEvent::Level { mic, .. } => {
                    levels.fetch_add(1, Ordering::Relaxed);
                    peak.fetch_max(mic.unwrap_or(0.0).to_bits(), Ordering::Relaxed);
                }
                other => println!("evento: {other:?}"),
            })
        };
        let (session, source_name) = begin(
            Arc::new(Engine::new()),
            model_path,
            model.into(),
            "auto".into(),
            Mode::Mic,
            None,
            Arc::new(AtomicBool::new(false)),
            sink,
        )
        .expect("abrir o microfone");
        println!("fonte: {source_name}");
        std::thread::sleep(Duration::from_secs(15));
        let t = Instant::now();
        let recorded = session.stop().expect("resultado");
        println!("parou em {} ms", t.elapsed().as_millis());
        println!(
            "eventos de nível: {}, maior nível: {:.3}",
            levels.load(Ordering::Relaxed),
            f32::from_bits(peak.load(Ordering::Relaxed))
        );
        for s in &recorded.segments {
            println!("[{} → {}] {:?} {}", s.start, s.end, s.speaker, s.text);
        }
        println!("texto: {:?}", recorded.text);
    }
}

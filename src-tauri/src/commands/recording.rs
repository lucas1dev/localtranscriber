// Gravação nativa: lista microfones, começa e para a sessão de `record::session`.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::transcription::{claim_slot, validate_request, SlotGuard};
use crate::error::AppError;
use crate::record::devices::{self, AudioInput, OpenSource, SourceKind, SourceMsg, OUTPUT_FALLBACK};
use crate::record::permissions;
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

    /// `output_name` é a saída escolhida (só conta no modo Som do computador).
    fn source_name(self, mic_name: &str, output_name: Option<&str>) -> String {
        match self {
            Mode::Mic => format!("Microfone — {mic_name}"),
            Mode::System => match output_name {
                Some(name) => format!("Som do computador — {name}"),
                None => "Som do computador".into(),
            },
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

/// `device_id`: o microfone para `Mic`, a saída escolhida para `System`.
fn open_source(
    kind: SourceKind,
    device_id: Option<&str>,
) -> Result<(OpenSource, Receiver<SourceMsg>), AppError> {
    let (tx, rx) = mpsc::sync_channel(SOURCE_QUEUE);
    Ok((devices::open(kind, device_id, tx)?, rx))
}

/// Estado do som do computador entre reaberturas: a saída escolhida ainda em uso
/// e se o aviso de queda ainda deve sair.
struct SystemOutput {
    chosen: Option<String>,
    notice_pending: bool,
}

impl SystemOutput {
    fn new(chosen: Option<String>) -> Self {
        Self { chosen, notice_pending: false }
    }

    /// Abre o som do computador com `open(id)`. Se a saída escolhida sumiu da
    /// listagem `present` (`None` = a listagem falhou), cai para a primeira opção
    /// (`id` = `None`) e fica nela até o fim. O aviso fica pendente até uma abertura
    /// dar certo: uma falha de `open` não o perde, e ele sai uma vez só.
    fn open<T>(
        &mut self,
        present: impl FnOnce() -> Option<Vec<String>>,
        open: impl FnOnce(Option<&str>) -> Result<T, AppError>,
    ) -> Result<(T, Option<String>), AppError> {
        if let Some(id) = &self.chosen {
            if present().is_some_and(|l| !l.contains(id)) {
                self.chosen = None;
                self.notice_pending = true;
            }
        }
        let opened = open(self.chosen.as_deref())?;
        let notice = std::mem::take(&mut self.notice_pending).then(|| OUTPUT_FALLBACK.to_string());
        Ok((opened, notice))
    }
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
    output_id: Option<String>,
    cancel: Arc<AtomicBool>,
    sink: Sink,
) -> Result<(Session, String), AppError> {
    // Microfone salvo que não existe mais: usa o padrão. Reabrir volta ao mesmo
    // dispositivo; se ele sumiu, a sessão desiste com "O microfone foi desconectado.".
    let mic_id = if mode == Mode::System {
        None
    } else {
        // No macOS um microfone negado só manda silêncio: confere antes de abrir.
        permissions::ensure_mic()?;
        let inputs = devices::list_inputs()?;
        let chosen = devices::resolve_mic(&inputs, mic_id.as_deref())
            .ok_or_else(|| AppError::User("Nenhum microfone encontrado.".into()))?;
        Some(chosen.id.clone())
    };
    // Saída salva que não existe mais: a primeira opção, sem aviso. Só a escolhida
    // que some no meio da gravação avisa (ver `next_output`).
    let output = if mode == Mode::Mic {
        None
    } else {
        let outputs = devices::list_outputs()?;
        devices::resolve_output(&outputs, output_id.as_deref()).cloned()
    };
    // Zero do relógio comum antes de abrir a primeira fonte: o que ela grava
    // enquanto a outra abre conta do zero, e a outra é completada com silêncio.
    let env = Env::system();
    let start = (env.now)();
    let mut specs = Vec::new();
    let mut mic_name = String::new();
    for &(kind, speaker) in mode.sources() {
        let device_id = match kind {
            SourceKind::Mic => mic_id.clone(),
            SourceKind::System => output.as_ref().map(|o| o.id.clone()),
        };
        // Se a segunda fonte falhar, a primeira é solta junto com `specs`.
        let (source, rx) = open_source(kind, device_id.as_deref())?;
        if kind == SourceKind::Mic {
            mic_name = source.device_name().to_string();
        }
        let reopen: Reopen = match kind {
            SourceKind::Mic => Box::new(move || {
                let (source, rx) = open_source(kind, device_id.as_deref())?;
                Ok(Opened { capture: Box::new(source), rx, notice: None })
            }),
            SourceKind::System => {
                let mut output = SystemOutput::new(device_id);
                Box::new(move || {
                    let ((source, rx), notice) = output.open(devices::output_ids, |id| {
                        open_source(kind, id)
                    })?;
                    Ok(Opened { capture: Box::new(source), rx, notice })
                })
            }
        };
        specs.push(SourceSpec {
            kind,
            speaker,
            opened: Opened { capture: Box::new(source), rx, notice: None },
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
    let session = Session::new(specs, transcriber, sink, env, start)?;
    let output_name = output.as_ref().map(|o| o.name.as_str());
    Ok((session, mode.source_name(&mic_name, output_name)))
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
        SessionEvent::Notice { message } => ("record://notice", json!({ "message": message })),
        SessionEvent::Error { message } => ("record://error", json!({ "message": message })),
    }
}

/// Guarda a fonte escolhida; o microfone e a saída só mudam quando são usados.
fn remember_choice(
    current: &Settings,
    source: &str,
    mic_id: Option<&str>,
    output_id: Option<&str>,
) -> Settings {
    let mut next = current.clone();
    next.record_source = source.to_string();
    if source != "system" {
        next.record_mic_id = mic_id.map(str::to_string);
    }
    if source != "mic" {
        next.record_output_id = output_id.map(str::to_string);
    }
    next
}

/// Salva a fonte escolhida; a memória só muda depois que o arquivo foi gravado.
fn save_choice(
    settings: &Mutex<Settings>,
    file: &Path,
    source: &str,
    mic_id: Option<&str>,
    output_id: Option<&str>,
) -> Result<(), AppError> {
    let mut current = settings.lock().unwrap_or_else(|e| e.into_inner());
    let next = remember_choice(&current, source, mic_id, output_id);
    settings::save(file, &next)?;
    *current = next;
    Ok(())
}

/// Evento para a interface confirmar o fechamento com uma gravação em andamento.
pub const CLOSE_REQUESTED: &str = "app://close-requested";

/// Segura o fechamento da janela enquanto há gravação, até a pessoa confirmar.
pub fn block_close(recording: bool, confirmed: bool) -> bool {
    recording && !confirmed
}

/// `CloseRequested` da janela: com gravação em andamento e sem confirmação,
/// impede o fechamento e pede a confirmação à interface.
pub fn on_close_requested(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    let state = window.state::<Arc<AppState>>();
    let recording = state.recording.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    if block_close(recording, state.close_confirmed.load(Ordering::SeqCst)) {
        api.prevent_close();
        let _ = window.emit(CLOSE_REQUESTED, ());
    }
}

/// A pessoa confirmou: fecha mesmo com a gravação (que é descartada).
#[tauri::command]
pub async fn confirm_close(window: tauri::Window, state: State<'_, Arc<AppState>>) -> Result<(), AppError> {
    state.close_confirmed.store(true, Ordering::SeqCst);
    window
        .close()
        .map_err(|e| AppError::Internal(format!("Não foi possível fechar a janela: {e}")))
}

#[tauri::command]
pub async fn list_audio_inputs() -> Result<Vec<AudioInput>, AppError> {
    tokio::task::spawn_blocking(devices::list_inputs)
        .await
        .map_err(|e| AppError::Internal(format!("Falha ao listar os microfones: {e}")))?
}

#[tauri::command]
pub async fn list_audio_outputs() -> Result<Vec<AudioInput>, AppError> {
    tokio::task::spawn_blocking(devices::list_outputs)
        .await
        .map_err(|e| AppError::Internal(format!("Falha ao listar as saídas de som: {e}")))?
}

#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    source: String,
    mic_id: Option<String>,
    output_id: Option<String>,
    model: String,
    language: String,
) -> Result<(), AppError> {
    let state = Arc::clone(&state);
    let mode = Mode::parse(&source)?;
    let model_path = validate_request(&state.paths.models_dir, &model, &language)?;
    // Erro daqui em diante solta `slot` e libera o espaço.
    let slot = claim_slot(&state.transcription_cancel)?;
    save_choice(
        &state.settings,
        &state.paths.settings_file,
        &source,
        mic_id.as_deref(),
        output_id.as_deref(),
    )?;

    let engine = Arc::clone(&state.engine);
    let cancel = slot.cancel();
    let sink: Sink = Arc::new(move |event| {
        let (name, payload) = event_payload(event);
        let _ = app.emit(name, payload);
    });
    let (m, l) = (model.clone(), language.clone());
    let (session, source_name) = tokio::task::spawn_blocking(move || {
        begin(engine, model_path, m, l, mode, mic_id, output_id, cancel, sink)
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
    fn close_is_blocked_only_while_recording_without_confirmation() {
        assert!(block_close(true, false));
        assert!(!block_close(true, true));
        assert!(!block_close(false, false));
        assert!(!block_close(false, true));
        assert_eq!(CLOSE_REQUESTED, "app://close-requested");
    }

    #[test]
    fn modes_parse_and_name_the_source() {
        assert!(matches!(Mode::parse("xyz"), Err(AppError::User(m)) if m == "Fonte de gravação inválida."));
        assert_eq!(Mode::parse("mic").unwrap().source_name("MacBook Pro Microfone", None), "Microfone — MacBook Pro Microfone");
        assert_eq!(Mode::parse("system").unwrap().source_name("", None), "Som do computador");
        assert_eq!(Mode::System.source_name("", Some("Fones")), "Som do computador — Fones");
        assert_eq!(Mode::parse("both").unwrap().source_name("X", Some("Fones")), "Microfone + som do computador");
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
            event_payload(SessionEvent::Notice { message: "aviso".into() }),
            ("record://notice", json!({ "message": "aviso" }))
        );
        assert_eq!(
            event_payload(SessionEvent::Error { message: "x".into() }),
            ("record://error", json!({ "message": "x" }))
        );
    }

    #[test]
    fn choice_keeps_mic_when_recording_only_the_computer() {
        let current = Settings { record_mic_id: Some("m1".into()), ..Settings::default() };
        let s = remember_choice(&current, "system", None, None);
        assert_eq!((s.record_source.as_str(), s.record_mic_id.as_deref()), ("system", Some("m1")));
        let s = remember_choice(&current, "both", Some("m2"), None);
        assert_eq!((s.record_source.as_str(), s.record_mic_id.as_deref()), ("both", Some("m2")));
        let s = remember_choice(&current, "mic", None, None);
        assert_eq!(s.record_mic_id, None);
    }

    #[test]
    fn choice_stores_the_output_only_when_the_computer_is_recorded() {
        let current = Settings { record_output_id: Some("o1".into()), ..Settings::default() };
        let s = remember_choice(&current, "system", None, Some("o2"));
        assert_eq!(s.record_output_id.as_deref(), Some("o2"));
        let s = remember_choice(&current, "both", Some("m"), Some("o3"));
        assert_eq!(s.record_output_id.as_deref(), Some("o3"));
        // Primeira opção escolhida: apaga a saída salva.
        let s = remember_choice(&current, "system", None, None);
        assert_eq!(s.record_output_id, None);
        // Microfone puro não mexe na saída salva.
        let s = remember_choice(&current, "mic", Some("m"), Some("ignorada"));
        assert_eq!(s.record_output_id.as_deref(), Some("o1"));
    }

    fn ids(l: &[&str]) -> Option<Vec<String>> {
        Some(l.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn system_output_falls_back_once_and_stays() {
        let mut out = SystemOutput::new(Some("a".into()));
        let open = |id: Option<&str>| Ok::<_, AppError>(id.map(String::from));
        // Presente, ou listagem que falhou: continua presa, sem aviso.
        assert_eq!(out.open(|| ids(&["a", "b"]), open).unwrap(), (Some("a".into()), None));
        assert_eq!(out.open(|| None, open).unwrap(), (Some("a".into()), None));
        // Sumiu: primeira opção, com um aviso.
        assert_eq!(
            out.open(|| ids(&["b"]), open).unwrap(),
            (None, Some(OUTPUT_FALLBACK.to_string()))
        );
        // Mesmo que ela volte, fica na primeira opção, sem novo aviso.
        assert_eq!(out.open(|| ids(&["a", "b"]), open).unwrap(), (None, None));
    }

    #[test]
    fn fallback_notice_survives_a_failed_first_option_open() {
        let mut out = SystemOutput::new(Some("a".into()));
        let fail = |_: Option<&str>| Err::<Option<String>, _>(AppError::User("falhou".into()));
        let ok = |id: Option<&str>| Ok::<_, AppError>(id.map(String::from));
        // A saída sumiu e a primeira opção também falha ao abrir: sem aviso ainda.
        assert!(out.open(|| ids(&["b"]), fail).is_err());
        // Nas tentativas seguintes a escolhida nem é consultada de novo.
        assert!(out.open(|| panic!("não consulta a listagem"), fail).is_err());
        // A primeira abertura que dá certo leva o aviso, uma vez só.
        assert_eq!(out.open(|| None, ok).unwrap(), (None, Some(OUTPUT_FALLBACK.to_string())));
        assert_eq!(out.open(|| None, ok).unwrap(), (None, None));
    }

    #[test]
    fn save_choice_updates_memory_only_after_saving() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Mutex::new(Settings::default());
        let file = dir.path().join("settings.json");
        save_choice(&settings, &file, "both", Some("m1"), Some("o1")).unwrap();
        assert_eq!(settings.lock().unwrap().record_source, "both");
        assert_eq!(crate::settings::load(&file).record_mic_id.as_deref(), Some("m1"));
        assert_eq!(crate::settings::load(&file).record_output_id.as_deref(), Some("o1"));

        // Um diretório no lugar do arquivo: salvar falha, o erro sobe e a memória fica igual.
        let blocked = dir.path().join("bloqueado");
        std::fs::create_dir(&blocked).unwrap();
        assert!(save_choice(&settings, &blocked, "system", None, None).is_err());
        assert_eq!(settings.lock().unwrap().record_source, "both");
    }

    /// Handle com uma sessão sem fontes, ocupando `slot`.
    fn handle(slot: &CancelSlot) -> RecordingHandle {
        let session = Session::new(
            Vec::new(),
            Arc::new(|_: &[f32]| Ok(Vec::new())),
            Arc::new(|_| {}),
            Env::system(),
            std::time::Instant::now(),
        )
        .unwrap();
        RecordingHandle {
            session,
            model: "large-v3-turbo".into(),
            language: "pt".into(),
            source_name: "Som do computador".into(),
            slot: claim_slot(slot).unwrap(),
        }
    }

    #[test]
    fn finish_builds_transcript_and_frees_the_slot() {
        let slot = CancelSlot::default();
        let t = handle(&slot).finish().unwrap();
        assert_eq!(
            (t.model.as_str(), t.language.as_str(), t.source_name.as_str(), t.text.as_str()),
            ("large-v3-turbo", "pt", "Som do computador", "")
        );
        assert!(slot.lock().unwrap().is_none());
    }

    #[test]
    fn finish_after_cancel_returns_cancelled_and_frees_the_slot() {
        let slot = CancelSlot::default();
        let h = handle(&slot);
        // `cancel_transcription` liga a flag guardada no espaço.
        slot.lock().unwrap().as_ref().unwrap().store(true, Ordering::Relaxed);
        assert!(matches!(h.finish(), Err(AppError::Cancelled)));
        assert!(slot.lock().unwrap().is_none());
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

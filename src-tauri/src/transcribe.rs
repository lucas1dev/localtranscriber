use std::ffi::{c_int, c_void, OsString};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::error::AppError;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Segment {
    pub start: String,
    pub end: String,
    pub text: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Transcript {
    pub text: String,
    pub segments: Vec<Segment>,
    pub model: String,
    pub language: String,
    pub source_name: String,
}

/// Converte um instante do whisper (centésimos de segundo) em `HH:MM:SS,mmm`.
pub fn format_ts(centis: i64) -> String {
    let ms = centis.max(0) * 10;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// Argumentos do FFmpeg para gerar PCM f32 mono 16 kHz no stdout.
pub fn ffmpeg_args(input: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-i"]
        .into_iter()
        .map(OsString::from)
        .collect();
    args.push(input.as_os_str().to_os_string());
    args.extend(
        ["-ac", "1", "-ar", "16000", "-f", "f32le", "-"]
            .into_iter()
            .map(OsString::from),
    );
    args
}

/// Converte f32 little-endian que chega em pedaços de qualquer tamanho; os 1–3 bytes
/// que sobram de um pedaço ficam guardados para o próximo.
#[derive(Default)]
struct F32leDecoder {
    rest: Vec<u8>,
}

impl F32leDecoder {
    fn push(&mut self, mut bytes: &[u8], out: &mut Vec<f32>) {
        if !self.rest.is_empty() {
            let need = (4 - self.rest.len()).min(bytes.len());
            self.rest.extend_from_slice(&bytes[..need]);
            bytes = &bytes[need..];
            if self.rest.len() < 4 {
                return;
            }
            out.push(f32::from_le_bytes([self.rest[0], self.rest[1], self.rest[2], self.rest[3]]));
            self.rest.clear();
        }
        let mut samples = bytes.chunks_exact(4);
        out.extend(samples.by_ref().map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
        self.rest.extend_from_slice(samples.remainder());
    }
}

/// Decodifica qualquer áudio/vídeo para as amostras que o whisper espera, lendo a saída
/// do FFmpeg aos poucos. Com `cancel` ligado, mata o FFmpeg e devolve `Cancelled`.
pub fn decode_to_pcm(ffmpeg: &Path, input: &Path, cancel: &AtomicBool) -> Result<Vec<f32>, AppError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(AppError::Cancelled);
    }
    let mut cmd = Command::new(ffmpeg);
    cmd.args(ffmpeg_args(input))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Internal(format!("Não foi possível executar o FFmpeg: {e}")))?;
    let mut stdout = child.stdout.take().expect("stdout em pipe");
    let mut stderr = child.stderr.take().expect("stderr em pipe");
    // O stderr é lido em paralelo para o FFmpeg nunca travar com o pipe cheio.
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let mut pcm = Vec::new();
    let mut decoder = F32leDecoder::default();
    let mut buf = vec![0u8; 64 * 1024];
    let read = loop {
        if cancel.load(Ordering::Relaxed) {
            break Err(AppError::Cancelled);
        }
        match stdout.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => decoder.push(&buf[..n], &mut pcm),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                break Err(AppError::Internal(format!(
                    "Falha ao ler a saída do FFmpeg: {e}"
                )))
            }
        }
    };
    if read.is_err() {
        let _ = child.kill();
    }
    drop(stdout);
    let status = child.wait();
    let stderr = stderr_reader.join().unwrap_or_default();
    read?;
    let status =
        status.map_err(|e| AppError::Internal(format!("Falha ao aguardar o FFmpeg: {e}")))?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        return Err(AppError::User(format!(
            "Não foi possível ler o áudio: {}",
            last_chars(stderr.trim(), 600)
        )));
    }
    Ok(pcm)
}

fn last_chars(s: &str, n: usize) -> &str {
    let skip = s.chars().count().saturating_sub(n);
    s.char_indices().nth(skip).map_or("", |(i, _)| &s[i..])
}

/// Junta os textos dos segmentos, sem espaços nas pontas, separados por um espaço.
pub fn join_text(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|s| s.text.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// RMS (-60 dBFS) abaixo do qual uma janela de 20 ms conta como silêncio.
const SILENCE_RMS: f32 = 0.001;

/// O whisper inventa texto ("Thank you.") em áudio sem som; se nenhuma janela
/// de 20 ms passa do limiar, nem chamamos o modelo.
fn is_silent(pcm: &[f32]) -> bool {
    pcm.chunks(320).all(|w| {
        let mean_sq = w.iter().map(|x| x * x).sum::<f32>() / w.len() as f32;
        mean_sq.sqrt() < SILENCE_RMS
    })
}

pub struct Engine {
    cached: Mutex<Option<(String, WhisperContext)>>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

type ProgressFn = Box<dyn FnMut(i32) + Send>;

// Callbacks crus do whisper.cpp. O `set_abort_callback_safe` do whisper-rs 0.16
// converte o ponteiro para o tipo errado e o `set_progress_callback_safe` vaza o
// closure; por isso os dados apontam para valores locais de `Engine::transcribe`,
// que ficam vivos até `full` retornar.

unsafe extern "C" fn abort_requested(data: *mut c_void) -> bool {
    // SAFETY: `data` aponta para o AtomicBool de `cancel` (ver `transcribe`).
    unsafe { (*(data as *const AtomicBool)).load(Ordering::Relaxed) }
}

// Genérico para não depender da feature `raw-api`: `Ctx`/`State` são inferidos
// como `whisper_context`/`whisper_state` pelo tipo de `set_progress_callback`.
unsafe extern "C" fn report_progress<Ctx, State>(
    _: *mut Ctx,
    _: *mut State,
    progress: c_int,
    data: *mut c_void,
) {
    // SAFETY: `data` aponta para o `ProgressFn` local de `transcribe`.
    let on_progress = unsafe { &mut *(data as *mut ProgressFn) };
    on_progress(progress);
}

impl Engine {
    pub fn new() -> Self {
        Self {
            cached: Mutex::new(None),
        }
    }

    /// Descarta o modelo carregado em cache (ex.: antes de apagar o arquivo).
    pub fn evict(&self) {
        *self.cached.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn transcribe(
        &self,
        model_path: &Path,
        model_id: &str,
        pcm: &[f32],
        language: &str,
        cancel: Arc<AtomicBool>,
        mut on_progress: ProgressFn,
    ) -> Result<Vec<Segment>, AppError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(AppError::Cancelled);
        }
        if is_silent(pcm) {
            return Ok(Vec::new());
        }

        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if cached.as_ref().map(|(id, _)| id.as_str()) != Some(model_id) {
            // Libera o modelo anterior antes de carregar o próximo.
            *cached = None;
            let ctx =
                WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
                    .map_err(|e| {
                        AppError::User(format!(
                            "Não foi possível carregar o modelo {model_id}: {e}"
                        ))
                    })?;
            *cached = Some((model_id.to_string(), ctx));
        }
        let (_, ctx) = cached.as_ref().expect("modelo carregado acima");
        let mut state = ctx
            .create_state()
            .map_err(|e| AppError::Internal(format!("Falha ao preparar o whisper: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: 5,
            patience: -1.0,
        });
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(6));
        params.set_n_threads(threads as i32);
        params.set_language(if language == "auto" {
            None
        } else {
            Some(language)
        });
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // SAFETY: `cancel` e `on_progress` vivem até o fim desta função, depois de `full`.
        unsafe {
            params.set_abort_callback(Some(abort_requested));
            params.set_abort_callback_user_data(Arc::as_ptr(&cancel) as *mut c_void);
            params.set_progress_callback(Some(report_progress));
            params.set_progress_callback_user_data(
                &mut on_progress as *mut ProgressFn as *mut c_void,
            );
        }

        let result = state.full(params, pcm);
        if cancel.load(Ordering::Relaxed) {
            return Err(AppError::Cancelled);
        }
        result.map_err(|e| AppError::Internal(format!("Falha na transcrição: {e}")))?;

        let segments = state
            .as_iter()
            .filter_map(|seg| {
                let text = seg.to_str_lossy().ok()?.trim().to_string();
                if text.is_empty() {
                    return None;
                }
                Some(Segment {
                    start: format_ts(seg.start_timestamp()),
                    end: format_ts(seg.end_timestamp()),
                    text,
                })
            })
            .collect();
        Ok(segments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evict_on_empty_engine_is_noop() {
        let e = Engine::new();
        e.evict();
        assert!(e.cached.lock().unwrap().is_none());
    }

    fn seg(text: &str) -> Segment {
        Segment {
            start: "00:00:00,000".into(),
            end: "00:00:01,000".into(),
            text: text.into(),
        }
    }

    #[test]
    fn format_ts_examples() {
        assert_eq!(format_ts(0), "00:00:00,000");
        assert_eq!(format_ts(123), "00:00:01,230");
        assert_eq!(format_ts(366_050), "01:01:00,500");
    }

    #[test]
    fn ffmpeg_args_keeps_unicode_path() {
        let p = Path::new("/tmp/Reunião ação 1.m4a");
        let a = ffmpeg_args(p);
        assert_eq!(a[5], OsString::from("/tmp/Reunião ação 1.m4a"));
        assert_eq!(a.last().unwrap(), "-");
    }

    #[test]
    fn last_chars_counts_characters_not_bytes() {
        assert_eq!(last_chars("ação", 2), "ão");
        assert_eq!(last_chars("ação", 10), "ação");
        assert_eq!(last_chars("ação", 0), "");
    }

    fn sine(amplitude: f32, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| amplitude * (i as f32 * 440.0 * std::f32::consts::TAU / 16000.0).sin())
            .collect()
    }

    /// Senoide com o RMS em `db` dBFS.
    fn sine_dbfs(db: f32, samples: usize) -> Vec<f32> {
        sine(10f32.powf(db / 20.0) * std::f32::consts::SQRT_2, samples)
    }

    #[test]
    fn digital_silence_and_faint_hiss_are_silent() {
        assert!(is_silent(&[]));
        assert!(is_silent(&vec![0.0; 16000 * 5]));
        assert!(is_silent(&sine_dbfs(-65.0, 16000)));
    }

    #[test]
    fn quiet_speech_level_passes_the_gate() {
        assert!(!is_silent(&sine_dbfs(-55.0, 16000)));
    }

    #[test]
    fn short_sound_in_silence_is_not_silent() {
        let mut pcm = vec![0.0; 16000 * 5];
        pcm.extend(sine(0.1, 1600));
        pcm.extend(vec![0.0; 16000]);
        assert!(!is_silent(&pcm));
    }

    fn decode_all(pieces: &[&[u8]]) -> Vec<f32> {
        let mut dec = F32leDecoder::default();
        let mut out = Vec::new();
        for p in pieces {
            dec.push(p, &mut out);
        }
        out
    }

    #[test]
    fn f32le_decoder_handles_any_split() {
        let samples = [1.0f32, 2.0, -0.5, 0.25];
        let bytes: Vec<u8> = samples.iter().flat_map(|x| x.to_le_bytes()).collect();
        assert_eq!(decode_all(&[&bytes]), samples);
        for split in 0..bytes.len() {
            let (a, b) = bytes.split_at(split);
            assert_eq!(decode_all(&[a, b]), samples, "split {split}");
        }
        let singles: Vec<&[u8]> = bytes.chunks(1).collect();
        assert_eq!(decode_all(&singles), samples);
        // Sobra incompleta no fim é descartada.
        assert_eq!(decode_all(&[&bytes[..6]]), [1.0]);
    }

    #[cfg(unix)]
    mod fake_ffmpeg {
        use super::*;
        use crate::providers::cli::tests::script;
        use std::time::{Duration, Instant};

        #[test]
        fn decodes_streamed_output_in_odd_pieces() {
            let tmp = tempfile::tempdir().unwrap();
            // 1.0, 2.0, -0.5 em f32le, escritos em pedaços que cortam as amostras.
            let ffmpeg = script(
                tmp.path(),
                "ffmpeg",
                r"printf '\000\000'; sleep 0.05; printf '\200\077\000'; sleep 0.05; printf '\000\000\100\000\000'; sleep 0.05; printf '\000\277'",
            );
            let pcm = decode_to_pcm(&ffmpeg, Path::new("x.wav"), &AtomicBool::new(false)).unwrap();
            assert_eq!(pcm, [1.0, 2.0, -0.5]);
        }

        #[test]
        fn failure_reports_stderr() {
            let tmp = tempfile::tempdir().unwrap();
            let ffmpeg = script(tmp.path(), "ffmpeg", "echo 'Invalid data found' >&2; exit 1");
            match decode_to_pcm(&ffmpeg, Path::new("x.wav"), &AtomicBool::new(false)) {
                Err(AppError::User(m)) => assert_eq!(m, "Não foi possível ler o áudio: Invalid data found"),
                other => panic!("{other:?}"),
            }
        }

        #[test]
        fn cancel_before_decoding_does_not_run_ffmpeg() {
            let tmp = tempfile::tempdir().unwrap();
            let ran = tmp.path().join("ran");
            let ffmpeg = script(tmp.path(), "ffmpeg", &format!("touch '{}'", ran.display()));
            let r = decode_to_pcm(&ffmpeg, Path::new("x.wav"), &AtomicBool::new(true));
            assert!(matches!(r, Err(AppError::Cancelled)), "{r:?}");
            assert!(!ran.exists());
        }

        #[test]
        fn cancel_while_decoding_kills_ffmpeg() {
            let tmp = tempfile::tempdir().unwrap();
            let pid_file = tmp.path().join("pid");
            // Saída sem fim, como um áudio muito longo.
            let ffmpeg = script(
                tmp.path(),
                "ffmpeg",
                &format!(
                    r"echo $$ > '{}'; while true; do printf '\000\000\000\000\000\000\000\000'; sleep 0.01; done",
                    pid_file.display()
                ),
            );
            let cancel = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&cancel);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                flag.store(true, Ordering::Relaxed);
            });
            let started = Instant::now();
            let r = decode_to_pcm(&ffmpeg, Path::new("x.wav"), &cancel);
            assert!(matches!(r, Err(AppError::Cancelled)), "{r:?}");
            assert!(started.elapsed() < Duration::from_secs(5));
            let pid = std::fs::read_to_string(&pid_file).unwrap();
            let alive = std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .status()
                .unwrap()
                .success();
            assert!(!alive, "o FFmpeg deveria ter sido encerrado");
        }
    }

    #[test]
    fn engine_can_be_shared_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Engine>();
    }

    #[test]
    fn join_text_empty_is_empty() {
        assert_eq!(join_text(&[]), "");
    }

    #[test]
    fn join_text_trims_and_joins() {
        assert_eq!(join_text(&[seg(" Olá "), seg("mundo ")]), "Olá mundo");
    }
}

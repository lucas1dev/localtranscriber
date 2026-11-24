// Dispositivos de áudio com cpal: lista de microfones, captura do microfone e do
// som do computador (loopback) em mono 16 kHz.
//
// O callback da cpal só converte as amostras para f32 e faz `try_send` para a
// thread de conversão do `OpenSource`; se a fila estiver cheia, descarta e conta.
// A reamostragem (que pode entrar em pânico) roda nessa thread, nunca no callback.
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use serde::Serialize;

use super::resample::ToMono16k;
use crate::error::AppError;

/// Pacotes brutos na fila entre o callback e a thread de conversão
/// (callbacks de 3 a 10 ms: de 3 a 10 s de folga).
const RAW_QUEUE: usize = 1024;
/// Intervalo em que a thread de conversão confere falhas e parada.
const WORKER_TICK: Duration = Duration::from_millis(50);
/// Espera entre tentativas quando o canal de saída está cheio.
const DELIVER_RETRY: Duration = Duration::from_millis(5);

const MIC_GONE: &str = "O microfone foi desconectado.";
const OUTPUT_GONE: &str = "A saída de som foi desconectada.";
const SYSTEM_PERMISSION: &str = "Permita a gravação do som do computador em Ajustes do Sistema → Privacidade e Segurança → Gravação de áudio do sistema e tente de novo.";
const MIC_PERMISSION: &str = "Permita a gravação do microfone em Ajustes do Sistema → Privacidade e Segurança → Microfone e tente de novo.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioInput {
    /// `cpal::DeviceId` em texto (`Display`/`FromStr`), estável entre execuções.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Mic,
    System,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceMsg {
    /// Áudio já em mono 16 kHz.
    Pcm(Vec<f32>),
    /// A captura parou; a fonte não manda mais nada.
    Failed(String),
}

/// Uma captura aberta. O drop para o stream, esvazia a conversão e encerra a thread.
pub struct OpenSource {
    stream: Option<cpal::Stream>,
    worker: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    device_name: String,
    device_id: String,
}

impl OpenSource {
    /// Nome do dispositivo aberto (para o `sourceName`).
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Id do dispositivo aberto. No som do computador, é o id da saída usada,
    /// para comparar com a saída padrão em `should_reopen`.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Pacotes de áudio descartados porque uma fila estava cheia.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for OpenSource {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        // Soltar o stream solta o callback e o remetente da fila bruta.
        drop(self.stream.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Microfones disponíveis; o padrão do sistema vem primeiro.
pub fn list_inputs() -> Result<Vec<AudioInput>, AppError> {
    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| d.id().ok());
    let devices = host
        .input_devices()
        .map_err(|e| AppError::Internal(format!("Não foi possível listar os microfones: {e}")))?;
    let mut list: Vec<AudioInput> = devices
        .filter_map(|device| {
            let id = device.id().ok()?;
            let id_text = id.to_string();
            let name = device
                .description()
                .map(|d| d.name().trim().to_string())
                .ok()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| id_text.clone());
            Some(AudioInput {
                is_default: default_id.as_ref() == Some(&id),
                id: id_text,
                name,
            })
        })
        .collect();
    list.sort_by_key(|i| !i.is_default);
    Ok(list)
}

/// O microfone pedido se existir; senão o padrão; senão o primeiro.
pub fn resolve_mic<'a>(inputs: &'a [AudioInput], wanted: Option<&str>) -> Option<&'a AudioInput> {
    wanted
        .and_then(|w| inputs.iter().find(|i| i.id == w))
        .or_else(|| inputs.iter().find(|i| i.is_default))
        .or_else(|| inputs.first())
}

/// Verdadeiro quando há uma saída padrão nova, diferente da usada.
pub fn should_reopen(current_output: &str, default_output_now: Option<&str>) -> bool {
    matches!(default_output_now, Some(now) if now != current_output)
}

/// Id da saída padrão atual, para `should_reopen`.
pub fn default_output_id() -> Option<String> {
    let device = cpal::default_host().default_output_device()?;
    device.id().ok().map(|id| id.to_string())
}

/// Abre o microfone (`mic_id`, ou o padrão) ou o som do computador (loopback da
/// saída padrão) e manda áudio mono 16 kHz por `tx`.
pub fn open(
    kind: SourceKind,
    mic_id: Option<&str>,
    tx: SyncSender<SourceMsg>,
) -> Result<OpenSource, AppError> {
    let host = cpal::default_host();
    let device = match kind {
        SourceKind::Mic => find_mic(&host, mic_id)?,
        SourceKind::System => host
            .default_output_device()
            .ok_or_else(|| AppError::User("Nenhuma saída de som encontrada.".into()))?,
    };
    // Loopback: stream de entrada sobre o dispositivo de saída, no formato da saída.
    let config = match kind {
        SourceKind::Mic => device.default_input_config(),
        SourceKind::System => device.default_output_config(),
    }
    .map_err(|e| open_error(kind, &e))?;
    let device_id = device.id().map_err(|e| open_error(kind, &e))?.to_string();
    let device_name = device
        .description()
        .map(|d| d.name().trim().to_string())
        .ok()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| device_id.clone());

    let converter = ToMono16k::new(config.sample_rate(), config.channels())?;
    let (raw_tx, raw_rx) = mpsc::sync_channel::<Vec<f32>>(RAW_QUEUE);
    let dropped = Arc::new(AtomicU64::new(0));
    let fault = Arc::new(AtomicU8::new(0));
    let stopping = Arc::new(AtomicBool::new(false));

    let shared = Callbacks {
        raw_tx,
        dropped: dropped.clone(),
        fault: fault.clone(),
    };
    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config.config(), shared),
        SampleFormat::I16 => build::<i16>(&device, config.config(), shared),
        SampleFormat::U16 => build::<u16>(&device, config.config(), shared),
        other => {
            return Err(AppError::User(format!(
                "Formato de áudio não suportado pelo dispositivo ({other})."
            )))
        }
    }
    .map_err(|e| open_error(kind, &e))?;

    let worker = Worker {
        raw_rx,
        converter,
        tx: tx.clone(),
        fault,
        stopping: stopping.clone(),
        dropped: dropped.clone(),
        kind,
    };
    let worker = std::thread::Builder::new()
        .name(format!("audio-{kind:?}").to_lowercase())
        .spawn(move || {
            if catch_unwind(AssertUnwindSafe(|| worker.run())).is_err() {
                let _ = tx.try_send(SourceMsg::Failed(Fault::Other.message(kind)));
            }
        })?;

    let source = OpenSource {
        stream: Some(stream),
        worker: Some(worker),
        stopping,
        dropped,
        device_name,
        device_id,
    };
    if let Some(stream) = &source.stream {
        stream.play().map_err(|e| open_error(kind, &e))?;
    }
    Ok(source)
}

fn find_mic(host: &cpal::Host, mic_id: Option<&str>) -> Result<cpal::Device, AppError> {
    match mic_id {
        // `DeviceId` faz ida e volta por `Display`/`FromStr`.
        Some(id) => cpal::DeviceId::from_str(id)
            .ok()
            .and_then(|id| host.device_by_id(&id))
            .ok_or_else(|| AppError::User(MIC_GONE.into())),
        None => host
            .default_input_device()
            .ok_or_else(|| AppError::User("Nenhum microfone encontrado.".into())),
    }
}

/// O que o callback da cpal leva: só operações que não bloqueiam.
struct Callbacks {
    raw_tx: SyncSender<Vec<f32>>,
    dropped: Arc<AtomicU64>,
    fault: Arc<AtomicU8>,
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    shared: Callbacks,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let Callbacks {
        raw_tx,
        dropped,
        fault,
    } = shared;
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if let Err(TrySendError::Full(_)) = raw_tx.try_send(to_f32(data)) {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        },
        move |err: cpal::Error| {
            // Só marca a falha; quem avisa é a thread de conversão.
            if let Some(f) = Fault::from_kind(err.kind()) {
                let _ = fault.compare_exchange(0, f as u8, Ordering::Relaxed, Ordering::Relaxed);
            }
        },
        None,
    )
}

/// Thread de conversão: fila bruta → mono 16 kHz → `tx`.
struct Worker {
    raw_rx: Receiver<Vec<f32>>,
    converter: ToMono16k,
    tx: SyncSender<SourceMsg>,
    fault: Arc<AtomicU8>,
    stopping: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    kind: SourceKind,
}

impl Worker {
    fn run(mut self) {
        loop {
            match self.raw_rx.recv_timeout(WORKER_TICK) {
                Ok(first) => {
                    let mut out = self.converter.push(&first);
                    while let Ok(more) = self.raw_rx.try_recv() {
                        out.extend(self.converter.push(&more));
                    }
                    if !out.is_empty() && !self.deliver(SourceMsg::Pcm(out)) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.stopping.load(Ordering::Relaxed) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if let Some(fault) = Fault::from_code(self.fault.load(Ordering::Relaxed)) {
                self.flush();
                self.deliver(SourceMsg::Failed(fault.message(self.kind)));
                return;
            }
        }
        self.flush();
    }

    fn flush(&mut self) {
        let tail = self.converter.flush();
        if !tail.is_empty() {
            self.deliver(SourceMsg::Pcm(tail));
        }
    }

    /// Entrega sem perder nada enquanto grava; ao parar, não espera quem não lê.
    /// Devolve falso se o receptor já não existe.
    fn deliver(&self, mut msg: SourceMsg) -> bool {
        loop {
            match self.tx.try_send(msg) {
                Ok(()) => return true,
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(back)) => {
                    if self.stopping.load(Ordering::Relaxed) {
                        self.dropped.fetch_add(1, Ordering::Relaxed);
                        return true;
                    }
                    msg = back;
                    std::thread::sleep(DELIVER_RETRY);
                }
            }
        }
    }
}

/// Falha do stream que encerra a captura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    Disconnected = 1,
    Permission = 2,
    Other = 3,
}

impl Fault {
    /// `None` para avisos que não param o stream.
    fn from_kind(kind: cpal::ErrorKind) -> Option<Fault> {
        use cpal::ErrorKind as K;
        match kind {
            K::Xrun | K::DeviceChanged | K::RealtimeDenied => None,
            K::DeviceNotAvailable => Some(Fault::Disconnected),
            K::PermissionDenied => Some(Fault::Permission),
            _ => Some(Fault::Other),
        }
    }

    fn from_code(code: u8) -> Option<Fault> {
        match code {
            1 => Some(Fault::Disconnected),
            2 => Some(Fault::Permission),
            3 => Some(Fault::Other),
            _ => None,
        }
    }

    fn message(self, source: SourceKind) -> String {
        let generic = match source {
            SourceKind::Mic => "A captura do microfone falhou.",
            SourceKind::System => "A captura do som do computador falhou.",
        };
        let text = match (self, source) {
            (Fault::Disconnected, SourceKind::Mic) => MIC_GONE,
            (Fault::Disconnected, SourceKind::System) => OUTPUT_GONE,
            (Fault::Permission, s) => permission_message(s).unwrap_or(generic),
            (Fault::Other, _) => generic,
        };
        text.to_string()
    }
}

/// Mensagem de permissão negada; só o macOS pede permissão.
fn permission_message(source: SourceKind) -> Option<&'static str> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    Some(match source {
        SourceKind::Mic => MIC_PERMISSION,
        SourceKind::System => SYSTEM_PERMISSION,
    })
}

/// Erro ao abrir o stream, em português para quem usa.
fn open_error(source: SourceKind, err: &cpal::Error) -> AppError {
    use cpal::ErrorKind as K;
    let what = match source {
        SourceKind::Mic => "o microfone",
        SourceKind::System => "o som do computador",
    };
    let text = match (err.kind(), permission_message(source)) {
        (K::PermissionDenied, Some(msg)) => msg.to_string(),
        (K::DeviceNotAvailable, _) => match source {
            SourceKind::Mic => MIC_GONE.to_string(),
            SourceKind::System => OUTPUT_GONE.to_string(),
        },
        (K::DeviceBusy, _) => format!("Outro aplicativo está usando {what}."),
        _ => format!("Não foi possível abrir {what} ({err})."),
    };
    AppError::User(text)
}

fn to_f32<T: Sample>(data: &[T]) -> Vec<f32>
where
    f32: FromSample<T>,
{
    data.iter().map(|s| s.to_sample::<f32>()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, is_default: bool) -> AudioInput {
        AudioInput {
            id: id.into(),
            name: format!("Nome {id}"),
            is_default,
        }
    }

    #[test]
    fn resolve_mic_returns_wanted_when_present() {
        let list = [input("a", true), input("b", false)];
        assert_eq!(resolve_mic(&list, Some("b")).unwrap().id, "b");
    }

    #[test]
    fn resolve_mic_falls_back_to_default_when_wanted_missing() {
        let list = [input("a", false), input("b", true)];
        assert_eq!(resolve_mic(&list, Some("x")).unwrap().id, "b");
        assert_eq!(resolve_mic(&list, None).unwrap().id, "b");
    }

    #[test]
    fn resolve_mic_falls_back_to_first_without_default() {
        let list = [input("a", false), input("b", false)];
        assert_eq!(resolve_mic(&list, Some("x")).unwrap().id, "a");
    }

    #[test]
    fn resolve_mic_empty_list_is_none() {
        assert!(resolve_mic(&[], Some("a")).is_none());
        assert!(resolve_mic(&[], None).is_none());
    }

    #[test]
    fn should_reopen_same_id_is_false() {
        assert!(!should_reopen("coreaudio:x", Some("coreaudio:x")));
    }

    #[test]
    fn should_reopen_new_id_is_true() {
        assert!(should_reopen("coreaudio:x", Some("coreaudio:y")));
    }

    #[test]
    fn should_reopen_without_default_is_false() {
        assert!(!should_reopen("coreaudio:x", None));
    }

    #[test]
    fn fault_classifies_cpal_error_kinds() {
        use cpal::ErrorKind as K;
        assert_eq!(
            Fault::from_kind(K::DeviceNotAvailable),
            Some(Fault::Disconnected)
        );
        assert_eq!(
            Fault::from_kind(K::PermissionDenied),
            Some(Fault::Permission)
        );
        assert_eq!(Fault::from_kind(K::StreamInvalidated), Some(Fault::Other));
        assert_eq!(Fault::from_kind(K::BackendError), Some(Fault::Other));
        // Avisos que não interrompem a captura.
        assert_eq!(Fault::from_kind(K::Xrun), None);
        assert_eq!(Fault::from_kind(K::DeviceChanged), None);
        assert_eq!(Fault::from_kind(K::RealtimeDenied), None);
    }

    #[test]
    fn mic_disconnect_message() {
        assert_eq!(
            Fault::Disconnected.message(SourceKind::Mic),
            "O microfone foi desconectado."
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permission_denied_maps_to_user_message() {
        let err = cpal::Error::new(cpal::ErrorKind::PermissionDenied);
        match open_error(SourceKind::System, &err) {
            AppError::User(m) => assert_eq!(m, SYSTEM_PERMISSION),
            other => panic!("esperava User, veio {other:?}"),
        }
        match open_error(SourceKind::Mic, &err) {
            AppError::User(m) => assert_eq!(m, MIC_PERMISSION),
            other => panic!("esperava User, veio {other:?}"),
        }
        assert_eq!(
            SYSTEM_PERMISSION,
            "Permita a gravação do som do computador em Ajustes do Sistema → Privacidade e Segurança → Gravação de áudio do sistema e tente de novo."
        );
        assert_eq!(
            MIC_PERMISSION,
            "Permita a gravação do microfone em Ajustes do Sistema → Privacidade e Segurança → Microfone e tente de novo."
        );
    }

    #[test]
    fn open_error_for_missing_device_is_user_error() {
        let err = cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable);
        assert!(matches!(
            open_error(SourceKind::Mic, &err),
            AppError::User(_)
        ));
    }

    #[test]
    fn integer_samples_convert_to_f32() {
        assert_eq!(to_f32(&[0i16, i16::MIN]), vec![0.0, -1.0]);
        assert_eq!(to_f32(&[32768u16, 0u16]), vec![0.0, -1.0]);
        assert_eq!(to_f32(&[0.25f32]), vec![0.25]);
    }

    #[test]
    fn open_source_is_send() {
        // A sessão (Task 4) guarda as fontes abertas em outra thread.
        fn assert_send<T: Send>() {}
        assert_send::<OpenSource>();
    }

    #[test]
    fn list_inputs_does_not_panic() {
        // Os runners do CI não têm áudio: Ok vazio ou Err são válidos.
        if let Ok(list) = list_inputs() {
            for item in &list {
                assert!(!item.id.is_empty(), "id vazio: {item:?}");
                assert!(!item.name.is_empty(), "nome vazio: {item:?}");
            }
        }
    }

    /// Abre o microfone padrão e o som do computador ao mesmo tempo por 3 s e
    /// imprime amostras, nível e descartes. Precisa de dispositivos reais.
    #[test]
    #[ignore]
    fn real_capture_smoke() {
        use std::time::Instant;

        match list_inputs() {
            Ok(list) => {
                for i in &list {
                    println!("entrada: {:?} id={} padrão={}", i.name, i.id, i.is_default);
                }
            }
            Err(e) => println!("list_inputs falhou: {e}"),
        }

        let mut opened = Vec::new();
        for kind in [SourceKind::Mic, SourceKind::System] {
            let (tx, rx) = mpsc::sync_channel(64);
            let t = Instant::now();
            match open(kind, None, tx) {
                Ok(src) => {
                    println!(
                        "{kind:?}: aberto {:?} id={} em {} ms",
                        src.device_name(),
                        src.device_id(),
                        t.elapsed().as_millis()
                    );
                    opened.push((kind, src, rx));
                }
                Err(e) => println!("{kind:?}: falhou ao abrir: {e}"),
            }
        }

        #[derive(Default)]
        struct Stats {
            samples: usize,
            peak: f32,
            sum_sq: f64,
            failed: Option<String>,
        }
        fn take(stats: &mut Stats, msg: SourceMsg) {
            match msg {
                SourceMsg::Pcm(p) => {
                    stats.samples += p.len();
                    for s in p {
                        stats.peak = stats.peak.max(s.abs());
                        stats.sum_sq += (s as f64) * (s as f64);
                    }
                }
                SourceMsg::Failed(m) => stats.failed = Some(m),
            }
        }

        let mut stats: Vec<Stats> = opened.iter().map(|_| Stats::default()).collect();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            for ((_, _, rx), st) in opened.iter().zip(stats.iter_mut()) {
                while let Ok(msg) = rx.try_recv() {
                    take(st, msg);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output_now = default_output_id();
        for ((kind, src, rx), mut st) in opened.into_iter().zip(stats) {
            let dropped = src.dropped();
            let reopen = should_reopen(src.device_id(), output_now.as_deref());
            drop(src);
            while let Ok(msg) = rx.try_recv() {
                take(&mut st, msg);
            }
            let rms = if st.samples > 0 {
                (st.sum_sq / st.samples as f64).sqrt()
            } else {
                0.0
            };
            println!(
                "{kind:?}: {} amostras ({:.2} s a 16 kHz), pico {:.4}, rms {:.4}, descartes {dropped}, falha {:?}, should_reopen {reopen}",
                st.samples,
                st.samples as f64 / 16_000.0,
                st.peak,
                rms,
                st.failed
            );
        }
    }
}

// Dispositivos de áudio: lista de microfones, captura do microfone (cpal) e do som
// do computador em mono 16 kHz. O som do computador usa o process tap global do
// Core Audio no Mac (`mac_tap`) e o loopback WASAPI da cpal no Windows.
//
// O callback de áudio só converte as amostras para f32 e faz `try_send` para a
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
pub(super) const SYSTEM_PERMISSION: &str = "Permita a gravação do som do computador em Ajustes do Sistema → Privacidade e Segurança → Gravação de áudio do sistema e tente de novo.";
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

/// Um pacote do callback: amostras intercaladas e o formato em que chegaram.
/// O formato vem em cada pacote porque o tap do Mac pode mudar de taxa quando
/// a saída muda (por exemplo, um fone Bluetooth em modo de chamada).
pub(super) struct Raw {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub channels: u16,
}

/// O que o callback de áudio leva: só operações que não bloqueiam.
pub(super) struct RawSink {
    raw_tx: SyncSender<Raw>,
    dropped: Arc<AtomicU64>,
}

impl RawSink {
    /// `try_send`; se a fila estiver cheia, descarta e conta. Nunca bloqueia.
    pub fn push(&self, raw: Raw) {
        if let Err(TrySendError::Full(_)) = self.raw_tx.try_send(raw) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

enum Backend {
    Cpal(cpal::Stream),
    #[cfg(target_os = "macos")]
    MacTap(super::mac_tap::SystemTap),
}

/// Uma captura aberta. O drop para a captura, esvazia a conversão e encerra a thread.
pub struct OpenSource {
    backend: Option<Backend>,
    worker: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    device_name: String,
    output_id: Option<String>,
}

impl OpenSource {
    /// Nome do dispositivo aberto (para o `sourceName`).
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Contrato com a sessão para a troca de saída:
    /// - `Some(id)`: som do computador preso a uma saída (loopback WASAPI). Reabrir
    ///   quando `should_reopen(id, default_output_id())` for verdadeiro.
    /// - `None`: nunca reabrir por troca de saída. É o microfone, ou o tap global
    ///   do Mac, que já segue qualquer saída.
    pub fn output_id(&self) -> Option<&str> {
        self.output_id.as_deref()
    }

    /// Pacotes de áudio descartados porque uma fila estava cheia.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for OpenSource {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        // Soltar a captura solta o callback e o remetente da fila bruta.
        drop(self.backend.take());
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
            let name = device_name(&device).unwrap_or_else(|| id_text.clone());
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

/// Abre o microfone (`mic_id`, ou o padrão) ou o som do computador e manda áudio
/// mono 16 kHz por `tx`.
pub fn open(
    kind: SourceKind,
    mic_id: Option<&str>,
    tx: SyncSender<SourceMsg>,
) -> Result<OpenSource, AppError> {
    let (raw_tx, raw_rx) = mpsc::sync_channel::<Raw>(RAW_QUEUE);
    let dropped = Arc::new(AtomicU64::new(0));
    let fault = Arc::new(AtomicU8::new(0));
    let stopping = Arc::new(AtomicBool::new(false));
    let sink = RawSink {
        raw_tx,
        dropped: dropped.clone(),
    };

    let (backend, device_name, output_id) = match kind {
        SourceKind::Mic => {
            let device = find_mic(mic_id)?;
            let config = device
                .default_input_config()
                .map_err(|e| open_error(kind, &e))?;
            let name = device_name(&device).unwrap_or_else(|| "Microfone".into());
            let stream = build_stream(kind, &device, &config, sink, fault.clone())?;
            (Backend::Cpal(stream), name, None)
        }
        #[cfg(target_os = "macos")]
        SourceKind::System => {
            let _ = &fault; // o tap não tem callback de erro
            let tap = super::mac_tap::SystemTap::open(sink)?;
            (Backend::MacTap(tap), "Som do computador".to_string(), None)
        }
        #[cfg(not(target_os = "macos"))]
        SourceKind::System => {
            // Loopback: stream de entrada sobre a saída padrão, no formato da saída.
            let device = cpal::default_host()
                .default_output_device()
                .ok_or_else(|| AppError::User("Nenhuma saída de som encontrada.".into()))?;
            let config = device
                .default_output_config()
                .map_err(|e| open_error(kind, &e))?;
            let id = device.id().map_err(|e| open_error(kind, &e))?.to_string();
            let name = device_name(&device).unwrap_or_else(|| id.clone());
            let stream = build_stream(kind, &device, &config, sink, fault.clone())?;
            (Backend::Cpal(stream), name, Some(id))
        }
    };

    let worker = Worker {
        raw_rx,
        converter: None,
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
        backend: Some(backend),
        worker: Some(worker),
        stopping,
        dropped,
        device_name,
        output_id,
    };
    // O tap do Mac já começa em `SystemTap::open`; o stream da cpal começa aqui.
    if let Some(Backend::Cpal(stream)) = &source.backend {
        stream.play().map_err(|e| open_error(kind, &e))?;
    }
    Ok(source)
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device
        .description()
        .map(|d| d.name().trim().to_string())
        .ok()
        .filter(|n| !n.is_empty())
}

fn find_mic(mic_id: Option<&str>) -> Result<cpal::Device, AppError> {
    let host = cpal::default_host();
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

fn build_stream(
    kind: SourceKind,
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    sink: RawSink,
    fault: Arc<AtomicU8>,
) -> Result<cpal::Stream, AppError> {
    let stream_config = config.config();
    match config.sample_format() {
        SampleFormat::F32 => build::<f32>(device, stream_config, sink, fault),
        SampleFormat::I16 => build::<i16>(device, stream_config, sink, fault),
        SampleFormat::U16 => build::<u16>(device, stream_config, sink, fault),
        other => {
            return Err(AppError::User(format!(
                "Formato de áudio não suportado pelo dispositivo ({other})."
            )))
        }
    }
    .map_err(|e| open_error(kind, &e))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    sink: RawSink,
    fault: Arc<AtomicU8>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let (rate, channels) = (config.sample_rate, config.channels);
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            sink.push(Raw {
                samples: to_f32(data),
                rate,
                channels,
            });
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

/// Conversor ativo e o formato de entrada para o qual foi criado.
struct Converter {
    rate: u32,
    channels: u16,
    inner: ToMono16k,
}

/// Thread de conversão: fila bruta → mono 16 kHz → `tx`.
struct Worker {
    raw_rx: Receiver<Raw>,
    converter: Option<Converter>,
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
                    let mut out = Vec::new();
                    let mut result = self.convert(first, &mut out);
                    while result.is_ok() {
                        match self.raw_rx.try_recv() {
                            Ok(more) => result = self.convert(more, &mut out),
                            Err(_) => break,
                        }
                    }
                    if !out.is_empty() && !self.deliver(SourceMsg::Pcm(out)) {
                        return;
                    }
                    if result.is_err() {
                        self.fail(Fault::Other);
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

    /// Converte um pacote; se o formato mudou, esvazia o conversor antigo e cria outro.
    fn convert(&mut self, raw: Raw, out: &mut Vec<f32>) -> Result<(), AppError> {
        let same = matches!(&self.converter,
            Some(c) if c.rate == raw.rate && c.channels == raw.channels);
        if !same {
            if let Some(mut old) = self.converter.take() {
                out.extend(old.inner.flush());
            }
            self.converter = Some(Converter {
                rate: raw.rate,
                channels: raw.channels,
                inner: ToMono16k::new(raw.rate, raw.channels)?,
            });
        }
        if let Some(conv) = self.converter.as_mut() {
            out.extend(conv.inner.push(&raw.samples));
        }
        Ok(())
    }

    /// Marca uma falha para o laço avisar e encerrar.
    fn fail(&self, fault: Fault) {
        let _ = self
            .fault
            .compare_exchange(0, fault as u8, Ordering::Relaxed, Ordering::Relaxed);
    }

    fn flush(&mut self) {
        if let Some(mut conv) = self.converter.take() {
            let tail = conv.inner.flush();
            if !tail.is_empty() {
                self.deliver(SourceMsg::Pcm(tail));
            }
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

    fn run_worker(packets: Vec<Raw>) -> Vec<SourceMsg> {
        let (raw_tx, raw_rx) = mpsc::sync_channel(packets.len() + 1);
        for p in packets {
            raw_tx.send(p).unwrap();
        }
        drop(raw_tx);
        let (tx, rx) = mpsc::sync_channel(10_000);
        Worker {
            raw_rx,
            converter: None,
            tx,
            fault: Arc::new(AtomicU8::new(0)),
            stopping: Arc::new(AtomicBool::new(false)),
            dropped: Arc::new(AtomicU64::new(0)),
            kind: SourceKind::System,
        }
        .run();
        rx.try_iter().collect()
    }

    fn packets(rate: u32, channels: u16, seconds: u32) -> Vec<Raw> {
        let frames_per_packet = rate / 100;
        (0..seconds * 100)
            .map(|_| Raw {
                samples: vec![0.1; (frames_per_packet * channels as u32) as usize],
                rate,
                channels,
            })
            .collect()
    }

    #[test]
    fn worker_follows_format_changes() {
        // 1 s a 48 kHz mono e depois 1 s a 24 kHz estéreo: 2 s a 16 kHz.
        let mut input = packets(48_000, 1, 1);
        input.extend(packets(24_000, 2, 1));
        let msgs = run_worker(input);
        let mut total = 0;
        for m in &msgs {
            match m {
                SourceMsg::Pcm(p) => total += p.len(),
                SourceMsg::Failed(e) => panic!("falha inesperada: {e}"),
            }
        }
        assert!((total as i64 - 32_000).abs() <= 2, "total {total}");
    }

    #[test]
    fn worker_reports_invalid_format() {
        let mut input = packets(48_000, 1, 1);
        input.push(Raw {
            samples: vec![0.0; 10],
            rate: 0,
            channels: 1,
        });
        let msgs = run_worker(input);
        assert_eq!(
            msgs.last(),
            Some(&SourceMsg::Failed(
                "A captura do som do computador falhou.".into()
            ))
        );
        // O áudio anterior à falha não se perde.
        let total: usize = msgs
            .iter()
            .map(|m| match m {
                SourceMsg::Pcm(p) => p.len(),
                _ => 0,
            })
            .sum();
        assert!((total as i64 - 16_000).abs() <= 2, "total {total}");
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
                        "{kind:?}: aberto {:?} output_id={:?} em {} ms",
                        src.device_name(),
                        src.output_id(),
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
            let reopen = src
                .output_id()
                .map(|id| should_reopen(id, output_now.as_deref()));
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
                "{kind:?}: {} amostras ({:.2} s a 16 kHz), pico {:.4}, rms {:.4}, descartes {dropped}, falha {:?}, should_reopen {reopen:?}",
                st.samples,
                st.samples as f64 / 16_000.0,
                st.peak,
                rms,
                st.failed
            );
        }
    }
}

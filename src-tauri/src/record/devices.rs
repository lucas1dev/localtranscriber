// Dispositivos de áudio: lista de microfones, captura do microfone (cpal) e do som
// do computador em mono 16 kHz. O som do computador usa o process tap global do
// Core Audio no Mac (`mac_tap`) e o loopback WASAPI da cpal no Windows.
//
// O callback de áudio só converte as amostras para f32 (um `Vec` pequeno por
// callback) e faz `try_send` para a thread de conversão do `OpenSource`; se a fila
// estiver cheia, descarta e conta. A thread de conversão consulta a fila sem
// esperar nela, para o `try_send` nunca ter de acordar ninguém.
// A reamostragem (que pode entrar em pânico) roda nessa thread, nunca no callback.
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
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
/// Intervalo entre as consultas da thread de conversão à fila bruta.
const WORKER_POLL: Duration = Duration::from_millis(5);
/// Parando, a thread de conversão termina depois de tanto tempo sem pacotes.
const WORKER_TICK: Duration = Duration::from_millis(50);
/// Espera entre tentativas quando o canal de saída está cheio.
const DELIVER_RETRY: Duration = Duration::from_millis(5);

pub(super) const MIC_GONE: &str = "O microfone foi desconectado.";
const OUTPUT_GONE: &str = "A saída de som foi desconectada.";
pub(super) const SYSTEM_PERMISSION: &str = "Permita a gravação do som do computador em Ajustes do Sistema → Privacidade e Segurança → Gravação de áudio do sistema e tente de novo.";
const WINDOWS_MIC_PRIVACY: &str = "Permita o acesso ao microfone em Configurações → Privacidade e segurança → Microfone (inclusive para aplicativos da área de trabalho) e tente de novo.";
pub(super) const MIC_PERMISSION: &str = "Permita a gravação do microfone em Ajustes do Sistema → Privacidade e Segurança → Microfone e tente de novo.";

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
    // Só fica guardado para o drop parar o tap.
    #[cfg(target_os = "macos")]
    MacTap(#[allow(dead_code)] super::mac_tap::SystemTap),
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
            // Os listeners do tap guardam um ponteiro para a célula de falha e
            // podem rodar logo depois de removidos: a célula nunca é liberada.
            let fault_cell: &'static Arc<AtomicU8> = Box::leak(Box::new(fault.clone()));
            let tap = super::mac_tap::SystemTap::open(sink, fault_cell)?;
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
            // Um `Vec` pequeno por callback (alguns KB): aceito; o resto não bloqueia.
            sink.push(Raw {
                samples: to_f32(data),
                rate,
                channels,
            });
        },
        move |err: cpal::Error| {
            // Só marca a falha; quem avisa é a thread de conversão.
            if let Some(f) = Fault::from_kind(err.kind()) {
                record_fault(&fault, f);
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
    /// Consulta a fila bruta com `try_recv` e dorme `WORKER_POLL` entre as voltas.
    /// Nunca espera bloqueada na fila: com o receptor estacionado, o `try_send`
    /// do callback de tempo real teria de pegar uma trava e acordá-lo.
    fn run(mut self) {
        let mut idle = Duration::ZERO;
        loop {
            let mut out = Vec::new();
            let mut result = Ok(());
            let mut received = false;
            let mut closed = false;
            while result.is_ok() {
                match self.raw_rx.try_recv() {
                    Ok(raw) => {
                        received = true;
                        result = self.convert(raw, &mut out);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        closed = true;
                        break;
                    }
                }
            }
            if !out.is_empty() && !self.deliver(SourceMsg::Pcm(out)) {
                return;
            }
            if result.is_err() {
                self.fail(Fault::Other);
            }
            let stopping = self.stopping.load(Ordering::Relaxed);
            // A captura soltou a fila sem a gravação ter parado: no WASAPI a cpal
            // avisa o erro e encerra a thread dela. Sem `Failed`, a sessão
            // completaria esta fonte com silêncio para sempre.
            let fault = Fault::from_code(self.fault.load(Ordering::Relaxed))
                .or((closed && !stopping).then_some(Fault::Other));
            if let Some(fault) = fault {
                self.flush();
                self.deliver(SourceMsg::Failed(fault.message(self.kind)));
                return;
            }
            if closed {
                break;
            }
            // Parando: termina quando a fila fecha ou fica `WORKER_TICK` sem nada.
            idle = if received { Duration::ZERO } else { idle + WORKER_POLL };
            if stopping && idle >= WORKER_TICK {
                break;
            }
            std::thread::sleep(WORKER_POLL);
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
        record_fault(&self.fault, fault);
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

/// Guarda a primeira falha da captura; a thread de conversão avisa e encerra.
/// Só um `compare_exchange`: pode ser chamado de callbacks de áudio e listeners.
pub(super) fn record_fault(cell: &AtomicU8, fault: Fault) {
    let _ = cell.compare_exchange(0, fault as u8, Ordering::Relaxed, Ordering::Relaxed);
}

/// Falha do stream que encerra a captura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fault {
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
        self.message_on(Platform::CURRENT, source)
    }

    fn message_on(self, platform: Platform, source: SourceKind) -> String {
        let generic = match source {
            SourceKind::Mic => "A captura do microfone falhou.",
            SourceKind::System => "A captura do som do computador falhou.",
        };
        let text = match (self, source) {
            (Fault::Disconnected, SourceKind::Mic) => MIC_GONE,
            (Fault::Disconnected, SourceKind::System) => OUTPUT_GONE,
            (Fault::Permission, s) => permission_message(platform, s).unwrap_or(generic),
            (Fault::Other, _) => generic,
        };
        text.to_string()
    }
}

/// Onde o app roda (só macOS e Windows); parâmetro para testar as mensagens das duas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Mac,
    Windows,
}

impl Platform {
    const CURRENT: Platform = if cfg!(target_os = "macos") {
        Platform::Mac
    } else {
        Platform::Windows
    };
}

/// Mensagem de falha genérica da captura de `source`.
pub(super) fn capture_failed(source: SourceKind) -> String {
    Fault::Other.message(source)
}

/// Mensagem de permissão negada. No Windows não há pedido, mas o microfone pode
/// estar bloqueado nas configurações de privacidade; o som do computador não.
fn permission_message(platform: Platform, source: SourceKind) -> Option<&'static str> {
    match (platform, source) {
        (Platform::Mac, SourceKind::Mic) => Some(MIC_PERMISSION),
        (Platform::Mac, SourceKind::System) => Some(SYSTEM_PERMISSION),
        (Platform::Windows, SourceKind::Mic) => Some(WINDOWS_MIC_PRIVACY),
        (Platform::Windows, SourceKind::System) => None,
    }
}

/// Acesso negado: o tipo da cpal ou, no WASAPI, `E_ACCESSDENIED`, que a cpal 0.18
/// entrega como `BackendError` com o texto (localizado) do sistema seguido de
/// "(os error -2147024891)".
fn is_access_denied(err: &cpal::Error) -> bool {
    err.kind() == cpal::ErrorKind::PermissionDenied
        || err.message().is_some_and(|m| {
            let m = m.to_ascii_lowercase();
            m.contains("-2147024891") || m.contains("0x80070005")
        })
}

/// Erro ao abrir o stream, em português para quem usa.
fn open_error(source: SourceKind, err: &cpal::Error) -> AppError {
    open_error_on(Platform::CURRENT, source, err)
}

fn open_error_on(platform: Platform, source: SourceKind, err: &cpal::Error) -> AppError {
    use cpal::ErrorKind as K;
    let what = match source {
        SourceKind::Mic => "o microfone",
        SourceKind::System => "o som do computador",
    };
    let denied = is_access_denied(err)
        .then(|| permission_message(platform, source))
        .flatten();
    let text = match (err.kind(), denied) {
        (_, Some(msg)) => msg.to_string(),
        (K::DeviceNotAvailable, _) => match source {
            SourceKind::Mic => MIC_GONE.to_string(),
            SourceKind::System => OUTPUT_GONE.to_string(),
        },
        (K::DeviceBusy, _) => format!("Outro aplicativo está usando {what}."),
        // O detalhe da cpal (em inglês) só no fim, entre parênteses.
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

    fn user_text(e: AppError) -> String {
        match e {
            AppError::User(m) => m,
            other => panic!("esperava User, veio {other:?}"),
        }
    }

    #[test]
    fn permission_denied_maps_to_user_message() {
        let err = cpal::Error::new(cpal::ErrorKind::PermissionDenied);
        assert_eq!(user_text(open_error_on(Platform::Mac, SourceKind::System, &err)), SYSTEM_PERMISSION);
        assert_eq!(user_text(open_error_on(Platform::Mac, SourceKind::Mic, &err)), MIC_PERMISSION);
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
    fn windows_mic_access_denied_points_to_privacy_settings() {
        assert_eq!(
            WINDOWS_MIC_PRIVACY,
            "Permita o acesso ao microfone em Configurações → Privacidade e segurança → Microfone (inclusive para aplicativos da área de trabalho) e tente de novo."
        );
        let kind = cpal::Error::new(cpal::ErrorKind::PermissionDenied);
        assert_eq!(user_text(open_error_on(Platform::Windows, SourceKind::Mic, &kind)), WINDOWS_MIC_PRIVACY);
        // A cpal 0.18 entrega E_ACCESSDENIED como BackendError com o texto do sistema
        // (localizado); o código do HRESULT aparece em "(os error -2147024891)".
        for message in [
            "Failed to initialize audio client: Access is denied. (os error -2147024891)",
            "Falha ao iniciar: Acesso negado. (os error -2147024891)",
            "HRESULT 0x80070005",
        ] {
            let err = cpal::Error::with_message(cpal::ErrorKind::BackendError, message);
            assert_eq!(user_text(open_error_on(Platform::Windows, SourceKind::Mic, &err)), WINDOWS_MIC_PRIVACY, "{message}");
        }
        // O mesmo erro do som do computador não fala de microfone.
        let err = cpal::Error::with_message(cpal::ErrorKind::BackendError, "Access is denied. (os error -2147024891)");
        assert!(user_text(open_error_on(Platform::Windows, SourceKind::System, &err)).starts_with("Não foi possível abrir o som do computador ("));
    }

    #[test]
    fn other_open_errors_start_in_portuguese_with_detail_at_the_end() {
        let err = cpal::Error::with_message(cpal::ErrorKind::BackendError, "Device format query failed");
        assert_eq!(
            user_text(open_error_on(Platform::Windows, SourceKind::Mic, &err)),
            "Não foi possível abrir o microfone (Device format query failed)."
        );
    }

    #[test]
    fn windows_runtime_permission_fault_uses_privacy_message() {
        assert_eq!(Fault::Permission.message_on(Platform::Windows, SourceKind::Mic), WINDOWS_MIC_PRIVACY);
        assert_eq!(Fault::Permission.message_on(Platform::Mac, SourceKind::Mic), MIC_PERMISSION);
        assert_eq!(
            Fault::Permission.message_on(Platform::Windows, SourceKind::System),
            "A captura do som do computador falhou."
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

    fn worker(raw_rx: Receiver<Raw>, tx: SyncSender<SourceMsg>, kind: SourceKind, stopping: bool) -> (Worker, Arc<AtomicU8>) {
        let fault = Arc::new(AtomicU8::new(0));
        let worker = Worker {
            raw_rx,
            converter: None,
            tx,
            fault: fault.clone(),
            stopping: Arc::new(AtomicBool::new(stopping)),
            dropped: Arc::new(AtomicU64::new(0)),
            kind,
        };
        (worker, fault)
    }

    /// Como o drop do `OpenSource`: marca a parada e solta a fila bruta.
    fn run_worker(packets: Vec<Raw>) -> Vec<SourceMsg> {
        let (raw_tx, raw_rx) = mpsc::sync_channel(packets.len() + 1);
        for p in packets {
            raw_tx.send(p).unwrap();
        }
        drop(raw_tx);
        let (tx, rx) = mpsc::sync_channel(10_000);
        worker(raw_rx, tx, SourceKind::System, true).0.run();
        rx.try_iter().collect()
    }

    #[test]
    fn capture_thread_ending_with_fault_reports_it() {
        // WASAPI: a cpal chama o callback de erro e encerra a thread de captura,
        // soltando o remetente da fila bruta sem a gravação ter parado.
        let (raw_tx, raw_rx) = mpsc::sync_channel(16);
        let (tx, rx) = mpsc::sync_channel(16);
        let (w, fault) = worker(raw_rx, tx, SourceKind::Mic, false);
        record_fault(&fault, Fault::Disconnected);
        drop(raw_tx);
        w.run();
        let msgs: Vec<SourceMsg> = rx.try_iter().collect();
        assert_eq!(msgs.last(), Some(&SourceMsg::Failed("O microfone foi desconectado.".into())));
    }

    #[test]
    fn capture_thread_ending_without_fault_is_a_failure() {
        let (raw_tx, raw_rx) = mpsc::sync_channel(16);
        let (tx, rx) = mpsc::sync_channel(16);
        let (w, _) = worker(raw_rx, tx, SourceKind::Mic, false);
        drop(raw_tx);
        w.run();
        let msgs: Vec<SourceMsg> = rx.try_iter().collect();
        assert_eq!(msgs, [SourceMsg::Failed("A captura do microfone falhou.".into())]);
    }

    #[test]
    fn stopping_ends_worker_once_the_queue_goes_quiet() {
        // O stream ainda não soltou a fila, mas a gravação parou e nada mais chega:
        // a thread termina sozinha e entrega o que já tinha.
        let (raw_tx, raw_rx) = mpsc::sync_channel(1024);
        let (tx, rx) = mpsc::sync_channel(1024);
        let (w, _) = worker(raw_rx, tx, SourceKind::System, true);
        for p in packets(48_000, 1, 1) {
            raw_tx.send(p).unwrap();
        }
        let started = std::time::Instant::now();
        w.run();
        assert!(started.elapsed() < Duration::from_secs(2));
        let total: usize = rx
            .try_iter()
            .map(|m| match m {
                SourceMsg::Pcm(p) => p.len(),
                SourceMsg::Failed(e) => panic!("falha inesperada: {e}"),
            })
            .sum();
        assert!((total as i64 - 16_000).abs() <= 2, "total {total}");
        drop(raw_tx);
    }

    #[test]
    fn raw_queue_closing_while_stopping_is_not_a_failure() {
        let (raw_tx, raw_rx) = mpsc::sync_channel(16);
        let (tx, rx) = mpsc::sync_channel(16);
        let (w, _) = worker(raw_rx, tx, SourceKind::Mic, true);
        drop(raw_tx);
        w.run();
        assert!(rx.try_iter().next().is_none());
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
    fn system_fault_cell_ends_worker_with_failed() {
        // O fio de falha que o tap do Mac usa: a fila bruta continua aberta e só a
        // célula de falha encerra a captura.
        let (raw_tx, raw_rx) = mpsc::sync_channel(1024);
        let (tx, rx) = mpsc::sync_channel(1024);
        let (worker, fault) = worker(raw_rx, tx, SourceKind::System, false);
        let handle = std::thread::spawn(move || worker.run());
        for p in packets(48_000, 1, 1) {
            raw_tx.send(p).unwrap();
        }
        std::thread::sleep(Duration::from_millis(200));
        record_fault(&fault, Fault::Other);
        handle.join().unwrap();

        let msgs: Vec<SourceMsg> = rx.try_iter().collect();
        assert_eq!(
            msgs.last(),
            Some(&SourceMsg::Failed(
                "A captura do som do computador falhou.".into()
            ))
        );
        let total: usize = msgs
            .iter()
            .map(|m| match m {
                SourceMsg::Pcm(p) => p.len(),
                _ => 0,
            })
            .sum();
        assert!((total as i64 - 16_000).abs() <= 2, "total {total}");
        drop(raw_tx);
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

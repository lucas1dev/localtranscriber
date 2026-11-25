// Sessão de gravação: junta as fontes num relógio comum, corta blocos de 12 s e
// os transcreve numa fila única (um Whisper por vez). Não depende da cpal: as
// fontes chegam como canais de `SourceMsg`, e reabrir, o relógio e a saída
// padrão são injetados, para os testes rodarem sem dispositivos.
//
// Duas threads:
// - coleta: a cada 100 ms drena os canais para os `Track`s, completa com silêncio
//   quem atrasou, emite o nível, manda os blocos prontos para a fila e cuida de
//   reabrir fontes que falharam (ou, no Windows, cuja saída padrão mudou);
// - transcrição: tira os blocos da fila (ilimitada) um por vez.
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::devices::{self, OpenSource, OutputBinding, SourceKind, SourceMsg, MIC_GONE};
use super::merge::{interleave, labeled_text, shift_segments};
use super::track::{level, Track};
use crate::error::AppError;
use crate::transcribe::{join_text, Segment};

/// Intervalo da coleta (e do evento de nível).
const TICK: Duration = Duration::from_millis(100);
/// O preenchimento periódico fica esta folga atrás do relógio: um bloco de até
/// 500 ms (o tap do Core Audio manda blocos maiores que 200 ms) pode estar a
/// caminho, e preencher antes dele chegar contaria o mesmo trecho duas vezes.
const PAD_MARGIN: Duration = Duration::from_millis(500);
/// A cada quantos ciclos conferir se a saída padrão mudou (1 s).
const OUTPUT_POLL_TICKS: u64 = 10;
/// Reaberturas seguidas (sem a fonte se firmar) antes de desistir.
const MAX_TRIES: u32 = 3;
/// Tempo (pelo relógio da sessão) que uma fonte reaberta precisa ficar de pé, sem
/// `Failed`, para zerar a contagem. Uma fonte que abre e logo falha de novo
/// continua contando; um loopback mudo do Windows, que não manda pacotes, não.
const HEALTHY_FOR: Duration = Duration::from_secs(5);
/// Ciclos de espera antes da primeira tentativa de reabrir (500 ms); dobra a cada
/// tentativa seguida.
const RETRY_TICKS: u32 = 5;

/// Ciclos de espera antes da próxima tentativa, com `tries` tentativas já feitas.
/// O ciclo da coleta conta: a espera real fica a até 100 ms disso.
fn retry_ticks(tries: u32) -> u32 {
    RETRY_TICKS << tries.min(MAX_TRIES)
}
/// Sem áudio novo por tantos ciclos, o nível cai para zero (500 ms).
const LEVEL_HOLD_TICKS: u32 = 5;

/// Uma captura aberta. Soltar o valor para a captura.
pub trait Capture: Send {
    /// `Some` só quando a captura está presa a uma saída (ver `OpenSource::binding`).
    fn output_id(&self) -> Option<&str>;
    /// Pacotes de áudio descartados porque uma fila estava cheia.
    fn dropped(&self) -> u64;
}

impl Capture for OpenSource {
    fn output_id(&self) -> Option<&str> {
        match OpenSource::binding(self) {
            OutputBinding::FollowDefault(id) => Some(id),
            _ => None,
        }
    }
    fn dropped(&self) -> u64 {
        OpenSource::dropped(self)
    }
}

/// Uma captura e o canal por onde ela manda o áudio.
pub struct Opened {
    pub capture: Box<dyn Capture>,
    pub rx: Receiver<SourceMsg>,
}

/// Abre de novo a mesma fonte (o mesmo `mic_id`, ou a saída padrão atual).
pub type Reopen = Box<dyn FnMut() -> Result<Opened, AppError> + Send>;
pub type TranscribeFn = dyn Fn(&[f32]) -> Result<Vec<Segment>, AppError> + Send + Sync;
pub type Transcriber = Arc<TranscribeFn>;
pub type Sink = Arc<dyn Fn(SessionEvent) + Send + Sync>;

pub struct SourceSpec {
    pub kind: SourceKind,
    /// `None` com uma fonte só; `"voce"`/`"reuniao"` no modo duplo.
    pub speaker: Option<&'static str>,
    pub opened: Opened,
    pub reopen: Reopen,
}

/// O que a sessão consulta do mundo: o relógio e a saída de som padrão.
pub struct Env {
    pub now: Arc<dyn Fn() -> Instant + Send + Sync>,
    pub default_output: Arc<dyn Fn() -> Option<String> + Send + Sync>,
}

impl Env {
    pub fn system() -> Self {
        Self {
            now: Arc::new(Instant::now),
            default_output: Arc::new(devices::default_output_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Text { speaker: Option<&'static str>, text: String },
    Level { mic: Option<f32>, system: Option<f32> },
    Error { message: String },
}

/// Resultado da gravação, já montado.
#[derive(Debug)]
pub struct Recorded {
    pub text: String,
    pub segments: Vec<Segment>,
}

/// Estado comum às duas threads: só o primeiro erro vira evento e encerra a sessão.
struct Shared {
    sink: Sink,
    failed: AtomicBool,
}

impl Shared {
    fn fail(&self, message: String) {
        if !self.failed.swap(true, Ordering::SeqCst) {
            (self.sink)(SessionEvent::Error { message });
        }
    }

    fn has_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }
}

struct Job {
    source: usize,
    index: u32,
    pcm: Vec<f32>,
}

pub struct Session {
    stop_tx: Option<Sender<()>>,
    collector: Option<JoinHandle<()>>,
    transcriber: Option<JoinHandle<Vec<Vec<Segment>>>>,
}

impl Session {
    /// Começa a coletar das fontes já abertas. `start` é o zero do relógio comum:
    /// o instante de antes de abrir a primeira fonte, para o áudio que ela já
    /// guardou enquanto as outras abriam contar a partir do zero, e as que abriram
    /// depois serem completadas com silêncio. Com duas fontes, a primeira ganha os
    /// empates de tempo no texto (Você antes de Reunião).
    pub fn new(
        sources: Vec<SourceSpec>,
        transcriber: Transcriber,
        sink: Sink,
        env: Env,
        start: Instant,
    ) -> Result<Session, AppError> {
        let speakers: Vec<Option<&'static str>> = sources.iter().map(|s| s.speaker).collect();
        let shared = Arc::new(Shared { sink, failed: AtomicBool::new(false) });
        let (stop_tx, stop_rx) = mpsc::channel();
        let (job_tx, job_rx) = mpsc::channel();

        let worker_shared = Arc::clone(&shared);
        let transcriber = std::thread::Builder::new()
            .name("gravacao-transcricao".into())
            .spawn(move || transcribe_jobs(job_rx, &speakers, &*transcriber, &worker_shared))?;

        let panic_shared = Arc::clone(&shared);
        let collector = Collector {
            sources: sources.into_iter().map(|s| Live::new(s, start)).collect(),
            env,
            stop_rx,
            job_tx,
            shared,
        };
        let collector = std::thread::Builder::new()
            .name("gravacao-coleta".into())
            .spawn(move || {
                // O pânico solta as capturas e a fila; a transcrição termina o que
                // já estava nela e `stop` devolve esse resultado parcial.
                if catch_unwind(AssertUnwindSafe(|| collector.run())).is_err() {
                    panic_shared.fail("A captura da gravação falhou.".into());
                }
            })?;

        Ok(Session {
            stop_tx: Some(stop_tx),
            collector: Some(collector),
            transcriber: Some(transcriber),
        })
    }

    /// Para as fontes, transcreve o que estiver na fila e monta o resultado.
    /// Se a sessão já tinha parado sozinha por erro (inclusive pânico na coleta,
    /// que já virou um `Error`), devolve o que foi transcrito.
    /// Bloqueia até a fila esvaziar.
    pub fn stop(mut self) -> Result<Recorded, AppError> {
        drop(self.stop_tx.take());
        if let Some(collector) = self.collector.take() {
            let _ = collector.join();
        }
        let by_source = self
            .transcriber
            .take()
            .and_then(|h| h.join().ok())
            .ok_or_else(|| AppError::Internal("A transcrição da gravação falhou.".into()))?;
        let dual = by_source.len() > 1;
        let segments = by_source.into_iter().reduce(interleave).unwrap_or_default();
        let text = if dual { labeled_text(&segments) } else { join_text(&segments) };
        Ok(Recorded { text, segments })
    }
}

/// Thread de transcrição: um bloco por vez, até a fila fechar e esvaziar.
fn transcribe_jobs(
    jobs: Receiver<Job>,
    speakers: &[Option<&'static str>],
    transcriber: &TranscribeFn,
    shared: &Shared,
) -> Vec<Vec<Segment>> {
    let mut by_source = vec![Vec::new(); speakers.len()];
    for job in jobs {
        let speaker = speakers[job.source];
        let result = catch_unwind(AssertUnwindSafe(|| transcriber(&job.pcm)))
            .unwrap_or_else(|_| Err(AppError::Internal("Falha na transcrição.".into())));
        match result {
            Ok(segs) => {
                let text = join_text(&segs);
                if !text.is_empty() {
                    (shared.sink)(SessionEvent::Text { speaker, text });
                }
                by_source[job.source].extend(shift_segments(segs, job.index, speaker));
            }
            // Cancelado pela pessoa: os blocos restantes passam direto.
            Err(AppError::Cancelled) => {}
            Err(e) => shared.fail(e.to_string()),
        }
    }
    by_source
}

/// Uma fonte durante a gravação.
struct Live {
    kind: SourceKind,
    /// `None` enquanto a fonte está caída, esperando reabrir.
    capture: Option<Box<dyn Capture>>,
    rx: Option<Receiver<SourceMsg>>,
    reopen: Reopen,
    track: Track,
    /// Tentativas de reabrir desde a última vez que a fonte se firmou (`HEALTHY_FOR`
    /// de pé depois de reabrir). Troca de saída bem-sucedida não conta.
    tries: u32,
    /// Quando a captura atual foi aberta.
    opened_at: Instant,
    /// Ciclos até a próxima tentativa, com a fonte caída.
    retry_in: u32,
    /// Áudio drenado no último ciclo.
    pending: Vec<f32>,
    level: f32,
    quiet_ticks: u32,
    dropped: u64,
}

impl Live {
    fn new(spec: SourceSpec, start: Instant) -> Self {
        Self {
            kind: spec.kind,
            capture: Some(spec.opened.capture),
            rx: Some(spec.opened.rx),
            reopen: spec.reopen,
            track: Track::new(start),
            tries: 0,
            opened_at: start,
            retry_in: 0,
            pending: Vec::new(),
            level: 0.0,
            quiet_ticks: 0,
            dropped: 0,
        }
    }

    /// Passa todo o áudio que chegou para a trilha, num `push` só: assim um ciclo
    /// atrasado (vários blocos na fila) não parece fonte atrasada.
    /// Devolve a mensagem de falha, se a captura avisou que parou.
    fn drain(&mut self, now: Instant) -> Option<String> {
        self.pending.clear();
        let mut failed = None;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(SourceMsg::Pcm(pcm)) => self.pending.extend_from_slice(&pcm),
                    Ok(SourceMsg::Failed(message)) => {
                        failed = Some(message);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    // Com a captura aberta, o canal só fecha se a thread de
                    // conversão sumiu sem avisar: é uma falha, não silêncio.
                    Err(TryRecvError::Disconnected) => {
                        if self.capture.is_some() {
                            failed = Some(devices::capture_failed(self.kind));
                        }
                        break;
                    }
                }
            }
        }
        if !self.pending.is_empty() {
            self.track.push(&self.pending, now);
        }
        failed
    }

    fn update_level(&mut self) {
        if self.pending.is_empty() {
            self.quiet_ticks += 1;
            if self.quiet_ticks >= LEVEL_HOLD_TICKS {
                self.level = 0.0;
            }
        } else {
            self.level = level(&self.pending);
            self.quiet_ticks = 0;
        }
    }

    /// Solta a captura e guarda o que ela ainda tinha mandado.
    fn close(&mut self, now: Instant) {
        if let Some(capture) = self.capture.take() {
            self.dropped += capture.dropped();
        }
        self.drain(now);
        self.rx = None;
    }

    fn install(&mut self, opened: Opened, now: Instant) {
        self.capture = Some(opened.capture);
        self.rx = Some(opened.rx);
        self.opened_at = now;
    }

    /// A captura parou. Microfone desconectado, ou uma fonte que já foi reaberta
    /// `MAX_TRIES` vezes sem se firmar, encerra a gravação; qualquer outra falha
    /// deixa a fonte caída, para reabrir.
    fn on_failed(&mut self, message: String, now: Instant) -> Result<(), String> {
        self.close(now);
        if self.kind == SourceKind::Mic && message == MIC_GONE {
            return Err(message);
        }
        if self.tries >= MAX_TRIES {
            return Err(message);
        }
        self.retry_in = retry_ticks(self.tries);
        Ok(())
    }

    /// Erro quando acabam as tentativas; senão espera para tentar de novo.
    fn reopen_failed(&mut self, err: AppError) -> Result<(), String> {
        self.retry_in = retry_ticks(self.tries);
        if self.tries >= MAX_TRIES {
            Err(err.to_string())
        } else {
            Ok(())
        }
    }

    /// Reabre a fonte caída, ou a de sistema cuja saída padrão mudou.
    /// Roda depois de `drain`/`on_failed` no mesmo ciclo: uma captura ainda aberta
    /// aqui não falhou até `now`.
    fn maintain(&mut self, tick: u64, env: &Env, now: Instant) -> Result<(), String> {
        if self.capture.is_some()
            && self.tries > 0
            && now.saturating_duration_since(self.opened_at) >= HEALTHY_FOR
        {
            self.tries = 0;
        }
        match &self.capture {
            None => {
                if self.retry_in > 0 {
                    self.retry_in -= 1;
                    return Ok(());
                }
                self.tries += 1;
                match (self.reopen)() {
                    Ok(opened) => {
                        self.install(opened, now);
                        Ok(())
                    }
                    Err(e) => self.reopen_failed(e),
                }
            }
            Some(capture) if tick.is_multiple_of(OUTPUT_POLL_TICKS) => {
                let Some(current) = capture.output_id() else {
                    return Ok(());
                };
                if !devices::should_reopen(current, (env.default_output)().as_deref()) {
                    return Ok(());
                }
                match (self.reopen)() {
                    Ok(opened) => {
                        // O relógio continua; o intervalo da troca vira silêncio.
                        self.close(now);
                        self.install(opened, now);
                        Ok(())
                    }
                    Err(e) => {
                        self.tries += 1;
                        self.reopen_failed(e)
                    }
                }
            }
            Some(_) => Ok(()),
        }
    }
}

struct Collector {
    sources: Vec<Live>,
    env: Env,
    stop_rx: Receiver<()>,
    job_tx: Sender<Job>,
    shared: Arc<Shared>,
}

impl Collector {
    fn run(mut self) {
        let mut tick: u64 = 0;
        let mut fatal = None;
        loop {
            // `stop` solta o remetente: a espera acaba na hora.
            let stopping = !matches!(self.stop_rx.recv_timeout(TICK), Err(RecvTimeoutError::Timeout));
            tick += 1;
            let now = (self.env.now)();
            for s in &mut self.sources {
                let failed = s.drain(now);
                s.update_level();
                if let Some(message) = failed {
                    if let Err(m) = s.on_failed(message, now) {
                        fatal.get_or_insert(m);
                    }
                }
            }
            if !stopping && fatal.is_none() {
                for s in &mut self.sources {
                    if let Err(m) = s.maintain(tick, &self.env, now) {
                        fatal.get_or_insert(m);
                    }
                }
            }
            if let Some(until) = now.checked_sub(PAD_MARGIN) {
                for s in &mut self.sources {
                    s.track.pad_to(until);
                }
            }
            self.emit_level();
            self.send_ready();
            if stopping || fatal.is_some() || self.shared.has_failed() {
                break;
            }
        }

        // Encerramento (por `stop` ou por erro): solta as capturas, guarda o resto do
        // áudio, completa até agora e manda as sobras para a fila.
        let now = (self.env.now)();
        for s in &mut self.sources {
            s.close(now);
            s.track.pad_to(now);
        }
        if let Some(message) = fatal {
            self.shared.fail(message);
        }
        self.send_ready();
        for (i, s) in self.sources.into_iter().enumerate() {
            if s.dropped > 0 {
                eprintln!("gravação: {} pacotes de áudio descartados ({:?})", s.dropped, s.kind);
            }
            if let Some((index, pcm)) = s.track.finish() {
                let _ = self.job_tx.send(Job { source: i, index, pcm });
            }
        }
        // `job_tx` sai daqui: a thread de transcrição esvazia a fila e termina.
    }

    fn emit_level(&self) {
        let (mut mic, mut system) = (None, None);
        for s in &self.sources {
            match s.kind {
                SourceKind::Mic => mic = Some(s.level),
                SourceKind::System => system = Some(s.level),
            }
        }
        (self.shared.sink)(SessionEvent::Level { mic, system });
    }

    /// Blocos completos para a fila; no modo duplo as fontes se alternam.
    fn send_ready(&mut self) {
        for (i, s) in self.sources.iter_mut().enumerate() {
            while let Some((index, pcm)) = s.track.take_chunk() {
                let _ = self.job_tx.send(Job { source: i, index, pcm });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, SyncSender};
    use std::sync::Mutex;
    use std::time::Duration;

    use crate::transcribe::format_ts;

    const MIC_GONE: &str = "O microfone foi desconectado.";
    const WAIT: Duration = Duration::from_secs(10);
    const SECOND: usize = 16_000;

    /// Relógio que só anda quando o teste manda.
    #[derive(Clone)]
    struct FakeClock {
        base: Instant,
        ms: Arc<AtomicU64>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self { base: Instant::now(), ms: Arc::new(AtomicU64::new(0)) }
        }
        fn now(&self) -> Instant {
            self.base + Duration::from_millis(self.ms.load(Ordering::SeqCst))
        }
        fn advance(&self, secs: f64) {
            self.ms.fetch_add((secs * 1000.0).round() as u64, Ordering::SeqCst);
        }
        fn env(&self, output: Arc<Mutex<Option<String>>>) -> Env {
            let clock = self.clone();
            Env {
                now: Arc::new(move || clock.now()),
                default_output: Arc::new(move || output.lock().unwrap().clone()),
            }
        }
        fn plain_env(&self) -> Env {
            self.env(Arc::new(Mutex::new(None)))
        }
    }

    struct FakeCapture {
        output: Option<String>,
        dropped: Arc<AtomicBool>,
    }

    impl Capture for FakeCapture {
        fn output_id(&self) -> Option<&str> {
            self.output.as_deref()
        }
        fn dropped(&self) -> u64 {
            0
        }
    }

    impl Drop for FakeCapture {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    /// Fonte fake: o remetente fica com o teste; a flag diz se a captura foi solta.
    fn fake_source(output: Option<&str>) -> (SyncSender<SourceMsg>, Opened, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::sync_channel(4096);
        let dropped = Arc::new(AtomicBool::new(false));
        let capture = FakeCapture { output: output.map(str::to_string), dropped: dropped.clone() };
        (tx, Opened { capture: Box::new(capture), rx }, dropped)
    }

    fn no_reopen() -> Reopen {
        Box::new(|| Err(AppError::User("sem reabrir".into())))
    }

    fn spec(kind: SourceKind, speaker: Option<&'static str>, opened: Opened, reopen: Reopen) -> SourceSpec {
        SourceSpec { kind, speaker, opened, reopen }
    }

    fn seg(start: &str, text: String) -> Segment {
        Segment { start: start.into(), end: start.into(), text, speaker: None }
    }

    /// Devolve um segmento em 00:00:01 com o texto `b{n}` (n = chamadas com som);
    /// bloco só de zeros não tem fala, como o filtro de energia do motor real.
    fn fake_transcriber() -> Transcriber {
        let calls = AtomicUsize::new(0);
        Arc::new(move |pcm: &[f32]| {
            if pcm.iter().all(|s| *s == 0.0) {
                return Ok(Vec::new());
            }
            let n = calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![seg("00:00:01,000", format!("b{n}"))])
        })
    }

    fn sink() -> (Sink, Receiver<SessionEvent>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (Arc::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }), rx)
    }

    fn wait_for(rx: &Receiver<SessionEvent>, pred: impl Fn(&SessionEvent) -> bool) -> SessionEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(e) if pred(&e) => return e,
                Ok(_) => continue,
                Err(e) => panic!("evento esperado não chegou: {e:?}"),
            }
        }
    }

    fn second(v: f32) -> SourceMsg {
        SourceMsg::Pcm(vec![v; SECOND])
    }

    /// Manda `secs` blocos de 1 s e avança o relógio junto (a fonte nunca atrasa).
    fn feed(tx: &SyncSender<SourceMsg>, clock: &FakeClock, secs: usize) {
        for _ in 0..secs {
            tx.send(second(0.1)).unwrap();
            clock.advance(1.0);
        }
    }

    fn starts(segs: &[Segment], speaker: Option<&str>) -> Vec<String> {
        segs.iter()
            .filter(|s| s.speaker.as_deref() == speaker)
            .map(|s| s.start.clone())
            .collect()
    }

    #[test]
    fn single_source_produces_plain_text() {
        let clock = FakeClock::new();
        let (tx, opened, _) = fake_source(None);
        let (sink, _events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::Mic, None, opened, no_reopen())],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&tx, &clock, 25);
        let out = session.stop().unwrap();
        assert_eq!(out.text, "b0 b1 b2");
        assert!(out.segments.iter().all(|s| s.speaker.is_none()));
        assert_eq!(starts(&out.segments, None), ["00:00:01,000", "00:00:13,000", "00:00:25,000"]);
    }

    #[test]
    fn dual_source_interleaves_and_labels() {
        let clock = FakeClock::new();
        let (mic_tx, mic, _) = fake_source(None);
        let (sys_tx, sys, _) = fake_source(None);
        let (sink, _events) = sink();
        let session = Session::new(
            vec![
                spec(SourceKind::Mic, Some("voce"), mic, no_reopen()),
                spec(SourceKind::System, Some("reuniao"), sys, no_reopen()),
            ],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        for _ in 0..25 {
            mic_tx.send(second(0.1)).unwrap();
            sys_tx.send(second(0.2)).unwrap();
            clock.advance(1.0);
        }
        let out = session.stop().unwrap();
        // "[00:01] Você: b0" → "[00:01] Você:"
        let prefixes: Vec<String> = out
            .text
            .lines()
            .map(|l| l.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect();
        assert_eq!(
            prefixes,
            [
                "[00:01] Você:",
                "[00:01] Reunião:",
                "[00:13] Você:",
                "[00:13] Reunião:",
                "[00:25] Você:",
                "[00:25] Reunião:",
            ],
            "{}",
            out.text
        );
        assert_eq!(starts(&out.segments, Some("voce")).len(), 3);
        assert_eq!(starts(&out.segments, Some("reuniao")).len(), 3);
    }

    #[test]
    fn silent_system_source_keeps_clock() {
        let clock = FakeClock::new();
        let (mic_tx, mic, _) = fake_source(None);
        let (sys_tx, sys, _) = fake_source(None);
        let (sink, _events) = sink();
        let session = Session::new(
            vec![
                spec(SourceKind::Mic, Some("voce"), mic, no_reopen()),
                spec(SourceKind::System, Some("reuniao"), sys, no_reopen()),
            ],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&mic_tx, &clock, 25);
        // O som do computador fica mudo 24 s; o bloco que chega em 25 s cobre 24–25 s.
        sys_tx.send(second(0.2)).unwrap();
        let out = session.stop().unwrap();
        let reuniao = starts(&out.segments, Some("reuniao"));
        assert_eq!(reuniao.len(), 1, "{}", out.text);
        assert!(reuniao[0].as_str() >= "00:00:24,000", "{reuniao:?}");
        assert_eq!(starts(&out.segments, Some("voce")).len(), 3);
    }

    #[test]
    fn slow_transcriber_loses_nothing() {
        let clock = FakeClock::new();
        let (mic_tx, mic, _) = fake_source(None);
        let (sys_tx, sys, _) = fake_source(None);
        let (sink, _events) = sink();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let transcriber: Transcriber = {
            let (active, max_active) = (active.clone(), max_active.clone());
            Arc::new(move |_pcm: &[f32]| {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(vec![seg("00:00:01,000", "x".into())])
            })
        };
        let session = Session::new(
            vec![
                spec(SourceKind::Mic, Some("voce"), mic, no_reopen()),
                spec(SourceKind::System, Some("reuniao"), sys, no_reopen()),
            ],
            transcriber,
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        // 60 s de cada fonte de uma vez: 5 + 5 blocos na fila.
        for _ in 0..60 {
            mic_tx.send(second(0.1)).unwrap();
            sys_tx.send(second(0.2)).unwrap();
        }
        clock.advance(60.0);
        let out = session.stop().unwrap();
        assert_eq!(out.segments.len(), 10);
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn mic_failure_stops_with_partial_result() {
        let clock = FakeClock::new();
        let (tx, opened, dropped) = fake_source(None);
        let (sink, events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::Mic, None, opened, no_reopen())],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&tx, &clock, 12);
        tx.send(SourceMsg::Failed(MIC_GONE.into())).unwrap();
        let e = wait_for(&events, |e| matches!(e, SessionEvent::Error { .. }));
        assert_eq!(e, SessionEvent::Error { message: MIC_GONE.into() });
        // Encerrou sozinha, como em `stop`: a captura já foi solta.
        assert!(dropped.load(Ordering::SeqCst));
        let out = session.stop().unwrap();
        assert_eq!(out.text, "b0");
    }

    /// Reabertura fake: avisa o teste por `opened_tx` com o remetente da nova fonte.
    fn counting_reopen(
        output: Option<&'static str>,
    ) -> (Reopen, Receiver<SyncSender<SourceMsg>>, Arc<AtomicUsize>, Arc<AtomicBool>) {
        let (opened_tx, opened_rx) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let new_dropped = Arc::new(AtomicBool::new(false));
        let reopen: Reopen = {
            let (calls, new_dropped) = (calls.clone(), new_dropped.clone());
            Box::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                let (tx, rx) = mpsc::sync_channel(4096);
                let capture = FakeCapture {
                    output: output.map(str::to_string),
                    dropped: new_dropped.clone(),
                };
                let _ = opened_tx.send(tx);
                Ok(Opened { capture: Box::new(capture), rx })
            })
        };
        (reopen, opened_rx, calls, new_dropped)
    }

    #[test]
    fn system_reopens_on_default_change() {
        let clock = FakeClock::new();
        let output = Arc::new(Mutex::new(Some("A".to_string())));
        let (old_tx, opened, old_dropped) = fake_source(Some("A"));
        let (reopen, new_sources, calls, _) = counting_reopen(Some("B"));
        let (sink, _events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::System, None, opened, reopen)],
            fake_transcriber(),
            sink,
            clock.env(output.clone()),
            clock.now(),
        )
        .unwrap();
        feed(&old_tx, &clock, 6);
        *output.lock().unwrap() = Some("B".into());
        let new_tx = new_sources.recv_timeout(WAIT).expect("reabriu na saída nova");
        feed(&new_tx, &clock, 7);
        let out = session.stop().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(old_dropped.load(Ordering::SeqCst));
        // Os 6 s da saída antiga e os 7 s da nova formam um bloco e uma sobra.
        assert_eq!(out.text, "b0 b1");
    }

    #[test]
    fn mic_capture_failure_reopens_same_mic() {
        let clock = FakeClock::new();
        let (old_tx, opened, _) = fake_source(None);
        let (reopen, new_sources, calls, _) = counting_reopen(None);
        let (sink, events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::Mic, None, opened, reopen)],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&old_tx, &clock, 12);
        old_tx.send(SourceMsg::Failed("A captura do microfone falhou.".into())).unwrap();
        let new_tx = new_sources.recv_timeout(WAIT).expect("reabriu o microfone");
        feed(&new_tx, &clock, 12);
        let out = session.stop().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(out.text, "b0 b1");
        assert!(!events.try_iter().any(|e| matches!(e, SessionEvent::Error { .. })));
    }

    #[test]
    fn source_channel_closing_without_failed_reopens() {
        // A thread de conversão sumiu sem avisar (pânico com a fila cheia, por
        // exemplo): o canal fechado conta como falha, não como silêncio.
        let clock = FakeClock::new();
        let (old_tx, opened, old_dropped) = fake_source(None);
        let (reopen, new_sources, calls, _) = counting_reopen(None);
        let (sink, events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::Mic, None, opened, reopen)],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&old_tx, &clock, 12);
        drop(old_tx);
        let new_tx = new_sources.recv_timeout(WAIT).expect("reabriu o microfone");
        assert!(old_dropped.load(Ordering::SeqCst));
        feed(&new_tx, &clock, 12);
        let out = session.stop().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(out.text, "b0 b1");
        assert!(!events.try_iter().any(|e| matches!(e, SessionEvent::Error { .. })));
    }

    #[test]
    fn source_channel_closing_gives_up_like_a_failure() {
        let clock = FakeClock::new();
        let (tx, opened, _) = fake_source(None);
        let (sink, events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::System, None, opened, no_reopen())],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&tx, &clock, 12);
        drop(tx);
        let e = wait_for(&events, |e| matches!(e, SessionEvent::Error { .. }));
        assert_eq!(e, SessionEvent::Error { message: "sem reabrir".into() });
        assert_eq!(session.stop().unwrap().text, "b0");
    }

    #[test]
    fn retry_delays_double_from_half_a_second() {
        // Ciclos de 100 ms antes da 1ª, 2ª e 3ª tentativa: 0,5 s, 1 s, 2 s.
        assert_eq!([0, 1, 2].map(retry_ticks), [5, 10, 20]);
    }

    #[test]
    fn system_failure_gives_up_after_three_tries() {
        let clock = FakeClock::new();
        let (tx, opened, _) = fake_source(None);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let reopen: Reopen = {
            let calls = calls.clone();
            Box::new(move || {
                calls.lock().unwrap().push(Instant::now());
                Err(AppError::User("Nenhuma saída de som encontrada.".into()))
            })
        };
        let (sink, events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::System, None, opened, reopen)],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&tx, &clock, 12);
        let failed_at = Instant::now();
        tx.send(SourceMsg::Failed("A captura do som do computador falhou.".into())).unwrap();
        let e = wait_for(&events, |e| matches!(e, SessionEvent::Error { .. }));
        assert_eq!(e, SessionEvent::Error { message: "Nenhuma saída de som encontrada.".into() });
        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 3);
        // Espera 0,5 s, 1 s e 2 s antes de cada tentativa (com folga de um ciclo).
        let gaps = [calls[0] - failed_at, calls[1] - calls[0], calls[2] - calls[1]];
        for (gap, min_ms) in gaps.iter().zip([400, 900, 1900]) {
            assert!(gap.as_millis() >= min_ms, "intervalos {gaps:?}");
        }
        assert_eq!(session.stop().unwrap().text, "b0");
    }

    /// Reabertura que sempre abre; cada fonte nova entrega `secs` segundos e, até a
    /// chamada `fail_until`, já manda `Failed` em seguida. Avisa o número da chamada.
    fn flaky_reopen(
        secs: usize,
        fail_until: usize,
        message: &'static str,
    ) -> (Reopen, Receiver<usize>, Arc<AtomicUsize>) {
        let (called_tx, called_rx) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let reopen: Reopen = {
            let calls = calls.clone();
            Box::new(move || {
                let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                let (tx, rx) = mpsc::sync_channel(4096);
                for _ in 0..secs {
                    tx.send(second(0.1)).unwrap();
                }
                if n <= fail_until {
                    tx.send(SourceMsg::Failed(message.into())).unwrap();
                }
                let _ = called_tx.send(n);
                let capture = FakeCapture { output: None, dropped: Arc::new(AtomicBool::new(false)) };
                Ok(Opened { capture: Box::new(capture), rx })
            })
        };
        (reopen, called_rx, calls)
    }

    #[test]
    fn reopened_source_failing_at_once_gives_up_after_three_tries() {
        const FAILED: &str = "A captura do som do computador falhou.";
        let clock = FakeClock::new();
        let (tx, opened, _) = fake_source(None);
        let (reopen, _called, calls) = flaky_reopen(0, usize::MAX, FAILED);
        let (sink, events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::System, None, opened, reopen)],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&tx, &clock, 12);
        tx.send(SourceMsg::Failed(FAILED.into())).unwrap();
        let e = wait_for(&events, |e| matches!(e, SessionEvent::Error { .. }));
        assert_eq!(e, SessionEvent::Error { message: FAILED.into() });
        assert_eq!(session.stop().unwrap().text, "b0");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(!events.try_iter().any(|e| matches!(e, SessionEvent::Error { .. })));
    }

    /// A fonte cai logo no começo e cada reabertura fica de pé 6 s de relógio antes
    /// de falhar de novo (com ou sem áudio); a quinta fica de pé até o fim.
    fn recover_four_times(audio: bool) -> (Recorded, usize, Vec<SessionEvent>) {
        const FAILED: &str = "A captura do microfone falhou.";
        let clock = FakeClock::new();
        let (tx, opened, _) = fake_source(None);
        let (reopen, new_sources, calls, _) = counting_reopen(None);
        let (sink, events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::Mic, None, opened, reopen)],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        tx.send(SourceMsg::Failed(FAILED.into())).unwrap();
        let mut seen = Vec::new();
        for _ in 0..4 {
            let tx = new_sources.recv_timeout(WAIT).expect("reabriu de novo");
            if audio {
                feed(&tx, &clock, 6);
            } else {
                clock.advance(6.0);
            }
            full_tick(&events, &mut seen); // um ciclo inteiro viu a fonte de pé há 6 s
            tx.send(SourceMsg::Failed(FAILED.into())).unwrap();
        }
        let _last = new_sources.recv_timeout(WAIT).expect("quinta reabertura");
        let out = session.stop().unwrap();
        seen.extend(events.try_iter());
        (out, calls.load(Ordering::SeqCst), seen)
    }

    /// Espera um ciclo inteiro da coleta começar e terminar depois desta chamada:
    /// descarta (guardando em `seen`) os eventos já na fila e espera dois de nível.
    fn full_tick(events: &Receiver<SessionEvent>, seen: &mut Vec<SessionEvent>) {
        seen.extend(events.try_iter());
        let mut levels = 0;
        while levels < 2 {
            let e = events.recv_timeout(WAIT).expect("ciclo da coleta");
            levels += usize::from(matches!(e, SessionEvent::Level { .. }));
            seen.push(e);
        }
    }

    #[test]
    fn source_that_recovers_keeps_recording_past_three_failures() {
        let (out, calls, events) = recover_four_times(true);
        assert_eq!(calls, 5);
        assert!(!events.iter().any(|e| matches!(e, SessionEvent::Error { .. })));
        // 4 × 6 s = 24 s: dois blocos.
        assert_eq!(out.text, "b0 b1");
    }

    #[test]
    fn silent_reopened_source_still_resets_the_count() {
        // Como o loopback do Windows sem nada tocando: de pé, mas sem pacotes.
        let (out, calls, events) = recover_four_times(false);
        assert_eq!(calls, 5);
        assert!(!events.iter().any(|e| matches!(e, SessionEvent::Error { .. })));
        assert_eq!(out.text, "");
    }

    #[test]
    fn audio_queued_while_other_source_opens_keeps_the_clock() {
        // O microfone abre primeiro e grava 1 s enquanto o som do computador abre.
        // O zero do relógio é antes de abrir o microfone: Você começa em 0 s e
        // Reunião (que só existe a partir de 1 s) é completada com silêncio.
        let clock = FakeClock::new();
        let start = clock.now();
        let (mic_tx, mic, _) = fake_source(None);
        mic_tx.send(second(0.1)).unwrap();
        clock.advance(1.0);
        let (sys_tx, sys, _) = fake_source(None);
        let (sink, _events) = sink();
        // Segmento no primeiro instante com som do bloco.
        let transcriber: Transcriber = Arc::new(|pcm: &[f32]| {
            Ok(match pcm.iter().position(|s| *s != 0.0) {
                Some(i) => vec![seg(&format_ts(i as i64 / 160), "x".into())],
                None => Vec::new(),
            })
        });
        let session = Session::new(
            vec![
                spec(SourceKind::Mic, Some("voce"), mic, no_reopen()),
                spec(SourceKind::System, Some("reuniao"), sys, no_reopen()),
            ],
            transcriber,
            sink,
            clock.plain_env(),
            start,
        )
        .unwrap();
        for _ in 0..12 {
            mic_tx.send(second(0.1)).unwrap();
            clock.advance(1.0);
            // O bloco do sistema chega no fim do segundo que ele cobre.
            sys_tx.send(second(0.2)).unwrap();
        }
        let out = session.stop().unwrap();
        assert_eq!(starts(&out.segments, Some("voce"))[0], "00:00:00,000");
        let reuniao = &starts(&out.segments, Some("reuniao"))[0];
        assert!(
            ("00:00:01,000".."00:00:02,000").contains(&reuniao.as_str()),
            "{reuniao}"
        );
    }

    /// Transcritor fake que faz `on(n)` na chamada `n` (só blocos com som).
    fn scripted_transcriber(
        on: impl Fn(usize) -> Result<Vec<Segment>, AppError> + Send + Sync + 'static,
    ) -> Transcriber {
        let calls = AtomicUsize::new(0);
        Arc::new(move |pcm: &[f32]| {
            if pcm.iter().all(|s| *s == 0.0) {
                return Ok(Vec::new());
            }
            on(calls.fetch_add(1, Ordering::SeqCst))
        })
    }

    fn b(n: usize) -> Result<Vec<Segment>, AppError> {
        Ok(vec![seg("00:00:01,000", format!("b{n}"))])
    }

    /// Grava 36 s (três blocos) de uma fonte e devolve o resultado e os eventos.
    /// Com `ends_alone`, antes de `stop` exige que a sessão solte a captura sozinha.
    fn record_36s(transcriber: Transcriber, ends_alone: bool) -> (Result<Recorded, AppError>, Vec<SessionEvent>) {
        let clock = FakeClock::new();
        let (tx, opened, dropped) = fake_source(None);
        let (sink, events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::Mic, None, opened, no_reopen())],
            transcriber,
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&tx, &clock, 36);
        if ends_alone {
            let deadline = Instant::now() + WAIT;
            while !dropped.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "a sessão não encerrou sozinha");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let out = session.stop();
        // `stop` juntou as duas threads: todos os eventos já chegaram.
        (out, events.try_iter().collect())
    }

    fn errors(events: &[SessionEvent]) -> Vec<&SessionEvent> {
        events.iter().filter(|e| matches!(e, SessionEvent::Error { .. })).collect()
    }

    #[test]
    fn transcriber_error_ends_with_one_error_and_partial_result() {
        let (out, events) = record_36s(
            scripted_transcriber(|n| match n {
                0 => b(0),
                _ => Err(AppError::User("Não foi possível carregar o modelo.".into())),
            }),
            true,
        );
        assert_eq!(
            errors(&events),
            [&SessionEvent::Error { message: "Não foi possível carregar o modelo.".into() }]
        );
        assert_eq!(out.unwrap().text, "b0");
    }

    #[test]
    fn transcriber_panic_ends_with_error_and_partial_result() {
        let (out, events) = record_36s(
            scripted_transcriber(|n| match n {
                0 => b(0),
                _ => panic!("whisper quebrou"),
            }),
            true,
        );
        assert_eq!(errors(&events), [&SessionEvent::Error { message: "Falha na transcrição.".into() }]);
        assert_eq!(out.unwrap().text, "b0");
    }

    #[test]
    fn cancelled_chunks_are_skipped_without_error() {
        let (out, events) = record_36s(
            scripted_transcriber(|n| match n {
                1 => Err(AppError::Cancelled),
                n => b(n),
            }),
            false,
        );
        assert!(errors(&events).is_empty());
        assert_eq!(out.unwrap().text, "b0 b2");
    }

    #[test]
    fn collector_panic_ends_with_one_error_and_partial_result() {
        let clock = FakeClock::new();
        let (tx, opened, dropped) = fake_source(None);
        // O nível é emitido pela thread de coleta: um sink que entra em pânico ali a
        // derruba. Ele só é armado depois do primeiro bloco transcrito.
        let armed = Arc::new(AtomicBool::new(false));
        let (events_tx, events) = mpsc::channel();
        let sink: Sink = {
            let armed = armed.clone();
            let events_tx = Mutex::new(events_tx);
            Arc::new(move |e| {
                if armed.load(Ordering::SeqCst) && matches!(e, SessionEvent::Level { .. }) {
                    panic!("sink quebrou");
                }
                let _ = events_tx.lock().unwrap().send(e);
            })
        };
        let session = Session::new(
            vec![spec(SourceKind::Mic, None, opened, no_reopen())],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&tx, &clock, 12);
        wait_for(&events, |e| matches!(e, SessionEvent::Text { .. }));
        armed.store(true, Ordering::SeqCst);
        let e = wait_for(&events, |e| matches!(e, SessionEvent::Error { .. }));
        assert_eq!(e, SessionEvent::Error { message: "A captura da gravação falhou.".into() });
        let out = session.stop().expect("resultado parcial");
        assert_eq!(out.text, "b0");
        assert!(dropped.load(Ordering::SeqCst));
        assert!(errors(&events.try_iter().collect::<Vec<_>>()).is_empty());
    }

    #[test]
    fn long_blocks_on_time_are_not_padded() {
        // Blocos de 300 ms (como o tap do Core Audio) chegam no fim do trecho que cobrem.
        // Entre o relógio andar e o bloco chegar a fonte parece 300 ms atrasada; com a
        // margem de 500 ms, o preenchimento periódico não pode colocar zeros aí.
        let clock = FakeClock::new();
        let (tx, opened, _) = fake_source(None);
        let (sink, events) = sink();
        let zeros = Arc::new(AtomicUsize::new(0));
        let lens = Arc::new(Mutex::new(Vec::new()));
        let transcriber: Transcriber = {
            let (zeros, lens) = (zeros.clone(), lens.clone());
            Arc::new(move |pcm: &[f32]| {
                zeros.fetch_add(pcm.iter().filter(|s| **s == 0.0).count(), Ordering::SeqCst);
                lens.lock().unwrap().push(pcm.len());
                Ok(Vec::new())
            })
        };
        let session = Session::new(
            vec![spec(SourceKind::Mic, None, opened, no_reopen())],
            transcriber,
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        let tick = || {
            // Dois eventos de nível: pelo menos um ciclo inteiro rodou com o relógio atual.
            for _ in 0..2 {
                wait_for(&events, |e| matches!(e, SessionEvent::Level { .. }));
            }
        };
        for _ in 0..6 {
            clock.advance(0.3);
            tick(); // o bloco ainda a caminho
            tx.send(SourceMsg::Pcm(vec![0.1; 4_800])).unwrap();
            tick(); // o bloco chega e é drenado no mesmo instante
        }
        session.stop().unwrap();
        assert_eq!(zeros.load(Ordering::SeqCst), 0);
        assert_eq!(*lens.lock().unwrap(), [28_800]);
    }

    #[test]
    fn level_reports_only_active_sources() {
        let clock = FakeClock::new();
        let (tx, opened, _) = fake_source(None);
        let (sink, events) = sink();
        let session = Session::new(
            vec![spec(SourceKind::Mic, None, opened, no_reopen())],
            fake_transcriber(),
            sink,
            clock.plain_env(),
            clock.now(),
        )
        .unwrap();
        feed(&tx, &clock, 1);
        let e = wait_for(&events, |e| {
            matches!(e, SessionEvent::Level { mic: Some(l), .. } if *l > 0.0)
        });
        assert!(matches!(e, SessionEvent::Level { system: None, .. }), "{e:?}");
        session.stop().unwrap();
    }
}

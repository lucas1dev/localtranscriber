// Gravação: lógica pura (reamostragem, relógio, etiquetas), dispositivos e a
// sessão que liga tudo.

pub mod devices;
#[cfg(target_os = "macos")]
mod mac_tap;
pub mod merge;
pub mod resample;
pub mod session;
pub mod track;

/// Duração de cada bloco enviado ao transcritor.
pub const CHUNK_SECONDS: u32 = 12;
/// Taxa de amostragem comum a todas as trilhas.
pub const SAMPLE_RATE: u32 = 16_000;

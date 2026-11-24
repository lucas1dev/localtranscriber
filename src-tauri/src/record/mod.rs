// Gravação: lógica pura (reamostragem, relógio, etiquetas) e dispositivos cpal.
// Task 4 liga tudo à sessão; até lá, parte do código só é usada nos testes.
#![allow(dead_code)]

pub mod devices;
pub mod merge;
pub mod resample;
pub mod track;

/// Duração de cada bloco enviado ao transcritor.
pub const CHUNK_SECONDS: u32 = 12;
/// Taxa de amostragem comum a todas as trilhas.
pub const SAMPLE_RATE: u32 = 16_000;

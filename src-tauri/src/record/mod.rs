// Lógica pura da gravação (sem dispositivos). Task 4 liga isto à captura nativa;
// até lá, parte do código só é usada nos testes.
#![allow(dead_code)]

pub mod merge;
pub mod resample;
pub mod track;

/// Duração de cada bloco enviado ao transcritor.
pub const CHUNK_SECONDS: u32 = 12;
/// Taxa de amostragem comum a todas as trilhas.
pub const SAMPLE_RATE: u32 = 16_000;

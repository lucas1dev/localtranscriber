use std::time::Instant;

use super::{CHUNK_SECONDS, SAMPLE_RATE};

const CHUNK_SAMPLES: usize = (CHUNK_SECONDS * SAMPLE_RATE) as usize;
/// Atraso tolerado antes de preencher com silêncio (200 ms).
const LAG_TOLERANCE: usize = (SAMPLE_RATE / 5) as usize;
/// Menor bloco final que vale transcrever (0,5 s).
const MIN_FINAL: usize = (SAMPLE_RATE / 2) as usize;

/// Trilha de uma fonte, alinhada a um relógio comum (o `start` da gravação).
pub struct Track {
    start: Instant,
    buf: Vec<f32>,
    samples_total: usize,
    next_index: u32,
}

impl Track {
    pub fn new(start: Instant) -> Self {
        Self { start, buf: Vec::new(), samples_total: 0, next_index: 0 }
    }

    pub fn push(&mut self, pcm16k: &[f32], now: Instant) {
        // `now` é o instante em que o bloco terminou de chegar: ele cobre os últimos
        // `len` instantes, então só o que veio antes disso conta como atraso.
        self.pad_until(now, pcm16k.len());
        self.buf.extend_from_slice(pcm16k);
        self.samples_total += pcm16k.len();
    }

    pub fn pad_to(&mut self, now: Instant) {
        self.pad_until(now, 0);
    }

    fn pad_until(&mut self, now: Instant, reserve: usize) {
        let elapsed = now.saturating_duration_since(self.start);
        let expected = (elapsed.as_secs_f64() * SAMPLE_RATE as f64).round() as usize;
        let expected = expected.saturating_sub(reserve);
        if self.samples_total + LAG_TOLERANCE < expected {
            let missing = expected - self.samples_total;
            self.buf.resize(self.buf.len() + missing, 0.0);
            self.samples_total = expected;
        }
    }

    pub fn take_chunk(&mut self) -> Option<(u32, Vec<f32>)> {
        if self.buf.len() < CHUNK_SAMPLES {
            return None;
        }
        let chunk: Vec<f32> = self.buf.drain(..CHUNK_SAMPLES).collect();
        let index = self.next_index;
        self.next_index += 1;
        Some((index, chunk))
    }

    pub fn finish(self) -> Option<(u32, Vec<f32>)> {
        if self.buf.len() >= MIN_FINAL {
            Some((self.next_index, self.buf))
        } else {
            None
        }
    }
}

/// RMS em dBFS mapeado linearmente de −60..0 para 0..1.
pub fn level(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let mean_sq = pcm.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / pcm.len() as f64;
    let db = 10.0 * mean_sq.log10();
    (((db + 60.0) / 60.0) as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn secs(start: Instant, s: f64) -> Instant {
        start + Duration::from_secs_f64(s)
    }

    #[test]
    fn continuous_30s_gives_two_chunks_and_a_tail() {
        let start = Instant::now();
        let mut t = Track::new(start);
        let mut chunks = Vec::new();
        for i in 1..=30 {
            t.push(&vec![0.1; 16_000], secs(start, i as f64));
            while let Some(c) = t.take_chunk() {
                chunks.push(c);
            }
        }
        assert_eq!(chunks.len(), 2);
        assert_eq!((chunks[0].0, chunks[0].1.len()), (0, 192_000));
        assert_eq!((chunks[1].0, chunks[1].1.len()), (1, 192_000));
        let (idx, tail) = t.finish().unwrap();
        assert_eq!((idx, tail.len()), (2, 96_000));
    }

    #[test]
    fn silent_source_is_padded_with_zeros() {
        let start = Instant::now();
        let mut t = Track::new(start);
        t.push(&vec![0.5; 16_000], secs(start, 1.0));
        t.pad_to(secs(start, 20.0));
        let (idx, chunk) = t.take_chunk().unwrap();
        assert_eq!(idx, 0);
        assert!(chunk[..16_000].iter().all(|s| *s == 0.5));
        assert!(chunk[16_000..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn small_jitter_does_not_pad() {
        let start = Instant::now();
        let mut t = Track::new(start);
        t.push(&vec![0.1; 16_000], secs(start, 1.15));
        assert_eq!(t.samples_total, 16_000);
        assert_eq!(t.buf.len(), 16_000);
    }

    #[test]
    fn short_tail_is_dropped() {
        let start = Instant::now();
        let mut t = Track::new(start);
        t.push(&vec![0.1; 4_800], secs(start, 0.3));
        assert!(t.finish().is_none());
    }

    #[test]
    fn level_maps_dbfs_to_unit_range() {
        assert_eq!(level(&[0.0; 1600]), 0.0);
        assert_eq!(level(&[]), 0.0);
        let sine: Vec<f32> = (0..16_000)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin())
            .collect();
        let l = level(&sine);
        assert!((0.94..=1.0).contains(&l), "{l}");
        // Contínuo de amplitude 10^(-30/20) tem RMS de -30 dBFS.
        let a = 10f32.powf(-30.0 / 20.0);
        let l = level(&vec![a; 1600]);
        assert!((l - 0.5).abs() < 0.05, "{l}");
    }
}

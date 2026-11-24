use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use super::SAMPLE_RATE;
use crate::error::AppError;

const CHUNK_FRAMES: usize = 1024;

/// Converte áudio intercalado de qualquer taxa/canais para mono 16 kHz.
pub struct ToMono16k {
    channels: usize,
    in_rate: u32,
    resampler: Fft<f32>,
    pending: Vec<f32>,
    out_buf: Vec<f32>,
    delay_left: usize,
    in_total: u64,
    out_total: u64,
}

impl ToMono16k {
    pub fn new(in_rate: u32, channels: u16) -> Result<Self, AppError> {
        if in_rate == 0 || channels == 0 {
            return Err(AppError::Internal("Formato de áudio inválido.".into()));
        }
        let resampler = Fft::<f32>::new(
            in_rate as usize,
            SAMPLE_RATE as usize,
            CHUNK_FRAMES,
            1,
            FixedSync::Input,
        )
        .map_err(|e| AppError::Internal(format!("Falha ao criar o reamostrador: {e}")))?;
        let delay_left = resampler.output_delay();
        let out_buf = vec![0.0; resampler.output_frames_max()];
        Ok(Self {
            channels: channels as usize,
            in_rate,
            resampler,
            pending: Vec::new(),
            out_buf,
            delay_left,
            in_total: 0,
            out_total: 0,
        })
    }

    pub fn push(&mut self, interleaved: &[f32]) -> Vec<f32> {
        let ch = self.channels;
        let frames = interleaved.len() / ch;
        self.in_total += frames as u64;
        self.pending.extend(
            interleaved
                .chunks_exact(ch)
                .map(|f| f.iter().sum::<f32>() / ch as f32),
        );
        let mut out = Vec::new();
        while self.pending.len() >= self.resampler.input_frames_next() {
            let n = self.run(None);
            self.collect(n, &mut out);
        }
        out
    }

    pub fn flush(&mut self) -> Vec<f32> {
        let expected = (self.in_total * SAMPLE_RATE as u64 + self.in_rate as u64 / 2)
            / self.in_rate as u64;
        let mut out = Vec::new();
        // O FFT tem latência: alimenta silêncio até sair toda a duração real da entrada.
        for _ in 0..8 {
            if self.out_total >= expected {
                break;
            }
            let len = self.pending.len();
            let n = self.run(Some(len));
            self.collect(n, &mut out);
        }
        let overshoot = self.out_total.saturating_sub(expected) as usize;
        out.truncate(out.len().saturating_sub(overshoot));
        self.out_total = self.out_total.min(expected);
        out
    }

    /// Processa um bloco do início de `pending` e o consome; devolve as amostras de saída em `out_buf`.
    fn run(&mut self, partial: Option<usize>) -> usize {
        let need = self.resampler.input_frames_next();
        let input = InterleavedSlice::new(&self.pending, 1, self.pending.len()).unwrap();
        let cap = self.out_buf.len();
        let mut output = InterleavedSlice::new_mut(&mut self.out_buf, 1, cap).unwrap();
        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: partial,
            active_channels_mask: None,
        };
        let (nin, nout) = self
            .resampler
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .expect("tamanhos de buffer validados");
        let consumed = if partial.is_some() { self.pending.len() } else { nin.min(need) };
        self.pending.drain(..consumed);
        nout
    }

    fn collect(&mut self, nout: usize, out: &mut Vec<f32>) {
        let skip = self.delay_left.min(nout);
        self.delay_left -= skip;
        out.extend_from_slice(&self.out_buf[skip..nout]);
        self.out_total += (nout - skip) as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, channels: usize, secs: f32, hz: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        let mut v = Vec::with_capacity(n * channels);
        for i in 0..n {
            let s = (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin() * 0.5;
            for _ in 0..channels {
                v.push(s);
            }
        }
        v
    }

    fn within(actual: usize, expected: usize, pct: f64) -> bool {
        (actual as f64 - expected as f64).abs() <= expected as f64 * pct / 100.0
    }

    fn crossings(v: &[f32]) -> usize {
        v.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0 || w[0] >= 0.0 && w[1] < 0.0).count()
    }

    fn run_all(r: &mut ToMono16k, input: &[f32], step: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for c in input.chunks(step) {
            out.extend(r.push(c));
        }
        out.extend(r.flush());
        out
    }

    #[test]
    fn stereo_48k_becomes_mono_16k_keeping_frequency() {
        let mut r = ToMono16k::new(48_000, 2).unwrap();
        let out = run_all(&mut r, &sine(48_000, 2, 1.0, 440.0), 4800);
        assert!(within(out.len(), 16_000, 1.0), "len {}", out.len());
        assert!(within(crossings(&out), 880, 2.0), "cruzamentos {}", crossings(&out));
    }

    #[test]
    fn mono_44k1_becomes_16k() {
        let mut r = ToMono16k::new(44_100, 1).unwrap();
        let out = run_all(&mut r, &sine(44_100, 1, 1.0, 440.0), 4410);
        assert!(within(out.len(), 16_000, 1.0), "len {}", out.len());
    }

    #[test]
    fn arbitrary_chunk_sizes_give_same_total() {
        let input = sine(48_000, 1, 1.0, 440.0);
        for step in [1usize, 7, 1000] {
            let mut r = ToMono16k::new(48_000, 1).unwrap();
            let out = run_all(&mut r, &input, step);
            assert!(within(out.len(), 16_000, 1.0), "step {step}: len {}", out.len());
        }
    }

    #[test]
    fn rejects_invalid_format() {
        assert!(ToMono16k::new(0, 1).is_err());
        assert!(ToMono16k::new(48_000, 0).is_err());
    }
}

//! FFT spectrum with windowing and averaging, in dBFS.
//!
//! Used for the main spectrum/waterfall. Each call takes the newest `size` IQ samples,
//! multiplies them by a Blackman-Harris window, runs an in-place FFT and converts each bin to
//! dB. The result is reordered ("FFT shift") so index 0 is the lowest frequency
//! (centre − rate/2) and index size/2 is the tuned centre frequency.

use super::C32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub struct SpectrumProcessor {
    /// FFT length (number of bins).
    pub size: usize,
    /// Planned FFT (rustfft picks the algorithm for this size once).
    fft: Arc<dyn Fft<f32>>,
    /// Window coefficients, scaled to sum 1 so a full-scale tone reads 0 dBFS.
    window: Vec<f32>,
    /// Work buffer: windowed input, transformed in place into bins (natural FFT order).
    buf: Vec<C32>,
    /// Scratch space rustfft needs for in-place transforms.
    scratch: Vec<C32>,
    /// Output in dB, FFT-shifted, exponentially averaged across calls.
    avg: Vec<f32>,
    /// Averaging length: each new frame contributes 1/averaging of the result.
    pub averaging: usize,
    /// False until the first frame, which initialises `avg` directly.
    have_avg: bool,
}

impl SpectrumProcessor {
    pub fn new(size: usize, averaging: usize) -> Self {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(size);
        // Blackman-Harris window, normalised to unity coherent gain
        let n = size as f64;
        let mut window: Vec<f32> = (0..size)
            .map(|i| {
                let x = 2.0 * std::f64::consts::PI * i as f64 / (n - 1.0);
                (0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos()) as f32
            })
            .collect();
        let sum: f32 = window.iter().sum();
        window.iter_mut().for_each(|w| *w /= sum);
        let scratch = vec![C32::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        Self { size, fft, window, buf: vec![C32::new(0.0, 0.0); size], scratch, avg: vec![-150.0; size], averaging: averaging.max(1), have_avg: false }
    }

    /// Computes the spectrum of the last `size` samples; output is FFT-shifted (lowest frequency first), in dB.
    pub fn compute(&mut self, samples: &[C32]) -> &[f32] {
        let n = self.size;
        // take the newest n samples (or fewer, zero-padded) and apply the window
        let start = samples.len().saturating_sub(n);
        let s = &samples[start..];
        for i in 0..n {
            self.buf[i] = if i < s.len() { s[i] * self.window[i] } else { C32::new(0.0, 0.0) };
        }
        self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);
        let alpha = 1.0 / self.averaging as f32;
        for i in 0..n {
            // FFT shift: output i comes from bin j. Bins n/2..n are the negative frequencies,
            // so they go first. norm_sqr = re²+im² is power; clamp avoids log10(0).
            let j = (i + n / 2) % n;
            let p = self.buf[j].norm_sqr().max(1e-20);
            let db = 10.0 * p.log10();
            if self.have_avg && self.averaging > 1 {
                // average in the power domain (averaging dB values would bias noise low):
                // convert back to power, move 1/averaging of the way to the new value, back to dB
                let old = 10f32.powf(self.avg[i] / 10.0);
                self.avg[i] = 10.0 * (old + alpha * (p - old)).max(1e-20).log10();
            } else {
                self.avg[i] = db;
            }
        }
        self.have_avg = true;
        &self.avg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_scale_tone_is_0_db() {
        let n = 1024;
        let mut sp = SpectrumProcessor::new(n, 1);
        let f_bin = 100;
        let s: Vec<C32> = (0..n)
            .map(|i| {
                let ph = 2.0 * std::f64::consts::PI * f_bin as f64 * i as f64 / n as f64;
                C32::new(ph.cos() as f32, ph.sin() as f32)
            })
            .collect();
        let out = sp.compute(&s).to_vec();
        let (imax, vmax) = out.iter().enumerate().fold((0, -999.0f32), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
        assert_eq!(imax, n / 2 + f_bin);
        assert!(vmax.abs() < 0.5, "peak {vmax}");
    }
}

//! Channel selection: frequency shift, multistage integer decimation and a fractional resampler.
#![allow(dead_code)]

use super::filters::{design_lowpass, DecimFirC};
use super::C32;
use std::f64::consts::PI;

/// Complex oscillator that shifts a signal by `-offset_hz` (moves `offset_hz` to 0 Hz).
pub struct FreqShifter {
    phase: C32,
    step: C32,
    count: u32,
}

impl FreqShifter {
    pub fn new(sample_rate: f64, offset_hz: f64) -> Self {
        let w = -2.0 * PI * offset_hz / sample_rate;
        Self { phase: C32::new(1.0, 0.0), step: C32::new(w.cos() as f32, w.sin() as f32), count: 0 }
    }
    pub fn set(&mut self, sample_rate: f64, offset_hz: f64) {
        let w = -2.0 * PI * offset_hz / sample_rate;
        self.step = C32::new(w.cos() as f32, w.sin() as f32);
    }
    pub fn is_zero(&self) -> bool {
        self.step.im == 0.0 && self.step.re == 1.0
    }
    pub fn process_in_place(&mut self, buf: &mut [C32]) {
        if self.is_zero() {
            return;
        }
        for x in buf.iter_mut() {
            *x *= self.phase;
            self.phase *= self.step;
            self.count += 1;
            if self.count >= 1024 {
                self.count = 0;
                let n = self.phase.norm();
                self.phase /= n;
            }
        }
    }
}

/// Fractional resampler (polyphase windowed sinc with linear interpolation between phases).
/// Ratio out/in must be <= 1 (downsampling) or slightly above (fine adjustment).
pub struct FracResampler {
    phases: usize,
    taps_per_phase: usize,
    table: Vec<f32>, // (phases+1) x taps_per_phase
    hist: Vec<C32>,  // doubled history
    pos: usize,
    /// input samples per output sample
    step: f64,
    t: f64, // fractional time of the next output relative to the newest input
}

impl FracResampler {
    pub fn new(in_rate: f64, out_rate: f64) -> Self {
        let phases = 128;
        let taps_per_phase = 24;
        let cutoff = (out_rate.min(in_rate) / in_rate) * 0.5 * 0.92; // cycles per input sample
        let total = phases * taps_per_phase;
        let mut proto = vec![0f64; total + phases];
        let m = (taps_per_phase as f64) / 2.0;
        for (k, p) in proto.iter_mut().enumerate() {
            // position in input samples
            let x = k as f64 / phases as f64 - m;
            let sinc = if x.abs() < 1e-12 { 2.0 * cutoff } else { (2.0 * PI * cutoff * x).sin() / (PI * x) };
            let wx = (k as f64) / (total as f64);
            let w = if wx <= 1.0 { 0.42 - 0.5 * (2.0 * PI * wx).cos() + 0.08 * (4.0 * PI * wx).cos() } else { 0.0 };
            *p = sinc * w;
        }
        // table[ph][j] = proto[j*phases + ph]
        let mut table = vec![0f32; (phases + 1) * taps_per_phase];
        for ph in 0..=phases {
            let mut sum = 0.0;
            for j in 0..taps_per_phase {
                let idx = j * phases + ph;
                sum += proto.get(idx).copied().unwrap_or(0.0);
            }
            for j in 0..taps_per_phase {
                let idx = j * phases + ph;
                table[ph * taps_per_phase + j] = (proto.get(idx).copied().unwrap_or(0.0) / sum.max(1e-9)) as f32;
            }
        }
        Self {
            phases,
            taps_per_phase,
            table,
            hist: vec![C32::new(0.0, 0.0); 2 * taps_per_phase],
            pos: 0,
            step: in_rate / out_rate,
            t: 0.0,
        }
    }

    pub fn set_ratio(&mut self, in_rate: f64, out_rate: f64) {
        self.step = in_rate / out_rate;
    }

    pub fn process(&mut self, input: &[C32], out: &mut Vec<C32>) {
        let n = self.taps_per_phase;
        for &x in input {
            self.hist[self.pos] = x;
            self.hist[self.pos + n] = x;
            self.pos += 1;
            if self.pos == n {
                self.pos = 0;
            }
            self.t -= 1.0;
            while self.t < 0.0 {
                // output at fractional delay `frac` behind the newest sample; the table phase is 1-frac
                let frac = (-self.t).clamp(0.0, 0.999_999);
                let fp = (1.0 - frac) * self.phases as f64;
                let ph = (fp as usize).min(self.phases - 1);
                let mu = (fp - ph as f64) as f32;
                let t0 = &self.table[ph * n..ph * n + n];
                let t1 = &self.table[(ph + 1) * n..(ph + 1) * n + n];
                let window = &self.hist[self.pos..self.pos + n];
                let mut acc = C32::new(0.0, 0.0);
                // window is oldest first; tap j of the prototype applies to sample (newest - j - frac)
                for j in 0..n {
                    let c = t0[j] + (t1[j] - t0[j]) * mu;
                    acc += window[n - 1 - j] * c;
                }
                out.push(acc);
                self.t += self.step;
            }
        }
    }
}

/// From the source rate to a demodulator rate: integer stages, then a fractional stage.
pub struct Channelizer {
    pub in_rate: f64,
    pub out_rate: f64,
    shifter: FreqShifter,
    stages: Vec<DecimFirC>,
    frac: Option<FracResampler>,
    buf_a: Vec<C32>,
    buf_b: Vec<C32>,
    shifted: Vec<C32>,
}

impl Channelizer {
    /// `pass_hz`: one-sided bandwidth that must survive (≤ 0.45 · out_rate).
    pub fn new(in_rate: f64, out_rate: f64, pass_hz: f64) -> Self {
        let mut stages = Vec::new();
        let mut rate = in_rate;
        let pass = pass_hz.min(out_rate * 0.45);
        // integer stages while there is at least a factor 2 left above the output rate
        loop {
            let ratio = rate / out_rate;
            if ratio < 2.0 {
                break;
            }
            let factor = if ratio >= 16.0 {
                8
            } else if ratio >= 8.0 {
                4
            } else if ratio >= 4.0 {
                2
            } else if ratio >= 3.0 {
                3
            } else {
                2
            };
            let next = rate / factor as f64;
            // aliases fold back into [0, pass] from next - pass
            let transition = (next - 2.0 * pass).max(next * 0.05);
            let is_last = next / out_rate < 2.0;
            let taps = if is_last {
                // last integer stage also protects the fractional stage: stop at next/2
                design_lowpass(rate, pass, (next / 2.0 - pass).max(next * 0.05) * 1.6, 1023)
            } else {
                design_lowpass(rate, pass, transition, 511)
            };
            stages.push(DecimFirC::new(taps, factor));
            rate = next;
        }
        let frac = if (rate - out_rate).abs() > 0.5 { Some(FracResampler::new(rate, out_rate)) } else { None };
        Self {
            in_rate,
            out_rate,
            shifter: FreqShifter::new(in_rate, 0.0),
            stages,
            frac,
            buf_a: Vec::new(),
            buf_b: Vec::new(),
            shifted: Vec::new(),
        }
    }

    pub fn set_offset(&mut self, offset_hz: f64) {
        self.shifter.set(self.in_rate, offset_hz);
    }

    pub fn process(&mut self, input: &[C32], out: &mut Vec<C32>) {
        self.shifted.clear();
        self.shifted.extend_from_slice(input);
        self.shifter.process_in_place(&mut self.shifted);
        let mut cur = std::mem::take(&mut self.shifted);
        for st in self.stages.iter_mut() {
            self.buf_a.clear();
            st.process(&cur, &mut self.buf_a);
            std::mem::swap(&mut cur, &mut self.buf_a);
        }
        if let Some(fr) = self.frac.as_mut() {
            fr.process(&cur, out);
        } else {
            out.extend_from_slice(&cur);
        }
        self.shifted = cur;
        let _ = &self.buf_b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: f64, f: f64, n: usize) -> Vec<C32> {
        (0..n)
            .map(|i| {
                let ph = 2.0 * PI * f * i as f64 / rate;
                C32::new(ph.cos() as f32, ph.sin() as f32)
            })
            .collect()
    }

    fn power(x: &[C32]) -> f32 {
        x.iter().map(|c| c.norm_sqr()).sum::<f32>() / x.len() as f32
    }

    #[test]
    fn channelizer_selects_offset_tone() {
        let rate = 2_400_000.0;
        let mut ch = Channelizer::new(rate, 192_000.0, 90_000.0);
        ch.set_offset(300_000.0);
        let input = tone(rate, 300_010.0, 240_000);
        let mut out = Vec::new();
        ch.process(&input, &mut out);
        assert!((out.len() as i64 - 19_200).abs() < 50, "len {}", out.len());
        let p = power(&out[2000..]);
        assert!(p > 0.8 && p < 1.2, "power {p}");
        // a tone far away is rejected
        let mut ch2 = Channelizer::new(rate, 192_000.0, 90_000.0);
        ch2.set_offset(0.0);
        let mut out2 = Vec::new();
        ch2.process(&tone(rate, 500_000.0, 240_000), &mut out2);
        assert!(power(&out2[2000..]) < 1e-4, "leak {}", power(&out2[2000..]));
    }

    #[test]
    fn hackrf_rate_to_wfm() {
        let rate = 10_000_000.0;
        let mut ch = Channelizer::new(rate, 192_000.0, 90_000.0);
        let input = tone(rate, 50_000.0, 1_000_000);
        let mut out = Vec::new();
        ch.process(&input, &mut out);
        assert!((out.len() as i64 - 19_200).abs() < 50);
        let p = power(&out[2000..]);
        assert!(p > 0.8 && p < 1.2, "power {p}");
    }
}

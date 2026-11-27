//! Channel selection: frequency shift, multistage integer decimation and a fractional resampler.
//!
//! The SDR delivers IQ at a high rate (2.4 Msps for an RTL-SDR, up to 10 Msps for a HackRF).
//! A demodulator only wants a narrow slice around the station, at a convenient rate
//! (e.g. 192 kHz for WFM). [`Channelizer`] gets there in three steps:
//!
//! 1. [`FreqShifter`] multiplies by a complex exponential so the wanted station sits at 0 Hz.
//! 2. A chain of [`DecimFirC`] stages low-pass filters and keeps every 2nd/3rd/4th/8th sample,
//!    until the rate is less than twice the output rate. Integer stages are cheap because the
//!    filter only runs for the samples that are kept.
//! 3. [`FracResampler`] covers the remaining non-integer ratio (e.g. 300 kHz -> 192 kHz).
#![allow(dead_code)]

use super::filters::{design_lowpass, DecimFirC};
use super::C32;
use std::f64::consts::PI;

/// Complex oscillator that shifts a signal by `-offset_hz` (moves `offset_hz` to 0 Hz).
///
/// Instead of calling sin/cos for every sample, the oscillator keeps the current phasor
/// `phase` (a unit-length complex number) and multiplies it by the constant rotation `step`
/// (`e^{-j·2π·offset/rate}`) once per sample. Float rounding makes `|phase|` drift away from 1
/// slowly, so it is renormalised every 1024 samples (`count`).
pub struct FreqShifter {
    /// Current oscillator value e^{jθ}; the next input sample is multiplied by it.
    phase: C32,
    /// Per-sample rotation e^{-jω}. 1+0j means "no shift" and lets processing be skipped.
    step: C32,
    /// Samples since the last renormalisation of `phase`.
    count: u32,
}

impl FreqShifter {
    pub fn new(sample_rate: f64, offset_hz: f64) -> Self {
        // ω in radians per sample; negative so that +offset_hz moves down to 0 Hz.
        let w = -2.0 * PI * offset_hz / sample_rate;
        Self { phase: C32::new(1.0, 0.0), step: C32::new(w.cos() as f32, w.sin() as f32), count: 0 }
    }
    /// Retune: only the rotation changes; the running phase is kept so there is no click.
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
            // mix the sample down, then advance the oscillator by one sample
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
///
/// Idea: an output sample falls *between* two input samples, at some fraction of a sample.
/// The ideal value there is the input convolved with a sinc centred on that point. We
/// precompute the sinc (windowed, low-passed) at `phases` = 128 sub-sample positions, so each
/// output needs only a 24-tap dot product, with linear interpolation between the two nearest
/// precomputed positions for the remaining fraction.
///
/// Data layout:
/// * `table` is a flattened 2-D array of `(phases + 1)` rows × `taps_per_phase` columns.
///   Row `ph` holds the 24 filter taps for a sub-sample offset of `ph / phases`. Row `phases`
///   (one extra) exists so `ph + 1` is always a valid row when interpolating.
///   Element (ph, j) is at `table[ph * taps_per_phase + j]`.
/// * `hist` is the same doubled ring buffer used in `filters.rs`: each input is written at
///   `pos` and at `pos + n`, so the last `n` samples are always the contiguous slice
///   `hist[pos .. pos + n]`, oldest first, without wrap-around logic.
pub struct FracResampler {
    /// Number of sub-sample positions the prototype filter is tabulated at (128).
    phases: usize,
    /// Filter length in input samples (24).
    taps_per_phase: usize,
    /// (phases+1) x taps_per_phase coefficients, row-major (see above).
    table: Vec<f32>,
    /// Doubled history of the last `taps_per_phase` inputs (length 2·n).
    hist: Vec<C32>,
    /// Next write position in `hist`, 0..n.
    pos: usize,
    /// input samples per output sample
    step: f64,
    /// Time of the next output, in input samples, relative to the newest input.
    /// Negative means "the next output is already due, |t| samples behind the newest input".
    t: f64,
}

impl FracResampler {
    pub fn new(in_rate: f64, out_rate: f64) -> Self {
        let phases = 128;
        let taps_per_phase = 24;
        let cutoff = (out_rate.min(in_rate) / in_rate) * 0.5 * 0.92; // cycles per input sample
        // The prototype is one long sinc filter sampled `phases` times per input sample:
        // proto[k] is the filter value at x = k/phases - m input samples from its centre.
        // `phases` extra entries at the end (all zero after the window) back the extra table row.
        let total = phases * taps_per_phase;
        let mut proto = vec![0f64; total + phases];
        let m = (taps_per_phase as f64) / 2.0;
        for (k, p) in proto.iter_mut().enumerate() {
            // position in input samples
            let x = k as f64 / phases as f64 - m;
            // ideal low-pass impulse response sin(2π·fc·x)/(π·x), with its limit 2·fc at x = 0
            let sinc = if x.abs() < 1e-12 { 2.0 * cutoff } else { (2.0 * PI * cutoff * x).sin() / (PI * x) };
            // Blackman window over the whole prototype length (0 beyond it)
            let wx = (k as f64) / (total as f64);
            let w = if wx <= 1.0 { 0.42 - 0.5 * (2.0 * PI * wx).cos() + 0.08 * (4.0 * PI * wx).cos() } else { 0.0 };
            *p = sinc * w;
        }
        // table[ph][j] = proto[j*phases + ph]: pick every `phases`-th prototype value starting
        // at offset ph. That gives the 24 taps for one sub-sample delay ("polyphase split").
        // Each row is normalised to sum 1 so every phase has unity DC gain (no amplitude
        // ripple as the fractional position moves).
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
            // push x into the doubled ring buffer (see struct docs)
            self.hist[self.pos] = x;
            self.hist[self.pos + n] = x;
            self.pos += 1;
            if self.pos == n {
                self.pos = 0;
            }
            // one input sample has passed, so the next output is one sample closer
            self.t -= 1.0;
            // emit every output that is now due (0 for decimation steps that skip, sometimes
            // 2 when upsampling slightly); each output moves the next one `step` samples on
            while self.t < 0.0 {
                // output at fractional delay `frac` behind the newest sample; the table phase is 1-frac
                let frac = (-self.t).clamp(0.0, 0.999_999);
                // fp: position in table rows (fractional). ph: the row below, mu: 0..1 weight
                // of the row above, for linear interpolation between rows ph and ph+1.
                let fp = (1.0 - frac) * self.phases as f64;
                let ph = (fp as usize).min(self.phases - 1);
                let mu = (fp - ph as f64) as f32;
                // borrow the two neighbouring rows of the table and the last n inputs
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
///
/// Buffers are kept in the struct and reused on every call, so steady-state processing does
/// no heap allocation: `shifted` and `buf_a` are ping-ponged between stages with `mem::swap`.
pub struct Channelizer {
    pub in_rate: f64,
    pub out_rate: f64,
    /// Moves the wanted station to 0 Hz before any filtering.
    shifter: FreqShifter,
    /// Integer decimators, applied in order (largest factor first).
    stages: Vec<DecimFirC>,
    /// Final non-integer step; None when the integer stages already land on `out_rate`.
    frac: Option<FracResampler>,
    /// Scratch buffer for a stage's output.
    buf_a: Vec<C32>,
    /// Unused spare (kept so the struct layout matches older code).
    buf_b: Vec<C32>,
    /// Shifted copy of the input; afterwards holds whichever buffer was last in use.
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
            // Big factors first (fewest samples for later stages). A stage of 4 is used for
            // ratio 8..16 rather than 8, so the following stages still have room to filter.
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
            // After decimating to `next`, energy at f folds to next - f. Anything below
            // next - pass stays clear of the passband, so the filter may roll off
            // between pass and next - pass (at least 5 % of `next` for a sane length).
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
        // only add the fractional stage if the integer stages didn't already land exactly
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
        // `cur` owns the current signal. Each stage reads `cur`, writes into `buf_a`, and the
        // two Vecs swap places (a pointer swap, no copy) so `cur` again holds the newest data.
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
        // give the allocation back so it is reused next call
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

//! Broadcast FM at 192 kHz complex baseband to stereo audio at 48 kHz (port of the Android fork's
//! MpxDemodulator): FM discriminator -> MPX, 19 kHz pilot PLL, 38 kHz L-R decode, 15 kHz audio
//! low-pass, de-emphasis. The MPX of the last call stays available for RDS and the MPX view.
//! MPX units: 1.0 = 75 kHz deviation.
//!
//! The MPX ("multiplex") signal is what the FM transmitter modulates onto the carrier:
//!
//! | Band          | Content                                   |
//! |---------------|-------------------------------------------|
//! | 0–15 kHz      | L+R (mono-compatible sum)                 |
//! | 19 kHz        | pilot tone (tells receivers it's stereo)  |
//! | 23–53 kHz     | L−R, AM double-sideband on 38 kHz (2× pilot), carrier suppressed |
//! | 57 kHz        | RDS (3× pilot), decoded in `rds.rs` from `mpx` |
//!
//! Processing per call to [`MpxDemodulator::process`]:
//! 1. Discriminator: the phase change between consecutive IQ samples is the instantaneous
//!    frequency, scaled so ±75 kHz deviation = ±1.0. Results are kept in `self.mpx`.
//! 2. Pilot PLL: an oscillator (phase in cycles) is mixed with the MPX; a two-pole low-pass on
//!    the I/Q products isolates the pilot, and its angle is the phase error that steers the
//!    oscillator (proportional-integral loop, ~4 Hz bandwidth).
//! 3. The locked oscillator gives the 38 kHz carrier via the double-angle identity
//!    (2·sin·cos = sin 2θ), with no second oscillator. MPX × carrier shifts L−R down to baseband.
//! 4. Both the sum (MPX) and the difference (MPX × carrier) go through the same 16 kHz
//!    low-pass and are decimated to 48 kHz. L = sum + diff, R = sum − diff.
//! 5. One-pole de-emphasis (50 µs in Europe) undoes the transmitter's treble boost.

use super::filters::{lowpass_taps, DecimFirR};
use super::sincos::SinCos;
use super::C32;
use std::f64::consts::PI;

pub const MPX_RATE: u32 = 192_000;
pub const PILOT_HZ: f64 = 19_000.0;
pub const MAX_DEVIATION_HZ: f64 = 75_000.0;
const PILOT_LOCK_MIN_LEVEL: f32 = 0.02;

pub struct MpxDemodulator {
    input_rate: f64,
    /// User switch: when false, output is mono even if a pilot is present.
    pub stereo_enabled: bool,
    deemphasis_us: f64,
    /// MPX samples from the last `process` call (one per input sample, at input_rate).
    /// The engine hands this slice to the RDS decoder and the MPX spectrum view.
    pub mpx: Vec<f32>,
    /// Last IQ sample of the previous call, so the discriminator is continuous across calls.
    last: C32,
    /// rate / (2π·75 kHz): converts radians per sample into MPX units.
    disc_gain: f32,
    /// PLL oscillator phase in cycles, kept in [0,1).
    pll_phase: f64,
    /// 19 kHz expressed in cycles per sample.
    pll_nominal: f64,
    /// Current PLL frequency in cycles per sample (the loop's integrator).
    pll_freq: f64,
    /// Allowed pull range around nominal (±30 Hz), in cycles per sample.
    pll_max_offset: f64,
    /// Proportional and integral gains (critically damped 2nd-order loop, ωn = 2π·4 Hz).
    pll_kp: f64,
    pll_ki: f64,
    /// Coefficient of the 150 Hz one-pole low-pass used on the pilot I/Q products.
    iq_alpha: f32,
    /// Pilot I/Q after the first (pi1/pq1) and second (pi2/pq2) low-pass pole.
    pi1: f32,
    pq1: f32,
    pi2: f32,
    pq2: f32,
    /// Slow (2 Hz) average of |phase error|; small means locked.
    err_avg: f64,
    err_alpha: f64,
    /// 16 kHz low-pass + decimate-to-48 kHz filters for L+R and L−R.
    sum_fir: DecimFirR,
    diff_fir: DecimFirR,
    /// One-pole de-emphasis coefficient and per-channel filter states.
    de_alpha: f32,
    de_l: f32,
    de_r: f32,
    /// Pilot amplitude in MPX units (≈0.08–0.10 for a normal station), updated every call.
    pub pilot_level: f32,
    pub pilot_locked: bool,
    /// Largest |MPX| since the last `take_peak_deviation_hz` (for the deviation meter).
    peak_abs_mpx: f32,
    sc: &'static SinCos,
}

impl MpxDemodulator {
    pub fn new(input_rate: u32) -> Self {
        let r = input_rate as f64;
        let decimation = (input_rate / 48_000) as usize;
        let taps = lowpass_taps(191, r, 16_000.0);
        // loop natural frequency 4 Hz in radians per sample; kp = 2ζωn, ki = ωn² with ζ = 0.707
        let wn = 2.0 * PI * 4.0 / r;
        let mut s = Self {
            input_rate: r,
            stereo_enabled: true,
            deemphasis_us: 50.0,
            mpx: Vec::new(),
            last: C32::new(0.0, 0.0),
            disc_gain: (r / (2.0 * PI * MAX_DEVIATION_HZ)) as f32,
            pll_phase: 0.0,
            pll_nominal: PILOT_HZ / r,
            pll_freq: PILOT_HZ / r,
            pll_max_offset: 30.0 / r,
            pll_kp: 2.0 * 0.707 * wn,
            pll_ki: wn * wn,
            iq_alpha: (1.0 - (-2.0 * PI * 150.0 / r).exp()) as f32,
            pi1: 0.0,
            pq1: 0.0,
            pi2: 0.0,
            pq2: 0.0,
            err_avg: 1.0,
            err_alpha: 1.0 - (-2.0 * PI * 2.0 / r).exp(),
            sum_fir: DecimFirR::new(taps.clone(), decimation),
            diff_fir: DecimFirR::new(taps, decimation),
            de_alpha: 1.0,
            de_l: 0.0,
            de_r: 0.0,
            pilot_level: 0.0,
            pilot_locked: false,
            peak_abs_mpx: 0.0,
            sc: SinCos::get(),
        };
        s.set_deemphasis(50.0);
        s
    }

    pub fn set_deemphasis(&mut self, us: f64) {
        self.deemphasis_us = us;
        // one-pole RC low-pass at 48 kHz: alpha = 1 - e^{-T/τ}; alpha 1 = pass-through
        self.de_alpha = if us <= 0.0 { 1.0 } else { (1.0 - (-1.0 / (48_000.0 * us * 1e-6)).exp()) as f32 };
    }

    pub fn is_stereo(&self) -> bool {
        self.stereo_enabled && self.pilot_locked
    }

    pub fn take_peak_deviation_hz(&mut self) -> f32 {
        let v = self.peak_abs_mpx * MAX_DEVIATION_HZ as f32;
        self.peak_abs_mpx = 0.0;
        v
    }

    pub fn pilot_level_db(&self) -> f32 {
        20.0 * self.pilot_level.max(1e-6).log10()
    }

    pub fn pll_freq_hz(&self) -> f64 {
        self.pll_freq * self.input_rate
    }

    /// Demodulates `input`, appends stereo audio at 48 kHz to `out`.
    pub fn process(&mut self, input: &[C32], out: &mut Vec<[f32; 2]>) {
        let n = input.len();
        self.mpx.clear();
        self.mpx.reserve(n);
        let mut p = self.last;
        let mut peak = self.peak_abs_mpx;
        for &x in input {
            // x · conj(previous) has the angle (θ_now − θ_prev): the phase step, i.e. the
            // instantaneous frequency. atan2 gives it in −π..π; disc_gain scales to MPX units.
            let d = x * p.conj();
            let v = self.disc_gain * d.im.atan2(d.re);
            self.mpx.push(v);
            peak = peak.max(v.abs());
            p = x;
        }
        self.last = p;
        self.peak_abs_mpx = peak;

        // copy loop state into locals for the hot loop; written back afterwards
        let stereo_wanted = self.stereo_enabled;
        let mut phase = self.pll_phase;
        let mut freq = self.pll_freq;
        let alpha = self.iq_alpha;
        let sc = self.sc;
        for i in 0..n {
            let x = self.mpx[i];
            let c = sc.cos(phase);
            let s = sc.sin(phase);
            // 38 kHz carrier: -4·sin·cos = -2·sin(2θ), phase-locked to the pilot. The sign and
            // the factor 2 (mixing halves the amplitude) make the low-passed MPX·sub equal to
            // the transmitted (L−R)/2, matching the sum's (L+R)/2, so L = sum + diff and
            // R = sum − diff below.
            let sub = -4.0 * s * c;
            // mix the MPX with the oscillator (I = x·cos, Q = −x·sin) and low-pass twice;
            // only the pilot survives as a slowly rotating I/Q vector
            self.pi1 += alpha * (x * c - self.pi1);
            self.pq1 += alpha * (-x * s - self.pq1);
            self.pi2 += alpha * (self.pi1 - self.pi2);
            self.pq2 += alpha * (self.pq1 - self.pq2);
            // the pilot vector's angle is the phase error, converted to cycles
            let err = (self.pq2.atan2(self.pi2) as f64) / (2.0 * PI);
            self.err_avg += self.err_alpha * (err.abs() - self.err_avg);
            // PI loop: the integrator nudges the frequency (within ±30 Hz), the proportional
            // term nudges the phase directly
            freq += self.pll_ki * err;
            freq = freq.clamp(self.pll_nominal - self.pll_max_offset, self.pll_nominal + self.pll_max_offset);
            phase += freq + self.pll_kp * err;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            // both FIRs decimate by the same factor, so they become ready on the same sample
            let sum_ready = self.sum_fir.push(x);
            let diff_ready = self.diff_fir.push(x * sub);
            if sum_ready && diff_ready {
                let sum = self.sum_fir.output;
                // no lock (or stereo off): diff = 0 gives mono, avoiding hiss from L−R noise
                let diff = if stereo_wanted && self.pilot_locked { self.diff_fir.output } else { 0.0 };
                // matrix to L/R and de-emphasise each (one-pole IIR: y += α·(x − y))
                self.de_l += self.de_alpha * (sum + diff - self.de_l);
                self.de_r += self.de_alpha * (sum - diff - self.de_r);
                out.push([self.de_l, self.de_r]);
            }
        }
        self.pll_phase = phase;
        self.pll_freq = freq;
        // mixing halves the amplitude, so 2·|I/Q| is the pilot amplitude
        let level = 2.0 * (self.pi2 * self.pi2 + self.pq2 * self.pq2).sqrt();
        self.pilot_level = level;
        // hysteresis: harder to get lock (error < 0.06 cycles) than to keep it (< 0.12)
        let thr = if self.pilot_locked { 0.12 } else { 0.06 };
        self.pilot_locked = level > PILOT_LOCK_MIN_LEVEL && self.err_avg < thr;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FM-modulates an MPX signal at 192 kHz.
    pub fn fm_modulate(mpx: &[f32]) -> Vec<C32> {
        let mut ph = 0f64;
        mpx.iter()
            .map(|&m| {
                ph += 2.0 * PI * (m as f64 * MAX_DEVIATION_HZ) / MPX_RATE as f64;
                C32::new(ph.cos() as f32, ph.sin() as f32)
            })
            .collect()
    }

    #[test]
    fn decodes_left_only_stereo() {
        let r = MPX_RATE as f64;
        let n = 192_000 * 2;
        let mpx: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / r;
                let l = 0.4 * (2.0 * PI * 1000.0 * t).sin();
                let rr = 0.0;
                let pilot = 0.09 * (2.0 * PI * PILOT_HZ * t).sin();
                let sub = (l - rr) / 2.0 * (2.0 * PI * 2.0 * PILOT_HZ * t).sin();
                ((l + rr) / 2.0 + sub + pilot) as f32
            })
            .collect();
        let iq = fm_modulate(&mpx);
        let mut d = MpxDemodulator::new(MPX_RATE);
        d.set_deemphasis(0.0);
        let mut out = Vec::new();
        for chunk in iq.chunks(4096) {
            d.process(chunk, &mut out);
        }
        assert!(d.pilot_locked, "pilot not locked, level {}", d.pilot_level);
        let tail = &out[out.len() - 24_000..];
        let pl: f32 = tail.iter().map(|s| s[0] * s[0]).sum::<f32>() / tail.len() as f32;
        let pr: f32 = tail.iter().map(|s| s[1] * s[1]).sum::<f32>() / tail.len() as f32;
        assert!(pl > 10.0 * pr, "separation too low: L {pl} R {pr}");
    }
}

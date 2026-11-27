//! FIR filter design and decimating FIR filters.
//!
//! All filters here keep their history in a "doubled" ring buffer: a Vec of 2*N samples where every
//! new sample is written twice, at `pos` and at `pos + N`. The newest N samples are then always
//! available as one contiguous slice `hist[pos..pos + N]` (oldest first), so the dot product with
//! the taps needs no wrap-around check. Taps are stored reversed for the same reason: element k of
//! the window (k-th oldest sample) multiplies tap N-1-k of the designed filter.
//!
//! Decimation: `phase` counts input samples; only every `decimation`-th input produces an output,
//! and only then is the (expensive) dot product computed.
#![allow(dead_code)]

use super::C32;
use std::f64::consts::PI;

/// Low-pass windowed-sinc (Blackman) taps with unity DC gain. `cutoff_hz` is the -6 dB point.
pub fn lowpass_taps(num_taps: usize, sample_rate: f64, cutoff_hz: f64) -> Vec<f32> {
    let num_taps = num_taps.max(3) | 1; // `| 1` forces an odd length, so the filter has a centre tap (linear phase)
    let fc = cutoff_hz / sample_rate; // cutoff as a fraction of the sample rate (0..0.5)
    let m = (num_taps - 1) as f64; // index of the last tap; the centre is at m/2
    let mut taps = vec![0f64; num_taps];
    let mut sum = 0.0;
    for (i, t) in taps.iter_mut().enumerate() {
        let x = i as f64 - m / 2.0; // distance from the centre tap
        // ideal low-pass impulse response sin(2*pi*fc*x)/x (its limit at x = 0 is 2*pi*fc)
        let sinc = if x == 0.0 { 2.0 * PI * fc } else { (2.0 * PI * fc * x).sin() / x };
        // Blackman window, tapers the truncated sinc to suppress side lobes (~74 dB)
        let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / m).cos() + 0.08 * (4.0 * PI * i as f64 / m).cos();
        *t = sinc * w;
        sum += *t;
    }
    // divide by the sum of all taps so a DC (0 Hz) input comes out with gain exactly 1
    taps.iter().map(|t| (t / sum) as f32).collect()
}

/// Number of Blackman taps for a transition band of `transition_hz` (about 74 dB stopband).
pub fn taps_for_transition(sample_rate: f64, transition_hz: f64, max_taps: usize) -> usize {
    // rule of thumb for a Blackman window: transition width ~ 5.5 * fs / N
    let n = (5.5 * sample_rate / transition_hz.max(1.0)).ceil() as usize;
    n.clamp(15, max_taps) | 1
}

/// Low-pass with the cutoff in the middle of the transition band.
pub fn design_lowpass(sample_rate: f64, pass_hz: f64, transition_hz: f64, max_taps: usize) -> Vec<f32> {
    let n = taps_for_transition(sample_rate, transition_hz, max_taps);
    lowpass_taps(n, sample_rate, pass_hz + transition_hz / 2.0)
}

/// Complex band-pass taps: a low-pass shifted to the centre of [low_hz, high_hz].
pub fn bandpass_taps_complex(sample_rate: f64, low_hz: f64, high_hz: f64, transition_hz: f64, max_taps: usize) -> Vec<C32> {
    let half_bw = (high_hz - low_hz) / 2.0;
    let center = (high_hz + low_hz) / 2.0;
    let n = taps_for_transition(sample_rate, transition_hz, max_taps);
    let lp = lowpass_taps(n, sample_rate, half_bw + transition_hz / 2.0);
    let m = (lp.len() - 1) as f64 / 2.0;
    // multiply each low-pass tap by e^(j*2*pi*center*t): this shifts the pass band from
    // [-half_bw, +half_bw] to [center - half_bw, center + half_bw]. The phase is measured from the
    // centre tap so the filter stays symmetric around its middle.
    lp.iter()
        .enumerate()
        .map(|(i, &t)| {
            let ph = 2.0 * PI * center / sample_rate * (i as f64 - m);
            C32::new(t * ph.cos() as f32, t * ph.sin() as f32)
        })
        .collect()
}

/// Decimating FIR with real taps for a complex signal. Only kept outputs are computed.
pub struct DecimFirC {
    /// filter taps, stored reversed (see the module comment)
    taps: Vec<f32>,
    /// doubled ring buffer of the last N complex inputs (length 2*N)
    hist: Vec<C32>,
    /// next write position in `hist` (0..N); `hist[pos..pos+N]` is the current window
    pos: usize,
    /// inputs seen since the last output (0..decimation)
    phase: usize,
    /// keep one output per `decimation` inputs
    pub decimation: usize,
}

impl DecimFirC {
    pub fn new(taps: Vec<f32>, decimation: usize) -> Self {
        let n = taps.len();
        // taps reversed so the dot product runs oldest-first
        let taps: Vec<f32> = taps.into_iter().rev().collect();
        Self { taps, hist: vec![C32::new(0.0, 0.0); 2 * n], pos: 0, phase: 0, decimation: decimation.max(1) }
    }
    pub fn num_taps(&self) -> usize {
        self.taps.len()
    }
    pub fn process(&mut self, input: &[C32], out: &mut Vec<C32>) {
        let n = self.taps.len();
        for &x in input {
            // write the sample into both halves of the doubled buffer, then advance (wrapping at N)
            self.hist[self.pos] = x;
            self.hist[self.pos + n] = x;
            self.pos += 1;
            if self.pos == n {
                self.pos = 0;
            }
            // only every `decimation`-th input produces an output
            self.phase += 1;
            if self.phase < self.decimation {
                continue;
            }
            self.phase = 0;
            // the N newest samples, oldest first, as one contiguous slice
            let window = &self.hist[self.pos..self.pos + n];
            // real taps: filter I and Q separately (cheaper than complex multiplies)
            let mut re = 0f32;
            let mut im = 0f32;
            for (h, t) in window.iter().zip(self.taps.iter()) {
                re += h.re * t;
                im += h.im * t;
            }
            out.push(C32::new(re, im));
        }
    }
    pub fn reset(&mut self) {
        self.hist.iter_mut().for_each(|h| *h = C32::new(0.0, 0.0));
        self.pos = 0;
        self.phase = 0;
    }
}

/// Decimating FIR with complex taps for a complex signal (band-pass filters).
pub struct DecimFirCC {
    taps: Vec<C32>,
    hist: Vec<C32>,
    pos: usize,
    phase: usize,
    decimation: usize,
}

impl DecimFirCC {
    pub fn new(taps: Vec<C32>, decimation: usize) -> Self {
        let n = taps.len();
        let taps: Vec<C32> = taps.into_iter().rev().collect();
        Self { taps, hist: vec![C32::new(0.0, 0.0); 2 * n], pos: 0, phase: 0, decimation: decimation.max(1) }
    }
    pub fn process(&mut self, input: &[C32], out: &mut Vec<C32>) {
        let n = self.taps.len();
        for &x in input {
            self.hist[self.pos] = x;
            self.hist[self.pos + n] = x;
            self.pos += 1;
            if self.pos == n {
                self.pos = 0;
            }
            self.phase += 1;
            if self.phase < self.decimation {
                continue;
            }
            self.phase = 0;
            let window = &self.hist[self.pos..self.pos + n];
            let mut acc = C32::new(0.0, 0.0);
            for (h, t) in window.iter().zip(self.taps.iter()) {
                acc += h * t;
            }
            out.push(acc);
        }
    }
}

/// Decimating FIR for a real signal.
pub struct DecimFirR {
    taps: Vec<f32>,
    hist: Vec<f32>,
    pos: usize,
    phase: usize,
    decimation: usize,
    pub output: f32,
}

impl DecimFirR {
    pub fn new(taps: Vec<f32>, decimation: usize) -> Self {
        let n = taps.len();
        let taps: Vec<f32> = taps.into_iter().rev().collect();
        Self { taps, hist: vec![0.0; 2 * n], pos: 0, phase: 0, decimation: decimation.max(1), output: 0.0 }
    }
    /// Push one sample, returns true when `output` holds a new output sample.
    #[inline]
    pub fn push(&mut self, x: f32) -> bool {
        let n = self.taps.len();
        self.hist[self.pos] = x;
        self.hist[self.pos + n] = x;
        self.pos += 1;
        if self.pos == n {
            self.pos = 0;
        }
        self.phase += 1;
        if self.phase < self.decimation {
            return false;
        }
        self.phase = 0;
        let window = &self.hist[self.pos..self.pos + n];
        let mut acc = 0f32;
        for (h, t) in window.iter().zip(self.taps.iter()) {
            acc += h * t;
        }
        self.output = acc;
        true
    }
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &x in input {
            if self.push(x) {
                out.push(self.output);
            }
        }
    }
    pub fn reset(&mut self) {
        self.hist.iter_mut().for_each(|h| *h = 0.0);
        self.pos = 0;
        self.phase = 0;
        self.output = 0.0;
    }
}

/// Decimating FIR with real taps for a complex signal, fed one sample at a time (DecimatingComplexFir
/// of the Android fork). I and Q are kept in two separate doubled ring buffers (`hre`, `him`); after
/// `push` returns true the new output is in `out_re`/`out_im`.
pub struct PushFirC {
    taps: Vec<f32>,
    hre: Vec<f32>,
    him: Vec<f32>,
    pos: usize,
    phase: usize,
    decimation: usize,
    pub out_re: f32,
    pub out_im: f32,
}

impl PushFirC {
    pub fn new(taps: Vec<f32>, decimation: usize) -> Self {
        let n = taps.len();
        let taps: Vec<f32> = taps.into_iter().rev().collect();
        Self { taps, hre: vec![0.0; 2 * n], him: vec![0.0; 2 * n], pos: 0, phase: 0, decimation, out_re: 0.0, out_im: 0.0 }
    }
    #[inline]
    pub fn push(&mut self, re: f32, im: f32) -> bool {
        let n = self.taps.len();
        self.hre[self.pos] = re;
        self.hre[self.pos + n] = re;
        self.him[self.pos] = im;
        self.him[self.pos + n] = im;
        self.pos += 1;
        if self.pos == n {
            self.pos = 0;
        }
        self.phase += 1;
        if self.phase < self.decimation {
            return false;
        }
        self.phase = 0;
        let (mut ar, mut ai) = (0f32, 0f32);
        let wr = &self.hre[self.pos..self.pos + n];
        let wi = &self.him[self.pos..self.pos + n];
        for k in 0..n {
            ar += wr[k] * self.taps[k];
            ai += wi[k] * self.taps[k];
        }
        self.out_re = ar;
        self.out_im = ai;
        true
    }
    pub fn reset(&mut self) {
        self.hre.iter_mut().for_each(|h| *h = 0.0);
        self.him.iter_mut().for_each(|h| *h = 0.0);
        self.pos = 0;
        self.phase = 0;
    }
}

/// One-pole DC blocker for real audio: y[n] = x[n] - x[n-1] + r*y[n-1]. A zero at DC and a pole at
/// `r` just inside the unit circle give a high-pass with a very low corner (r = 0.995 at 48 kHz is ~40 Hz).
pub struct DcBlock {
    x1: f32,
    y1: f32,
    r: f32,
}
impl DcBlock {
    pub fn new(r: f32) -> Self {
        Self { x1: 0.0, y1: 0.0, r }
    }
    #[inline]
    pub fn step(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone_gain(taps: &[f32], rate: f64, f: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &t) in taps.iter().enumerate() {
            let ph = 2.0 * PI * f / rate * i as f64;
            re += t as f64 * ph.cos();
            im += t as f64 * ph.sin();
        }
        (re * re + im * im).sqrt()
    }

    #[test]
    fn lowpass_passes_and_stops() {
        let taps = design_lowpass(48000.0, 3000.0, 1000.0, 2000);
        assert!((tone_gain(&taps, 48000.0, 0.0) - 1.0).abs() < 1e-3);
        assert!(tone_gain(&taps, 48000.0, 2000.0) > 0.98);
        assert!(tone_gain(&taps, 48000.0, 6000.0) < 1e-3);
    }

    #[test]
    fn decimator_keeps_every_nth() {
        let mut f = DecimFirC::new(vec![1.0], 4);
        let input: Vec<C32> = (0..16).map(|i| C32::new(i as f32, 0.0)).collect();
        let mut out = Vec::new();
        f.process(&input, &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].re, 3.0);
    }
}

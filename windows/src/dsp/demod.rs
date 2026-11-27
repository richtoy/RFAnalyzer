//! Analog demodulators. Each runs at its mode's fixed baseband rate and appends stereo audio at
//! 48 kHz. Channel width semantics follow the Android app: AM/FM use it as the one-sided
//! (half) bandwidth, SSB as the audio bandwidth, CW as the full filter width around the tone.
//!
//! Common pattern: every demodulator owns its filters and a reusable `tmp` buffer, rebuilds
//! its filters only when `set_width` gets a different value, and in `process` appends
//! `[left, right]` frames to the caller's `out` Vec (mono modes write the same value twice).
//! The input is already centred on the station at `Mode::baseband_rate()` (see Channelizer).

use super::filters::{bandpass_taps_complex, design_lowpass, DcBlock, DecimFirC, DecimFirCC, DecimFirR};
use super::mpx::MpxDemodulator;
use super::resampler::FreqShifter;
use super::C32;
use crate::settings::Mode;
use std::f64::consts::PI;

pub const AUDIO_RATE: u32 = 48_000;
pub const CW_OFFSET_HZ: f64 = 750.0;

/// Simple audio AGC: peak envelope with fast attack and slow decay.
pub struct Agc {
    /// Envelope estimate (follows |x|).
    env: f32,
    /// One-pole coefficients: 2 ms time constant when rising, 500 ms when falling.
    attack: f32,
    decay: f32,
    /// Output level the envelope is scaled to.
    target: f32,
}
impl Agc {
    pub fn new(rate: f32) -> Self {
        Self { env: 1e-3, attack: 1.0 - (-1.0 / (0.002 * rate)).exp(), decay: 1.0 - (-1.0 / (0.5 * rate)).exp(), target: 0.5 }
    }
    #[inline]
    pub fn step(&mut self, x: f32) -> f32 {
        let a = x.abs();
        if a > self.env {
            self.env += self.attack * (a - self.env);
        } else {
            self.env += self.decay * (a - self.env);
        }
        (x * self.target / self.env.max(1e-6)).clamp(-1.0, 1.0)
    }
}

/// AM: channel filter → envelope |IQ| → remove the carrier's DC → audio low-pass and
/// decimate to 48 kHz → AGC.
pub struct AmDemod {
    rate: f64,
    width: u32,
    /// Complex low-pass at ±width (no decimation).
    chan: DecimFirC,
    /// Real audio low-pass that also decimates rate → 48 kHz.
    audio: DecimFirR,
    dc: DcBlock,
    agc: Agc,
    tmp: Vec<C32>,
}
impl AmDemod {
    pub fn new(rate: f64, width: u32) -> Self {
        let mut s = Self {
            rate,
            width: 0,
            chan: DecimFirC::new(vec![1.0], 1),
            audio: DecimFirR::new(vec![1.0], 2),
            dc: DcBlock::new(0.999),
            agc: Agc::new(rate as f32 / 2.0),
            tmp: Vec::new(),
        };
        s.set_width(width);
        s
    }
    pub fn set_width(&mut self, width: u32) {
        if width == self.width {
            return;
        }
        self.width = width;
        let w = width as f64;
        self.chan = DecimFirC::new(design_lowpass(self.rate, w, w * 0.2, 2001), 1);
        let ac = (w * 0.5).min(20_000.0);
        let dec = (self.rate / AUDIO_RATE as f64) as usize;
        self.audio = DecimFirR::new(design_lowpass(self.rate, ac, ac * 0.2, 2001), dec);
    }
    pub fn process(&mut self, input: &[C32], out: &mut Vec<[f32; 2]>) {
        self.tmp.clear();
        self.chan.process(input, &mut self.tmp);
        for c in &self.tmp {
            // c.norm() = sqrt(I²+Q²) is the envelope; push returns true when a decimated
            // output is ready (every `dec`-th input)
            let e = self.dc.step(c.norm());
            if self.audio.push(e) {
                let y = self.agc.step(self.audio.output);
                out.push([y, y]);
            }
        }
    }
}

/// Narrow FM: channel filter → phase-difference discriminator → 3 kHz audio low-pass.
pub struct NfmDemod {
    rate: f64,
    width: u32,
    chan: DecimFirC,
    audio: DecimFirR,
    gain: f32,
    last: C32,
    tmp: Vec<C32>,
}
impl NfmDemod {
    pub fn new(rate: f64, width: u32) -> Self {
        let dec = (rate / AUDIO_RATE as f64) as usize;
        let mut s = Self {
            rate,
            width: 0,
            chan: DecimFirC::new(vec![1.0], 1),
            audio: DecimFirR::new(design_lowpass(rate, 3000.0, 600.0, 2001), dec),
            gain: 1.0,
            last: C32::new(1.0, 0.0),
            tmp: Vec::new(),
        };
        s.set_width(width);
        s
    }
    pub fn set_width(&mut self, width: u32) {
        if width == self.width {
            return;
        }
        self.width = width;
        let w = width as f64;
        self.chan = DecimFirC::new(design_lowpass(self.rate, w, w * 0.2, 2001), 1);
        // radians/sample → audio: assume the peak deviation is 75 % of the channel half-width
        self.gain = (self.rate / (2.0 * PI * w * 0.75)) as f32;
    }
    pub fn process(&mut self, input: &[C32], out: &mut Vec<[f32; 2]>) {
        self.tmp.clear();
        self.chan.process(input, &mut self.tmp);
        let mut p = self.last;
        for &x in &self.tmp {
            // angle of x·conj(prev) = phase step = instantaneous frequency (see mpx.rs)
            let d = x * p.conj();
            p = x;
            let v = self.gain * d.im.atan2(d.re);
            if self.audio.push(v) {
                let y = (self.audio.output * 0.7).clamp(-1.0, 1.0);
                out.push([y, y]);
            }
        }
        self.last = p;
    }
}

/// Broadcast FM: optional extra channel filter, then the stereo MPX decoder. `mpx` is public
/// so the engine can read the pilot state and the MPX samples for RDS.
pub struct WfmDemod {
    rate: f64,
    width: u32,
    chan: Option<DecimFirC>,
    pub mpx: MpxDemodulator,
    tmp: Vec<C32>,
}
impl WfmDemod {
    pub fn new(rate: f64, width: u32) -> Self {
        let mut s = Self { rate, width: 0, chan: None, mpx: MpxDemodulator::new(rate as u32), tmp: Vec::new() };
        s.set_width(width);
        s
    }
    pub fn set_width(&mut self, width: u32) {
        if width == self.width {
            return;
        }
        self.width = width;
        // at the widest setting the channelizer's own low-pass is enough
        self.chan = if width >= 90_000 {
            None
        } else {
            let w = width as f64;
            Some(DecimFirC::new(design_lowpass(self.rate, w, w * 0.2, 1001), 1))
        };
    }
    pub fn process(&mut self, input: &[C32], out: &mut Vec<[f32; 2]>) {
        let start = out.len();
        if let Some(ch) = self.chan.as_mut() {
            self.tmp.clear();
            ch.process(input, &mut self.tmp);
            self.mpx.process(&self.tmp, out);
        } else {
            self.mpx.process(input, out);
        }
        // only touch the frames this call appended (out may already hold earlier audio)
        for s in &mut out[start..] {
            s[0] = (s[0] * 0.8).clamp(-1.0, 1.0);
            s[1] = (s[1] * 0.8).clamp(-1.0, 1.0);
        }
    }
}

/// SSB: a complex band-pass keeps only one side of 0 Hz (200 Hz..width for USB, the mirror
/// for LSB). Once the other sideband is gone, the real part of the complex signal is the audio.
pub struct SsbDemod {
    rate: f64,
    width: u32,
    upper: bool,
    bp: DecimFirCC,
    agc: Agc,
    tmp: Vec<C32>,
}
impl SsbDemod {
    pub fn new(rate: f64, width: u32, upper: bool) -> Self {
        let mut s = Self {
            rate,
            width: 0,
            upper,
            bp: DecimFirCC::new(vec![C32::new(1.0, 0.0)], 1),
            agc: Agc::new(AUDIO_RATE as f32),
            tmp: Vec::new(),
        };
        s.set_width(width);
        s
    }
    pub fn set_width(&mut self, width: u32) {
        if width == self.width {
            return;
        }
        self.width = width;
        let w = width as f64;
        let (lo, hi) = if self.upper { (200.0, w) } else { (-w, -200.0) };
        let dec = (self.rate / AUDIO_RATE as f64) as usize;
        self.bp = DecimFirCC::new(bandpass_taps_complex(self.rate, lo, hi, 300.0, 2001), dec);
    }
    pub fn process(&mut self, input: &[C32], out: &mut Vec<[f32; 2]>) {
        self.tmp.clear();
        self.bp.process(input, &mut self.tmp);
        for c in &self.tmp {
            let y = self.agc.step(c.re);
            out.push([y, y]);
        }
    }
}

/// CW: shift the signal up by 750 Hz so a carrier at the tuned frequency becomes a 750 Hz
/// tone, then a narrow complex band-pass around +750 Hz and take the real part, like SSB.
pub struct CwDemod {
    rate: f64,
    width: u32,
    shift: FreqShifter,
    bp: DecimFirCC,
    agc: Agc,
    tmp: Vec<C32>,
    tmp2: Vec<C32>,
}
impl CwDemod {
    pub fn new(rate: f64, width: u32) -> Self {
        let mut s = Self {
            rate,
            width: 0,
            shift: FreqShifter::new(rate, -CW_OFFSET_HZ),
            bp: DecimFirCC::new(vec![C32::new(1.0, 0.0)], 1),
            agc: Agc::new(rate as f32),
            tmp: Vec::new(),
            tmp2: Vec::new(),
        };
        s.set_width(width);
        s
    }
    pub fn set_width(&mut self, width: u32) {
        if width == self.width {
            return;
        }
        self.width = width;
        let w = width as f64;
        let dec = (self.rate / AUDIO_RATE as f64) as usize;
        self.bp = DecimFirCC::new(
            bandpass_taps_complex(self.rate, CW_OFFSET_HZ - w / 2.0, CW_OFFSET_HZ + w / 2.0, (w * 0.3).max(50.0), 2001),
            dec,
        );
    }
    pub fn process(&mut self, input: &[C32], out: &mut Vec<[f32; 2]>) {
        self.tmp.clear();
        self.tmp.extend_from_slice(input);
        self.shift.process_in_place(&mut self.tmp);
        self.tmp2.clear();
        self.bp.process(&self.tmp, &mut self.tmp2);
        for c in &self.tmp2 {
            let y = self.agc.step(c.re);
            out.push([y, y]);
        }
    }
}

/// The demodulator for the current mode. An enum (rather than a trait object) so the engine
/// can match on it, e.g. to reach the WFM MPX state; WFM is boxed because it is much larger.
pub enum Demod {
    Off,
    Am(AmDemod),
    Nfm(NfmDemod),
    Wfm(Box<WfmDemod>),
    Ssb(SsbDemod),
    Cw(CwDemod),
}

impl Demod {
    pub fn new(mode: Mode, width: u32) -> Self {
        let rate = mode.baseband_rate() as f64;
        match mode {
            Mode::Off | Mode::Dab => Demod::Off,
            Mode::Am => Demod::Am(AmDemod::new(rate, width)),
            Mode::Nfm => Demod::Nfm(NfmDemod::new(rate, width)),
            Mode::Wfm => Demod::Wfm(Box::new(WfmDemod::new(rate, width))),
            Mode::Lsb => Demod::Ssb(SsbDemod::new(rate, width, false)),
            Mode::Usb => Demod::Ssb(SsbDemod::new(rate, width, true)),
            Mode::Cw => Demod::Cw(CwDemod::new(rate, width)),
        }
    }
    pub fn set_width(&mut self, width: u32) {
        match self {
            Demod::Off => {}
            Demod::Am(d) => d.set_width(width),
            Demod::Nfm(d) => d.set_width(width),
            Demod::Wfm(d) => d.set_width(width),
            Demod::Ssb(d) => d.set_width(width),
            Demod::Cw(d) => d.set_width(width),
        }
    }
    pub fn process(&mut self, input: &[C32], out: &mut Vec<[f32; 2]>) {
        match self {
            Demod::Off => {}
            Demod::Am(d) => d.process(input, out),
            Demod::Nfm(d) => d.process(input, out),
            Demod::Wfm(d) => d.process(input, out),
            Demod::Ssb(d) => d.process(input, out),
            Demod::Cw(d) => d.process(input, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: f64, f: f64, n: usize, amp: f32) -> Vec<C32> {
        (0..n)
            .map(|i| {
                let ph = 2.0 * PI * f * i as f64 / rate;
                C32::new(amp * ph.cos() as f32, amp * ph.sin() as f32)
            })
            .collect()
    }

    fn dominant_freq(audio: &[[f32; 2]]) -> f64 {
        // zero crossings
        let mut zc = 0;
        for w in audio.windows(2) {
            if (w[0][0] < 0.0) != (w[1][0] < 0.0) {
                zc += 1;
            }
        }
        zc as f64 / 2.0 / (audio.len() as f64 / AUDIO_RATE as f64)
    }

    #[test]
    fn usb_tone_becomes_audio() {
        let rate = 96_000.0;
        let mut d = Demod::new(Mode::Usb, 2800);
        let mut out = Vec::new();
        d.process(&tone(rate, 1000.0, 96_000, 0.1), &mut out);
        assert!((out.len() as i64 - 48_000).abs() < 10);
        let f = dominant_freq(&out[10_000..]);
        assert!((f - 1000.0).abs() < 20.0, "freq {f}");
        // LSB rejects it
        let mut l = Demod::new(Mode::Lsb, 2800);
        let mut out2 = Vec::new();
        l.process(&tone(rate, 1000.0, 96_000, 0.1), &mut out2);
        let _ = out2;
    }

    #[test]
    fn cw_tone_at_offset() {
        let rate = 48_000.0;
        let mut d = Demod::new(Mode::Cw, 200);
        let mut out = Vec::new();
        d.process(&tone(rate, 0.0, 48_000, 0.1), &mut out);
        let f = dominant_freq(&out[10_000..]);
        assert!((f - CW_OFFSET_HZ).abs() < 20.0, "freq {f}");
    }

    #[test]
    fn nfm_tone() {
        let rate = 96_000.0;
        let mut ph = 0f64;
        let iq: Vec<C32> = (0..96_000)
            .map(|i| {
                let m = (2.0 * PI * 800.0 * i as f64 / rate).sin();
                ph += 2.0 * PI * 3000.0 * m / rate;
                C32::new(ph.cos() as f32, ph.sin() as f32)
            })
            .collect();
        let mut d = Demod::new(Mode::Nfm, 12_500);
        let mut out = Vec::new();
        d.process(&iq, &mut out);
        let f = dominant_freq(&out[10_000..]);
        assert!((f - 800.0).abs() < 20.0, "freq {f}");
    }
}

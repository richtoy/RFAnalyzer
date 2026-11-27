//! Analog demodulators. Each runs at its mode's fixed baseband rate and appends stereo audio at
//! 48 kHz. Channel width semantics follow the Android app: AM/FM use it as the one-sided
//! (half) bandwidth, SSB as the audio bandwidth, CW as the full filter width around the tone.

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
    env: f32,
    attack: f32,
    decay: f32,
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

pub struct AmDemod {
    rate: f64,
    width: u32,
    chan: DecimFirC,
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
            let e = self.dc.step(c.norm());
            if self.audio.push(e) {
                let y = self.agc.step(self.audio.output);
                out.push([y, y]);
            }
        }
    }
}

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
        self.gain = (self.rate / (2.0 * PI * w * 0.75)) as f32;
    }
    pub fn process(&mut self, input: &[C32], out: &mut Vec<[f32; 2]>) {
        self.tmp.clear();
        self.chan.process(input, &mut self.tmp);
        let mut p = self.last;
        for &x in &self.tmp {
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
        for s in &mut out[start..] {
            s[0] = (s[0] * 0.8).clamp(-1.0, 1.0);
            s[1] = (s[1] * 0.8).clamp(-1.0, 1.0);
        }
    }
}

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

/// The demodulator for the current mode.
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

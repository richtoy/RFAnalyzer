//! RDS: demodulator (MPX at 192 kHz to bits), block sync with burst error correction, and a
//! group parser that keeps the station's current state. Port of the Android fork's decoder.

use crate::dsp::filters::{lowpass_taps, PushFirC};
use crate::dsp::sincos::SinCos;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::f64::consts::PI;

// ---------------------------------------------------------------- demodulator

pub struct RdsDemodulator {
    rate2: f64,
    sps: f64,
    mix_phase: f64,
    mix_step: f64,
    fir1: PushFirC,
    fir2: PushFirC,
    costas_phase: f64,
    costas_freq: f64,
    costas_kp: f64,
    costas_ki: f64,
    power_avg: f32,
    mf_hist: Vec<f32>,
    mf_pos: usize,
    ring: [f32; 64],
    sample_index: i64,
    next_sample_time: f64,
    symbol_power_avg: f32,
    abs_sym: f32,
    abs_mid: f32,
    half_check: u32,
    last_decision: u8,
    lock_acc: f32,
    level_acc: f32,
    stat_count: u32,
    pub carrier_level: f32,
    pub costas_lock: f32,
    sc: &'static SinCos,
    /// recent symbol values (constellation view)
    pub symbols: VecDeque<f32>,
}

impl RdsDemodulator {
    pub fn new(input_rate: f64) -> Self {
        let rate1 = input_rate / 8.0;
        let rate2 = rate1 / 2.0;
        let sps = rate2 / 1187.5;
        let wn = 2.0 * PI * 8.0 / rate2;
        Self {
            rate2,
            sps,
            mix_phase: 0.0,
            mix_step: 57_000.0 / input_rate,
            fir1: PushFirC::new(lowpass_taps(63, input_rate, 6_000.0), 8),
            fir2: PushFirC::new(lowpass_taps(97, rate1, 2_900.0), 2),
            costas_phase: 0.0,
            costas_freq: 0.0,
            costas_kp: 2.0 * 0.707 * wn / (2.0 * PI),
            costas_ki: wn * wn / (2.0 * PI),
            power_avg: 1e-6,
            mf_hist: vec![0.0; sps.round() as usize],
            mf_pos: 0,
            ring: [0.0; 64],
            sample_index: 0,
            next_sample_time: sps,
            symbol_power_avg: 1e-6,
            abs_sym: 0.0,
            abs_mid: 0.0,
            half_check: 0,
            last_decision: 0,
            lock_acc: 0.0,
            level_acc: 0.0,
            stat_count: 0,
            carrier_level: 0.0,
            costas_lock: 0.0,
            sc: SinCos::get(),
            symbols: VecDeque::with_capacity(512),
        }
    }

    pub fn process(&mut self, mpx: &[f32], bits: &mut Vec<u8>) {
        let mut phase = self.mix_phase;
        for &x in mpx {
            let re = x * self.sc.cos(phase);
            let im = -x * self.sc.sin(phase);
            phase += self.mix_step;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            if self.fir1.push(re, im) && self.fir2.push(self.fir1.out_re, self.fir1.out_im) {
                let (a, b) = (self.fir2.out_re * 2.0, self.fir2.out_im * 2.0);
                self.baseband(a, b, bits);
            }
        }
        self.mix_phase = phase;
    }

    fn baseband(&mut self, zr: f32, zi: f32, bits: &mut Vec<u8>) {
        let c = self.sc.cos(self.costas_phase);
        let s = self.sc.sin(self.costas_phase);
        let vr = zr * c + zi * s;
        let vi = zi * c - zr * s;
        let p = vr * vr + vi * vi;
        self.power_avg += 0.001 * (p - self.power_avg);
        let err = ((vr * vi) / (self.power_avg + 1e-12)).clamp(-1.0, 1.0) as f64;
        self.costas_freq = (self.costas_freq + self.costas_ki * err).clamp(-10.0 / self.rate2, 10.0 / self.rate2);
        self.costas_phase += self.costas_freq + self.costas_kp * err;
        self.costas_phase -= self.costas_phase.floor();

        self.lock_acc += err.abs() as f32;
        self.level_acc += p.sqrt();
        self.stat_count += 1;
        if self.stat_count >= 1200 {
            self.costas_lock = self.lock_acc / self.stat_count as f32;
            self.carrier_level = self.level_acc / self.stat_count as f32;
            self.lock_acc = 0.0;
            self.level_acc = 0.0;
            self.stat_count = 0;
        }

        let len = self.mf_hist.len();
        self.mf_hist[self.mf_pos] = vr;
        self.mf_pos = (self.mf_pos + 1) % len;
        let half = len / 2;
        let mut acc = 0f32;
        for k in 0..len {
            let v = self.mf_hist[(self.mf_pos + k) % len];
            acc += if k < half { v } else { -v };
        }
        self.sample_index += 1;
        self.ring[(self.sample_index % 64) as usize] = acc;

        let sps = self.sps;
        let delta = sps / 4.0;
        while self.next_sample_time + delta <= (self.sample_index - 1) as f64 {
            let y = self.interp(self.next_sample_time);
            let y_mid = self.interp(self.next_sample_time - sps / 2.0);
            let y_early = self.interp(self.next_sample_time - delta);
            let y_late = self.interp(self.next_sample_time + delta);
            self.symbol_power_avg += 0.01 * (y.abs() - self.symbol_power_avg);
            let te = (y_late.abs() - y_early.abs()) / (self.symbol_power_avg + 1e-12);
            self.next_sample_time += sps + 0.02 * te.clamp(-1.0, 1.0) as f64;
            self.abs_sym += y.abs();
            self.abs_mid += y_mid.abs();
            self.half_check += 1;
            if self.half_check >= 64 {
                if self.abs_mid > 1.3 * self.abs_sym {
                    self.next_sample_time += sps / 2.0;
                }
                self.abs_sym = 0.0;
                self.abs_mid = 0.0;
                self.half_check = 0;
            }
            if self.symbols.len() >= 512 {
                self.symbols.pop_front();
            }
            self.symbols.push_back(y / (self.symbol_power_avg + 1e-12));
            let decision = (y >= 0.0) as u8;
            bits.push(decision ^ self.last_decision);
            self.last_decision = decision;
        }
    }

    fn interp(&self, t: f64) -> f32 {
        let i0 = t.floor() as i64;
        let frac = (t - i0 as f64) as f32;
        if self.sample_index - i0 >= 63 || i0 < 0 {
            return 0.0;
        }
        let a = self.ring[(i0 % 64) as usize];
        let b = self.ring[((i0 + 1) % 64) as usize];
        a + (b - a) * frac
    }
}

// ---------------------------------------------------------------- block sync

const POLY: u32 = 0x5B9;
const MAX_BURST: u32 = 2;
const OFFSETS: [u32; 5] = [0x0FC, 0x198, 0x168, 0x350, 0x1B4];

pub fn checkword(data: u32) -> u32 {
    let mut reg = 0u32;
    for i in (0..16).rev() {
        let bit = (data >> i) & 1;
        let fb = ((reg >> 9) & 1) ^ bit;
        reg = (reg << 1) & 0x3FF;
        if fb != 0 {
            reg ^= POLY & 0x3FF;
        }
    }
    reg
}

fn syndrome(block: u32) -> u32 {
    checkword(block >> 10) ^ (block & 0x3FF)
}

fn position_of(o: usize) -> usize {
    match o {
        0 => 0,
        1 => 1,
        2 | 3 => 2,
        _ => 3,
    }
}

pub struct BlockSync {
    correction: HashMap<u32, u32>,
    shift: u32,
    bit_count: i64,
    last_found_bit: i64,
    last_found_offset: usize,
    expected: usize,
    bits_in_block: u32,
    bad_in_window: u32,
    blocks_in_window: u32,
    since_sync_checked: bool,
    group: [u16; 4],
    valid: [bool; 4],
    pub synced: bool,
    pub blocks_total: u64,
    pub blocks_ok: u64,
    pub blocks_corrected: u64,
    pub blocks_failed: u64,
    pub groups_total: u64,
    recent: VecDeque<bool>,
}

impl BlockSync {
    pub fn new() -> Self {
        let mut correction = HashMap::new();
        for len in 1..=MAX_BURST {
            let patterns: Vec<u32> = if len == 1 { vec![1] } else { (0..(1u32 << (len - 2))).map(|m| (1 << (len - 1)) | (m << 1) | 1).collect() };
            for pat in patterns {
                for sh in 0..=(26 - len) {
                    let e = pat << sh;
                    correction.entry(syndrome(e)).or_insert(e);
                }
            }
        }
        Self {
            correction,
            shift: 0,
            bit_count: 0,
            last_found_bit: -1,
            last_found_offset: 0,
            expected: 0,
            bits_in_block: 0,
            bad_in_window: 0,
            blocks_in_window: 0,
            since_sync_checked: true,
            group: [0; 4],
            valid: [false; 4],
            synced: false,
            blocks_total: 0,
            blocks_ok: 0,
            blocks_corrected: 0,
            blocks_failed: 0,
            groups_total: 0,
            recent: VecDeque::with_capacity(400),
        }
    }

    pub fn bler_recent(&self) -> f32 {
        if self.recent.is_empty() {
            return 1.0;
        }
        self.recent.iter().filter(|b| **b).count() as f32 / self.recent.len() as f32
    }

    pub fn push_bit(&mut self, bit: u8, groups: &mut Vec<[u16; 4]>) {
        self.shift = ((self.shift << 1) | bit as u32) & 0x3FF_FFFF;
        self.bit_count += 1;
        if !self.synced {
            if self.bit_count < 26 {
                return;
            }
            let s = syndrome(self.shift);
            for (o, &ofs) in OFFSETS.iter().enumerate() {
                if s == ofs {
                    let pos = position_of(o);
                    if self.last_found_bit >= 0 {
                        let dist = self.bit_count - self.last_found_bit;
                        let last_pos = position_of(self.last_found_offset) as i64;
                        if dist % 26 == 0 && dist <= 26 * 6 && (last_pos + dist / 26) % 4 == pos as i64 {
                            self.synced = true;
                            self.since_sync_checked = false;
                            self.bad_in_window = 0;
                            self.blocks_in_window = 0;
                            self.valid = [false; 4];
                            self.accept(pos, (self.shift >> 10) as u16, true, false, groups);
                            self.expected = (pos + 1) % 4;
                            self.bits_in_block = 0;
                            return;
                        }
                    }
                    self.last_found_bit = self.bit_count;
                    self.last_found_offset = o;
                    break;
                }
            }
            return;
        }
        self.bits_in_block += 1;
        if self.bits_in_block < 26 {
            return;
        }
        self.bits_in_block = 0;
        let pos = self.expected;
        self.expected = (self.expected + 1) % 4;
        let s = syndrome(self.shift);
        let candidates: &[usize] = match pos {
            0 => &[0],
            1 => &[1],
            2 => &[2, 3],
            _ => &[4],
        };
        let mut data: Option<u16> = None;
        let mut corrected = false;
        for &o in candidates {
            if s == OFFSETS[o] {
                data = Some((self.shift >> 10) as u16);
                break;
            }
        }
        if data.is_none() {
            for &o in candidates {
                if let Some(&e) = self.correction.get(&(s ^ OFFSETS[o])) {
                    data = Some(((self.shift ^ e) >> 10) as u16);
                    corrected = true;
                    break;
                }
            }
        }
        let ok = data.is_some();
        self.blocks_in_window += 1;
        if !ok {
            self.bad_in_window += 1;
        }
        self.accept(pos, data.unwrap_or(0), ok, corrected, groups);
        if !self.since_sync_checked && self.blocks_in_window >= 10 {
            self.since_sync_checked = true;
            if self.bad_in_window >= 4 {
                self.synced = false;
                self.last_found_bit = -1;
                self.blocks_in_window = 0;
                self.bad_in_window = 0;
                return;
            }
        }
        if self.blocks_in_window >= 50 {
            if self.bad_in_window > 35 {
                self.synced = false;
                self.last_found_bit = -1;
            }
            self.blocks_in_window = 0;
            self.bad_in_window = 0;
        }
    }

    fn accept(&mut self, pos: usize, data: u16, ok: bool, corrected: bool, groups: &mut Vec<[u16; 4]>) {
        self.blocks_total += 1;
        if !ok {
            self.blocks_failed += 1;
        } else if corrected {
            self.blocks_corrected += 1;
        } else {
            self.blocks_ok += 1;
        }
        if self.recent.len() >= 400 {
            self.recent.pop_front();
        }
        self.recent.push_back(!ok);
        if pos == 0 {
            self.valid = [false; 4];
        }
        self.group[pos] = data;
        self.valid[pos] = ok;
        if pos == 3 {
            if self.valid.iter().all(|v| *v) {
                self.groups_total += 1;
                groups.push(self.group);
            }
            self.valid = [false; 4];
        }
    }
}

// ---------------------------------------------------------------- parser / station state

pub const PTY_RDS: [&str; 32] = [
    "None", "News", "Current Affairs", "Information", "Sport", "Education", "Drama", "Culture", "Science", "Varied", "Pop Music",
    "Rock Music", "Easy Listening", "Light Classical", "Serious Classical", "Other Music", "Weather", "Finance", "Children's",
    "Social Affairs", "Religion", "Phone-In", "Travel", "Leisure", "Jazz Music", "Country Music", "National Music", "Oldies Music",
    "Folk Music", "Documentary", "Alarm Test", "Alarm",
];

pub const PTY_RBDS: [&str; 32] = [
    "None", "News", "Information", "Sports", "Talk", "Rock", "Classic Rock", "Adult Hits", "Soft Rock", "Top 40", "Country",
    "Oldies", "Soft", "Nostalgia", "Jazz", "Classical", "Rhythm and Blues", "Soft R&B", "Language", "Religious Music",
    "Religious Talk", "Personality", "Public", "College", "Spanish Talk", "Spanish Music", "Hip Hop", "", "", "Weather",
    "Emergency Test", "Emergency",
];

#[derive(Clone, Debug, Default)]
pub struct EonStation {
    pub pi: u16,
    pub ps: String,
    pub af: Vec<f64>,
    pub tp: Option<bool>,
    pub ta: Option<bool>,
    pub pty: Option<u8>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct TmcMessage {
    pub event: u16,
    pub location: u16,
    pub extent: u8,
    pub direction: bool,
    pub single_group: bool,
    pub optional_fields: usize,
    pub seen: u32,
}

#[derive(Clone, Debug, Default)]
pub struct RdsStation {
    pub pi: Option<u16>,
    pub ps: String,
    pub ps_partial: String,
    pub pty: Option<u8>,
    pub tp: bool,
    pub ta: bool,
    pub music: bool,
    pub rt: String,
    pub rt_history: Vec<String>,
    pub af: Vec<f64>,
    pub af_complete: bool,
    pub clock: Option<String>,
    pub ecc: Option<u8>,
    pub language: Option<u8>,
    pub pin: Option<String>,
    pub ptyn: Option<String>,
    pub long_ps: Option<String>,
    pub rtplus_title: Option<String>,
    pub rtplus_artist: Option<String>,
    pub di: Option<u8>,
    pub oda: BTreeMap<String, u16>,
    pub eon: BTreeMap<u16, EonStation>,
    pub tmc: Vec<TmcMessage>,
    pub tmc_ltn: Option<u8>,
    pub group_counts: BTreeMap<String, u64>,
}

impl RdsStation {
    pub fn country_hint(&self) -> Option<&'static str> {
        let pi = self.pi?;
        let cc = (pi >> 12) as u8;
        let ecc = self.ecc?;
        country(ecc, cc)
    }
}

/// A few common ECC/country-code pairs (IEC 62106 Annex D), enough for Western Europe.
fn country(ecc: u8, cc: u8) -> Option<&'static str> {
    Some(match (ecc, cc) {
        (0xE3, 0x8) => "Netherlands",
        (0xE0, 0xD) | (0xE0, 0x1) => "Germany",
        (0xE0, 0x6) => "Belgium",
        (0xE1, 0xC) => "United Kingdom",
        (0xE1, 0xF) => "France",
        (0xE4, 0x7) => "Luxembourg",
        (0xE2, 0xC) => "Ireland",
        (0xE1, 0x4) => "Switzerland",
        (0xE0, 0xA) => "Austria",
        (0xE2, 0x5) => "Italy",
        (0xE2, 0xE) => "Spain",
        (0xE1, 0x9) => "Denmark",
        (0xE3, 0xF) => "Norway",
        (0xE3, 0xE) => "Sweden",
        (0xA0, 0x1) | (0xA0, 0x2) | (0xA0, 0x3) | (0xA0, 0x4) | (0xA0, 0x5) | (0xA0, 0x6) | (0xA0, 0x7) | (0xA0, 0x8) | (0xA0, 0x9) | (0xA0, 0xA) | (0xA0, 0xB) | (0xA0, 0xD) | (0xA0, 0xE) => {
            "USA"
        }
        _ => return None,
    })
}

const CHARSET_HIGH: [char; 128] = [
    'á', 'à', 'é', 'è', 'í', 'ì', 'ó', 'ò', 'ú', 'ù', 'Ñ', 'Ç', 'Ş', 'β', '¡', 'Ĳ', 'â', 'ä', 'ê', 'ë', 'î', 'ï', 'ô', 'ö', 'û', 'ü', 'ñ', 'ç', 'ş',
    'ğ', 'ı', 'ĳ', 'ª', 'α', '©', '‰', 'Ğ', 'ě', 'ň', 'ő', 'π', '€', '£', '$', '←', '↑', '→', '↓', 'º', '¹', '²', '³', '±', 'İ', 'ń', 'ű', 'µ',
    '¿', '÷', '°', '¼', '½', '¾', '§', 'Á', 'À', 'É', 'È', 'Í', 'Ì', 'Ó', 'Ò', 'Ú', 'Ù', 'Ř', 'Č', 'Š', 'Ž', 'Đ', 'Ŀ', 'Â', 'Ä', 'Ê', 'Ë', 'Î',
    'Ï', 'Ô', 'Ö', 'Û', 'Ü', 'ř', 'č', 'š', 'ž', 'đ', 'ŀ', 'Ã', 'Å', 'Æ', 'Œ', 'ŷ', 'Ý', 'Õ', 'Ø', 'Þ', 'Ŋ', 'Ŕ', 'Ć', 'Ś', 'Ź', 'Ŧ', 'ð', 'ã',
    'å', 'æ', 'œ', 'ŵ', 'ý', 'õ', 'ø', 'þ', 'ŋ', 'ŕ', 'ć', 'ś', 'ź', 'ŧ', ' ',
];

pub fn rds_char(code: u16) -> char {
    let code = (code & 0xFF) as u8;
    match code {
        0x20..=0x7D => code as char,
        0x7E => '¯',
        0x80..=0xFF => CHARSET_HIGH[(code - 0x80) as usize],
        _ => ' ',
    }
}

fn af_freq(code: u16) -> Option<f64> {
    if (1..=204).contains(&code) { Some(((87.5 + code as f64 * 0.1) * 10.0).round() / 10.0) } else { None }
}

pub struct GroupParser {
    pub st: RdsStation,
    ps: [char; 8],
    ps_mask: u8,
    rt: [char; 64],
    rt_mask: u64,
    rt_ab: i32,
    rt_end: i32,
    rt_is_b: bool,
    af_expected: usize,
    tmc_group: (u8, bool),
    rtplus_group: Option<(u8, bool)>,
    ptyn: [char; 8],
    ptyn_mask: u8,
    long_ps: [u8; 32],
    long_ps_mask: u8,
    eon_ps_mask: HashMap<u16, (u8, [char; 8], usize)>,
}

impl GroupParser {
    pub fn new() -> Self {
        Self {
            st: RdsStation::default(),
            ps: [' '; 8],
            ps_mask: 0,
            rt: [' '; 64],
            rt_mask: 0,
            rt_ab: -1,
            rt_end: -1,
            rt_is_b: false,
            af_expected: 0,
            tmc_group: (8, true),
            rtplus_group: None,
            ptyn: [' '; 8],
            ptyn_mask: 0,
            long_ps: [0x20; 32],
            long_ps_mask: 0,
            eon_ps_mask: HashMap::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn parse(&mut self, g: [u16; 4]) {
        let [a, b, c, d] = g;
        if self.st.pi != Some(a) {
            let counts = std::mem::take(&mut self.st.group_counts);
            self.reset();
            self.st.group_counts = counts;
            self.st.pi = Some(a);
        }
        let typ = (b >> 12) as u8;
        let version_a = (b >> 11) & 1 == 0;
        let name = format!("{}{}", typ, if version_a { 'A' } else { 'B' });
        *self.st.group_counts.entry(name).or_insert(0) += 1;
        self.st.tp = (b >> 10) & 1 == 1;
        self.st.pty = Some(((b >> 5) & 0x1F) as u8);
        match (typ, version_a) {
            (0, _) => self.group0(b, c, d, version_a),
            (1, true) => self.group1a(c, d),
            (2, _) => self.group2(b, c, d, version_a),
            (3, true) => self.group3a(b, c, d),
            (4, true) => self.group4a(b, c, d),
            (10, true) => self.group10a(b, c, d),
            (14, _) => self.group14(b, c, d, version_a),
            (15, false) => {
                self.st.ta = (b >> 4) & 1 == 1;
                self.st.music = (b >> 3) & 1 == 1;
            }
            (15, true) => self.group15a(b, c, d),
            _ => {}
        }
        if (typ, version_a) == self.tmc_group && typ != 0 && typ != 2 && typ != 14 {
            self.group_tmc(b, c, d);
        }
        if Some((typ, version_a)) == self.rtplus_group && typ != 0 && typ != 2 {
            self.group_rtplus(b, c, d);
        }
    }

    fn group0(&mut self, b: u16, c: u16, d: u16, version_a: bool) {
        self.st.ta = (b >> 4) & 1 == 1;
        self.st.music = (b >> 3) & 1 == 1;
        let seg = (b & 3) as usize;
        let di_bit = ((b >> 2) & 1) as u8;
        let di = (self.st.di.unwrap_or(0) & !(1 << (3 - seg))) | (di_bit << (3 - seg));
        self.st.di = Some(di);
        self.ps[seg * 2] = rds_char(d >> 8);
        self.ps[seg * 2 + 1] = rds_char(d);
        self.ps_mask |= 1 << seg;
        self.st.ps_partial = self.ps.iter().collect();
        if self.ps_mask == 0xF {
            self.st.ps = self.ps.iter().collect();
            self.ps_mask = 0;
        }
        if version_a {
            self.af_code(c >> 8);
            self.af_code(c & 0xFF);
        }
    }

    fn af_code(&mut self, code: u16) {
        match code {
            225..=249 => {
                let n = (code - 224) as usize;
                if n != self.af_expected || self.st.af.len() > n {
                    self.st.af.clear();
                    self.st.af_complete = false;
                }
                self.af_expected = n;
            }
            1..=204 => {
                if self.af_expected == 0 {
                    return;
                }
                let f = af_freq(code).unwrap();
                if !self.st.af.contains(&f) {
                    self.st.af.push(f);
                }
                if self.st.af.len() > self.af_expected {
                    self.st.af.clear();
                }
                self.st.af_complete = self.st.af.len() >= self.af_expected;
            }
            _ => {}
        }
    }

    fn group1a(&mut self, c: u16, d: u16) {
        match (c >> 12) & 7 {
            0 => self.st.ecc = Some((c & 0xFF) as u8),
            3 => self.st.language = Some((c & 0xFF) as u8),
            _ => {}
        }
        if d != 0 {
            let day = d >> 11;
            let hour = (d >> 6) & 0x1F;
            let min = d & 0x3F;
            self.st.pin = Some(format!("day {day}, {hour:02}:{min:02}"));
        }
    }

    fn group2(&mut self, b: u16, c: u16, d: u16, version_a: bool) {
        let ab = ((b >> 4) & 1) as i32;
        let seg = (b & 0xF) as usize;
        if ab != self.rt_ab || version_a == self.rt_is_b {
            self.rt = [' '; 64];
            self.rt_mask = 0;
            self.rt_end = -1;
            self.rt_ab = ab;
            self.rt_is_b = !version_a;
        }
        let chars: Vec<u16> = if version_a { vec![c >> 8, c & 0xFF, d >> 8, d & 0xFF] } else { vec![d >> 8, d & 0xFF] };
        let base = seg * chars.len();
        for (k, &code) in chars.iter().enumerate() {
            let idx = base + k;
            if code == 0x0D {
                if self.rt_end < 0 || (idx as i32) < self.rt_end {
                    self.rt_end = idx as i32;
                }
                continue;
            }
            if idx < 64 {
                self.rt[idx] = rds_char(code);
            }
        }
        self.rt_mask |= 1 << seg;
        let seg_len = if self.rt_is_b { 2 } else { 4 };
        let max_len = 16 * seg_len;
        let len = if self.rt_end >= 0 && (self.rt_end as usize) < max_len { self.rt_end as usize } else { max_len };
        let needed = len.div_ceil(seg_len);
        let need_mask = if needed >= 64 { u64::MAX } else { (1u64 << needed) - 1 };
        if needed > 0 && self.rt_mask & need_mask == need_mask {
            let text: String = self.rt[..len].iter().collect::<String>().trim_end().to_string();
            if !text.is_empty() && text != self.st.rt {
                self.st.rt = text.clone();
                self.st.rt_history.retain(|t| t != &text);
                self.st.rt_history.insert(0, text);
                self.st.rt_history.truncate(20);
            }
        }
    }

    fn group3a(&mut self, b: u16, c: u16, d: u16) {
        let app_group = ((b >> 1) & 0xF) as u8;
        let app_a = b & 1 == 0;
        let aid = d;
        let gname = format!("{}{}", app_group, if app_a { 'A' } else { 'B' });
        let label = match aid {
            0xCD46 | 0xCD47 => "TMC (ALERT-C)",
            0x4BD7 => "RadioText+",
            0x0093 => "DAB cross-reference",
            0xE911 => "EAS",
            0x6552 => "Enhanced RadioText",
            _ => "ODA",
        };
        self.st.oda.insert(format!("{label} ({aid:04X}) in {gname}"), aid);
        match aid {
            0xCD46 | 0xCD47 => {
                if app_group != 0 {
                    self.tmc_group = (app_group, app_a);
                }
                if (c >> 14) & 3 == 0 {
                    self.st.tmc_ltn = Some(((c >> 6) & 0x3F) as u8);
                }
            }
            0x4BD7 => self.rtplus_group = Some((app_group, app_a)),
            _ => {}
        }
    }

    fn group4a(&mut self, b: u16, c: u16, d: u16) {
        let mjd = (((b & 3) as u32) << 15) | (c as u32 >> 1);
        let hour = (((c & 1) as u32) << 4) | (d as u32 >> 12);
        let minute = (d as u32 >> 6) & 0x3F;
        let sign = if (d >> 5) & 1 == 1 { -1.0 } else { 1.0 };
        let offset_h = sign * (d & 0x1F) as f64 / 2.0;
        if mjd == 0 || hour > 23 || minute > 59 {
            return;
        }
        let mjdf = mjd as f64;
        let yp = ((mjdf - 15078.2) / 365.25) as i64;
        let mp = ((mjdf - 14956.1 - (yp as f64 * 365.25).floor()) / 30.6001) as i64;
        let day = mjd as i64 - 14956 - (yp as f64 * 365.25) as i64 - (mp as f64 * 30.6001) as i64;
        let k = if mp == 14 || mp == 15 { 1 } else { 0 };
        let year = 1900 + yp + k;
        let month = mp - 1 - k * 12;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return;
        }
        let mut local = (hour * 60 + minute) as i64 + (offset_h * 60.0) as i64;
        local = local.rem_euclid(1440);
        self.st.clock = Some(format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC (local {:02}:{:02}, UTC{offset_h:+.1})",
            local / 60,
            local % 60
        ));
    }

    fn group10a(&mut self, b: u16, c: u16, d: u16) {
        let seg = (b & 1) as usize;
        for (k, code) in [c >> 8, c & 0xFF, d >> 8, d & 0xFF].into_iter().enumerate() {
            self.ptyn[seg * 4 + k] = rds_char(code);
        }
        self.ptyn_mask |= 1 << seg;
        if self.ptyn_mask == 3 {
            self.st.ptyn = Some(self.ptyn.iter().collect::<String>().trim().to_string());
        }
    }

    fn group15a(&mut self, b: u16, c: u16, d: u16) {
        let seg = (b & 7) as usize;
        for (k, v) in [c >> 8, c & 0xFF, d >> 8, d & 0xFF].into_iter().enumerate() {
            self.long_ps[seg * 4 + k] = v as u8;
        }
        self.long_ps_mask |= 1 << seg;
        let end = self.long_ps.iter().position(|&x| x == 0x0D).unwrap_or(32);
        let needed = end.div_ceil(4);
        if needed > 0 && (self.long_ps_mask as u32 & ((1u32 << needed) - 1)) == (1u32 << needed) - 1 {
            self.st.long_ps = Some(String::from_utf8_lossy(&self.long_ps[..end]).trim().to_string());
        }
    }

    fn group_rtplus(&mut self, b: u16, c: u16, d: u16) {
        let bits = (((b & 0x1F) as u64) << 32) | ((c as u64) << 16) | d as u64;
        let t1 = ((bits >> 29) & 0x3F) as usize;
        let s1 = ((bits >> 23) & 0x3F) as usize;
        let l1 = ((bits >> 17) & 0x3F) as usize;
        let t2 = ((bits >> 11) & 0x3F) as usize;
        let s2 = ((bits >> 5) & 0x3F) as usize;
        let l2 = (bits & 0x1F) as usize;
        let text: Vec<char> = self.st.rt.chars().collect();
        for (t, s, l) in [(t1, s1, l1), (t2, s2, l2)] {
            if t == 0 {
                continue;
            }
            let end = (s + l + 1).min(text.len());
            if s >= end {
                continue;
            }
            let v: String = text[s..end].iter().collect::<String>().trim().to_string();
            match t {
                1 => self.st.rtplus_title = Some(v),
                4 => self.st.rtplus_artist = Some(v),
                _ => {}
            }
        }
    }

    fn group_tmc(&mut self, b: u16, c: u16, d: u16) {
        let t_flag = (b >> 4) & 1;
        let f_flag = (b >> 3) & 1;
        if t_flag == 1 {
            return;
        }
        let first = f_flag == 1 || (c >> 15) & 1 == 1;
        if !first {
            return; // continuation groups only carry optional fields
        }
        let direction = (c >> 14) & 1 == 1;
        let extent = ((c >> 11) & 7) as u8;
        let event = c & 0x7FF;
        let location = d;
        if let Some(m) = self.st.tmc.iter_mut().find(|m| m.event == event && m.location == location && m.direction == direction) {
            m.seen += 1;
        } else {
            self.st.tmc.insert(0, TmcMessage { event, location, extent, direction, single_group: f_flag == 1, optional_fields: 0, seen: 1 });
            self.st.tmc.truncate(200);
        }
    }

    fn group14(&mut self, b: u16, c: u16, d: u16, version_a: bool) {
        let pi_on = d;
        if pi_on == 0 || Some(pi_on) == self.st.pi {
            return;
        }
        let e = self.st.eon.entry(pi_on).or_insert_with(|| EonStation { pi: pi_on, ..Default::default() });
        e.tp = Some((b >> 4) & 1 == 1);
        if !version_a {
            e.ta = Some((b >> 3) & 1 == 1);
            return;
        }
        let variant = (b & 0xF) as usize;
        let ps_state = self.eon_ps_mask.entry(pi_on).or_insert((0, [' '; 8], 0));
        match variant {
            0..=3 => {
                ps_state.1[variant * 2] = rds_char(c >> 8);
                ps_state.1[variant * 2 + 1] = rds_char(c);
                ps_state.0 |= 1 << variant;
                if ps_state.0 == 0xF {
                    e.ps = ps_state.1.iter().collect();
                }
            }
            4 => {
                for code in [c >> 8, c & 0xFF] {
                    match code {
                        225..=249 => {
                            let n = (code - 224) as usize;
                            if n != ps_state.2 {
                                e.af.clear();
                            }
                            ps_state.2 = n;
                        }
                        1..=204 => {
                            if ps_state.2 > 0 {
                                let f = af_freq(code).unwrap();
                                if !e.af.contains(&f) {
                                    e.af.push(f);
                                }
                                if e.af.len() > ps_state.2 {
                                    e.af.clear();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            13 => {
                e.pty = Some((c >> 11) as u8);
                e.ta = Some(c & 1 == 1);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- decoder (all three)

pub struct RdsDecoder {
    pub demod: RdsDemodulator,
    pub sync: BlockSync,
    pub parser: GroupParser,
    bits: Vec<u8>,
    groups: Vec<[u16; 4]>,
}

impl RdsDecoder {
    pub fn new(rate: f64) -> Self {
        Self { demod: RdsDemodulator::new(rate), sync: BlockSync::new(), parser: GroupParser::new(), bits: Vec::new(), groups: Vec::new() }
    }
    pub fn process(&mut self, mpx: &[f32]) -> usize {
        self.bits.clear();
        self.demod.process(mpx, &mut self.bits);
        self.groups.clear();
        for i in 0..self.bits.len() {
            let b = self.bits[i];
            self.sync.push_bit(b, &mut self.groups);
        }
        for g in self.groups.drain(..) {
            self.parser.parse(g);
        }
        self.bits.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds the 104-bit stream of a group.
    fn group_bits(g: [u16; 4], c_prime: bool) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, &w) in g.iter().enumerate() {
            let ofs = match i {
                0 => OFFSETS[0],
                1 => OFFSETS[1],
                2 => {
                    if c_prime {
                        OFFSETS[3]
                    } else {
                        OFFSETS[2]
                    }
                }
                _ => OFFSETS[4],
            };
            let block = ((w as u32) << 10) | (checkword(w as u32) ^ ofs);
            for k in (0..26).rev() {
                out.push(((block >> k) & 1) as u8);
            }
        }
        out
    }

    fn ps_groups(pi: u16, ps: &str) -> Vec<[u16; 4]> {
        let b: Vec<u8> = ps.bytes().collect();
        (0..4).map(|seg| [pi, (0 << 12) | (10 << 5) | seg as u16, 0xE0CD, ((b[seg * 2] as u16) << 8) | b[seg * 2 + 1] as u16]).collect()
    }

    #[test]
    fn sync_and_parse_ps() {
        let mut sync = BlockSync::new();
        let mut parser = GroupParser::new();
        let mut groups = Vec::new();
        for _ in 0..3 {
            for g in ps_groups(0x8201, "NPO 2   ") {
                for bit in group_bits(g, false) {
                    sync.push_bit(bit, &mut groups);
                }
            }
        }
        for g in groups.drain(..) {
            parser.parse(g);
        }
        assert!(sync.synced);
        assert_eq!(parser.st.ps, "NPO 2   ");
        assert_eq!(parser.st.pi, Some(0x8201));
        assert_eq!(parser.st.pty, Some(10));
    }

    #[test]
    fn corrects_burst_error() {
        let mut sync = BlockSync::new();
        let mut groups = Vec::new();
        let mut stream = Vec::new();
        for _ in 0..4 {
            for g in ps_groups(0x8201, "RADIO 1 ") {
                stream.extend(group_bits(g, false));
            }
        }
        // flip two adjacent bits in the last group's block C
        let n = stream.len();
        stream[n - 104 + 52 + 5] ^= 1;
        stream[n - 104 + 52 + 6] ^= 1;
        for bit in stream {
            sync.push_bit(bit, &mut groups);
        }
        assert!(sync.blocks_corrected >= 1);
    }

    /// End-to-end through the demodulator: an MPX with an RDS subcarrier.
    #[test]
    fn demodulates_rds_subcarrier() {
        let rate = 192_000.0;
        let mut bits = Vec::new();
        for _ in 0..12 {
            for g in ps_groups(0x83C2, "SKY RADI") {
                bits.extend(group_bits(g, false));
            }
        }
        // differential encoding
        let mut enc = Vec::with_capacity(bits.len());
        let mut prev = 0u8;
        for b in &bits {
            prev ^= b;
            enc.push(prev);
        }
        let spb = rate / 1187.5;
        let n = (enc.len() as f64 * spb) as usize;
        let mpx: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / rate;
                let bitpos = i as f64 / spb;
                let k = bitpos as usize;
                let sym = if enc[k.min(enc.len() - 1)] == 1 { 1.0 } else { -1.0 };
                // biphase: + first half, - second half
                let half = if bitpos - (k as f64) < 0.5 { 1.0 } else { -1.0 };
                let carrier = (2.0 * PI * 57_000.0 * t).sin();
                let pilot = 0.09 * (2.0 * PI * 19_000.0 * t).sin();
                (0.04 * sym * half * carrier + pilot + 0.3 * (2.0 * PI * 1000.0 * t).sin()) as f32
            })
            .collect();
        let mut dec = RdsDecoder::new(rate);
        for chunk in mpx.chunks(8192) {
            dec.process(chunk);
        }
        assert_eq!(dec.parser.st.ps, "SKY RADI", "synced {} groups {}", dec.sync.synced, dec.sync.groups_total);
    }
}

//! FM band scan: a quick spectrum survey of the band finds the carriers, then each one is tuned in
//! WFM to check stereo and collect its RDS (PI, PS, PTY, TP/TA, ECC/country, AF, RT...). The
//! results become the FM presets list.

use crate::app::RfaApp;
use crate::settings::{Mode, SourceKind};
use serde::{Deserialize, Serialize};

/// One station found by the scan (or kept from an earlier one).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FmPreset {
    pub freq: u64,
    /// user's own name; empty means "use the PS"
    pub name: String,
    pub level_db: f32,
    pub snr_db: f32,
    pub stereo: bool,
    pub pilot_db: f32,
    pub rds: bool,
    pub pi: Option<u16>,
    pub ps: String,
    pub long_ps: Option<String>,
    pub pty: Option<u8>,
    pub ptyn: Option<String>,
    pub tp: bool,
    pub ta: bool,
    pub music: bool,
    pub ecc: Option<u8>,
    pub country: Option<String>,
    pub af: Vec<f64>,
    pub rt: String,
    pub bler: Option<f32>,
    pub groups_per_sec: f32,
    pub rtplus: bool,
    pub eon: usize,
    pub tmc: bool,
    pub clock: bool,
    pub oda: Vec<String>,
    /// seconds until the full PS was received
    pub ps_secs: Option<f32>,
    pub scanned: String,
}

impl FmPreset {
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else if !self.ps.trim().is_empty() {
            self.ps.trim().to_string()
        } else {
            format!("{:.1} MHz", self.freq as f64 / 1e6)
        }
    }
}

/// Settings of the scan (kept in Settings).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FmScanConfig {
    pub start_hz: u64,
    pub end_hz: u64,
    pub step_hz: u64,
    /// a channel counts as a carrier when it is this far above the noise floor
    pub threshold_db: f32,
    /// longest time spent on one station waiting for RDS
    pub max_secs: f32,
}

impl Default for FmScanConfig {
    fn default() -> Self {
        Self { start_hz: 87_500_000, end_hz: 108_000_000, step_hz: 100_000, threshold_db: 10.0, max_secs: 10.0 }
    }
}

pub enum FmScanPhase {
    Survey { window: usize, t0: f64, frames: usize },
    Verify { index: usize, t0: f64, pilot_seen: bool, rds_seen: bool, ps_at: Option<f64>, level_sum: f64, level_n: u32 },
}

pub struct FmScan {
    pub cfg: FmScanConfig,
    pub grid: Vec<u64>,
    pub levels: Vec<f32>,
    pub centers: Vec<u64>,
    pub rate: f64,
    pub floor_db: f32,
    pub candidates: Vec<usize>,
    pub phase: FmScanPhase,
    pub found: Vec<FmPreset>,
    pub prev: (Mode, u64, u64, bool, bool),
    pub started: f64,
}

#[derive(Default)]
pub struct FmScanStatus {
    pub running: bool,
    pub stage: String,
    pub progress: f32,
    pub log: Vec<String>,
    pub finished: bool,
}

/// Mean power (dB) in +-50 kHz around `f` in a spectrum frame, or None if `f` is outside the usable
/// part of the frame (band edges, or the DC spike at the centre for live sources).
pub fn level_at(db: &[f32], center: f64, rate: f64, f: f64, dc_excl: f64, edge: f64) -> Option<f32> {
    let off = f - center;
    if off.abs() < dc_excl || off.abs() > edge {
        return None;
    }
    let n = db.len() as f64;
    let bin = (off / rate + 0.5) * n;
    let hw = (50_000.0 / rate * n).max(1.0);
    let a = (bin - hw).floor().max(0.0) as usize;
    let b = ((bin + hw).ceil() as usize).min(db.len() - 1);
    if a >= b {
        return None;
    }
    let p: f64 = db[a..=b].iter().map(|&d| 10f64.powf(d as f64 / 10.0)).sum::<f64>() / (b - a + 1) as f64;
    Some((10.0 * p.max(1e-20).log10()) as f32)
}

/// Noise floor (20th percentile of the measured channels) and the channels that are local maxima
/// at least `threshold_db` above it.
pub fn pick_candidates(levels: &[f32], threshold_db: f32) -> (f32, Vec<usize>) {
    let mut v: Vec<f32> = levels.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return (f32::NEG_INFINITY, Vec::new());
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor = v[v.len() / 5];
    let mut out = Vec::new();
    for i in 0..levels.len() {
        let l = levels[i];
        if !l.is_finite() || l < floor + threshold_db {
            continue;
        }
        let left = if i > 0 { levels[i - 1] } else { f32::NEG_INFINITY };
        let right = levels.get(i + 1).copied().unwrap_or(f32::NEG_INFINITY);
        if l >= left && l > right {
            out.push(i);
        }
    }
    (floor, out)
}

impl RfaApp {
    pub fn start_fm_scan(&mut self) {
        if self.scan.is_some() {
            self.stop_scan();
        }
        let prev = (self.s.mode, self.s.channel_freq, self.s.source.frequency, self.s.muted, self.s.rds_enabled);
        if self.s.mode != Mode::Wfm {
            self.set_mode(Mode::Wfm);
        }
        self.s.rds_enabled = true;
        self.s.muted = true;
        if !self.running {
            self.start();
        }
        let cfg = self.s.fm_scan.clone();
        let step = cfg.step_hz.max(10_000);
        let grid: Vec<u64> = (0..).map(|i| cfg.start_hz + i * step).take_while(|&f| f <= cfg.end_hz).collect();
        let rate = self.s.source.sample_rate as f64;
        let centers = if self.s.source.kind == SourceKind::File {
            vec![self.s.source.frequency]
        } else {
            // windows 0.35*rate apart; each channel is measured 0.06..0.42*rate from some centre
            let w = 0.35 * rate;
            let mut c = cfg.start_hz as f64 - 0.2 * rate;
            let mut v = Vec::new();
            while c < cfg.end_hz as f64 + 0.42 * rate {
                v.push(((c / 1000.0).round() * 1000.0) as u64);
                c += w;
            }
            v
        };
        self.fm_scan_status = FmScanStatus { running: true, stage: "Surveying the band".into(), ..Default::default() };
        let levels = vec![f32::NEG_INFINITY; grid.len()];
        fm_log(&format!(
            "=== FM scan {:.1}-{:.1} MHz, step {} kHz, source {:?} at {} sps, {} windows",
            cfg.start_hz as f64 / 1e6,
            cfg.end_hz as f64 / 1e6,
            step / 1000,
            self.s.source.kind,
            self.s.source.sample_rate,
            centers.len()
        ));
        let now = self.now();
        let first = centers[0];
        self.fm_scan = Some(FmScan {
            cfg,
            grid,
            levels,
            centers,
            rate,
            floor_db: 0.0,
            candidates: Vec::new(),
            phase: FmScanPhase::Survey { window: 0, t0: now, frames: 0 },
            found: Vec::new(),
            prev,
            started: now,
        });
        self.fm_tune_window(first, rate);
    }

    pub fn stop_fm_scan(&mut self, finished: bool) {
        let Some(sc) = self.fm_scan.take() else { return };
        let (mode, ch, src, muted, rds) = sc.prev;
        self.s.muted = muted;
        self.s.rds_enabled = rds || finished;
        if finished && !sc.found.is_empty() {
            let mut found = sc.found;
            // keep the names the user gave stations that were found again
            for f in found.iter_mut() {
                if let Some(old) = self.s.fm_presets.iter().find(|p| p.freq == f.freq && !p.name.is_empty()) {
                    f.name = old.name.clone();
                }
            }
            let n = found.len();
            let n_rds = found.iter().filter(|p| p.rds).count();
            self.s.fm_presets = found;
            let msg = format!("FM scan finished: {n} stations, {n_rds} with RDS ({:.0} s)", self.now() - sc.started);
            fm_log(&format!("=== {msg}"));
            self.fm_scan_status.log.push(msg.clone());
            self.toast(msg);
        } else if finished {
            fm_log("=== FM scan finished: no stations found");
            self.toast("FM scan finished: no stations found. Check the antenna and gain.");
        }
        self.fm_scan_status.running = false;
        self.fm_scan_status.finished = finished;
        // back to where we were
        if mode != Mode::Wfm {
            self.set_mode(mode);
        }
        if self.s.source.kind != SourceKind::File {
            self.s.source.frequency = src;
        }
        self.set_channel(ch);
        if self.auto_quit && self.auto_scans == 0 {
            self.auto_next_at = -1.0;
        }
    }

    fn fm_tune_window(&mut self, c: u64, rate: f64) {
        if self.s.source.kind == SourceKind::File {
            return;
        }
        self.s.source.frequency = c;
        // keep the demodulator inside the band so nothing retunes the source
        self.s.channel_freq = c + (0.2 * rate) as u64;
    }

    /// Feeds spectrum frames to the survey (called with every batch from the engine).
    pub fn fm_scan_frames(&mut self, frames: &[crate::engine::SpectrumFrame]) {
        let file = self.s.source.kind == SourceKind::File;
        let now = self.now();
        let Some(sc) = self.fm_scan.as_mut() else { return };
        let FmScanPhase::Survey { window, t0, frames: count } = &mut sc.phase else { return };
        if now - *t0 < 0.35 {
            return;
        }
        let c = sc.centers[*window] as f64;
        let (dc, edge) = if file { (0.0, 0.45 * sc.rate) } else { (0.06 * sc.rate, 0.42 * sc.rate) };
        for fr in frames {
            if (fr.center - c).abs() > 500.0 || (fr.rate - sc.rate).abs() > 1.0 {
                continue;
            }
            for (k, &g) in sc.grid.iter().enumerate() {
                if let Some(l) = level_at(&fr.db, fr.center, fr.rate, g as f64, dc, edge) {
                    if l > sc.levels[k] {
                        sc.levels[k] = l;
                    }
                }
            }
            *count += 1;
        }
    }

    /// Advances the scan; called once per UI frame.
    pub fn fm_scan_step(&mut self) {
        let now = self.now();
        let Some(mut sc) = self.fm_scan.take() else { return };
        let finished = self.fm_scan_advance(&mut sc, now);
        self.fm_scan = Some(sc);
        if finished {
            self.stop_fm_scan(true);
        }
    }

    /// One step of the scan state machine; true when the scan is complete.
    fn fm_scan_advance(&mut self, sc: &mut FmScan, now: f64) -> bool {
        match &mut sc.phase {
            FmScanPhase::Survey { window, t0, frames } => {
                let enough = *frames >= 6 && now - *t0 > 0.8;
                if !(enough || now - *t0 > 4.0) {
                    return false;
                }
                let next = *window + 1;
                if next < sc.centers.len() {
                    sc.phase = FmScanPhase::Survey { window: next, t0: now, frames: 0 };
                    self.fm_scan_status.progress = 0.3 * next as f32 / sc.centers.len() as f32;
                    self.fm_tune_window(sc.centers[next], sc.rate);
                    return false;
                }
                let (floor, cands) = pick_candidates(&sc.levels, sc.cfg.threshold_db);
                sc.floor_db = floor;
                sc.candidates = cands;
                let msg = format!(
                    "Survey: noise floor {:.1} dB, {} carriers above {:.0} dB: {}",
                    floor,
                    sc.candidates.len(),
                    sc.cfg.threshold_db,
                    sc.candidates.iter().map(|&i| format!("{:.1}", sc.grid[i] as f64 / 1e6)).collect::<Vec<_>>().join(" ")
                );
                fm_log(&msg);
                self.fm_scan_status.log.push(msg);
                if sc.candidates.is_empty() {
                    return true;
                }
                sc.phase = FmScanPhase::Verify { index: 0, t0: now, pilot_seen: false, rds_seen: false, ps_at: None, level_sum: 0.0, level_n: 0 };
                let f = sc.grid[sc.candidates[0]];
                self.fm_scan_status.stage = format!("Checking {:.1} MHz", f as f64 / 1e6);
                self.set_channel(f);
                false
            }
            FmScanPhase::Verify { index, t0, pilot_seen, rds_seen, ps_at, level_sum, level_n } => {
                let t = now - *t0;
                if t < 0.8 {
                    return false; // let the engine retune and drop the previous station's RDS
                }
                if self.fm.pilot_locked {
                    *pilot_seen = true;
                }
                let r = self.rds.clone();
                if r.as_ref().map(|r| r.pi.is_some()).unwrap_or(false) || self.rds_stats.synced {
                    *rds_seen = true;
                }
                if ps_at.is_none() && r.as_ref().map(|r| !r.ps.trim().is_empty()).unwrap_or(false) {
                    *ps_at = Some(t);
                }
                if self.channel_db.is_finite() {
                    *level_sum += self.channel_db as f64;
                    *level_n += 1;
                }
                let max = sc.cfg.max_secs as f64;
                let done = (!*rds_seen && t >= 3.0) || ps_at.map(|p| t >= p + 2.0).unwrap_or(false) || t >= max;
                if !done {
                    return false;
                }
                let f = sc.grid[sc.candidates[*index]];
                let level = if *level_n > 0 { (*level_sum / *level_n as f64) as f32 } else { sc.levels[sc.candidates[*index]] };
                let survey_level = sc.levels[sc.candidates[*index]];
                let mut p = FmPreset {
                    freq: f,
                    level_db: level,
                    snr_db: survey_level - sc.floor_db,
                    stereo: *pilot_seen || self.fm.stereo,
                    pilot_db: self.fm.pilot_db,
                    rds: *rds_seen,
                    ps_secs: ps_at.map(|x| x as f32),
                    scanned: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    ..Default::default()
                };
                if let Some(r) = r.filter(|_| *rds_seen) {
                    p.pi = r.pi;
                    p.ps = if r.ps.trim().is_empty() { r.ps_partial.trim().to_string() } else { r.ps.trim().to_string() };
                    p.long_ps = r.long_ps.clone();
                    p.pty = r.pty;
                    p.ptyn = r.ptyn.clone();
                    p.tp = r.tp;
                    p.ta = r.ta;
                    p.music = r.music;
                    p.ecc = r.ecc;
                    p.country = r.country_hint().map(|s| s.to_string());
                    p.af = r.af.clone();
                    p.rt = r.rt.trim().to_string();
                    p.rtplus = r.rtplus_title.is_some() || r.rtplus_artist.is_some();
                    p.eon = r.eon.len();
                    p.tmc = !r.tmc.is_empty() || r.tmc_ltn.is_some();
                    p.clock = r.clock.is_some();
                    p.oda = r.oda.keys().cloned().collect();
                    p.bler = Some(self.rds_stats.bler);
                    p.groups_per_sec = self.rds_stats.groups_per_sec;
                }
                let line = format!(
                    "{:6.1} MHz  {:8}  PI {:4}  {}  {}  level {:.1} dB  SNR {:.1} dB  {}{}{}",
                    f as f64 / 1e6,
                    if p.ps.is_empty() { "-" } else { &p.ps },
                    p.pi.map(|x| format!("{x:04X}")).unwrap_or_else(|| "-".into()),
                    if p.stereo { "stereo" } else { "mono  " },
                    if p.rds { format!("RDS BLER {:.0}%", p.bler.unwrap_or(1.0) * 100.0) } else { "no RDS".into() },
                    p.level_db,
                    p.snr_db,
                    p.pty.map(|x| format!("PTY {x} ")).unwrap_or_default(),
                    if p.tp { "TP " } else { "" },
                    if p.rt.is_empty() { String::new() } else { format!("RT \"{}\"", p.rt) },
                );
                fm_log(&line);
                self.fm_scan_status.log.push(line);
                sc.found.push(p);
                let next = *index + 1;
                self.fm_scan_status.progress = 0.3 + 0.7 * next as f32 / sc.candidates.len() as f32;
                if next >= sc.candidates.len() {
                    return true;
                }
                sc.phase = FmScanPhase::Verify { index: next, t0: now, pilot_seen: false, rds_seen: false, ps_at: None, level_sum: 0.0, level_n: 0 };
                let f = sc.grid[sc.candidates[next]];
                self.fm_scan_status.stage = format!("Checking {:.1} MHz ({} of {})", f as f64 / 1e6, next + 1, sc.candidates.len());
                self.set_channel(f);
                false
            }
        }
    }

    pub fn tune_preset(&mut self, freq: u64) {
        if self.fm_scan.is_some() {
            self.stop_fm_scan(false);
        }
        if self.s.mode != Mode::Wfm {
            self.set_mode(Mode::Wfm);
        }
        self.set_channel(freq);
        if !self.running {
            self.start();
        }
    }
}

/// Presets as CSV (one row per station).
pub fn presets_csv(list: &[FmPreset]) -> String {
    let mut s = String::from("freq_mhz,name,ps,pi,pty,stereo,rds,tp,ta,country,ecc,level_db,snr_db,bler_pct,groups_per_s,af_mhz,rt,ptyn,long_ps,rtplus,eon,tmc,clock,oda,scanned\n");
    let q = |x: &str| format!("\"{}\"", x.replace('"', "\"\""));
    for p in list {
        s.push_str(&format!(
            "{:.2},{},{},{},{},{},{},{},{},{},{},{:.1},{:.1},{},{:.1},{},{},{},{},{},{},{},{},{},{}\n",
            p.freq as f64 / 1e6,
            q(&p.name),
            q(&p.ps),
            p.pi.map(|x| format!("{x:04X}")).unwrap_or_default(),
            p.pty.map(|x| x.to_string()).unwrap_or_default(),
            p.stereo,
            p.rds,
            p.tp,
            p.ta,
            q(p.country.as_deref().unwrap_or("")),
            p.ecc.map(|x| format!("{x:02X}")).unwrap_or_default(),
            p.level_db,
            p.snr_db,
            p.bler.map(|b| format!("{:.1}", b * 100.0)).unwrap_or_default(),
            p.groups_per_sec,
            q(&p.af.iter().map(|f| format!("{f:.1}")).collect::<Vec<_>>().join(" ")),
            q(&p.rt),
            q(p.ptyn.as_deref().unwrap_or("")),
            q(p.long_ps.as_deref().unwrap_or("")),
            p.rtplus,
            p.eon,
            p.tmc,
            p.clock,
            q(&p.oda.join(" ")),
            p.scanned
        ));
    }
    s
}

fn fm_log(line: &str) {
    use std::io::Write;
    let Some(dir) = crate::config_dir() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("fm_scan_log.txt")) {
        let _ = writeln!(f, "{} {}", chrono::Local::now().format("%H:%M:%S"), line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_local_maxima_above_floor() {
        // floor ~ -90, a station at index 5 with skirts, another at 12
        let mut l = vec![-90.0f32; 20];
        l[4] = -75.0;
        l[5] = -60.0;
        l[6] = -74.0;
        l[12] = -70.0;
        l[13] = -70.0; // flat top: only one is kept
        let (floor, c) = pick_candidates(&l, 10.0);
        assert!((floor + 90.0).abs() < 0.01);
        assert_eq!(c, vec![5, 13]);
    }

    #[test]
    fn level_in_frame() {
        let n = 1024;
        let rate = 2_048_000.0;
        let mut db = vec![-100.0f32; n];
        // carrier 300 kHz above centre
        let bin = ((300_000.0 / rate + 0.5) * n as f64) as usize;
        for b in bin - 20..=bin + 20 {
            db[b] = -40.0;
        }
        let l = level_at(&db, 100e6, rate, 100.3e6, 0.0, 0.45 * rate).unwrap();
        assert!(l > -42.0, "{l}");
        let q = level_at(&db, 100e6, rate, 100.6e6, 0.0, 0.45 * rate).unwrap();
        assert!(q < -95.0, "{q}");
        assert!(level_at(&db, 100e6, rate, 100.05e6, 0.06 * rate, 0.45 * rate).is_none());
    }
}

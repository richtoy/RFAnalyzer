//! The application: window layout, settings, and the link between the UI and the engine.

use crate::audio::{AudioOut, AudioShared};
use crate::dab::model::{self, DabScanState, DabState, SavedEnsemble};
use crate::engine::{Cmd, DemodParams, Engine, FmStats, RdsStats, RecordingInfo};
use crate::rds::RdsStation;
use crate::settings::{Mode, Settings, SourceConfig, SourceKind};
use crate::ui::freq::{frequency_display, parse_frequency};
use crate::ui::spectrum::{Action, SpectrumView, ViewParams};
use eframe::egui::{self, Color32, RichText};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SideTab {
    Source,
    Demod,
    Display,
    Recording,
    Bookmarks,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScanPhase {
    Tuning,
    WaitSync,
    WaitInfo,
    Collect,
}

#[allow(dead_code)]
pub struct Scan {
    pub channels: Vec<model::DabChannel>,
    pub index: usize,
    pub phase: ScanPhase,
    pub t0: f64,
    pub previous_channel: String,
    /// false when this step's channel was already tuned, so no fresh tune will come
    pub retune: bool,
}

pub struct RfaApp {
    pub s: Settings,
    pub engine: Engine,
    pub audio: Arc<AudioShared>,
    pub audio_out: Option<AudioOut>,
    pub audio_error: Option<String>,
    pub view: SpectrumView,
    pub tab: SideTab,
    // engine status snapshot
    pub running: bool,
    pub error: Option<String>,
    pub description: String,
    pub gains: Vec<i32>,
    pub channel_db: f32,
    pub squelch_open: bool,
    pub load: f32,
    pub sample_rate: u32,
    pub input_db: f32,
    pub recording: Option<RecordingInfo>,
    pub last_recording: Option<RecordingInfo>,
    pub fm: FmStats,
    pub rds: Option<RdsStation>,
    pub rds_stats: RdsStats,
    pub mpx: Vec<f32>,
    pub dab: Option<DabState>,
    pub dab_error: Option<String>,
    pub overflows: u64,
    // what was sent to the engine last
    last_src: Option<SourceConfig>,
    last_demod: Option<(Mode, u32, u64, bool, u32, bool)>,
    last_disp: (usize, usize, u32),
    last_sq: (bool, i32),
    last_mpx_view: bool,
    // UI state
    pub freq_input: String,
    pub bookmark_name: String,
    pub show_rds_window: bool,
    pub show_dab_window: bool,
    pub dab_tab: usize,
    pub show_about: bool,
    pub show_help: bool,
    pub scan: Option<Scan>,
    pub scan_state: DabScanState,
    pub slide_tex: Option<(u32, egui::TextureHandle)>,
    pub rtl_devices: Vec<String>,
    pub audio_devices: Vec<String>,
    pub rate_before_dab: Option<u32>,
    pub start_time: std::time::Instant,
    pub toast: Option<(String, f64)>,
    pub pty_rbds: bool,
    applied_scale: f32,
    applied_dark: Option<bool>,
    /// --scan N: scans still to run automatically, LNA step between them, quit when done
    auto_scans: u32,
    auto_lna_step: i32,
    auto_quit: bool,
    auto_next_at: f64,
    audio_retry_at: f64,
    next_settings_check: f64,
    saved_settings: String,
    /// Source stopped by itself (e.g. USB reset after sleep): restart it at this time
    reconnect_at: Option<f64>,
    reconnect_tries: u32,
    audio_watch: (u32, f64),
    next_default_check: f64,
}

pub fn icon() -> egui::IconData {
    // a simple generated icon: a spectrum peak on dark blue
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let i = (y * n + x) * 4;
            let fx = (x as f32 - 32.0) / 10.0;
            let curve = 52.0 - 38.0 * (-fx * fx).exp() - 4.0 * ((x as f32) * 0.7).sin().abs();
            let (r, g, b) = if (y as f32) > curve { (40, 170, 240) } else { (14, 22, 40) };
            rgba[i] = r;
            rgba[i + 1] = g;
            rgba[i + 2] = b;
            rgba[i + 3] = 255;
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}

impl RfaApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // settings.json (written on every change) wins over eframe's store
        let s: Settings = crate::config_dir()
            .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
            .and_then(|js| serde_json::from_str(&js).ok())
            .or_else(|| cc.storage.and_then(|st| eframe::get_value(st, "settings")))
            .unwrap_or_default();
        let audio = AudioShared::new();
        let ctx = cc.egui_ctx.clone();
        let engine = Engine::spawn(audio.clone(), move || ctx.request_repaint());
        let (audio_out, audio_error) = match AudioOut::open(audio.clone(), &s.audio_device) {
            Ok(a) => (Some(a), None),
            Err(e) => (None, Some(e)),
        };
        let mut style = (*cc.egui_ctx.global_style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        cc.egui_ctx.set_global_style(style);
        let mut app = Self {
            engine,
            audio,
            audio_out,
            audio_error,
            view: SpectrumView::new(),
            tab: SideTab::Source,
            running: false,
            error: None,
            description: String::new(),
            gains: Vec::new(),
            channel_db: -150.0,
            squelch_open: true,
            load: 0.0,
            sample_rate: 0,
            input_db: -100.0,
            recording: None,
            last_recording: None,
            fm: FmStats::default(),
            rds: None,
            rds_stats: RdsStats::default(),
            mpx: Vec::new(),
            dab: None,
            dab_error: None,
            overflows: 0,
            last_src: None,
            last_demod: None,
            last_disp: (0, 0, 0),
            last_sq: (false, 0),
            last_mpx_view: false,
            freq_input: String::new(),
            bookmark_name: String::new(),
            show_rds_window: false,
            show_dab_window: false,
            dab_tab: 0,
            show_about: false,
            show_help: false,
            scan: None,
            scan_state: DabScanState { found: s.dab_ensembles.clone(), ..Default::default() },
            slide_tex: None,
            rtl_devices: crate::source::rtlsdr::list_devices(),
            audio_devices: crate::audio::output_device_names(),
            rate_before_dab: None,
            start_time: std::time::Instant::now(),
            toast: None,
            pty_rbds: false,
            applied_scale: 0.0,
            applied_dark: None,
            auto_scans: 0,
            auto_lna_step: 0,
            auto_quit: false,
            auto_next_at: 0.0,
            audio_retry_at: 0.0,
            next_settings_check: 0.0,
            saved_settings: String::new(),
            reconnect_at: None,
            reconnect_tries: 0,
            audio_watch: (0, 0.0),
            next_default_check: 5.0,
            s,
        };
        app.engine.send(Cmd::DabRemembered(app.s.dab_last_service.clone()));
        let start = app.apply_args();
        app.sync_engine(true);
        if start {
            app.start();
        }
        app
    }

    /// Command line: [IQ file] [--file PATH] [--mode wfm|am|nfm|usb|lsb|cw|dab|off] [--freq HZ|97.3M]
    /// [--dab-channel 12C] [--start]. Opening an IQ file (e.g. by dropping it on the exe) plays it.
    fn apply_args(&mut self) -> bool {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut start = false;
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            let next = args.get(i + 1).cloned();
            match a {
                "--file" => {
                    if let Some(p) = next {
                        self.s.source.kind = SourceKind::File;
                        self.s.source.file_path = p;
                        i += 1;
                    }
                }
                "--mode" => {
                    if let Some(m) = next {
                        if let Some(mode) = Mode::ALL.iter().copied().find(|x| x.name().trim_end_matches('+').eq_ignore_ascii_case(m.trim_end_matches('+'))) {
                            self.s.mode = mode;
                        }
                        i += 1;
                    }
                }
                "--freq" => {
                    if let Some(f) = next.as_deref().and_then(parse_frequency) {
                        self.s.channel_freq = f;
                        i += 1;
                    }
                }
                "--dab-channel" => {
                    if let Some(c) = next {
                        self.s.dab_channel = c.to_ascii_uppercase();
                        i += 1;
                    }
                }
                "--start" => start = true,
                "--scan" => {
                    self.auto_scans = next.as_deref().and_then(|x| x.parse().ok()).unwrap_or(1);
                    self.auto_next_at = 3.0;
                    self.s.mode = Mode::Dab;
                    start = true;
                    i += 1;
                }
                "--scan-lna-step" => {
                    self.auto_lna_step = next.as_deref().and_then(|x| x.parse().ok()).unwrap_or(0);
                    i += 1;
                }
                "--quit-after-scan" => self.auto_quit = true,
                other if !other.starts_with("--") && std::path::Path::new(other).is_file() => {
                    self.s.source.kind = SourceKind::File;
                    self.s.source.file_path = other.to_string();
                    start = true;
                }
                _ => {}
            }
            i += 1;
        }
        start
    }

    pub fn now(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    pub fn reopen_audio(&mut self) {
        self.audio_out = None;
        match AudioOut::open(self.audio.clone(), &self.s.audio_device) {
            Ok(a) => {
                self.audio_out = Some(a);
                self.audio_error = None;
            }
            Err(e) => self.audio_error = Some(e),
        }
    }

    /// Rebuilds the audio stream after the device changed or went away (WASAPI invalidates it),
    /// retrying every 2 s while it fails.
    fn check_audio(&mut self) {
        let mut failed = self.audio.stream_failed.load(std::sync::atomic::Ordering::Relaxed);
        // a stream that silently stopped calling back (seen after sleep/wake) counts as failed
        let n = self.audio.callbacks.load(std::sync::atomic::Ordering::Relaxed);
        let now = self.now();
        if n != self.audio_watch.0 || self.audio_out.is_none() {
            self.audio_watch = (n, now);
        } else if now - self.audio_watch.1 > 3.0 {
            crate::log_line("audio output stalled (no callbacks for 3 s), reopening");
            self.audio_watch = (n, now);
            failed = true;
        }
        // "System default" is opened as a concrete device, so follow the default when it changes
        // (e.g. after wake the monitor's HDMI audio returns a few seconds after the fallback).
        if self.s.audio_device.is_empty() && now >= self.next_default_check {
            self.next_default_check = now + 3.0;
            if let (Some(def), Some(out)) = (crate::audio::default_device_name(), self.audio_out.as_ref()) {
                if def != out.device_name {
                    crate::log_line(&format!("default output changed to {def}, reopening"));
                    failed = true;
                    self.audio_retry_at = 0.0;
                }
            }
        }
        let missing = self.audio_out.is_none();
        if (failed || missing) && self.now() >= self.audio_retry_at {
            self.audio_retry_at = self.now() + 2.0;
            self.reopen_audio();
            match &self.audio_error {
                None => crate::log_line("audio output reopened"),
                Some(e) if !missing || failed => crate::log_line(&format!("audio reopen failed: {e}")),
                _ => {}
            }
        }
    }

    /// Writes the settings to %APPDATA%\\RFAnalyzer\\settings.json whenever they change, so they
    /// survive even if the app is killed or eframe's own store isn't written.
    fn autosave_settings(&mut self) {
        if self.now() < self.next_settings_check {
            return;
        }
        self.next_settings_check = self.now() + 2.0;
        let Ok(js) = serde_json::to_string_pretty(&self.s) else { return };
        if js == self.saved_settings {
            return;
        }
        if let Some(dir) = crate::config_dir() {
            let tmp = dir.join("settings.json.tmp");
            if std::fs::write(&tmp, &js).is_ok() && std::fs::rename(&tmp, dir.join("settings.json")).is_ok() {
                self.saved_settings = js;
            }
        }
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), self.now() + 4.0));
    }

    // ------------------------------------------------------------------ engine link

    /// The channel frequency the demodulator uses.
    pub fn channel_freq(&self) -> u64 {
        if self.s.mode == Mode::Dab {
            model::channel_by_name(&self.s.dab_channel).map(|c| c.freq).unwrap_or(227_360_000)
        } else {
            self.s.channel_freq
        }
    }

    fn sync_engine(&mut self, force: bool) {
        let dp = (self.s.mode, self.s.width(self.s.mode), self.channel_freq(), self.s.wfm_stereo, self.s.deemphasis_us as u32, self.s.rds_enabled);
        if force || self.last_demod != Some(dp) {
            self.engine.send(Cmd::Demod(DemodParams {
                mode: dp.0,
                width: dp.1,
                channel_freq: dp.2,
                stereo: dp.3,
                deemphasis_us: self.s.deemphasis_us,
                rds: dp.5,
            }));
            self.last_demod = Some(dp);
        }
        let d = &self.s.display;
        let disp = (d.fft_size, d.averaging, d.frame_rate);
        if force || disp != self.last_disp {
            self.engine.send(Cmd::Display { fft_size: disp.0, averaging: disp.1, frame_rate: disp.2 });
            self.last_disp = disp;
        }
        let sq = (self.s.squelch_enabled, (self.s.squelch_db * 10.0) as i32);
        if force || sq != self.last_sq {
            self.engine.send(Cmd::Squelch { enabled: self.s.squelch_enabled, db: self.s.squelch_db });
            self.last_sq = sq;
        }
        if self.running && self.last_src.as_ref() != Some(&self.s.source) {
            self.engine.send(Cmd::Source(self.s.source.clone()));
            self.last_src = Some(self.s.source.clone());
        }
        let mpx_view = self.show_rds_window && self.s.mode == Mode::Wfm;
        if mpx_view != self.last_mpx_view {
            self.engine.send(Cmd::MpxView(mpx_view));
            self.last_mpx_view = mpx_view;
        }
        self.audio.set_volume(self.s.volume);
        self.audio.set_muted(self.s.muted);
    }

    pub fn start(&mut self) {
        if self.s.source.kind == SourceKind::File {
            let (f, r, fmt) = crate::source::file::parse_file_name(&self.s.source.file_path);
            if let Some(f) = f {
                self.s.source.file_frequency = f;
            }
            if let Some(r) = r {
                self.s.source.file_rate = r;
            }
            if let Some(fmt) = fmt {
                self.s.source.file_format = fmt;
            }
            self.s.source.frequency = self.s.source.file_frequency;
            self.s.source.sample_rate = self.s.source.file_rate;
        }
        if self.s.mode == Mode::Dab {
            self.prepare_dab_tuning();
        } else {
            self.ensure_channel_in_band();
        }
        self.engine.send(Cmd::Start(self.s.source.clone()));
        self.last_src = Some(self.s.source.clone());
        self.view.clear();
        self.running = true;
        self.error = None;
    }

    pub fn stop(&mut self) {
        self.reconnect_at = None;
        self.reconnect_tries = 0;
        self.engine.send(Cmd::Stop);
        self.running = false;
        self.scan = None;
        self.scan_state.running = false;
    }

    pub fn toggle_run(&mut self) {
        if self.running { self.stop() } else { self.start() }
    }

    fn poll_status(&mut self, ctx: &egui::Context) {
        let st = self.engine.status.clone();
        let mut st = st.lock();
        let frames: Vec<_> = st.frames.drain(..).collect();
        let was_running = self.running;
        self.running = st.running || (self.running && st.error.is_none());
        if !st.running && st.error.is_some() {
            self.running = false;
        }
        if !st.running && st.error.is_some() {
            if let Some(e) = &st.error {
                crate::log_line(&format!("source error: {e}"));
            }
            // a live SDR that stops by itself (USB reset on sleep/wake, brief unplug): keep retrying
            if (was_running || self.reconnect_at.is_some()) && self.s.source.kind != SourceKind::File {
                self.reconnect_tries += 1;
                let delay = if self.reconnect_tries <= 5 { 2.0 } else { 5.0 };
                self.reconnect_at = Some(self.start_time.elapsed().as_secs_f64() + delay);
            }
        } else if st.running && self.reconnect_at.is_none() && self.reconnect_tries > 0 && !frames.is_empty() {
            crate::log_line(&format!("source running again after {} attempt(s)", self.reconnect_tries));
            self.reconnect_tries = 0;
        }
        if st.error != self.error && st.error.is_some() {
            self.error = st.error.clone();
        }
        st.error = None;
        self.description = st.description.clone();
        self.gains = st.gains.clone();
        self.channel_db = st.channel_db;
        self.squelch_open = st.squelch_open;
        self.load = st.load;
        self.sample_rate = st.sample_rate;
        self.input_db = st.input_level_db;
        self.recording = st.recording.clone();
        if let Some(fin) = st.recording_finished.take() {
            self.last_recording = Some(fin.clone());
            self.toast = Some((format!("Recording saved: {} ({:.1} MB)", fin.path, fin.bytes as f64 / 1e6), self.start_time.elapsed().as_secs_f64() + 6.0));
        }
        self.fm = st.fm.clone();
        self.rds = st.rds.clone();
        self.rds_stats = st.rds_stats.clone();
        if !st.mpx_spectrum.is_empty() {
            self.mpx = st.mpx_spectrum.clone();
        }
        self.dab = st.dab.clone();
        self.dab_error = st.dab_error.clone();
        self.overflows = st.overflow_blocks;
        if !st.dab_remembered.is_empty() && st.dab_remembered != self.s.dab_last_service {
            self.s.dab_last_service = st.dab_remembered.clone();
        }
        drop(st);
        let d = &self.s.display;
        for f in frames {
            self.view.push_frame(ctx, &f.db, f.center, f.rate, d.db_min, d.db_max, d.colormap);
        }
    }

    // ------------------------------------------------------------------ tuning

    /// Sets the channel and retunes the source if the channel would leave the received band.
    pub fn set_channel(&mut self, f: u64) {
        if self.s.mode == Mode::Dab {
            return;
        }
        self.s.channel_freq = f;
        if self.s.mode == Mode::Off {
            if self.s.source.kind != SourceKind::File {
                self.s.source.frequency = f;
            }
            return;
        }
        self.ensure_channel_in_band();
    }

    pub fn ensure_channel_in_band(&mut self) {
        if self.s.source.kind == SourceKind::File || self.s.mode == Mode::Off {
            return;
        }
        let rate = self.s.source.sample_rate as f64;
        let ch = self.s.channel_freq as f64;
        let center = self.s.source.frequency as f64;
        let (lo, hi) = self.s.mode.passband(self.s.width(self.s.mode));
        let margin = rate * 0.45;
        if ch + lo < center - margin || ch + hi > center + margin {
            // put the channel a quarter of the band away from the centre (off the DC spike)
            let nf = (ch - rate / 4.0).max(0.0);
            self.s.source.frequency = ((nf / 1000.0).round() * 1000.0) as u64;
        }
    }

    pub fn step_channel(&mut self, n: i32) {
        if self.s.mode == Mode::Dab {
            // channel by channel through Band III
            let list = &model::BAND_III;
            let i = list.iter().position(|c| c.name == self.s.dab_channel).unwrap_or(0) as i32;
            let j = (i + n).clamp(0, list.len() as i32 - 1) as usize;
            self.set_dab_channel(list[j].name.to_string());
            return;
        }
        let step = self.s.step(self.s.mode) as i64;
        let f = (self.s.channel_freq as i64 / step + n as i64) * step;
        self.set_channel(f.max(0) as u64);
    }

    pub fn set_mode(&mut self, m: Mode) {
        if m == self.s.mode {
            return;
        }
        let was_dab = self.s.mode == Mode::Dab;
        self.s.mode = m;
        if m == Mode::Dab {
            self.prepare_dab_tuning();
        } else {
            if was_dab {
                if let Some(r) = self.rate_before_dab.take() {
                    self.s.source.sample_rate = r;
                }
                self.scan = None;
                self.scan_state.running = false;
            }
            self.ensure_channel_in_band();
        }
        self.view.reset_zoom();
    }

    fn prepare_dab_tuning(&mut self) {
        if self.s.source.kind == SourceKind::File {
            return;
        }
        if self.s.source.sample_rate != 2_048_000 {
            if self.rate_before_dab.is_none() {
                self.rate_before_dab = Some(self.s.source.sample_rate);
            }
            self.s.source.sample_rate = 2_048_000;
        }
        self.s.source.frequency = self.channel_freq();
    }

    pub fn set_dab_channel(&mut self, name: String) {
        self.s.dab_channel = name;
        self.prepare_dab_tuning();
    }

    pub fn dab_select(&mut self, sid: u32) {
        self.engine.send(Cmd::DabSelect(sid));
    }

    // ------------------------------------------------------------------ DAB scan

    pub fn start_scan(&mut self) {
        if !self.running {
            self.start();
        }
        scan_log_line(&format!(
            "=== scan start: source {:?}, rate {}, lna {}, vga {}, amp {}, rtl gain {}, agc {}",
            self.s.source.kind,
            self.s.source.sample_rate,
            self.s.source.hackrf_lna,
            self.s.source.hackrf_vga,
            self.s.source.hackrf_amp,
            self.s.source.rtl_gain,
            self.s.source.rtl_agc
        ));
        let channels = model::scan_list();
        self.scan_state = DabScanState { running: true, count: channels.len(), ..Default::default() };
        let first = channels[0].name.to_string();
        let retune = self.needs_retune(channels[0].freq);
        self.scan = Some(Scan { channels, index: 0, phase: ScanPhase::Tuning, t0: self.now(), previous_channel: self.s.dab_channel.clone(), retune });
        self.set_dab_channel(first);
    }

    fn auto_scan_step(&mut self) {
        if self.auto_scans == 0 || self.scan.is_some() || self.now() < self.auto_next_at {
            return;
        }
        if self.scan_state.finished && self.auto_lna_step != 0 {
            let lna = (self.s.source.hackrf_lna as i32 + self.auto_lna_step).clamp(0, 40) as u32;
            self.s.source.hackrf_lna = lna;
            scan_log_line(&format!("auto: LNA set to {lna}"));
        }
        self.auto_scans -= 1;
        self.start_scan();
        self.auto_next_at = 0.0;
    }

    /// Whether tuning to `freq` will give the DAB receiver a fresh start (it only restarts when the
    /// channel moves by more than 100 kHz).
    fn needs_retune(&self, freq: u64) -> bool {
        match &self.dab {
            Some(d) if d.active => d.frequency.abs_diff(freq) > 100_000,
            _ => true,
        }
    }

    pub fn stop_scan(&mut self) {
        self.scan = None;
        self.scan_state.running = false;
        if self.scan_state.found.is_empty() {
            self.scan_state.found = self.s.dab_ensembles.clone();
        }
    }

    fn scan_step(&mut self) {
        let Some(scan) = self.scan.as_mut() else { return };
        let now = self.start_time.elapsed().as_secs_f64();
        let ch = scan.channels[scan.index];
        let el = now - scan.t0;
        let st = self.dab.clone().unwrap_or_default();
        let on_channel = st.active && st.frequency.abs_diff(ch.freq) < 1000;
        let mut next = false;
        let mut note: Option<String> = None;
        match scan.phase {
            ScanPhase::Tuning => {
                // tuned since this step began (the age is only refreshed every 0.5 s)
                if on_channel && (!scan.retune || (st.seconds_since_tune as f64) < el + 0.6) {
                    scan.phase = ScanPhase::WaitSync;
                    scan.t0 = now;
                } else if el > 4.0 {
                    note = Some(format!("{}: not tuned (is the source running?)", ch.name));
                    next = true;
                }
            }
            ScanPhase::WaitSync => {
                if st.sync_seen {
                    scan.phase = ScanPhase::WaitInfo;
                    scan.t0 = now;
                } else if el > 4.0 {
                    note = Some(format!("{}: no signal", ch.name));
                    next = true;
                }
            }
            ScanPhase::WaitInfo => {
                if !st.ensemble_label.is_empty() && !st.services.is_empty() {
                    scan.phase = ScanPhase::Collect;
                    scan.t0 = now;
                } else if el > 10.0 {
                    note = Some(format!("{}: DAB signal but no ensemble info (SNR {} dB, FIC {}%)", ch.name, st.snr, st.quality.fic));
                    next = true;
                }
            }
            ScanPhase::Collect => {
                if el > 2.5 {
                    let label = if st.ensemble_label.is_empty() { "(unnamed)".to_string() } else { st.ensemble_label.clone() };
                    self.scan_state.found.push(SavedEnsemble {
                        channel: ch.name.to_string(),
                        frequency: ch.freq,
                        label: label.clone(),
                        ensemble_id: st.ensemble_id,
                        services: st.services.iter().map(|s| s.label.clone()).collect(),
                    });
                    note = Some(format!("{}: {}, {} services (SNR {} dB)", ch.name, label, st.services.len(), st.snr));
                    next = true;
                }
            }
        }
        if let Some(n) = note {
            let diag = format!(
                "  [t {:.1}s, in {:.2} Msps, level {:.1} dB, dropped {}, overflows {}, since tune {:.1}s, sync {}]",
                now,
                st.input_rate as f64 / 1e6,
                st.input_level_db,
                st.dropped_samples,
                self.overflows,
                st.seconds_since_tune,
                st.sync_seen
            );
            scan_log_line(&format!("{n}{diag}"));
            self.scan_state.log.push(n);
        }
        if next {
            let scan = self.scan.as_mut().unwrap();
            scan.index += 1;
            if scan.index >= scan.channels.len() {
                let found = self.scan_state.found.clone();
                self.scan = None;
                self.scan_state.running = false;
                self.scan_state.finished = true;
                scan_log_line(&format!("=== scan finished: {} ensembles", found.len()));
                if self.auto_scans > 0 {
                    self.auto_next_at = now + 20.0;
                } else if self.auto_quit {
                    self.auto_next_at = -1.0;
                }
                if !found.is_empty() {
                    self.s.dab_ensembles = found.clone();
                    self.set_dab_channel(found[0].channel.clone());
                    self.toast(format!("Scan finished: {} ensembles found", found.len()));
                } else {
                    self.scan_state.found = self.s.dab_ensembles.clone();
                    self.toast("Scan finished: nothing found. Check the antenna and gain.");
                }
            } else {
                let f = scan.channels[scan.index].freq;
                let retune = self.needs_retune(f);
                let scan = self.scan.as_mut().unwrap();
                scan.retune = retune;
                scan.phase = ScanPhase::Tuning;
                scan.t0 = now;
                let name = scan.channels[scan.index].name.to_string();
                self.scan_state.index = scan.index;
                self.scan_state.current = name.clone();
                self.set_dab_channel(name);
            }
        } else {
            self.scan_state.current = ch.name.to_string();
            self.scan_state.index = scan.index;
        }
    }

    // ------------------------------------------------------------------ recording

    pub fn start_recording(&mut self) {
        if !self.running {
            self.toast("Start the source first.");
            return;
        }
        let dir = if self.s.record_dir.is_empty() {
            dirs_home().join("RFAnalyzer recordings")
        } else {
            std::path::PathBuf::from(&self.s.record_dir)
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.toast(format!("Could not create {}: {e}", dir.display()));
            return;
        }
        let fmt = match self.s.source.kind {
            SourceKind::HackRf => crate::settings::FileFormat::Cs8,
            SourceKind::File => self.s.source.file_format,
            _ => crate::settings::FileFormat::Cu8,
        };
        let name = format!(
            "RFA_{}_{}Hz_{}sps.{}",
            chrono::Local::now().format("%Y%m%d_%H%M%S"),
            self.s.source.frequency,
            self.s.source.sample_rate,
            fmt.ext()
        );
        let path = dir.join(name).to_string_lossy().into_owned();
        self.engine.send(Cmd::StartRecording {
            path,
            only_squelch: self.s.record_only_squelch,
            stop_bytes: self.s.record_stop_mb as u64 * 1_000_000,
            stop_secs: self.s.record_stop_sec as u64,
        });
    }

    pub fn stop_recording(&mut self) {
        self.engine.send(Cmd::StopRecording);
    }

    // ------------------------------------------------------------------ UI

    fn apply_look(&mut self, ctx: &egui::Context) {
        if (self.s.display.ui_scale - self.applied_scale).abs() > 0.001 {
            ctx.set_zoom_factor(self.s.display.ui_scale);
            self.applied_scale = self.s.display.ui_scale;
        }
        if self.applied_dark != Some(self.s.display.dark) {
            ctx.set_theme(if self.s.display.dark { egui::Theme::Dark } else { egui::Theme::Light });
            self.applied_dark = Some(self.s.display.dark);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (f5, left, right, pgup, pgdn, m, space) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::F5),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::M),
                i.key_pressed(egui::Key::Space),
            )
        });
        if f5 || space {
            self.toggle_run();
        }
        if left {
            self.step_channel(-1);
        }
        if right {
            self.step_channel(1);
        }
        if pgup {
            self.step_channel(10);
        }
        if pgdn {
            self.step_channel(-10);
        }
        if m {
            self.s.muted = !self.s.muted;
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let (label, col) = if self.running { ("■  Stop", Color32::from_rgb(190, 50, 50)) } else { ("▶  Start", Color32::from_rgb(40, 140, 70)) };
            if ui.add(egui::Button::new(RichText::new(label).size(20.0).color(Color32::WHITE)).fill(col).min_size(egui::vec2(120.0, 44.0))).clicked() {
                self.toggle_run();
            }
            ui.vertical(|ui| {
                ui.label(RichText::new(self.s.source.kind.name()).strong());
                let info = if self.running && self.sample_rate > 0 { format!("{:.3} Msps", self.sample_rate as f64 / 1e6) } else { "stopped".into() };
                ui.label(RichText::new(info).weak());
            });
            ui.separator();
            // channel frequency
            let freq = self.channel_freq();
            let editable = self.s.mode != Mode::Dab;
            if let Some(nf) = frequency_display(ui, "chan", freq, 40.0, editable) {
                if self.s.mode == Mode::Off {
                    self.s.channel_freq = nf;
                    self.s.source.frequency = nf;
                } else {
                    self.set_channel(nf);
                }
            }
            ui.label(RichText::new("Hz").weak());
            ui.vertical(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.freq_input).hint_text("Go to… (e.g. 97.3)").desired_width(130.0));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(f) = parse_frequency(&self.freq_input) {
                        if self.s.mode == Mode::Dab {
                            if let Some(c) = model::channel_for_freq(f) {
                                self.set_dab_channel(c.name.to_string());
                            }
                        } else if self.s.mode == Mode::Off {
                            self.s.channel_freq = f;
                            self.s.source.frequency = f;
                        } else {
                            self.set_channel(f);
                        }
                        self.freq_input.clear();
                    }
                }
                if self.s.mode == Mode::Dab {
                    let ch = model::channel_by_name(&self.s.dab_channel);
                    ui.label(RichText::new(format!("DAB channel {}", ch.map(|c| c.name).unwrap_or("-"))).weak());
                } else {
                    ui.label(RichText::new(format!("Step {}", fmt_hz(self.s.step(self.s.mode) as f64))).weak());
                }
            });
            ui.separator();
            // modes
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    for m in Mode::ALL {
                        let sel = self.s.mode == m;
                        let b = egui::Button::selectable(sel, RichText::new(m.name()).size(15.0).strong()).min_size(egui::vec2(48.0, 26.0));
                        if ui.add(b).on_hover_text(m.description()).clicked() {
                            self.set_mode(m);
                        }
                    }
                });
                // status line under the modes
                let txt = match self.s.mode {
                    Mode::Wfm => {
                        let mut t = if self.fm.stereo { "STEREO".to_string() } else if self.fm.pilot_locked { "pilot (stereo off)".into() } else { "MONO".into() };
                        if let Some(r) = &self.rds {
                            t += &format!("   RDS {}   {}", r.ps.trim(), r.pi.map(|p| format!("PI {p:04X}")).unwrap_or_default());
                        }
                        t
                    }
                    Mode::Dab => self
                        .dab
                        .as_ref()
                        .map(|d| {
                            let svc = d.selected().map(|s| s.label.clone()).unwrap_or_default();
                            format!("{}  {}  {}", if d.sync { "SYNC" } else { "no sync" }, d.ensemble_label, svc)
                        })
                        .unwrap_or_else(|| "DAB+ (start the source)".into()),
                    _ => self.s.mode.description().to_string(),
                };
                ui.label(RichText::new(txt).weak());
            });
            ui.separator();
            // level meter
            ui.vertical(|ui| {
                let lvl = if self.s.mode == Mode::Off { self.input_db } else { self.channel_db };
                let frac = ((lvl + 100.0) / 80.0).clamp(0.0, 1.0);
                let col = if self.s.squelch_enabled && !self.squelch_open { Color32::from_rgb(120, 120, 120) } else { Color32::from_rgb(60, 180, 90) };
                ui.add(egui::ProgressBar::new(frac).desired_width(140.0).fill(col).text(format!("{lvl:.1} dB")));
                ui.horizontal(|ui| {
                    let mute = if self.s.muted { "🔇" } else { "🔊" };
                    if ui.button(mute).on_hover_text("Mute (M)").clicked() {
                        self.s.muted = !self.s.muted;
                    }
                    ui.add(egui::Slider::new(&mut self.s.volume, 0.0..=1.5).show_value(false)).on_hover_text("Volume");
                });
            });
            ui.separator();
            let rec = self.recording.is_some();
            let rec_label = if rec { RichText::new("⏺ Recording").color(Color32::from_rgb(230, 60, 60)) } else { RichText::new("⏺ Record") };
            if ui.add(egui::Button::new(rec_label.size(16.0)).min_size(egui::vec2(110.0, 36.0))).clicked() {
                if rec { self.stop_recording() } else { self.start_recording() }
            }
            if let Some(r) = &self.recording {
                ui.label(format!("{:.1} MB\n{:.0} s", r.bytes as f64 / 1e6, r.seconds));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(RichText::new("☰").size(20.0), |ui| {
                    ui.checkbox(&mut self.s.show_side_panel, "Settings panel");
                    ui.checkbox(&mut self.s.show_info_panel, "Info panel (RDS / DAB)");
                    ui.checkbox(&mut self.s.display.dark, "Dark theme");
                    ui.add(egui::Slider::new(&mut self.s.display.ui_scale, 0.8..=2.5).text("UI scale"));
                    ui.separator();
                    if ui.button("Keyboard and mouse help").clicked() {
                        self.show_help = true;
                    }
                    if ui.button("About").clicked() {
                        self.show_about = true;
                    }
                });
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(e) = &self.error {
                ui.label(RichText::new(format!("⚠ {e}")).color(Color32::from_rgb(230, 90, 80)));
                if ui.small_button("✕").clicked() {
                    self.error = None;
                }
                ui.separator();
            }
            if self.running {
                ui.label(&self.description);
                ui.separator();
                ui.label(format!("centre {:.4} MHz", self.view.center / 1e6));
                ui.separator();
                ui.label(format!("CPU {:.0}%", self.load * 100.0));
                if self.overflows > 0 {
                    ui.label(RichText::new(format!("{} overflows", self.overflows)).color(Color32::from_rgb(230, 160, 60)));
                }
            } else {
                ui.label("Stopped");
            }
            ui.separator();
            match (&self.audio_out, &self.audio_error) {
                (Some(a), _) => {
                    ui.label(format!("Audio: {} ({} Hz)", a.device_name, a.device_rate));
                }
                (None, Some(e)) => {
                    ui.label(RichText::new(format!("Audio: {e}")).color(Color32::from_rgb(230, 90, 80)));
                }
                _ => {}
            }
            if let Some((msg, until)) = self.toast.clone() {
                if self.now() < until {
                    ui.separator();
                    ui.label(RichText::new(msg).color(Color32::from_rgb(120, 200, 255)));
                } else {
                    self.toast = None;
                }
            }
        });
    }

    fn spectrum_area(&mut self, ui: &mut egui::Ui) {
        let dab_label = if self.s.mode == Mode::Dab {
            self.dab.as_ref().map(|d| format!("{} {}", self.s.dab_channel, d.ensemble_label)).or(Some(self.s.dab_channel.clone()))
        } else {
            None
        };
        let p = ViewParams {
            db_min: self.s.display.db_min,
            db_max: self.s.display.db_max,
            channel: self.channel_freq(),
            mode: self.s.mode,
            width: self.s.width(self.s.mode),
            step: self.s.step(self.s.mode),
            fill: self.s.display.fill_spectrum,
            peak_hold: self.s.display.peak_hold,
            split: self.s.display.split,
            squelch_db: None,
            dab_label,
        };
        for a in self.view.show(ui, &p) {
            match a {
                Action::SetChannel(f) => self.set_channel(f),
                Action::StepChannel(n) => self.step_channel(n),
                Action::TuneSource(df) => {
                    if self.s.source.kind != SourceKind::File && self.s.mode != Mode::Dab {
                        let nf = (self.s.source.frequency as f64 + df).max(0.0);
                        self.s.source.frequency = nf as u64;
                        if self.s.mode == Mode::Off {
                            self.s.channel_freq = nf as u64;
                        }
                    }
                }
                Action::SetDbRange(lo, hi) => {
                    self.s.display.db_min = lo.clamp(-200.0, 50.0);
                    self.s.display.db_max = hi.clamp(lo + 5.0, 60.0);
                }
            }
        }
    }

    pub fn autoscale(&mut self) {
        if self.view.latest.is_empty() {
            return;
        }
        let mut v = self.view.latest.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let noise = v[v.len() / 5];
        let peak = v[v.len() - 1 - v.len() / 1000];
        self.s.display.db_min = (noise - 10.0).floor();
        self.s.display.db_max = (peak + 10.0).max(noise + 30.0).ceil();
    }
}

pub fn dirs_home() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(std::path::PathBuf::from).unwrap_or_else(|| ".".into())
}

pub fn fmt_hz(f: f64) -> String {
    if f >= 1e6 {
        format!("{:.3} MHz", f / 1e6)
    } else if f >= 1e3 {
        let k = f / 1e3;
        if (k - k.round()).abs() < 1e-6 { format!("{k:.0} kHz") } else { format!("{k} kHz") }
    } else {
        format!("{f:.0} Hz")
    }
}

impl eframe::App for RfaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_look(&ctx);
        self.poll_status(&ctx);
        self.shortcuts(&ctx);
        self.check_audio();
        if let Some(t) = self.reconnect_at {
            if self.now() >= t {
                self.reconnect_at = None;
                crate::log_line(&format!("reconnecting to the source (attempt {})", self.reconnect_tries));
                self.start();
                self.toast(format!("The SDR stopped; reconnecting (attempt {})…", self.reconnect_tries));
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        self.autosave_settings();
        self.scan_step();
        self.auto_scan_step();
        if self.auto_next_at < 0.0 && self.scan.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.scan.is_some() || self.auto_scans > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        egui::Panel::top("top").frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(10, 8))).show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        if self.s.show_side_panel {
            egui::Panel::left("side").default_size(340.0).min_size(260.0).show(ui, |ui| self.side_panel(ui));
        }
        let info_wanted = self.s.show_info_panel && matches!(self.s.mode, Mode::Wfm | Mode::Dab);
        if info_wanted {
            egui::Panel::right("info").default_size(400.0).min_size(300.0).show(ui, |ui| self.info_panel(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.spectrum_area(ui));

        self.windows(&ctx);
        self.sync_engine(false);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "settings", &self.s);
        self.next_settings_check = 0.0;
        self.autosave_settings();
    }

    fn on_exit(&mut self) {
        self.engine.send(Cmd::Stop);
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

/// Appends a line to %APPDATA%\\RFAnalyzer\\dab_scan_log.txt (scan diagnostics).
fn scan_log_line(line: &str) {
    use std::io::Write;
    let Some(dir) = crate::config_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("dab_scan_log.txt")) {
        let _ = writeln!(f, "{} {}", chrono::Local::now().format("%H:%M:%S"), line);
    }
}

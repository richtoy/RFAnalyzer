//! The processing thread: runs the source, computes the spectrum, records, demodulates (analog,
//! FM stereo + RDS, DAB+) and feeds the audio output. The UI talks to it through commands and
//! reads a shared status.

use crate::audio::AudioShared;
use crate::dab::model::DabState;
use crate::dab::receiver::{DabReceiver, INPUT_RATE as DAB_RATE};
use crate::dsp::demod::Demod;
use crate::dsp::resampler::Channelizer;
use crate::dsp::spectrum::SpectrumProcessor;
use crate::dsp::C32;
use crate::rds::{RdsDecoder, RdsStation};
use crate::settings::{FileFormat, Mode, SourceConfig};
use crate::source::{self, IqBlock, Source};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct DemodParams {
    pub mode: Mode,
    pub width: u32,
    /// displayed channel frequency (Hz)
    pub channel_freq: u64,
    pub stereo: bool,
    pub deemphasis_us: f32,
    pub rds: bool,
}

pub enum Cmd {
    Start(SourceConfig),
    Stop,
    Source(SourceConfig),
    Demod(DemodParams),
    Display { fft_size: usize, averaging: usize, frame_rate: u32 },
    Squelch { enabled: bool, db: f32 },
    StartRecording { path: String, only_squelch: bool, stop_bytes: u64, stop_secs: u64 },
    StopRecording,
    DabSelect(u32),
    DabRemembered(HashMap<String, u32>),
    MpxView(bool),
}

pub struct SpectrumFrame {
    pub db: Vec<f32>,
    /// displayed centre frequency and sample rate of this frame
    pub center: f64,
    pub rate: f64,
}

#[derive(Clone, Debug, Default)]
pub struct RecordingInfo {
    pub path: String,
    pub bytes: u64,
    pub seconds: f64,
    pub format: Option<FileFormat>,
}

#[derive(Clone, Debug, Default)]
pub struct FmStats {
    pub active: bool,
    pub pilot_db: f32,
    pub pilot_locked: bool,
    pub stereo: bool,
    pub deviation_khz: f32,
    pub pll_hz: f64,
    pub rds_level: f32,
    pub costas_lock: f32,
}

#[derive(Clone, Debug, Default)]
pub struct RdsStats {
    pub synced: bool,
    pub bler: f32,
    pub bler_history: Vec<f32>,
    pub groups_per_sec: f32,
    pub groups_total: u64,
    pub blocks_ok: u64,
    pub blocks_corrected: u64,
    pub blocks_failed: u64,
    pub symbols: Vec<f32>,
}

#[derive(Default)]
pub struct Status {
    pub running: bool,
    pub error: Option<String>,
    pub description: String,
    pub gains: Vec<i32>,
    pub frames: VecDeque<SpectrumFrame>,
    pub channel_db: f32,
    pub squelch_open: bool,
    pub recording: Option<RecordingInfo>,
    pub recording_finished: Option<RecordingInfo>,
    pub load: f32,
    pub sample_rate: u32,
    pub hw_frequency: u64,
    pub fixed_tuning: Option<(u64, u32)>,
    pub fm: FmStats,
    pub mpx_spectrum: Vec<f32>,
    pub rds: Option<RdsStation>,
    pub rds_stats: RdsStats,
    pub dab: Option<DabState>,
    pub dab_remembered: HashMap<String, u32>,
    pub dab_error: Option<String>,
    pub input_level_db: f32,
    pub overflow_blocks: u64,
}

pub struct Engine {
    pub tx: Sender<Cmd>,
    pub status: Arc<Mutex<Status>>,
}

impl Engine {
    pub fn spawn(audio: Arc<AudioShared>, repaint: impl Fn() + Send + 'static) -> Engine {
        let (tx, rx) = bounded::<Cmd>(256);
        let status = Arc::new(Mutex::new(Status::default()));
        let st = status.clone();
        std::thread::Builder::new()
            .name("engine".into())
            .spawn(move || {
                let mut w = Worker::new(rx, st, audio, Box::new(repaint));
                w.run();
            })
            .expect("engine thread");
        Engine { tx, status }
    }
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

struct Recorder {
    file: std::io::BufWriter<std::fs::File>,
    info: RecordingInfo,
    only_squelch: bool,
    stop_bytes: u64,
    stop_secs: u64,
    rate: u32,
}

struct Worker {
    rx: Receiver<Cmd>,
    status: Arc<Mutex<Status>>,
    audio: Arc<AudioShared>,
    repaint: Box<dyn Fn() + Send>,
    source: Option<Box<dyn Source>>,
    iq_rx: Option<Receiver<IqBlock>>,
    cfg: SourceConfig,
    demod_params: DemodParams,
    demod: Demod,
    channelizer: Option<Channelizer>,
    chan_key: (u32, Mode),
    rds: Option<RdsDecoder>,
    dab: DabReceiver,
    spectrum: SpectrumProcessor,
    frame_rate: u32,
    fft_tail: Vec<C32>,
    since_frame: usize,
    squelch: (bool, f32),
    squelch_open: bool,
    channel_db: f32,
    recorder: Option<Recorder>,
    bb: Vec<C32>,
    audio_buf: Vec<[f32; 2]>,
    load_acc: f64,
    time_acc: f64,
    last_stat: Instant,
    rds_last_groups: u64,
    bler_history: Vec<f32>,
    mpx_view: bool,
    mpx_spec: SpectrumProcessor,
    mpx_tail: Vec<C32>,
    dab_published: u64,
}

impl Worker {
    fn new(rx: Receiver<Cmd>, status: Arc<Mutex<Status>>, audio: Arc<AudioShared>, repaint: Box<dyn Fn() + Send>) -> Self {
        Self {
            rx,
            status,
            audio,
            repaint,
            source: None,
            iq_rx: None,
            cfg: SourceConfig::default(),
            demod_params: DemodParams { mode: Mode::Off, width: 0, channel_freq: 0, stereo: true, deemphasis_us: 50.0, rds: true },
            demod: Demod::Off,
            channelizer: None,
            chan_key: (0, Mode::Off),
            rds: None,
            dab: DabReceiver::new(),
            spectrum: SpectrumProcessor::new(8192, 3),
            frame_rate: 30,
            fft_tail: Vec::new(),
            since_frame: 0,
            squelch: (false, -80.0),
            squelch_open: true,
            channel_db: -150.0,
            recorder: None,
            bb: Vec::new(),
            audio_buf: Vec::new(),
            load_acc: 0.0,
            time_acc: 0.0,
            last_stat: Instant::now(),
            rds_last_groups: 0,
            bler_history: Vec::new(),
            mpx_view: false,
            mpx_spec: SpectrumProcessor::new(2048, 4),
            mpx_tail: Vec::new(),
            dab_published: u64::MAX,
        }
    }

    fn run(&mut self) {
        loop {
            // commands
            loop {
                let cmd = if self.source.is_some() {
                    match self.rx.try_recv() {
                        Ok(c) => c,
                        Err(crossbeam_channel::TryRecvError::Empty) => break,
                        Err(_) => return,
                    }
                } else {
                    match self.rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(c) => c,
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(_) => return,
                    }
                };
                self.handle(cmd);
            }
            let Some(iq_rx) = self.iq_rx.clone() else { continue };
            match iq_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(blk) => {
                    let backlog = iq_rx.len();
                    if backlog > 30 {
                        // we are behind: drop old blocks rather than lag further
                        for _ in 0..backlog - 4 {
                            let _ = iq_rx.try_recv();
                        }
                        self.status.lock().overflow_blocks += 1;
                    }
                    self.process(blk);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.stop_source();
                    let mut st = self.status.lock();
                    st.error = Some("The source stopped (unplugged, end of file or connection lost).".into());
                    drop(st);
                    (self.repaint)();
                }
            }
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Start(cfg) => {
                self.stop_source();
                self.cfg = cfg.clone();
                match source::open(&cfg) {
                    Ok(mut src) => {
                        let (tx, rx) = bounded::<IqBlock>(64);
                        match src.start(tx) {
                            Ok(()) => {
                                let mut st = self.status.lock();
                                st.running = true;
                                st.error = None;
                                st.description = src.description();
                                st.gains = src.gains();
                                st.fixed_tuning = src.fixed_tuning();
                                drop(st);
                                self.source = Some(src);
                                self.iq_rx = Some(rx);
                                self.audio.clear();
                            }
                            Err(e) => self.status.lock().error = Some(e),
                        }
                    }
                    Err(e) => {
                        let mut st = self.status.lock();
                        st.error = Some(e);
                        st.running = false;
                    }
                }
                (self.repaint)();
            }
            Cmd::Stop => {
                self.stop_source();
                (self.repaint)();
            }
            Cmd::Source(cfg) => {
                if let Some(src) = self.source.as_mut() {
                    if let Err(e) = src.apply(&cfg) {
                        self.status.lock().error = Some(e);
                    }
                }
                self.cfg = cfg;
            }
            Cmd::Demod(p) => {
                let mode_changed = p.mode != self.demod_params.mode;
                if mode_changed {
                    self.demod = Demod::new(p.mode, p.width);
                    self.channelizer = None;
                    self.rds = None;
                    self.audio.clear();
                    if self.demod_params.mode == Mode::Dab {
                        self.dab.release();
                        self.status.lock().dab = None;
                    }
                    let mut st = self.status.lock();
                    st.rds = None;
                    st.fm = FmStats::default();
                } else if p.width != self.demod_params.width {
                    self.demod.set_width(p.width);
                }
                if let Demod::Wfm(w) = &mut self.demod {
                    w.mpx.stereo_enabled = p.stereo;
                    w.mpx.set_deemphasis(p.deemphasis_us as f64);
                }
                if !p.rds {
                    self.rds = None;
                    self.status.lock().rds = None;
                }
                // a new station: forget the old RDS data
                if p.mode == Mode::Wfm && p.channel_freq.abs_diff(self.demod_params.channel_freq) >= 50_000 {
                    self.rds = None;
                    self.bler_history.clear();
                    self.status.lock().rds = None;
                }
                self.demod_params = p;
            }
            Cmd::Display { fft_size, averaging, frame_rate } => {
                if fft_size != self.spectrum.size {
                    self.spectrum = SpectrumProcessor::new(fft_size, averaging);
                } else {
                    self.spectrum.averaging = averaging.max(1);
                }
                self.frame_rate = frame_rate.clamp(1, 120);
            }
            Cmd::Squelch { enabled, db } => self.squelch = (enabled, db),
            Cmd::StartRecording { path, only_squelch, stop_bytes, stop_secs } => {
                self.finish_recording();
                match std::fs::File::create(&path) {
                    Ok(f) => {
                        let format = self.source.as_ref().map(|s| s.raw_format());
                        self.recorder = Some(Recorder {
                            file: std::io::BufWriter::with_capacity(4 << 20, f),
                            info: RecordingInfo { path, bytes: 0, seconds: 0.0, format },
                            only_squelch,
                            stop_bytes,
                            stop_secs,
                            rate: self.cfg.sample_rate,
                        });
                    }
                    Err(e) => self.status.lock().error = Some(format!("Could not create {path}: {e}")),
                }
            }
            Cmd::StopRecording => self.finish_recording(),
            Cmd::DabSelect(sid) => self.dab.select(sid),
            Cmd::DabRemembered(m) => self.dab.remembered = m,
            Cmd::MpxView(on) => self.mpx_view = on,
        }
    }

    fn finish_recording(&mut self) {
        if let Some(mut r) = self.recorder.take() {
            let _ = r.file.flush();
            let mut st = self.status.lock();
            st.recording = None;
            st.recording_finished = Some(r.info.clone());
        }
    }

    fn stop_source(&mut self) {
        self.finish_recording();
        if let Some(mut s) = self.source.take() {
            s.stop();
        }
        self.iq_rx = None;
        self.dab.release();
        self.rds = None;
        let mut st = self.status.lock();
        st.running = false;
        st.frames.clear();
        st.dab = None;
        st.rds = None;
        st.fm = FmStats::default();
    }

    fn process(&mut self, blk: IqBlock) {
        let t0 = Instant::now();
        let rate = blk.rate as f64;
        let n = blk.samples.len();
        let center = blk.frequency as f64 + self.cfg.freq_offset as f64;

        // ---- spectrum
        let fft_n = self.spectrum.size;
        if n >= fft_n {
            self.fft_tail.clear();
            self.fft_tail.extend_from_slice(&blk.samples[n - fft_n..]);
        } else {
            self.fft_tail.extend_from_slice(&blk.samples);
            if self.fft_tail.len() > fft_n {
                let d = self.fft_tail.len() - fft_n;
                self.fft_tail.drain(..d);
            }
        }
        self.since_frame += n;
        let frame_every = (rate / self.frame_rate as f64) as usize;
        if self.since_frame >= frame_every && self.fft_tail.len() >= fft_n {
            self.since_frame = 0;
            let db = self.spectrum.compute(&self.fft_tail).to_vec();
            self.channel_db = self.measure_channel(&db, center, rate);
            let input_db = {
                let p: f32 = self.fft_tail.iter().step_by(4).map(|c| c.norm_sqr()).sum::<f32>() / (fft_n / 4) as f32;
                10.0 * p.max(1e-20).log10()
            };
            let mut st = self.status.lock();
            if st.frames.len() >= 16 {
                st.frames.pop_front();
            }
            st.frames.push_back(SpectrumFrame { db, center, rate });
            st.channel_db = self.channel_db;
            st.squelch_open = self.squelch_open;
            st.sample_rate = blk.rate;
            st.hw_frequency = blk.frequency;
            st.input_level_db = input_db;
            drop(st);
            (self.repaint)();
        }
        self.squelch_open = !self.squelch.0 || self.channel_db >= self.squelch.1;

        // ---- recording
        if let Some(r) = self.recorder.as_mut() {
            if !r.only_squelch || self.squelch_open {
                if r.file.write_all(&blk.raw).is_ok() {
                    r.info.bytes += blk.raw.len() as u64;
                    let bps = r.info.format.map(|f| f.bytes_per_sample()).unwrap_or(2) as f64;
                    r.info.seconds = r.info.bytes as f64 / bps / r.rate.max(1) as f64;
                }
            }
            let done = (r.stop_bytes > 0 && r.info.bytes >= r.stop_bytes) || (r.stop_secs > 0 && r.info.seconds >= r.stop_secs as f64);
            let info = r.info.clone();
            self.status.lock().recording = Some(info);
            if done {
                self.finish_recording();
            }
        }

        // ---- demodulation
        let p = self.demod_params.clone();
        self.audio_buf.clear();
        match p.mode {
            Mode::Off => {}
            Mode::Dab => {
                let offset = p.channel_freq as f64 - center;
                let ch_ok = offset.abs() + 768_000.0 <= rate / 2.0;
                if !ch_ok {
                    // channel outside the received band
                } else if blk.rate == DAB_RATE && offset.abs() < 1.0 {
                    self.dab.process(&blk.samples, blk.rate, p.channel_freq, &mut self.audio_buf);
                } else {
                    let key = (blk.rate, Mode::Dab);
                    if self.channelizer.is_none() || self.chan_key != key {
                        self.channelizer = Some(Channelizer::new(rate, DAB_RATE as f64, 780_000.0));
                        self.chan_key = key;
                    }
                    let ch = self.channelizer.as_mut().unwrap();
                    ch.set_offset(offset);
                    self.bb.clear();
                    ch.process(&blk.samples, &mut self.bb);
                    let bb = std::mem::take(&mut self.bb);
                    self.dab.process(&bb, DAB_RATE, p.channel_freq, &mut self.audio_buf);
                    self.bb = bb;
                }
                if !self.audio_buf.is_empty() {
                    self.audio.push(&self.audio_buf);
                }
                let serial = self.dab.state_serial;
                if serial != self.dab_published {
                    let mut st = self.status.lock();
                    st.dab = Some(self.dab.state.clone());
                    st.dab_remembered = self.dab.remembered.clone();
                    st.dab_error = self.dab.error.clone();
                    drop(st);
                    self.dab_published = serial;
                }
            }
            mode => {
                let out_rate = mode.baseband_rate();
                let key = (blk.rate, mode);
                if self.channelizer.is_none() || self.chan_key != key {
                    let pass = match mode {
                        Mode::Wfm => 96_000.0,
                        _ => out_rate as f64 * 0.45,
                    };
                    self.channelizer = Some(Channelizer::new(rate, out_rate as f64, pass));
                    self.chan_key = key;
                }
                let offset = p.channel_freq as f64 - center;
                let ch = self.channelizer.as_mut().unwrap();
                ch.set_offset(offset);
                self.bb.clear();
                ch.process(&blk.samples, &mut self.bb);
                let in_band = offset.abs() < rate / 2.0;
                self.demod.process(&self.bb, &mut self.audio_buf);
                if let Demod::Wfm(w) = &mut self.demod {
                    let mpx = &w.mpx.mpx;
                    if p.rds {
                        let rds = self.rds.get_or_insert_with(|| RdsDecoder::new(crate::dsp::mpx::MPX_RATE as f64));
                        rds.process(mpx);
                    }
                    if self.mpx_view {
                        self.mpx_tail.extend(mpx.iter().map(|&x| C32::new(x, 0.0)));
                        let n = self.mpx_spec.size * 2;
                        if self.mpx_tail.len() > n {
                            let d = self.mpx_tail.len() - n;
                            self.mpx_tail.drain(..d);
                        }
                    }
                }
                if !self.squelch_open || !in_band {
                    for s in self.audio_buf.iter_mut() {
                        *s = [0.0, 0.0];
                    }
                }
                self.audio.push(&self.audio_buf);
            }
        }

        // ---- load
        self.load_acc += t0.elapsed().as_secs_f64();
        self.time_acc += n as f64 / rate;
        if self.last_stat.elapsed() >= Duration::from_millis(500) {
            let load = (self.load_acc / self.time_acc.max(1e-6)) as f32;
            self.load_acc = 0.0;
            self.time_acc = 0.0;
            self.publish_fm_stats(load);
            self.last_stat = Instant::now();
        }
    }

    fn publish_fm_stats(&mut self, load: f32) {
        let mut fm = FmStats::default();
        let mut mpx_spec = Vec::new();
        if let Demod::Wfm(w) = &mut self.demod {
            fm.active = true;
            fm.pilot_db = w.mpx.pilot_level_db();
            fm.pilot_locked = w.mpx.pilot_locked;
            fm.stereo = w.mpx.is_stereo();
            fm.deviation_khz = w.mpx.take_peak_deviation_hz() / 1000.0;
            fm.pll_hz = w.mpx.pll_freq_hz();
            if self.mpx_view && self.mpx_tail.len() >= self.mpx_spec.size {
                let tail = self.mpx_tail.clone();
                let db = self.mpx_spec.compute(&tail);
                // real signal: keep the positive half (0..96 kHz)
                let n = db.len();
                mpx_spec = db[n / 2..].to_vec();
            }
        }
        let mut rds_station = None;
        let mut rs = RdsStats::default();
        if let Some(r) = self.rds.as_mut() {
            fm.rds_level = r.demod.carrier_level;
            fm.costas_lock = r.demod.costas_lock;
            let groups = r.sync.groups_total;
            let gps = (groups - self.rds_last_groups) as f32 / 0.5;
            self.rds_last_groups = groups;
            let bler = r.sync.bler_recent();
            self.bler_history.push(if r.sync.synced { bler } else { 1.0 });
            if self.bler_history.len() > 120 {
                self.bler_history.remove(0);
            }
            rs = RdsStats {
                synced: r.sync.synced,
                bler,
                bler_history: self.bler_history.clone(),
                groups_per_sec: gps,
                groups_total: groups,
                blocks_ok: r.sync.blocks_ok,
                blocks_corrected: r.sync.blocks_corrected,
                blocks_failed: r.sync.blocks_failed,
                symbols: r.demod.symbols.iter().copied().collect(),
            };
            if r.parser.st.pi.is_some() {
                rds_station = Some(r.parser.st.clone());
            }
        } else {
            self.rds_last_groups = 0;
        }
        let mut st = self.status.lock();
        st.load = load;
        st.fm = fm;
        if !mpx_spec.is_empty() {
            st.mpx_spectrum = mpx_spec;
        }
        st.rds = rds_station;
        st.rds_stats = rs;
        drop(st);
        (self.repaint)();
    }

    /// Mean power over the channel's passband, from the spectrum (dBFS per bin).
    fn measure_channel(&self, db: &[f32], center: f64, rate: f64) -> f32 {
        let p = &self.demod_params;
        if p.mode == Mode::Off {
            return -150.0;
        }
        let (lo, hi) = p.mode.passband(p.width);
        let n = db.len() as f64;
        let f_lo = p.channel_freq as f64 + lo - center;
        let f_hi = p.channel_freq as f64 + hi - center;
        let bin = |f: f64| (((f / rate) + 0.5) * n).round().clamp(0.0, n - 1.0) as usize;
        let (a, b) = (bin(f_lo), bin(f_hi.max(f_lo + rate / n)));
        if b <= a {
            return db[a];
        }
        let mean_p: f64 = db[a..=b.min(db.len() - 1)].iter().map(|&d| 10f64.powf(d as f64 / 10.0)).sum::<f64>() / (b - a + 1) as f64;
        // total channel power: sum over the bins
        (10.0 * (mean_p * (b - a + 1) as f64).max(1e-20).log10()) as f32
    }
}


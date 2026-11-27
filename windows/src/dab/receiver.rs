//! Drives the native DAB/DAB+ receiver from the engine thread: pushes the 2.048 Msps channel
//! samples, folds its events into a DabState, plays the selected service and paces its audio by
//! the input sample clock. A new channel gets a fresh native receiver (as in the Android fork).

use super::ffi::Native;
use super::model::DabState;
use crate::dsp::C32;
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

pub const INPUT_RATE: u32 = 2_048_000;
const PREBUFFER_MS: usize = 300;

/// Engine-side wrapper around the native decoder. All times are seconds since `start`.
///
/// Audio pacing: the native decoder produces audio in bursts (one superframe = 120 ms at a
/// time) and on its own threads. To play smoothly at the rate the radio actually delivers,
/// decoded audio goes into `fifo`, and each IQ block "earns" `owed` output frames in
/// proportion to its length (2.048 M input samples = 48 000 audio frames). Only owed frames
/// are released, so the audio clock follows the SDR clock.
pub struct DabReceiver {
    /// The C++ receiver; None until the first samples arrive, or after `release`.
    native: Option<Native>,
    /// Ensemble state built from the decoder's events (see model.rs).
    pub state: DabState,
    start: Instant,
    tune_time: f64,
    /// Times of the last 0.5 s statistics update and the last "describe" requests.
    last_status: f64,
    last_describe: f64,
    /// Service to start, retried once a second until the decoder accepts it.
    pending_sid: Option<u32>,
    last_select_attempt: f64,
    /// When the first service appeared (0 = none yet); auto-select waits 4 s after it.
    first_service: f64,
    /// Ensemble key ("e<id>" or "f<kHz>") → last service chosen there. Shared with the UI
    /// via Status so it ends up in settings.
    pub remembered: HashMap<String, u32>,
    /// Crash guard: the SId that was playing when the app last died. Never auto-played.
    pub suspect: Option<u32>,
    /// Whether the guard file currently names the playing service.
    guard_set: bool,
    /// Milliseconds of audio played since selection; the guard is cleared after 10 s.
    played_ms: f64,
    last_tick: f64,
    /// Input statistics for the current 0.5 s period (sample count, power sum over every
    /// 16th sample, and how many samples that sum covers).
    input_count: u64,
    input_power: f64,
    input_power_n: u64,
    audio_in_frames: u64,
    audio_out_frames: u64,
    /// Scratch buffer for interleaved 16-bit PCM pulled from the decoder.
    pcm: Vec<i16>,
    /// Decoded audio at 48 kHz waiting to be released (max 2 s).
    fifo: VecDeque<[f32; 2]>,
    /// False while prebuffering (needs 300 ms in `fifo` before output starts).
    playing: bool,
    /// The decoder's output rate (32/48 kHz for DAB+, 48 kHz for MP2 mostly).
    audio_rate: i32,
    /// Output frames earned by input samples but not yet released.
    owed: f64,
    /// Linear resampler state for non-48 kHz services.
    resample_pos: f64,
    resample_prev: [f32; 2],
    pub error: Option<String>,
    /// Incremented on every change to `state`; the engine copies the state only when it moves.
    pub state_serial: u64,
}

/// %APPDATA%\RFAnalyzer\dab_playing_guard.txt: holds the SId being played until it has played
/// for 10 s. If the app crashes during that time, the file survives and names the culprit.
pub fn guard_path() -> Option<std::path::PathBuf> {
    let dir = crate::config_dir()?;
    Some(dir.join("dab_playing_guard.txt"))
}

impl DabReceiver {
    pub fn new() -> Self {
        // A service that was playing when the app last crashed is not auto-played again.
        let suspect = guard_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.trim().parse::<u32>().ok());
        Self {
            native: None,
            state: DabState::default(),
            start: Instant::now(),
            tune_time: 0.0,
            last_status: 0.0,
            last_describe: 0.0,
            pending_sid: None,
            last_select_attempt: 0.0,
            first_service: 0.0,
            remembered: HashMap::new(),
            suspect,
            guard_set: false,
            played_ms: 0.0,
            last_tick: 0.0,
            input_count: 0,
            input_power: 0.0,
            input_power_n: 0,
            audio_in_frames: 0,
            audio_out_frames: 0,
            pcm: vec![0; 16384],
            fifo: VecDeque::with_capacity(96_000),
            playing: false,
            audio_rate: 48_000,
            owed: 0.0,
            resample_pos: 0.0,
            resample_prev: [0.0; 2],
            error: None,
            state_serial: 0,
        }
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn ensure_started(&mut self) -> bool {
        if self.native.is_some() {
            return true;
        }
        match Native::create() {
            Some(n) => {
                n.start();
                self.native = Some(n);
                true
            }
            None => {
                self.error = Some("The DAB decoder is not available in this build.".into());
                false
            }
        }
    }

    pub fn release(&mut self) {
        if let Some(n) = self.native.take() {
            n.stop();
        }
        self.state = DabState::default();
        self.clear_audio();
        self.clear_guard();
        self.state_serial += 1;
    }

    fn clear_audio(&mut self) {
        self.fifo.clear();
        self.playing = false;
        self.owed = 0.0;
    }

    fn set_guard(&mut self, sid: u32) {
        if let Some(p) = guard_path() {
            let _ = std::fs::write(p, sid.to_string());
            self.guard_set = true;
        }
    }

    fn clear_guard(&mut self) {
        if self.guard_set {
            if let Some(p) = guard_path() {
                let _ = std::fs::remove_file(p);
            }
            self.guard_set = false;
        }
    }

    pub fn select(&mut self, sid: u32) {
        self.pending_sid = Some(sid);
        self.last_select_attempt = 0.0;
        if self.suspect == Some(sid) {
            // the user picked it on purpose
            self.suspect = None;
        }
    }

    /// Key for `remembered`: the ensemble id once known, else the frequency in kHz.
    fn remember_key(&self) -> String {
        if self.state.ensemble_id >= 0 { format!("e{}", self.state.ensemble_id) } else { format!("f{}", self.state.frequency / 1000) }
    }

    /// Feeds channel samples centred on `freq`. Returns audio frames (48 kHz stereo) to play.
    pub fn process(&mut self, samples: &[C32], rate: u32, freq: u64, out: &mut Vec<[f32; 2]>) {
        if samples.is_empty() || rate != INPUT_RATE {
            return;
        }
        if !self.ensure_started() {
            return;
        }
        let now = self.now();
        if !self.state.active || freq.abs_diff(self.state.frequency) > 100_000 {
            if self.state.active {
                // fresh receiver for a new channel
                if let Some(n) = self.native.take() {
                    n.stop();
                }
                if !self.ensure_started() {
                    return;
                }
            }
            self.state = DabState { active: true, frequency: freq, ensemble_id: -1, audio_rate: 48_000, ..Default::default() };
            self.tune_time = now;
            self.pending_sid = None;
            self.first_service = 0.0;
            self.clear_audio();
            self.state_serial += 1;
        } else if freq != self.state.frequency {
            self.state.frequency = freq;
        }

        // input level estimate from every 16th sample (cheap and plenty accurate)
        self.input_count += samples.len() as u64;
        let mut p = 0.0f64;
        for s in samples.iter().step_by(16) {
            p += s.norm_sqr() as f64;
        }
        self.input_power += p;
        self.input_power_n += samples.len().div_ceil(16) as u64;
        let native = self.native.as_ref().unwrap();
        native.push(samples);

        // events arrive as a block of JSON lines; apply each to the state
        if let Some(ev) = native.poll_events() {
            for line in ev.lines() {
                if !line.trim().is_empty() {
                    self.state.apply_event(line, now);
                }
            }
            self.state_serial += 1;
        }
        if let Some((data, name, typ)) = native.take_slide() {
            self.state.slide = Some(std::sync::Arc::new(data));
            self.state.slide_name = name;
            self.state.slide_type = typ;
            self.state.slide_serial += 1;
            self.state_serial += 1;
        }

        self.handle_selection(now);
        self.pull_audio();
        // this block earns len · 48000/2048000 output frames (capped at 1 s of credit)
        self.owed = (self.owed + samples.len() as f64 * 48_000.0 / INPUT_RATE as f64).min(48_000.0);
        self.paced_output(out);

        if now - self.last_status >= 0.5 {
            let native = self.native.as_ref().unwrap();
            // ask the decoder for details (subchannel, bitrate...) of services that lack them
            if now - self.last_describe >= 1.0 {
                self.last_describe = now;
                for s in self.state.services.iter().filter(|s| !s.described()) {
                    native.describe(s.sid);
                }
            }
            let pts = native.poll_constellation(1536);
            if !pts.is_empty() {
                self.state.constellation = pts;
            }
            let dt = (now - self.last_status).max(0.001);
            self.state.input_rate = (self.input_count as f64 / dt) as i32;
            self.state.input_level_db = if self.input_power_n > 0 { (10.0 * (self.input_power / self.input_power_n as f64 + 1e-12).log10()) as f32 } else { -120.0 };
            self.input_count = 0;
            self.input_power = 0.0;
            self.input_power_n = 0;
            self.state.audio_in_per_sec = (self.audio_in_frames as f64 / dt) as i32;
            self.state.audio_out_per_sec = (self.audio_out_frames as f64 / dt) as i32;
            self.audio_in_frames = 0;
            self.audio_out_frames = 0;
            self.state.audio_rate = self.audio_rate;
            self.state.audio_buffered_ms = (self.fifo.len() * 1000 / 48_000) as i32;
            self.state.dropped_samples = native.dropped() as u64;
            self.state.seconds_since_tune = (now - self.tune_time) as f32;
            if self.state.sync {
                self.state.snr_history.push(self.state.snr as f32);
            } else {
                self.state.snr_history.push(0.0);
            }
            if self.state.snr_history.len() > 240 {
                self.state.snr_history.remove(0);
            }
            self.last_status = now;
            self.state_serial += 1;
        }
    }

    /// Chooses what to play and starts it.
    /// Auto-select (nothing chosen yet, sync present): the remembered service for this
    /// ensemble if it is known and described, otherwise, 4 s after the first service showed
    /// up, the first audio service. Never the crash suspect.
    fn handle_selection(&mut self, now: f64) {
        if !self.state.services.is_empty() && self.first_service == 0.0 {
            self.first_service = now;
        }
        if self.pending_sid.is_none() && self.state.selected_sid.is_none() && self.first_service > 0.0 && self.state.sync {
            let remembered = self.remembered.get(&self.remember_key()).copied();
            // `ok` takes `self` as a parameter (not captured) so it can be called while
            // `self.pending_sid` is being assigned
            let ok = |sid: u32, s: &Self| s.suspect != Some(sid) && s.state.services.iter().any(|x| x.sid == sid && x.described());
            self.pending_sid = match remembered {
                Some(r) if ok(r, self) => Some(r),
                _ if now - self.first_service > 4.0 => {
                    self.state.services.iter().find(|s| s.is_audio == Some(true) && self.suspect != Some(s.sid)).map(|s| s.sid)
                }
                _ => None,
            };
        }
        if let Some(sid) = self.pending_sid {
            if Some(sid) != self.state.selected_sid && now - self.last_select_attempt >= 1.0 {
                self.last_select_attempt = now;
                // write the guard *before* starting playback, so a crash inside the decoder
                // leaves it behind
                self.set_guard(sid);
                self.played_ms = 0.0;
                if self.native.as_ref().map(|n| n.select(sid)).unwrap_or(false) {
                    self.clear_audio();
                    self.state.selected_sid = Some(sid);
                    let key = self.remember_key();
                    self.remembered.insert(key, sid);
                    self.pending_sid = None;
                    self.state_serial += 1;
                }
            } else if Some(sid) == self.state.selected_sid {
                self.pending_sid = None;
            }
        }
        if self.playing {
            self.played_ms += ((now - self.last_tick) * 1000.0).clamp(0.0, 1000.0);
        }
        self.last_tick = now;
        if self.guard_set && self.played_ms > 10_000.0 {
            self.clear_guard();
        }
    }

    /// Moves all decoded PCM from the decoder into `fifo`, converting i16 → f32 and
    /// resampling to 48 kHz if needed. `pcm` is interleaved L,R,L,R; `k` is the sample count.
    fn pull_audio(&mut self) {
        let Some(native) = self.native.as_ref() else { return };
        loop {
            let k = native.poll_audio(&mut self.pcm);
            if k == 0 {
                break;
            }
            let rate = native.audio_rate();
            if rate > 0 && rate != self.audio_rate {
                self.audio_rate = rate;
                self.fifo.clear();
                self.playing = false;
            }
            self.audio_in_frames += (k / 2) as u64;
            let step = self.audio_rate as f64 / 48_000.0;
            for fr in self.pcm[..k].chunks_exact(2) {
                let cur = [fr[0] as f32 / 32768.0, fr[1] as f32 / 32768.0];
                if self.audio_rate == 48_000 {
                    self.fifo.push_back(cur);
                } else {
                    // linear resampling to 48 kHz: emit outputs at positions resample_pos
                    // (0..1 between prev and cur), stepping by in_rate/48000
                    while self.resample_pos < 1.0 {
                        let mu = self.resample_pos as f32;
                        self.fifo.push_back([
                            self.resample_prev[0] + (cur[0] - self.resample_prev[0]) * mu,
                            self.resample_prev[1] + (cur[1] - self.resample_prev[1]) * mu,
                        ]);
                        self.resample_pos += step;
                    }
                    self.resample_pos -= 1.0;
                    self.resample_prev = cur;
                }
            }
            while self.fifo.len() > 96_000 {
                self.fifo.pop_front();
            }
            // a partly filled buffer means the decoder is drained for now
            if k < self.pcm.len() {
                break;
            }
        }
    }

    /// Releases the owed number of frames from `fifo`, nudged ±2 % when the fifo drifts far
    /// from the 300 ms target, so long-term clock differences don't empty or overfill it.
    fn paced_output(&mut self, out: &mut Vec<[f32; 2]>) {
        let target = 48_000 * PREBUFFER_MS / 1000;
        if !self.playing {
            if self.fifo.len() < target {
                self.owed = 0.0;
                return;
            }
            self.playing = true;
            self.owed = 0.0;
        }
        if self.fifo.is_empty() {
            self.playing = false;
            return;
        }
        let nudge = if self.fifo.len() > 2 * target {
            1.02
        } else if self.fifo.len() < target / 2 {
            0.98
        } else {
            1.0
        };
        let due = (self.owed * nudge) as usize;
        let k = due.min(self.fifo.len());
        if k == 0 {
            return;
        }
        self.owed -= k as f64 / nudge;
        out.extend(self.fifo.drain(..k));
        self.audio_out_frames += k as u64;
    }
}

impl Drop for DabReceiver {
    fn drop(&mut self) {
        self.release();
    }
}

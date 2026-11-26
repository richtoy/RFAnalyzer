//! Audio output through cpal (WASAPI on Windows). The engine pushes 48 kHz stereo into a queue;
//! the output callback resamples to the device rate and gently adapts the ratio to keep the
//! queue near its target fill, so source and sound card clocks can differ.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

pub const IN_RATE: f64 = 48_000.0;

pub struct AudioShared {
    queue: Mutex<VecDeque<[f32; 2]>>,
    volume: AtomicU32,
    muted: AtomicBool,
    pub underruns: AtomicU32,
    pub overruns: AtomicU32,
    /// Peak level of the last callback (f32 bits), for the meter
    pub level: AtomicU32,
}

impl AudioShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(VecDeque::with_capacity(48_000)),
            volume: AtomicU32::new(0.7f32.to_bits()),
            muted: AtomicBool::new(false),
            underruns: AtomicU32::new(0),
            overruns: AtomicU32::new(0),
            level: AtomicU32::new(0),
        })
    }
    pub fn push(&self, samples: &[[f32; 2]]) {
        let mut q = self.queue.lock();
        q.extend(samples.iter().copied());
        // never let latency grow beyond ~0.5 s
        if q.len() > 24_000 {
            let drop = q.len() - 9_600;
            q.drain(..drop);
            self.overruns.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn clear(&self) {
        self.queue.lock().clear();
    }
    pub fn set_volume(&self, v: f32) {
        self.volume.store(v.to_bits(), Ordering::Relaxed);
    }
    pub fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Relaxed);
    }
    #[allow(dead_code)]
    pub fn queued(&self) -> usize {
        self.queue.lock().len()
    }
}

pub struct AudioOut {
    _stream: cpal::Stream,
    pub device_name: String,
    pub device_rate: u32,
}

pub fn output_device_names() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|it| it.filter_map(|d| d.description().ok().map(|x| x.name().to_string())).collect())
        .unwrap_or_default()
}

impl AudioOut {
    pub fn open(shared: Arc<AudioShared>, wanted: &str) -> Result<AudioOut, String> {
        let host = cpal::default_host();
        let mut device = None;
        if !wanted.is_empty() {
            if let Ok(devs) = host.output_devices() {
                for d in devs {
                    if d.description().map(|x| x.name() == wanted).unwrap_or(false) {
                        device = Some(d);
                        break;
                    }
                }
            }
        }
        let device = match device {
            Some(d) => d,
            None => host.default_output_device().ok_or("No audio output device")?,
        };
        let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "audio".into());
        let supported = device.default_output_config().map_err(|e| format!("audio: {e}"))?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let rate = config.sample_rate;
        let channels = config.channels as usize;
        let err_fn = |e: cpal::Error| eprintln!("audio stream error: {e}");
        let mut state = Resampler::new(IN_RATE / rate as f64);
        let sh = shared.clone();
        let stream = match format {
            cpal::SampleFormat::F32 => device.build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| state.fill(&sh, data, channels, |x| x),
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| state.fill(&sh, data, channels, |x| (x * 32767.0) as i16),
                err_fn,
                None,
            ),
            cpal::SampleFormat::I32 => device.build_output_stream(
                config,
                move |data: &mut [i32], _: &cpal::OutputCallbackInfo| state.fill(&sh, data, channels, |x| (x as f64 * 2147483647.0) as i32),
                err_fn,
                None,
            ),
            other => return Err(format!("audio: unsupported sample format {other:?}")),
        }
        .map_err(|e| format!("audio: {e}"))?;
        stream.play().map_err(|e| format!("audio: {e}"))?;
        Ok(AudioOut { _stream: stream, device_name: name, device_rate: rate })
    }
}

struct Resampler {
    nominal: f64, // input samples per output sample
    pos: f64,
    prev: [f32; 2],
    cur: [f32; 2],
    primed: bool,
}

impl Resampler {
    fn new(nominal: f64) -> Self {
        Self { nominal, pos: 0.0, prev: [0.0; 2], cur: [0.0; 2], primed: false }
    }

    fn fill<T: Copy>(&mut self, sh: &AudioShared, data: &mut [T], channels: usize, conv: impl Fn(f32) -> T) {
        let vol = if sh.muted.load(Ordering::Relaxed) { 0.0 } else { f32::from_bits(sh.volume.load(Ordering::Relaxed)) };
        let mut q = sh.queue.lock();
        let frames = data.len() / channels.max(1);
        // target ~100 ms queued; wait until that much is there before (re)starting
        let target = 4_800.0;
        if !self.primed {
            if (q.len() as f64) < target {
                for x in data.iter_mut() {
                    *x = conv(0.0);
                }
                return;
            }
            self.primed = true;
        }
        let err = (q.len() as f64 - target) / target;
        let step = self.nominal * (1.0 + (err * 0.005).clamp(-0.01, 0.01));
        let mut peak = 0f32;
        for f in 0..frames {
            self.pos += step;
            while self.pos >= 1.0 {
                self.pos -= 1.0;
                self.prev = self.cur;
                match q.pop_front() {
                    Some(s) => self.cur = s,
                    None => {
                        self.primed = false;
                        sh.underruns.fetch_add(1, Ordering::Relaxed);
                        self.cur = [0.0; 2];
                    }
                }
            }
            let mu = self.pos as f32;
            let l = (self.prev[0] + (self.cur[0] - self.prev[0]) * mu) * vol;
            let r = (self.prev[1] + (self.cur[1] - self.prev[1]) * mu) * vol;
            peak = peak.max(l.abs()).max(r.abs());
            let base = f * channels;
            if channels == 1 {
                data[base] = conv((l + r) * 0.5);
            } else {
                data[base] = conv(l.clamp(-1.0, 1.0));
                data[base + 1] = conv(r.clamp(-1.0, 1.0));
                for c in 2..channels {
                    data[base + c] = conv(0.0);
                }
            }
        }
        sh.level.store(peak.to_bits(), Ordering::Relaxed);
    }
}

//! RTL-SDR through rtlsdr.dll (librtlsdr; the RTL-SDR Blog build supports the V4).
//!
//! The library is normally linked statically (feature `static-sdr`, built by build.rs); the
//! DLL loader path remains for builds without it. Either way the functions end up in an
//! [`Api`] table of function pointers, so the rest of the code doesn't care which.
//! Reading uses `rtlsdr_read_sync` in a loop on our own thread.

#[allow(unused_imports)]
use super::{convert_u8, hw_freq, load_library, IqBlock, SendPtr, Source};
use crate::settings::{FileFormat, SourceConfig};
use crossbeam_channel::Sender;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Opaque `rtlsdr_dev_t*` handle.
type Dev = *mut c_void;

/// librtlsdr entry points as function pointers. `_lib` keeps a dynamically loaded DLL alive
/// for as long as the pointers are in use (None when statically linked).
struct Api {
    _lib: Option<libloading::Library>,
    get_device_count: unsafe extern "C" fn() -> u32,
    get_device_name: unsafe extern "C" fn(u32) -> *const c_char,
    open: unsafe extern "C" fn(*mut Dev, u32) -> c_int,
    close: unsafe extern "C" fn(Dev) -> c_int,
    set_center_freq: unsafe extern "C" fn(Dev, u32) -> c_int,
    set_freq_correction: unsafe extern "C" fn(Dev, c_int) -> c_int,
    get_tuner_gains: unsafe extern "C" fn(Dev, *mut c_int) -> c_int,
    set_tuner_gain: unsafe extern "C" fn(Dev, c_int) -> c_int,
    set_tuner_gain_mode: unsafe extern "C" fn(Dev, c_int) -> c_int,
    set_sample_rate: unsafe extern "C" fn(Dev, u32) -> c_int,
    set_agc_mode: unsafe extern "C" fn(Dev, c_int) -> c_int,
    set_direct_sampling: unsafe extern "C" fn(Dev, c_int) -> c_int,
    reset_buffer: unsafe extern "C" fn(Dev) -> c_int,
    read_sync: unsafe extern "C" fn(Dev, *mut c_void, c_int, *mut c_int) -> c_int,
    set_bias_tee: Option<unsafe extern "C" fn(Dev, c_int) -> c_int>,
    get_tuner_type: unsafe extern "C" fn(Dev) -> c_int,
}

#[cfg(feature = "static-sdr")]
mod linked {
    use std::ffi::{c_char, c_int, c_void};
    type Dev = *mut c_void;
    unsafe extern "C" {
        pub fn rtlsdr_get_device_count() -> u32;
        pub fn rtlsdr_get_device_name(i: u32) -> *const c_char;
        pub fn rtlsdr_open(d: *mut Dev, i: u32) -> c_int;
        pub fn rtlsdr_close(d: Dev) -> c_int;
        pub fn rtlsdr_set_center_freq(d: Dev, f: u32) -> c_int;
        pub fn rtlsdr_set_freq_correction(d: Dev, ppm: c_int) -> c_int;
        pub fn rtlsdr_get_tuner_gains(d: Dev, g: *mut c_int) -> c_int;
        pub fn rtlsdr_set_tuner_gain(d: Dev, g: c_int) -> c_int;
        pub fn rtlsdr_set_tuner_gain_mode(d: Dev, m: c_int) -> c_int;
        pub fn rtlsdr_set_sample_rate(d: Dev, r: u32) -> c_int;
        pub fn rtlsdr_set_agc_mode(d: Dev, on: c_int) -> c_int;
        pub fn rtlsdr_set_direct_sampling(d: Dev, on: c_int) -> c_int;
        pub fn rtlsdr_reset_buffer(d: Dev) -> c_int;
        pub fn rtlsdr_read_sync(d: Dev, buf: *mut c_void, len: c_int, n: *mut c_int) -> c_int;
        pub fn rtlsdr_set_bias_tee(d: Dev, on: c_int) -> c_int;
        pub fn rtlsdr_get_tuner_type(d: Dev) -> c_int;
    }
}

impl Api {
    #[cfg(feature = "static-sdr")]
    fn load() -> Result<Api, String> {
        use linked::*;
        Ok(Api {
            _lib: None,
            get_device_count: rtlsdr_get_device_count,
            get_device_name: rtlsdr_get_device_name,
            open: rtlsdr_open,
            close: rtlsdr_close,
            set_center_freq: rtlsdr_set_center_freq,
            set_freq_correction: rtlsdr_set_freq_correction,
            get_tuner_gains: rtlsdr_get_tuner_gains,
            set_tuner_gain: rtlsdr_set_tuner_gain,
            set_tuner_gain_mode: rtlsdr_set_tuner_gain_mode,
            set_sample_rate: rtlsdr_set_sample_rate,
            set_agc_mode: rtlsdr_set_agc_mode,
            set_direct_sampling: rtlsdr_set_direct_sampling,
            reset_buffer: rtlsdr_reset_buffer,
            read_sync: rtlsdr_read_sync,
            set_bias_tee: Some(rtlsdr_set_bias_tee),
            get_tuner_type: rtlsdr_get_tuner_type,
        })
    }

    #[cfg(not(feature = "static-sdr"))]
    fn load() -> Result<Api, String> {
        let lib = load_library(&["rtlsdr.dll", "librtlsdr.dll", "librtlsdr.so.0", "librtlsdr.so"])?;
        unsafe {
            // sym!("name") looks up a NUL-terminated symbol and copies out the function
            // pointer; `?` returns a readable error if it's missing
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("rtlsdr.dll: missing {}: {e}", $name))?
                };
            }
            let set_bias_tee = lib.get(b"rtlsdr_set_bias_tee\0").ok().map(|s| *s);
            Ok(Api {
                get_device_count: sym!("rtlsdr_get_device_count"),
                get_device_name: sym!("rtlsdr_get_device_name"),
                open: sym!("rtlsdr_open"),
                close: sym!("rtlsdr_close"),
                set_center_freq: sym!("rtlsdr_set_center_freq"),
                set_freq_correction: sym!("rtlsdr_set_freq_correction"),
                get_tuner_gains: sym!("rtlsdr_get_tuner_gains"),
                set_tuner_gain: sym!("rtlsdr_set_tuner_gain"),
                set_tuner_gain_mode: sym!("rtlsdr_set_tuner_gain_mode"),
                set_sample_rate: sym!("rtlsdr_set_sample_rate"),
                set_agc_mode: sym!("rtlsdr_set_agc_mode"),
                set_direct_sampling: sym!("rtlsdr_set_direct_sampling"),
                reset_buffer: sym!("rtlsdr_reset_buffer"),
                read_sync: sym!("rtlsdr_read_sync"),
                get_tuner_type: sym!("rtlsdr_get_tuner_type"),
                set_bias_tee,
                _lib: Some(lib),
            })
        }
    }
}

pub fn list_devices() -> Vec<String> {
    match Api::load() {
        Ok(api) => unsafe {
            let n = (api.get_device_count)();
            (0..n)
                .map(|i| {
                    let p = (api.get_device_name)(i);
                    if p.is_null() { format!("Device {i}") } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
                })
                .collect()
        },
        Err(_) => Vec::new(),
    }
}

pub struct RtlSdrSource {
    /// Shared with the reader thread (Arc) so it can call read_sync.
    api: Arc<Api>,
    dev: SendPtr,
    /// Last applied settings, to send only what changes in `apply`.
    cfg: SourceConfig,
    /// Supported tuner gains, tenths of dB.
    gains: Vec<i32>,
    /// Shared with the reader thread (see rtltcp.rs for the same pattern).
    running: Arc<AtomicBool>,
    freq: Arc<AtomicU64>,
    rate: Arc<AtomicU32>,
    thread: Option<JoinHandle<()>>,
    name: String,
    tuner: i32,
}

impl RtlSdrSource {
    pub fn open(cfg: &SourceConfig) -> Result<Self, String> {
        let api = Arc::new(Api::load()?);
        let mut dev: Dev = std::ptr::null_mut();
        unsafe {
            let n = (api.get_device_count)();
            if n == 0 {
                return Err("No RTL-SDR found. Is it plugged in, with the WinUSB driver installed (Zadig)?".into());
            }
            let idx = cfg.rtl_device_index.min(n - 1);
            let r = (api.open)(&mut dev, idx);
            if r < 0 || dev.is_null() {
                return Err(format!("Could not open the RTL-SDR (error {r}). Is another program using it?"));
            }
            let name = {
                let p = (api.get_device_name)(idx);
                if p.is_null() { "RTL-SDR".into() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
            };
            // C idiom: call once with NULL to get the count, then again with a buffer
            let n_gains = (api.get_tuner_gains)(dev, std::ptr::null_mut());
            let mut gains = vec![0 as c_int; n_gains.max(0) as usize];
            if n_gains > 0 {
                (api.get_tuner_gains)(dev, gains.as_mut_ptr());
            }
            let tuner = (api.get_tuner_type)(dev);
            let mut s = Self {
                api,
                dev: SendPtr(dev),
                cfg: cfg.clone(),
                gains: gains.into_iter().collect(),
                running: Arc::new(AtomicBool::new(false)),
                freq: Arc::new(AtomicU64::new(hw_freq(cfg))),
                rate: Arc::new(AtomicU32::new(cfg.sample_rate)),
                thread: None,
                name,
                tuner,
            };
            s.apply_all(cfg)?;
            Ok(s)
        }
    }

    fn apply_all(&mut self, cfg: &SourceConfig) -> Result<(), String> {
        let d = self.dev.0;
        let a = &self.api;
        unsafe {
            (a.set_sample_rate)(d, cfg.sample_rate);
            (a.set_direct_sampling)(d, cfg.rtl_direct_sampling as c_int);
            (a.set_center_freq)(d, hw_freq(cfg) as u32);
            if cfg.ppm != 0 {
                (a.set_freq_correction)(d, cfg.ppm);
            }
            (a.set_agc_mode)(d, cfg.rtl_agc as c_int);
            (a.set_tuner_gain_mode)(d, cfg.rtl_manual_gain as c_int);
            if cfg.rtl_manual_gain {
                (a.set_tuner_gain)(d, self.nearest_gain(cfg.rtl_gain));
            }
            if let Some(f) = a.set_bias_tee {
                f(d, cfg.rtl_bias_tee as c_int);
            }
        }
        self.freq.store(hw_freq(cfg), Ordering::Relaxed);
        self.rate.store(cfg.sample_rate, Ordering::Relaxed);
        self.cfg = cfg.clone();
        Ok(())
    }

    fn nearest_gain(&self, g: i32) -> i32 {
        self.gains.iter().copied().min_by_key(|x| (x - g).abs()).unwrap_or(g)
    }
}

impl Source for RtlSdrSource {
    fn start(&mut self, tx: Sender<IqBlock>) -> Result<(), String> {
        let api = self.api.clone();
        let dev = self.dev;
        let running = self.running.clone();
        let freq = self.freq.clone();
        let rate = self.rate.clone();
        running.store(true, Ordering::SeqCst);
        unsafe {
            (api.reset_buffer)(dev.0);
        }
        self.thread = Some(
            std::thread::Builder::new()
                .name("rtlsdr-rx".into())
                .spawn(move || {
                    let dev = dev;
                    let mut errors = 0;
                    while running.load(Ordering::Relaxed) {
                        let r = rate.load(Ordering::Relaxed);
                        // ~20 blocks per second, multiple of 512 bytes
                        let len = ((r as usize * 2 / 20) / 16384).clamp(1, 32) * 16384;
                        let mut raw = vec![0u8; len];
                        let mut n_read: c_int = 0;
                        // blocking read straight into the Vec's buffer; n_read = bytes filled.
                        // 50 consecutive failures (~0.5 s) = device gone; end the thread
                        let res = unsafe { (api.read_sync)(dev.0, raw.as_mut_ptr() as *mut c_void, len as c_int, &mut n_read) };
                        if res < 0 || n_read <= 0 {
                            errors += 1;
                            if errors > 50 {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(10));
                            continue;
                        }
                        errors = 0;
                        raw.truncate(n_read as usize);
                        let mut samples = Vec::new();
                        convert_u8(&raw, &mut samples);
                        let blk = IqBlock { samples, raw, frequency: freq.load(Ordering::Relaxed), rate: r };
                        if tx.send(blk).is_err() {
                            break;
                        }
                    }
                    running.store(false, Ordering::SeqCst);
                })
                .map_err(|e| e.to_string())?,
        );
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn apply(&mut self, cfg: &SourceConfig) -> Result<(), String> {
        let d = self.dev.0;
        let a = self.api.clone();
        let old = self.cfg.clone();
        unsafe {
            if cfg.sample_rate != old.sample_rate {
                (a.set_sample_rate)(d, cfg.sample_rate);
                self.rate.store(cfg.sample_rate, Ordering::Relaxed);
                (a.reset_buffer)(d);
            }
            if cfg.rtl_direct_sampling != old.rtl_direct_sampling {
                (a.set_direct_sampling)(d, cfg.rtl_direct_sampling as c_int);
            }
            if hw_freq(cfg) != hw_freq(&old) || cfg.rtl_direct_sampling != old.rtl_direct_sampling {
                (a.set_center_freq)(d, hw_freq(cfg) as u32);
                self.freq.store(hw_freq(cfg), Ordering::Relaxed);
            }
            if cfg.ppm != old.ppm {
                (a.set_freq_correction)(d, cfg.ppm);
            }
            if cfg.rtl_agc != old.rtl_agc {
                (a.set_agc_mode)(d, cfg.rtl_agc as c_int);
            }
            if cfg.rtl_manual_gain != old.rtl_manual_gain {
                (a.set_tuner_gain_mode)(d, cfg.rtl_manual_gain as c_int);
            }
            if cfg.rtl_manual_gain && (cfg.rtl_gain != old.rtl_gain || cfg.rtl_manual_gain != old.rtl_manual_gain) {
                (a.set_tuner_gain)(d, self.nearest_gain(cfg.rtl_gain));
            }
            if cfg.rtl_bias_tee != old.rtl_bias_tee {
                if let Some(f) = a.set_bias_tee {
                    f(d, cfg.rtl_bias_tee as c_int);
                }
            }
        }
        self.cfg = cfg.clone();
        Ok(())
    }

    fn raw_format(&self) -> FileFormat {
        FileFormat::Cu8
    }

    fn description(&self) -> String {
        let tuner = match self.tuner {
            1 => "E4000",
            2 => "FC0012",
            3 => "FC0013",
            4 => "FC2580",
            5 => "R820T",
            6 => "R828D",
            _ => "unknown tuner",
        };
        format!("{} ({tuner})", self.name)
    }

    fn gains(&self) -> Vec<i32> {
        self.gains.clone()
    }
}

impl Drop for RtlSdrSource {
    fn drop(&mut self) {
        self.stop();
        unsafe {
            (self.api.close)(self.dev.0);
        }
    }
}

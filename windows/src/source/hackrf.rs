//! HackRF One through hackrf.dll (libhackrf).

#[allow(unused_imports)]
use super::{convert_i8, hw_freq, load_library, IqBlock, SendPtr, Source};
use crate::settings::{FileFormat, SourceConfig};
use crossbeam_channel::Sender;
use std::ffi::{c_char, c_double, c_int, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

type Dev = *mut c_void;

#[repr(C)]
struct HackrfTransfer {
    device: Dev,
    buffer: *mut u8,
    buffer_length: c_int,
    valid_length: c_int,
    rx_ctx: *mut c_void,
    tx_ctx: *mut c_void,
}

type RxCallback = unsafe extern "C" fn(*mut HackrfTransfer) -> c_int;

struct Api {
    _lib: Option<libloading::Library>,
    open: unsafe extern "C" fn(*mut Dev) -> c_int,
    close: unsafe extern "C" fn(Dev) -> c_int,
    start_rx: unsafe extern "C" fn(Dev, RxCallback, *mut c_void) -> c_int,
    stop_rx: unsafe extern "C" fn(Dev) -> c_int,
    set_freq: unsafe extern "C" fn(Dev, u64) -> c_int,
    set_sample_rate: unsafe extern "C" fn(Dev, c_double) -> c_int,
    set_baseband_filter_bandwidth: unsafe extern "C" fn(Dev, u32) -> c_int,
    compute_baseband_filter_bw: unsafe extern "C" fn(u32) -> u32,
    set_amp_enable: unsafe extern "C" fn(Dev, u8) -> c_int,
    set_lna_gain: unsafe extern "C" fn(Dev, u32) -> c_int,
    set_vga_gain: unsafe extern "C" fn(Dev, u32) -> c_int,
    set_antenna_enable: unsafe extern "C" fn(Dev, u8) -> c_int,
    error_name: unsafe extern "C" fn(c_int) -> *const c_char,
    board_id_read: unsafe extern "C" fn(Dev, *mut u8) -> c_int,
    version_string_read: unsafe extern "C" fn(Dev, *mut c_char, u8) -> c_int,
}

#[cfg(feature = "static-sdr")]
mod linked {
    use super::{HackrfTransfer, RxCallback};
    use std::ffi::{c_char, c_double, c_int, c_void};
    type Dev = *mut c_void;
    unsafe extern "C" {
        pub fn hackrf_init() -> c_int;
        pub fn hackrf_open(d: *mut Dev) -> c_int;
        pub fn hackrf_close(d: Dev) -> c_int;
        pub fn hackrf_start_rx(d: Dev, cb: RxCallback, ctx: *mut c_void) -> c_int;
        pub fn hackrf_stop_rx(d: Dev) -> c_int;
        pub fn hackrf_set_freq(d: Dev, f: u64) -> c_int;
        pub fn hackrf_set_sample_rate(d: Dev, r: c_double) -> c_int;
        pub fn hackrf_set_baseband_filter_bandwidth(d: Dev, bw: u32) -> c_int;
        pub fn hackrf_compute_baseband_filter_bw(bw: u32) -> u32;
        pub fn hackrf_set_amp_enable(d: Dev, v: u8) -> c_int;
        pub fn hackrf_set_lna_gain(d: Dev, v: u32) -> c_int;
        pub fn hackrf_set_vga_gain(d: Dev, v: u32) -> c_int;
        pub fn hackrf_set_antenna_enable(d: Dev, v: u8) -> c_int;
        pub fn hackrf_error_name(e: c_int) -> *const c_char;
        pub fn hackrf_board_id_read(d: Dev, v: *mut u8) -> c_int;
        pub fn hackrf_version_string_read(d: Dev, v: *mut c_char, len: u8) -> c_int;
    }
    #[allow(dead_code)]
    fn _t(_: *mut HackrfTransfer) {}
}

#[cfg(feature = "static-sdr")]
fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        use linked::*;
        let r = unsafe { hackrf_init() };
        if r != 0 {
            return Err(format!("hackrf_init failed ({r})"));
        }
        Ok(Api {
            _lib: None,
            open: hackrf_open,
            close: hackrf_close,
            start_rx: hackrf_start_rx,
            stop_rx: hackrf_stop_rx,
            set_freq: hackrf_set_freq,
            set_sample_rate: hackrf_set_sample_rate,
            set_baseband_filter_bandwidth: hackrf_set_baseband_filter_bandwidth,
            compute_baseband_filter_bw: hackrf_compute_baseband_filter_bw,
            set_amp_enable: hackrf_set_amp_enable,
            set_lna_gain: hackrf_set_lna_gain,
            set_vga_gain: hackrf_set_vga_gain,
            set_antenna_enable: hackrf_set_antenna_enable,
            error_name: hackrf_error_name,
            board_id_read: hackrf_board_id_read,
            version_string_read: hackrf_version_string_read,
        })
    })
    .as_ref()
    .map_err(|e| e.clone())
}

#[cfg(not(feature = "static-sdr"))]
fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        let lib = load_library(&["hackrf.dll", "libhackrf.dll", "libhackrf.so.0", "libhackrf.so"])?;
        unsafe {
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("hackrf.dll: missing {}: {e}", $name))?
                };
            }
            let init: unsafe extern "C" fn() -> c_int = sym!("hackrf_init");
            let r = init();
            if r != 0 {
                return Err(format!("hackrf_init failed ({r})"));
            }
            Ok(Api {
                open: sym!("hackrf_open"),
                close: sym!("hackrf_close"),
                start_rx: sym!("hackrf_start_rx"),
                stop_rx: sym!("hackrf_stop_rx"),
                set_freq: sym!("hackrf_set_freq"),
                set_sample_rate: sym!("hackrf_set_sample_rate"),
                set_baseband_filter_bandwidth: sym!("hackrf_set_baseband_filter_bandwidth"),
                compute_baseband_filter_bw: sym!("hackrf_compute_baseband_filter_bw"),
                set_amp_enable: sym!("hackrf_set_amp_enable"),
                set_lna_gain: sym!("hackrf_set_lna_gain"),
                set_vga_gain: sym!("hackrf_set_vga_gain"),
                set_antenna_enable: sym!("hackrf_set_antenna_enable"),
                error_name: sym!("hackrf_error_name"),
                board_id_read: sym!("hackrf_board_id_read"),
                version_string_read: sym!("hackrf_version_string_read"),
                _lib: Some(lib),
            })
        }
    })
    .as_ref()
    .map_err(|e| e.clone())
}

struct RxCtx {
    tx: Sender<IqBlock>,
    freq: Arc<AtomicU64>,
    rate: Arc<AtomicU32>,
    ok: AtomicBool,
}

unsafe extern "C" fn rx_callback(t: *mut HackrfTransfer) -> c_int {
    unsafe {
        let t = &*t;
        let ctx = &*(t.rx_ctx as *const RxCtx);
        if !ctx.ok.load(Ordering::Relaxed) {
            return -1;
        }
        let raw = std::slice::from_raw_parts(t.buffer, t.valid_length.max(0) as usize).to_vec();
        let mut samples = Vec::new();
        convert_i8(&raw, &mut samples);
        let blk = IqBlock { samples, raw, frequency: ctx.freq.load(Ordering::Relaxed), rate: ctx.rate.load(Ordering::Relaxed) };
        if ctx.tx.send(blk).is_err() {
            ctx.ok.store(false, Ordering::Relaxed);
            return -1;
        }
        0
    }
}

pub struct HackRfSource {
    dev: SendPtr,
    cfg: SourceConfig,
    freq: Arc<AtomicU64>,
    rate: Arc<AtomicU32>,
    ctx: Option<Box<RxCtx>>,
    desc: String,
}

impl HackRfSource {
    pub fn open(cfg: &SourceConfig) -> Result<Self, String> {
        let a = api()?;
        let mut dev: Dev = std::ptr::null_mut();
        unsafe {
            let r = (a.open)(&mut dev);
            if r != 0 || dev.is_null() {
                let name = CStr::from_ptr((a.error_name)(r)).to_string_lossy();
                return Err(format!("Could not open the HackRF: {name}. Is it plugged in and not used by another program?"));
            }
            let mut board = 0u8;
            (a.board_id_read)(dev, &mut board);
            let mut ver = [0 as c_char; 128];
            (a.version_string_read)(dev, ver.as_mut_ptr(), 127);
            let ver = CStr::from_ptr(ver.as_ptr()).to_string_lossy().into_owned();
            let board_name = match board {
                2 => "HackRF One",
                4 => "HackRF One r9",
                1 => "Jawbreaker",
                3 => "rad1o",
                _ => "HackRF",
            };
            let mut s = Self {
                dev: SendPtr(dev),
                cfg: cfg.clone(),
                freq: Arc::new(AtomicU64::new(hw_freq(cfg))),
                rate: Arc::new(AtomicU32::new(cfg.sample_rate)),
                ctx: None,
                desc: format!("{board_name}, firmware {ver}"),
            };
            s.set_rate(cfg.sample_rate);
            (a.set_freq)(dev, hw_freq(cfg));
            (a.set_amp_enable)(dev, cfg.hackrf_amp as u8);
            (a.set_lna_gain)(dev, cfg.hackrf_lna);
            (a.set_vga_gain)(dev, cfg.hackrf_vga);
            (a.set_antenna_enable)(dev, cfg.hackrf_bias_tee as u8);
            Ok(s)
        }
    }

    fn set_rate(&mut self, rate: u32) {
        let a = api().unwrap();
        unsafe {
            (a.set_sample_rate)(self.dev.0, rate as f64);
            let bw = (a.compute_baseband_filter_bw)((rate as f64 * 0.75) as u32);
            (a.set_baseband_filter_bandwidth)(self.dev.0, bw);
        }
        self.rate.store(rate, Ordering::Relaxed);
    }
}

impl Source for HackRfSource {
    fn start(&mut self, tx: Sender<IqBlock>) -> Result<(), String> {
        let a = api()?;
        let ctx = Box::new(RxCtx { tx, freq: self.freq.clone(), rate: self.rate.clone(), ok: AtomicBool::new(true) });
        let ptr = &*ctx as *const RxCtx as *mut c_void;
        self.ctx = Some(ctx);
        let r = unsafe { (a.start_rx)(self.dev.0, rx_callback, ptr) };
        if r != 0 {
            self.ctx = None;
            return Err(format!("hackrf_start_rx failed ({r})"));
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(ctx) = self.ctx.as_ref() {
            ctx.ok.store(false, Ordering::SeqCst);
            if let Ok(a) = api() {
                unsafe {
                    (a.stop_rx)(self.dev.0);
                }
            }
        }
        self.ctx = None;
    }

    fn apply(&mut self, cfg: &SourceConfig) -> Result<(), String> {
        let a = api()?;
        let old = self.cfg.clone();
        let d = self.dev.0;
        if cfg.sample_rate != old.sample_rate {
            self.set_rate(cfg.sample_rate);
        }
        unsafe {
            if hw_freq(cfg) != hw_freq(&old) {
                (a.set_freq)(d, hw_freq(cfg));
                self.freq.store(hw_freq(cfg), Ordering::Relaxed);
            }
            if cfg.hackrf_amp != old.hackrf_amp {
                (a.set_amp_enable)(d, cfg.hackrf_amp as u8);
            }
            if cfg.hackrf_lna != old.hackrf_lna {
                (a.set_lna_gain)(d, cfg.hackrf_lna);
            }
            if cfg.hackrf_vga != old.hackrf_vga {
                (a.set_vga_gain)(d, cfg.hackrf_vga);
            }
            if cfg.hackrf_bias_tee != old.hackrf_bias_tee {
                (a.set_antenna_enable)(d, cfg.hackrf_bias_tee as u8);
            }
        }
        self.cfg = cfg.clone();
        Ok(())
    }

    fn raw_format(&self) -> FileFormat {
        FileFormat::Cs8
    }

    fn description(&self) -> String {
        self.desc.clone()
    }
}

impl Drop for HackRfSource {
    fn drop(&mut self) {
        self.stop();
        if let Ok(a) = api() {
            unsafe {
                (a.close)(self.dev.0);
            }
        }
    }
}

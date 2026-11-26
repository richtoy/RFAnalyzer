//! IQ sources: RTL-SDR and HackRF (through their DLLs), rtl_tcp and IQ files.

pub mod file;
pub mod hackrf;
pub mod rtlsdr;
pub mod rtltcp;

use crate::dsp::C32;
use crate::settings::{FileFormat, SourceConfig, SourceKind};
use crossbeam_channel::Sender;

/// A block of samples as delivered by a source.
pub struct IqBlock {
    pub samples: Vec<C32>,
    /// The same samples in the source's native format, for recording.
    pub raw: Vec<u8>,
    /// Hardware centre frequency the block was received at (Hz, without converter offset)
    pub frequency: u64,
    pub rate: u32,
}

pub trait Source: Send {
    /// Starts streaming into `tx`.
    fn start(&mut self, tx: Sender<IqBlock>) -> Result<(), String>;
    fn stop(&mut self);
    /// Applies changed settings (frequency, sample rate, gains...).
    fn apply(&mut self, cfg: &SourceConfig) -> Result<(), String>;
    fn raw_format(&self) -> FileFormat;
    fn description(&self) -> String;
    /// Gain steps in tenths of dB (RTL-SDR).
    fn gains(&self) -> Vec<i32> {
        Vec::new()
    }
    /// For files: the frequency and rate are given by the file.
    fn fixed_tuning(&self) -> Option<(u64, u32)> {
        None
    }
}

pub fn open(cfg: &SourceConfig) -> Result<Box<dyn Source>, String> {
    match cfg.kind {
        SourceKind::RtlSdr => Ok(Box::new(rtlsdr::RtlSdrSource::open(cfg)?)),
        SourceKind::HackRf => Ok(Box::new(hackrf::HackRfSource::open(cfg)?)),
        SourceKind::RtlTcp => Ok(Box::new(rtltcp::RtlTcpSource::open(cfg)?)),
        SourceKind::File => Ok(Box::new(file::FileSource::open(cfg)?)),
    }
}

/// Hardware frequency for a displayed frequency (subtracts the converter offset).
pub fn hw_freq(cfg: &SourceConfig) -> u64 {
    (cfg.frequency as i64 - cfg.freq_offset).max(0) as u64
}

pub fn convert_u8(raw: &[u8], out: &mut Vec<C32>) {
    out.reserve(raw.len() / 2);
    for p in raw.chunks_exact(2) {
        out.push(C32::new((p[0] as f32 - 127.4) / 128.0, (p[1] as f32 - 127.4) / 128.0));
    }
}

pub fn convert_i8(raw: &[u8], out: &mut Vec<C32>) {
    out.reserve(raw.len() / 2);
    for p in raw.chunks_exact(2) {
        out.push(C32::new(p[0] as i8 as f32 / 128.0, p[1] as i8 as f32 / 128.0));
    }
}

pub fn convert_i16(raw: &[u8], out: &mut Vec<C32>) {
    out.reserve(raw.len() / 4);
    for p in raw.chunks_exact(4) {
        let i = i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.0;
        let q = i16::from_le_bytes([p[2], p[3]]) as f32 / 32768.0;
        out.push(C32::new(i, q));
    }
}

pub fn convert_f32(raw: &[u8], out: &mut Vec<C32>) {
    out.reserve(raw.len() / 8);
    for p in raw.chunks_exact(8) {
        let i = f32::from_le_bytes([p[0], p[1], p[2], p[3]]);
        let q = f32::from_le_bytes([p[4], p[5], p[6], p[7]]);
        out.push(C32::new(i, q));
    }
}

pub fn convert(fmt: FileFormat, raw: &[u8], out: &mut Vec<C32>) {
    match fmt {
        FileFormat::Cu8 => convert_u8(raw, out),
        FileFormat::Cs8 => convert_i8(raw, out),
        FileFormat::Cs16 => convert_i16(raw, out),
        FileFormat::Cf32 => convert_f32(raw, out),
    }
}

/// Loads a DLL from the executable's folder first, then the normal search path.
#[allow(dead_code)]
pub fn load_library(names: &[&str]) -> Result<libloading::Library, String> {
    let mut errors = Vec::new();
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    for name in names {
        if let Some(dir) = &exe_dir {
            let p = dir.join(name);
            if p.exists() {
                match unsafe { libloading::Library::new(&p) } {
                    Ok(l) => return Ok(l),
                    Err(e) => errors.push(format!("{}: {e}", p.display())),
                }
            }
        }
        match unsafe { libloading::Library::new(*name) } {
            Ok(l) => return Ok(l),
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    Err(format!("Could not load {}. Put it next to rfanalyzer.exe. ({})", names[0], errors.join("; ")))
}

/// A raw pointer that may cross threads (the C libraries are thread-safe for our use).
#[derive(Clone, Copy)]
pub struct SendPtr(pub *mut std::ffi::c_void);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

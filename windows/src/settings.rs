//! Persistent settings (saved by eframe between runs).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum Mode {
    Off,
    Am,
    Nfm,
    Wfm,
    Lsb,
    Usb,
    Cw,
    Dab,
}

impl Mode {
    pub const ALL: [Mode; 8] = [Mode::Off, Mode::Am, Mode::Nfm, Mode::Wfm, Mode::Lsb, Mode::Usb, Mode::Cw, Mode::Dab];
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "OFF",
            Mode::Am => "AM",
            Mode::Nfm => "NFM",
            Mode::Wfm => "WFM",
            Mode::Lsb => "LSB",
            Mode::Usb => "USB",
            Mode::Cw => "CW",
            Mode::Dab => "DAB+",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Mode::Off => "Spectrum and waterfall only",
            Mode::Am => "Amplitude modulation: aviation, CB, shortwave broadcast",
            Mode::Nfm => "Narrow FM: PMR, ham repeaters",
            Mode::Wfm => "Wide FM: broadcast radio, stereo and RDS",
            Mode::Lsb => "Lower sideband: HF voice below 10 MHz",
            Mode::Usb => "Upper sideband: HF/VHF voice above 10 MHz",
            Mode::Cw => "Continuous wave: Morse code",
            Mode::Dab => "DAB/DAB+ digital radio (Band III)",
        }
    }
    /// Demodulator input rate.
    pub fn baseband_rate(self) -> u32 {
        match self {
            Mode::Off => 48_000,
            Mode::Am | Mode::Nfm | Mode::Lsb | Mode::Usb => 96_000,
            Mode::Wfm => 192_000,
            Mode::Cw => 48_000,
            Mode::Dab => 2_048_000,
        }
    }
    pub fn width_range(self) -> (u32, u32) {
        match self {
            Mode::Off => (0, 50_000),
            Mode::Am => (3_000, 20_000),
            Mode::Nfm => (5_000, 25_000),
            Mode::Wfm => (30_000, 95_000),
            Mode::Lsb | Mode::Usb => (1_500, 5_000),
            Mode::Cw => (100, 600),
            Mode::Dab => (768_000, 768_000),
        }
    }
    pub fn default_width(self) -> u32 {
        match self {
            Mode::Off => 0,
            Mode::Am => 10_000,
            Mode::Nfm => 12_500,
            Mode::Wfm => 95_000,
            Mode::Lsb | Mode::Usb => 2_800,
            Mode::Cw => 200,
            Mode::Dab => 768_000,
        }
    }
    pub fn tune_steps(self) -> &'static [u32] {
        match self {
            Mode::Off => &[1_000, 10_000, 100_000],
            Mode::Am => &[100, 1_000, 5_000, 9_000, 10_000],
            Mode::Nfm => &[100, 250, 500, 1_000, 6_250, 12_500, 25_000],
            Mode::Wfm => &[10_000, 50_000, 100_000, 200_000],
            Mode::Lsb | Mode::Usb => &[10, 50, 100, 500, 1_000],
            Mode::Cw => &[10, 20, 50, 100, 500],
            Mode::Dab => &[1_000, 8_000, 16_000],
        }
    }
    pub fn default_step(self) -> u32 {
        match self {
            Mode::Off => 10_000,
            Mode::Am => 1_000,
            Mode::Nfm => 12_500,
            Mode::Wfm => 100_000,
            Mode::Lsb | Mode::Usb => 100,
            Mode::Cw => 50,
            Mode::Dab => 16_000,
        }
    }
    /// The channel's extent relative to the channel frequency, for the spectrum overlay.
    pub fn passband(self, width: u32) -> (f64, f64) {
        let w = width as f64;
        match self {
            Mode::Off => (0.0, 0.0),
            Mode::Am | Mode::Nfm | Mode::Wfm | Mode::Dab => (-w, w),
            Mode::Lsb => (-w, 0.0),
            Mode::Usb => (0.0, w),
            Mode::Cw => (-w / 2.0, w / 2.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    RtlSdr,
    HackRf,
    RtlTcp,
    File,
}

impl SourceKind {
    pub const ALL: [SourceKind; 4] = [SourceKind::RtlSdr, SourceKind::HackRf, SourceKind::RtlTcp, SourceKind::File];
    pub fn name(self) -> &'static str {
        match self {
            SourceKind::RtlSdr => "RTL-SDR",
            SourceKind::HackRf => "HackRF",
            SourceKind::RtlTcp => "rtl_tcp (network)",
            SourceKind::File => "IQ file",
        }
    }
    pub fn sample_rates(self) -> &'static [u32] {
        match self {
            SourceKind::RtlSdr | SourceKind::RtlTcp => &[1_000_000, 1_024_000, 1_400_000, 1_800_000, 2_000_000, 2_048_000, 2_400_000, 2_560_000, 2_880_000, 3_200_000],
            SourceKind::HackRf => &[2_000_000, 2_048_000, 4_000_000, 6_000_000, 8_000_000, 10_000_000, 12_500_000, 16_000_000, 20_000_000],
            SourceKind::File => &[],
        }
    }
    pub fn default_rate(self) -> u32 {
        match self {
            SourceKind::HackRf => 8_000_000,
            _ => 2_400_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileFormat {
    Cu8,
    Cs8,
    Cs16,
    Cf32,
}

impl FileFormat {
    pub const ALL: [FileFormat; 4] = [FileFormat::Cu8, FileFormat::Cs8, FileFormat::Cs16, FileFormat::Cf32];
    pub fn name(self) -> &'static str {
        match self {
            FileFormat::Cu8 => "8-bit unsigned (RTL-SDR, .cu8)",
            FileFormat::Cs8 => "8-bit signed (HackRF, .cs8)",
            FileFormat::Cs16 => "16-bit signed (.cs16)",
            FileFormat::Cf32 => "32-bit float (.cf32)",
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            FileFormat::Cu8 => "cu8",
            FileFormat::Cs8 => "cs8",
            FileFormat::Cs16 => "cs16",
            FileFormat::Cf32 => "cf32",
        }
    }
    pub fn bytes_per_sample(self) -> usize {
        match self {
            FileFormat::Cu8 | FileFormat::Cs8 => 2,
            FileFormat::Cs16 => 4,
            FileFormat::Cf32 => 8,
        }
    }
    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "cu8" | "raw" | "bin" => Some(FileFormat::Cu8),
            "cs8" | "iq" => Some(FileFormat::Cs8),
            "cs16" => Some(FileFormat::Cs16),
            "cf32" | "fc32" => Some(FileFormat::Cf32),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceConfig {
    pub kind: SourceKind,
    pub frequency: u64,
    pub sample_rate: u32,
    /// Added to the displayed frequency (up/down converter), Hz
    pub freq_offset: i64,
    pub ppm: i32,
    // RTL-SDR
    pub rtl_device_index: u32,
    pub rtl_agc: bool,
    pub rtl_manual_gain: bool,
    /// tenths of dB
    pub rtl_gain: i32,
    pub rtl_bias_tee: bool,
    /// 0 off, 1 I, 2 Q
    pub rtl_direct_sampling: u32,
    // HackRF
    pub hackrf_amp: bool,
    pub hackrf_lna: u32,
    pub hackrf_vga: u32,
    pub hackrf_bias_tee: bool,
    // rtl_tcp
    pub tcp_host: String,
    pub tcp_port: u16,
    // file
    pub file_path: String,
    pub file_format: FileFormat,
    pub file_rate: u32,
    pub file_frequency: u64,
    pub file_repeat: bool,
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            kind: SourceKind::RtlSdr,
            frequency: 97_000_000,
            sample_rate: 2_400_000,
            freq_offset: 0,
            ppm: 0,
            rtl_device_index: 0,
            rtl_agc: false,
            rtl_manual_gain: true,
            rtl_gain: 340,
            rtl_bias_tee: false,
            rtl_direct_sampling: 0,
            hackrf_amp: true,
            hackrf_lna: 24,
            hackrf_vga: 40,
            hackrf_bias_tee: false,
            tcp_host: "127.0.0.1".into(),
            tcp_port: 1234,
            file_path: String::new(),
            file_format: FileFormat::Cu8,
            file_rate: 2_400_000,
            file_frequency: 100_000_000,
            file_repeat: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMap {
    Gqrx,
    Jet,
    Turbo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    pub fft_size: usize,
    pub averaging: usize,
    pub frame_rate: u32,
    pub db_min: f32,
    pub db_max: f32,
    pub colormap: ColorMap,
    pub peak_hold: bool,
    pub fill_spectrum: bool,
    /// Fraction of the plot area given to the spectrum (rest is waterfall)
    pub split: f32,
    pub ui_scale: f32,
    pub dark: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            fft_size: 8192,
            averaging: 3,
            frame_rate: 30,
            db_min: -110.0,
            db_max: -20.0,
            colormap: ColorMap::Gqrx,
            peak_hold: false,
            fill_spectrum: true,
            split: 0.4,
            ui_scale: 1.25,
            dark: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub frequency: u64,
    pub mode: Mode,
    pub width: u32,
    #[serde(default)]
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub source: SourceConfig,
    pub display: DisplayConfig,
    pub mode: Mode,
    /// Channel frequency (Hz, as displayed, i.e. including the converter offset)
    pub channel_freq: u64,
    pub widths: std::collections::HashMap<String, u32>,
    pub steps: std::collections::HashMap<String, u32>,
    pub volume: f32,
    pub muted: bool,
    pub squelch_enabled: bool,
    pub squelch_db: f32,
    pub wfm_stereo: bool,
    pub deemphasis_us: f32,
    pub rds_enabled: bool,
    pub audio_device: String,
    pub record_dir: String,
    pub record_only_squelch: bool,
    pub record_stop_mb: u32,
    pub record_stop_sec: u32,
    pub bookmarks: Vec<Bookmark>,
    pub dab_channel: String,
    pub dab_last_service: std::collections::HashMap<String, u32>,
    pub dab_ensembles: Vec<crate::dab::model::SavedEnsemble>,
    pub fm_presets: Vec<crate::fmscan::FmPreset>,
    pub fm_scan: crate::fmscan::FmScanConfig,
    pub show_side_panel: bool,
    pub show_info_panel: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source: SourceConfig::default(),
            display: DisplayConfig::default(),
            mode: Mode::Wfm,
            channel_freq: 97_000_000,
            widths: Default::default(),
            steps: Default::default(),
            volume: 0.7,
            muted: false,
            squelch_enabled: false,
            squelch_db: -80.0,
            wfm_stereo: true,
            deemphasis_us: 50.0,
            rds_enabled: true,
            audio_device: String::new(),
            record_dir: String::new(),
            record_only_squelch: false,
            record_stop_mb: 0,
            record_stop_sec: 0,
            bookmarks: Vec::new(),
            dab_channel: "12C".into(),
            dab_last_service: Default::default(),
            dab_ensembles: Vec::new(),
            fm_presets: Vec::new(),
            fm_scan: Default::default(),
            show_side_panel: true,
            show_info_panel: true,
        }
    }
}

impl Settings {
    pub fn width(&self, mode: Mode) -> u32 {
        let (lo, hi) = mode.width_range();
        self.widths.get(mode.name()).copied().unwrap_or(mode.default_width()).clamp(lo, hi)
    }
    pub fn set_width(&mut self, mode: Mode, w: u32) {
        let (lo, hi) = mode.width_range();
        self.widths.insert(mode.name().into(), w.clamp(lo, hi));
    }
    pub fn step(&self, mode: Mode) -> u32 {
        self.steps.get(mode.name()).copied().unwrap_or(mode.default_step())
    }
    pub fn set_step(&mut self, mode: Mode, s: u32) {
        self.steps.insert(mode.name().into(), s);
    }
}

//! RF Analyzer for Windows: a desktop port of Richard's RF Analyzer fork (spectrum/waterfall,
//! analog demodulation, FM stereo + RDS, DAB/DAB+), for RTL-SDR and HackRF.
//! License: GPL-2.0-or-later.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod app;
mod audio;
mod dab;
mod dsp;
mod engine;
mod rds;
mod settings;
mod source;
mod ui;

// libusb (built from source by libusb1-sys) for the built-in RTL-SDR and HackRF drivers
#[cfg(feature = "static-sdr")]
use libusb1_sys as _;

/// %APPDATA%\RFAnalyzer (or ~/.config/rfanalyzer elsewhere), created on demand.
pub fn config_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA").map(std::path::PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
    })?;
    let dir = base.join("RFAnalyzer");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("RF Analyzer")
            .with_inner_size([1600.0, 950.0])
            .with_min_inner_size([900.0, 600.0])
            .with_icon(app::icon()),
        ..Default::default()
    };
    eframe::run_native("RF Analyzer", options, Box::new(|cc| Ok(Box::new(app::RfaApp::new(cc)))))
}

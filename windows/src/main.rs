//! RF Analyzer for Windows: a desktop port of Richard's RF Analyzer fork (spectrum/waterfall,
//! analog demodulation, FM stereo + RDS, DAB/DAB+), for RTL-SDR and HackRF.
//! License: GPL-2.0-or-later.
//!
//! Module map:
//! * `app` – the egui application object [`app::RfaApp`]: owns settings, talks to the engine,
//!   runs the DAB/FM scans, draws via `ui`.
//! * `engine` – the processing thread (source → spectrum, demodulation, audio).
//! * `dsp` – filters, resampling, demodulators, FFT. `rds` – RDS decoder.
//! * `dab` – Rust side of the native DAB+ decoder. `fmscan` – FM band scan and presets.
//! * `source` – RTL-SDR, HackRF, rtl_tcp and IQ file inputs. `audio` – sound output.
//! * `settings` – everything persisted to settings.json.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod app;
mod audio;
mod dab;
mod dsp;
mod engine;
mod fmscan;
mod rds;
mod settings;
mod source;
mod ui;

// libusb (built from source by libusb1-sys) for the built-in RTL-SDR and HackRF drivers
#[cfg(feature = "static-sdr")]
use libusb1_sys as _;

/// Appends a timestamped line to %APPDATA%\\RFAnalyzer\\rfanalyzer.log (the release build has no
/// console, so errors would otherwise be lost). Trimmed to the last ~1 MB at startup.
pub fn log_line(line: &str) {
    use std::io::Write;
    let Some(dir) = config_dir() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("rfanalyzer.log")) {
        let _ = writeln!(f, "{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), line);
    }
}

/// %APPDATA%\RFAnalyzer (or ~/.config/RFAnalyzer elsewhere), created on demand.
pub fn config_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA").map(std::path::PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
    })?;
    let dir = base.join("RFAnalyzer");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Renderer setup: wgpu's primary backends only (DX12 or Vulkan on Windows). By default
/// egui-wgpu also creates an OpenGL instance as a fallback; tearing that down on exit
/// sometimes panicked inside wgpu-hal (gles/wgl.rs, "The operation completed successfully"),
/// so the app exited with code 101 after its settings were saved. WGPU_BACKEND=gl still
/// selects OpenGL on a machine that needs it.
fn gpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = eframe::wgpu::Backends::from_env().unwrap_or(eframe::wgpu::Backends::PRIMARY);
    WgpuConfiguration { wgpu_setup: WgpuSetup::CreateNew(setup), ..Default::default() }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("RF Analyzer")
            .with_inner_size([1600.0, 950.0])
            .with_min_inner_size([900.0, 600.0])
            .with_icon(app::icon()),
        // eframe saved a minimized window as 0x0 at (-32000,-32000) and then panicked on every start;
        // always open at the default size instead
        persist_window: false,
        wgpu_options: gpu_options(),
        ..Default::default()
    };
    eframe::run_native("RF Analyzer", options, Box::new(|cc| Ok(Box::new(app::RfaApp::new(cc)))))
}

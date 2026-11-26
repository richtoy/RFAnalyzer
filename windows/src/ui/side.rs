//! The settings panel on the left: Source, Demodulation, Display, Recording, Bookmarks.

use crate::app::{fmt_hz, RfaApp, SideTab};
use crate::settings::{Bookmark, ColorMap, FileFormat, Mode, SourceKind};
use eframe::egui::{self, RichText};

impl RfaApp {
    pub fn side_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for (t, name) in [
                (SideTab::Source, "Source"),
                (SideTab::Demod, "Demodulation"),
                (SideTab::Display, "Display"),
                (SideTab::Recording, "Recording"),
                (SideTab::Bookmarks, "Bookmarks"),
            ] {
                ui.selectable_value(&mut self.tab, t, RichText::new(name).size(15.0));
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
            SideTab::Source => self.source_tab(ui),
            SideTab::Demod => self.demod_tab(ui),
            SideTab::Display => self.display_tab(ui),
            SideTab::Recording => self.recording_tab(ui),
            SideTab::Bookmarks => self.bookmarks_tab(ui),
        });
    }

    fn source_tab(&mut self, ui: &mut egui::Ui) {
        let running = self.running;
        ui.add_enabled_ui(!running, |ui| {
            egui::ComboBox::from_label("Source")
                .selected_text(self.s.source.kind.name())
                .show_ui(ui, |ui| {
                    for k in SourceKind::ALL {
                        if ui.selectable_value(&mut self.s.source.kind, k, k.name()).clicked() && !k.sample_rates().is_empty() {
                            if !k.sample_rates().contains(&self.s.source.sample_rate) {
                                self.s.source.sample_rate = if self.s.mode == Mode::Dab { 2_048_000 } else { k.default_rate() };
                            }
                        }
                    }
                });
        });
        if running {
            ui.label(RichText::new("Stop the source to change it.").weak());
        }
        ui.add_space(6.0);
        let src = &mut self.s.source;
        match src.kind {
            SourceKind::RtlSdr | SourceKind::RtlTcp => {
                if src.kind == SourceKind::RtlSdr {
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(!running, |ui| {
                            let cur = self.rtl_devices.get(src.rtl_device_index as usize).cloned().unwrap_or_else(|| format!("Device {}", src.rtl_device_index));
                            egui::ComboBox::from_label("Device").selected_text(cur).show_ui(ui, |ui| {
                                for (i, n) in self.rtl_devices.iter().enumerate() {
                                    ui.selectable_value(&mut src.rtl_device_index, i as u32, n);
                                }
                                if self.rtl_devices.is_empty() {
                                    ui.label("No RTL-SDR found");
                                }
                            });
                        });
                        if ui.small_button("⟳").on_hover_text("Search again").clicked() {
                            self.rtl_devices = crate::source::rtlsdr::list_devices();
                        }
                    });
                } else {
                    ui.add_enabled_ui(!running, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Host");
                            ui.text_edit_singleline(&mut src.tcp_host);
                        });
                        ui.horizontal(|ui| {
                            ui.label("Port");
                            ui.add(egui::DragValue::new(&mut src.tcp_port));
                        });
                    });
                }
                rate_combo(ui, src.kind.sample_rates(), &mut src.sample_rate, self.s.mode == Mode::Dab);
                ui.add_space(6.0);
                ui.checkbox(&mut src.rtl_agc, "RTL AGC (digital)");
                ui.checkbox(&mut src.rtl_manual_gain, "Manual tuner gain");
                if src.rtl_manual_gain {
                    let gains = if self.gains.is_empty() {
                        vec![0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372, 386, 402, 421, 434, 439, 445, 480, 496]
                    } else {
                        self.gains.clone()
                    };
                    let mut idx = gains.iter().position(|&g| g >= src.rtl_gain).unwrap_or(gains.len() - 1);
                    let label = format!("Gain {:.1} dB", gains[idx] as f32 / 10.0);
                    let r = ui.add(egui::Slider::new(&mut idx, 0..=gains.len() - 1).show_value(false).text(label));
                    if r.changed() {
                        src.rtl_gain = gains[idx];
                    }
                }
                ui.label(RichText::new("DAB on the RTL-SDR V4: AGC off, manual gain 30-40 dB.").weak().small());
                egui::ComboBox::from_label("Direct sampling")
                    .selected_text(["Off", "I branch", "Q branch (HF on V3/V4)"][src.rtl_direct_sampling.min(2) as usize])
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut src.rtl_direct_sampling, 0, "Off");
                        ui.selectable_value(&mut src.rtl_direct_sampling, 1, "I branch");
                        ui.selectable_value(&mut src.rtl_direct_sampling, 2, "Q branch (HF on V3/V4)");
                    });
                ui.checkbox(&mut src.rtl_bias_tee, "Bias tee (powers an LNA on the antenna input)");
            }
            SourceKind::HackRf => {
                rate_combo(ui, src.kind.sample_rates(), &mut src.sample_rate, self.s.mode == Mode::Dab);
                ui.add_space(6.0);
                ui.checkbox(&mut src.hackrf_amp, "RF amplifier (+14 dB)");
                let mut lna = src.hackrf_lna / 8;
                let label = format!("LNA {} dB", lna * 8);
                if ui.add(egui::Slider::new(&mut lna, 0..=5).show_value(false).text(label)).changed() {
                    src.hackrf_lna = lna * 8;
                }
                let mut vga = src.hackrf_vga / 2;
                let label = format!("VGA {} dB", vga * 2);
                if ui.add(egui::Slider::new(&mut vga, 0..=31).show_value(false).text(label)).changed() {
                    src.hackrf_vga = vga * 2;
                }
                ui.label(RichText::new("DAB on the HackRF: amp on, LNA 24 dB, VGA 40 dB.").weak().small());
                ui.checkbox(&mut src.hackrf_bias_tee, "Antenna port power (bias tee)");
            }
            SourceKind::File => {
                ui.add_enabled_ui(!running, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Choose file…").clicked() {
                            if let Some(p) = rfd::FileDialog::new()
                                .add_filter("IQ recordings", &["cu8", "cs8", "cs16", "cf32", "fc32", "raw", "bin", "iq"])
                                .add_filter("All files", &["*"])
                                .pick_file()
                            {
                                src.file_path = p.to_string_lossy().into_owned();
                                let (f, r, fmt) = crate::source::file::parse_file_name(&src.file_path);
                                if let Some(f) = f {
                                    src.file_frequency = f;
                                }
                                if let Some(r) = r {
                                    src.file_rate = r;
                                }
                                if let Some(fmt) = fmt {
                                    src.file_format = fmt;
                                }
                            }
                        }
                    });
                    let name = std::path::Path::new(&src.file_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "(none)".into());
                    ui.label(RichText::new(name).monospace());
                    egui::ComboBox::from_label("Format").selected_text(src.file_format.name()).show_ui(ui, |ui| {
                        for f in FileFormat::ALL {
                            ui.selectable_value(&mut src.file_format, f, f.name());
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Sample rate");
                        ui.add(egui::DragValue::new(&mut src.file_rate).speed(1000).suffix(" sps"));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Centre frequency");
                        ui.add(egui::DragValue::new(&mut src.file_frequency).speed(1000).suffix(" Hz"));
                    });
                    ui.checkbox(&mut src.file_repeat, "Loop");
                });
            }
        }
        ui.add_space(8.0);
        ui.separator();
        if src.kind != SourceKind::File {
            ui.horizontal(|ui| {
                ui.label("Frequency correction");
                ui.add(egui::DragValue::new(&mut src.ppm).range(-200..=200).suffix(" ppm"));
            });
            ui.horizontal(|ui| {
                ui.label("Converter offset");
                ui.add(egui::DragValue::new(&mut src.freq_offset).speed(1000).suffix(" Hz"));
            })
            .response
            .on_hover_text("For an up/down converter: the displayed frequency is the hardware frequency plus this offset.");
            ui.horizontal(|ui| {
                ui.label("Source centre");
                let mut mhz = src.frequency as f64 / 1e6;
                if ui.add(egui::DragValue::new(&mut mhz).speed(0.01).range(0.0..=7000.0).suffix(" MHz").max_decimals(6)).changed() && self.s.mode != Mode::Dab {
                    src.frequency = (mhz * 1e6).round() as u64;
                }
            });
        }
        ui.add_space(8.0);
        ui.collapsing("Drivers (first-time setup)", |ui| {
            ui.label("RTL-SDR: install the WinUSB driver once with Zadig (Options > List all devices, pick 'Bulk-In, Interface (Interface 0)', WinUSB, Replace driver). rtlsdr.dll comes with this app.");
            ui.label("HackRF: recent firmware installs its WinUSB driver by itself. If Windows shows it without a driver, use Zadig the same way. hackrf.dll comes with this app.");
            ui.label("rtl_tcp: run rtl_tcp on another PC or phone and enter its address.");
        });
    }

    fn demod_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Mode").strong());
        for m in Mode::ALL {
            let sel = self.s.mode == m;
            if ui.selectable_label(sel, format!("{}  ·  {}", m.name(), m.description())).clicked() {
                self.set_mode(m);
            }
        }
        ui.separator();
        let mode = self.s.mode;
        if mode != Mode::Off && mode != Mode::Dab {
            let (lo, hi) = mode.width_range();
            let mut w = self.s.width(mode);
            let label = match mode {
                Mode::Lsb | Mode::Usb => "Audio bandwidth",
                Mode::Cw => "Filter width",
                _ => "Channel width (one side)",
            };
            if ui.add(egui::Slider::new(&mut w, lo..=hi).text(label).suffix(" Hz")).changed() {
                self.s.set_width(mode, w);
            }
            let mut step = self.s.step(mode);
            egui::ComboBox::from_label("Tune step").selected_text(fmt_hz(step as f64)).show_ui(ui, |ui| {
                for &st in mode.tune_steps() {
                    ui.selectable_value(&mut step, st, fmt_hz(st as f64));
                }
            });
            self.s.set_step(mode, step);
            ui.add_space(6.0);
            ui.checkbox(&mut self.s.squelch_enabled, "Squelch");
            ui.add_enabled(self.s.squelch_enabled, egui::Slider::new(&mut self.s.squelch_db, -130.0..=0.0).suffix(" dB").text("threshold"));
            ui.label(RichText::new(format!("Channel level {:.1} dB {}", self.channel_db, if self.squelch_open { "" } else { "(squelched)" })).weak());
        }
        if mode == Mode::Wfm {
            ui.separator();
            ui.label(RichText::new("Stereo & RDS").strong());
            ui.checkbox(&mut self.s.wfm_stereo, "Stereo").on_hover_text("Off keeps weak stations in mono while RDS still decodes.");
            ui.horizontal(|ui| {
                ui.label("De-emphasis");
                for (l, us) in [("50 µs (Europe)", 50.0), ("75 µs (Americas)", 75.0), ("Off", 0.0)] {
                    ui.selectable_value(&mut self.s.deemphasis_us, us, l);
                }
            });
            ui.checkbox(&mut self.s.rds_enabled, "Decode RDS");
            ui.horizontal(|ui| {
                ui.label("PTY names");
                ui.selectable_value(&mut self.pty_rbds, false, "RDS (Europe)");
                ui.selectable_value(&mut self.pty_rbds, true, "RBDS (N. America)");
            });
        }
        if mode == Mode::Dab {
            ui.label("Use the DAB+ panel on the right to pick a channel, scan, and choose a station.");
            ui.label(RichText::new("The source is centred on the channel at 2.048 Msps while DAB+ is on.").weak());
        }
        ui.separator();
        ui.label(RichText::new("Audio").strong());
        ui.add(egui::Slider::new(&mut self.s.volume, 0.0..=1.5).text("Volume"));
        ui.checkbox(&mut self.s.muted, "Mute");
        let cur = if self.s.audio_device.is_empty() { "System default".to_string() } else { self.s.audio_device.clone() };
        let mut chosen = None;
        egui::ComboBox::from_label("Output").selected_text(cur).show_ui(ui, |ui| {
            if ui.selectable_label(self.s.audio_device.is_empty(), "System default").clicked() {
                chosen = Some(String::new());
            }
            for d in &self.audio_devices {
                if ui.selectable_label(&self.s.audio_device == d, d).clicked() {
                    chosen = Some(d.clone());
                }
            }
        });
        if let Some(d) = chosen {
            self.s.audio_device = d;
            self.audio_out = None;
            match crate::audio::AudioOut::open(self.audio.clone(), &self.s.audio_device) {
                Ok(a) => {
                    self.audio_out = Some(a);
                    self.audio_error = None;
                }
                Err(e) => self.audio_error = Some(e),
            }
        }
    }

    fn display_tab(&mut self, ui: &mut egui::Ui) {
        let d = &mut self.s.display;
        egui::ComboBox::from_label("FFT size").selected_text(d.fft_size.to_string()).show_ui(ui, |ui| {
            for n in [1024usize, 2048, 4096, 8192, 16384, 32768, 65536] {
                ui.selectable_value(&mut d.fft_size, n, n.to_string());
            }
        });
        ui.add(egui::Slider::new(&mut d.averaging, 1..=20).text("Averaging"));
        ui.add(egui::Slider::new(&mut d.frame_rate, 5..=60).text("Frames per second"));
        ui.add(egui::Slider::new(&mut d.db_max, -80.0..=30.0).text("Top (dB)"));
        ui.add(egui::Slider::new(&mut d.db_min, -160.0..=-20.0).text("Bottom (dB)"));
        if d.db_min > d.db_max - 5.0 {
            d.db_min = d.db_max - 5.0;
        }
        ui.horizontal(|ui| {
            if ui.button("Autoscale").clicked() {
                self.autoscale();
            }
            if ui.button("Reset scale").clicked() {
                self.s.display.db_min = -110.0;
                self.s.display.db_max = -20.0;
            }
            if ui.button("Reset zoom").clicked() {
                self.view.reset_zoom();
            }
        });
        let d = &mut self.s.display;
        egui::ComboBox::from_label("Waterfall colours")
            .selected_text(match d.colormap {
                ColorMap::Gqrx => "GQRX",
                ColorMap::Jet => "JET",
                ColorMap::Turbo => "Turbo",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.colormap, ColorMap::Gqrx, "GQRX");
                ui.selectable_value(&mut d.colormap, ColorMap::Jet, "JET");
                ui.selectable_value(&mut d.colormap, ColorMap::Turbo, "Turbo");
            });
        ui.checkbox(&mut d.fill_spectrum, "Filled spectrum");
        ui.checkbox(&mut d.peak_hold, "Peak hold");
        ui.add(egui::Slider::new(&mut d.split, 0.15..=0.85).text("Spectrum / waterfall split"));
        ui.separator();
        ui.add(egui::Slider::new(&mut d.ui_scale, 0.8..=2.5).text("UI scale"));
        ui.checkbox(&mut d.dark, "Dark theme");
    }

    fn recording_tab(&mut self, ui: &mut egui::Ui) {
        ui.label("Records the raw IQ from the source (cu8 for RTL-SDR, cs8 for HackRF). Play it back with the IQ file source.");
        ui.horizontal(|ui| {
            let dir = if self.s.record_dir.is_empty() { crate::app::dirs_home().join("RFAnalyzer recordings").to_string_lossy().into_owned() } else { self.s.record_dir.clone() };
            ui.label(RichText::new(dir).monospace().small());
        });
        if ui.button("Choose folder…").clicked() {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                self.s.record_dir = p.to_string_lossy().into_owned();
            }
        }
        ui.checkbox(&mut self.s.record_only_squelch, "Only while the squelch is open");
        ui.horizontal(|ui| {
            ui.label("Stop after");
            ui.add(egui::DragValue::new(&mut self.s.record_stop_mb).suffix(" MB"));
            ui.label("or");
            ui.add(egui::DragValue::new(&mut self.s.record_stop_sec).suffix(" s"));
        });
        ui.label(RichText::new("0 = no limit").weak().small());
        ui.add_space(6.0);
        if let Some(r) = self.recording.clone() {
            ui.label(format!("Recording: {:.1} MB, {:.0} s", r.bytes as f64 / 1e6, r.seconds));
            ui.label(RichText::new(&r.path).monospace().small());
            if ui.button("Stop recording").clicked() {
                self.stop_recording();
            }
        } else if ui.button("Start recording").clicked() {
            self.start_recording();
        }
        if let Some(r) = self.last_recording.clone() {
            ui.separator();
            ui.label(format!("Last recording: {:.1} MB, {:.0} s", r.bytes as f64 / 1e6, r.seconds));
            ui.label(RichText::new(&r.path).monospace().small());
            if ui.button("Play it back (IQ file source)").clicked() {
                self.stop();
                self.s.source.kind = SourceKind::File;
                self.s.source.file_path = r.path.clone();
                self.start();
            }
        }
    }

    fn bookmarks_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.bookmark_name).hint_text("Name").desired_width(160.0));
            if ui.button("➕ Add current").clicked() {
                let name = if self.bookmark_name.trim().is_empty() {
                    self.rds.as_ref().map(|r| r.ps.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| fmt_hz(self.channel_freq() as f64))
                } else {
                    self.bookmark_name.trim().to_string()
                };
                let mode = self.s.mode;
                self.s.bookmarks.push(Bookmark { name, frequency: self.channel_freq(), mode, width: self.s.width(mode), notes: String::new() });
                self.bookmark_name.clear();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Export…").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).set_file_name("rfanalyzer-bookmarks.json").save_file() {
                    if let Ok(s) = serde_json::to_string_pretty(&self.s.bookmarks) {
                        let _ = std::fs::write(p, s);
                    }
                }
            }
            if ui.button("Import…").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                    if let Ok(text) = std::fs::read_to_string(p) {
                        if let Ok(list) = serde_json::from_str::<Vec<Bookmark>>(&text) {
                            for b in list {
                                if !self.s.bookmarks.contains(&b) {
                                    self.s.bookmarks.push(b);
                                }
                            }
                        }
                    }
                }
            }
        });
        ui.separator();
        let mut tune = None;
        let mut delete = None;
        egui::Grid::new("bm").striped(true).num_columns(4).show(ui, |ui| {
            for (i, b) in self.s.bookmarks.iter_mut().enumerate() {
                if ui.button("▶").on_hover_text("Tune").clicked() {
                    tune = Some(i);
                }
                ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(120.0));
                ui.label(format!("{}  {}", fmt_hz(b.frequency as f64), b.mode.name()));
                if ui.small_button("🗑").clicked() {
                    delete = Some(i);
                }
                ui.end_row();
            }
        });
        if self.s.bookmarks.is_empty() {
            ui.label(RichText::new("No bookmarks yet.").weak());
        }
        if let Some(i) = tune {
            let b = self.s.bookmarks[i].clone();
            self.set_mode(b.mode);
            if b.mode == Mode::Dab {
                if let Some(c) = crate::dab::model::channel_for_freq(b.frequency) {
                    self.set_dab_channel(c.name.to_string());
                }
            } else {
                self.s.set_width(b.mode, b.width);
                self.set_channel(b.frequency);
            }
        }
        if let Some(i) = delete {
            self.s.bookmarks.remove(i);
        }
    }
}

fn rate_combo(ui: &mut egui::Ui, rates: &[u32], rate: &mut u32, dab: bool) {
    ui.add_enabled_ui(!dab, |ui| {
        egui::ComboBox::from_label("Sample rate").selected_text(format!("{:.3} Msps", *rate as f64 / 1e6)).show_ui(ui, |ui| {
            for &r in rates {
                ui.selectable_value(rate, r, format!("{:.3} Msps", r as f64 / 1e6));
            }
        });
    });
}

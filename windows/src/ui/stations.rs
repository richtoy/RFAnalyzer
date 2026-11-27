//! The FM stations window: band scan and the presets list with RDS attributes.

use crate::app::RfaApp;
use crate::rds::PTY_RDS;
use eframe::egui::{self, Color32, RichText};

const GOOD: Color32 = Color32::from_rgb(46, 157, 87);
const DIM: Color32 = Color32::from_rgb(110, 110, 110);

fn flag(ui: &mut egui::Ui, on: bool, text: &str) {
    ui.label(RichText::new(text).color(if on { GOOD } else { DIM }));
}

impl RfaApp {
    /// Compact preset picker for the RDS panel.
    pub fn preset_strip(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let cur = self.s.fm_presets.iter().find(|p| p.freq.abs_diff(self.s.channel_freq) < 50_000).map(|p| p.label());
            let mut pick = None;
            egui::ComboBox::from_id_salt("fm_presets")
                .width(200.0)
                .selected_text(cur.unwrap_or_else(|| if self.s.fm_presets.is_empty() { "No presets yet".into() } else { "Presets".into() }))
                .show_ui(ui, |ui| {
                    for p in &self.s.fm_presets {
                        let t = format!("{:>6.1}  {}{}", p.freq as f64 / 1e6, p.label(), if p.rds { "" } else { "  (no RDS)" });
                        if ui.selectable_label(p.freq.abs_diff(self.s.channel_freq) < 50_000, RichText::new(t).monospace()).clicked() {
                            pick = Some(p.freq);
                        }
                    }
                });
            if let Some(f) = pick {
                self.tune_preset(f);
            }
            if ui.button("◀").on_hover_text("Previous preset").clicked() {
                self.step_preset(-1);
            }
            if ui.button("▶").on_hover_text("Next preset").clicked() {
                self.step_preset(1);
            }
            if ui.button("Stations…").on_hover_text("Scan the FM band and manage presets").clicked() {
                self.show_fm_window = true;
            }
        });
        if self.fm_scan_status.running {
            ui.add(egui::ProgressBar::new(self.fm_scan_status.progress).text(self.fm_scan_status.stage.clone()));
        }
    }

    pub fn step_preset(&mut self, dir: i32) {
        if self.s.fm_presets.is_empty() {
            return;
        }
        let ch = self.s.channel_freq;
        let list = &self.s.fm_presets;
        let next = if dir > 0 {
            list.iter().find(|p| p.freq > ch + 25_000).or(list.first())
        } else {
            list.iter().rev().find(|p| p.freq + 25_000 < ch).or(list.last())
        };
        if let Some(p) = next {
            let f = p.freq;
            self.tune_preset(f);
        }
    }

    pub fn fm_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_fm_window;
        egui::Window::new("FM stations").open(&mut open).default_size([1100.0, 640.0]).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if self.fm_scan.is_some() {
                    if ui.add(egui::Button::new(RichText::new("■ Stop scan").strong())).clicked() {
                        self.stop_fm_scan(false);
                    }
                } else if ui.add(egui::Button::new(RichText::new("⟳ Scan FM band").strong())).clicked() {
                    self.start_fm_scan();
                }
                ui.separator();
                let c = &mut self.s.fm_scan;
                let mut a = c.start_hz as f64 / 1e6;
                let mut b = c.end_hz as f64 / 1e6;
                ui.label("From");
                ui.add(egui::DragValue::new(&mut a).range(64.0..=110.0).speed(0.1).suffix(" MHz").fixed_decimals(1));
                ui.label("to");
                ui.add(egui::DragValue::new(&mut b).range(64.0..=110.0).speed(0.1).suffix(" MHz").fixed_decimals(1));
                c.start_hz = (a * 1e6).round() as u64;
                c.end_hz = (b * 1e6).round().max(a * 1e6) as u64;
                ui.label("step");
                egui::ComboBox::from_id_salt("fm_step").width(80.0).selected_text(format!("{} kHz", c.step_hz / 1000)).show_ui(ui, |ui| {
                    for st in [50_000u64, 100_000, 200_000] {
                        ui.selectable_value(&mut c.step_hz, st, format!("{} kHz", st / 1000));
                    }
                });
                ui.label("threshold");
                ui.add(egui::DragValue::new(&mut c.threshold_db).range(3.0..=30.0).speed(0.5).suffix(" dB"));
                ui.label("max per station");
                ui.add(egui::DragValue::new(&mut c.max_secs).range(4.0..=30.0).speed(0.5).suffix(" s"));
            });
            if self.fm_scan_status.running {
                ui.add(egui::ProgressBar::new(self.fm_scan_status.progress).text(self.fm_scan_status.stage.clone()));
            }
            if !self.fm_scan_status.log.is_empty() {
                egui::CollapsingHeader::new(format!("Scan log ({} lines)", self.fm_scan_status.log.len())).id_salt("fm_log").show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(140.0).stick_to_bottom(true).show(ui, |ui| {
                        for l in &self.fm_scan_status.log {
                            ui.label(RichText::new(l).monospace().small());
                        }
                    });
                });
            }
            ui.separator();
            let n = self.s.fm_presets.len();
            let n_rds = self.s.fm_presets.iter().filter(|p| p.rds).count();
            let n_st = self.s.fm_presets.iter().filter(|p| p.stereo).count();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{n} stations · {n_st} stereo · {n_rds} with RDS")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Clear").clicked() {
                        self.s.fm_presets.clear();
                    }
                    if ui.button("Import JSON…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                            match std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Vec<crate::fmscan::FmPreset>>(&t).ok()) {
                                Some(v) => self.s.fm_presets = v,
                                None => self.toast("That file is not an FM presets export."),
                            }
                        }
                    }
                    if ui.button("Export JSON…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).set_file_name("fm-presets.json").save_file() {
                            let _ = std::fs::write(p, serde_json::to_string_pretty(&self.s.fm_presets).unwrap_or_default());
                        }
                    }
                    if ui.button("Export CSV…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("fm-presets.csv").save_file() {
                            let _ = std::fs::write(p, crate::fmscan::presets_csv(&self.s.fm_presets));
                        }
                    }
                });
            });
            ui.add_space(4.0);
            let mut tune = None;
            let mut delete = None;
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                egui::Grid::new("fm_presets_grid").striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                    for h in ["", "MHz", "PS", "Your name", "PI", "Programme type", "Stereo", "RDS", "TP", "TA", "Country", "Level", "SNR", "BLER", "AF", "Extras", "RadioText", ""] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for (i, p) in self.s.fm_presets.iter_mut().enumerate() {
                        let on = p.freq.abs_diff(self.s.channel_freq) < 50_000;
                        if ui.selectable_label(on, "▶").on_hover_text("Tune").clicked() {
                            tune = Some(p.freq);
                        }
                        ui.label(RichText::new(format!("{:.1}", p.freq as f64 / 1e6)).monospace().strong());
                        ui.label(RichText::new(if p.ps.is_empty() { "—".to_string() } else { p.ps.clone() }).monospace().strong());
                        ui.add(egui::TextEdit::singleline(&mut p.name).hint_text("optional").desired_width(130.0));
                        ui.label(RichText::new(p.pi.map(|x| format!("{x:04X}")).unwrap_or_default()).monospace());
                        ui.label(p.pty.map(|x| PTY_RDS.get(x as usize).copied().unwrap_or("").to_string()).unwrap_or_default());
                        flag(ui, p.stereo, if p.stereo { "stereo" } else { "mono" });
                        flag(ui, p.rds, if p.rds { "RDS" } else { "—" });
                        flag(ui, p.tp, "TP");
                        flag(ui, p.ta, "TA");
                        ui.label(p.country.clone().unwrap_or_else(|| p.ecc.map(|e| format!("ECC {e:02X}")).unwrap_or_default()));
                        ui.label(format!("{:.0} dB", p.level_db));
                        ui.label(format!("{:.0} dB", p.snr_db));
                        ui.label(p.bler.map(|b| format!("{:.0}%", b * 100.0)).unwrap_or_default());
                        let af = if p.af.is_empty() { String::new() } else { format!("{}", p.af.len()) };
                        ui.label(af).on_hover_text(p.af.iter().map(|f| format!("{f:.1}")).collect::<Vec<_>>().join(", "));
                        let mut ex = Vec::new();
                        if let Some(f) = p.same_pi_as {
                            ex.push(format!("= {:.1}", f as f64 / 1e6));
                        }
                        if p.rtplus {
                            ex.push("RT+".to_string());
                        }
                        if p.eon > 0 {
                            ex.push(format!("EON {}", p.eon));
                        }
                        if p.tmc {
                            ex.push("TMC".into());
                        }
                        if p.clock {
                            ex.push("CT".into());
                        }
                        if p.ptyn.is_some() {
                            ex.push("PTYN".into());
                        }
                        if p.long_ps.is_some() {
                            ex.push("LPS".into());
                        }
                        ui.label(ex.join(" ")).on_hover_text(if p.oda.is_empty() { String::new() } else { format!("ODA: {}", p.oda.join(", ")) });
                        let rt: String = p.rt.chars().take(48).collect();
                        ui.label(RichText::new(rt).weak()).on_hover_text(&p.rt);
                        if ui.small_button("🗑").on_hover_text("Remove").clicked() {
                            delete = Some(i);
                        }
                        ui.end_row();
                    }
                });
                if self.s.fm_presets.is_empty() && !self.fm_scan_status.running {
                    ui.label(RichText::new("No stations yet. Press \"Scan FM band\": it surveys the band, then checks each carrier for stereo and RDS (a few minutes).").weak());
                }
            });
            if let Some(i) = delete {
                self.s.fm_presets.remove(i);
            }
            if let Some(f) = tune {
                self.tune_preset(f);
            }
        });
        self.show_fm_window = open;
    }
}

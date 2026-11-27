//! The info panel on the right (RDS for FM, the DAB+ receiver) and the detail windows.

use crate::app::RfaApp;
use crate::dab::model::{self, language_name, pty_name, DabState};
use crate::rds::{PTY_RBDS, PTY_RDS};
use crate::settings::Mode;
use eframe::egui::{self, pos2, vec2, Color32, RichText, Sense, Stroke};

const GOOD: Color32 = Color32::from_rgb(46, 157, 87);
const WARN: Color32 = Color32::from_rgb(208, 138, 0);
const BAD: Color32 = Color32::from_rgb(192, 57, 43);

fn quality_color(p: i32) -> Color32 {
    if p >= 90 {
        GOOD
    } else if p >= 60 {
        WARN
    } else {
        BAD
    }
}

fn kv(ui: &mut egui::Ui, k: &str, v: impl Into<String>) {
    ui.label(RichText::new(k).weak());
    ui.label(v.into());
    ui.end_row();
}

impl RfaApp {
    fn pty_text(&self, code: u8) -> String {
        let t = if self.pty_rbds { PTY_RBDS } else { PTY_RDS };
        t.get(code as usize).map(|s| s.to_string()).unwrap_or_default()
    }

    pub fn info_panel(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.s.mode {
            Mode::Wfm => self.rds_panel(ui),
            Mode::Dab => self.dab_panel(ui),
            _ => {}
        });
    }

    // ------------------------------------------------------------------ FM / RDS

    fn rds_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        self.preset_strip(ui);
        ui.add_space(4.0);
        let r = self.rds.clone();
        let ps = r.as_ref().map(|r| if r.ps.trim().is_empty() { r.ps_partial.clone() } else { r.ps.clone() }).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(RichText::new(if ps.trim().is_empty() { "--------".to_string() } else { ps.clone() }).size(34.0).monospace().strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (txt, col) = if self.fm.stereo { ("STEREO", GOOD) } else { ("MONO", Color32::GRAY) };
                ui.label(RichText::new(txt).strong().color(col));
            });
        });
        if let Some(r) = &r {
            ui.horizontal_wrapped(|ui| {
                if let Some(pi) = r.pi {
                    ui.label(RichText::new(format!("PI {pi:04X}")).monospace());
                }
                if let Some(p) = r.pty {
                    ui.label(RichText::new(self.pty_text(p)).italics());
                }
                for (on, name) in [(r.tp, "TP"), (r.ta, "TA"), (r.music, "MUSIC")] {
                    ui.label(RichText::new(name).small().color(if on { GOOD } else { Color32::DARK_GRAY }));
                }
            });
            if !r.rt.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(&r.rt).size(17.0));
            }
            if r.rtplus_title.is_some() || r.rtplus_artist.is_some() {
                ui.label(RichText::new(format!("♪ {} — {}", r.rtplus_artist.clone().unwrap_or_default(), r.rtplus_title.clone().unwrap_or_default())).weak());
            }
            if let Some(c) = &r.clock {
                ui.label(RichText::new(c).weak().small());
            }
        } else if self.s.rds_enabled {
            ui.label(RichText::new("Waiting for RDS…").weak());
        } else {
            ui.label(RichText::new("RDS decoding is off (Demodulation tab).").weak());
        }
        ui.separator();
        // signal & decoder panel
        let st = self.rds_stats.clone();
        egui::Grid::new("rds_sig").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            let bler_col = if !st.synced { BAD } else if st.bler < 0.05 { GOOD } else if st.bler < 0.2 { WARN } else { BAD };
            ui.label(RichText::new("RDS").weak());
            ui.label(RichText::new(if st.synced { format!("synced, BLER {:.1}%", st.bler * 100.0) } else { "no sync".into() }).color(bler_col));
            ui.end_row();
            kv(ui, "Groups", format!("{:.1}/s  ({} total)", st.groups_per_sec, st.groups_total));
            kv(ui, "Blocks", format!("{} ok · {} corrected · {} failed", st.blocks_ok, st.blocks_corrected, st.blocks_failed));
            kv(ui, "Pilot", format!("{:.1} dB {}", self.fm.pilot_db, if self.fm.pilot_locked { "(locked)" } else { "" }));
            kv(ui, "RDS injection", format!("{:.1} kHz", self.fm.rds_level * 75.0));
            kv(ui, "Peak deviation", format!("{:.1} kHz", self.fm.deviation_khz));
        });
        sparkline(ui, &st.bler_history, 0.0, 1.0, "BLER, last 60 s", true);
        ui.add_space(4.0);
        if ui.button("RDS details…").clicked() {
            self.show_rds_window = true;
        }
    }

    fn rds_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_rds_window;
        egui::Window::new("RDS details").open(&mut open).default_size([640.0, 720.0]).vscroll(true).show(ctx, |ui| {
            let Some(r) = self.rds.clone() else {
                ui.label("No RDS station yet.");
                return;
            };
            egui::Grid::new("rds_det").num_columns(2).spacing([16.0, 4.0]).striped(true).show(ui, |ui| {
                kv(ui, "PI", r.pi.map(|p| format!("{p:04X}")).unwrap_or_default());
                kv(ui, "PS", r.ps.clone());
                if let Some(l) = &r.long_ps {
                    kv(ui, "Long PS", l.clone());
                }
                kv(ui, "PTY", r.pty.map(|p| format!("{} ({p})", self.pty_text(p))).unwrap_or_default());
                if let Some(p) = &r.ptyn {
                    kv(ui, "PTYN", p.clone());
                }
                kv(ui, "ECC", r.ecc.map(|e| format!("{e:02X}{}", r.country_hint().map(|c| format!(" ({c})")).unwrap_or_default())).unwrap_or("-".into()));
                kv(ui, "Language", r.language.map(|l| format!("{l:02X}")).unwrap_or("-".into()));
                kv(ui, "PIN", r.pin.clone().unwrap_or("-".into()));
                kv(ui, "DI", r.di.map(|d| format!("{}{}{}{}", if d & 1 != 0 { "stereo " } else { "mono " }, if d & 2 != 0 { "artificial-head " } else { "" }, if d & 4 != 0 { "compressed " } else { "" }, if d & 8 != 0 { "dynamic PTY" } else { "static PTY" })).unwrap_or("-".into()));
                kv(ui, "Clock", r.clock.clone().unwrap_or("-".into()));
                kv(ui, "RT+ title", r.rtplus_title.clone().unwrap_or("-".into()));
                kv(ui, "RT+ artist", r.rtplus_artist.clone().unwrap_or("-".into()));
                kv(ui, "TMC", r.tmc_ltn.map(|l| format!("location table {l}, {} messages", r.tmc.len())).unwrap_or("-".into()));
            });
            ui.separator();
            ui.label(RichText::new(format!("Alternative frequencies ({}{})", r.af.len(), if r.af_complete { ", complete" } else { "" })).strong());
            ui.label(r.af.iter().map(|f| format!("{f:.1}")).collect::<Vec<_>>().join("  "));
            ui.separator();
            ui.label(RichText::new("RadioText history").strong());
            for t in &r.rt_history {
                ui.label(t);
            }
            if !r.eon.is_empty() {
                ui.separator();
                ui.label(RichText::new("Other networks (EON)").strong());
                egui::Grid::new("eon").striped(true).num_columns(4).show(ui, |ui| {
                    for e in r.eon.values() {
                        ui.monospace(format!("{:04X}", e.pi));
                        ui.label(&e.ps);
                        ui.label(e.af.iter().map(|f| format!("{f:.1}")).collect::<Vec<_>>().join(" "));
                        ui.label(format!("{}{}", if e.tp == Some(true) { "TP " } else { "" }, if e.ta == Some(true) { "TA" } else { "" }));
                        ui.end_row();
                    }
                });
            }
            if !r.oda.is_empty() {
                ui.separator();
                ui.label(RichText::new("Open data applications").strong());
                for k in r.oda.keys() {
                    ui.label(k);
                }
            }
            if !r.tmc.is_empty() {
                ui.separator();
                ui.label(RichText::new("TMC messages (event / location codes)").strong());
                egui::Grid::new("tmc").striped(true).num_columns(4).show(ui, |ui| {
                    for m in r.tmc.iter().take(50) {
                        ui.monospace(format!("event {}", m.event));
                        ui.monospace(format!("loc {}", m.location));
                        ui.label(format!("extent {} {}", m.extent, if m.direction { "−" } else { "+" }));
                        ui.label(format!("×{}", m.seen));
                        ui.end_row();
                    }
                });
            }
            ui.separator();
            ui.label(RichText::new("Group types").strong());
            let total: u64 = r.group_counts.values().sum();
            for (g, n) in &r.group_counts {
                ui.horizontal(|ui| {
                    ui.monospace(format!("{g:>3}"));
                    let frac = *n as f32 / total.max(1) as f32;
                    ui.add(egui::ProgressBar::new(frac).desired_width(260.0).text(format!("{n}  ({:.1}%)", frac * 100.0)));
                });
            }
            ui.separator();
            ui.label(RichText::new("MPX spectrum (0-96 kHz)").strong());
            mpx_view(ui, &self.mpx);
            ui.label(RichText::new("RDS symbols (BPSK decision values)").strong());
            symbol_view(ui, &self.rds_stats.symbols);
        });
        self.show_rds_window = open;
    }

    // ------------------------------------------------------------------ DAB

    fn dab_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let cur = self.s.dab_channel.clone();
            let mut pick = None;
            egui::ComboBox::from_id_salt("dabch")
                .selected_text(RichText::new(format!("{}  {:.3} MHz", cur, model::channel_by_name(&cur).map(|c| c.freq as f64 / 1e6).unwrap_or(0.0))).monospace())
                .height(500.0)
                .show_ui(ui, |ui| {
                    if !self.s.dab_ensembles.is_empty() {
                        ui.label(RichText::new("Found ensembles").small().weak());
                        for e in &self.s.dab_ensembles {
                            if ui.selectable_label(e.channel == cur, format!("{} · {} ({})", e.channel, e.label, e.services.len())).clicked() {
                                pick = Some(e.channel.clone());
                            }
                        }
                        ui.separator();
                    }
                    for c in model::BAND_III.iter() {
                        if ui.selectable_label(c.name == cur, RichText::new(format!("{:<4} {:.3} MHz", c.name, c.freq as f64 / 1e6)).monospace()).clicked() {
                            pick = Some(c.name.to_string());
                        }
                    }
                });
            if let Some(p) = pick {
                self.stop_scan();
                self.set_dab_channel(p);
            }
            if self.scan.is_some() {
                if ui.button("Stop scan").clicked() {
                    self.stop_scan();
                }
            } else if ui.button("Scan").on_hover_text("Tune through Band III and list the ensembles").clicked() {
                self.start_scan();
            }
            if ui.button("Details…").clicked() {
                self.show_dab_window = true;
            }
        });
        if self.scan_state.running {
            ui.add(egui::ProgressBar::new(self.scan_state.index as f32 / self.scan_state.count.max(1) as f32).text(format!(
                "Scanning {} ({}/{}) · {} found",
                self.scan_state.current,
                self.scan_state.index + 1,
                self.scan_state.count,
                self.scan_state.found.len()
            )));
        }
        if !self.scan_state.log.is_empty() {
            egui::CollapsingHeader::new(format!("Scan log ({})", self.scan_state.log.len())).default_open(self.scan_state.running).show(ui, |ui| {
                for l in self.scan_state.log.iter().rev().take(40) {
                    ui.label(RichText::new(l).small().monospace());
                }
            });
        }
        if let Some(e) = &self.dab_error {
            ui.label(RichText::new(e).color(BAD));
        }
        let Some(d) = self.dab.clone() else {
            ui.add_space(10.0);
            ui.label(RichText::new(if self.running { "Starting the DAB receiver…" } else { "Press Start to receive DAB+." }).weak());
            return;
        };
        // status
        ui.horizontal(|ui| {
            let (txt, col) = if d.sync {
                ("SYNC", GOOD)
            } else if d.sync_seen {
                ("sync lost", WARN)
            } else if d.seconds_since_tune < 5.0 {
                ("searching…", WARN)
            } else {
                ("no DAB signal", BAD)
            };
            ui.label(RichText::new(txt).strong().color(col));
            if d.sync {
                let snr_col = if d.snr >= 10 { GOOD } else if d.snr >= 5 { WARN } else { BAD };
                ui.label(RichText::new(format!("SNR {} dB", d.snr)).color(snr_col));
                ui.label(RichText::new(format!("FIC {}%", d.quality.fic)).color(quality_color(d.quality.fic)));
            } else {
                ui.label(RichText::new(format!("input {:.2} Msps, {:.1} dBFS", d.input_rate as f64 / 1e6, d.input_level_db)).weak());
            }
        });
        if !d.ensemble_label.is_empty() {
            ui.label(RichText::new(format!("{}   ·   EId {}   ·   {}", d.ensemble_label, d.ensemble_id_hex(), d.time_utc)).strong());
        }
        if !d.sync && d.seconds_since_tune > 5.0 && !d.sync_seen {
            ui.label(RichText::new("No sync. Check the channel, antenna and gain (HackRF: amp on, LNA 24, VGA 40; RTL-SDR: AGC off, 30-40 dB).").weak().small());
        }
        ui.separator();
        self.now_playing(ui, &d);
        ui.separator();
        // services
        let audio: Vec<_> = d.services.iter().filter(|s| s.is_audio != Some(false)).cloned().collect();
        ui.label(RichText::new(format!("Services ({})", audio.len())).strong());
        for s in &audio {
            let sel = d.selected_sid == Some(s.sid);
            let text = format!("{}   {}", s.label, if s.bitrate > 0 { format!("{} {} kbps", s.codec_short(), s.bitrate) } else { String::new() });
            let resp = ui.add(egui::Button::selectable(sel, RichText::new(text).size(15.0)).min_size(vec2(ui.available_width(), 26.0)));
            if resp.clicked() {
                self.dab_select(s.sid);
            }
        }
    }

    fn now_playing(&mut self, ui: &mut egui::Ui, d: &DabState) {
        let Some(svc) = d.selected().cloned() else {
            ui.label(RichText::new(if d.services.is_empty() { "Waiting for the service list…" } else { "Pick a station below." }).weak());
            return;
        };
        ui.label(RichText::new(&svc.label).size(26.0).strong());
        ui.label(RichText::new(format!("{} · {} kbps · {} · {}", svc.codec(), svc.bitrate, svc.protection(), pty_name(svc.pty))).weak());
        // slideshow
        if let Some(slide) = &d.slide {
            let serial = d.slide_serial;
            if self.slide_tex.as_ref().map(|(s, _)| *s) != Some(serial) {
                if let Ok(img) = image::load_from_memory(slide) {
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let ci = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    let tex = ui.ctx().load_texture("slide", ci, egui::TextureOptions::LINEAR);
                    self.slide_tex = Some((serial, tex));
                }
            }
        } else {
            self.slide_tex = None;
        }
        if let Some((_, tex)) = &self.slide_tex {
            let w = ui.available_width().min(420.0);
            let sz = tex.size_vec2();
            let h = w * sz.y / sz.x.max(1.0);
            ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(tex)).fit_to_exact_size(vec2(w, h)).corner_radius(6.0));
        }
        if !d.dls.is_empty() {
            ui.label(RichText::new(&d.dls).size(16.0));
        }
        if d.selected_sid.is_some() {
            let q = &d.quality;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Audio:").weak());
                ui.label(RichText::new(format!("frames {}%", q.frames)).color(quality_color(q.frames)));
                if svc.ascty == 63 {
                    ui.label(RichText::new(format!("RS {}%", q.rs)).color(quality_color(q.rs)));
                    ui.label(RichText::new(format!("AAC {}%", q.aac)).color(quality_color(q.aac)));
                }
            });
        }
    }

    fn dab_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_dab_window;
        let title = format!("DAB+ · {}", self.s.dab_channel);
        egui::Window::new(title).id(egui::Id::unique("dabwin")).open(&mut open).default_size([760.0, 680.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (i, t) in ["Services", "Ensemble", "Signal", "Now playing"].iter().enumerate() {
                    ui.selectable_value(&mut self.dab_tab, i, RichText::new(*t).size(15.0));
                }
            });
            ui.separator();
            let Some(d) = self.dab.clone() else {
                ui.label("The DAB receiver is not running.");
                return;
            };
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.dab_tab {
                0 => self.dab_services_page(ui, &d),
                1 => dab_ensemble_page(ui, &d),
                2 => dab_signal_page(ui, &d),
                _ => {
                    self.now_playing(ui, &d);
                    if !d.dls_history.is_empty() {
                        ui.separator();
                        ui.label(RichText::new("Text history").strong());
                        for t in &d.dls_history {
                            ui.label(t);
                        }
                    }
                }
            });
        });
        self.show_dab_window = open;
    }

    fn dab_services_page(&mut self, ui: &mut egui::Ui, d: &DabState) {
        if d.services.is_empty() {
            ui.label("No services yet.");
            return;
        }
        let mut play = None;
        egui::Grid::new("svc").striped(true).num_columns(9).spacing([14.0, 4.0]).show(ui, |ui| {
            for h in ["", "Service", "SId", "Codec", "kbps", "Prot.", "Subch", "CU", "PTY / language"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for s in &d.services {
                let sel = d.selected_sid == Some(s.sid);
                if s.is_audio != Some(false) {
                    if ui.small_button(if sel { "♪" } else { "▶" }).clicked() {
                        play = Some(s.sid);
                    }
                } else {
                    ui.label("");
                }
                ui.label(if sel { RichText::new(&s.label).strong() } else { RichText::new(&s.label) });
                ui.monospace(format!("{:04X}", s.sid));
                ui.label(s.codec());
                ui.label(if s.bitrate > 0 { s.bitrate.to_string() } else { "-".into() });
                ui.label(s.protection());
                ui.label(if s.subch >= 0 { s.subch.to_string() } else { "-".into() });
                ui.label(if s.subch >= 0 { format!("{}+{}", s.start_cu, s.length_cu) } else { "-".into() });
                ui.label(format!("{} / {}", pty_name(s.pty), language_name(s.language)));
                ui.end_row();
            }
        });
        ui.label(RichText::new("Subchannel details appear once the receiver has read the ensemble's configuration (FIG 0/1, 0/2).").weak().small());
        if let Some(sid) = play {
            self.dab_select(sid);
        }
    }

    pub fn windows(&mut self, ctx: &egui::Context) {
        if self.show_rds_window {
            self.rds_window(ctx);
        }
        if self.show_dab_window {
            self.dab_window(ctx);
        }
        if self.show_fm_window {
            self.fm_window(ctx);
        }
        if self.show_about {
            let mut open = true;
            egui::Window::new("About RF Analyzer for Windows").open(&mut open).collapsible(false).show(ctx, |ui| {
                ui.label(RichText::new("RF Analyzer for Windows").size(20.0).strong());
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label("A desktop port of Richard's RF Analyzer fork (Android app by Dennis Mantz): spectrum and waterfall, AM/FM/SSB/CW, FM stereo with RDS, and DAB/DAB+.");
                ui.label("DAB decoding: dab-cmdline (Jan van Katwijk) with faad2 and KISS FFT. RTL-SDR and HackRF through librtlsdr and libhackrf.");
                ui.label("License: GPL-2.0-or-later.");
            });
            self.show_about = open;
        }
        if self.show_help {
            let mut open = true;
            egui::Window::new("Keyboard and mouse").open(&mut open).collapsible(false).show(ctx, |ui| {
                egui::Grid::new("help").num_columns(2).spacing([20.0, 4.0]).show(ui, |ui| {
                    kv(ui, "Click spectrum", "Tune the channel there");
                    kv(ui, "Drag the channel", "Move the channel");
                    kv(ui, "Drag elsewhere", "Move the source frequency");
                    kv(ui, "Mouse wheel", "Step the channel by the tune step");
                    kv(ui, "Ctrl + wheel", "Zoom the spectrum (double-click to reset)");
                    kv(ui, "Wheel / drag on dB axis", "Shift the dB range (Ctrl + wheel scales it)");
                    kv(ui, "Wheel on a frequency digit", "Change that digit");
                    kv(ui, "← / →", "Step the channel (DAB: previous/next channel)");
                    kv(ui, "Page Up / Down", "10 steps");
                    kv(ui, "F5 or Space", "Start / stop");
                    kv(ui, "M", "Mute");
                    kv(ui, "Ctrl + / Ctrl -", "UI size");
                });
            });
            self.show_help = open;
        }
    }
}

fn dab_ensemble_page(ui: &mut egui::Ui, d: &DabState) {
    egui::Grid::new("ens").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        kv(ui, "Ensemble", d.ensemble_label.clone());
        kv(ui, "EId", d.ensemble_id_hex());
        kv(ui, "Channel", d.channel().map(|c| format!("{} ({:.3} MHz)", c.name, c.freq as f64 / 1e6)).unwrap_or_default());
        kv(ui, "Time (UTC)", d.time_utc.clone());
        kv(ui, "Services", d.services.len().to_string());
        kv(ui, "Capacity used", format!("{} of 864 CU", d.used_cu()));
    });
    ui.add_space(8.0);
    ui.label(RichText::new("Main service channel (capacity units)").strong());
    // CU map
    let w = ui.available_width().min(720.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, Color32::from_gray(40));
    let mut subs: Vec<_> = d.services.iter().filter(|s| s.subch >= 0).collect();
    subs.sort_by_key(|s| s.start_cu);
    subs.dedup_by_key(|s| s.subch);
    let palette = [
        Color32::from_rgb(66, 133, 244),
        Color32::from_rgb(219, 68, 55),
        Color32::from_rgb(244, 180, 0),
        Color32::from_rgb(15, 157, 88),
        Color32::from_rgb(171, 71, 188),
        Color32::from_rgb(0, 172, 193),
        Color32::from_rgb(255, 112, 67),
        Color32::from_rgb(158, 157, 36),
    ];
    let mut hovered = None;
    for (i, s) in subs.iter().enumerate() {
        let x0 = rect.left() + s.start_cu as f32 / 864.0 * rect.width();
        let x1 = rect.left() + (s.start_cu + s.length_cu) as f32 / 864.0 * rect.width();
        let r = egui::Rect::from_min_max(pos2(x0, rect.top()), pos2(x1.max(x0 + 1.0), rect.bottom()));
        p.rect_filled(r.shrink2(vec2(0.5, 0.0)), 2.0, palette[i % palette.len()]);
        if r.width() > 30.0 {
            p.text(r.center(), egui::Align2::CENTER_CENTER, s.label.chars().take((r.width() / 7.0) as usize).collect::<String>(), egui::FontId::proportional(11.0), Color32::WHITE);
        }
        if let Some(pos) = resp.hover_pos() {
            if r.contains(pos) {
                hovered = Some(*s);
            }
        }
    }
    if let Some(s) = hovered {
        resp.on_hover_text(format!("{}: subch {}, CU {}+{}, {} kbps, {}", s.label, s.subch, s.start_cu, s.length_cu, s.bitrate, s.protection()));
    }
    ui.add_space(8.0);
    egui::Grid::new("subch").striped(true).num_columns(6).spacing([14.0, 3.0]).show(ui, |ui| {
        for h in ["Subch", "Start CU", "Size CU", "kbps", "Protection", "Service"] {
            ui.label(RichText::new(h).strong());
        }
        ui.end_row();
        for s in &subs {
            ui.label(s.subch.to_string());
            ui.label(s.start_cu.to_string());
            ui.label(s.length_cu.to_string());
            ui.label(s.bitrate.to_string());
            ui.label(s.protection());
            ui.label(&s.label);
            ui.end_row();
        }
    });
    if subs.is_empty() {
        ui.label(RichText::new("Subchannels appear as the receiver reads them.").weak());
    }
}

fn dab_signal_page(ui: &mut egui::Ui, d: &DabState) {
    ui.horizontal(|ui| {
        for (label, value, col) in [
            ("SNR", format!("{} dB", d.snr), if d.snr >= 10 { GOOD } else if d.snr >= 5 { WARN } else { BAD }),
            ("FIC", format!("{}%", d.quality.fic), quality_color(d.quality.fic)),
            ("Frames", format!("{}%", d.quality.frames), quality_color(d.quality.frames)),
            ("RS", format!("{}%", d.quality.rs), quality_color(d.quality.rs)),
            ("AAC", format!("{}%", d.quality.aac), quality_color(d.quality.aac)),
        ] {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).weak().small());
                ui.label(RichText::new(value).size(22.0).strong().color(col));
            });
            ui.add_space(14.0);
        }
    });
    sparkline(ui, &d.snr_history, 0.0, 25.0, "SNR (dB), last 2 minutes", false);
    egui::Grid::new("sig").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        kv(ui, "Sync", if d.sync { "yes".into() } else if d.sync_seen { "lost".into() } else { "no".to_string() });
        kv(ui, "Frequency offset", format!("{} Hz", d.freq_offset));
        kv(ui, "Input", format!("{:.3} Msps, {:.1} dBFS", d.input_rate as f64 / 1e6, d.input_level_db));
        kv(ui, "Dropped samples", d.dropped_samples.to_string());
        kv(ui, "Audio", format!("{} Hz, in {}/s, out {}/s, buffered {} ms", d.audio_rate, d.audio_in_per_sec, d.audio_out_per_sec, d.audio_buffered_ms));
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Constellation (DQPSK, after differential demodulation)").strong());
    let size = 260.0;
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, Color32::from_gray(25));
    p.line_segment([pos2(rect.center().x, rect.top()), pos2(rect.center().x, rect.bottom())], Stroke::new(1.0, Color32::from_gray(70)));
    p.line_segment([pos2(rect.left(), rect.center().y), pos2(rect.right(), rect.center().y)], Stroke::new(1.0, Color32::from_gray(70)));
    for pt in &d.constellation {
        let x = rect.center().x + pt[0] * size * 0.42;
        let y = rect.center().y - pt[1] * size * 0.42;
        p.circle_filled(pos2(x, y), 1.3, Color32::from_rgba_unmultiplied(120, 200, 255, 140));
    }
    ui.add_space(6.0);
    ui.label(RichText::new("Transmitters (TII)").strong());
    if d.transmitters.is_empty() {
        ui.label(RichText::new("None identified yet. TII needs a good signal and takes a while.").weak().small());
    } else {
        egui::Grid::new("tii").striped(true).num_columns(3).show(ui, |ui| {
            for t in &d.transmitters {
                ui.monospace(format!("main {:>2}  sub {:>2}", t.main_id, t.sub_id));
                ui.add(egui::ProgressBar::new((t.strength / 2.0).clamp(0.0, 1.0)).desired_width(160.0));
                ui.label(format!("{:.2}", t.strength));
                ui.end_row();
            }
        });
    }
}

fn sparkline(ui: &mut egui::Ui, data: &[f32], lo: f32, hi: f32, label: &str, invert_good: bool) {
    let w = ui.available_width().min(500.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 50.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, Color32::from_gray(30));
    if data.len() >= 2 {
        let n = data.len();
        let pts: Vec<_> = data
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let x = rect.left() + i as f32 / (n - 1) as f32 * rect.width();
                let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
                pos2(x, rect.bottom() - t * rect.height())
            })
            .collect();
        let col = if invert_good { Color32::from_rgb(230, 150, 60) } else { Color32::from_rgb(90, 200, 120) };
        p.add(egui::Shape::line(pts, Stroke::new(1.5, col)));
    }
    p.text(rect.left_top() + vec2(4.0, 2.0), egui::Align2::LEFT_TOP, label, egui::FontId::proportional(11.0), Color32::from_gray(160));
}

fn mpx_view(ui: &mut egui::Ui, db: &[f32]) {
    let w = ui.available_width().min(600.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 140.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, Color32::from_gray(25));
    for (f, name) in [(19.0, "19"), (38.0, "38"), (57.0, "57")] {
        let x = rect.left() + (f / 96.0) * rect.width();
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 200, 60, 90)));
        p.text(pos2(x + 2.0, rect.top() + 2.0), egui::Align2::LEFT_TOP, format!("{name} kHz"), egui::FontId::proportional(10.0), Color32::from_gray(170));
    }
    if db.len() > 2 {
        let n = db.len();
        let (lo, hi) = (-110.0f32, -10.0f32);
        let pts: Vec<_> = db
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let x = rect.left() + i as f32 / (n - 1) as f32 * rect.width();
                pos2(x, rect.bottom() - ((v - lo) / (hi - lo)).clamp(0.0, 1.0) * rect.height())
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.0, Color32::from_rgb(120, 200, 255))));
    } else {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, "collecting…", egui::FontId::proportional(12.0), Color32::GRAY);
    }
}

fn symbol_view(ui: &mut egui::Ui, sym: &[f32]) {
    let w = ui.available_width().min(600.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 70.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, Color32::from_gray(25));
    p.line_segment([pos2(rect.left(), rect.center().y), pos2(rect.right(), rect.center().y)], Stroke::new(1.0, Color32::from_gray(70)));
    let n = sym.len().max(1);
    for (i, &v) in sym.iter().enumerate() {
        let x = rect.left() + i as f32 / n as f32 * rect.width();
        let y = rect.center().y - (v / 2.0).clamp(-1.0, 1.0) * rect.height() * 0.45;
        p.circle_filled(pos2(x, y), 1.4, Color32::from_rgb(120, 200, 255));
    }
}

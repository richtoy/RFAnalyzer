//! Spectrum plot and waterfall, with tuning by click, drag, wheel and zoom.
//!
//! Waterfall storage: one GPU texture of `wf_w` × WF_ROWS pixels used as a circular buffer.
//! Each new spectrum frame becomes one row, written with `set_partial` (only that row is
//! uploaded) at `wf_row`, which moves *up* one row per frame. So reading the texture from
//! `wf_row` downwards (wrapping at the bottom) gives newest → oldest, and drawing it takes two
//! textured rectangles: rows wf_row..end at the top of the screen, then rows 0..wf_row below.
//!
//! Coordinates: `view()` gives the visible span (centre, width in Hz); zooming only changes
//! that span, while the data always covers the full sample rate. The closures `x_of`/`f_of`
//! convert between Hz and screen x, `y_of` between dB and screen y.
//!
//! The view never changes settings itself: user input becomes [`Action`]s that `show` returns
//! and the app applies (tuning, dB range).

use super::colormap;
use crate::settings::{ColorMap, Mode};
use eframe::egui::{self, pos2, vec2, Color32, Mesh, Pos2, Rect, Sense, Shape, Stroke, TextureHandle, TextureOptions, Ui};

const WF_ROWS: usize = 1024;
const AXIS_W: f32 = 48.0;

pub enum Action {
    /// New channel frequency (displayed Hz)
    SetChannel(u64),
    /// Move the source centre by this many Hz
    TuneSource(f64),
    /// Step the channel by n tune steps
    StepChannel(i32),
    SetDbRange(f32, f32),
}

#[derive(PartialEq)]
enum Drag {
    None,
    Pan,
    Channel(f64),
    Axis,
}

pub struct SpectrumView {
    /// Waterfall texture (circular buffer of rows, see module docs).
    tex: Option<TextureHandle>,
    /// Waterfall texture width in pixels (bins are max-pooled down to this).
    wf_w: usize,
    /// Texture row holding the newest line.
    wf_row: usize,
    /// 256-entry colour lookup table for the current colour map.
    cmap: Vec<Color32>,
    cmap_kind: Option<ColorMap>,
    /// Latest spectrum (dB per bin) and the decaying peak-hold trace.
    pub latest: Vec<f32>,
    peak: Vec<f32>,
    /// Centre frequency and sample rate of the latest frame (the full data span).
    pub center: f64,
    pub rate: f64,
    /// Zoom/pan; None = full span centred on the data.
    view_center: Option<f64>,
    view_span: Option<f64>,
    /// What the current mouse drag is doing (decided when the drag starts).
    drag: Drag,
    has_frame: bool,
}

pub struct ViewParams {
    pub db_min: f32,
    pub db_max: f32,
    pub channel: u64,
    pub mode: Mode,
    pub width: u32,
    pub step: u32,
    pub fill: bool,
    pub peak_hold: bool,
    pub split: f32,
    pub squelch_db: Option<f32>,
    pub dab_label: Option<String>,
}

impl SpectrumView {
    pub fn new() -> Self {
        Self {
            tex: None,
            wf_w: 2048,
            wf_row: 0,
            cmap: colormap::build(ColorMap::Gqrx),
            cmap_kind: None,
            latest: Vec::new(),
            peak: Vec::new(),
            center: 100e6,
            rate: 2.4e6,
            view_center: None,
            view_span: None,
            drag: Drag::None,
            has_frame: false,
        }
    }

    pub fn reset_zoom(&mut self) {
        self.view_center = None;
        self.view_span = None;
    }

    pub fn clear(&mut self) {
        self.has_frame = false;
        self.latest.clear();
        self.peak.clear();
    }

    /// Adds a spectrum frame: updates the plot data and writes one waterfall row.
    pub fn push_frame(&mut self, ctx: &egui::Context, db: &[f32], center: f64, rate: f64, db_min: f32, db_max: f32, cmap: ColorMap) {
        if self.cmap_kind != Some(cmap) {
            self.cmap = colormap::build(cmap);
            self.cmap_kind = Some(cmap);
        }
        if (rate - self.rate).abs() > 1.0 {
            // a new sample rate: the zoom no longer applies
            self.reset_zoom();
        }
        self.center = center;
        self.rate = rate;
        self.has_frame = true;
        if self.peak.len() != db.len() {
            self.peak = db.to_vec();
        } else {
            // peak hold: jump up to a new maximum, otherwise decay 0.05 dB per frame
            for (p, &v) in self.peak.iter_mut().zip(db) {
                *p = if v > *p { v } else { *p - 0.05 };
            }
        }
        self.latest.clear();
        self.latest.extend_from_slice(db);

        let w = self.wf_w;
        if self.tex.is_none() {
            let img = egui::ColorImage::filled([w, WF_ROWS], Color32::BLACK);
            self.tex = Some(ctx.load_texture("waterfall", img, TextureOptions::LINEAR));
        }
        // one row: max of the bins in each column
        let n = db.len();
        let range = (db_max - db_min).max(1.0);
        let mut row = Vec::with_capacity(w);
        for x in 0..w {
            // bins b0..b1 fall into pixel column x; take their maximum so narrow carriers
            // don't vanish, then map dB to 0..1 and look up the colour
            let b0 = x * n / w;
            let b1 = ((x + 1) * n / w).max(b0 + 1).min(n);
            let v = db[b0..b1].iter().copied().fold(f32::MIN, f32::max);
            let t = ((v - db_min) / range).clamp(0.0, 1.0);
            row.push(self.cmap[(t * 255.0) as usize]);
        }
        // rows are written upwards, so texture rows wf_row.. are newest to oldest
        self.wf_row = (self.wf_row + WF_ROWS - 1) % WF_ROWS;
        let img = egui::ColorImage::new([w, 1], row);
        if let Some(t) = self.tex.as_mut() {
            t.set_partial([0, self.wf_row], img, TextureOptions::LINEAR);
        }
    }

    /// Visible (centre, span) in Hz, clamped so the view stays inside the data.
    fn view(&self) -> (f64, f64) {
        let span = self.view_span.unwrap_or(self.rate).min(self.rate);
        let lo_lim = self.center - self.rate / 2.0 + span / 2.0;
        let hi_lim = self.center + self.rate / 2.0 - span / 2.0;
        let vc = self.view_center.unwrap_or(self.center).clamp(lo_lim, hi_lim.max(lo_lim));
        (vc, span)
    }

    pub fn show(&mut self, ui: &mut Ui, p: &ViewParams) -> Vec<Action> {
        let mut actions = Vec::new();
        let full = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(full, Sense::click_and_drag());
        let painter = ui.painter_at(full);
        let dark = ui.visuals().dark_mode;
        let bg = if dark { Color32::from_rgb(12, 14, 18) } else { Color32::from_rgb(250, 250, 252) };
        painter.rect_filled(full, 0.0, bg);

        let plot = Rect::from_min_max(pos2(full.left() + AXIS_W, full.top()), full.max);
        let split_y = plot.top() + plot.height() * p.split.clamp(0.15, 0.85);
        let spec = Rect::from_min_max(plot.min, pos2(plot.right(), split_y - 18.0));
        let axis_rect = Rect::from_min_max(pos2(plot.left(), split_y - 18.0), pos2(plot.right(), split_y));
        let wf = Rect::from_min_max(pos2(plot.left(), split_y), plot.max);
        let db_axis = Rect::from_min_max(full.min, pos2(full.left() + AXIS_W, split_y - 18.0));

        let (vc, span) = self.view();
        let f_lo = vc - span / 2.0;
        let x_of = |f: f64| plot.left() + ((f - f_lo) / span) as f32 * plot.width();
        let f_of = |x: f32| f_lo + ((x - plot.left()) / plot.width()) as f64 * span;
        let db_range = (p.db_max - p.db_min).max(1.0);
        let y_of = |db: f32| spec.top() + (p.db_max - db) / db_range * spec.height();

        let grid = if dark { Color32::from_rgba_unmultiplied(255, 255, 255, 22) } else { Color32::from_rgba_unmultiplied(0, 0, 0, 25) };
        let text_col = if dark { Color32::from_gray(170) } else { Color32::from_gray(70) };
        let font = egui::FontId::proportional(12.0);

        // dB grid
        let mut db = (p.db_min / 10.0).ceil() * 10.0;
        while db <= p.db_max {
            let y = y_of(db);
            painter.line_segment([pos2(spec.left(), y), pos2(spec.right(), y)], Stroke::new(1.0, grid));
            painter.text(pos2(full.left() + AXIS_W - 4.0, y), egui::Align2::RIGHT_CENTER, format!("{db:.0}"), font.clone(), text_col);
            db += 10.0;
        }

        // frequency grid
        let step = nice_step(span / (plot.width() as f64 / 110.0));
        let mut f = (f_lo / step).ceil() * step;
        let decimals = if step >= 1e6 { 1 } else if step >= 1e5 { 2 } else if step >= 1e4 { 3 } else if step >= 1e3 { 4 } else { 6 };
        while f <= f_lo + span {
            let x = x_of(f);
            painter.line_segment([pos2(x, spec.top()), pos2(x, spec.bottom())], Stroke::new(1.0, grid));
            painter.line_segment([pos2(x, axis_rect.top()), pos2(x, axis_rect.top() + 4.0)], Stroke::new(1.0, text_col));
            painter.text(pos2(x, axis_rect.center().y + 2.0), egui::Align2::CENTER_CENTER, format!("{:.*}", decimals, f / 1e6), font.clone(), text_col);
            f += step;
        }

        if self.has_frame && !self.latest.is_empty() {
            let n = self.latest.len();
            let band_lo = self.center - self.rate / 2.0;
            let cols = plot.width().max(1.0) as usize;
            let bin_of = |f: f64| ((f - band_lo) / self.rate * n as f64).clamp(0.0, n as f64 - 1.0);
            // one point per screen column: the max of the bins under that column
            let curve = |data: &[f32]| -> Vec<Pos2> {
                let mut pts = Vec::with_capacity(cols + 1);
                for c in 0..=cols {
                    let x0 = plot.left() + c as f32;
                    let b0 = bin_of(f_of(x0)) as usize;
                    let b1 = (bin_of(f_of(x0 + 1.0)) as usize).max(b0);
                    let v = data[b0..=b1.min(n - 1)].iter().copied().fold(f32::MIN, f32::max);
                    pts.push(pos2(x0, y_of(v).clamp(spec.top(), spec.bottom())));
                }
                pts
            };
            let pts = curve(&self.latest);
            let line_col = if dark { Color32::from_rgb(120, 200, 255) } else { Color32::from_rgb(20, 90, 180) };
            if p.fill {
                // filled area under the curve as a triangle strip: for each point a vertex on
                // the curve (index 2i) and one at the bottom (2i+1); two triangles per column
                let mut mesh = Mesh::default();
                let top_col = if dark { Color32::from_rgba_unmultiplied(80, 160, 230, 120) } else { Color32::from_rgba_unmultiplied(40, 110, 200, 90) };
                let bot_col = Color32::from_rgba_unmultiplied(40, 80, 140, 10);
                for (i, pt) in pts.iter().enumerate() {
                    mesh.colored_vertex(*pt, top_col);
                    mesh.colored_vertex(pos2(pt.x, spec.bottom()), bot_col);
                    if i > 0 {
                        let k = (i * 2) as u32;
                        mesh.add_triangle(k - 2, k - 1, k);
                        mesh.add_triangle(k - 1, k + 1, k);
                    }
                }
                painter.add(Shape::mesh(mesh));
            }
            painter.add(Shape::line(pts, Stroke::new(1.2, line_col)));
            if p.peak_hold && self.peak.len() == n {
                let pk = curve(&self.peak);
                painter.add(Shape::line(pk, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 200, 60, 160))));
            }

            // waterfall
            if let Some(tex) = &self.tex {
                // texture coordinates: u0..u1 = the zoomed frequency range (0..1 = full span);
                // r = the newest row's position, where the circular buffer is split in two
                let u0 = ((f_lo - band_lo) / self.rate) as f32;
                let u1 = ((f_lo + span - band_lo) / self.rate) as f32;
                let r = self.wf_row as f32 / WF_ROWS as f32;
                let h1 = wf.height() * (1.0 - r);
                let top = Rect::from_min_max(wf.min, pos2(wf.right(), wf.top() + h1));
                painter.image(tex.id(), top, Rect::from_min_max(pos2(u0, r), pos2(u1, 1.0)), Color32::WHITE);
                if r > 0.0 {
                    let bottom = Rect::from_min_max(pos2(wf.left(), wf.top() + h1), wf.max);
                    painter.image(tex.id(), bottom, Rect::from_min_max(pos2(u0, 0.0), pos2(u1, r)), Color32::WHITE);
                }
            }
        } else {
            painter.text(spec.center(), egui::Align2::CENTER_CENTER, "Press Start (or F5) to begin", egui::FontId::proportional(22.0), text_col);
        }

        // channel overlay
        if p.mode != Mode::Off {
            let (lo, hi) = p.mode.passband(p.width);
            let ch = p.channel as f64;
            let xa = x_of(ch + lo);
            let xb = x_of(ch + hi);
            let xc = x_of(ch);
            let band = Rect::from_min_max(pos2(xa.min(xb - 2.0), spec.top()), pos2(xb.max(xa + 2.0), wf.bottom()));
            painter.rect_filled(band.intersect(plot), 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, if dark { 22 } else { 40 }));
            if xc >= plot.left() && xc <= plot.right() {
                painter.line_segment([pos2(xc, spec.top()), pos2(xc, wf.bottom())], Stroke::new(1.5, Color32::from_rgb(230, 60, 60)));
            }
            if let Some(sq) = p.squelch_db {
                let y = y_of(sq);
                if y > spec.top() && y < spec.bottom() {
                    painter.line_segment([pos2(xa.max(plot.left()), y), pos2(xb.min(plot.right()), y)], Stroke::new(1.5, Color32::from_rgb(250, 180, 40)));
                }
            }
            if let Some(label) = &p.dab_label {
                painter.text(pos2(xc + 6.0, spec.top() + 6.0), egui::Align2::LEFT_TOP, label, egui::FontId::proportional(14.0), Color32::from_rgb(240, 240, 240));
            }
        }

        // hover readout
        if let Some(pos) = resp.hover_pos() {
            if plot.contains(pos) {
                let f = f_of(pos.x);
                let mut text = format!("{:.4} MHz", f / 1e6);
                if spec.contains(pos) {
                    let dbv = p.db_max - (pos.y - spec.top()) / spec.height() * db_range;
                    text += &format!("   {dbv:.1} dB");
                }
                painter.line_segment([pos2(pos.x, plot.top()), pos2(pos.x, plot.bottom())], Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 40)));
                painter.text(pos2(spec.right() - 8.0, spec.top() + 6.0), egui::Align2::RIGHT_TOP, text, egui::FontId::monospace(14.0), text_col);
            }
        }

        // ---- interaction
        let snap = |f: f64| {
            let s = p.step.max(1) as f64;
            ((f / s).round() * s).max(0.0) as u64
        };
        if resp.drag_started() {
            if let Some(pos) = resp.interact_pointer_pos() {
                self.drag = if db_axis.contains(pos) {
                    Drag::Axis
                } else if p.mode != Mode::Off && p.mode != Mode::Dab && {
                    let (lo, hi) = p.mode.passband(p.width);
                    let ch = p.channel as f64;
                    let xa = x_of(ch + lo).min(x_of(ch) - 6.0);
                    let xb = x_of(ch + hi).max(x_of(ch) + 6.0);
                    pos.x >= xa && pos.x <= xb
                } {
                    Drag::Channel(f_of(pos.x) - p.channel as f64)
                } else {
                    Drag::Pan
                };
            }
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            match self.drag {
                Drag::Pan => {
                    let df = -(d.x as f64) / plot.width() as f64 * span;
                    if self.view_span.is_some() && span < self.rate - 1.0 {
                        // zoomed: pan the view, retune only beyond the band edge
                        let new_vc = vc + df;
                        let lo_lim = self.center - self.rate / 2.0 + span / 2.0;
                        let hi_lim = self.center + self.rate / 2.0 - span / 2.0;
                        if new_vc < lo_lim {
                            actions.push(Action::TuneSource(new_vc - lo_lim));
                        } else if new_vc > hi_lim {
                            actions.push(Action::TuneSource(new_vc - hi_lim));
                        }
                        self.view_center = Some(new_vc);
                    } else {
                        actions.push(Action::TuneSource(df));
                    }
                }
                Drag::Channel(off) => {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        actions.push(Action::SetChannel(snap(f_of(pos.x) - off)));
                    }
                }
                Drag::Axis => {
                    let ddb = d.y / spec.height() * db_range;
                    actions.push(Action::SetDbRange(p.db_min + ddb, p.db_max + ddb));
                }
                Drag::None => {}
            }
        }
        if resp.drag_stopped() {
            self.drag = Drag::None;
        }
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if plot.contains(pos) && p.mode != Mode::Off && p.mode != Mode::Dab {
                    actions.push(Action::SetChannel(snap(f_of(pos.x))));
                }
            }
        }
        if resp.double_clicked() {
            self.reset_zoom();
        }
        if resp.hovered() {
            let (zoom, notches, pos) = ui.input(|i| {
                let mut notches = 0f32;
                for e in &i.events {
                    if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = e {
                        if modifiers.ctrl || modifiers.command {
                            continue;
                        }
                        notches += match unit {
                            egui::MouseWheelUnit::Line => delta.y,
                            egui::MouseWheelUnit::Page => delta.y * 3.0,
                            egui::MouseWheelUnit::Point => delta.y / 50.0,
                        };
                    }
                }
                (i.zoom_delta(), notches, i.pointer.hover_pos())
            });
            if let Some(pos) = pos {
                if (zoom - 1.0).abs() > 1e-3 {
                    if db_axis.contains(pos) {
                        // scale the dB range around the pointer
                        let at = p.db_max - (pos.y - spec.top()) / spec.height() * db_range;
                        let k = 1.0 / zoom;
                        actions.push(Action::SetDbRange(at - (at - p.db_min) * k, at + (p.db_max - at) * k));
                    } else {
                        // zoom around the pointer: keep the frequency under it at the same x
                        let at = f_of(pos.x);
                        let new_span = (span / zoom as f64).clamp(self.rate / 512.0, self.rate);
                        let frac = ((pos.x - plot.left()) / plot.width()) as f64;
                        self.view_span = if new_span >= self.rate - 1.0 { None } else { Some(new_span) };
                        self.view_center = Some(at - (frac - 0.5) * new_span);
                    }
                } else if notches.abs() >= 0.5 && plot.contains(pos) && p.mode != Mode::Off {
                    actions.push(Action::StepChannel(notches.round() as i32));
                } else if notches.abs() >= 0.5 && db_axis.contains(pos) {
                    let ddb = notches * 5.0;
                    actions.push(Action::SetDbRange(p.db_min + ddb, p.db_max + ddb));
                }
            }
        }
        // zoom indicator
        if self.view_span.is_some() {
            painter.text(
                pos2(spec.left() + 6.0, spec.top() + 6.0),
                egui::Align2::LEFT_TOP,
                format!("zoom {:.0}x (double-click to reset)", self.rate / span),
                egui::FontId::proportional(12.0),
                text_col,
            );
        }
        let _ = vec2(0.0, 0.0);
        actions
    }
}

/// Rounds a grid spacing up to 1, 2, 2.5 or 5 × a power of ten.
fn nice_step(raw: f64) -> f64 {
    let exp = raw.log10().floor();
    let base = 10f64.powf(exp);
    let m = raw / base;
    let k = if m <= 1.0 {
        1.0
    } else if m <= 2.0 {
        2.0
    } else if m <= 2.5 {
        2.5
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    };
    k * base
}

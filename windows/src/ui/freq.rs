//! Large frequency display: hover a digit and scroll (or click its upper/lower half) to change it.

use eframe::egui::{self, pos2, vec2, Color32, Rect, Sense, Ui};

pub fn frequency_display(ui: &mut Ui, id: &str, freq: u64, digit_h: f32, enabled: bool) -> Option<u64> {
    const DIGITS: usize = 10; // up to 9.999 999 999 GHz
    let digit_w = digit_h * 0.62;
    let sep_w = digit_h * 0.28;
    let total_w = DIGITS as f32 * digit_w + 3.0 * sep_w;
    let (rect, _) = ui.allocate_exact_size(vec2(total_w, digit_h * 1.15), Sense::hover());
    let painter = ui.painter_at(rect);
    let dark = ui.visuals().dark_mode;
    let on = if dark { Color32::from_rgb(120, 220, 255) } else { Color32::from_rgb(10, 80, 160) };
    let dim = if dark { Color32::from_gray(70) } else { Color32::from_gray(190) };
    let s = format!("{:0width$}", freq.min(9_999_999_999), width = DIGITS);
    let first_sig = s.find(|c: char| c != '0').unwrap_or(DIGITS - 1);
    let font = egui::FontId::monospace(digit_h);
    let mut x = rect.left();
    let mut result = None;
    for (i, ch) in s.chars().enumerate() {
        if i == 1 || i == 4 || i == 7 {
            painter.text(pos2(x + sep_w / 2.0, rect.bottom() - digit_h * 0.18), egui::Align2::CENTER_BOTTOM, ".", font.clone(), if i > first_sig { on } else { dim });
            x += sep_w;
        }
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(digit_w, rect.height()));
        let power = 10u64.pow((DIGITS - 1 - i) as u32);
        let resp = ui.interact(r, ui.scope_id().with((id, i)), Sense::click());
        if enabled && resp.hovered() {
            painter.rect_filled(r.shrink(1.0), 4.0, Color32::from_rgba_unmultiplied(128, 128, 128, 40));
            let notches = ui.input(|inp| {
                let mut n = 0f32;
                for e in &inp.events {
                    if let egui::Event::MouseWheel { unit, delta, .. } = e {
                        n += match unit {
                            egui::MouseWheelUnit::Line => delta.y,
                            egui::MouseWheelUnit::Page => delta.y * 3.0,
                            egui::MouseWheelUnit::Point => delta.y / 50.0,
                        };
                    }
                }
                n
            });
            let n = notches.round() as i64;
            if n != 0 {
                result = Some((freq as i64 + n * power as i64).clamp(0, 9_999_999_999) as u64);
            }
        }
        if enabled && resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let up = p.y < r.center().y;
                let v = freq as i64 + if up { power as i64 } else { -(power as i64) };
                result = Some(v.clamp(0, 9_999_999_999) as u64);
            }
        }
        painter.text(r.center(), egui::Align2::CENTER_CENTER, ch, font.clone(), if i >= first_sig { on } else { dim });
        x += digit_w;
    }
    result
}

/// Parses "97.3", "97.3M", "1090000k", "7100 kHz", "227360000" into Hz. Plain numbers below
/// 10 000 are taken as MHz.
pub fn parse_frequency(s: &str) -> Option<u64> {
    let t = s.trim().to_ascii_lowercase().replace(',', ".").replace(' ', "");
    let t = t.trim_end_matches("hz");
    let (num, mult) = if let Some(v) = t.strip_suffix('g') {
        (v, 1e9)
    } else if let Some(v) = t.strip_suffix('m') {
        (v, 1e6)
    } else if let Some(v) = t.strip_suffix('k') {
        (v, 1e3)
    } else {
        (t, 0.0)
    };
    let v: f64 = num.parse().ok()?;
    let hz = if mult > 0.0 { v * mult } else if v < 10_000.0 { v * 1e6 } else { v };
    if hz.is_finite() && hz >= 0.0 { Some(hz.round() as u64) } else { None }
}

#[cfg(test)]
mod tests {
    use super::parse_frequency;
    #[test]
    fn parses() {
        assert_eq!(parse_frequency("97.3"), Some(97_300_000));
        assert_eq!(parse_frequency("7100 kHz"), Some(7_100_000));
        assert_eq!(parse_frequency("227360000"), Some(227_360_000));
        assert_eq!(parse_frequency("1.09G"), Some(1_090_000_000));
    }
}

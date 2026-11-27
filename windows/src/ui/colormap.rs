//! Waterfall colour maps (GQRX colours from gqrx/CuteSDR, simplified BSD; JET; Turbo).

use crate::settings::ColorMap;
use eframe::egui::Color32;

pub fn build(map: ColorMap) -> Vec<Color32> {
    (0..256).map(|i| color(map, i)).collect()
}

fn color(map: ColorMap, i: i32) -> Color32 {
    match map {
        ColorMap::Gqrx => {
            let (r, g, b) = if i < 20 {
                (0, 0, 0)
            } else if i < 70 {
                (0, 0, 140 * (i - 20) / 50)
            } else if i < 100 {
                (60 * (i - 70) / 30, 125 * (i - 70) / 30, 115 * (i - 70) / 30 + 140)
            } else if i < 150 {
                (195 * (i - 100) / 50 + 60, 130 * (i - 100) / 50 + 125, 255 - 255 * (i - 100) / 50)
            } else if i < 250 {
                (255, 255 - 255 * (i - 150) / 100, 0)
            } else {
                (255, 255 * (i - 250) / 5, 255 * (i - 250) / 5)
            };
            Color32::from_rgb(r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8)
        }
        ColorMap::Jet => {
            let x = i as f32 / 255.0;
            let (r, g, b) = if x < 0.125 {
                (0.0, 0.0, 0.5 + 0.5 * x / 0.125)
            } else if x < 0.375 {
                (0.0, (x - 0.125) / 0.25, 1.0)
            } else if x < 0.625 {
                let y = (x - 0.375) / 0.25;
                (y, 1.0, 1.0 - y)
            } else if x < 0.875 {
                (1.0, 1.0 - (x - 0.625) / 0.25, 0.0)
            } else {
                (1.0 - 0.5 * (x - 0.875) / 0.125, 0.0, 0.0)
            };
            Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
        }
        ColorMap::Turbo => {
            // polynomial approximation of Google's Turbo colormap
            let x = i as f64 / 255.0;
            let r = 0.13572138 + x * (4.61539260 + x * (-42.66032258 + x * (132.13108234 + x * (-152.94239396 + x * 59.28637943))));
            let g = 0.09140261 + x * (2.19418839 + x * (4.84296658 + x * (-14.18503333 + x * (4.27729857 + x * 2.82956604))));
            let b = 0.10667330 + x * (12.64194608 + x * (-60.58204836 + x * (110.36276771 + x * (-89.90310912 + x * 27.34824973))));
            Color32::from_rgb((r.clamp(0.0, 1.0) * 255.0) as u8, (g.clamp(0.0, 1.0) * 255.0) as u8, (b.clamp(0.0, 1.0) * 255.0) as u8)
        }
    }
}

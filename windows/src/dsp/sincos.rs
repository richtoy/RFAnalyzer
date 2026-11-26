//! Fast sine/cosine for phases given in cycles.

use std::sync::OnceLock;

const BITS: u32 = 14;
const SIZE: usize = 1 << BITS;
const MASK: usize = SIZE - 1;

pub struct SinCos {
    table: Vec<f32>,
}

impl SinCos {
    pub fn get() -> &'static SinCos {
        static T: OnceLock<SinCos> = OnceLock::new();
        T.get_or_init(|| SinCos {
            table: (0..SIZE).map(|i| (2.0 * std::f64::consts::PI * i as f64 / SIZE as f64).sin() as f32).collect(),
        })
    }
    #[inline]
    pub fn sin(&self, cycles: f64) -> f32 {
        self.table[(((cycles - cycles.floor()) * SIZE as f64) as usize) & MASK]
    }
    #[inline]
    pub fn cos(&self, cycles: f64) -> f32 {
        self.table[((((cycles - cycles.floor()) * SIZE as f64) as usize) + SIZE / 4) & MASK]
    }
}

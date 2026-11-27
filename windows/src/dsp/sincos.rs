//! Fast sine/cosine for phases given in cycles.
//!
//! One shared lookup table of 2^14 sine values over one full cycle. A phase in cycles
//! (1.0 = 360°) is reduced to its fractional part, scaled to a table index and masked, so
//! the lookup is a multiply, a cast and an AND. Cosine is the same table read a quarter
//! cycle (SIZE/4 entries) ahead. Resolution is 360°/16384 ≈ 0.022°, plenty for the PLLs.

use std::sync::OnceLock;

const BITS: u32 = 14;
const SIZE: usize = 1 << BITS;
const MASK: usize = SIZE - 1;

pub struct SinCos {
    /// table[i] = sin(2π·i/SIZE), i in 0..SIZE.
    table: Vec<f32>,
}

impl SinCos {
    /// Returns the process-wide table, building it on first use (thread safe via OnceLock).
    pub fn get() -> &'static SinCos {
        static T: OnceLock<SinCos> = OnceLock::new();
        T.get_or_init(|| SinCos {
            table: (0..SIZE).map(|i| (2.0 * std::f64::consts::PI * i as f64 / SIZE as f64).sin() as f32).collect(),
        })
    }
    #[inline]
    pub fn sin(&self, cycles: f64) -> f32 {
        // frac(cycles) in [0,1) -> index in [0,SIZE); the mask guards the 1.0-rounding edge
        self.table[(((cycles - cycles.floor()) * SIZE as f64) as usize) & MASK]
    }
    #[inline]
    pub fn cos(&self, cycles: f64) -> f32 {
        // cos(x) = sin(x + 90°): same index plus a quarter of the table, wrapped by the mask
        self.table[((((cycles - cycles.floor()) * SIZE as f64) as usize) + SIZE / 4) & MASK]
    }
}

//! Signal processing, independent of the UI and the SDR drivers.
//!
//! Data flow for analog modes (driven by `engine.rs`):
//! IQ from the source → `resampler::Channelizer` (shift + decimate to the mode's rate) →
//! `demod::Demod` (audio at 48 kHz). In WFM, `mpx::MpxDemodulator` also exposes the MPX
//! signal, which `crate::rds` decodes. `spectrum::SpectrumProcessor` works on the raw IQ.
pub mod demod;
pub mod filters;
pub mod mpx;
pub mod resampler;
pub mod sincos;
pub mod spectrum;

/// One complex IQ sample (re = I, im = Q), 32-bit floats, full scale ±1.
pub type C32 = rustfft::num_complex::Complex32;

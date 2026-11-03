//! VintHalfband -- polyphase half-band IIR, for 2x / 4x / 8x oversampling.
//! `Engine 11 - Vint/Source/VintHalfband.h` is a renamed copy of
//! `Engine 8 - Sat/Source/SatHalfband.h` (identical code apart from the namespace, diffed), so its
//! translation is the Sat one, re-exported: `sat/sat_halfband.rs`.

pub use crate::plugins::fs::sat::sat_halfband::*;

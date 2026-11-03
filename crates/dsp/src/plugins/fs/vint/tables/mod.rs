//! Vint's generated tables, converted mechanically by `cargo xtask fs-tables vint` from the C++
//! headers (never retyped):
//! - `vint_gain_table.rs` <- `VintGainTable.h` (the output stages' auto-gain, `Tests/calibrate.cpp`);
//! - `vint_british_tables.rs` <- `VintBritishTables.h` (the SSL knobs' pot tables, `Tests/britishgen.cpp`);
//! - `vint_neve_curves.rs` <- `VintNeveCurves.h` (the 1073 preamp's fast model, `Tests/prefit.cpp`).
//!
//! `VintNeveCurves.h` also declares the `StageModel` struct its aggregates fill. A struct is code,
//! not data, so it is written here by hand (field for field, in the C++ order), with the
//! `stage_model!` macro the converter's aggregate output calls: it takes the C++ initializer's
//! values positionally and gives each its field's type (`nullptr` is `None`).
//!
//! `tables_match_source` re-parses the C++ headers and checks every table's element count and a
//! checksum of its values, and every `StageModel`'s scalars, against these files.

/// A stage's fast model (`vintpre::StageModel`). Its core's curves against the input on a warped
/// axis, u = asinh (x / x0) * invA; its slow states' offsets referred to the input (`in_ref`),
/// and what they add to the output and every flow (`r_out`, `r_flow`); up to two extra table axes.
#[derive(Debug, Clone, Copy)]
pub struct StageModel {
    pub n: usize,
    pub xm: f64,
    pub x0: f64,
    pub inv_a: f64,
    pub out: &'static [f64],
    pub flow: [Option<&'static [f64]>; vint_neve_curves::K_MAX_SLOW],
    /// the capacitor's farads, the inductor's henries
    pub inertia: [f64; vint_neve_curves::K_MAX_SLOW],
    /// input-referred volts per unit of each state's offset
    pub in_ref: [f64; vint_neve_curves::K_MAX_SLOW],
    pub r_out: &'static [f64],
    pub r_flow: &'static [f64],
    /// which of rOut (bit j), rFlow (bit n + k n + j) to read input by input
    pub varies: u64,
    /// a second axis (two >= 0): that state's offsets, lo + i step
    pub two: i32,
    pub two_lo: f64,
    pub two_step: f64,
    pub two_n: i32,
    pub out2: &'static [f64],
    pub flow2: &'static [f64],
    /// a third axis (three >= 0), likewise; three_n 1 when none
    pub three: i32,
    pub three_lo: f64,
    pub three_step: f64,
    pub three_n: i32,
    /// the third axis's warp: v = asinh ((d - c) / w) (w 0: v = d)
    pub three_c: f64,
    pub three_w: f64,
}

macro_rules! stage_ptr {
    (nullptr) => {
        None
    };
    ($t:ident) => {
        Some(&$t)
    };
}

/// The C++ aggregate initializer of a `StageModel`, positionally.
macro_rules! stage_model {
    ($n:literal, $xm:expr, $x0:expr, $inv_a:expr, $out:ident, [$($flow:tt),*], [$($inertia:expr),*], [$($in_ref:expr),*],
     $r_out:ident, $r_flow:ident, $varies:literal, $two:literal, $two_lo:expr, $two_step:expr, $two_n:literal,
     $out2:ident, $flow2:ident, $three:literal, $three_lo:expr, $three_step:expr, $three_n:literal, $three_c:expr, $three_w:expr) => {
        StageModel {
            n: $n,
            xm: $xm as f64,
            x0: $x0 as f64,
            inv_a: $inv_a as f64,
            out: &$out,
            flow: [$(stage_ptr!($flow)),*],
            inertia: [$($inertia as f64),*],
            in_ref: [$($in_ref as f64),*],
            r_out: &$r_out,
            r_flow: &$r_flow,
            varies: $varies,
            two: $two,
            two_lo: $two_lo as f64,
            two_step: $two_step as f64,
            two_n: $two_n,
            out2: &$out2,
            flow2: &$flow2,
            three: $three,
            three_lo: $three_lo as f64,
            three_step: $three_step as f64,
            three_n: $three_n,
            three_c: $three_c as f64,
            three_w: $three_w as f64,
        }
    };
}

pub mod vint_british_tables;
pub mod vint_gain_table;
#[rustfmt::skip]
#[allow(clippy::all, clippy::unreadable_literal, clippy::excessive_precision)]
pub mod vint_neve_curves;

#[cfg(test)]
mod tests;

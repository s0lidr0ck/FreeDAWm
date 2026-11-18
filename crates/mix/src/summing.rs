//! Mix Engine FX (Studio Pro's console models): a bus whose summing strip (its aux or routing
//! folder) has a [`Summing`] model passes every source through the same non-linear stage as it
//! is added, so the sum has a console's or tape's character: a static waveshaper on each channel
//! of each source, a little tolerance per channel (seeded from the track id and the channel, so
//! a bounce repeats and two channels never quite match), a little crosstalk from each channel's
//! neighbours and a gentle high-frequency roll-off per channel (a summing amplifier's filter).
//! Off (the default) the bus adds every source exactly as before: no code here runs, and at drive
//! 0 neither (bit for bit the clean sum). As on a console, the stage sits after a source's send
//! level (or fader) and pan into the bus, so a hotter send drives it harder.

use crate::{MAX_CHANNELS, Matrix};
use soundcraft_model::{BusId, Route, SEND_SLOTS, Session, Summing, SummingModel, TrackId};
use std::ops::Range;

/// Whether any bus of the session has a summing model (a cheap check before any lookup).
pub fn any(s: &Session) -> bool {
    s.tracks.iter().any(|t| t.mixer.summing.is_some())
}

/// The summing model of bus `b`: its summing strip's (the track whose input it is). `None` at
/// drive 0 too: the bus then sums cleanly, bit for bit.
pub fn of_bus(s: &Session, b: BusId) -> Option<Summing> {
    s.tracks.iter().find(|t| t.mixer.input == Route::Bus(b)).and_then(|t| t.mixer.summing).filter(|sm| sm.drive.is_finite() && sm.drive > 0.0)
}

/// The summing filter's memory of one strip: per way into a bus (each send slot, then the
/// output at [`SEND_SLOTS`]) and per bus channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SumState([[f32; MAX_CHANNELS]; SEND_SLOTS + 1]);

impl Default for SumState {
    fn default() -> Self {
        SumState([[0.0; MAX_CHANNELS]; SEND_SLOTS + 1])
    }
}

impl SumState {
    /// Forget the filter's history (transport stop / locate).
    pub fn clear(&mut self) {
        *self = SumState::default();
    }

    /// The memory of way `i` (a send slot, or [`SEND_SLOTS`] for the output).
    pub fn way(&mut self, i: usize) -> Option<&mut [f32; MAX_CHANNELS]> {
        self.0.get_mut(i)
    }
}

/// The roll-off: a one-pole low-pass at this frequency (or under Nyquist), blended in by this
/// much of the drive. At full drive about -1.5 dB at 20 kHz at 48 kHz: a gentle top.
const FILTER_HZ: f32 = 16_000.0;
const FILTER_MIX: f32 = 0.5;

/// Adds frames `span` of `src` into `dst` through pan matrix `m` (`m[source][bus channel]`) and
/// `gain`, each bus channel then through model `sm`: the shaper, the crosstalk from its
/// neighbouring bus channels and the roll-off, whose memory is `mem` (one per bus channel).
/// `who` is the source track (it seeds the tolerances) and the sample rate. Allocates nothing.
#[allow(clippy::too_many_arguments)]
pub fn add_span(
    src: &[Vec<f32>],
    span: Range<usize>,
    dst: &mut [Vec<f32>],
    m: &Matrix,
    gain: f32,
    sm: Summing,
    who: (TrackId, f32),
    mem: &mut [f32; MAX_CHANNELS],
) {
    let (track, sr) = who;
    let drive = if sm.drive.is_finite() { sm.drive.clamp(0.0, 1.0) } else { 0.0 };
    let ns = src.len().min(MAX_CHANNELS);
    let nd = dst.len().min(MAX_CHANNELS);
    let mut ks = [0.0f32; MAX_CHANNELS];
    for (c, k) in ks.iter_mut().enumerate().take(nd) {
        *k = drive * (1.0 + 0.1 * tolerance(track, c));
    }
    let talk = CROSSTALK * drive;
    let sr = if sr.is_finite() && sr > 0.0 { sr } else { 48_000.0 };
    let a = 1.0 - (-std::f32::consts::TAU * FILTER_HZ.min(0.45 * sr) / sr).exp();
    let mix = FILTER_MIX * drive;
    for i in span {
        // This frame on the bus's channels, after the level and the pan.
        let mut x = [0.0f32; MAX_CHANNELS];
        for (d, v) in x.iter_mut().enumerate().take(nd) {
            let mut acc = 0.0f32;
            for (s, row) in m.iter().enumerate().take(ns) {
                let g = row.get(d).copied().unwrap_or(0.0);
                if g != 0.0 {
                    acc += src.get(s).and_then(|c| c.get(i)).copied().unwrap_or(0.0) * gain * g;
                }
            }
            *v = acc;
        }
        for (d, out) in dst.iter_mut().enumerate().take(nd) {
            let here = x.get(d).copied().unwrap_or(0.0);
            let leak =
                d.checked_sub(1).and_then(|p| x.get(p)).copied().unwrap_or(0.0) + if d + 1 < nd { x.get(d + 1).copied().unwrap_or(0.0) } else { 0.0 };
            let y = shape(sm.model, ks.get(d).copied().unwrap_or(0.0), here) + talk * leak;
            let Some(lp) = mem.get_mut(d) else { continue };
            *lp += a * (y - *lp);
            if !lp.is_finite() {
                *lp = 0.0;
            }
            let y = y + mix * (*lp - y);
            if let Some(o) = out.get_mut(i) {
                *o += y;
            }
        }
    }
}

/// A channel's tolerance, -1 to 1, from the track and channel (splitmix64).
fn tolerance(track: TrackId, channel: usize) -> f32 {
    let mut z = ((track.0 << 8) ^ channel as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// One sample through the model at drive `k` (0 to about 1.1, the tolerance included). At 0 it
/// is the sample itself; it never grows past the clean sample's level and keeps its sign.
pub fn shape(model: SummingModel, k: f32, x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    match model {
        // Toward tanh: odd harmonics that grow with the level.
        SummingModel::Console => x + 0.35 * k * (x.tanh() - x),
        // A rounder, earlier bend.
        SummingModel::Tape => x + 0.6 * k * ((1.8 * x).tanh() / 1.8 - x),
    }
}

/// How much of each neighbouring channel leaks in at full drive (about -46 dB).
const CROSSTALK: f32 = 0.005;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shape_is_clean_at_no_drive_and_bends_loud_samples() {
        for m in SummingModel::ALL {
            assert_eq!(shape(m, 0.0, 0.7), 0.7);
            let loud = shape(m, 1.0, 0.9);
            assert!(loud < 0.9 && loud > 0.5, "{m:?}: {loud}");
            assert!((shape(m, 1.0, 0.01) - 0.01).abs() < 1e-4, "quiet passes nearly clean");
            assert_eq!(shape(m, 1.0, -0.9), -loud, "odd: keeps its sign");
            assert_eq!(shape(m, 1.0, f32::NAN), 0.0);
        }
        assert!(shape(SummingModel::Tape, 1.0, 0.9) < shape(SummingModel::Console, 1.0, 0.9), "tape bends sooner");
    }

    /// One mono span through the model at full drive, the identity pan, as `sr` hears it.
    fn through(model: SummingModel, drive: f32, x: &[f32], sr: f32) -> Vec<f32> {
        let mut m = [[0.0f32; MAX_CHANNELS]; MAX_CHANNELS];
        m[0][0] = 1.0;
        let mut out = vec![vec![0.0f32; x.len()]];
        let mut mem = [0.0f32; MAX_CHANNELS];
        add_span(&[x.to_vec()], 0..x.len(), &mut out, &m, 1.0, Summing { model, drive }, (TrackId(3), sr), &mut mem);
        out.remove(0)
    }

    #[test]
    fn the_filter_rolls_off_the_top_gently_and_leaves_the_low_end() {
        let sine = |hz: f32| (0..9_600).map(|i| 0.01 * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin()).collect::<Vec<f32>>();
        let rms = |v: &[f32]| (v.iter().skip(480).map(|x| x * x).sum::<f32>() / (v.len() - 480) as f32).sqrt();
        let db = |hz: f32| {
            let x = sine(hz);
            20.0 * (rms(&through(SummingModel::Console, 1.0, &x, 48_000.0)) / rms(&x)).log10()
        };
        let (low, high) = (db(100.0), db(18_000.0));
        assert!(low.abs() < 0.05, "the low end passes: {low} dB");
        assert!((-3.0..-0.5).contains(&high), "the top rolls off gently: {high} dB");
        let half = {
            let x = sine(18_000.0);
            20.0 * (rms(&through(SummingModel::Console, 0.5, &x, 48_000.0)) / rms(&x)).log10()
        };
        assert!(half > high && half < -0.2, "less drive, less roll-off: {half} dB");
    }

    #[test]
    fn a_hotter_span_bends_more() {
        let x: Vec<f32> = (0..4_800).map(|i| (i as f32 * 0.01).sin()).collect();
        let rel = |g: f32| {
            let y: Vec<f32> = x.iter().map(|v| v * g).collect();
            let out = through(SummingModel::Tape, 1.0, &y, 48_000.0);
            out.iter().zip(&y).map(|(o, c)| (o - c).abs()).fold(0.0f32, f32::max) / g
        };
        assert!(rel(1.0) > 10.0 * rel(0.1), "{} vs {}", rel(1.0), rel(0.1));
    }

    #[test]
    fn each_channel_differs_a_little_and_repeats() {
        let a = tolerance(TrackId(7), 0);
        assert_eq!(a, tolerance(TrackId(7), 0));
        assert_ne!(a, tolerance(TrackId(7), 1));
        assert_ne!(a, tolerance(TrackId(8), 0));
        for t in 0..200 {
            let v = tolerance(TrackId(t), t as usize % 3);
            assert!((-1.0..1.0).contains(&v), "{v}");
        }
    }
}

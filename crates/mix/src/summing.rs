//! Mix Engine FX (Studio Pro's console models): a bus whose summing strip (its aux or routing
//! folder) has a [`Summing`] model passes every source through the same non-linear stage as it
//! is added, so the sum has a console's or tape's character: a static waveshaper on each channel
//! of each source, a little tolerance per channel (seeded from the track id and the channel, so
//! a bounce repeats and two channels never quite match) and a little crosstalk from each
//! channel's neighbours. Off (the default) the bus adds every source exactly as before: no code
//! here runs. The stage sits on a source's output as it enters the bus, before the send level
//! and the pan into the bus.

use soundcraft_model::{BusId, Route, Session, Summing, SummingModel, TrackId};

/// Whether any bus of the session has a summing model (a cheap check before any lookup).
pub fn any(s: &Session) -> bool {
    s.tracks.iter().any(|t| t.mixer.summing.is_some())
}

/// The summing model of bus `b`: its summing strip's (the track whose input it is).
pub fn of_bus(s: &Session, b: BusId) -> Option<Summing> {
    s.tracks.iter().find(|t| t.mixer.input == Route::Bus(b)).and_then(|t| t.mixer.summing)
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

/// `src` (a strip's channels, `frames` long) as bus model `sm` makes it on its way in, written
/// into `out` (sized here: allocates only the first time, or when the strip's width changes).
pub fn shape_into(src: &[Vec<f32>], out: &mut Vec<Vec<f32>>, sm: Summing, track: TrackId, frames: usize) {
    if out.len() != src.len() {
        out.resize_with(src.len(), Vec::new);
    }
    let drive = if sm.drive.is_finite() { sm.drive.clamp(0.0, 1.0) } else { 0.0 };
    let n = src.len();
    for (c, o) in out.iter_mut().enumerate() {
        o.clear();
        let Some(x) = src.get(c) else { continue };
        let frames = frames.min(x.len());
        let k = drive * (1.0 + 0.1 * tolerance(track, c));
        let talk = CROSSTALK * drive;
        let (prev, next) = (c.checked_sub(1).and_then(|p| src.get(p)), if c + 1 < n { src.get(c + 1) } else { None });
        o.extend((0..frames).map(|i| {
            let leak = prev.and_then(|p| p.get(i)).copied().unwrap_or(0.0) + next.and_then(|q| q.get(i)).copied().unwrap_or(0.0);
            shape(sm.model, k, x[i]) + talk * leak
        }));
    }
}

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

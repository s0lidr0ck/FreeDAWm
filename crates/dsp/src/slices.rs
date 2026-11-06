//! Where to cut a sample into slices, for Lift's Slice mode: at its hits (onsets), on a beat
//! grid, or into equal parts. Positions are frames, ascending, the first always 0.

/// Most slices a sample is cut into (the Sampler's `slice_count` maximum).
pub const MAX_SLICES: usize = 64;

/// Analysis hop in frames at 48 kHz (scaled with the rate): about 5 ms.
const HOP_48K: f32 = 256.0;

/// The frames where `x` (mono, at `sample_rate`) has a hit, the first slice at 0. `sensitivity`
/// 0..100: higher finds quieter hits. Hits closer than 60 ms to the last are skipped, and
/// nothing quieter than -40 dB of the loudest part counts.
pub fn hits(x: &[f32], sample_rate: f32, sensitivity: f32) -> Vec<usize> {
    let sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
    let hop = ((HOP_48K * sr / 48_000.0) as usize).clamp(32, 4096);
    let frames = x.len() / hop;
    let mut out = vec![0usize];
    if frames < 3 {
        return out;
    }
    // Energy per hop.
    let energy: Vec<f32> = (0..frames)
        .map(|i| {
            let a = i * hop;
            x.get(a..a + hop).map_or(0.0, |w| w.iter().map(|v| if v.is_finite() { v * v } else { 0.0 }).sum::<f32>() / hop as f32)
        })
        .collect();
    let peak = energy.iter().copied().fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return out;
    }
    let floor = peak * 1e-4; // -40 dB in energy
    let s = if sensitivity.is_finite() { (sensitivity / 100.0).clamp(0.0, 1.0) } else { 0.5 };
    // The rise (in log energy) a hit needs over the quietest of the hops just before it.
    let need = 3.0 - 2.6 * s;
    let gap = ((0.06 * sr) as usize / hop).max(1);
    let mut last = 0usize;
    for i in 1..frames {
        let e = energy.get(i).copied().unwrap_or(0.0);
        if e < floor {
            continue;
        }
        let before = energy.get(i.saturating_sub(4)..i).map_or(0.0, |w| w.iter().copied().fold(f32::INFINITY, f32::min));
        let rise = (e + 1e-12).ln() - (before + 1e-12).ln();
        // The hit is where the rise peaks: not still rising into the next hop.
        let next = energy.get(i + 1).copied().unwrap_or(0.0);
        let still = (next + 1e-12).ln() - (before + 1e-12).ln() > rise && next > e * 1.5;
        if rise > need && !still && i - last >= gap {
            let at = i.saturating_sub(1) * hop;
            if at > 0 {
                out.push(at);
                last = i;
                if out.len() == MAX_SLICES {
                    break;
                }
            }
        }
    }
    out
}

/// Slices every `1/per_beat` of a beat at `bpm` across `len` frames.
pub fn grid(len: usize, sample_rate: f32, bpm: f64, per_beat: u32) -> Vec<usize> {
    let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
    let bpm = if bpm.is_finite() && bpm > 0.0 { bpm.clamp(20.0, 400.0) } else { 120.0 };
    let step = sr * 60.0 / bpm / f64::from(per_beat.clamp(1, 16));
    let mut out = Vec::new();
    let mut at = 0.0f64;
    while (at as usize) < len && out.len() < MAX_SLICES {
        out.push(at as usize);
        at += step;
    }
    if out.is_empty() {
        out.push(0);
    }
    out
}

/// `count` equal slices across `len` frames.
pub fn equal(len: usize, count: usize) -> Vec<usize> {
    let n = count.clamp(1, MAX_SLICES);
    (0..n).map(|i| len.saturating_mul(i) / n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clicks(at: &[usize], len: usize) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        for (k, a) in at.iter().enumerate() {
            let gain = if k % 2 == 0 { 0.9 } else { 0.3 };
            for i in 0..2400 {
                if let Some(v) = x.get_mut(a + i) {
                    *v = gain * (-(i as f32) / 300.0).exp() * ((i as f32) * 0.3).sin();
                }
            }
        }
        x
    }

    #[test]
    fn hits_land_on_the_hits() {
        let at = [0, 12_000, 24_000, 30_000, 36_000];
        let found = hits(&clicks(&at, 48_000), 48_000.0, 50.0);
        assert_eq!(found.len(), at.len(), "{found:?}");
        for (f, a) in found.iter().zip(at) {
            assert!(f.abs_diff(a) <= 512, "{f} vs {a}");
        }
    }

    #[test]
    fn sensitivity_and_silence() {
        // A quiet hit after a loud one: found when sensitive.
        let mut x = clicks(&[0], 48_000);
        for i in 0..2400 {
            if let Some(v) = x.get_mut(24_000 + i) {
                *v = 0.05 * (-(i as f32) / 300.0).exp() * ((i as f32) * 0.3).sin();
            }
        }
        assert_eq!(hits(&x, 48_000.0, 100.0).len(), 2);
        assert_eq!(hits(&vec![0.0; 48_000], 48_000.0, 50.0), vec![0]);
        assert_eq!(hits(&[], f32::NAN, f32::NAN), vec![0]);
        assert_eq!(hits(&vec![f32::NAN; 9000], 48_000.0, 50.0), vec![0]);
    }

    #[test]
    fn grid_and_equal() {
        // Two bars of 120 bpm in eighths: 16 slices, a quarter second apart.
        let g = grid(192_000, 48_000.0, 120.0, 2);
        assert_eq!(g.len(), 16);
        assert_eq!(g.get(1), Some(&12_000));
        assert_eq!(grid(10_000_000, 48_000.0, 120.0, 16).len(), MAX_SLICES);
        assert_eq!(grid(0, 48_000.0, f64::NAN, 0), vec![0]);
        assert_eq!(equal(1000, 4), vec![0, 250, 500, 750]);
        assert_eq!(equal(1000, 0), vec![0]);
        assert_eq!(equal(10, 999).len(), MAX_SLICES);
    }
}

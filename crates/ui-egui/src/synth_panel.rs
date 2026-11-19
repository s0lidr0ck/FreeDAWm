//! Drawing helpers the synth panels share: a filter's response (the synths' state-variable
//! filter) and an ADSR drawn to its times.

/// The Subtractive Synth's plugin id.
pub const SYNTH: &str = "subtractive_synth";

/// The filter's gain in dB at `hz` for cutoff `fc`, damping `k` (the synth's: 2 − 1.95 ×
/// resonance) and type `kind` (0 and 1 low-pass 12 and 24 dB, 2 high-pass, 3 band-pass, 4
/// notch): the analogue state-variable filter the synth's TPT filter is drawn from.
pub fn response_db(hz: f32, fc: f32, k: f32, kind: usize) -> f32 {
    let w = f64::from(hz / fc.max(1.0));
    let k = f64::from(k);
    // H(s) = N(s) / (s² + k s + 1), s = jw.
    let (den_re, den_im) = (1.0 - w * w, k * w);
    let den = (den_re * den_re + den_im * den_im).sqrt().max(1e-12);
    let num = match kind {
        2 => w * w,
        3 => w,
        4 => (1.0 - w * w).abs(),
        _ => 1.0,
    };
    let mut g = num / den;
    if kind == 1 {
        g *= g;
    }
    (20.0 * g.max(1e-9).log10()) as f32
}

/// The points of an ADSR drawn over `width` seconds: attack (linear), decay and release
/// (exponential, their times to -60 dB), and the sustain held for a quarter of the drawing.
pub fn envelope_points(a_ms: f32, d_ms: f32, sustain: f32, r_ms: f32) -> Vec<(f32, f32)> {
    let (a, d, r) = (a_ms.max(0.0) / 1000.0, d_ms.max(1.0) / 1000.0, r_ms.max(1.0) / 1000.0);
    let s = sustain.clamp(0.0, 1.0);
    let hold = ((a + d + r) * 0.25).max(0.05);
    let mut pts = vec![(0.0, 0.0), (a, 1.0)];
    let curve = |t0: f32, len: f32, from: f32, to: f32, pts: &mut Vec<(f32, f32)>| {
        for i in 1..=24 {
            let x = i as f32 / 24.0;
            // To -60 dB over `len`: e^(ln(0.001) · x).
            let e = (0.001f32.ln() * x).exp();
            pts.push((t0 + len * x, to + (from - to) * e));
        }
    };
    curve(a, d, 1.0, s, &mut pts);
    let release_from = pts.last().map_or(s, |p| p.1);
    pts.push((a + d + hold, release_from));
    curve(a + d + hold, r, release_from, 0.0, &mut pts);
    pts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_filter_type_has_its_shape() {
        let k = 2.0;
        // Low-pass: flat below, -12 dB an octave above; 24 dB twice that.
        assert!(response_db(100.0, 1000.0, k, 0).abs() < 0.5);
        assert!((response_db(8000.0, 1000.0, k, 0) + 36.0).abs() < 2.0);
        assert!((response_db(8000.0, 1000.0, k, 1) - 2.0 * response_db(8000.0, 1000.0, k, 0)).abs() < 0.01);
        assert!(response_db(100.0, 1000.0, k, 2) < -30.0 && response_db(10_000.0, 1000.0, k, 2).abs() < 0.5);
        assert!(response_db(1000.0, 1000.0, k, 3) > response_db(50.0, 1000.0, k, 3) + 15.0);
        assert!(response_db(1000.0, 1000.0, k, 4) < -60.0);
        // Resonance raises the peak at the cutoff.
        assert!(response_db(1000.0, 1000.0, 0.05, 0) > 20.0);
    }

    #[test]
    fn an_envelope_is_drawn_to_its_times() {
        let p = envelope_points(100.0, 200.0, 0.5, 400.0);
        assert_eq!(p[1], (0.1, 1.0));
        let at_decay_end = p.iter().find(|q| (q.0 - 0.3).abs() < 1e-4).map(|q| q.1).unwrap_or(-1.0);
        assert!((at_decay_end - 0.5).abs() < 0.01, "{at_decay_end}");
        let last = p.last().copied().unwrap_or_default();
        assert!(last.1 < 0.01 && last.0 > 0.7);
        assert!(p.windows(2).all(|w| w[1].0 >= w[0].0));
    }
}

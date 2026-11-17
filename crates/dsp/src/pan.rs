//! Mono-to-stereo pan laws and the surround (multichannel) panner.

use std::f32::consts::FRAC_PI_4;

/// Attenuation of a centered signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum PanLaw {
    /// Constant power: -3 dB at center.
    #[default]
    Minus3,
    /// Compromise: -4.5 dB at center.
    Minus4_5,
    /// Constant voltage: -6 dB at center.
    Minus6,
    /// Balance: 0 dB at center, the far side fades out linearly.
    Zero,
}

impl PanLaw {
    /// Center attenuation in dB (negative).
    pub fn center_db(self) -> f32 {
        match self {
            PanLaw::Minus3 => -3.0,
            PanLaw::Minus4_5 => -4.5,
            PanLaw::Minus6 => -6.0,
            PanLaw::Zero => 0.0,
        }
    }
}

/// Left/right gains for `pan` in -1 (hard left) ..= 1 (hard right). NaN pans center.
pub fn gains(pan: f32, law: PanLaw) -> (f32, f32) {
    let pan = if pan.is_nan() { 0.0 } else { pan.clamp(-1.0, 1.0) };
    if law == PanLaw::Zero {
        return ((1.0 - pan).min(1.0), (1.0 + pan).min(1.0));
    }
    let theta = (pan + 1.0) * FRAC_PI_4;
    let (l, r) = (theta.cos().max(0.0), theta.sin().max(0.0));
    // cos(pi/4) = -3.0103 dB; raise to a power to reach the requested center level.
    let exp = law.center_db() / -3.010_3;
    let l = if pan <= -1.0 { 1.0 } else { l.powf(exp) };
    let r = if pan >= 1.0 { 1.0 } else { r.powf(exp) };
    (l, r)
}

/// Most loudspeakers the surround panner handles (9.1.6 = 16).
pub const MAX_SPEAKERS: usize = 16;

/// A loudspeaker direction for the surround panner: azimuth in degrees (0 = front, positive =
/// right), elevation in degrees (> 0 = height layer), and whether it is the LFE (never panned to).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SpeakerPos {
    pub az: f32,
    pub el: f32,
    pub lfe: bool,
}

/// Surround panner controls (see `soundcraft_model::SurroundPan`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurroundParams {
    /// -1 (left) ..= 1 (right).
    pub x: f32,
    /// -1 (back) ..= 1 (front).
    pub y: f32,
    /// 0 (ear level) ..= 1 (overhead).
    pub z: f32,
    /// 0 = point source ..= 1 = every speaker.
    pub divergence: f32,
    /// 0..=1 share of centre-speaker energy kept on C (the rest goes phantom to L/R).
    pub center: f32,
}

fn fin(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

fn wrap180(a: f32) -> f32 {
    let mut a = fin(a, 0.0) % 360.0;
    if a > 180.0 {
        a -= 360.0;
    } else if a <= -180.0 {
        a += 360.0;
    }
    a
}

/// Azimuth (degrees) of a square puck position. The square's front corners land on the front
/// L/R speakers (±30°), its side midpoints at ±90° and its rear corners on the layout's rear-most
/// surround (±110° for 5.x, ±150° for 7.x), so a puck in a corner hits exactly one speaker.
pub fn puck_azimuth(layout: &[SpeakerPos], x: f32, y: f32) -> f32 {
    let (x, y) = (fin(x, 0.0).clamp(-1.0, 1.0), fin(y, 1.0).clamp(-1.0, 1.0));
    if x.abs() < 1e-9 && y.abs() < 1e-9 {
        return 0.0;
    }
    let rear =
        layout.iter().filter(|s| !s.lfe && s.el.abs() < 1.0 && s.az.abs() > 90.5 && s.az.abs() < 179.5).map(|s| s.az.abs()).fold(0.0f32, f32::max);
    let rear = if rear > 0.0 { rear } else { 135.0 };
    let front = if layout.iter().any(|s| !s.lfe && s.el.abs() < 1.0 && (s.az.abs() - 30.0).abs() < 0.5) { 30.0 } else { 45.0 };
    let phi = x.atan2(y).to_degrees();
    let a = phi.abs();
    let warped = if a <= 45.0 {
        a / 45.0 * front
    } else if a <= 90.0 {
        front + (a - 45.0) / 45.0 * (90.0 - front)
    } else if a <= 135.0 {
        90.0 + (a - 90.0) / 45.0 * (rear - 90.0)
    } else {
        rear + (a - 135.0) / 45.0 * (180.0 - rear)
    };
    warped.copysign(phi)
}

/// Inverse of [`puck_azimuth`] on the square's edge (for drawing speakers on a square panner).
pub fn azimuth_to_puck(layout: &[SpeakerPos], az: f32) -> (f32, f32) {
    // Bisect the monotonic warp on the square angle.
    let target = wrap180(az);
    let (mut lo, mut hi) = (0.0f32, 180.0f32);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        let r = mid.to_radians();
        let w = puck_azimuth(layout, r.sin(), r.cos());
        if w < target.abs() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let phi = (0.5 * (lo + hi)).to_radians().copysign(if target < 0.0 { -1.0 } else { 1.0 });
    let (sx, sy) = (phi.sin(), phi.cos());
    let m = sx.abs().max(sy.abs()).max(1e-6);
    (sx / m, sy / m)
}

/// Constant-power pairwise pan of a direction onto one layer (bed or height) of `layout`,
/// accumulating `weight × gain²` into `pow`. Returns false when the layer has no speakers.
fn pair_power(layout: &[SpeakerPos], height: bool, az: f32, weight: f32, pow: &mut [f32; MAX_SPEAKERS]) -> bool {
    let mut idx = [0usize; MAX_SPEAKERS];
    let mut n = 0;
    for (i, s) in layout.iter().enumerate().take(MAX_SPEAKERS) {
        if !s.lfe
            && (s.el > 1.0) == height
            && let Some(slot) = idx.get_mut(n)
        {
            *slot = i;
            n += 1;
        }
    }
    let ids = idx.get(..n).unwrap_or(&[]);
    let az_of = |i: usize| layout.get(i).map_or(0.0, |s| wrap180(s.az));
    match ids {
        [] => false,
        [only] => {
            if let Some(p) = pow.get_mut(*only) {
                *p += weight;
            }
            true
        }
        [first, ..] => {
            let a = wrap180(az);
            // The nearest speaker clockwise (to the right, increasing azimuth) and anticlockwise.
            let (mut cw, mut cw_d) = (*first, f32::MAX);
            let (mut acw, mut acw_d) = (*first, f32::MAX);
            for &i in ids {
                let d = (az_of(i) - a).rem_euclid(360.0);
                if d < cw_d {
                    cw_d = d;
                    cw = i;
                }
                let d2 = (a - az_of(i)).rem_euclid(360.0);
                if d2 < acw_d {
                    acw_d = d2;
                    acw = i;
                }
            }
            if cw == acw || cw_d < 1e-4 {
                if let Some(p) = pow.get_mut(cw) {
                    *p += weight;
                }
                return true;
            }
            let span = (cw_d + acw_d).max(1e-6);
            let t = (acw_d / span).clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
            if let Some(p) = pow.get_mut(acw) {
                *p += weight * t.cos() * t.cos();
            }
            if let Some(p) = pow.get_mut(cw) {
                *p += weight * t.sin() * t.sin();
            }
            true
        }
    }
}

/// Point-source gains for a direction (azimuth, elevation in degrees) on `layout`: pairwise
/// constant power on the speaker layer nearest in elevation; a height direction on a layout
/// without heights folds to the bed at -3 dB. Writes `out[..layout.len()]`.
pub fn direction_gains(layout: &[SpeakerPos], az: f32, el: f32, out: &mut [f32]) {
    let mut pow = [0.0f32; MAX_SPEAKERS];
    let height = fin(el, 0.0) > 1.0;
    if height && !pair_power(layout, true, az, 1.0, &mut pow) {
        pair_power(layout, false, az, 0.5, &mut pow);
    } else if !height {
        pair_power(layout, false, az, 1.0, &mut pow);
    }
    for (o, p) in out.iter_mut().zip(pow.iter()) {
        *o = p.max(0.0).sqrt();
    }
}

/// Surround panner: gains for every speaker of `layout` (LFE gets 0; the caller adds an LFE send).
/// Energy-preserving: the squared gains of the non-LFE speakers always sum to 1.
///
/// The puck's azimuth comes from [`puck_azimuth`]. Divergence (and moving the puck towards the
/// middle of the room) spreads the image: the gains are the power average of point sources
/// sampled across `±spread × 180°`. `center` < 1 moves that share of the centre speaker's power
/// to a phantom centre on L/R. Elevation crossfades bed and height layers (constant power).
pub fn surround_gains(layout: &[SpeakerPos], p: &SurroundParams, out: &mut [f32]) {
    out.iter_mut().for_each(|g| *g = 0.0);
    let layout = layout.get(..layout.len().min(MAX_SPEAKERS)).unwrap_or(&[]);
    if layout.is_empty() {
        return;
    }
    let az = puck_azimuth(layout, p.x, p.y);
    let dist = fin(p.x, 0.0).abs().max(fin(p.y, 1.0).abs()).clamp(0.0, 1.0);
    let div = fin(p.divergence, 0.0).clamp(0.0, 1.0);
    let spread = 1.0 - dist * (1.0 - div);
    let k = 1 + (spread * 24.0).round() as usize;
    let width = spread * 180.0;
    let has_height = layout.iter().any(|s| !s.lfe && s.el > 1.0);
    let z = if has_height { fin(p.z, 0.0).clamp(0.0, 1.0) } else { 0.0 };
    let (bed_w, top_w) = {
        let a = z * std::f32::consts::FRAC_PI_2;
        (a.cos() * a.cos(), a.sin() * a.sin())
    };
    let mut pow = [0.0f32; MAX_SPEAKERS];
    for i in 0..k {
        let a = if k == 1 { az } else { az - width + 2.0 * width * (i as f32 + 0.5) / k as f32 };
        let w = 1.0 / k as f32;
        if bed_w > 1e-9 && !pair_power(layout, false, a, w * bed_w, &mut pow) {
            pair_power(layout, true, a, w * bed_w, &mut pow);
        }
        if top_w > 1e-9 {
            pair_power(layout, true, a, w * top_w, &mut pow);
        }
    }
    // Centre %: move energy from C to a phantom centre on L/R.
    let find = |az: f32| layout.iter().position(|s| !s.lfe && s.el.abs() < 1.0 && (s.az - az).abs() < 0.5);
    let c_share = fin(p.center, 1.0).clamp(0.0, 1.0);
    if let (Some(c), Some(l), Some(r)) = (find(0.0), find(-30.0), find(30.0))
        && c_share < 1.0
    {
        let pc = pow.get(c).copied().unwrap_or(0.0);
        let moved = pc * (1.0 - c_share);
        if let Some(x) = pow.get_mut(c) {
            *x = pc - moved;
        }
        for i in [l, r] {
            if let Some(x) = pow.get_mut(i) {
                *x += 0.5 * moved;
            }
        }
    }
    for (o, p) in out.iter_mut().zip(pow.iter()) {
        *o = p.max(0.0).sqrt();
    }
}

/// Highest Ambisonics order [`ambisonic_encode`] fills.
pub const MAX_AMBISONIC_ORDER: usize = 7;

/// Ambisonics encoding gains (ACN channel order, SN3D, no Condon-Shortley phase) for a direction,
/// up to the order `out` holds ((n+1)² channels; 7th order at most, channels past it get 0).
/// Components of order `l` are scaled by `(1 - spread)^l`, so a spread of 1 leaves only W.
// The Legendre recurrences read neighbouring rows and columns by index: clearer as index loops.
#[allow(clippy::needless_range_loop)]
pub fn ambisonic_encode(az: f32, el: f32, spread: f32, out: &mut [f32]) {
    out.iter_mut().for_each(|g| *g = 0.0);
    const N: usize = MAX_AMBISONIC_ORDER + 1;
    // Ambisonic azimuth is anticlockwise (left positive); ours is clockwise.
    let a = -f64::from(fin(az, 0.0)).to_radians();
    let e = f64::from(fin(el, 0.0)).to_radians();
    let d = 1.0 - f64::from(fin(spread, 0.0).clamp(0.0, 1.0));
    let (x, c) = (e.sin(), e.cos());
    // Associated Legendre functions P_l^m(sin el), without the Condon-Shortley phase.
    let mut p = [[0.0f64; N]; N];
    p[0][0] = 1.0;
    for m in 1..N {
        p[m][m] = (2 * m - 1) as f64 * c * p[m - 1][m - 1];
    }
    for m in 0..N - 1 {
        p[m + 1][m] = (2 * m + 1) as f64 * x * p[m][m];
    }
    for m in 0..N {
        for l in m + 2..N {
            p[l][m] = ((2 * l - 1) as f64 * x * p[l - 1][m] - (l + m - 1) as f64 * p[l - 2][m]) / (l - m) as f64;
        }
    }
    // (l - m)! / (l + m)! for the SN3D normalisation.
    let ratio = |l: usize, m: usize| ((l - m + 1)..=(l + m)).fold(1.0f64, |r, k| r / k as f64);
    for l in 0..N {
        let dl = d.powi(l as i32);
        for m in 0..=l {
            let norm = if m == 0 { 1.0 } else { (2.0 * ratio(l, m)).sqrt() };
            let base = norm * p[l][m] * dl;
            let mf = m as f64;
            // ACN = l² + l + m, with m negative for the sine terms.
            if let Some(o) = out.get_mut(l * l + l + m) {
                *o = (base * (mf * a).cos()) as f32;
            }
            if m > 0
                && let Some(o) = out.get_mut(l * l + l - m)
            {
                *o = (base * (mf * a).sin()) as f32;
            }
        }
    }
}

/// Basic first-order Ambisonics decode gains (W, Y, Z, X) for one speaker of an `n`-speaker layout.
pub fn ambisonic_decode(az: f32, el: f32, n: usize) -> [f32; 4] {
    let (a, e) = (fin(az, 0.0).to_radians(), fin(el, 0.0).to_radians());
    let k = 1.0 / n.max(1) as f32;
    [k, 3.0 * k * -a.sin() * e.cos(), 3.0 * k * e.sin(), 3.0 * k * a.cos() * e.cos()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gain_to_db;

    #[test]
    fn ambisonic_encode_fills_every_order_in_sn3d() {
        let mut g = [0.0f32; 64];
        for (az, el) in [(0.0f32, 0.0f32), (37.0, 12.0), (-120.0, -40.0), (90.0, 75.0), (180.0, 0.0)] {
            ambisonic_encode(az, el, 0.0, &mut g);
            // SN3D: the components of each order have unit energy in every direction.
            for l in 0..=MAX_AMBISONIC_ORDER {
                let e: f32 = g[l * l..(l + 1) * (l + 1)].iter().map(|v| v * v).sum();
                assert!((e - 1.0).abs() < 1e-4, "order {l} at ({az}, {el}): {e}");
            }
            // Closed forms for orders 1 to 3 (ambisonic azimuth anticlockwise).
            let (a, e) = (-az.to_radians(), el.to_radians());
            let (s, c) = (e.sin(), e.cos());
            let want = [
                (1, a.sin() * c),
                (2, s),
                (3, a.cos() * c),
                (4, 3f32.sqrt() / 2.0 * c * c * (2.0 * a).sin()),
                (6, 0.5 * (3.0 * s * s - 1.0)),
                (8, 3f32.sqrt() / 2.0 * c * c * (2.0 * a).cos()),
                (9, (5.0f32 / 8.0).sqrt() * c * c * c * (3.0 * a).sin()),
                (12, 0.5 * s * (5.0 * s * s - 3.0)),
                (15, (5.0f32 / 8.0).sqrt() * c * c * c * (3.0 * a).cos()),
            ];
            for (i, v) in want {
                assert!((g[i] - v).abs() < 1e-4, "ACN {i} at ({az}, {el}): {} vs {v}", g[i]);
            }
        }
        // A 2nd-order output is filled to its last channel; spread 1 leaves only W.
        let mut g9 = [0.0f32; 9];
        ambisonic_encode(30.0, 20.0, 0.0, &mut g9);
        assert!(g9[4..].iter().any(|v| v.abs() > 0.1), "{g9:?}");
        ambisonic_encode(30.0, 20.0, 1.0, &mut g9);
        assert_eq!(g9[0], 1.0);
        assert!(g9[1..].iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn center_levels() {
        for law in [PanLaw::Minus3, PanLaw::Minus4_5, PanLaw::Minus6, PanLaw::Zero] {
            let (l, r) = gains(0.0, law);
            assert!((gain_to_db(l) - law.center_db()).abs() < 0.02, "{law:?}");
            assert!((l - r).abs() < 1e-6);
            let (l, r) = gains(-1.0, law);
            assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
        }
    }

    fn layout_51() -> Vec<SpeakerPos> {
        [(-30.0, false), (30.0, false), (0.0, false), (0.0, true), (-110.0, false), (110.0, false)]
            .iter()
            .map(|&(az, lfe)| SpeakerPos { az, el: 0.0, lfe })
            .collect()
    }

    fn layout_714() -> Vec<SpeakerPos> {
        let mut v: Vec<SpeakerPos> =
            [-30.0, 30.0, 0.0, 0.0, -150.0, 150.0, -90.0, 90.0].iter().enumerate().map(|(i, &az)| SpeakerPos { az, el: 0.0, lfe: i == 3 }).collect();
        for az in [-45.0, 45.0, -135.0, 135.0] {
            v.push(SpeakerPos { az, el: 45.0, lfe: false });
        }
        v
    }

    fn params(x: f32, y: f32) -> SurroundParams {
        SurroundParams { x, y, z: 0.0, divergence: 0.0, center: 1.0 }
    }

    #[test]
    fn surround_energy_is_constant() {
        for layout in [layout_51(), layout_714()] {
            let mut g = [0.0f32; MAX_SPEAKERS];
            for xi in -4..=4 {
                for yi in -4..=4 {
                    for div in [0.0, 0.3, 1.0] {
                        for z in [0.0, 0.5, 1.0] {
                            for center in [0.0, 0.5, 1.0] {
                                let p = SurroundParams { x: xi as f32 / 4.0, y: yi as f32 / 4.0, z, divergence: div, center };
                                surround_gains(&layout, &p, &mut g);
                                let e: f32 = g.iter().zip(layout.iter()).filter(|(_, s)| !s.lfe).map(|(x, _)| x * x).sum();
                                assert!((e - 1.0).abs() < 1e-4, "energy {e} at {p:?}");
                                assert!(g.iter().zip(layout.iter()).all(|(x, s)| !s.lfe || *x == 0.0), "LFE is never panned to");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn surround_point_positions() {
        let l = layout_51();
        let mut g = [0.0f32; MAX_SPEAKERS];
        // Front centre with no divergence: C only.
        surround_gains(&l, &params(0.0, 1.0), &mut g);
        assert!((g[2] - 1.0).abs() < 1e-5 && g.iter().enumerate().all(|(i, x)| i == 2 || x.abs() < 1e-3), "{g:?}");
        // Divergence spreads the centre image.
        surround_gains(&l, &SurroundParams { divergence: 0.5, ..params(0.0, 1.0) }, &mut g);
        assert!(g[2] < 0.99 && g[0] > 0.05 && g[1] > 0.05);
        // Full left (front-left corner): L only.
        surround_gains(&l, &params(-1.0, 1.0), &mut g);
        assert!((g[0] - 1.0).abs() < 1e-5 && g.iter().enumerate().all(|(i, x)| i == 0 || x.abs() < 1e-3), "{g:?}");
        // Rear-right corner: Rs only.
        surround_gains(&l, &params(1.0, -1.0), &mut g);
        assert!((g[5] - 1.0).abs() < 1e-5, "{g:?}");
        // Phantom centre (centre 0 %): L and R at -3 dB, no C.
        surround_gains(&l, &SurroundParams { center: 0.0, ..params(0.0, 1.0) }, &mut g);
        assert!(g[2].abs() < 1e-6 && (g[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        // Overhead on 7.1.4: heights only.
        let l = layout_714();
        surround_gains(&l, &SurroundParams { z: 1.0, ..params(-1.0, 1.0) }, &mut g);
        let top: f32 = g[8..12].iter().map(|x| x * x).sum();
        assert!((top - 1.0).abs() < 1e-4 && g[8] > 0.9, "{g:?}");
        // Corner positions map onto speaker angles and back.
        let (x, y) = azimuth_to_puck(&l, -150.0);
        assert!((x + 1.0).abs() < 1e-3 && (y + 1.0).abs() < 1e-3, "{x} {y}");
        assert!((puck_azimuth(&l, 0.0, -1.0).abs() - 180.0).abs() < 1e-3);
        // Hostile input never produces NaN.
        surround_gains(&l, &SurroundParams { x: f32::NAN, y: f32::INFINITY, z: f32::NAN, divergence: f32::NAN, center: f32::NAN }, &mut g);
        assert!(g.iter().all(|x| x.is_finite()));
    }
}

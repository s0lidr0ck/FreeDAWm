//! The Wavetable Synth's tables, every one made here from mathematics: additive spectra, exact
//! Fourier series of piecewise-linear shapes (pulse-width and hard-sync sweeps, steps), formant
//! envelopes laid over a harmonic series, and the spectra of smooth functions (phase bending,
//! folding) taken by a discrete Fourier transform. No sample files and no tables from elsewhere.
//!
//! Each bank holds [`FRAMES`] single-cycle frames (the Position control moves through them).
//! Each frame is stored at [`LEVELS`] band limits half an octave apart (a mip-map): level `L`
//! keeps the first `512 · 2^(−L/2)` harmonics in a table at least four times that long, so the
//! oscillator reads the level whose highest harmonic is still under half the sample rate and
//! nothing folds back (and at most half an octave of the top is left out). The tables are
//! built once for the whole program, off the audio thread, the first time a Wavetable Synth is
//! made, and only read after that.

use std::sync::OnceLock;

/// Frames in each bank.
pub const FRAMES: usize = 32;
/// Harmonics kept at each band-limit level, half an octave apart (512 · 2^(−level/2)).
const HARMONICS: [usize; 18] = [512, 362, 256, 181, 128, 90, 64, 45, 32, 22, 16, 11, 8, 5, 4, 2, 2, 1];
/// Band-limit levels per frame.
pub const LEVELS: usize = HARMONICS.len();
/// Harmonics in the fullest level.
pub const TOP: usize = HARMONICS[0];

/// The banks, in the Table control's order.
pub const BANKS: &[&str] = &[
    "Classic",
    "Harmonic Sweep",
    "Pulse Width",
    "Sync Saw",
    "Sync Square",
    "Vowels",
    "Odd to Even",
    "Drawbars",
    "Phase Bend",
    "Fold",
    "Steps",
    "Comb",
    "Grit",
];

/// Harmonics kept at `level`.
pub const fn harmonics(level: usize) -> usize {
    if level < LEVELS { HARMONICS[level] } else { 1 }
}

/// Samples in one cycle at `level`: at least four per harmonic, a power of two, at least 64.
pub const fn size(level: usize) -> usize {
    let mut n = 64;
    while n < 4 * harmonics(level) {
        n *= 2;
    }
    n
}

/// Where `level` starts inside a frame (each level is followed by one guard sample, a copy of
/// its first, so reading between the last sample and the first needs no wrap).
const fn offset(level: usize) -> usize {
    let mut at = 0;
    let mut l = 0;
    while l < level {
        at += size(l) + 1;
        l += 1;
    }
    at
}

/// Samples per frame, every level with its guard.
pub const FRAME_LEN: usize = offset(LEVELS);

/// The level an oscillator stepping `inc` cycles per sample reads: the fullest whose highest
/// harmonic stays below half the sample rate.
#[inline]
pub fn level_for(inc: f32) -> usize {
    if !(inc.is_finite() && inc > 0.0) {
        return 0;
    }
    let most = 0.5 / inc;
    let mut l = 0;
    while l + 1 < LEVELS && harmonics(l) as f32 > most {
        l += 1;
    }
    l
}

/// Every bank's frames at every level.
pub struct Tables {
    data: Vec<f32>,
}

impl Tables {
    /// One frame's table at `level` with its guard sample (empty if out of range).
    #[inline]
    pub fn cycle(&self, bank: usize, frame: usize, level: usize) -> &[f32] {
        let level = level.min(LEVELS - 1);
        let start = (bank * FRAMES + frame) * FRAME_LEN + offset(level);
        self.data.get(start..start + size(level) + 1).unwrap_or(&[])
    }

    /// The wave of `bank` at `pos` (0..1, between frames a crossfade), band limit `level`, at
    /// `phase` (0..1): linear interpolation in a table four times oversampled.
    #[inline]
    pub fn read(&self, bank: usize, pos: f32, level: usize, phase: f32) -> f32 {
        let fp = pos.clamp(0.0, 1.0) * (FRAMES - 1) as f32;
        let i = (fp as usize).min(FRAMES - 1);
        let j = (i + 1).min(FRAMES - 1);
        let f = fp - i as f32;
        let a = lerp_cycle(self.cycle(bank, i, level), phase);
        if f <= 0.0 || i == j {
            return a;
        }
        a + (lerp_cycle(self.cycle(bank, j, level), phase) - a) * f
    }
}

#[inline]
fn lerp_cycle(t: &[f32], phase: f32) -> f32 {
    let n = t.len().saturating_sub(1);
    if n == 0 {
        return 0.0;
    }
    let x = phase.clamp(0.0, 1.0) * n as f32;
    let k = (x as usize).min(n - 1);
    let fr = x - k as f32;
    let a = t.get(k).copied().unwrap_or(0.0);
    let b = t.get(k + 1).copied().unwrap_or(a);
    a + (b - a) * fr
}

/// The tables, built the first time they are asked for (never on the audio thread: a synth asks
/// when it is made).
pub fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(build)
}

/// One cycle of `bank` at `pos` (0..1) drawn into `out` (for the panel): the fullest level,
/// resampled to `out`'s length.
pub fn draw(bank: usize, pos: f32, out: &mut [f32]) {
    let t = tables();
    let n = out.len().max(1);
    for (i, o) in out.iter_mut().enumerate() {
        *o = t.read(bank.min(BANKS.len() - 1), pos, 0, i as f32 / n as f32);
    }
}

type C = (f64, f64);

fn build() -> Tables {
    let mut data = vec![0.0f32; BANKS.len() * FRAMES * FRAME_LEN];
    let mut buf: Vec<C> = vec![(0.0, 0.0); size(0)];
    for bank in 0..BANKS.len() {
        for frame in 0..FRAMES {
            let spec = spectrum(bank, frame as f64 / (FRAMES - 1) as f64);
            let base = (bank * FRAMES + frame) * FRAME_LEN;
            let mut scale = 1.0;
            for level in 0..LEVELS {
                let n = size(level);
                let h = harmonics(level).min(n / 2 - 1);
                buf.clear();
                buf.resize(n, (0.0, 0.0));
                for k in 1..=h {
                    let c = spec.get(k).copied().unwrap_or((0.0, 0.0));
                    if let Some(x) = buf.get_mut(k) {
                        *x = c;
                    }
                    if let Some(x) = buf.get_mut(n - k) {
                        *x = (c.0, -c.1);
                    }
                }
                fft(&mut buf, true);
                if level == 0 {
                    let peak = buf.iter().fold(0.0f64, |m, x| m.max(x.0.abs()));
                    scale = if peak > 1e-9 { 1.0 / peak } else { 0.0 };
                }
                let at = base + offset(level);
                if let Some(dst) = data.get_mut(at..at + n + 1) {
                    for (d, s) in dst.iter_mut().zip(buf.iter()) {
                        *d = (s.0 * scale) as f32;
                    }
                    if let (Some(first), Some(guard)) = (buf.first(), dst.last_mut()) {
                        *guard = (first.0 * scale) as f32;
                    }
                }
            }
        }
    }
    Tables { data }
}

/// In-place radix-2 FFT (length a power of two): `inverse` uses e^{+i...} and does not divide.
fn fft(x: &mut [C], inverse: bool) {
    let n = x.len();
    if n < 2 || !n.is_power_of_two() {
        return;
    }
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            x.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let ang = sign * std::f64::consts::TAU / len as f64;
        for k in 0..len / 2 {
            let (s, c) = (ang * k as f64).sin_cos();
            for start in (0..n).step_by(len) {
                let (Some(&u), Some(&v)) = (x.get(start + k), x.get(start + k + len / 2)) else { continue };
                let t = (v.0 * c - v.1 * s, v.0 * s + v.1 * c);
                if let Some(a) = x.get_mut(start + k) {
                    *a = (u.0 + t.0, u.1 + t.1);
                }
                if let Some(b) = x.get_mut(start + k + len / 2) {
                    *b = (u.0 - t.0, u.1 - t.1);
                }
            }
        }
        len <<= 1;
    }
}

/// A sine-phase partial of amplitude `a` as a coefficient (`a sin` = 2 Re(c e^{iωt})).
fn sine(a: f64) -> C {
    (0.0, -a / 2.0)
}

/// The Fourier coefficients c_1..c_TOP (index 0 unused) of one period (0..1) of the
/// piecewise-linear shape through `pts` (time, value), in order: two points at the same time are
/// a jump; the shape jumps back to the first point at the end of the period.
fn pwl(pts: &[(f64, f64)]) -> Vec<C> {
    let mut c = vec![(0.0, 0.0); TOP + 1];
    for (k, ck) in c.iter_mut().enumerate().skip(1) {
        let w = std::f64::consts::TAU * k as f64;
        // ∫ (a + b t) e^{-iwt} dt = e^{-iwt} (b/w² + i (a + b t)/w).
        let anti = |a: f64, b: f64, t: f64| {
            let (cs, sn) = ((w * t).cos(), -(w * t).sin());
            let (re, im) = (b / (w * w), (a + b * t) / w);
            (cs * re - sn * im, cs * im + sn * re)
        };
        let mut sum = (0.0, 0.0);
        for seg in pts.windows(2) {
            let (Some(&(t0, v0)), Some(&(t1, v1))) = (seg.first(), seg.get(1)) else { continue };
            if t1 - t0 <= 1e-12 {
                continue;
            }
            let b = (v1 - v0) / (t1 - t0);
            let a = v0 - b * t0;
            let (f1, f0) = (anti(a, b, t1), anti(a, b, t0));
            sum = (sum.0 + f1.0 - f0.0, sum.1 + f1.1 - f0.1);
        }
        *ck = sum;
    }
    c
}

/// The coefficients of a smooth function of one period (0..1), by a 4096-point transform.
fn sampled(f: impl Fn(f64) -> f64) -> Vec<C> {
    const N: usize = 4096;
    let mut x: Vec<C> = (0..N).map(|i| (f(i as f64 / N as f64), 0.0)).collect();
    fft(&mut x, false);
    let mut c = vec![(0.0, 0.0); TOP + 1];
    for (k, ck) in c.iter_mut().enumerate().skip(1) {
        let v = x.get(k).copied().unwrap_or((0.0, 0.0));
        *ck = (v.0 / N as f64, v.1 / N as f64);
    }
    c
}

/// Sine-phase partials with amplitudes `amp(k)`.
fn additive(amp: impl Fn(usize) -> f64) -> Vec<C> {
    (0..=TOP).map(|k| if k == 0 { (0.0, 0.0) } else { sine(amp(k)) }).collect()
}

fn mix(a: &[C], b: &[C], f: f64) -> Vec<C> {
    a.iter().zip(b).map(|(x, y)| (x.0 + (y.0 - x.0) * f, x.1 + (y.1 - x.1) * f)).collect()
}

/// `n` anchor spectra spread over 0..1, crossfaded at `t`.
fn across(t: f64, anchors: usize, spec: impl Fn(usize) -> Vec<C>) -> Vec<C> {
    let span = (anchors.max(2) - 1) as f64;
    let x = (t * span).clamp(0.0, span);
    let i = (x.floor() as usize).min(anchors.max(2) - 2);
    mix(&spec(i), &spec(i + 1), x - i as f64)
}

/// A hash of `k` and `seed` to 0..1 (splitmix64), for the Grit bank's fixed random spectra.
fn hash01(k: u64, seed: u64) -> f64 {
    let mut z = k.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// The frame of `bank` at position `t` (0..1) as Fourier coefficients.
fn spectrum(bank: usize, t: f64) -> Vec<C> {
    use std::f64::consts::PI;
    let odd = |k: usize| k % 2 == 1;
    match bank {
        // Sine, triangle, saw, square, morphing harmonic by harmonic.
        0 => across(t, 4, |i| {
            additive(move |k| match i {
                0 => f64::from(u8::from(k == 1)),
                1 if odd(k) => 8.0 / (PI * PI * (k * k) as f64) * if (k / 2) % 2 == 0 { 1.0 } else { -1.0 },
                2 => 2.0 / (PI * k as f64),
                3 if odd(k) => 4.0 / (PI * k as f64),
                _ => 0.0,
            })
        }),
        // A saw's harmonics under a steep spectral low-pass opening from the fundamental to 256.
        1 => {
            let kc = 2f64.powf(t * 8.0);
            additive(|k| (1.0 / k as f64) / (1.0 + (k as f64 / kc).powi(8)).sqrt())
        }
        // A pulse narrowing from square (50%) to 3%.
        2 => {
            let w = 0.5 - 0.47 * t;
            pwl(&[(0.0, 1.0), (w, 1.0), (w, -1.0), (1.0, -1.0)])
        }
        // A saw hard-synced to the fundamental, its own pitch rising from 1 to 8 times.
        3 => {
            let r = 1.0 + 7.0 * t;
            let mut pts = vec![(0.0, -1.0)];
            let mut at = 1.0 / r;
            while at < 1.0 - 1e-9 {
                pts.push((at, 1.0));
                pts.push((at, -1.0));
                at += 1.0 / r;
            }
            pts.push((1.0, 2.0 * (r - (r - 1e-9).floor()).min(1.0) - 1.0));
            pwl(&pts)
        }
        // A square hard-synced the same way.
        4 => {
            let r = 1.0 + 7.0 * t;
            let mut pts = vec![(0.0, 1.0)];
            let (mut at, mut v) = (0.5 / r, 1.0);
            while at < 1.0 - 1e-9 {
                pts.push((at, v));
                v = -v;
                pts.push((at, v));
                at += 0.5 / r;
            }
            pts.push((1.0, v));
            pwl(&pts)
        }
        // Vowel formants (a, e, i, o, u) over a 110 Hz harmonic series.
        5 => {
            const V: [[f64; 3]; 5] =
                [[650.0, 1080.0, 2650.0], [400.0, 1700.0, 2600.0], [290.0, 1870.0, 2800.0], [400.0, 800.0, 2600.0], [350.0, 600.0, 2700.0]];
            let x = (t * 4.0).clamp(0.0, 4.0);
            let i = (x.floor() as usize).min(3);
            let f = x - i as f64;
            let (a, b) = (V.get(i).copied().unwrap_or(V[0]), V.get(i + 1).copied().unwrap_or(V[0]));
            let fm: Vec<f64> = a.iter().zip(&b).map(|(p, q)| p + (q - p) * f).collect();
            additive(|k| {
                let hz = 110.0 * k as f64;
                let env: f64 =
                    fm.iter().zip([(80.0, 1.0), (90.0, 0.5), (120.0, 0.25)]).map(|(f0, (bw, g))| g / (1.0 + ((hz - f0) / bw).powi(2))).sum();
                env / k as f64
            })
        }
        // From odd harmonics only (hollow) to even ones only, the fundamental kept.
        6 => additive(|k| {
            let w = if k == 1 {
                1.0
            } else if odd(k) {
                1.0 - t
            } else {
                t
            };
            w / k as f64
        }),
        // Organ drawbar registrations (harmonics 1, 2, 3, 4, 5, 6, 8; 3 dB a step).
        7 => {
            const H: [usize; 7] = [1, 2, 3, 4, 5, 6, 8];
            const R: [[u8; 7]; 5] =
                [[8, 0, 0, 0, 0, 0, 0], [8, 8, 0, 0, 0, 0, 0], [8, 8, 8, 0, 0, 0, 0], [8, 6, 8, 6, 4, 2, 0], [8, 8, 8, 8, 8, 8, 8]];
            across(t, R.len(), |i| {
                let reg = R.get(i).copied().unwrap_or(R[0]);
                additive(move |k| {
                    H.iter()
                        .position(|h| *h == k)
                        .and_then(|j| reg.get(j))
                        .map_or(0.0, |&v| if v == 0 { 0.0 } else { 10f64.powf(-3.0 * f64::from(8 - v) / 20.0) })
                })
            })
        }
        // A sine bending its own phase: sin(x + β sin x), β from 0 to 6.
        8 => {
            let b = 6.0 * t;
            sampled(|x| (std::f64::consts::TAU * x + b * (std::f64::consts::TAU * x).sin()).sin())
        }
        // A sine folded back on itself, harder along the table.
        9 => {
            let g = 1.0 + 7.0 * t;
            sampled(|x| (PI / 2.0 * g * (std::f64::consts::TAU * x).sin()).sin())
        }
        // A rising ramp in fewer and fewer steps (32 down to 2).
        10 => {
            let n = (2f64.powf(5.0 - 4.0 * t)).round().max(2.0) as usize;
            let mut pts = Vec::with_capacity(2 * n);
            for s in 0..n {
                let v = -1.0 + 2.0 * s as f64 / (n - 1) as f64;
                pts.push((s as f64 / n as f64, v));
                pts.push(((s + 1) as f64 / n as f64, v));
            }
            pwl(&pts)
        }
        // A saw through a comb moving along its harmonics.
        11 => {
            let d = 0.5 * t;
            additive(|k| (PI * k as f64 * d).cos().abs() / k as f64)
        }
        // Four fixed random spectra (amplitudes and phases), crossfaded.
        _ => across(t, 4, |i| {
            (0..=TOP)
                .map(|k| {
                    if k == 0 {
                        return (0.0, 0.0);
                    }
                    let a = hash01(k as u64, 11 + i as u64) / (k as f64).powf(0.8);
                    let ph = std::f64::consts::TAU * hash01(k as u64, 101 + i as u64);
                    (a / 2.0 * ph.cos(), a / 2.0 * ph.sin())
                })
                .collect()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fft_and_the_exact_series_agree() {
        // A saw's exact series against its transform: c_k = i/(πk).
        let c = pwl(&[(0.0, -1.0), (1.0, 1.0)]);
        for (k, ck) in c.iter().enumerate().take(20).skip(1) {
            let want = 1.0 / (std::f64::consts::PI * k as f64);
            assert!(ck.0.abs() < 1e-9 && (ck.1 - want).abs() < 1e-9, "{k}: {ck:?}");
        }
        let s = sampled(|x| (std::f64::consts::TAU * 3.0 * x).sin());
        assert!((s[3].1 + 0.5).abs() < 1e-9 && s[2].0.abs() < 1e-9);
    }

    #[test]
    fn every_frame_is_normalised_and_finite_and_levels_hold_their_harmonics() {
        let t = tables();
        for (bank, name) in BANKS.iter().enumerate() {
            for frame in [0, FRAMES / 2, FRAMES - 1] {
                let c = t.cycle(bank, frame, 0);
                assert_eq!(c.len(), size(0) + 1);
                let peak = c.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                assert!((peak - 1.0).abs() < 1e-4, "{name} frame {frame}: {peak}");
                for level in 0..LEVELS {
                    let c = t.cycle(bank, frame, level);
                    assert!(c.iter().all(|x| x.is_finite() && x.abs() < 1.5));
                    assert_eq!(c.first(), c.last(), "guard sample");
                }
            }
        }
        assert_eq!(harmonics(level_for(440.0 / 48_000.0)), 45, "54 would fit: 45 is the fullest level under");
        assert_eq!(level_for(20.0 / 48_000.0), 0);
        assert_eq!(harmonics(level_for(3_000.0 / 48_000.0)), 8);
        assert_eq!(level_for(30_000.0 / 48_000.0), LEVELS - 1);
    }
}

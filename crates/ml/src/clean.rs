//! Clean Vocal's options and its level handling: the fixed make-up gain after Smule Renaissance and
//! the wet/dry mix.
//!
//! Two modes:
//! - **Vocal (singing)**, the default: Smule Renaissance Small alone (trained on singing; it takes
//!   noise and reverb off together). DeepFilterNet 3, a speech model, treats sustained sung notes
//!   as noise and pumps them (measured on a sung chorus: 36–105 gain jumps of more than 3 dB per
//!   20 ms window in 45 s against 2–7 for Renaissance).
//! - **Speech**: DeepFilterNet 3 de-noise (attenuation limit `atten_db`), optionally followed by
//!   Renaissance de-reverb.
//!
//! **Make-up gain** (whenever Renaissance runs): one constant gain for the whole result, the same
//! on every channel, never time-varying. Loudness is measured BS.1770-style without the shelf:
//! both signals through an 80 Hz second-order high-pass (so rumble the model removes doesn't
//! count), mean square over all channels in 400 ms blocks with a 100 ms hop; blocks of the
//! *input* above −60 dBFS are kept, then those within 10 dB of their mean power (the relative
//! gate), so the measurement covers the voiced parts and ignores the gaps between phrases. The
//! output is measured over the same blocks. The gain is their ratio (loudness matched fully),
//! clamped to ±12 dB, then lowered only if needed to keep the cleaned output's sample peak at or
//! under −1 dBFS ([`PEAK_CEILING_DBFS`]; never raised). Silence (no block above the gate) gets
//! 0 dB. (Renaissance also de-clips, so its peaks can sit above the source's; the owner chose
//! matched loudness over the source's peak, with the −1 dBFS ceiling as the only limit.)
//!
//! **Amount**: a wet/dry mix, 0–100 %. In the DAW it is the clip's own (`Clip::clean`, applied at
//! playback, non-destructively); the job renders all wet there, and [`CleanOptions::mix`] is for
//! the CLI's files. Renaissance's output is aligned with its input closely enough to blend:
//! measured on 45 s of a live sung chorus, the cross-correlation peaks at lag −1 sample (r 0.934,
//! 0.921 at lag 0) and the cross-spectrum's phase is within 1.2 samples from 300 Hz to 16 kHz
//! (the 60 Hz high-pass shifts the lows); a 50 % blend keeps the level (mean −0.01 dB). The
//! dry signal is the untouched input.

/// Which chain Clean Vocal runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CleanMode {
    /// Smule Renaissance alone (noise and reverb together), with make-up gain and Amount.
    #[default]
    Vocal,
    /// DeepFilterNet 3 de-noise, then (optionally) Renaissance de-reverb.
    Speech,
}

impl CleanMode {
    pub fn id(self) -> &'static str {
        match self {
            CleanMode::Vocal => "vocal",
            CleanMode::Speech => "speech",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CleanMode::Vocal => "Vocal (singing)",
            CleanMode::Speech => "Speech",
        }
    }

    pub fn parse(s: &str) -> Option<CleanMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "vocal" | "singing" | "sung" => Some(CleanMode::Vocal),
            "speech" | "spoken" | "dialog" | "dialogue" => Some(CleanMode::Speech),
            _ => None,
        }
    }
}

/// Clean Vocal's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CleanOptions {
    pub mode: CleanMode,
    /// Speech: de-reverb with Renaissance after the de-noise. (Vocal mode always runs it.)
    pub dereverb: bool,
    /// Speech: DeepFilterNet's attenuation limit, dB (100 = unlimited).
    pub atten_db: f32,
    /// Vocal: the wet share of the wet/dry mix, 0–1 (1 = all cleaned).
    pub mix: f32,
}

impl Default for CleanOptions {
    fn default() -> Self {
        CleanOptions { mode: CleanMode::Vocal, dereverb: true, atten_db: crate::denoise::DEFAULT_ATTEN_DB, mix: 1.0 }
    }
}

impl CleanOptions {
    /// Vocal mode with the wet share `mix` (0–1).
    pub fn vocal(mix: f32) -> Self {
        CleanOptions { mix: sane_mix(mix), ..CleanOptions::default() }
    }

    /// Speech mode: de-noise at `atten_db`, then de-reverb when `dereverb`.
    pub fn speech(atten_db: f32, dereverb: bool) -> Self {
        CleanOptions { mode: CleanMode::Speech, dereverb, atten_db, mix: 1.0 }
    }

    /// The options the old `denoise` / `dereverb` switches meant: anything with de-reverb is the
    /// Vocal mode (Renaissance takes the noise off too); de-noise alone is the Speech mode without
    /// de-reverb; neither is `None` (nothing to do).
    pub fn from_legacy(denoise: bool, dereverb: bool, atten_db: f32) -> Option<Self> {
        match (denoise, dereverb) {
            (_, true) => Some(CleanOptions::vocal(1.0)),
            (true, false) => Some(CleanOptions::speech(atten_db, false)),
            (false, false) => None,
        }
    }

    /// Whether Renaissance runs.
    pub fn runs_srs(&self) -> bool {
        self.mode == CleanMode::Vocal || self.dereverb
    }
}

fn sane_mix(m: f32) -> f32 {
    if m.is_finite() { m.clamp(0.0, 1.0) } else { 1.0 }
}

/// The make-up gain's limit, dB (either way).
pub const MAKEUP_LIMIT_DB: f64 = 12.0;
/// The highest sample peak the made-up output may have, dBFS.
pub const PEAK_CEILING_DBFS: f64 = -1.0;
/// Absolute gate for the loudness blocks, dBFS (mean square).
pub const ABS_GATE_DB: f64 = -60.0;
/// Relative gate below the mean power of the blocks above the absolute gate, dB.
pub const REL_GATE_DB: f64 = 10.0;

/// An 80 Hz second-order (RBJ, Q 0.707) high-pass on each channel, for measuring only.
fn measure_filter(audio: &[Vec<f32>], rate: u32) -> Vec<Vec<f64>> {
    let w0 = 2.0 * std::f64::consts::PI * 80.0 / f64::from(rate.max(1));
    let alpha = w0.sin() / 2.0 / std::f64::consts::FRAC_1_SQRT_2;
    let c = w0.cos();
    let a0 = 1.0 + alpha;
    let b = [(1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0];
    let (a1, a2) = (-2.0 * c / a0, (1.0 - alpha) / a0);
    audio
        .iter()
        .map(|ch| {
            let (mut x1, mut x2, mut y1, mut y2) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            ch.iter()
                .map(|&v| {
                    let x = if v.is_finite() { f64::from(v) } else { 0.0 };
                    let y = b[0] * x + b[1] * x1 + b[2] * x2 - a1 * y1 - a2 * y2;
                    (x2, x1, y2, y1) = (x1, x, y1, y);
                    y
                })
                .collect()
        })
        .collect()
}

/// Mean square over all channels of `[start, start + n)`.
fn block_power(f: &[Vec<f64>], start: usize, n: usize) -> f64 {
    let mut sum = 0.0;
    let mut count = 0usize;
    for ch in f {
        for v in ch.iter().skip(start).take(n) {
            sum += v * v;
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

fn db(p: f64) -> f64 {
    10.0 * p.max(1e-30).log10()
}

/// The gated loudness (mean square, dB) of `input` and `output` over the blocks the input's gate
/// keeps, or `None` when no block passes.
pub fn gated_powers(input: &[Vec<f32>], output: &[Vec<f32>], rate: u32) -> Option<(f64, f64)> {
    let fi = measure_filter(input, rate);
    let fo = measure_filter(output, rate);
    let len = crate::frames(input).min(crate::frames(output));
    let block = ((f64::from(rate) * 0.4) as usize).max(1);
    let hop = (block / 4).max(1);
    let mut starts = Vec::new();
    let mut s = 0usize;
    loop {
        starts.push(s);
        if s + block >= len {
            break;
        }
        s += hop;
    }
    let pin: Vec<f64> = starts.iter().map(|&s| block_power(&fi, s, block.min(len - s.min(len)))).collect();
    let abs_gate = 10f64.powf(ABS_GATE_DB / 10.0);
    let above: Vec<f64> = pin.iter().copied().filter(|p| *p > abs_gate).collect();
    if above.is_empty() {
        return None;
    }
    let rel_gate = above.iter().sum::<f64>() / above.len() as f64 * 10f64.powf(-REL_GATE_DB / 10.0);
    let (mut si, mut so, mut n) = (0.0f64, 0.0f64, 0usize);
    for (&s, &p) in starts.iter().zip(&pin) {
        if p > abs_gate && p > rel_gate {
            si += p;
            so += block_power(&fo, s, block.min(len - s.min(len)));
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    Some((db(si / n as f64), db(so / n as f64)))
}

fn peak(audio: &[Vec<f32>]) -> f32 {
    audio.iter().flatten().filter(|v| v.is_finite()).fold(0.0f32, |m, v| m.max(v.abs()))
}

/// The fixed make-up gain (linear) that brings `output`'s gated loudness to `input`'s, clamped to
/// ±[`MAKEUP_LIMIT_DB`] and lowered so `output`'s peak stays at or under [`PEAK_CEILING_DBFS`].
/// Both are `[channel][frame]` at `rate`.
pub fn makeup_gain(input: &[Vec<f32>], output: &[Vec<f32>], rate: u32) -> f32 {
    let mut g_db = match gated_powers(input, output, rate) {
        Some((pi, po)) if po > -300.0 => (pi - po).clamp(-MAKEUP_LIMIT_DB, MAKEUP_LIMIT_DB),
        _ => 0.0,
    };
    let pk_out = f64::from(peak(output));
    if pk_out > 0.0 {
        g_db = g_db.min(PEAK_CEILING_DBFS - 20.0 * pk_out.log10());
    }
    let g = 10f64.powf(g_db / 20.0) as f32;
    if g.is_finite() { g } else { 1.0 }
}

/// Applies `gain` to every sample.
pub fn apply_gain(audio: &mut [Vec<f32>], gain: f32) {
    for v in audio.iter_mut().flatten() {
        *v *= gain;
    }
}

/// `wet` becomes `mix · wet + (1 − mix) · dry`, sample by sample (same layout).
pub fn mix_dry(wet: &mut [Vec<f32>], dry: &[Vec<f32>], mix: f32) {
    let m = sane_mix(mix);
    if m >= 1.0 {
        return;
    }
    for (w, d) in wet.iter_mut().zip(dry) {
        for (a, b) in w.iter_mut().zip(d) {
            *a = m * *a + (1.0 - m) * b;
        }
    }
}

/// How steady a processor's gain is on the voiced parts: per 20 ms window, the output's level
/// over the input's (both through the 80 Hz measuring high-pass, all channels), over the windows
/// whose input is within `range_db` of the loudest window (and above −60 dBFS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelStats {
    /// Voiced windows.
    pub windows: usize,
    /// Mean and standard deviation of the per-window gain, dB.
    pub mean_db: f64,
    pub sd_db: f64,
    /// Neighbouring voiced windows whose gains differ by more than 3 dB.
    pub jumps: usize,
    /// Gated loudness (as for the make-up gain) of the output minus the input's, dB.
    pub loudness_diff_db: f64,
}

/// [`LevelStats`] of `output` against `input` (`[channel][frame]` at `rate`).
pub fn level_stats(input: &[Vec<f32>], output: &[Vec<f32>], rate: u32, range_db: f64) -> LevelStats {
    let fi = measure_filter(input, rate);
    let fo = measure_filter(output, rate);
    let len = crate::frames(input).min(crate::frames(output));
    let w = ((f64::from(rate) * 0.02) as usize).max(1);
    let n = len / w;
    let pin: Vec<f64> = (0..n).map(|k| block_power(&fi, k * w, w)).collect();
    let pout: Vec<f64> = (0..n).map(|k| block_power(&fo, k * w, w)).collect();
    let top = pin.iter().copied().fold(0.0f64, f64::max);
    let gate = (top * 10f64.powf(-range_db / 10.0)).max(10f64.powf(ABS_GATE_DB / 10.0));
    let gains: Vec<Option<f64>> = pin.iter().zip(&pout).map(|(i, o)| (*i > gate).then(|| db(*o) - db(*i))).collect();
    let voiced: Vec<f64> = gains.iter().flatten().copied().collect();
    let count = voiced.len();
    let mean = if count > 0 { voiced.iter().sum::<f64>() / count as f64 } else { 0.0 };
    let sd = if count > 1 { (voiced.iter().map(|g| (g - mean) * (g - mean)).sum::<f64>() / (count - 1) as f64).sqrt() } else { 0.0 };
    let jumps = gains.windows(2).filter(|p| matches!(p, [Some(a), Some(b)] if (a - b).abs() > 3.0)).count();
    let loudness_diff_db = gated_powers(input, output, rate).map_or(0.0, |(i, o)| o - i);
    LevelStats { windows: count, mean_db: mean, sd_db: sd, jumps, loudness_diff_db }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(len: usize, amp: f32, rate: u32) -> Vec<f32> {
        (0..len).map(|i| amp * (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / rate as f32).sin()).collect()
    }

    fn gain_db(g: f32) -> f64 {
        20.0 * f64::from(g).log10()
    }

    #[test]
    fn makeup_matches_loudness() {
        let rate = 48_000;
        let x = vec![tone(rate as usize * 2, 0.5, rate)];
        let y = vec![tone(rate as usize * 2, 0.25, rate)];
        // 6 dB quieter output: matched fully (its peak, 0.5, is well under -1 dBFS).
        assert!((gain_db(makeup_gain(&x, &y, rate)) - 6.02).abs() < 0.05);
        assert!((gain_db(makeup_gain(&x, &x, rate))).abs() < 1e-3);
    }

    #[test]
    fn makeup_is_clamped_to_12_db() {
        let rate = 48_000;
        let x = vec![tone(rate as usize, 0.5, rate)];
        let quiet = vec![tone(rate as usize, 0.005, rate)]; // -40 dB
        let loud = vec![tone(rate as usize, 0.9, rate)];
        let xq = vec![tone(rate as usize, 0.05, rate)];
        let up = gain_db(makeup_gain(&x, &quiet, rate));
        assert!((up - 12.0).abs() < 1e-3, "{up}");
        let down = gain_db(makeup_gain(&xq, &loud, rate));
        assert!((down + 12.0).abs() < 1e-3, "{down}");
    }

    #[test]
    fn makeup_matches_past_the_input_peak_but_stops_at_minus_1_dbfs() {
        let rate = 48_000;
        let n = rate as usize;
        // De-clipped output: same loudness wanted (+6 dB) though its spike passes the input's peak.
        let x = vec![tone(n, 0.5, rate)];
        let mut y = vec![tone(n, 0.25, rate)];
        y[0][1000] = 0.4;
        let g = makeup_gain(&x, &y, rate);
        assert!((gain_db(g) - 6.02).abs() < 0.05, "{}", gain_db(g));
        // A loud source: the make-up stops where the output's peak reaches -1 dBFS.
        let x = vec![tone(n, 0.85, rate)];
        let mut y = vec![tone(n, 0.25, rate)];
        y[0][1000] = 0.5;
        let g = makeup_gain(&x, &y, rate);
        apply_gain(&mut y, g);
        let ceiling = 10f32.powf(-1.0 / 20.0);
        assert!((peak(&y) - ceiling).abs() < 1e-4, "{}", peak(&y));
    }

    #[test]
    fn silence_gets_no_gain_and_one_gain_for_all_channels() {
        let rate = 48_000;
        let z = vec![vec![0.0f32; rate as usize]; 2];
        assert_eq!(makeup_gain(&z, &z, rate), 1.0);
        let x = vec![tone(rate as usize, 0.5, rate), tone(rate as usize, 0.5, rate)];
        let mut y = vec![tone(rate as usize, 0.25, rate), tone(rate as usize, 0.25, rate)];
        let g = makeup_gain(&x, &y, rate);
        apply_gain(&mut y, g);
        assert!((y[0][123] - y[1][123]).abs() < 1e-7);
    }

    #[test]
    fn gate_ignores_the_gaps() {
        // Voice at -6 dBFS in half the time, silence-ish noise in the rest: the output keeps the
        // voice and removes the noise; the make-up must be ~0 dB, not the ungated difference.
        let rate = 48_000;
        let n = rate as usize;
        let mut x = tone(2 * n, 0.5, rate);
        let mut y = x.clone();
        for i in n..2 * n {
            x[i] = if i % 2 == 0 { 0.003 } else { -0.003 };
            y[i] = 0.0;
        }
        let g = gain_db(makeup_gain(&[x], &[y], rate));
        assert!(g.abs() < 0.1, "{g}");
    }

    #[test]
    fn mix_blends_and_modes_map() {
        let mut w = vec![vec![1.0f32; 4]];
        mix_dry(&mut w, &[vec![0.0f32; 4]], 0.25);
        assert!((w[0][0] - 0.25).abs() < 1e-7);
        assert_eq!(CleanOptions::from_legacy(true, true, 24.0), Some(CleanOptions::vocal(1.0)));
        assert_eq!(CleanOptions::from_legacy(false, true, 24.0).map(|o| o.mode), Some(CleanMode::Vocal));
        assert_eq!(CleanOptions::from_legacy(true, false, 18.0), Some(CleanOptions::speech(18.0, false)));
        assert_eq!(CleanOptions::from_legacy(false, false, 18.0), None);
        assert_eq!(CleanMode::parse("Speech"), Some(CleanMode::Speech));
        assert!(CleanOptions::vocal(f32::NAN).mix == 1.0);
    }
}

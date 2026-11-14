//! Sift on made-up voices: vowels are harmonic tones with a falling spectrum, sibilants are
//! bands of noise, plosives are short low thumps. Real voices are messier; these hold the
//! behaviour that must survive a retune.

use super::*;
use rustfft::{FftPlanner, num_complex::Complex};

const SR: f32 = 48_000.0;
const N: usize = 384;

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

fn ms(t: f32) -> usize {
    (t * 0.001 * SR) as usize
}

/// A raised-cosine fade of 5 ms at both ends.
fn faded(mut x: Vec<f32>) -> Vec<f32> {
    let n = ms(5.0).min(x.len() / 2);
    for i in 0..n {
        let g = 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / n as f32).cos();
        x[i] *= g;
        let j = x.len() - 1 - i;
        x[j] *= g;
    }
    x
}

/// A vowel: harmonics of `f0` up to `top` Hz, flat to 600 Hz and falling `slope` dB per octave
/// above, about `amp` in size.
fn vowel(len: usize, f0: f32, amp: f32, top: f32, slope: f32) -> Vec<f32> {
    let mut x = vec![0.0f32; len];
    let mut h = 1;
    while f0 * h as f32 <= top {
        let f = f0 * h as f32;
        let a = if f <= 600.0 { 1.0 } else { 10f32.powf(-slope * (f / 600.0).log2() / 20.0) };
        let w = std::f32::consts::TAU * f / SR;
        for (i, v) in x.iter_mut().enumerate() {
            *v += a * (w * i as f32 + h as f32).sin();
        }
        h += 1;
    }
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
    x.iter_mut().for_each(|v| *v *= amp / peak);
    faded(x)
}

/// An ordinary vowel: nothing much above 5 kHz.
fn ah(len: usize, amp: f32) -> Vec<f32> {
    vowel(len, 140.0, amp, 5_000.0, 12.0)
}

/// Noise between `lo` and `hi` Hz, `rms` in size.
fn noise_band(len: usize, lo: f32, hi: f32, rms: f32, seed: u32) -> Vec<f32> {
    let n = len.next_power_of_two();
    let mut r = Rng(seed);
    let mut buf: Vec<Complex<f32>> = (0..n).map(|_| Complex::new(r.next(), 0.0)).collect();
    let mut planner = FftPlanner::<f32>::new();
    planner.plan_fft_forward(n).process(&mut buf);
    for (k, c) in buf.iter_mut().enumerate() {
        let f = k.min(n - k) as f32 * SR / n as f32;
        if f < lo || f >= hi {
            *c = Complex::new(0.0, 0.0);
        }
    }
    planner.plan_fft_inverse(n).process(&mut buf);
    let mut x: Vec<f32> = buf.iter().take(len).map(|c| c.re).collect();
    let now = (x.iter().map(|v| v * v).sum::<f32>() / len as f32).sqrt().max(1e-12);
    x.iter_mut().for_each(|v| *v *= rms / now);
    faded(x)
}

/// A sibilant: noise from 5 to 9 kHz.
fn ess(len: usize, rms: f32, seed: u32) -> Vec<f32> {
    noise_band(len, 5_000.0, 9_000.0, rms, seed)
}

/// A plosive: a 60 Hz thump that dies away in about 40 ms, `amp` at its start.
fn pop(len: usize, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let env = (-t / 0.015).exp() * (1.0 - (-t / 0.0015).exp());
            amp * env * ((std::f32::consts::TAU * 60.0 * t).sin() + 0.5 * (std::f32::consts::TAU * 95.0 * t).sin())
        })
        .collect()
}

fn silence(len: usize) -> Vec<f32> {
    vec![0.0; len]
}

/// Parts end to end, with where each one starts and ends.
fn line(parts: Vec<Vec<f32>>) -> (Vec<f32>, Vec<std::ops::Range<usize>>) {
    let mut out = Vec::new();
    let mut at = Vec::new();
    for p in parts {
        at.push(out.len()..out.len() + p.len());
        out.extend(p);
    }
    (out, at)
}

fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

/// Mean power between `lo` and `hi` Hz, in dB, over Hann frames of 2048.
fn band_db(x: &[f32], lo: f32, hi: f32) -> f32 {
    let n = 2048;
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let (mut sum, mut count) = (0.0f64, 0usize);
    for frame in x.chunks_exact(n) {
        let mut buf: Vec<Complex<f32>> =
            frame.iter().enumerate().map(|(i, v)| Complex::new(v * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()), 0.0)).collect();
        fft.process(&mut buf);
        for (k, c) in buf.iter().enumerate().take(n / 2) {
            let f = k as f32 * SR / n as f32;
            if f >= lo && f < hi {
                sum += f64::from(c.norm_sqr());
                count += 1;
            }
        }
    }
    (10.0 * (sum / count.max(1) as f64).max(1e-30).log10()) as f32
}

fn rms_db(x: &[f32]) -> f32 {
    10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).max(1e-30).log10()
}

fn sift(set: &[(&str, f32)]) -> Sift {
    let mut s = Sift::new();
    s.prepare(SR, 512, 2);
    for (id, v) in set {
        assert!(s.set_param(id, *v), "{id}");
    }
    s
}

/// Runs `x` on both channels in blocks of `block`, takes the latency off the front, and gives
/// the left output with the largest readouts seen.
fn run(s: &mut Sift, x: &[f32], block: usize) -> (Vec<f32>, [f32; 4]) {
    let lat = s.latency();
    let total = x.len() + lat;
    let padded: Vec<f32> = x.iter().copied().chain(std::iter::repeat_n(0.0, lat)).collect();
    let mut out = Vec::with_capacity(total);
    let mut top = [0.0f32; 4];
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        let mut buf = vec![padded[pos..pos + n].to_vec(), padded[pos..pos + n].to_vec()];
        s.process(&mut buf, n);
        let mut r = [f32::NAN; 8];
        s.readouts(&mut r);
        for i in 0..4 {
            top[i] = top[i].max(r[i]);
        }
        out.extend_from_slice(&buf[0]);
        pos += n;
    }
    (out[lat..].to_vec(), top)
}

/// "ah", a gap, "s", a gap, "ah": the vowel 0.1 in size, the sibilant `s_rms`.
fn ah_s_ah(s_rms: f32) -> (Vec<f32>, Vec<std::ops::Range<usize>>) {
    line(vec![ah(ms(400.0), 0.1), silence(ms(30.0)), ess(ms(250.0), s_rms, 7), silence(ms(30.0)), ah(ms(400.0), 0.1)])
}

#[test]
fn registered_listed_and_a_frame_late() {
    let mut p = crate::create("fs_sift").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    assert!(p.info().listed());
    assert_eq!(p.latency(), 384, "8 ms at 48 kHz");
    p.prepare(44_100.0, 512, 2);
    assert_eq!(p.latency(), 352);
    p.prepare(96_000.0, 512, 2);
    assert_eq!(p.latency(), 768);
    assert!(crate::plugins::fs::engine("fs_sift").unwrap().fidelity.is_none(), "there is no C++ original");
    let ids: Vec<&str> = SIFT_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(ids, ["sibAmount", "sibRange", "sibLow", "sibHigh", "mode", "ploAmount", "ploRange", "ploHigh", "listen", "bypass"]);
    assert_eq!(SIFT_PARAMS[9].group, "", "the engine's bypass is hidden");
    assert_eq!(SIFT_PARAMS[4].choices, ["Split", "Wide"]);
    assert_eq!(SIFT_PARAMS[8].choices, ["Off", "Sibilance", "Plosives", "Removed"]);
}

#[test]
fn with_both_amounts_at_zero_it_is_a_pure_delay() {
    let (x, _) = ah_s_ah(0.08);
    for block in [1usize, 64, 500, 4096] {
        let mut s = sift(&[("sibAmount", 0.0), ("ploAmount", 0.0)]);
        let (y, top) = run(&mut s, &x, block);
        let err = x.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 2e-5, "block {block}: {err}");
        assert_eq!((top[0], top[1]), (0.0, 0.0));
    }
    // And bypassed, whatever the knobs say.
    let mut s = sift(&[("sibAmount", 100.0), ("ploAmount", 100.0), ("listen", 1.0), ("bypass", 1.0)]);
    let (y, _) = run(&mut s, &x, 512);
    let err = x.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(err < 2e-5, "{err}");
}

#[test]
fn a_sibilant_comes_down_and_the_vowels_either_side_do_not() {
    let (x, at) = ah_s_ah(0.08);
    let mut s = sift(&[("sibAmount", 60.0), ("ploAmount", 0.0)]);
    let (y, top) = run(&mut s, &x, 512);
    let cut = rms_db(&y[at[2].clone()]) - rms_db(&x[at[2].clone()]);
    assert!(cut < -4.0, "the s came down {cut:.1} dB");
    assert!(cut > -12.5, "and no further than Range: {cut:.1} dB");
    for i in [0, 4] {
        let d = rms_db(&y[at[i].clone()]) - rms_db(&x[at[i].clone()]);
        assert!(d.abs() < 0.3, "vowel {i} moved {d:.2} dB");
    }
    assert!(top[0] > 4.0 && top[2] > 0.9, "it knew: {top:?}");
    assert!(top[1] == 0.0, "no plosive work with Plosives at 0: {top:?}");
    assert_eq!(s.gain_reduction_db(), 0.0, "the last block was silence");
}

#[test]
fn more_amount_takes_more_and_range_is_the_most() {
    let (x, at) = ah_s_ah(0.08);
    let cut_at = |amount: f32, range: f32| {
        let mut s = sift(&[("sibAmount", amount), ("sibRange", range), ("ploAmount", 0.0)]);
        let (y, _) = run(&mut s, &x, 512);
        rms_db(&x[at[2].clone()]) - rms_db(&y[at[2].clone()])
    };
    let (a, b, c) = (cut_at(25.0, 30.0), cut_at(50.0, 30.0), cut_at(100.0, 30.0));
    assert!(a < b && b < c, "{a:.1} < {b:.1} < {c:.1}");
    assert!(c > 12.0, "all the way up is firm: {c:.1} dB");
    let capped = cut_at(100.0, 6.0);
    assert!(capped > 4.5 && capped < 6.3, "Range 6 dB holds it: {capped:.1} dB");
}

#[test]
fn a_quiet_take_is_treated_like_a_loud_one() {
    let (x, at) = ah_s_ah(0.08);
    let cut_of = |gain: f32| {
        let scaled: Vec<f32> = x.iter().map(|v| v * gain).collect();
        let mut s = sift(&[("sibAmount", 60.0), ("ploAmount", 0.0)]);
        let (y, _) = run(&mut s, &scaled, 512);
        rms_db(&scaled[at[2].clone()]) - rms_db(&y[at[2].clone()])
    };
    let (loud, quiet) = (cut_of(4.0), cut_of(0.05));
    assert!(loud > 4.0, "{loud:.1}");
    assert!((loud - quiet).abs() < 0.5, "38 dB apart in level, {loud:.1} dB and {quiet:.1} dB of cut");
}

#[test]
fn a_bright_vowel_is_not_mistaken_for_a_sibilant() {
    // A vowel with harmonics right up to 12 kHz, falling only 6 dB an octave: bright, but still
    // far more body than top.
    let (x, at) = line(vec![vowel(ms(500.0), 180.0, 0.1, 12_000.0, 6.0), silence(ms(30.0)), ess(ms(250.0), 0.08, 9)]);
    let mut s = sift(&[("sibAmount", 60.0), ("ploAmount", 0.0)]);
    let (y, _) = run(&mut s, &x, 512);
    let top = band_db(&y[at[0].clone()], 5_000.0, 9_000.0) - band_db(&x[at[0].clone()], 5_000.0, 9_000.0);
    assert!(top > -1.0, "the vowel's top came down {top:.1} dB");
    let ess_cut = rms_db(&y[at[2].clone()]) - rms_db(&x[at[2].clone()]);
    assert!(ess_cut < -4.0, "the s after it still did: {ess_cut:.1} dB");
}

#[test]
fn split_leaves_the_voice_under_a_voiced_sibilant_and_wide_does_not() {
    // "z": a vowel and a sibilant at once.
    let len = ms(400.0);
    let z = add(&ah(len, 0.05), &ess(len, 0.12, 11));
    let (x, at) = line(vec![ah(ms(300.0), 0.1), z]);
    let moved = |mode: f32| {
        let mut s = sift(&[("sibAmount", 80.0), ("ploAmount", 0.0), ("mode", mode)]);
        let (y, _) = run(&mut s, &x, 512);
        let r = at[1].start + ms(50.0)..at[1].end;
        (
            band_db(&y[r.clone()], 200.0, 2_500.0) - band_db(&x[r.clone()], 200.0, 2_500.0),
            band_db(&y[r.clone()], 5_000.0, 9_000.0) - band_db(&x[r], 5_000.0, 9_000.0),
        )
    };
    let (body, top) = moved(0.0);
    assert!(top < -4.0 && body.abs() < 0.3, "Split: the hiss {top:.1} dB, the voice under it {body:.2} dB");
    let (body, top) = moved(1.0);
    assert!(top < -4.0 && body < -4.0, "Wide: both, {top:.1} dB and {body:.1} dB");
}

#[test]
fn a_phrase_that_starts_on_a_sibilant_is_handled_gently() {
    // No vowel has been heard, so there is nothing to measure the s against.
    let (x, at) = line(vec![ess(ms(250.0), 0.08, 13), silence(ms(30.0)), ah(ms(300.0), 0.1)]);
    let mut s = sift(&[("sibAmount", 50.0), ("ploAmount", 0.0)]);
    let (y, _) = run(&mut s, &x, 512);
    let cut = rms_db(&x[at[0].clone()]) - rms_db(&y[at[0].clone()]);
    assert!(cut > 1.0 && cut < 6.0, "{cut:.1} dB");
}

#[test]
fn a_plosive_comes_down_and_the_voice_around_it_does_not() {
    let (x, at) = line(vec![ah(ms(400.0), 0.1), silence(ms(80.0)), pop(ms(120.0), 0.5), ah(ms(400.0), 0.1)]);
    let mut s = sift(&[("sibAmount", 0.0), ("ploAmount", 60.0)]);
    let (y, top) = run(&mut s, &x, 512);
    let thump = at[2].start..at[2].start + ms(60.0);
    let cut = rms_db(&y[thump.clone()]) - rms_db(&x[thump]);
    assert!(cut < -6.0, "the thump came down {cut:.1} dB");
    for i in [0, 3] {
        let r = at[i].start + if i == 3 { ms(120.0) } else { 0 }..at[i].end;
        let d = rms_db(&y[r.clone()]) - rms_db(&x[r]);
        assert!(d.abs() < 0.3, "vowel {i} moved {d:.2} dB");
    }
    assert!(top[1] > 6.0 && top[3] > 0.6 && top[0] == 0.0, "{top:?}");
}

#[test]
fn a_low_voice_and_a_steady_rumble_are_not_plosives() {
    // A bass voice: its fundamental is in the low band all the time.
    let low_voice = vowel(ms(1500.0), 85.0, 0.1, 5_000.0, 12.0);
    let mut s = sift(&[("sibAmount", 0.0), ("ploAmount", 100.0)]);
    let (y, _) = run(&mut s, &low_voice, 512);
    let d = band_db(&y, 40.0, 200.0) - band_db(&low_voice, 40.0, 200.0);
    assert!(d.abs() < 1.0, "the fundamental moved {d:.2} dB");
    // (Its first few cycles from dead silence may be caught at Plosives 100 %; once it is going,
    // nothing is.)
    let going = ms(300.0)..low_voice.len();
    let d = band_db(&y[going.clone()], 40.0, 200.0) - band_db(&low_voice[going], 40.0, 200.0);
    assert!(d.abs() < 0.05, "once going, the fundamental moved {d:.3} dB");
    // Rumble under a voice: at its start it is a jump in the lows (and may be caught as one);
    // once it has gone on for a second it is how the room sounds.
    let len = ms(3000.0);
    let rumble: Vec<f32> = (0..len).map(|i| 0.2 * (std::f32::consts::TAU * 50.0 * i as f32 / SR).sin()).collect();
    let x = add(&vowel(len, 140.0, 0.03, 5_000.0, 12.0), &rumble);
    let mut s = sift(&[("sibAmount", 0.0), ("ploAmount", 100.0)]);
    let (y, _) = run(&mut s, &x, 512);
    let late = ms(1500.0)..len;
    let d = band_db(&y[late.clone()], 30.0, 120.0) - band_db(&x[late], 30.0, 120.0);
    assert!(d.abs() < 0.3, "the rumble, a second and a half in, moved {d:.2} dB");
}

#[test]
fn listen_plays_each_part_alone() {
    let (x, at) =
        line(vec![ah(ms(300.0), 0.1), silence(ms(80.0)), pop(ms(120.0), 0.5), ah(ms(300.0), 0.1), silence(ms(30.0)), ess(ms(250.0), 0.08, 15)]);
    let knobs = [("sibAmount", 60.0), ("ploAmount", 60.0)];
    let listen = |mode: f32| {
        let mut s = sift(&knobs);
        s.set_param("listen", mode);
        run(&mut s, &x, 512).0
    };
    let (treated, sib, plo, removed) = (listen(0.0), listen(1.0), listen(2.0), listen(3.0));
    let (vowel_r, pop_r, ess_r) = (at[0].clone(), at[2].start..at[2].start + ms(60.0), at[5].start + ms(20.0)..at[5].end - ms(20.0));
    // Sibilance: the s nearly whole, the vowel and the thump nearly nothing.
    assert!(rms_db(&sib[ess_r.clone()]) > rms_db(&x[ess_r.clone()]) - 1.5);
    assert!(rms_db(&sib[vowel_r.clone()]) < rms_db(&x[vowel_r.clone()]) - 40.0);
    assert!(rms_db(&sib[pop_r.clone()]) < rms_db(&x[pop_r.clone()]) - 40.0);
    // Plosives: the thump mostly, the vowel and the s nearly nothing.
    assert!(rms_db(&plo[pop_r.clone()]) > rms_db(&x[pop_r.clone()]) - 8.0);
    assert!(rms_db(&plo[vowel_r.clone()]) < rms_db(&x[vowel_r.clone()]) - 40.0);
    assert!(rms_db(&plo[ess_r.clone()]) < rms_db(&x[ess_r]) - 40.0);
    // Removed is the input less the treated voice.
    let err = (0..x.len()).map(|i| (x[i] - treated[i] - removed[i]).abs()).fold(0.0f32, f32::max);
    assert!(err < 1e-4, "{err}");
    assert!(removed[vowel_r.clone()].iter().all(|v| v.abs() < 1e-3), "nothing is removed from a vowel");
    assert!(rms_db(&removed[pop_r.clone()]) > rms_db(&x[pop_r]) - 6.0, "most of a thump is");
}

#[test]
fn mono_gets_the_same_treatment_and_a_silent_side_stays_silent() {
    let (x, _) = ah_s_ah(0.08);
    let knobs = [("sibAmount", 60.0), ("ploAmount", 60.0)];
    let mut st = sift(&knobs);
    let (y2, _) = run(&mut st, &x, 512);
    let mut mono = Sift::new();
    mono.prepare(SR, 512, 1);
    for (id, v) in knobs {
        mono.set_param(id, v);
    }
    let lat = mono.latency();
    let mut io = vec![x.iter().copied().chain(std::iter::repeat_n(0.0, lat)).collect::<Vec<f32>>()];
    let total = io[0].len();
    mono.process(&mut io, total);
    let err = (0..x.len()).map(|i| (io[0][i + lat] - y2[i]).abs()).fold(0.0f32, f32::max);
    assert!(err < 1e-4, "{err}");
    let mut s = sift(&knobs);
    let mut io = vec![x.clone(), vec![0.0; x.len()]];
    let len = x.len();
    s.process(&mut io, len);
    assert!(io[1].iter().all(|v| v.abs() < 1e-5));
}

#[test]
fn silence_hostile_values_and_odd_calls_are_harmless() {
    let mut s = sift(&[("sibAmount", 100.0), ("ploAmount", 100.0)]);
    let (y, top) = run(&mut s, &silence(24_000), 512);
    assert!(y.iter().all(|v| *v == 0.0) && top == [0.0; 4]);
    let mut io = vec![vec![f32::NAN; 4096], vec![f32::INFINITY; 4096]];
    s.process(&mut io, 4096);
    s.process(&mut io, 4096);
    assert!(io.iter().flatten().all(|v| v.is_finite()));
    for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1e30, 1e30] {
        for p in &SIFT_PARAMS {
            assert!(s.set_param(p.id, v));
        }
        let (x, _) = ah_s_ah(0.08);
        let mut io = vec![x.clone(), x];
        let len = io[0].len();
        s.process(&mut io, len);
        assert!(io.iter().flatten().all(|v| v.is_finite()));
    }
    // The sibilant limits the wrong way round, or on top of each other, still make a band.
    let mut d = SiftDsp::new();
    d.prepare(f64::NAN, 9);
    assert_eq!(d.latency_samples(), N);
    d.set(Settings { sib_amount: 1.0, sib_lo_hz: 9_000.0, sib_hi_hz: 5_000.0, ..Settings::default() });
    let mut io = vec![ess(8192, 0.1, 17), ess(8192, 0.1, 18)];
    d.process(&mut io, 8192);
    assert!(io.iter().flatten().all(|v| v.is_finite()));
    assert!(d.take_meters().sibilant > 0.9);
    // Unprepared, it does nothing and does not panic.
    let mut raw = SiftDsp::new();
    let mut io = vec![vec![0.5f32; 64]];
    raw.process(&mut io, 64);
    assert!(io[0].iter().all(|v| *v == 0.5));
}

#[test]
fn reset_forgets_the_voice_and_keeps_the_knobs() {
    let (x, _) = ah_s_ah(0.08);
    let mut s = sift(&[("sibAmount", 70.0)]);
    let (first, _) = run(&mut s, &x, 512);
    s.reset();
    let mut quiet = vec![silence(4096), silence(4096)];
    s.process(&mut quiet, 4096);
    assert!(quiet.iter().flatten().all(|v| *v == 0.0), "nothing left in the frame");
    s.reset();
    let (again, _) = run(&mut s, &x, 512);
    let err = first.iter().zip(&again).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(err < 1e-6, "the same line twice is treated the same: {err}");
    assert_eq!(s.param("sibAmount"), Some(70.0));
}

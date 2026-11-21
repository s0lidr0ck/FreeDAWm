//! What a small correction must sound like: a sung vowel (one glottal-like pulse per period, every
//! harmonic at a vowel's resonances, 5.5 Hz vibrato) moved by 30 cents keeps each period's
//! waveform shape, its formants, its vibrato and its stereo image, and has no buzz at the
//! vocoder's frame rate. The period shape, the stereo image and the end of a move mid-phrase
//! failed before the fix (renders of 0.10.0); formants, vibrato and frame-rate buzz were already
//! fine and are kept as guards. The measured numbers are in the asserts' comments.

mod common;

use common::no_cancel;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use soundcraft_vocal::resynth::{render_all, stft_sizes};
use soundcraft_vocal::{EditCurves, PitchTrack, track_pitch};
use std::f64::consts::{PI, TAU};

const SR: u32 = 48_000;
/// Vowel /a/ resonances (Hz, bandwidth Hz).
const FORMANTS: [(f64, f64); 4] = [(730.0, 90.0), (1090.0, 110.0), (2440.0, 160.0), (3400.0, 250.0)];
const VIB_HZ: f64 = 5.5;

/// The vowel's response at `f` Hz: a cascade of two-pole resonances (minimum phase) over a
/// −9 dB/octave glottal tilt.
fn response(f: f64) -> Complex<f64> {
    let s = Complex::new(0.0, TAU * f);
    let mut h = Complex::new(1.0, 0.0);
    for (fc, bw) in FORMANTS {
        let p = Complex::new(-PI * bw, TAU * fc);
        h *= (p * p.conj()) / ((s - p) * (s - p.conj()));
    }
    h * 10f64.powf(-9.0 * (f.max(100.0) / 100.0).log2() / 20.0)
}

/// f0 (Hz) at time `t` of a note at `base` Hz with `vib` cents of vibrato.
fn f0_at(t: f64, base: f64, vib: f64) -> f64 {
    base * 2f64.powf(vib * (TAU * VIB_HZ * t).sin() / 1200.0)
}

/// A sung vowel: `secs` at `base` Hz with `vib` cents of 5.5 Hz vibrato, harmonics up to 8 kHz at
/// the vowel's response (amplitude and phase), 30 ms fades, peak 0.3.
fn sung(secs: f64, base: f64, vib: f64) -> Vec<f32> {
    sung_breathy(secs, base, vib, 0.0)
}

/// [`sung`] with breath: high-passed noise pulsing with the glottal cycle, at `breath` times the
/// voice's RMS.
fn sung_breathy(secs: f64, base: f64, vib: f64, breath: f64) -> Vec<f32> {
    sung_line(secs, |t| f0_at(t, base, vib), breath)
}

/// A sung vowel whose fundamental follows `f0_of(t)` (Hz at `t` seconds), with breath as in
/// [`sung_breathy`].
fn sung_line(secs: f64, f0_of: impl Fn(f64) -> f64, breath: f64) -> Vec<f32> {
    let n = (secs * f64::from(SR)) as usize;
    // The response on a 1 Hz grid (linearly interpolated).
    let table: Vec<Complex<f64>> = (0..=8_100).map(|f| response(f as f64)).collect();
    let at = |f: f64| {
        let i = f.floor() as usize;
        let (a, b) = (table[i.min(8_100)], table[(i + 1).min(8_100)]);
        a + (b - a) * (f - i as f64)
    };
    let mut phase = 0.0f64;
    let fade = (0.03 * f64::from(SR)) as usize;
    let mut voice = Vec::with_capacity(n);
    let mut pulse = Vec::with_capacity(n);
    for i in 0..n {
        let f0 = f0_of(i as f64 / f64::from(SR));
        let mut v = 0.0;
        let mut k = 1;
        while f0 * k as f64 <= 8_000.0 {
            let h = at(f0 * k as f64);
            v += h.norm() * (k as f64 * phase + h.arg()).cos();
            k += 1;
        }
        voice.push(v);
        pulse.push((0.5 + 0.5 * phase.cos()).powi(2));
        phase = (phase + TAU * f0 / f64::from(SR)) % (TAU * 1e6);
    }
    let rms = (voice.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
    let noise = common::noise(SR, secs as f32, 11);
    let (mut prev, mut hp) = (0.0f64, 0.0f64);
    let mut out: Vec<f64> = (0..n)
        .map(|i| {
            // One-pole high-pass at about 1.5 kHz.
            let w = f64::from(noise.get(i).copied().unwrap_or(0.0));
            hp = 0.82 * (hp + w - prev);
            prev = w;
            let e = (i.min(n - 1 - i) as f64 / fade as f64).min(1.0);
            (voice[i] + breath * rms * 6.0 * hp * pulse[i]) * (0.5 - 0.5 * (PI * e).cos())
        })
        .collect();
    let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    out.iter_mut().for_each(|v| *v *= 0.3 / peak);
    out.into_iter().map(|v| v as f32).collect()
}

/// `cents` on every voiced frame of `t` whose time is in `[a, b)` seconds.
fn shift_between(t: &PitchTrack, a: f64, b: f64, cents: f32) -> EditCurves {
    let hop = f64::from(t.hop_samples) / f64::from(SR);
    let shift = t
        .frames
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let s = i as f64 * hop;
            if s >= a && s < b && f.cents.is_some() { cents } else { 0.0 }
        })
        .collect();
    EditCurves { shift_cents: shift, gain_db: vec![0.0; t.frames.len()] }
}

/// The first `kmax` harmonics of `f0` in `x` around sample `c` (a 2048-point Hann window centred
/// there): complex amplitudes with phases about the centre.
fn harmonics(x: &[f32], c: usize, f0: f64, kmax: usize) -> Vec<Complex<f64>> {
    let n = 2048usize;
    (1..=kmax)
        .map(|k| {
            let w = TAU * k as f64 * f0 / f64::from(SR);
            (0..n)
                .map(|i| {
                    let win = 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos();
                    let t = i as f64 - (n / 2) as f64;
                    let v = f64::from(x[c + i - n / 2]) * win;
                    Complex::from_polar(v, -w * t)
                })
                .sum()
        })
        .collect()
}

fn wrap(a: f64) -> f64 {
    (a + PI).rem_euclid(TAU) - PI
}

/// Centres (samples) every 20 ms over `[a, b)` seconds.
fn centres(a: f64, b: f64) -> Vec<usize> {
    let s = f64::from(SR);
    ((a * s) as usize..(b * s) as usize).step_by(960).collect()
}

fn shifted(cents: f64) -> f64 {
    2f64.powf(cents / 1200.0)
}

fn render(x: &[f32], t: &PitchTrack, c: &EditCurves) -> Vec<f32> {
    render_all(x, t, c, &no_cancel()).unwrap()
}

/// The waveform shape of each period (the harmonics' phases relative to the fundamental's) is
/// what makes a voice sound like itself; a shifter that lets the harmonics drift apart sounds
/// phasey, hollow and robotic even when every partial lands on the right frequency.
#[test]
fn a_30_cent_move_keeps_each_periods_waveform_shape() {
    let (base, vib) = (196.0, 30.0);
    let x = sung(1.6, base, vib);
    let t = track_pitch(&x, SR, &no_cancel()).unwrap();
    for cents in [30.0, -30.0] {
        let y = render(&x, &t, &shift_between(&t, 0.0, 2.0, cents as f32));
        let r = shifted(cents);
        let mut drift = Vec::new();
        for c in centres(0.25, 1.35) {
            let f0 = f0_at(c as f64 / f64::from(SR), base, vib);
            let (hi, ho) = (harmonics(&x, c, f0, 12), harmonics(&y, c, f0 * r, 12));
            let (mut num, mut den) = (0.0, 0.0);
            for k in 1..12 {
                let m = k as f64 + 1.0;
                let d = wrap((ho[k].arg() - m * ho[0].arg()) - (hi[k].arg() - m * hi[0].arg()));
                let w = (hi[k].norm() * ho[k].norm()).sqrt();
                num += w * d * d;
                den += w;
            }
            drift.push((num / den).sqrt());
        }
        drift.sort_by(f64::total_cmp);
        let med = drift[drift.len() / 2];
        // Old render: 1.46 rad (the harmonics' phases scrambled). Now: 0.02–0.04 rad.
        assert!(med <= 0.4, "{cents:+} c: the period shape moved by {med:.2} rad (median over harmonics 2–12)");
    }
}

/// The harmonics of the moved note sit on the same vowel envelope (formants stay put).
#[test]
fn a_30_cent_move_keeps_the_formants() {
    let (base, vib) = (196.0, 30.0);
    let x = sung(1.6, base, vib);
    let t = track_pitch(&x, SR, &no_cancel()).unwrap();
    for cents in [30.0, -30.0] {
        let y = render(&x, &t, &shift_between(&t, 0.0, 2.0, cents as f32));
        let r = shifted(cents);
        let (mut dev_out, mut dev_in) = (Vec::new(), Vec::new());
        for c in centres(0.25, 1.35) {
            let f0 = f0_at(c as f64 / f64::from(SR), base, vib);
            for (sig, f, dev) in [(&x, f0, &mut dev_in), (&y, f0 * r, &mut dev_out)] {
                let h = harmonics(sig, c, f, 24);
                let d: Vec<f64> = h
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| f * (*k as f64 + 1.0) < 5_000.0)
                    .map(|(k, z)| 20.0 * (z.norm() / response(f * (k as f64 + 1.0)).norm()).log10())
                    .collect();
                let mean = d.iter().sum::<f64>() / d.len() as f64;
                dev.extend(d.iter().map(|v| (v - mean).abs()));
            }
        }
        let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let (o, i) = (avg(&dev_out), avg(&dev_in));
        // Measured: input 0.2 dB (the method's own error), old render 0.85 dB, now 0.7 dB.
        assert!(o <= i + 1.0, "{cents:+} c: harmonics {o:.2} dB off the vowel envelope (input {i:.2} dB)");
    }
}

/// Least-squares amplitude of the 5.5 Hz component of `p` (cents per 5 ms frame).
fn vibrato_depth(p: &[(f64, f64)]) -> f64 {
    let mean = p.iter().map(|v| v.1).sum::<f64>() / p.len() as f64;
    let (mut ss, mut cc, mut sc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for &(t, v) in p {
        let (s, c) = ((TAU * VIB_HZ * t).sin(), (TAU * VIB_HZ * t).cos());
        ss += s * s;
        cc += c * c;
        sc += s * c;
        ys += (v - mean) * s;
        yc += (v - mean) * c;
    }
    let det = ss * cc - sc * sc;
    let a = (ys * cc - yc * sc) / det;
    let b = (yc * ss - ys * sc) / det;
    a.hypot(b)
}

/// The singer's vibrato survives the move, and the note lands where it was sent.
#[test]
fn a_30_cent_move_keeps_the_vibrato() {
    let x = sung(1.6, 220.0, 35.0);
    let t = track_pitch(&x, SR, &no_cancel()).unwrap();
    let y = render(&x, &t, &shift_between(&t, 0.0, 2.0, 30.0));
    let ty = track_pitch(&y, SR, &no_cancel()).unwrap();
    let pick = |tr: &PitchTrack| -> Vec<(f64, f64)> {
        tr.frames
            .iter()
            .enumerate()
            .filter(|(i, _)| (50..270).contains(i))
            .filter_map(|(i, f)| Some((i as f64 * 0.005, f64::from(f.cents?))))
            .collect()
    };
    let (pi, po) = (pick(&t), pick(&ty));
    let (di, d_o) = (vibrato_depth(&pi), vibrato_depth(&po));
    assert!(di > 25.0, "the test vowel's vibrato: {di:.1} c");
    assert!((d_o / di - 1.0).abs() <= 0.1, "vibrato depth {d_o:.1} c after, {di:.1} c before");
    let mean = |p: &[(f64, f64)]| p.iter().map(|v| v.1).sum::<f64>() / p.len() as f64;
    let moved = mean(&po) - mean(&pi);
    assert!((moved - 30.0).abs() <= 3.0, "moved {moved:.1} c, not 30");
}

/// Magnitude-squared coherence between two signals over `[a, b)` samples, averaged over the
/// 100–6000 Hz bins (each bin counts the same: the breath between the harmonics as much as the
/// harmonics).
fn coherence(l: &[f32], r: &[f32], a: usize, b: usize) -> f64 {
    let n = 2048;
    let fft = FftPlanner::<f64>::new().plan_fft_forward(n);
    let k = n / 2 + 1;
    let (mut sxy, mut sxx, mut syy) = (vec![Complex::new(0.0, 0.0); k], vec![0.0; k], vec![0.0; k]);
    let mut p = a;
    while p + n <= b {
        let frame = |s: &[f32]| {
            let mut v: Vec<Complex<f64>> =
                (0..n).map(|i| Complex::new(f64::from(s[p + i]) * (0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()), 0.0)).collect();
            fft.process(&mut v);
            v
        };
        let (x, y) = (frame(l), frame(r));
        for j in 0..k {
            sxy[j] += x[j] * y[j].conj();
            sxx[j] += x[j].norm_sqr();
            syy[j] += y[j].norm_sqr();
        }
        p += n / 4;
    }
    let (mut num, mut den) = (0.0, 0.0);
    for j in 0..k {
        let f = j as f64 * f64::from(SR) / n as f64;
        if !(100.0..6_000.0).contains(&f) || sxx[j] * syy[j] <= 0.0 {
            continue;
        }
        num += sxy[j].norm_sqr() / (sxx[j] * syy[j]);
        den += 1.0;
    }
    num / den
}

/// A stereo vocal (the right channel darker and 0.25 ms later, as from a second microphone) is
/// rendered channel by channel, as the engine does: both channels must move together, or the
/// image smears and the sum to mono goes thin.
#[test]
fn a_30_cent_move_keeps_the_stereo_image() {
    let l = sung_breathy(1.6, 196.0, 30.0, 0.1);
    let mut lp = 0.0f32;
    let dark: Vec<f32> = l
        .iter()
        .map(|&v| {
            lp += 0.25 * (v - lp);
            lp
        })
        .collect();
    let r: Vec<f32> = (0..l.len()).map(|i| 0.6 * i.checked_sub(12).map_or(0.0, |j| l[j]) + 0.4 * dark[i]).collect();
    let mono: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
    let t = track_pitch(&mono, SR, &no_cancel()).unwrap();
    let c = shift_between(&t, 0.0, 2.0, 30.0);
    let (yl, yr) = (render(&l, &t, &c), render(&r, &t, &c));
    let (a, b) = (12_000, 66_000);
    let before = coherence(&l, &r, a, b);
    let after = coherence(&yl, &yr, a, b);
    assert!(before > 0.99, "input coherence {before:.3}");
    // Old render: 0.67 (each channel's partials and breath drifted on their own). Now: 0.998.
    assert!(after >= 0.95, "left/right coherence {after:.3} after the move ({before:.3} before)");
}

/// Energy (dB, relative to the harmonics) at h·f0 ± the STFT frame rate and its double: a
/// shifter that modulates at its frame rate buzzes there.
fn frame_rate_sidebands(y: &[f32], f0: f64, frame_hz: f64) -> f64 {
    let (a, b) = (24_000usize, 72_000usize);
    let n = b - a;
    let mut v: Vec<Complex<f64>> = (0..n).map(|i| Complex::new(f64::from(y[a + i]) * (0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()), 0.0)).collect();
    FftPlanner::<f64>::new().plan_fft_forward(n).process(&mut v);
    let bin_hz = f64::from(SR) / n as f64;
    let power = |f: f64, width: f64| -> f64 {
        let (lo, hi) = (((f - width) / bin_hz).ceil() as usize, ((f + width) / bin_hz).floor() as usize);
        (lo..=hi).filter_map(|j| v.get(j)).map(|z| z.norm_sqr()).sum()
    };
    let (mut harm, mut side) = (0.0, 0.0);
    let mut k = 1.0;
    while k * f0 < 8_000.0 {
        harm += power(k * f0, 4.0);
        for g in [frame_hz, 2.0 * frame_hz] {
            for f in [k * f0 - g, k * f0 + g] {
                // Only where no harmonic is near (|f − m·f0| > 6 Hz).
                if f > 50.0 && ((f / f0).round() * f0 - f).abs() > 6.0 {
                    side += power(f, 2.0);
                }
            }
        }
        k += 1.0;
    }
    10.0 * (side / harm).log10()
}

/// No buzz at the vocoder's frame rate (48000 / 256 = 187.5 Hz) on a held note.
#[test]
fn a_30_cent_move_adds_no_frame_rate_buzz() {
    let x = sung(2.0, 220.0, 0.0);
    let t = track_pitch(&x, SR, &no_cancel()).unwrap();
    let (_, hop, _) = stft_sizes(SR);
    let frame_hz = f64::from(SR) / hop as f64;
    let dry = frame_rate_sidebands(&x, 220.0, frame_hz);
    for cents in [30.0, -30.0] {
        let y = render(&x, &t, &shift_between(&t, 0.0, 3.0, cents as f32));
        let s = frame_rate_sidebands(&y, 220.0 * shifted(cents), frame_hz);
        // Old render about −70 dB, now −73 to −79 dB: far below hearing.
        assert!(s <= -60.0, "{cents:+} c: frame-rate sidebands at {s:.1} dB (input {dry:.1} dB)");
    }
}

/// RMS (dB) of `x[c − 240 .. c + 240]` (10 ms).
fn level_db(x: &[f32], c: usize) -> f64 {
    let s: f64 = x[c - 240..c + 240].iter().map(|v| f64::from(*v).powi(2)).sum();
    10.0 * (s / 480.0).log10()
}

/// A note moved and the next one, sung on in the same breath, left alone: where the move ends,
/// the voice carries on without a dip, a click or a jump back in time.
#[test]
fn where_a_move_ends_mid_phrase_the_voice_carries_on() {
    let x = sung(1.8, 200.0, 0.0);
    let t = track_pitch(&x, SR, &no_cancel()).unwrap();
    let y = render(&x, &t, &shift_between(&t, 0.0, 0.7, 30.0));
    let mut worst = 0.0f64;
    for c in (int(0.55)..int(1.2)).step_by(120) {
        worst = worst.min(level_db(&y, c) - level_db(&x, c));
    }
    // Old render: a 2.6 dB dip (the phase of every partial snapped back to the input's at the
    // first unmoved frame, cancelling against the frames before it). Now 0.2 dB.
    assert!(worst >= -1.0, "the level dips by {:.1} dB where the move ends", -worst);
    // The largest sample-to-sample jump stays that of a held vowel (no click).
    let jump = |s: &[f32], a: usize, b: usize| s[a..b].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    let (jx, jy) = (jump(&x, int(0.55), int(1.2)), jump(&y, int(0.55), int(1.2)));
    assert!(jy <= jx * 1.5, "largest step {jy:.4} where the move ends (input {jx:.4})");
}

fn int(secs: f64) -> usize {
    (secs * f64::from(SR)) as usize
}

/// A tracker dropout of a frame or three inside a moved note (a rough patch in the voice) keeps
/// the note's move: the shift curve is 0 on those unvoiced frames, but the render fills them.
#[test]
fn a_dropout_inside_a_moved_note_keeps_the_move() {
    let x = sung(1.2, 200.0, 0.0);
    let mut t = track_pitch(&x, SR, &no_cancel()).unwrap();
    let mut c = shift_between(&t, 0.0, 2.0, 30.0);
    for i in 120..123 {
        t.frames[i].cents = None;
        c.shift_cents[i] = 0.0;
    }
    let y = render(&x, &t, &c);
    let ty = track_pitch(&y, SR, &no_cancel()).unwrap();
    let tx = track_pitch(&x, SR, &no_cancel()).unwrap();
    let low = (116..127).filter_map(|i| Some(ty.frames[i].cents? - tx.frames[i].cents?)).fold(f32::INFINITY, f32::min);
    // Old render: the pitch fell back to 11 c above the sung note.
    assert!(low >= 27.0, "the move dipped to {low:.1} c at the dropout");
}

/// A phrase as the Vocal Editor's Select All and drag gives it: two notes, a slide between them,
/// vibrato on both, all moved +30 c in Musical mode (`build_curves_with`, the engine's path).
/// The moved phrase must sit +30 c above the sung one all the way through, the slide included,
/// with no 5 ms step larger than the singer's own: a long move must not warble.
#[test]
fn a_phrase_moved_30_cents_stays_30_cents_up_throughout() {
    use soundcraft_vocal::{NoteEdit, NoteId, NoteMode, PitchClassSet, VocalAnalysis, build_curves_with, segment_with};
    use std::collections::BTreeMap;
    // 196 Hz for 0.9 s, an 80 ms slide, 233 Hz for 0.9 s; 5.5 Hz vibrato of 25 c throughout.
    let f0 = |t: f64| {
        let base = if t < 0.9 {
            196.0
        } else if t < 0.98 {
            let u = (t - 0.9) / 0.08;
            196.0 * (233.0f64 / 196.0).powf(u * u * (3.0 - 2.0 * u))
        } else {
            233.0
        };
        base * 2f64.powf(25.0 * (TAU * VIB_HZ * t).sin() / 1200.0)
    };
    let x = sung_line(2.0, f0, 0.05);
    let t = track_pitch(&x, SR, &no_cancel()).unwrap();
    let (notes, unvoiced) = segment_with(&t, NoteMode::Musical);
    let a = VocalAnalysis { track: t.clone(), notes, unvoiced };
    assert!(a.notes.len() >= 2, "{} notes", a.notes.len());
    let edits: BTreeMap<NoteId, NoteEdit> = a.notes.iter().map(|n| (n.id, NoteEdit { offset_cents: 30.0, ..NoteEdit::default() })).collect();
    let scale: Option<PitchClassSet> = None;
    let c = build_curves_with(&a, &edits, &scale, NoteMode::Musical);
    // The applied correction: +30 c on every voiced frame, slide included.
    for (i, (s, fr)) in c.shift_cents.iter().zip(&t.frames).enumerate() {
        if fr.cents.is_some() {
            assert!((s - 30.0).abs() < 0.5, "frame {i}: correction {s:.1} c");
        }
    }
    let y = render(&x, &t, &c);
    let ty = track_pitch(&y, SR, &no_cancel()).unwrap();
    // The exact answer: the same phrase sung 30 c higher (same breath), tracked the same way, so
    // the tracker's own lag on the slide cancels out.
    let ideal = sung_line(2.0, |s| f0(s) * shifted(30.0), 0.05);
    let ti = track_pitch(&ideal, SR, &no_cancel()).unwrap();
    // Away from the fades at the ends (60 ms).
    let (mut worst, mut step_out, mut step_in) = (0.0f32, 0.0f32, 0.0f32);
    for i in 12..t.frames.len() - 12 {
        let (Some(pi), Some(po)) = (ti.frames[i].cents, ty.frames[i].cents) else { continue };
        worst = worst.max((po - pi).abs());
        if let (Some(pi0), Some(po0)) = (ti.frames[i - 1].cents, ty.frames[i - 1].cents) {
            step_out = step_out.max((po - po0).abs());
            step_in = step_in.max((pi - pi0).abs());
        }
    }
    // 0.10.0: 8.7 c off on the slide; the first fix (a time offset re-centred by a whole period
    // now and then): 9.4 c; now 5.2 c.
    assert!(worst <= 6.5, "the moved phrase strays {worst:.1} c from the exact +30");
    assert!(step_out <= step_in + 3.0, "largest 5 ms step {step_out:.1} c (exact {step_in:.1} c)");
}

/// A moved note's tail (the frames the tracker has let go of, which the render still lets
/// through) moves with it instead of falling back to the sung pitch. (Onsets are treated alike.)
#[test]
fn a_moved_notes_tail_moves_with_it() {
    let x = sung(1.0, 200.0, 0.0);
    let mut t = track_pitch(&x, SR, &no_cancel()).unwrap();
    // The tracker lets go at 0.9 s while the voice sounds on (as on a breathy release): those
    // frames are unvoiced, so the correction curve is 0 there.
    let cut = (0.9 / 0.005) as usize;
    for f in &mut t.frames[cut..] {
        f.cents = None;
    }
    let c = shift_between(&t, 0.0, 2.0, 30.0);
    let y = render(&x, &t, &c);
    // Tracked again (the audio is still voiced there): the first frames after the tracker let
    // go keep the move.
    let (tx, ty) = (track_pitch(&x, SR, &no_cancel()).unwrap(), track_pitch(&y, SR, &no_cancel()).unwrap());
    let moved: Vec<f32> = (cut..cut + 2).filter_map(|i| Some(ty.frames[i].cents? - tx.frames[i].cents?)).collect();
    // 0.10.0: 7 and 2 c; the first fix: 19 and 14 c; now 28 and 25 c (the voiced mask still fades
    // to the input over the next 10 ms).
    assert!(moved.len() == 2 && moved.iter().all(|m| *m >= 22.0), "after the tracker let go the move fell to {moved:?} c");
}

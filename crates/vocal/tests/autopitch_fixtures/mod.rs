//! The real-time pitch-correction study's synthetic fixtures and measures, run through the real
//! Plumb module (`soundcraft_dsp`'s `autopitch`) block by block: generated sung vowels with
//! known answers (no recordings), note-level scores by the Vocal Editor's own tracker, period
//! shape and formants as `tuning_quality.rs` measures them, stereo coherence.
#![allow(dead_code, clippy::type_complexity, clippy::needless_range_loop)]

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use soundcraft_dsp::plugins::autopitch::{AutoPitch, STYLE_CUSTOM, STYLE_HARD, STYLE_NATURAL};
use soundcraft_dsp::{Plugin, Transport};
use soundcraft_vocal::{PitchTrack, track_pitch};
use std::f64::consts::{PI, TAU};
use std::sync::atomic::AtomicBool;

pub const SR: u32 = 48_000;
pub const SRF: f64 = 48_000.0;
pub const C_MAJOR: u16 = 0xAB5;
pub const VIB_HZ: f64 = 5.5;

pub fn hz_to_cents(hz: f64) -> f64 {
    6900.0 + 1200.0 * (hz / 440.0).log2()
}
pub fn cents_to_hz(c: f64) -> f64 {
    440.0 * 2f64.powf((c - 6900.0) / 1200.0)
}

const FORMANTS: [(f64, f64); 4] = [(730.0, 90.0), (1090.0, 110.0), (2440.0, 160.0), (3400.0, 250.0)];

pub fn response(f: f64) -> Complex<f64> {
    let s = Complex::new(0.0, TAU * f);
    let mut h = Complex::new(1.0, 0.0);
    for (fc, bw) in FORMANTS {
        let p = Complex::new(-PI * bw, TAU * fc);
        h *= (p * p.conj()) / ((s - p) * (s - p.conj()));
    }
    h * 10f64.powf(-9.0 * (f.max(100.0) / 100.0).log2() / 20.0)
}

#[derive(Clone, Debug)]
pub struct NoteSpec {
    pub start: f64,
    pub dur: f64,
    /// Sung centre (MIDI cents).
    pub sung: f64,
    /// The intended scale note (MIDI cents).
    pub target: f64,
    /// Vibrato depth (cents, peak), reached `vib_delay` + 0.25 s into the note.
    pub vib: f64,
    pub vib_delay: f64,
    /// Slide in from the previous note's centre (seconds; legato only).
    pub slide: f64,
    /// Slow drift amplitude (cents).
    pub drift: f64,
}

pub fn note(start: f64, dur: f64, sung: f64, target: f64, vib: f64) -> NoteSpec {
    NoteSpec { start, dur, sung, target, vib, vib_delay: 0.0, slide: 0.0, drift: 0.0 }
}

pub struct Fixture {
    pub name: String,
    pub x: Vec<Vec<f32>>,
    /// Truth per sample: f0 (Hz, 0 when silent), vibrato component (cents).
    pub f0: Vec<f64>,
    pub vibc: Vec<f64>,
    pub notes: Vec<NoteSpec>,
}

/// Renders a sung line from `notes` (legato where one note ends where the next starts).
pub fn synth(total: f64, notes: &[NoteSpec], breath: f64, bursts: &[(f64, f64)], seed: u64) -> (Vec<f32>, Vec<f64>, Vec<f64>) {
    let n = (total * SRF) as usize;
    let mut f0 = vec![0.0; n];
    let mut vibc = vec![0.0; n];
    let mut env = vec![0.0f64; n];
    for (i, nt) in notes.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| &notes[j]).filter(|p| (p.start + p.dur - nt.start).abs() < 1e-6);
        let a = (nt.start * SRF) as usize;
        let b = (((nt.start + nt.dur) * SRF) as usize).min(n);
        for s in a..b {
            let t = s as f64 / SRF;
            let u = t - nt.start;
            let mut c = nt.sung;
            if let Some(p) = prev
                && u < nt.slide
            {
                let v = u / nt.slide;
                c = p.sung + (nt.sung - p.sung) * v * v * (3.0 - 2.0 * v);
            }
            c += nt.drift * (TAU * 0.7 * t + i as f64).sin();
            let ramp = ((u - nt.vib_delay) / 0.25).clamp(0.0, 1.0);
            let v = nt.vib * ramp * (TAU * VIB_HZ * t).sin();
            f0[s] = cents_to_hz(c + v);
            vibc[s] = v;
        }
    }
    let mut s = 0;
    while s < n {
        if f0[s] == 0.0 {
            s += 1;
            continue;
        }
        let a = s;
        while s < n && f0[s] > 0.0 {
            s += 1;
        }
        let b = s;
        for k in a..b {
            let fi = ((k - a) as f64 / (0.03 * SRF)).min(1.0);
            let fo = ((b - k) as f64 / (0.04 * SRF)).min(1.0);
            env[k] = (0.5 - 0.5 * (PI * fi).cos()) * (0.5 - 0.5 * (PI * fo).cos());
        }
    }
    let table: Vec<Complex<f64>> = (0..=8_100).map(|f| response(f as f64)).collect();
    let at = |f: f64| {
        let i = f.floor() as usize;
        let (a, b) = (table[i.min(8_100)], table[(i + 1).min(8_100)]);
        a + (b - a) * (f - i as f64)
    };
    let mut phase = 0.0f64;
    let mut voice = vec![0.0f64; n];
    let mut pulse = vec![0.0f64; n];
    for i in 0..n {
        let f = f0[i];
        if f > 0.0 {
            let mut v = 0.0;
            let mut k = 1;
            while f * k as f64 <= 8_000.0 {
                let h = at(f * k as f64);
                v += h.norm() * (k as f64 * phase + h.arg()).cos();
                k += 1;
            }
            voice[i] = v * env[i];
            pulse[i] = (0.5 + 0.5 * phase.cos()).powi(2) * env[i];
            phase = (phase + TAU * f / SRF) % (TAU * 1e6);
        }
    }
    let voiced_n = env.iter().filter(|&&e| e > 0.5).count().max(1);
    let rms = (voice.iter().map(|v| v * v).sum::<f64>() / voiced_n as f64).sqrt();
    let mut rng = seed.max(1);
    let mut noise = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        ((rng >> 40) as f64 / (1u64 << 24) as f64) - 0.5
    };
    let (mut prev, mut hp) = (0.0f64, 0.0f64);
    let mut out = vec![0.0f64; n];
    for i in 0..n {
        let w = noise();
        hp = 0.82 * (hp + w - prev);
        prev = w;
        let t = i as f64 / SRF;
        let mut burst = 0.0;
        for &(bs, bd) in bursts {
            if t >= bs && t < bs + bd {
                burst += (PI * (t - bs) / bd).sin();
            }
        }
        out[i] = voice[i] + breath * rms * 6.0 * hp * pulse[i] + burst * rms * 2.5 * hp;
    }
    let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-9);
    (out.iter().map(|v| (v * 0.3 / peak) as f32).collect(), f0, vibc)
}

fn fixture(name: &str, total: f64, notes: Vec<NoteSpec>, breath: f64, bursts: &[(f64, f64)]) -> Fixture {
    let (x, f0, vibc) = synth(total, &notes, breath, bursts, 11);
    Fixture { name: name.into(), x: vec![x], f0, vibc, notes }
}

fn line(start: f64, items: &[(f64, f64, f64, f64)], slide: f64) -> Vec<NoteSpec> {
    let mut t = start;
    let mut v = Vec::new();
    for (i, &(d, s, tg, vib)) in items.iter().enumerate() {
        let mut n = note(t, d, s, tg, vib);
        if i > 0 {
            n.slide = slide;
        }
        v.push(n);
        t += d;
    }
    v
}

fn slides_notes() -> Vec<NoteSpec> {
    line(
        0.2,
        &[
            (0.6, 6015.0, 6000.0, 20.0),
            (0.6, 6185.0, 6200.0, 20.0),
            (0.6, 6420.0, 6400.0, 20.0),
            (0.6, 6680.0, 6700.0, 20.0),
            (0.6, 6410.0, 6400.0, 20.0),
            (0.8, 5990.0, 6000.0, 20.0),
        ],
        0.12,
    )
}

/// One fixture by name (the study's set), all in C major.
pub fn by_name(name: &str) -> Option<Fixture> {
    Some(match name {
        // A3 sung 40 c flat, plain, then with 30 c vibrato.
        "flat40" => fixture("flat40", 2.6, vec![note(0.2, 2.2, 5660.0, 5700.0, 0.0)], 0.03, &[]),
        "flat40_vib" => fixture("flat40_vib", 2.6, vec![note(0.2, 2.2, 5660.0, 5700.0, 30.0)], 0.03, &[]),
        // E4 on pitch with +-50 c vibrato at 5.5 Hz.
        "vib50" => fixture("vib50", 3.2, vec![note(0.2, 2.8, 6400.0, 6400.0, 50.0)], 0.03, &[]),
        "slides" => fixture("slides", 4.3, slides_notes(), 0.04, &[]),
        "melisma" => {
            let run =
                [7200.0, 7400.0, 7600.0, 7400.0, 7200.0, 7100.0, 6900.0, 6700.0, 6900.0, 7100.0, 7200.0, 7400.0, 7200.0, 7100.0, 6900.0, 7200.0];
            let items: Vec<(f64, f64, f64, f64)> = run
                .iter()
                .enumerate()
                .map(|(i, &c)| (if i + 1 == run.len() { 0.6 } else { 0.125 }, c + if i % 2 == 0 { 22.0 } else { -25.0 }, c, 0.0))
                .collect();
            fixture("melisma", 2.9, line(0.2, &items, 0.03), 0.04, &[])
        }
        "breathy" => fixture("breathy", 2.2, vec![note(0.4, 1.6, 5525.0, 5500.0, 15.0)], 0.3, &[(0.15, 0.3)]),
        "low_male" => fixture(
            "low_male",
            3.6,
            line(0.2, &[(0.8, 4030.0, 4000.0, 25.0), (0.8, 4270.0, 4300.0, 25.0), (0.8, 4528.0, 4500.0, 25.0), (0.8, 4775.0, 4800.0, 25.0)], 0.08),
            0.03,
            &[],
        ),
        // The wide-vibrato line: notes 30 c off with 40 c vibrato next to semitone neighbours.
        "high_female" => fixture(
            "high_female",
            3.6,
            line(0.2, &[(0.8, 7630.0, 7600.0, 40.0), (0.8, 7872.0, 7900.0, 40.0), (0.8, 8125.0, 8100.0, 40.0), (0.8, 8380.0, 8400.0, 40.0)], 0.08),
            0.03,
            &[],
        ),
        "gospel" => {
            let mut g = Vec::new();
            let mut t = 0.2;
            let push = |d: f64, s: f64, tg: f64, vib: f64, vd: f64, slide: f64, drift: f64, g: &mut Vec<NoteSpec>, t: &mut f64| {
                let mut n = note(*t, d, s, tg, vib);
                n.vib_delay = vd;
                n.slide = if g.is_empty() { 0.0 } else { slide };
                n.drift = drift;
                g.push(n);
                *t += d;
            };
            push(1.4, 6390.0, 6400.0, 55.0, 0.5, 0.0, 12.0, &mut g, &mut t);
            push(0.09, 6720.0, 6700.0, 0.0, 0.0, 0.03, 0.0, &mut g, &mut t);
            push(0.09, 6930.0, 6900.0, 0.0, 0.0, 0.03, 0.0, &mut g, &mut t);
            push(0.09, 6680.0, 6700.0, 0.0, 0.0, 0.03, 0.0, &mut g, &mut t);
            push(0.09, 6415.0, 6400.0, 0.0, 0.0, 0.03, 0.0, &mut g, &mut t);
            push(1.0, 6210.0, 6200.0, 40.0, 0.35, 0.05, 10.0, &mut g, &mut t);
            push(0.5, 6340.0, 6400.0, 20.0, 0.1, 0.18, 0.0, &mut g, &mut t);
            push(1.6, 6005.0, 6000.0, 60.0, 0.6, 0.15, 15.0, &mut g, &mut t);
            fixture("gospel", t + 0.3, g, 0.06, &[])
        }
        // Two microphones: the slides line, the right channel darker and 0.25 ms later.
        "stereo_2mic" => {
            let sl = slides_notes();
            let (l, f0, vibc) = synth(4.3, &sl, 0.08, &[], 11);
            let mut lp = 0.0f32;
            let dark: Vec<f32> = l
                .iter()
                .map(|&v| {
                    lp += 0.25 * (v - lp);
                    lp
                })
                .collect();
            let r: Vec<f32> = (0..l.len()).map(|i| 0.6 * i.checked_sub(12).map_or(0.0, |j| l[j]) + 0.4 * dark[i]).collect();
            Fixture { name: "stereo_2mic".into(), x: vec![l, r], f0, vibc, notes: sl }
        }
        _ => return None,
    })
}

pub const NAMES: [&str; 10] = ["flat40", "flat40_vib", "vib50", "slides", "melisma", "breathy", "low_male", "high_female", "gospel", "stereo_2mic"];

/// The three starting points: Gentle (= Natural), Medium, Hard (= Hard tune), as the module's
/// factory presets set them (parameter id, value).
pub fn setting(name: &str) -> Vec<(&'static str, f32)> {
    let p = soundcraft_dsp::plugins::autopitch::presets::AUTOPITCH_PRESETS.iter().find(|p| p.name == name);
    p.map(|p| p.set.to_vec()).unwrap_or_default()
}

pub const SETTINGS: [&str; 3] = ["Gentle", "Medium", "Hard"];

/// Whether a setting keeps the vibrato.
pub fn keeps_vibrato(name: &str) -> bool {
    name != "Hard"
}

/// One detection's decision as the readouts report it: (input time in samples, target cents).
#[derive(Clone, Copy, Debug)]
pub struct Est {
    pub tc: f64,
    pub target: Option<f64>,
}

/// Runs `x` through Plumb with `set` (and the key: own C major unless `set` says) in blocks
/// of `block`; returns the output aligned to the input (latency removed), the decisions (one per
/// 128-sample block when `block` is 128), the latency and the CPU seconds.
pub fn run_with(x: &[Vec<f32>], set: &[(&str, f32)], block: usize, sr: f32) -> (Vec<Vec<f32>>, Vec<Est>, usize, f64) {
    let ch = x.len();
    let len = x[0].len();
    let mut a = AutoPitch::new();
    a.set_param("key", 1.0);
    for (id, v) in set {
        assert!(a.set_param(id, *v), "{id}");
    }
    a.prepare(sr, block, ch);
    let d = a.latency();
    let total = len + d;
    let mut y = vec![Vec::with_capacity(total); ch];
    let mut io = vec![vec![0.0f32; block]; ch];
    let mut log = Vec::new();
    let half = (f64::from(sr) / 70.0).ceil() + 2.0;
    let t0 = std::time::Instant::now();
    let mut pos = 0;
    while pos < total {
        let take = block.min(total - pos);
        for c in 0..ch {
            for i in 0..take {
                io[c][i] = x[c].get(pos + i).copied().unwrap_or(0.0);
            }
        }
        a.set_transport(&Transport { playing: true, samples: pos as i64, ..Transport::default() });
        a.process(&mut io, take);
        let mut r = [f32::NAN; 8];
        a.readouts(&mut r);
        log.push(Est { tc: (pos + take) as f64 - half, target: Some(f64::from(r[2])).filter(|v| v.is_finite()) });
        for c in 0..ch {
            y[c].extend_from_slice(&io[c][..take]);
        }
        pos += take;
    }
    let secs = t0.elapsed().as_secs_f64();
    (y.into_iter().map(|v| v[d..d + len].to_vec()).collect(), log, d, secs)
}

pub fn run(x: &[Vec<f32>], set: &[(&str, f32)]) -> (Vec<Vec<f32>>, Vec<Est>, usize, f64) {
    run_with(x, set, 128, SR as f32)
}

pub fn track(x: &[f32]) -> PitchTrack {
    track_pitch(x, SR, &AtomicBool::new(false)).unwrap_or(PitchTrack { sample_rate: SR, hop_samples: 240, frames: Vec::new() })
}

#[derive(Default, Clone, Debug)]
pub struct Scores {
    pub centre_err: f64,
    pub body_rms: f64,
    pub settle_med_ms: Option<f64>,
    pub settled_pct: f64,
    pub flips_per_min: f64,
    pub vib_ret: Option<f64>,
}

/// Note-level scores of output `y` (aligned) against the fixture's intended notes (the study's
/// measures).
pub fn score(fx: &Fixture, y: &[f32], log: &[Est], keep_vib: bool) -> Scores {
    let ty = track(y);
    let hop = f64::from(ty.hop_samples) / SRF;
    let res_at = |i: usize| -> Option<f64> {
        let c = f64::from(ty.frames.get(i)?.cents?);
        let s = ((i as f64 * hop) * SRF) as usize;
        let v = if keep_vib { fx.vibc.get(s).copied().unwrap_or(0.0) } else { 0.0 };
        Some(c - v)
    };
    let (mut errs, mut rmss, mut settles) = (Vec::new(), Vec::new(), Vec::new());
    let mut settled = 0;
    for nt in &fx.notes {
        let a = ((nt.start + (0.15f64).min(nt.dur * 0.4)) / hop) as usize;
        let b = ((nt.start + nt.dur - 0.03) / hop) as usize;
        let vals: Vec<f64> = (a..b).filter_map(res_at).map(|r| r - nt.target).collect();
        if !vals.is_empty() {
            errs.push((vals.iter().sum::<f64>() / vals.len() as f64).abs());
            rmss.push((vals.iter().map(|v| v * v).sum::<f64>() / vals.len() as f64).sqrt());
        }
        let s0 = (nt.start / hop) as usize;
        let s1 = ((nt.start + nt.dur) / hop) as usize;
        let hold = 12;
        let found = (s0..s1.saturating_sub(hold))
            .find(|&k| (k..k + hold).all(|j| res_at(j).is_some_and(|r| (r - nt.target).abs() <= 10.0)))
            .map(|k| (k - s0) as f64 * hop * 1e3);
        if let Some(ms) = found {
            settles.push(ms);
            settled += 1;
        }
    }
    let mut flips = 0usize;
    let mut sung = 0.0;
    for nt in &fx.notes {
        sung += nt.dur;
        let mut prev: Option<f64> = None;
        let mut changes = 0usize;
        for e in log {
            let t = e.tc / SRF;
            if t < nt.start + nt.slide + 0.04 || t > nt.start + nt.dur - 0.02 {
                if t >= nt.start && t < nt.start + nt.slide + 0.04 && e.target.is_some() {
                    prev = e.target;
                }
                continue;
            }
            if let (Some(p), Some(tg)) = (prev, e.target)
                && (p - tg).abs() > 1.0
            {
                changes += 1;
            }
            if e.target.is_some() {
                prev = e.target;
            }
        }
        let started_wrong = log
            .iter()
            .find(|e| e.tc / SRF >= nt.start + nt.slide + 0.04 && e.target.is_some())
            .is_some_and(|e| e.target.is_some_and(|tg| (tg - nt.target).abs() > 1.0));
        flips += changes.saturating_sub(usize::from(started_wrong));
    }
    let med = |mut v: Vec<f64>| -> Option<f64> {
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        Some(v[v.len() / 2])
    };
    let vib_ret = fx.notes.iter().filter(|n| n.vib >= 25.0 && n.dur >= 1.0).max_by(|a, b| a.dur.total_cmp(&b.dur)).and_then(|nt| {
        let tx = track(&fx.x[0]);
        let a = ((nt.start + nt.vib_delay + 0.3) / hop) as usize;
        let b = ((nt.start + nt.dur - 0.05) / hop) as usize;
        let pick =
            |tr: &PitchTrack| -> Vec<(f64, f64)> { (a..b).filter_map(|i| Some((i as f64 * hop, f64::from(tr.frames.get(i)?.cents?)))).collect() };
        let (pi, po) = (pick(&tx), pick(&ty));
        if pi.len() < 20 || po.len() < 20 {
            return None;
        }
        Some(vibrato_depth(&po) / vibrato_depth(&pi))
    });
    Scores {
        centre_err: errs.iter().sum::<f64>() / errs.len().max(1) as f64,
        body_rms: rmss.iter().sum::<f64>() / rmss.len().max(1) as f64,
        settle_med_ms: med(settles),
        settled_pct: 100.0 * settled as f64 / fx.notes.len().max(1) as f64,
        flips_per_min: flips as f64 / (sung / 60.0),
        vib_ret,
    }
}

pub fn vibrato_depth(p: &[(f64, f64)]) -> f64 {
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

/// Complex amplitudes of harmonics 1..=kmax of f0 around sample c (2048 Hann).
pub fn harmonics(x: &[f32], c: usize, f0: f64, kmax: usize) -> Vec<Complex<f64>> {
    let n = 2048usize;
    (1..=kmax)
        .map(|k| {
            let w = TAU * k as f64 * f0 / SRF;
            (0..n)
                .map(|i| {
                    let win = 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos();
                    let t = i as f64 - (n / 2) as f64;
                    Complex::from_polar(f64::from(x[c + i - n / 2]) * win, -w * t)
                })
                .sum()
        })
        .collect()
}

fn wrap(a: f64) -> f64 {
    (a + PI).rem_euclid(TAU) - PI
}

/// Period shape change (median rad) and formant deviation out / in (dB) on a held vowel with
/// vibrato, as tuning_quality.rs measures them.
pub fn shape_and_formants(fx: &Fixture, y: &[f32]) -> (f64, f64, f64) {
    let x = &fx.x[0];
    let ty = track(y);
    let hop = ty.hop_samples as usize;
    let f_out = |c: usize| -> Option<f64> { ty.frames.get((c + hop / 2) / hop)?.cents.map(|v| cents_to_hz(f64::from(v))) };
    let mut drift = Vec::new();
    let (mut dev_o, mut dev_i) = (Vec::new(), Vec::new());
    for c in ((0.7 * SRF) as usize..(2.2 * SRF) as usize).step_by(960) {
        let f0 = fx.f0[c];
        let Some(fo) = f_out(c) else { continue };
        let (hi, ho) = (harmonics(x, c, f0, 12), harmonics(y, c, fo, 12));
        let (mut num, mut den) = (0.0, 0.0);
        for k in 1..12 {
            let m = k as f64 + 1.0;
            let d = wrap((ho[k].arg() - m * ho[0].arg()) - (hi[k].arg() - m * hi[0].arg()));
            let w = (hi[k].norm() * ho[k].norm()).sqrt();
            num += w * d * d;
            den += w;
        }
        drift.push((num / den).sqrt());
        for (sig, f, dev) in [(x.as_slice(), f0, &mut dev_i), (y, fo, &mut dev_o)] {
            let h = harmonics(sig, c, f, 24);
            let d: Vec<f64> = h
                .iter()
                .enumerate()
                .filter(|(k, _)| f * (*k as f64 + 1.0) < 3_000.0)
                .map(|(k, z)| 20.0 * (z.norm() / response(f * (k as f64 + 1.0)).norm()).log10())
                .collect();
            let mean = d.iter().sum::<f64>() / d.len() as f64;
            dev.extend(d.iter().map(|v| (v - mean).abs()));
        }
    }
    drift.sort_by(f64::total_cmp);
    let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    (drift.get(drift.len() / 2).copied().unwrap_or(f64::NAN), avg(&dev_o), avg(&dev_i))
}

/// Left/right magnitude-squared coherence, 100 Hz to 6 kHz, over samples `[a, b)`.
pub fn coherence(l: &[f32], r: &[f32], a: usize, b: usize) -> f64 {
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
        let f = j as f64 * SRF / n as f64;
        if !(100.0..6_000.0).contains(&f) || sxx[j] * syy[j] <= 0.0 {
            continue;
        }
        num += sxy[j].norm_sqr() / (sxx[j] * syy[j]);
        den += 1.0;
    }
    num / den
}

/// Style values re-exported for the tests' wording.
pub const NATURAL: f32 = STYLE_NATURAL as f32;
pub const HARD: f32 = STYLE_HARD as f32;
pub const CUSTOM: f32 = STYLE_CUSTOM as f32;

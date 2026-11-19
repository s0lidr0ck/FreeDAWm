//! The preset lab: renders the three built-in synths' factory presets (Burley, Cicada,
//! Steeple) over a standard set of notes and states and measures what comes out, so the
//! factory suite can be designed by measurement (nothing in this program can be listened to;
//! the presets are tuned by ear later).
//!
//! Used by the suite's tests (`plugins::preset_suite`) and by the report
//! (`cargo run --release -p soundcraft-dsp --example preset_report`), which prints a line per
//! preset and a summary. Everything here is analysis code that runs off the audio thread.
//!
//! What a preset is measured on:
//! - **loudness**: the loudest 400 ms of a held chord (C3 G3 C4 E4) at velocity 100, as RMS of
//!   both sides (target about −12 dBFS);
//! - **peaks**: low, middle and high chords, soft and hard, held and short, with the macros as
//!   saved, all down and all up, the mod wheel at 0 and full (target under −1 dBFS in every one);
//! - **DC**, **silence**, **stereo width** (side against mid), **attack** (to 90 % of the
//!   held chord's loudest), **release tail** (a short note's fall to −60 dB under its peak);
//! - **brightness**: the spectral centroid over a held middle C and how far it moves (octaves);
//! - **aliasing**: a high note rendered at 48 kHz against the same note at 192 kHz: energy in
//!   the 48 kHz render where the 192 kHz one has none is what folded back;
//! - **pitch**: the lowest strong partial of a held middle C against the played note;
//! - **cost**: the held chord's render time against Init's.

use crate::plugins::fs::{FsPreset, factory_presets};
use crate::plugins::synth_mod::{MACRO_PARAMS, SlotSpec, TargetSpec, compile};
use rustfft::{FftPlanner, num_complex::Complex};

/// The synths the lab measures.
pub const SYNTHS: [&str; 3] = ["subtractive_synth", "wavetable_synth", "fm_synth"];

/// The rate everything is rendered at (but the aliasing reference).
pub const RATE: f32 = 48_000.0;

/// One factory preset as the menu recalls it.
#[derive(Debug, Clone)]
pub struct Preset {
    pub plugin: &'static str,
    pub name: String,
    pub group: String,
    /// "Simple", "Expected", "Special" or "Wild" (empty when the JSON has none).
    pub tier: String,
    pub desc: String,
    /// Every parameter the preset sets.
    pub values: Vec<(String, f32)>,
    /// The modulation patch compiled for the synth (empty: none).
    pub prepared: Vec<f64>,
    /// The macros the preset names: (name, targets).
    pub macros: Vec<(String, usize)>,
    pub slots: usize,
}

impl Preset {
    /// A preset's value of parameter `id`.
    pub fn value(&self, id: &str) -> Option<f32> {
        self.values.iter().find(|(k, _)| k == id).map(|(_, v)| *v)
    }
}

/// A factory preset's JSON for `plugin` as a [`Preset`]; `None` if it does not read.
pub fn parse(plugin: &'static str, json: &str) -> Option<Preset> {
    let info = crate::plugin_info(plugin)?;
    let p = FsPreset::parse(json, info)?;
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let m = v.get("modulation");
    let mut slots: Vec<(String, String, f32)> = Vec::new();
    for s in m.and_then(|m| m.get("slots")).and_then(|s| s.as_array()).into_iter().flatten() {
        let source = s.get("source").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let dest = s.get("dest").and_then(|x| x.as_str()).unwrap_or("").to_string();
        slots.push((source, dest, s.get("amount").and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32));
    }
    let mut macros = Vec::new();
    let mut targets: Vec<(usize, String, f32, f32)> = Vec::new();
    for (i, mac) in m.and_then(|m| m.get("macros")).and_then(|s| s.as_array()).into_iter().flatten().enumerate() {
        let ts = mac.get("targets").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        macros.push((mac.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(), ts.len()));
        for t in ts {
            let num = |k: &str| t.get(k).and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
            targets.push((i, t.get("dest").and_then(|x| x.as_str()).unwrap_or("").to_string(), num("from"), num("to")));
        }
    }
    let slot_specs: Vec<SlotSpec<'_>> = slots.iter().map(|(s, d, a)| SlotSpec { source: s, dest: d, amount: *a }).collect();
    let target_specs: Vec<TargetSpec<'_>> = targets.iter().map(|(i, d, f, t)| TargetSpec { macro_index: *i, dest: d, from: *f, to: *t }).collect();
    let prepared = if slot_specs.is_empty() && target_specs.is_empty() { Vec::new() } else { compile(plugin, &slot_specs, &target_specs)? };
    Some(Preset {
        plugin,
        name: p.name,
        group: p.group,
        tier: text("tier"),
        desc: text("desc"),
        values: p.values.into_iter().collect(),
        prepared,
        macros,
        slots: slots.len(),
    })
}

/// `plugin`'s factory presets in menu order (those that read).
pub fn presets(plugin: &'static str) -> Vec<Preset> {
    factory_presets(plugin).iter().filter_map(|(_, json)| parse(plugin, json)).collect()
}

/// `plugin` at its defaults, as a preset named "defaults".
pub fn defaults(plugin: &'static str) -> Option<Preset> {
    let info = crate::plugin_info(plugin)?;
    Some(Preset {
        plugin,
        name: "defaults".into(),
        group: String::new(),
        tier: String::new(),
        desc: String::new(),
        values: info.params.iter().map(|p| (p.id.to_string(), p.default)).collect(),
        prepared: Vec::new(),
        macros: Vec::new(),
        slots: 0,
    })
}

/// Where the macros are for a render.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Macros {
    /// As the preset saves them.
    Saved,
    /// All four at 0.
    Down,
    /// All four at 100 %.
    Up,
    /// One macro (0-based) at a value (0..100), the others as saved.
    One(usize, f32),
}

/// The state a render plays in: the macros and the mod wheel (0..1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    pub macros: Macros,
    pub wheel: f32,
}

impl State {
    pub const SAVED: State = State { macros: Macros::Saved, wheel: 0.0 };
}

/// A note: starts at frame `at`, held `len` frames, key and velocity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    pub at: usize,
    pub len: usize,
    pub key: u8,
    pub vel: u8,
}

/// The chord the loudness is measured on (C3 G3 C4 E4).
pub const MID_CHORD: [u8; 4] = [48, 55, 60, 64];
/// A low chord (C2 G2 C3).
pub const LOW_CHORD: [u8; 3] = [36, 43, 48];
/// A high chord (C5 E5 G5 C6).
pub const HIGH_CHORD: [u8; 4] = [72, 76, 79, 84];

/// `keys` together at `vel`, held `len` frames from frame 0.
pub fn chord(keys: &[u8], vel: u8, len: usize) -> Vec<Note> {
    keys.iter().map(|&key| Note { at: 0, len, key, vel }).collect()
}

/// Seconds as frames at [`RATE`].
pub fn secs(s: f32) -> usize {
    (s * RATE) as usize
}

/// `p` playing `notes` for `frames` at `rate` in `state`: (left, right). `None` if the synth
/// cannot be made.
pub fn render_at(p: &Preset, rate: f32, state: State, notes: &[Note], frames: usize) -> Option<[Vec<f32>; 2]> {
    const BLOCK: usize = 256;
    let mut s = crate::create(p.plugin)?;
    s.prepare(rate, BLOCK, 2);
    s.set_offline(true);
    for (k, v) in &p.values {
        s.set_param(k, *v);
    }
    match state.macros {
        Macros::Saved => {}
        Macros::Down | Macros::Up => {
            let v = if state.macros == Macros::Up { 100.0 } else { 0.0 };
            for m in MACRO_PARAMS {
                s.set_param(m, v);
            }
        }
        Macros::One(i, v) => {
            if let Some(m) = MACRO_PARAMS.get(i) {
                s.set_param(m, v);
            }
        }
    }
    if !p.prepared.is_empty() {
        s.set_prepared_state(&p.prepared);
    }
    // Frames are counted at 48 kHz in `notes`; scaled to `rate`.
    let scale = f64::from(rate) / f64::from(RATE);
    let at = |f: usize| (f as f64 * scale).round() as usize;
    let total = at(frames);
    let mut ev: Vec<(usize, bool, u8, u8)> = Vec::with_capacity(notes.len() * 2);
    for n in notes {
        ev.push((at(n.at), true, n.key, n.vel));
        ev.push((at(n.at + n.len.max(1)), false, n.key, 0));
    }
    ev.sort_by_key(|e| (e.0, e.1));
    let mut out = [Vec::with_capacity(total), Vec::with_capacity(total)];
    let mut io = vec![vec![0.0f32; BLOCK]; 2];
    let wheel = (state.wheel.clamp(0.0, 1.0) * 127.0).round() as u8;
    if wheel > 0 {
        s.midi(0, [0xB0, 1, wheel]);
    }
    // 50 ms of silence first, so the synth's smoothed controls (its level among them) have
    // settled where the preset puts them, as they have by the time a loaded preset is played.
    for _ in 0..(rate * 0.05) as usize / BLOCK + 1 {
        s.process(&mut io, BLOCK);
    }
    io.iter_mut().for_each(|c| c.iter_mut().for_each(|x| *x = 0.0));
    let mut next = 0;
    let mut t = 0;
    while t < total {
        let n = BLOCK.min(total - t);
        while let Some(&(when, on, key, vel)) = ev.get(next) {
            if when >= t + n {
                break;
            }
            if on {
                s.note_on(when - t, key, vel);
            } else {
                s.note_off(when - t, key);
            }
            next += 1;
        }
        io.iter_mut().for_each(|c| c.iter_mut().for_each(|x| *x = 0.0));
        s.process(&mut io, n);
        for (o, c) in out.iter_mut().zip(&io) {
            o.extend_from_slice(c.get(..n).unwrap_or(&[]));
        }
        t += n;
    }
    Some(out)
}

/// [`render_at`] at [`RATE`].
pub fn render(p: &Preset, state: State, notes: &[Note], frames: usize) -> Option<[Vec<f32>; 2]> {
    render_at(p, RATE, state, notes, frames)
}

/// Amplitude as dB (floored at −200).
pub fn db(x: f32) -> f32 {
    20.0 * x.max(1e-10).log10()
}

/// RMS of `x`.
pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len() as f64).sqrt() as f32
}

/// RMS of both sides together.
pub fn rms2(x: &[Vec<f32>; 2], from: usize, to: usize) -> f32 {
    let a = x[0].get(from..to.min(x[0].len())).unwrap_or(&[]);
    let b = x[1].get(from..to.min(x[1].len())).unwrap_or(&[]);
    ((rms(a).powi(2) + rms(b).powi(2)) / 2.0).sqrt()
}

/// Largest absolute sample of either side (infinite if any is not finite).
pub fn peak(x: &[Vec<f32>; 2]) -> f32 {
    x.iter().flatten().fold(0.0f32, |m, v| if v.is_finite() { m.max(v.abs()) } else { f32::INFINITY })
}

/// The loudest 400 ms (100 ms steps) as RMS of both sides, dBFS.
pub fn loudness_db(x: &[Vec<f32>; 2]) -> f32 {
    let (w, hop) = (secs(0.4), secs(0.1));
    let n = x[0].len();
    if n <= w {
        return db(rms2(x, 0, n));
    }
    (0..=(n - w) / hop).map(|i| rms2(x, i * hop, i * hop + w)).fold(0.0f32, f32::max).max(1e-10).log10() * 20.0
}

/// The larger DC offset of the two sides, dBFS.
pub fn dc_db(x: &[Vec<f32>; 2]) -> f32 {
    x.iter()
        .map(|c| if c.is_empty() { 0.0 } else { (c.iter().map(|v| f64::from(*v)).sum::<f64>() / c.len() as f64).abs() as f32 })
        .fold(0.0, f32::max)
        .max(1e-10)
        .log10()
        * 20.0
}

/// Side against mid RMS (0: mono; 1: as much side as mid).
pub fn width(x: &[Vec<f32>; 2]) -> f32 {
    let mid: Vec<f32> = x[0].iter().zip(&x[1]).map(|(a, b)| 0.5 * (a + b)).collect();
    let side: Vec<f32> = x[0].iter().zip(&x[1]).map(|(a, b)| 0.5 * (a - b)).collect();
    rms(&side) / rms(&mid).max(1e-9)
}

/// The RMS of both sides in 10 ms frames.
pub fn envelope(x: &[Vec<f32>; 2]) -> Vec<f32> {
    let w = secs(0.01);
    (0..x[0].len() / w).map(|i| rms2(x, i * w, (i + 1) * w)).collect()
}

/// Milliseconds until the envelope first reaches half its largest value (−6 dB: a held
/// chord's beating seldom dips that far, so a steady sound reads as fast).
pub fn attack_ms(env: &[f32]) -> f32 {
    let top = env.iter().copied().fold(0.0f32, f32::max);
    env.iter().position(|v| *v >= 0.5 * top).map_or(0.0, |i| i as f32 * 10.0)
}

/// Milliseconds after frame `off` (a note-off) until the envelope stays more than 60 dB under
/// its peak (the render's length if it never does).
pub fn tail_ms(env: &[f32], off: usize) -> f32 {
    let top = env.iter().copied().fold(0.0f32, f32::max);
    let floor = top * 1e-3;
    let start = off / secs(0.01);
    let last = env.iter().rposition(|v| *v > floor).unwrap_or(0);
    last.saturating_sub(start) as f32 * 10.0
}

/// Hann-windowed power spectrum of `x` (zero-padded or cut to `n`), `n / 2 + 1` bins.
pub fn power(x: &[f32], n: usize) -> Vec<f64> {
    let fft = FftPlanner::<f64>::new().plan_fft_forward(n);
    let mut buf: Vec<Complex<f64>> = (0..n)
        .map(|i| {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            Complex::new(f64::from(x.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0)) * w, 0.0)
        })
        .collect();
    fft.process(&mut buf);
    buf.iter().take(n / 2 + 1).map(Complex::norm_sqr).collect()
}

/// The two sides summed to one.
pub fn mono(x: &[Vec<f32>; 2]) -> Vec<f32> {
    x[0].iter().zip(&x[1]).map(|(a, b)| 0.5 * (a + b)).collect()
}

/// The spectral centroid (Hz, up to 20 kHz, weighted by magnitude) of `x` at `rate`; `None`
/// when it is silent.
pub fn centroid(x: &[f32], rate: f32) -> Option<f32> {
    let n = x.len().next_power_of_two().max(64);
    let p = power(x, n);
    let hz = |k: usize| k as f64 * f64::from(rate) / n as f64;
    let (mut s, mut w) = (0.0, 0.0);
    for (k, v) in p.iter().enumerate().skip(1) {
        if hz(k) > 20_000.0 {
            break;
        }
        let m = v.sqrt();
        s += m * hz(k);
        w += m;
    }
    (w > 1e-9).then(|| (s / w) as f32)
}

/// The centroid of `x` in 2048-frame steps (silent frames left out).
pub fn centroid_track(x: &[f32]) -> Vec<f32> {
    x.chunks_exact(2048).filter(|c| rms(c) > 1e-4).filter_map(|c| centroid(c, RATE)).collect()
}

/// Octaves between the lowest and highest centroid in `track`.
pub fn movement_oct(track: &[f32]) -> f32 {
    let lo = track.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = track.iter().copied().fold(0.0f32, f32::max);
    if track.is_empty() || lo <= 0.0 { 0.0 } else { (hi / lo).log2() }
}

/// What folded back, dB against everything: `x48` (at 48 kHz) where `x192` (the same at
/// 192 kHz, the same stretch of time) has nothing within 60 dB of its loudest bin, below
/// 20 kHz.
pub fn alias_db(x48: &[f32], x192: &[f32]) -> f32 {
    let n = 8192;
    let a = power(x48, n);
    let b = power(x192, n * 4);
    let top = b.iter().take(a.len()).copied().fold(0.0f64, f64::max);
    if top <= 0.0 {
        return -200.0;
    }
    let lit: Vec<bool> = b.iter().take(a.len()).map(|v| *v > top * 1e-6).collect();
    let near = |k: usize| (k.saturating_sub(3)..=(k + 3).min(lit.len() - 1)).any(|j| lit[j]);
    let edge = (20_000.0 / (48_000.0 / n as f64)) as usize;
    let (mut folded, mut all) = (0.0, 0.0);
    for (k, v) in a.iter().enumerate().take(edge).skip(2) {
        all += v;
        if !near(k) {
            folded += v;
        }
    }
    if all <= 0.0 { -200.0 } else { (10.0 * (folded.max(1e-30) / all).log10()) as f32 }
}

/// The lowest strong partial (within 20 dB of the loudest, above 20 Hz) of `x` (at least
/// 16384 frames at 48 kHz): its frequency.
pub fn lowest_partial(x: &[f32]) -> Option<f32> {
    let n = 16384;
    let p = power(x, n);
    let top = p.iter().skip(3).copied().fold(0.0f64, f64::max);
    if top <= 0.0 {
        return None;
    }
    let bin = 48_000.0 / n as f64;
    let first = (2..p.len() - 1).find(|&k| p[k] > top * 0.01 && p[k] >= p[k - 1] && p[k] >= p[k + 1] && k as f64 * bin > 20.0)?;
    // The power-weighted middle of everything within 50 cents of it: a detuned stack's copies
    // and a tremolo's sidebands sit around the line they belong to.
    let (lo, hi) = (((first as f64) * 0.9715) as usize, ((first as f64) * 1.0293).ceil() as usize);
    let (mut s, mut w) = (0.0, 0.0);
    for (k, v) in p.iter().enumerate().take(hi.min(p.len() - 1) + 1).skip(lo.max(1)) {
        s += k as f64 * v;
        w += v;
    }
    (w > 0.0).then(|| (s / w * bin) as f32)
}

/// Cents from `hz` to the nearest octave of `f0` (−600..600).
pub fn cents_off_octave(hz: f32, f0: f32) -> f32 {
    let c = 1200.0 * (hz / f0).log2();
    c - 1200.0 * (c / 1200.0).round()
}

/// Everything the report and the tests read about one preset.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub name: String,
    pub group: String,
    pub tier: String,
    /// The held middle chord at velocity 100, macros as saved, wheel at 0.
    pub loudness_db: f32,
    pub peak_db: f32,
    /// The loudest peak over every state measured, and the state it came in.
    pub worst_peak_db: f32,
    pub worst_state: String,
    /// The quietest loudness over every held state measured.
    pub quietest_db: f32,
    pub quietest_state: String,
    pub dc_db: f32,
    pub width: f32,
    pub attack_ms: f32,
    pub tail_ms: f32,
    /// Centroid of a held middle C early (the first 200 ms) and late (from 1.5 s), Hz.
    pub centroid_early: f32,
    pub centroid_late: f32,
    pub movement_oct: f32,
    /// The worse of C6 and C7.
    pub alias_db: f32,
    /// Lowest strong partial of a held middle C, Hz, and its cents from the nearest octave of
    /// the note.
    pub partial_hz: f32,
    pub pitch_cents: f32,
    /// Render time of the held chord against Init's.
    pub cost: f32,
    pub seconds: f64,
}

/// The states the full report plays: macros as saved, down and up, each with the wheel at 0
/// and full.
pub const STATES: [(State, &str); 6] = [
    (State { macros: Macros::Saved, wheel: 0.0 }, "saved"),
    (State { macros: Macros::Saved, wheel: 1.0 }, "saved+wheel"),
    (State { macros: Macros::Down, wheel: 0.0 }, "down"),
    (State { macros: Macros::Down, wheel: 1.0 }, "down+wheel"),
    (State { macros: Macros::Up, wheel: 0.0 }, "up"),
    (State { macros: Macros::Up, wheel: 1.0 }, "up+wheel"),
];

/// The full measurement of `p` (`full`: every register, velocity, length and state; else the
/// held chords at hard velocity only).
pub fn measure(p: &Preset, full: bool) -> Option<Report> {
    let started = std::time::Instant::now();
    let mut r = Report { name: p.name.clone(), group: p.group.clone(), tier: p.tier.clone(), ..Report::default() };
    let x = render(p, State::SAVED, &chord(&MID_CHORD, 100, secs(3.0)), secs(3.0))?;
    r.seconds = started.elapsed().as_secs_f64();
    r.loudness_db = loudness_db(&x);
    r.peak_db = db(peak(&x));
    r.dc_db = dc_db(&x);
    r.width = width(&x);
    r.attack_ms = attack_ms(&envelope(&x));
    r.worst_peak_db = f32::NEG_INFINITY;
    r.quietest_db = f32::INFINITY;
    let regs: [(&[u8], &str); 3] = [(&LOW_CHORD, "low"), (&MID_CHORD, "mid"), (&HIGH_CHORD, "high")];
    let vels: &[u8] = if full { &[40, 127] } else { &[127] };
    let lens: &[(usize, &str)] = if full { &[(secs(1.5), "held"), (secs(0.15), "short")] } else { &[(secs(1.5), "held")] };
    for (state, sname) in STATES {
        for (keys, rname) in regs {
            for &vel in vels {
                for &(len, lname) in lens {
                    let y = render(p, state, &chord(keys, vel, len), secs(1.5))?;
                    let pk = db(peak(&y));
                    let what = format!("{rname} {lname} vel {vel} {sname}");
                    if pk > r.worst_peak_db {
                        r.worst_peak_db = pk;
                        r.worst_state.clone_from(&what);
                    }
                    let l = loudness_db(&y);
                    if lname == "held" && l < r.quietest_db {
                        r.quietest_db = l;
                        r.quietest_state = what;
                    }
                    r.dc_db = r.dc_db.max(dc_db(&y));
                }
            }
        }
    }
    // A short middle C: its tail.
    let short = render(p, State::SAVED, &[Note { at: 0, len: secs(0.3), key: 60, vel: 100 }], secs(8.0))?;
    r.tail_ms = tail_ms(&envelope(&short), secs(0.3));
    // A held middle C: brightness, its movement and the pitch.
    let held = mono(&render(p, State::SAVED, &[Note { at: 0, len: secs(3.0), key: 60, vel: 100 }], secs(3.0))?);
    let track = centroid_track(held.get(secs(0.05)..).unwrap_or(&[]));
    r.centroid_early = centroid(held.get(secs(0.02)..secs(0.22)).unwrap_or(&[]), RATE).unwrap_or(0.0);
    r.centroid_late = centroid(held.get(secs(1.5)..secs(2.5)).unwrap_or(&[]), RATE).unwrap_or(0.0);
    r.movement_oct = movement_oct(&track);
    r.partial_hz = lowest_partial(held.get(secs(0.05)..secs(0.05) + 16384).unwrap_or(&[])).unwrap_or(0.0);
    r.pitch_cents = cents_off_octave(r.partial_hz, crate::midi_to_hz(60.0) as f32);
    r.alias_db = -200.0;
    for key in [84u8, 96] {
        let note = [Note { at: 0, len: secs(1.0), key, vel: 100 }];
        let a = mono(&render_at(p, RATE, State::SAVED, &note, secs(0.5))?);
        let b = mono(&render_at(p, RATE * 4.0, State::SAVED, &note, secs(0.5))?);
        let from = secs(0.15);
        r.alias_db = r.alias_db.max(alias_db(a.get(from..).unwrap_or(&[]), b.get(from * 4..).unwrap_or(&[])));
    }
    Some(r)
}

/// The report's header line.
pub const HEADER: &str = "preset                     group            tier      loud  peak worst  quiet   dc  width  att ms  tail ms  cent Hz  late Hz  move  alias  pitch  cost";

/// One report line.
pub fn line(r: &Report) -> String {
    format!(
        "{:<26} {:<16} {:<8} {:>5.1} {:>5.1} {:>5.1} {:>6.1} {:>4.0} {:>6.2} {:>7.0} {:>8.0} {:>8.0} {:>8.0} {:>5.2} {:>6.1} {:>6.0} {:>5.2}",
        r.name,
        r.group,
        r.tier,
        r.loudness_db,
        r.peak_db,
        r.worst_peak_db,
        r.quietest_db,
        r.dc_db,
        r.width,
        r.attack_ms,
        r.tail_ms,
        r.centroid_early,
        r.centroid_late,
        r.movement_oct,
        r.alias_db,
        r.pitch_cents,
        r.cost
    )
}

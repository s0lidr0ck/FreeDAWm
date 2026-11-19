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
    /// The patch's slots as the preset names them: (source, destination, amount).
    pub slot_list: Vec<(String, String, f32)>,
    /// The macros' targets: (macro, 0-based; destination; from; to).
    pub targets: Vec<(usize, String, f32, f32)>,
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
        slot_list: slots,
        targets,
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
        slot_list: Vec::new(),
        targets: Vec::new(),
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

/// A parameter moved during a render: from frame `at` (counted at 48 kHz, applied at the start
/// of the block it falls in) parameter `param` is `value`; with `param` [`WHEEL`], the mod wheel
/// moves to `value` (0..1) instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change {
    pub at: usize,
    pub param: &'static str,
    pub value: f32,
}

/// The [`Change`] that moves the mod wheel (CC 1) rather than a parameter.
pub const WHEEL: &str = "@wheel";

/// `p` playing `notes` for `frames` at `rate` in `state`: (left, right). `None` if the synth
/// cannot be made.
pub fn render_at(p: &Preset, rate: f32, state: State, notes: &[Note], frames: usize) -> Option<[Vec<f32>; 2]> {
    render_changing(p, rate, state, notes, frames, &[])
}

/// [`render_at`] with parameters moved while it plays (`changes`, in time order): a knob
/// turned during a held note.
pub fn render_changing(p: &Preset, rate: f32, state: State, notes: &[Note], frames: usize, changes: &[Change]) -> Option<[Vec<f32>; 2]> {
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
    let mut moved = 0;
    let mut t = 0;
    while t < total {
        let n = BLOCK.min(total - t);
        while let Some(c) = changes.get(moved) {
            if at(c.at) >= t + n {
                break;
            }
            if c.param == WHEEL {
                s.midi(0, [0xB0, 1, (c.value.clamp(0.0, 1.0) * 127.0).round() as u8]);
            } else {
                s.set_param(c.param, c.value);
            }
            moved += 1;
        }
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

// ------------------------------------------------------------------------- what a macro does

/// The octave bands a sound's level is compared in (centre frequencies, Hz).
pub const BANDS: [f32; 9] = [63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];

/// What a stretch of sound is like, for telling two apart (a macro down and up).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Traits {
    /// RMS of both sides, dBFS.
    pub loud_db: f32,
    /// Spectral centroid (Hz; 0 when silent), from the averaged spectrum.
    pub centroid: f32,
    /// Level in each of [`BANDS`], dB (relative: only differences mean anything).
    pub bands_db: [f32; 9],
    /// Amplitude modulation: the spread (standard deviation) of the 10 ms level in dB around
    /// its straight-line trend (so a steady decay is not counted), and its rate (Hz).
    pub amp_mod_db: f32,
    pub amp_rate: f32,
    /// Tone modulation: the spread of the centroid in octaves (1024-frame steps) around its
    /// trend, and its rate (Hz).
    pub tone_mod_oct: f32,
    pub tone_rate: f32,
    /// Timbre modulation: how far the level of the octave bands moves around its trend (dB, the
    /// mean of the three bands that move most): a sweep through a wavetable or a changing
    /// formant that leaves the centroid where it was.
    pub band_mod_db: f32,
    /// Side against mid (see [`width`]).
    pub width: f32,
    /// The lowest strong partial (Hz; 0: none found).
    pub pitch_hz: f32,
}

/// Welch-averaged power spectrum of `x` (8192-frame Hann frames, half overlapping).
fn averaged_power(x: &[f32]) -> Vec<f64> {
    const N: usize = 8192;
    let mut sum = vec![0.0f64; N / 2 + 1];
    let mut frames = 0usize;
    let mut at = 0;
    while at + N <= x.len().max(N) {
        let part = x.get(at..(at + N).min(x.len())).unwrap_or(&[]);
        for (s, v) in sum.iter_mut().zip(power(part, N)) {
            *s += v;
        }
        frames += 1;
        at += N / 2;
    }
    let k = 1.0 / frames.max(1) as f64;
    sum.iter_mut().for_each(|s| *s *= k);
    sum
}

/// The residual spread of `ys` around its least-squares line, and the rate (per second, at one
/// value every `step_s` seconds) it crosses that line (a quarter of its spread either side, so
/// noise does not count).
fn wobble(ys: &[f32], step_s: f32) -> (f32, f32) {
    let n = ys.len();
    if n < 8 {
        return (0.0, 0.0);
    }
    let nf = n as f64;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (i, y) in ys.iter().enumerate() {
        let (x, y) = (i as f64, f64::from(*y));
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let den = nf * sxx - sx * sx;
    let slope = if den.abs() > 1e-12 { (nf * sxy - sx * sy) / den } else { 0.0 };
    let icpt = (sy - slope * sx) / nf;
    let res: Vec<f64> = ys.iter().enumerate().map(|(i, y)| f64::from(*y) - (icpt + slope * i as f64)).collect();
    let sd = (res.iter().map(|r| r * r).sum::<f64>() / nf).sqrt();
    let h = sd * 0.25;
    let mut side = 0i32;
    let mut flips = 0usize;
    for r in &res {
        let now = if *r > h {
            1
        } else if *r < -h {
            -1
        } else {
            side
        };
        if side != 0 && now != side {
            flips += 1;
        }
        side = now;
    }
    (sd as f32, flips as f32 / 2.0 / (n as f32 * step_s))
}

/// A held note's pitch over time: `x` (at [`RATE`], a note near `hint` Hz) cut into 16 ms steps,
/// each step's fundamental found by the YIN method (within 1.3 octaves of `hint`) on a copy
/// taken down to 12 kHz; `None` for a step that is silent or has no clear period. Off the audio
/// thread (allocates).
pub fn f0_track(x: &[f32], hint: f32) -> Vec<Option<f32>> {
    const DOWN: usize = 4;
    const W: usize = 256;
    const HOP: usize = 192;
    let rate = RATE / DOWN as f32;
    let y: Vec<f32> = x.chunks_exact(DOWN).map(|c| c.iter().sum::<f32>() / DOWN as f32).collect();
    let tmin = ((rate / (hint * 2.5)).floor() as usize).max(2);
    let tmax = (rate / (hint / 2.5)).ceil() as usize;
    let mut out = Vec::new();
    let mut d = vec![0.0f32; tmax + 2];
    let mut at = 0;
    while at + W + tmax + 2 <= y.len() {
        let frame = y.get(at..at + W + tmax + 2).unwrap_or(&[]);
        let energy: f32 = frame.iter().take(W).map(|v| v * v).sum::<f32>() / W as f32;
        if energy < 1e-8 {
            out.push(None);
            at += HOP;
            continue;
        }
        for (tau, dt) in d.iter_mut().enumerate().skip(1) {
            *dt = (0..W).map(|j| frame.get(j).copied().unwrap_or(0.0) - frame.get(j + tau).copied().unwrap_or(0.0)).map(|v| v * v).sum();
        }
        // The cumulative-mean-normalised difference, and its first dip under the threshold.
        let mut run = 0.0f32;
        let mut cm = vec![1.0f32; tmax + 2];
        for tau in 1..=tmax + 1 {
            run += d[tau];
            cm[tau] = if run > 0.0 { d[tau] * tau as f32 / run } else { 1.0 };
        }
        let mut pick = None;
        let mut tau = tmin;
        while tau <= tmax {
            if cm[tau] < 0.15 {
                while tau < tmax && cm[tau + 1] < cm[tau] {
                    tau += 1;
                }
                pick = Some(tau);
                break;
            }
            tau += 1;
        }
        // No dip that deep (beating copies blur it): the deepest one, if it is clear enough.
        if pick.is_none() {
            pick = (tmin..=tmax).min_by(|a, b| cm[*a].total_cmp(&cm[*b])).filter(|t| cm[*t] < 0.35);
        }
        out.push(pick.map(|t| {
            let (a, b, c) = (cm[t - 1], cm[t], cm[t + 1]);
            let den = a - 2.0 * b + c;
            let shift = if den.abs() > 1e-9 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 };
            rate / (t as f32 + shift)
        }));
        at += HOP;
    }
    out
}

/// What a held note's pitch does (from [`f0_track`]): (its middle value in Hz, the vibrato's
/// spread in cents around the trend, the vibrato's rate in Hz); `None` when fewer than 70 % of
/// the steps found a clear period (an inharmonic or noisy sound), or the pitch jumps about.
pub fn pitch_traits(x: &[f32], hint: f32) -> Option<(f32, f32, f32)> {
    let track = f0_track(x, hint);
    let heard: Vec<f32> = track.iter().flatten().copied().collect();
    let sounding = track.len();
    if sounding < 8 || (heard.len() as f32) < 0.7 * sounding as f32 {
        return None;
    }
    let mut sorted = heard.clone();
    sorted.sort_by(f32::total_cmp);
    let mid = sorted.get(sorted.len() / 2).copied()?;
    // A step read an octave off (the tracker's usual slip) is folded back to the note's octave.
    let cents: Vec<f32> = heard.iter().map(|f| 1200.0 * (f / mid).log2()).map(|c| c - 1200.0 * (c / 1200.0).round()).collect();
    let (sd, rate) = wobble(&cents, 192.0 * 4.0 / RATE);
    // A spread of more than 150 cents even so is no steady pitch to speak of.
    if sd > 150.0 {
        return None;
    }
    Some((mid, sd, rate))
}

/// [`Traits::band_mod_db`] of `x` (mono, at [`RATE`]): each octave band's level in 2048-frame
/// steps, its spread around its trend, the three that move most averaged (bands more than 30 dB
/// under the loudest left out).
fn band_motion(x: &[f32]) -> f32 {
    const N: usize = 2048;
    let bin = f64::from(RATE) / N as f64;
    let frames: Vec<[f32; 9]> = x
        .chunks_exact(N)
        .filter(|c| rms(c) > 1e-4)
        .map(|c| {
            let p = power(c, N);
            let mut b = [0.0f64; 9];
            for (k, v) in p.iter().enumerate().skip(1) {
                let hz = k as f64 * bin;
                for (o, cf) in b.iter_mut().zip(BANDS) {
                    let cf = f64::from(cf);
                    if hz >= cf / std::f64::consts::SQRT_2 && hz < cf * std::f64::consts::SQRT_2 {
                        *o += v;
                    }
                }
            }
            b.map(|v| (10.0 * (v + 1e-20).log10()) as f32)
        })
        .collect();
    if frames.len() < 8 {
        return 0.0;
    }
    let mean = |i: usize| frames.iter().map(|f| f[i]).sum::<f32>() / frames.len() as f32;
    let means: Vec<f32> = (0..9).map(mean).collect();
    let top = means.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sds: Vec<f32> =
        (0..9).filter(|&i| means[i] >= top - 30.0).map(|i| wobble(&frames.iter().map(|f| f[i]).collect::<Vec<f32>>(), N as f32 / RATE).0).collect();
    sds.sort_by(|a, b| b.total_cmp(a));
    let n = sds.len().min(3);
    if n == 0 { 0.0 } else { sds.iter().take(n).sum::<f32>() / n as f32 }
}

/// The [`Traits`] of frames `from..to` of `x` (at [`RATE`]).
pub fn traits(x: &[Vec<f32>; 2], from: usize, to: usize) -> Traits {
    let to = to.min(x[0].len());
    let from = from.min(to);
    let part = [x[0].get(from..to).unwrap_or(&[]).to_vec(), x[1].get(from..to).unwrap_or(&[]).to_vec()];
    let m = mono(&part);
    let spec = averaged_power(&m);
    let bin = f64::from(RATE) / 8192.0;
    let (mut s, mut w) = (0.0, 0.0);
    let mut bands = [0.0f64; 9];
    for (k, v) in spec.iter().enumerate().skip(1) {
        let hz = k as f64 * bin;
        if hz > 20_000.0 {
            break;
        }
        let mag = v.sqrt();
        s += mag * hz;
        w += mag;
        for (b, c) in bands.iter_mut().zip(BANDS) {
            let c = f64::from(c);
            if hz >= c / std::f64::consts::SQRT_2 && hz < c * std::f64::consts::SQRT_2 {
                *b += v;
            }
        }
    }
    let env: Vec<f32> = envelope(&part).into_iter().filter(|v| *v > 1e-5).map(db).collect();
    let (amp_mod_db, amp_rate) = wobble(&env, 0.01);
    let tone: Vec<f32> = m.chunks_exact(1024).filter(|c| rms(c) > 1e-4).filter_map(|c| centroid(c, RATE)).map(f32::log2).collect();
    let (tone_mod_oct, tone_rate) = wobble(&tone, 1024.0 / RATE);
    Traits {
        loud_db: db(rms2(&part, 0, part[0].len())),
        centroid: if w > 1e-12 { (s / w) as f32 } else { 0.0 },
        bands_db: bands.map(|b| (10.0 * (b + 1e-20).log10()) as f32),
        amp_mod_db,
        amp_rate,
        tone_mod_oct,
        tone_rate,
        band_mod_db: band_motion(&m),
        width: width(&part),
        pitch_hz: lowest_partial(m.get(..16384.min(m.len())).unwrap_or(&[])).unwrap_or(0.0),
    }
}

/// How clearly a change from `a` to `b` is heard, as a score: 1.0 is the "clearly audible"
/// line, and the reading that scored highest. The line is crossed by any one of these (each
/// well past what a listener just notices, so the change is heard on a laptop's speakers in a
/// mix, not only in a quiet room on headphones):
/// - the brightness (centroid) moves by 25 % or more: about a third of an octave, several times
///   the few per cent at which a change of brightness is first noticed;
/// - the level in some octave band changes by 6 dB or more (bands more than 30 dB under the
///   loudest are left out, being masked): twice the 3 dB that reads as "a little louder";
/// - the overall level changes by 6 dB or more;
/// - a modulation's depth changes by a factor of two or more, where the deeper one is at least
///   1.5 dB of tremolo or a tenth of an octave of sweep (less is hardly heard as movement), or
///   its rate by a factor of two where both are half as deep again (so a rate read from a
///   barely moving sound does not count);
/// - the timbre's motion (octave bands moving around their trend, [`Traits::band_mod_db`])
///   changes by a factor of two where the larger is at least 2 dB: a sweep through a wavetable
///   or between vowels that leaves the brightness where it was;
/// - the pitch of a held note moves by a semitone or more, or its vibrato deepens by a factor of
///   two where the deeper one spreads at least 8 cents (about ±11 cents: a gentle but plain
///   vibrato) — measured by [`macro_effect`] on a single held note ([`pitch_traits`]), as a
///   chord's pitch cannot be read reliably;
/// - the stereo width changes by 30 % or more, where the wider one is at least 0.15 (side at
///   −16 dB under mid: narrower images sound mono either way);
/// - an envelope time (the attack, or the tail after a short note) changes by a factor of two
///   or more where the longer one is at least 50 ms (attack) or 150 ms (tail).
pub fn audible(a: &Traits, b: &Traits) -> (f32, String) {
    let mut best = (0.0f32, String::from("nothing measurable"));
    let mut take = |score: f32, what: String| {
        if score.is_finite() && score > best.0 {
            best = (score, what);
        }
    };
    if a.centroid > 0.0 && b.centroid > 0.0 {
        let r = b.centroid / a.centroid;
        take(r.ln().abs() / 1.25f32.ln(), format!("brightness {:.0} -> {:.0} Hz", a.centroid, b.centroid));
    }
    let top = a.bands_db.iter().chain(&b.bands_db).copied().fold(f32::NEG_INFINITY, f32::max);
    for (i, (x, y)) in a.bands_db.iter().zip(&b.bands_db).enumerate() {
        if x.max(*y) < top - 30.0 {
            continue;
        }
        take((y - x).abs() / 6.0, format!("{:.0} Hz band {:+.1} dB", BANDS[i], y - x));
    }
    take((b.loud_db - a.loud_db).abs() / 6.0, format!("level {:+.1} dB", b.loud_db - a.loud_db));
    let depth = |x: f32, y: f32, floor: f32| if x.max(y) >= floor { (x.max(y) / x.min(y).max(floor * 0.25)).ln() / 2f32.ln() } else { 0.0 };
    take(depth(a.amp_mod_db, b.amp_mod_db, 1.5), format!("tremolo {:.1} -> {:.1} dB", a.amp_mod_db, b.amp_mod_db));
    take(depth(a.tone_mod_oct, b.tone_mod_oct, 0.1), format!("sweep {:.2} -> {:.2} oct", a.tone_mod_oct, b.tone_mod_oct));
    take(depth(a.band_mod_db, b.band_mod_db, 2.0), format!("timbre motion {:.1} -> {:.1} dB", a.band_mod_db, b.band_mod_db));
    if a.amp_mod_db.min(b.amp_mod_db) >= 2.25 && a.amp_rate > 0.0 && b.amp_rate > 0.0 {
        take((b.amp_rate / a.amp_rate).ln().abs() / 2f32.ln(), format!("tremolo rate {:.1} -> {:.1} Hz", a.amp_rate, b.amp_rate));
    }
    if a.tone_mod_oct.min(b.tone_mod_oct) >= 0.15 && a.tone_rate > 0.0 && b.tone_rate > 0.0 {
        take((b.tone_rate / a.tone_rate).ln().abs() / 2f32.ln(), format!("sweep rate {:.1} -> {:.1} Hz", a.tone_rate, b.tone_rate));
    }
    if a.width.max(b.width) >= 0.15 {
        take((b.width - a.width).abs() / a.width.max(b.width) / 0.3, format!("width {:.2} -> {:.2}", a.width, b.width));
    }
    best
}

/// [`audible`] for envelope times: `a` and `b` in ms, `floor` the shortest that counts.
fn time_score(a: f32, b: f32, floor: f32) -> f32 {
    if a.max(b) < floor { 0.0 } else { (a.max(b) / a.min(b).max(floor * 0.25)).ln() / 2f32.ln() }
}

/// What turning macro `i` (0-based) of `p` from 0 to 100 % does, the other macros as saved:
/// the [`audible`] score and what scored it, over a held chord (C3 G3 C4 E4, velocity 100,
/// 2.5 s) and a short middle C (150 ms, for plucks and other sounds that die away).
pub fn macro_effect(p: &Preset, i: usize) -> Option<(f32, String)> {
    let at = |up: bool, notes: &[Note], frames: usize| {
        render(p, State { macros: Macros::One(i, if up { 100.0 } else { 0.0 }), wheel: 0.0 }, notes, frames)
    };
    compare(at, 100, 0)
}

/// How clearly two renders of the same music differ (`at(false, ..)` against `at(true, ..)`):
/// the [`audible`] score and what scored it, over a held chord (C3 G3 C4 E4 moved by `shift`
/// semitones, velocity `vel`, 2.5 s), a short note (the C in it, 150 ms) and that note held
/// (its pitch and vibrato), with the chord's attack and the short note's tail. What
/// [`macro_effect`] and [`slot_effect`] measure with.
fn compare(at: impl Fn(bool, &[Note], usize) -> Option<[Vec<f32>; 2]>, vel: u8, shift: i8) -> Option<(f32, String)> {
    let key = |k: u8| k.saturating_add_signed(shift);
    let held: Vec<Note> = MID_CHORD.iter().map(|&k| Note { at: 0, len: secs(2.5), key: key(k), vel }).collect();
    let short = [Note { at: 0, len: secs(0.15), key: key(60), vel }];
    let (h0, h1) = (at(false, &held, secs(2.5))?, at(true, &held, secs(2.5))?);
    let (s0, s1) = (at(false, &short, secs(3.0))?, at(true, &short, secs(3.0))?);
    let mut best = audible(&traits(&h0, secs(0.3), secs(2.5)), &traits(&h1, secs(0.3), secs(2.5)));
    best.1 = format!("held: {}", best.1);
    let (sc, what) = audible(&traits(&s0, 0, secs(0.6)), &traits(&s1, 0, secs(0.6)));
    if sc > best.0 {
        best = (sc, format!("short note: {what}"));
    }
    let (a0, a1) = (attack_ms(&envelope(&h0)), attack_ms(&envelope(&h1)));
    let sc = time_score(a0, a1, 50.0);
    if sc > best.0 {
        best = (sc, format!("attack {a0:.0} -> {a1:.0} ms"));
    }
    let (t0, t1) = (tail_ms(&envelope(&s0), secs(0.15)), tail_ms(&envelope(&s1), secs(0.15)));
    let sc = time_score(t0, t1, 150.0);
    if sc > best.0 {
        best = (sc, format!("tail {t0:.0} -> {t1:.0} ms"));
    }
    // The C held: its pitch and vibrato.
    let one = [Note { at: 0, len: secs(2.5), key: key(60), vel }];
    let (n0, n1) = (at(false, &one, secs(2.5))?, at(true, &one, secs(2.5))?);
    let hint = crate::midi_to_hz(f64::from(key(60))) as f32;
    let pt = |x: &[Vec<f32>; 2]| pitch_traits(mono(x).get(secs(0.3)..).unwrap_or(&[]), hint);
    if let (Some(a), Some(b)) = (pt(&n0), pt(&n1)) {
        let cents = 1200.0 * (b.0 / a.0).log2();
        let mut take = |score: f32, what: String| {
            if score.is_finite() && score > best.0 {
                best = (score, what);
            }
        };
        take(cents.abs() / 100.0, format!("pitch {cents:+.0} cents"));
        if a.1.max(b.1) >= 8.0 {
            take((a.1.max(b.1) / a.1.min(b.1).max(2.0)).ln() / 2f32.ln(), format!("vibrato {:.0} -> {:.0} cents", a.1, b.1));
        }
        if a.1.min(b.1) >= 12.0 && a.2 > 0.0 && b.2 > 0.0 {
            take((b.2 / a.2).ln().abs() / 2f32.ln(), format!("vibrato rate {:.1} -> {:.1} Hz", a.2, b.2));
        }
    }
    Some(best)
}

/// `p` with slot `i` of its patch taken out (the other slots and the macros as they are);
/// `None` if there is no such slot or the rest does not compile.
pub fn without_slot(p: &Preset, i: usize) -> Option<Preset> {
    if i >= p.slot_list.len() {
        return None;
    }
    let mut q = p.clone();
    q.slot_list.remove(i);
    q.slots = q.slot_list.len();
    let slots: Vec<SlotSpec<'_>> = q.slot_list.iter().map(|(s, d, a)| SlotSpec { source: s, dest: d, amount: *a }).collect();
    let targets: Vec<TargetSpec<'_>> = q.targets.iter().map(|(m, d, f, t)| TargetSpec { macro_index: *m, dest: d, from: *f, to: *t }).collect();
    let prepared = if slots.is_empty() && targets.is_empty() { Vec::new() } else { compile(p.plugin, &slots, &targets)? };
    q.prepared = prepared;
    Some(q)
}

/// What slot `i` (0-based) of `p`'s patch does when its source moves: `p` against `p` without
/// that slot, measured as [`macro_effect`] measures a macro (the same [`audible`] score: 1.0 is
/// the clearly-heard line), with the source at the end of the travel a player reaches:
/// - **velocity**: every note struck at 127 (without the slot, the hardest note does through it
///   what the softest would);
/// - **mod wheel**: the wheel at full;
/// - **key**: the music two octaves up and two octaves down (key ±0.4), the larger;
/// - **random**: as the rest, and eight middle Cs in turn (a new value each), compared note by
///   note (their brightness, bands, level and pitch), the largest;
/// - **amp env, env 2, LFO**: they move by themselves while a note plays (velocity 100).
///
/// The macros stay as saved. `None` if there is no such slot or the synth cannot be made.
pub fn slot_effect(p: &Preset, i: usize) -> Option<(f32, String)> {
    let (source, _, _) = p.slot_list.get(i)?;
    let id = crate::plugins::synth_mod::SOURCES.get(crate::plugins::synth_mod::source_index(source)?)?.id;
    let off = without_slot(p, i)?;
    let wheel = if id == "mod_wheel" { 1.0 } else { 0.0 };
    let at = |with: bool, notes: &[Note], frames: usize| render(if with { p } else { &off }, State { macros: Macros::Saved, wheel }, notes, frames);
    match id {
        "velocity" => compare(at, 127, 0),
        "key" => {
            let up = compare(at, 100, 24)?;
            let down = compare(at, 100, -24)?;
            Some(if down.0 > up.0 { (down.0, format!("two octaves down, {}", down.1)) } else { (up.0, format!("two octaves up, {}", up.1)) })
        }
        "random" => {
            let mut best = compare(at, 100, 0)?;
            let notes: Vec<Note> = (0..8).map(|k| Note { at: k * secs(0.4), len: secs(0.3), key: 60, vel: 100 }).collect();
            let (a, b) = (at(false, &notes, secs(3.2))?, at(true, &notes, secs(3.2))?);
            let (ma, mb) = (mono(&a), mono(&b));
            let hint = crate::midi_to_hz(60.0) as f32;
            for k in 0..8 {
                let (from, to) = (k * secs(0.4), k * secs(0.4) + secs(0.3));
                let (sc, what) = audible(&traits(&a, from, to), &traits(&b, from, to));
                if sc > best.0 {
                    best = (sc, format!("note {}: {what}", k + 1));
                }
                let pa = pitch_traits(ma.get(from + secs(0.03)..to).unwrap_or(&[]), hint);
                let pb = pitch_traits(mb.get(from + secs(0.03)..to).unwrap_or(&[]), hint);
                if let (Some(x), Some(y)) = (pa, pb) {
                    let cents = 1200.0 * (y.0 / x.0).log2();
                    if cents.is_finite() && cents.abs() / 100.0 > best.0 {
                        best = (cents.abs() / 100.0, format!("note {}: pitch {cents:+.0} cents", k + 1));
                    }
                }
            }
            Some(best)
        }
        _ => compare(at, 100, 0),
    }
}

/// Macro `i` (0-based) of `p` turned from 0 to 100 % over 200 ms while a chord is held (from
/// 1.0 s; the other macros as saved), against the macro left at 0 and at 100 % throughout:
/// (how far apart the two fixed renders are, how far the turned one still is from the one at
/// 100 %), both [`audible`] scores of 1.6..2.8 s. A macro whose targets follow while a note is
/// held leaves the second near zero.
pub fn macro_during(p: &Preset, i: usize) -> Option<(f32, f32)> {
    let param = MACRO_PARAMS.get(i)?;
    let held = chord(&MID_CHORD, 100, secs(3.0));
    let state = State { macros: Macros::One(i, 0.0), wheel: 0.0 };
    let turn: Vec<Change> = (0..=10).map(|k| Change { at: secs(1.0) + k * secs(0.02), param, value: k as f32 * 10.0 }).collect();
    let moved = render_changing(p, RATE, state, &held, secs(3.0), &turn)?;
    let at0 = render(p, state, &held, secs(3.0))?;
    let at1 = render(p, State { macros: Macros::One(i, 100.0), wheel: 0.0 }, &held, secs(3.0))?;
    let t = |x: &[Vec<f32>; 2]| traits(x, secs(1.6), secs(2.8));
    let (t0, t1, tm) = (t(&at0), t(&at1), t(&moved));
    Some((audible(&t0, &t1).0, audible(&tm, &t1).0))
}

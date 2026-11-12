//! Tuning Manners (`fs_manners`) against a reference render.
//!
//! ```sh
//! cargo run --release -p soundcraft-dsp --example manners_tune -- gen probe.wav
//! cargo run --release -p soundcraft-dsp --example manners_tune -- render in.wav out.wav tame=50 recover=50
//! cargo run --release -p soundcraft-dsp --example manners_tune -- compare probe.wav ref_t50_r50.wav@tame=50,recover=50
//! cargo run --release -p soundcraft-dsp --example manners_tune -- fit probe.wav ref_a.wav@tame=100 ref_b.wav@recover=100
//! ```
//!
//! - `gen` writes the probe: 24-bit stereo, synthesised here (noise and tones with known
//!   spectra, one question per segment). Play it through the reference processor once per
//!   setting and bounce each pass, same length, no normalising, no dither.
//! - `render` runs any file through Manners (latency taken off).
//! - `compare` measures what the reference did to the dry file (gain per third octave, per
//!   moment), what Manners does at the same knobs, and prints both per segment with the error.
//! - `fit` moves Manners' constants ([`Tuning`]) to shrink that error over every pair given, and
//!   prints the result as Rust to paste into `Tuning::default`.
//!
//! Knobs after `@` (or as arguments to `render`): `tame recover bias brighten boost` in percent,
//! `lo hi` in Hz. Any `Tuning` field can be given the same way (`slope_up_db=12`).

use rustfft::{FftPlanner, num_complex::Complex};
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_dsp::plugins::fs::manners::{MannersDsp, Settings, Tuning};
use std::f32::consts::TAU;

type Res<T> = Result<T, String>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("gen") => generate(&args[1..]),
        Some("render") => render_cmd(&args[1..]),
        Some("compare") => compare_cmd(&args[1..], false),
        Some("fit") => compare_cmd(&args[1..], true),
        _ => Err("usage: manners_tune gen <out.wav> [rate] | render <in> <out> [k=v ..] | compare <dry> <ref@k=v,..> .. | fit <dry> <ref@k=v,..> .."
            .into()),
    };
    if let Err(e) = r {
        eprintln!("manners_tune: {e}");
        std::process::exit(1);
    }
}

// ------------------------------------------------------------------------------- files

fn read(path: &str) -> Res<AudioBuffer> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str());
    let (_, mut buf) = soundcraft_audio_io::decode(&bytes, ext).map_err(|e| format!("{path}: {e}"))?;
    match buf.channels.len() {
        0 => return Err(format!("{path}: no audio")),
        1 => buf.channels.push(buf.channels[0].clone()),
        _ => buf.channels.truncate(2),
    }
    Ok(buf)
}

fn write(path: &str, buf: &AudioBuffer) -> Res<()> {
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, ..EncodeOptions::default() };
    let bytes = soundcraft_audio_io::encode(buf, &opts).map_err(|e| format!("{path}: {e}"))?;
    std::fs::write(path, bytes).map_err(|e| format!("{path}: {e}"))
}

// ------------------------------------------------------------------------------- knobs

/// The `Tuning` fields a `k=v` may name and `fit` may move: (name, access, smallest, largest).
type Field = (&'static str, fn(&mut Tuning) -> &mut f32, f32, f32);
const FIELDS: &[Field] = &[
    ("slope_up_db", |t| &mut t.slope_up_db, 2.0, 60.0),
    ("slope_down_db", |t| &mut t.slope_down_db, 4.0, 60.0),
    ("self_width_erb", |t| &mut t.self_width_erb, 0.25, 4.0),
    ("tame_ratio", |t| &mut t.tame_ratio, 0.0, 1.5),
    ("recover_ratio", |t| &mut t.recover_ratio, 0.0, 1.5),
    ("knee_db", |t| &mut t.knee_db, 0.1, 8.0),
    ("max_cut_db", |t| &mut t.max_cut_db, 2.0, 24.0),
    ("max_boost_db", |t| &mut t.max_boost_db, 2.0, 18.0),
    ("level_attack_ms", |t| &mut t.level_attack_ms, 1.0, 400.0),
    ("level_release_ms", |t| &mut t.level_release_ms, 5.0, 1500.0),
    ("gain_attack_ms", |t| &mut t.gain_attack_ms, 10.0, 400.0),
    ("gain_release_ms", |t| &mut t.gain_release_ms, 10.0, 1500.0),
    ("level_comp", |t| &mut t.level_comp, 0.0, 1.0),
    ("bias_db", |t| &mut t.bias_db, 0.5, 12.0),
    ("brighten_db", |t| &mut t.brighten_db, 0.5, 12.0),
    ("boost_db", |t| &mut t.boost_db, 0.5, 12.0),
    ("floor_db", |t| &mut t.floor_db, -100.0, -30.0),
    ("bands_per_erb", |t| &mut t.bands_per_erb, 1.0, 3.0),
    ("broad_width_erb", |t| &mut t.broad_width_erb, 0.5, 8.0),
    ("tame_broad", |t| &mut t.tame_broad, 0.02, 1.5),
    ("recover_broad", |t| &mut t.recover_broad, 0.02, 1.5),
    ("tame_level", |t| &mut t.tame_level, 0.0, 4.0),
    ("recover_level", |t| &mut t.recover_level, 0.0, 4.0),
    ("level_ref_db", |t| &mut t.level_ref_db, -40.0, -6.0),
    ("tame_target0", |t| &mut t.tame_target[0], -20.0, 20.0),
    ("tame_target1", |t| &mut t.tame_target[1], -20.0, 20.0),
    ("tame_target2", |t| &mut t.tame_target[2], -20.0, 20.0),
    ("tame_target3", |t| &mut t.tame_target[3], -20.0, 20.0),
    ("tame_target4", |t| &mut t.tame_target[4], -20.0, 20.0),
    ("tame_target5", |t| &mut t.tame_target[5], -20.0, 20.0),
    ("tame_target6", |t| &mut t.tame_target[6], -20.0, 20.0),
    ("tame_target7", |t| &mut t.tame_target[7], -20.0, 20.0),
    ("recover_target0", |t| &mut t.recover_target[0], -20.0, 20.0),
    ("recover_target1", |t| &mut t.recover_target[1], -20.0, 20.0),
    ("recover_target2", |t| &mut t.recover_target[2], -20.0, 20.0),
    ("recover_target3", |t| &mut t.recover_target[3], -20.0, 20.0),
    ("recover_target4", |t| &mut t.recover_target[4], -20.0, 20.0),
    ("recover_target5", |t| &mut t.recover_target[5], -20.0, 20.0),
    ("recover_target6", |t| &mut t.recover_target[6], -20.0, 20.0),
    ("recover_target7", |t| &mut t.recover_target[7], -20.0, 20.0),
];

/// `k=v` pairs onto the knobs and the tuning.
fn apply(pairs: &str, set: &mut Settings, tuning: &mut Tuning) -> Res<()> {
    for pair in pairs.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').ok_or_else(|| format!("`{pair}` is not key=value"))?;
        let v: f32 = v.trim().parse().map_err(|_| format!("`{pair}`: not a number"))?;
        match k.trim() {
            "tame" => set.tame = v / 100.0,
            "recover" => set.recover = v / 100.0,
            "bias" => set.bias = v / 100.0,
            "brighten" => set.brighten = v / 100.0,
            "boost" => set.boost = v / 100.0,
            "lo" => set.lo_hz = v,
            "hi" => set.hi_hz = v,
            name => match FIELDS.iter().find(|f| f.0 == name) {
                Some(f) => *(f.1)(tuning) = v,
                None => return Err(format!("`{name}` is neither a knob nor a tuning field")),
            },
        }
    }
    Ok(())
}

/// The file through Manners, the latency taken off (same length as the input).
fn render(dry: &AudioBuffer, set: Settings, tuning: Tuning) -> Vec<Vec<f32>> {
    let mut dsp = MannersDsp::with_tuning(tuning);
    dsp.prepare(f64::from(dry.sample_rate), 2);
    dsp.set(set);
    let lat = dsp.latency_samples();
    let len = dry.frames();
    let mut io: Vec<Vec<f32>> =
        dry.channels.iter().take(2).map(|c| c.iter().take(len).copied().chain(std::iter::repeat_n(0.0, lat)).collect()).collect();
    dsp.process(&mut io, len + lat);
    io.into_iter().map(|c| c.get(lat..).unwrap_or(&[]).to_vec()).collect()
}

fn render_cmd(args: &[String]) -> Res<()> {
    let [input, output, rest @ ..] = args else { return Err("render <in.wav> <out.wav> [k=v ..]".into()) };
    let dry = read(input)?;
    let (mut set, mut tuning) = (Settings::default(), Tuning::default());
    apply(&rest.join(","), &mut set, &mut tuning)?;
    let out = render(&dry, set, tuning);
    write(output, &AudioBuffer { sample_rate: dry.sample_rate, channels: out })?;
    println!("{output}: {input} through Manners at {set:?}");
    Ok(())
}

// ------------------------------------------------------------------------------- the probe

struct Rng(u64);
impl Rng {
    /// 0..1.
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}

/// A bell in dB on a log-frequency axis: `gain` at `fc`, `width` octaves to one sigma.
fn bell(f: f32, fc: f32, gain: f32, width: f32) -> f32 {
    let o = (f / fc).log2() / width;
    gain * (-0.5 * o * o).exp()
}

/// The spectra the probe is made of: amplitude at `f` Hz, in dB against flat.
#[derive(Clone, Copy)]
enum Shape {
    Pink,
    White,
    Brown,
    /// Pink with a bell: (centre Hz, dB, width in octaves).
    PinkBell(f32, f32, f32),
    /// Pink through a 12 dB/octave low-pass at this frequency.
    PinkLowPass(f32),
    /// Pink through a 12 dB/octave high-pass at this frequency.
    PinkHighPass(f32),
}

impl Shape {
    fn db(self, f: f32) -> f32 {
        let pink = -10.0 * (f / 1000.0).log10();
        match self {
            Shape::Pink => pink,
            Shape::White => 0.0,
            Shape::Brown => -20.0 * (f / 1000.0).log10(),
            Shape::PinkBell(fc, gain, width) => pink + bell(f, fc, gain, width),
            Shape::PinkLowPass(fc) => pink - 10.0 * (1.0 + (f / fc).powi(4)).log10(),
            Shape::PinkHighPass(fc) => pink - 10.0 * (1.0 + (fc / f).powi(4)).log10(),
        }
    }
}

/// Noise with the shape's spectrum (random phases), `len` samples at `rms_db` dBFS.
fn noise(shape: Shape, len: usize, sr: f32, rms_db: f32, rng: &mut Rng) -> Vec<f32> {
    let n = len.next_power_of_two().max(2);
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let top = (0.45 * sr).min(20_000.0);
    for k in 1..n / 2 {
        let f = k as f32 * sr / n as f32;
        if !(20.0..=top).contains(&f) {
            continue;
        }
        let mag = 10f32.powf(shape.db(f) / 20.0);
        let ph = TAU * rng.next();
        buf[k] = Complex::new(mag * ph.cos(), mag * ph.sin());
        buf[n - k] = buf[k].conj();
    }
    FftPlanner::<f32>::new().plan_fft_inverse(n).process(&mut buf);
    let mut x: Vec<f32> = buf.iter().take(len).map(|c| c.re).collect();
    scale_to(&mut x, rms_db);
    x
}

fn scale_to(x: &mut [f32], rms_db: f32) {
    let rms = (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt() as f32;
    let g = if rms > 0.0 { 10f32.powf(rms_db / 20.0) / rms } else { 0.0 };
    x.iter_mut().for_each(|v| *v *= g);
}

/// A raised-cosine fade of `ms` at both ends.
fn fade(x: &mut [f32], sr: f32, ms: f32) {
    let n = ((ms / 1000.0 * sr) as usize).min(x.len() / 2);
    for i in 0..n {
        let g = 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / n as f32).cos();
        x[i] *= g;
        let j = x.len() - 1 - i;
        x[j] *= g;
    }
}

/// One segment of the probe: what it is called, what it asks, and how it is made.
struct Segment {
    name: &'static str,
    asks: &'static str,
    secs: f32,
    make: fn(usize, f32, &mut Rng) -> [Vec<f32>; 2],
    /// For the segments that change half way: (seconds of each part, the band to time, Hz).
    step: Option<(f32, f32)>,
}

fn both(x: Vec<f32>) -> [Vec<f32>; 2] {
    [x.clone(), x]
}

/// Parts of equal length one after the other, each faded 5 ms.
fn parts(shapes: &[Shape], len: usize, sr: f32, rng: &mut Rng) -> Vec<f32> {
    let each = len / shapes.len().max(1);
    let mut out = Vec::with_capacity(len);
    for s in shapes {
        let mut x = noise(*s, each, sr, -20.0, rng);
        fade(&mut x, sr, 5.0);
        out.extend(x);
    }
    out.resize(len, 0.0);
    out
}

const RES_3K: Shape = Shape::PinkBell(3_000.0, 12.0, 0.06);
const NOTCH_2K: Shape = Shape::PinkBell(2_000.0, -15.0, 0.25);

const SEGMENTS: &[Segment] = &[
    Segment {
        name: "pink",
        asks: "what it does to a balanced spectrum",
        secs: 4.0,
        make: |n, sr, r| both(noise(Shape::Pink, n, sr, -20.0, r)),
        step: None,
    },
    Segment { name: "white", asks: "a bright tilt", secs: 4.0, make: |n, sr, r| both(noise(Shape::White, n, sr, -20.0, r)), step: None },
    Segment { name: "brown", asks: "a dark tilt", secs: 4.0, make: |n, sr, r| both(noise(Shape::Brown, n, sr, -20.0, r)), step: None },
    Segment {
        name: "res3k",
        asks: "a narrow +12 dB resonance at 3 kHz",
        secs: 4.0,
        make: |n, sr, r| both(noise(RES_3K, n, sr, -20.0, r)),
        step: None,
    },
    Segment {
        name: "res250",
        asks: "the same at 250 Hz",
        secs: 4.0,
        make: |n, sr, r| both(noise(Shape::PinkBell(250.0, 12.0, 0.06), n, sr, -20.0, r)),
        step: None,
    },
    Segment {
        name: "res8k",
        asks: "the same at 8 kHz",
        secs: 4.0,
        make: |n, sr, r| both(noise(Shape::PinkBell(8_000.0, 12.0, 0.06), n, sr, -20.0, r)),
        step: None,
    },
    Segment {
        name: "bump1k",
        asks: "a broad +8 dB bump at 1 kHz",
        secs: 4.0,
        make: |n, sr, r| both(noise(Shape::PinkBell(1_000.0, 8.0, 0.4), n, sr, -20.0, r)),
        step: None,
    },
    Segment { name: "notch2k", asks: "a -15 dB hole at 2 kHz", secs: 4.0, make: |n, sr, r| both(noise(NOTCH_2K, n, sr, -20.0, r)), step: None },
    Segment {
        name: "notch400",
        asks: "the same at 400 Hz",
        secs: 4.0,
        make: |n, sr, r| both(noise(Shape::PinkBell(400.0, -15.0, 0.25), n, sr, -20.0, r)),
        step: None,
    },
    Segment {
        name: "dull",
        asks: "no highs (low-pass 5 kHz)",
        secs: 4.0,
        make: |n, sr, r| both(noise(Shape::PinkLowPass(5_000.0), n, sr, -20.0, r)),
        step: None,
    },
    Segment {
        name: "thin",
        asks: "no lows (high-pass 200 Hz)",
        secs: 4.0,
        make: |n, sr, r| both(noise(Shape::PinkHighPass(200.0), n, sr, -20.0, r)),
        step: None,
    },
    Segment {
        name: "tone1k",
        asks: "a steady 1 kHz tone over quiet pink",
        secs: 4.0,
        make: |n, sr, r| {
            let mut x = noise(Shape::Pink, n, sr, -32.0, r);
            for (i, v) in x.iter_mut().enumerate() {
                *v += 0.1 * (TAU * 1000.0 * i as f32 / sr).sin();
            }
            both(x)
        },
        step: None,
    },
    Segment {
        name: "step-res",
        asks: "how fast Tame comes and goes (pink, +resonance at 3 kHz, pink)",
        secs: 6.0,
        make: |n, sr, r| both(parts(&[Shape::Pink, RES_3K, Shape::Pink], n, sr, r)),
        step: Some((2.0, 3_000.0)),
    },
    Segment {
        name: "step-hole",
        asks: "how fast Recover comes and goes (pink, hole at 2 kHz, pink)",
        secs: 6.0,
        make: |n, sr, r| both(parts(&[Shape::Pink, NOTCH_2K, Shape::Pink], n, sr, r)),
        step: Some((2.0, 2_000.0)),
    },
    Segment { name: "quiet", asks: "the resonance again at -40 dBFS", secs: 4.0, make: |n, sr, r| both(noise(RES_3K, n, sr, -40.0, r)), step: None },
    Segment { name: "loud", asks: "the resonance again at -14 dBFS", secs: 4.0, make: |n, sr, r| both(noise(RES_3K, n, sr, -14.0, r)), step: None },
    Segment {
        name: "saw110",
        asks: "a harmonic tone (110 Hz sawtooth) over very quiet pink",
        secs: 4.0,
        make: |n, sr, r| {
            let mut x = noise(Shape::Pink, n, sr, -50.0, r);
            let mut tone = vec![0.0f32; n];
            let mut h = 1;
            while 110.0 * h as f32 <= 16_000.0 {
                let w = TAU * 110.0 * h as f32 / sr;
                let ph = TAU * r.next();
                for (i, v) in tone.iter_mut().enumerate() {
                    *v += (w * i as f32 + ph).sin() / h as f32;
                }
                h += 1;
            }
            scale_to(&mut tone, -20.0);
            x.iter_mut().zip(&tone).for_each(|(a, b)| *a += b);
            both(x)
        },
        step: None,
    },
    Segment {
        name: "bursts",
        asks: "30 ms bursts eight times a second over a quiet bed",
        secs: 4.0,
        make: |n, sr, r| {
            let mut x = noise(Shape::Pink, n, sr, -14.0, r);
            let bed = noise(Shape::Pink, n, sr, -44.0, r);
            let period = (sr / 8.0) as usize;
            let on = (0.030 * sr) as usize;
            for (i, v) in x.iter_mut().enumerate() {
                let p = i % period.max(1);
                let g = if p < on { 0.5 - 0.5 * (TAU * p as f32 / on as f32).cos() } else { 0.0 };
                *v = *v * g + bed[i];
            }
            both(x)
        },
        step: None,
    },
    Segment {
        name: "wide",
        asks: "the resonance with left and right unrelated",
        secs: 4.0,
        make: |n, sr, r| [noise(RES_3K, n, sr, -20.0, r), noise(RES_3K, n, sr, -20.0, r)],
        step: None,
    },
];

/// Silence between segments, seconds.
const GAP: f32 = 0.5;

/// Where each segment starts and how long it is, in samples, and the probe's whole length.
fn layout(sr: u32) -> (Vec<(usize, usize)>, usize) {
    let s = sr as f32;
    let mut at = (GAP * s) as usize;
    let mut out = Vec::new();
    for seg in SEGMENTS {
        let len = (seg.secs * s) as usize;
        out.push((at, len));
        at += len + (GAP * s) as usize;
    }
    (out, at)
}

fn generate(args: &[String]) -> Res<()> {
    let path = args.first().ok_or("gen <out.wav> [rate]")?;
    let sr: u32 = match args.get(1) {
        Some(r) => r.parse().map_err(|_| format!("`{r}` is not a sample rate"))?,
        None => 48_000,
    };
    if !(22_050..=192_000).contains(&sr) {
        return Err(format!("{sr} Hz: use 22050 to 192000"));
    }
    let (places, total) = layout(sr);
    let mut buf = AudioBuffer::new(sr, 2, total);
    let mut rng = Rng(0x7A12_4E55_0BAD_5EED);
    println!("{path}: {:.1} s, {sr} Hz, 24-bit stereo\n", total as f32 / sr as f32);
    println!("{:>7}  {:<10} what it asks", "at", "segment");
    for (seg, (at, len)) in SEGMENTS.iter().zip(&places) {
        let mut chans = (seg.make)(*len, sr as f32, &mut rng);
        for (c, x) in chans.iter_mut().enumerate() {
            fade(x, sr as f32, 20.0);
            buf.channels[c][*at..*at + *len].copy_from_slice(x);
        }
        println!("{:>6.1}s  {:<10} {}", *at as f32 / sr as f32, seg.name, seg.asks);
    }
    let peak = buf.channels.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
    println!("\npeak {:.1} dBFS", 20.0 * peak.max(1e-9).log10());
    write(path, &buf)
}

// ------------------------------------------------------------------------------- measuring

const M_N: usize = 4096;
const M_HOP: usize = 1024;

/// Third-octave centres from 40 Hz to 16 kHz.
fn centres() -> Vec<f32> {
    (0..27).map(|i| 40.0 * 2f32.powf(i as f32 / 3.0)).collect()
}

/// Power per third octave per frame, both channels summed: `[frame][band]`.
fn band_power(x: &[Vec<f32>], sr: u32) -> Vec<Vec<f32>> {
    let fft = FftPlanner::<f32>::new().plan_fft_forward(M_N);
    let window: Vec<f32> = (0..M_N).map(|i| 0.5 - 0.5 * (TAU * i as f32 / M_N as f32).cos()).collect();
    let cs = centres();
    let edge = 2f32.powf(1.0 / 6.0);
    // Each bin's band (the nearest bin for a band narrower than one).
    let bin_hz = sr as f32 / M_N as f32;
    let mut band_of = vec![usize::MAX; M_N / 2];
    for (b, c) in cs.iter().enumerate() {
        let (lo, hi) = ((c / edge / bin_hz).ceil() as usize, (c * edge / bin_hz).ceil() as usize);
        let (lo, hi) = if hi > lo { (lo, hi) } else { ((c / bin_hz).round() as usize, (c / bin_hz).round() as usize + 1) };
        for slot in band_of.iter_mut().take(hi.min(M_N / 2)).skip(lo) {
            *slot = b;
        }
    }
    let len = x.iter().map(Vec::len).min().unwrap_or(0);
    let frames = if len >= M_N { (len - M_N) / M_HOP + 1 } else { 0 };
    let mut out = vec![vec![0.0f32; cs.len()]; frames];
    let mut buf = vec![Complex::new(0.0f32, 0.0); M_N];
    for (t, row) in out.iter_mut().enumerate() {
        for ch in x.iter().take(2) {
            let at = t * M_HOP;
            for (i, b) in buf.iter_mut().enumerate() {
                *b = Complex::new(ch[at + i] * window[i], 0.0);
            }
            fft.process(&mut buf);
            for (k, band) in band_of.iter().enumerate() {
                if let Some(v) = row.get_mut(*band) {
                    *v += buf[k].norm_sqr();
                }
            }
        }
    }
    out
}

/// How many samples `wet` is behind `dry` (0..=16384), by correlating the first ten seconds
/// that hold sound.
fn lag(dry: &[Vec<f32>], wet: &[Vec<f32>]) -> usize {
    let len = dry[0].len().min(wet[0].len());
    let start = dry[0].iter().position(|v| v.abs() > 1e-4).unwrap_or(0);
    let span = (len.saturating_sub(start + 16_384)).min(96_000);
    let mut best = (0usize, f64::MIN);
    for d in 0..=16_384usize {
        let mut sum = 0.0f64;
        let mut i = start;
        while i < start + span {
            sum += f64::from(dry[0][i]) * f64::from(wet[0][i + d]);
            i += 3;
        }
        if sum > best.1 {
            best = (d, sum);
        }
    }
    best.0
}

/// A reference render and the knobs it was made at.
struct Pair {
    name: String,
    set: Settings,
    /// What the reference did: dB per frame per band (`NAN` where the dry file is too quiet to
    /// tell).
    gain: Vec<Vec<f32>>,
}

/// Wet over dry in dB per frame per band; `NAN` where dry is more than 70 dB under its loudest.
fn gain_of(dry: &[Vec<f32>], wet: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let top = dry.iter().flatten().fold(0.0f32, |m, v| m.max(*v));
    let floor = top * 1e-7;
    dry.iter()
        .zip(wet)
        .map(|(d, w)| d.iter().zip(w).map(|(a, b)| if *a > floor { 10.0 * (b.max(1e-30) / a).log10() } else { f32::NAN }).collect())
        .collect()
}

/// RMS of the difference over every frame and band both can tell, dB.
fn error(a: &[Vec<f32>], b: &[Vec<f32>]) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0usize);
    for (x, y) in a.iter().flatten().zip(b.iter().flatten()) {
        if x.is_finite() && y.is_finite() {
            sum += f64::from(x - y).powi(2);
            n += 1;
        }
    }
    (sum / n.max(1) as f64).sqrt()
}

/// The mean over frames `from..to` per band (`NAN` where nothing can be told).
fn mean(gain: &[Vec<f32>], from: usize, to: usize) -> Vec<f32> {
    let bands = gain.first().map_or(0, Vec::len);
    (0..bands)
        .map(|b| {
            let v: Vec<f32> = gain.iter().take(to).skip(from).filter_map(|r| r.get(b).copied().filter(|x| x.is_finite())).collect();
            if v.is_empty() { f32::NAN } else { v.iter().sum::<f32>() / v.len() as f32 }
        })
        .collect()
}

fn frame_at(sample: usize) -> usize {
    sample / M_HOP
}

/// Milliseconds after frame `step` until band `b` has gone 63 % of the way from its value
/// before to its settled value after (`None`: it does not move a decibel).
fn time_to_settle(gain: &[Vec<f32>], b: usize, step: usize, part: usize, sr: u32) -> Option<f32> {
    let at = |t: usize| gain.get(t).and_then(|r| r.get(b)).copied().filter(|v| v.is_finite());
    let avg = |from: usize, to: usize| {
        let v: Vec<f32> = (from..to).filter_map(at).collect();
        (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32)
    };
    let before = avg(step.saturating_sub(part / 2), step.saturating_sub(4))?;
    let after = avg(step + part / 2, step + part - 4)?;
    if (after - before).abs() < 1.0 {
        return None;
    }
    let t = (step..step + part).find(|t| at(*t).is_some_and(|v| (v - before) / (after - before) >= 0.63))?;
    Some((t - step) as f32 * M_HOP as f32 / sr as f32 * 1000.0)
}

fn row(label: &str, v: &[f32]) -> String {
    let mut s = format!("{label:<14}");
    for x in v {
        s.push_str(&if x.is_finite() { format!("{x:>6.1}") } else { format!("{:>6}", "-") });
    }
    s
}

fn compare_cmd(args: &[String], fit: bool) -> Res<()> {
    let [dry_path, refs @ ..] = args else { return Err("<dry.wav> <ref.wav@k=v,..> ..".into()) };
    if refs.is_empty() {
        return Err("give at least one reference: ref.wav@tame=50,recover=50".into());
    }
    let dry = read(dry_path)?;
    let sr = dry.sample_rate;
    let dry_power = band_power(&dry.channels, sr);
    let mut tuning = Tuning::default();
    let mut pairs = Vec::new();
    for spec in refs {
        let (path, knobs) = spec.split_once('@').unwrap_or((spec.as_str(), ""));
        let mut set = Settings::default();
        apply(knobs, &mut set, &mut tuning)?;
        let wet = read(path)?;
        if wet.sample_rate != sr {
            return Err(format!("{path} is {} Hz, {dry_path} is {sr} Hz", wet.sample_rate));
        }
        let d = lag(&dry.channels, &wet.channels);
        let len = dry.frames();
        let aligned: Vec<Vec<f32>> = wet.channels.iter().map(|c| (0..len).map(|i| c.get(i + d).copied().unwrap_or(0.0)).collect()).collect();
        println!("{path}: {d} samples behind the dry file ({:.1} ms), knobs {knobs}", d as f32 / sr as f32 * 1000.0);
        pairs.push(Pair { name: path.to_string(), set, gain: gain_of(&dry_power, &band_power(&aligned, sr)) });
    }
    let ours = |p: &Pair, t: Tuning| gain_of(&dry_power, &band_power(&render(&dry, p.set, t), sr));
    let cost = |t: Tuning| (pairs.iter().map(|p| error(&p.gain, &ours(p, t)).powi(2)).sum::<f64>() / pairs.len() as f64).sqrt();

    if fit {
        tuning = fit_tuning(tuning, &cost);
    }

    // The table: per pair, per segment (the whole file as one when it is not the probe).
    let (places, total) = layout(sr);
    let probe = dry.frames().abs_diff(total) <= 2;
    let cs = centres();
    let head: Vec<f32> = cs.iter().map(|c| if *c >= 1000.0 { (c / 100.0).round() / 10.0 } else { c.round() }).collect();
    for p in &pairs {
        let mine = ours(p, tuning);
        println!("\n== {} (RMS error {:.2} dB over every moment and band)", p.name, error(&p.gain, &mine));
        println!("{}", row("Hz (k above 1k)", &head).replace(".0", "  "));
        let show = |name: &str, from: usize, to: usize| {
            let (r, m) = (mean(&p.gain, from, to), mean(&mine, from, to));
            let diff: Vec<f32> = r.iter().zip(&m).map(|(a, b)| b - a).collect();
            println!("{}\n{}\n{}", row(&format!("{name} ref"), &r), row("  manners", &m), row("  manners-ref", &diff));
        };
        if probe {
            for (seg, (at, len)) in SEGMENTS.iter().zip(&places) {
                // The settled part: the last half (of the middle part, for a step).
                let (from, to) = match seg.step {
                    Some((part, _)) => (at + (1.5 * part * sr as f32) as usize, at + (2.0 * part * sr as f32) as usize),
                    None => (at + len / 2, at + len),
                };
                show(seg.name, frame_at(from), frame_at(to.saturating_sub(M_N)));
                if let Some((part, hz)) = seg.step {
                    let b = cs.iter().enumerate().min_by(|x, y| (x.1 - hz).abs().total_cmp(&(y.1 - hz).abs())).map_or(0, |x| x.0);
                    let part_frames = frame_at((part * sr as f32) as usize);
                    let on = frame_at(at + (part * sr as f32) as usize);
                    let off = on + part_frames;
                    let ms = |g: &[Vec<f32>], step| time_to_settle(g, b, step, part_frames, sr).map_or("-".to_string(), |v| format!("{v:.0} ms"));
                    println!(
                        "  at {hz} Hz, 63 % of the way: ref in {} out {}, manners in {} out {} (the meter itself blurs about {:.0} ms)",
                        ms(&p.gain, on),
                        ms(&p.gain, off),
                        ms(&mine, on),
                        ms(&mine, off),
                        M_N as f32 / sr as f32 * 500.0
                    );
                }
            }
        } else {
            show("whole file", 0, mine.len());
        }
    }
    println!("\noverall RMS error {:.2} dB", cost(tuning));
    if fit {
        println!("\n{tuning:#?}");
    }
    Ok(())
}

/// Coordinate descent over [`FIELDS`]: each field tried a step up and a step down, kept when the
/// error shrinks; the step halves each round.
fn fit_tuning(start: Tuning, cost: &dyn Fn(Tuning) -> f64) -> Tuning {
    let mut best = start;
    let mut best_cost = cost(best);
    println!("\nfit: starting at {best_cost:.3} dB");
    let mut step = 0.4f32;
    for round in 0..5 {
        for (name, field, lo, hi) in FIELDS {
            for dir in [1.0f32, -1.0] {
                loop {
                    let mut t = best;
                    let v = field(&mut t);
                    // Times and ratios move by a factor; levels in dB by a share of their range.
                    let moved = if *lo > 0.0 { *v * (1.0 + step).powf(dir) } else { *v + dir * step * 0.25 * (hi - lo) };
                    let moved = moved.clamp(*lo, *hi);
                    if (moved - *v).abs() < 1e-6 {
                        break;
                    }
                    *v = moved;
                    let c = cost(t);
                    if c + 1e-4 < best_cost {
                        println!("  round {round}: {name} = {moved:.3} -> {c:.3} dB");
                        best = t;
                        best_cost = c;
                    } else {
                        break;
                    }
                }
            }
        }
        step *= 0.5;
    }
    println!("fit: ended at {best_cost:.3} dB");
    best
}

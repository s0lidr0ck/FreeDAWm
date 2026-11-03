//! `ScoutMeter` against Scout2's C++ `ScoutDSP` (see `reference_harness.cpp`, which wrote
//! `reference.json` from the same generated signals).

use super::*;

const RATE: usize = 48_000;

/// The harness's LCG: uniform in [-1, 1), exact float ops only.
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 * (1.0f32 / 16_777_216.0) * 2.0 - 1.0
    }
}

/// The harness's Paul Kellet economy pink filter, in f32.
#[derive(Default)]
struct Pink {
    b0: f32,
    b1: f32,
    b2: f32,
}

impl Pink {
    fn step(&mut self, w: f32) -> f32 {
        self.b0 = 0.99765 * self.b0 + w * 0.099_046;
        self.b1 = 0.96300 * self.b1 + w * 0.296_516_4;
        self.b2 = 0.57000 * self.b2 + w * 1.052_691_3;
        self.b0 + self.b1 + self.b2 + w * 0.1848
    }
}

/// (a) 1 kHz sine at -20 dBFS peak, 5 s, L = R.
fn sine_1k() -> (Vec<f32>, Vec<f32>) {
    let l: Vec<f32> = (0..5 * RATE).map(|i| (0.1 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / RATE as f64).sin()) as f32).collect();
    (l.clone(), l)
}

/// (b) stereo pink-ish noise, L/R decorrelated, -18 dBFS RMS, 10 s.
fn pink_stereo() -> (Vec<f32>, Vec<f32>) {
    let n = 10 * RATE;
    let (mut gl, mut gr) = (Lcg(0x1234_5678), Lcg(0x9E37_79B9));
    let (mut pl, mut pr) = (Pink::default(), Pink::default());
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    let mut ss = 0.0f64;
    for _ in 0..n {
        let a = pl.step(gl.next());
        let b = pr.step(gr.next());
        ss += a as f64 * a as f64 + b as f64 * b as f64;
        l.push(a);
        r.push(b);
    }
    let target = 0.125_892_541_179_416_73f64; // 10^(-18/20)
    let g = (target / (ss / (2.0 * n as f64)).sqrt()) as f32;
    l.iter_mut().for_each(|x| *x *= g);
    r.iter_mut().for_each(|x| *x *= g);
    (l, r)
}

/// (c) drum-like: a 50 ms decaying 120 Hz burst every 500 ms over -60 dBFS noise, 8 s.
fn drum_bursts() -> (Vec<f32>, Vec<f32>) {
    let n = 8 * RATE;
    let (mut gl, mut gr) = (Lcg(0x00C0_FFEE), Lcg(0x0BAD_C0DE));
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        let pos = i % (RATE / 2);
        let b = if pos < RATE / 20 {
            (0.5 * (-(pos as f64) / 480.0).exp() * (2.0 * std::f64::consts::PI * 120.0 * pos as f64 / RATE as f64).sin()) as f32
        } else {
            0.0
        };
        l.push(b + 0.001 * gl.next());
        r.push(b + 0.001 * gr.next());
    }
    (l, r)
}

fn measure(l: &[f32], r: &[f32]) -> Metrics {
    let mut m = ScoutMeter::new(RATE as f64);
    m.start();
    for (bl, br) in l.chunks(512).zip(r.chunks(512)) {
        m.feed(bl, br);
    }
    assert_eq!(m.samples(), l.len() as u64);
    m.finish()
}

/// Max relative and absolute deviation, every numeric leaf compared; panics on a mismatch. A
/// leaf passes within `tol.0` relative, or `tol.1` absolute, or 1e-3 absolute for dB values.
fn compare(name: &str, path: &str, got: &serde_json::Value, want: &serde_json::Value, tol: (f64, f64), worst: &mut (f64, f64, String)) {
    use serde_json::Value;
    match (got, want) {
        (Value::Object(g), Value::Object(w)) => {
            let mut gk: Vec<_> = g.keys().collect();
            let mut wk: Vec<_> = w.keys().collect();
            gk.sort();
            wk.sort();
            assert_eq!(gk, wk, "{name}: field sets differ");
            for (k, wv) in w {
                compare(name, &format!("{path}.{k}"), &g[k], wv, tol, worst);
            }
        }
        (Value::Array(g), Value::Array(w)) => {
            assert_eq!(g.len(), w.len(), "{name}{path}: length");
            for (i, (gv, wv)) in g.iter().zip(w).enumerate() {
                compare(name, &format!("{path}[{i}]"), gv, wv, tol, worst);
            }
        }
        (Value::Number(g), Value::Number(w)) => {
            let (g, w) = (g.as_f64().unwrap_or(f64::NAN), w.as_f64().unwrap_or(f64::NAN));
            let abs = (g - w).abs();
            let rel = abs / w.abs().max(1e-30);
            // dB-valued fields may sit near -100 or 0 where relative error is meaningless.
            let is_db = path.contains("db") || path.contains("lufs") || path.contains("spectrum") || path.contains("zone") || path.ends_with(".lra");
            let ok = rel <= tol.0 || abs <= tol.1 || (is_db && abs <= 1e-3);
            assert!(ok, "{name}{path}: rust {g} vs c++ {w} (abs {abs:e}, rel {rel:e})");
            if abs > 1e-7 && rel > worst.0 {
                worst.0 = rel;
                worst.2 = path.to_string();
            }
            worst.1 = worst.1.max(abs);
        }
        (g, w) => assert_eq!(g, w, "{name}{path}"),
    }
}

#[test]
fn metrics_match_the_cpp_reference() {
    let reference: serde_json::Value = serde_json::from_str(include_str!("reference.json")).expect("reference.json parses");
    let cases: [(&str, fn() -> (Vec<f32>, Vec<f32>)); 3] = [("sine_1k", sine_1k), ("pink_stereo", pink_stereo), ("drum_bursts", drum_bursts)];
    // `fp_precise`: the C++ under IEEE evaluation order, the strict check of the translation
    // (1e-4 relative; measured: <= 5e-9, the reference's 9-digit printing).
    // `fp_fast`: the plugin's release flags (what the owner's Scout2 reports). Fast-math
    // reassociation alone moves the C++ itself: up to 4.1e-4 relative (pink spectral centroid)
    // and ~2e-6 absolute on near-zero correlations, so this check is looser.
    for (build, tol) in [("fp_precise", (1e-4, 1e-7)), ("fp_fast", (1e-3, 1e-5))] {
        for (name, generate) in cases {
            let (l, r) = generate();
            let got = serde_json::to_value(measure(&l, &r)).expect("metrics serialize");
            let want = &reference[build][name];
            let mut worst = (0.0, 0.0, String::new());
            compare(&format!("{build}/{name}"), "", &got, want, tol, &mut worst);
            // The reference also deserializes into the Rust struct, field for field.
            let _: Metrics = serde_json::from_value(want.clone()).expect("reference is a Metrics");
            println!("{build}/{name}: max rel {:e} ({}), max abs {:e}", worst.0, worst.2, worst.1);
        }
    }
}

#[test]
fn start_and_feed_do_not_allocate() {
    // No counting allocator in this crate: every heap block the meter owns is allocated in
    // `new`; their addresses and capacities stay put across `start` and 1000 `feed` calls.
    let (l, r) = drum_bursts();
    let mut m = ScoutMeter::new(48_000.0);
    let blocks = m.heap_blocks();
    for round in 0..3 {
        m.start();
        for (i, (bl, br)) in l.chunks(512).zip(r.chunks(512)).take(1000).enumerate() {
            // Odd block lengths, and more than `MAX_BLOCKS` K-blocks over the rounds.
            let n = 1 + (i * 37 + round) % bl.len();
            m.feed(&bl[..n], &br[..n]);
            m.feed(&bl[n..], &br[n..]);
        }
        assert_eq!(blocks, m.heap_blocks());
    }
    // A long run past the 256-block store keeps the store as it was.
    m.start();
    let silence = vec![0.0f32; 4096];
    for _ in 0..(110 * 48_000 / 4096) {
        m.feed(&silence, &silence);
    }
    assert_eq!(blocks, m.heap_blocks());
    assert!(m.finish().duration_sec > 100.0);
}

fn all_finite(m: &Metrics) -> bool {
    fn walk(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
            serde_json::Value::Array(a) => a.iter().all(walk),
            serde_json::Value::Object(o) => o.values().all(walk),
            serde_json::Value::Bool(_) => true,
            // serde_json writes a non-finite float as null.
            _ => false,
        }
    }
    serde_json::to_value(m).is_ok_and(|v| walk(&v))
}

#[test]
fn hostile_input_stays_finite() {
    let hostile = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30, f32::MAX, f32::MIN_POSITIVE, 0.5];
    let mut l = vec![0.0f32; 2 * 48_000];
    let mut r = vec![0.0f32; 2 * 48_000];
    for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
        *a = hostile[i % hostile.len()];
        *b = hostile[(i * 3 + 1) % hostile.len()];
    }
    let mut m = ScoutMeter::new(48_000.0);
    m.start();
    m.feed(&l, &r);
    let got = m.finish();
    assert!(all_finite(&got), "{got:?}");

    // All-NaN and all-huge captures, and mismatched slice lengths.
    for x in [f32::NAN, 1e30, -f32::MAX] {
        m.start();
        let v = vec![x; 48_000 + 7];
        m.feed(&v, &v[..48_000]);
        assert_eq!(m.samples(), 48_000);
        let got = m.finish();
        assert!(all_finite(&got), "{x}: {got:?}");
    }

    // Hostile sample rates.
    for sr in [0.0, -1.0, f64::NAN, f64::INFINITY, 1.0, 1e300] {
        let mut m = ScoutMeter::new(sr);
        m.start();
        m.feed(&l[..4096], &r[..4096]);
        let got = m.finish();
        assert!(all_finite(&got), "sr {sr}: {got:?}");
    }

    // Nothing fed at all.
    let m = ScoutMeter::new(48_000.0);
    assert!(all_finite(&m.finish()));
    assert!(!m.finish().valid);
}

#[test]
fn iso_centres_are_exact_third_octaves() {
    assert_eq!(iso_centre(17), 1000.0);
    assert!((iso_centre(0) - 19.69).abs() < 0.01);
    assert!((iso_centre(30) - 20159.4).abs() < 1.0);
    assert_eq!(MAX_CAPTURE_SECONDS, 30);
}

/// A 0 dBFS sine at `f` (L = R) for 5 s at `rate`, measured.
fn sine_bands(f: f64, rate: usize) -> Metrics {
    sine_bands_faded(f, rate, 0)
}

/// As [`sine_bands`], with a raised-cosine fade-in over the first `fade` samples.
fn sine_bands_faded(f: f64, rate: usize, fade: usize) -> Metrics {
    use std::f64::consts::PI;
    let l: Vec<f32> = (0..5 * rate)
        .map(|i| {
            let g = if i < fade { 0.5 - 0.5 * (PI * i as f64 / fade as f64).cos() } else { 1.0 };
            (g * (2.0 * PI * f * i as f64 / rate as f64).sin()) as f32
        })
        .collect();
    let mut m = ScoutMeter::new(rate as f64);
    m.start();
    for b in l.chunks(512) {
        m.feed(b, b);
    }
    m.finish()
}

#[test]
fn bands_are_flat_at_the_centre_and_minus_3_db_at_the_edges() {
    // A 0 dBFS sine has a mean square of 0.5 (-3.01 dB): a band's gain is its reading minus that.
    let sine_db = 10.0 * 0.5f64.log10();
    for (rate, bands) in [(RATE, [0usize, 5, 9, 17, 28]), (96_000, [0, 5, 17, 28, 30])] {
        for b in bands {
            let fc = f64::from(iso_centre(b));
            let gain = f64::from(sine_bands(fc, rate).spectrum31[b]) - sine_db;
            assert!(gain.abs() <= 0.5, "{rate} Hz band {b} ({fc:.1} Hz): centre {gain:.3} dB");
            for edge in [-1.0, 1.0] {
                let fe = fc * 2f64.powf(edge / 6.0);
                let gain = f64::from(sine_bands(fe, rate).spectrum31[b]) - sine_db;
                assert!((gain + 3.01).abs() <= 0.5, "{rate} Hz band {b} edge {fe:.1} Hz: {gain:.3} dB");
            }
        }
    }
}

#[test]
fn a_loud_150_hz_tone_stays_out_of_the_63_hz_band() {
    // A 10 ms fade-in keeps the onset click out of the measurement.
    let m = sine_bands_faded(150.0, RATE, 480);
    let own = (0..NUM_BANDS).fold(0, |a, b| if m.spectrum31[b] > m.spectrum31[a] { b } else { a });
    assert_eq!(own, 9, "150 Hz lands in the 160 Hz band");
    let rejection = m.spectrum31[own] - m.spectrum31[5];
    // About 56 dB; the old 2nd-order bands gave 18 dB.
    assert!(rejection >= 50.0, "rejection {rejection} dB");
}

#[test]
fn the_clamped_top_band_is_stable_at_44_1_khz() {
    let m = sine_bands(19_000.0, 44_100);
    assert!(m.spectrum31.iter().all(|v| v.is_finite() && *v < 1.0), "{:?}", m.spectrum31);
    assert!((m.spectrum31[30] + 3.21).abs() <= 0.5, "19 kHz in the clamped 20 kHz band: {}", m.spectrum31[30]);
}

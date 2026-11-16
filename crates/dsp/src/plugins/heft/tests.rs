use super::*;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

fn sine(n: usize, hz: f32, sr: f32, amp: f32) -> Vec<f32> {
    (0..n).map(|i| amp * (std::f64::consts::TAU * f64::from(hz) * i as f64 / f64::from(sr)).sin() as f32).collect()
}

fn heft(sr: f32, ch: usize, set: &[(&str, f32)]) -> Heft {
    let mut h = Heft::new();
    for (id, v) in set {
        assert!(h.set_param(id, *v), "{id}");
    }
    h.prepare(sr, 512, ch);
    h
}

/// Runs `chans` through in blocks of 256; returns the output.
fn run(h: &mut Heft, chans: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let len = chans[0].len();
    let mut out: Vec<Vec<f32>> = chans.iter().map(|_| Vec::with_capacity(len)).collect();
    let mut at = 0;
    while at < len {
        let n = 256.min(len - at);
        let mut io: Vec<Vec<f32>> = chans.iter().map(|c| c[at..at + n].to_vec()).collect();
        h.process(&mut io, n);
        for (o, c) in out.iter_mut().zip(&io) {
            o.extend_from_slice(c);
        }
        at += n;
    }
    out
}

/// The amplitude of `hz` in `y` (by correlation).
fn level(y: &[f32], hz: f32, sr: f32) -> f64 {
    let (mut c, mut s) = (0.0f64, 0.0f64);
    for (i, v) in y.iter().enumerate() {
        let ph = std::f64::consts::TAU * f64::from(hz) * i as f64 / f64::from(sr);
        c += f64::from(*v) * ph.cos();
        s += f64::from(*v) * ph.sin();
    }
    (c * c + s * s).sqrt() * 2.0 / y.len() as f64
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

#[test]
fn registered_listed_with_the_designs() {
    let p = crate::create("heft").unwrap();
    assert!(p.info().listed() && p.info().with_engines());
    assert_eq!(p.latency(), 0);
}

/// With Harmonics at 0 and Original Bass at 100 % the output is the input, sample for sample;
/// the latency reported (0) is what an impulse measures.
#[test]
fn transparent_at_zero_and_latency_is_measured() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let x: Vec<f32> = sine(8000, 55.0, sr, 0.5).iter().zip(sine(8000, 3000.0, sr, 0.3)).map(|(a, b)| a + b).collect();
        let y = run(&mut heft(sr, 2, &[("harmonics", 0.0)]), &[x.clone(), x.clone()]);
        assert_eq!(y[0], x, "{sr}");
        let mut imp = vec![0.0f32; 64];
        imp[0] = 1.0;
        let mut h = heft(sr, 1, &[("harmonics", 0.0)]);
        let y = run(&mut h, &[imp]);
        let at = y[0].iter().enumerate().fold((0, 0.0f32), |m, (i, v)| if v.abs() > m.1 { (i, v.abs()) } else { m }).0;
        assert_eq!(at, h.latency());
    }
}

/// A 50 Hz bass under a 120 Hz Frequency: its harmonics above 120 Hz (150, 200 Hz) are added,
/// the one below (100 Hz) is not, nothing above the frequency is changed (a 2 kHz tone), and
/// the added harmonics grow with Harmonics.
#[test]
fn harmonics_only_above_the_frequency_and_scaled_by_the_amount() {
    let sr = 48_000.0;
    let n = 48_000;
    let bass = sine(n, 50.0, sr, 0.5);
    let top = sine(n, 2000.0, sr, 0.2);
    let x: Vec<f32> = bass.iter().zip(&top).map(|(a, b)| a + b).collect();
    let added = |amount: f32, character: f32, freq: f32| {
        let y = run(&mut heft(sr, 1, &[("freq", freq), ("harmonics", amount), ("character", character)]), std::slice::from_ref(&x));
        let d: Vec<f32> = y[0].iter().zip(&x).map(|(a, b)| a - b).collect();
        d[n / 2..].to_vec()
    };
    // the second harmonic (100 Hz): added under an 80 Hz Frequency, left out under 120 Hz
    let above = db(level(&added(100.0, 0.0, 80.0), 100.0, sr));
    let below = db(level(&added(100.0, 0.0, 120.0), 100.0, sr));
    assert!(above > -20.0, "added above the frequency: {above:.1} dB");
    assert!(below < above - 10.0, "left out just below it: {below:.1} vs {above:.1} dB");
    let far = db(level(&added(100.0, 0.0, 250.0), 100.0, sr));
    assert!(far < above - 40.0, "and well below it, gone: {far:.1} dB");
    // the third (150 Hz) is above 120 Hz: added; the bass itself and the 2 kHz tone are untouched
    let odd = added(100.0, 100.0, 120.0);
    assert!(db(level(&odd, 150.0, sr)) > -20.0, "the 3rd (150 Hz) is added: {:.1}", db(level(&odd, 150.0, sr)));
    assert!(db(level(&odd, 50.0, sr)) < -40.0, "nothing at the fundamental: {:.1}", db(level(&odd, 50.0, sr)));
    assert!(db(level(&odd, 2000.0, sr)) < -60.0, "the 2 kHz tone is untouched");
    // half the amount, half the harmonics
    let half = added(50.0, 100.0, 120.0);
    let r = level(&half, 150.0, sr) / level(&odd, 150.0, sr);
    assert!((r - 0.5).abs() < 0.02, "{r}");
}

#[test]
fn original_bass_removes_the_fundamental() {
    let sr = 48_000.0;
    let x = sine(48_000, 50.0, sr, 0.5);
    let y = run(&mut heft(sr, 1, &[("freq", 120.0), ("harmonics", 0.0), ("original", 0.0)]), std::slice::from_ref(&x));
    assert!(db(level(&y[0][24_000..], 50.0, sr)) < db(0.5) - 20.0, "removed");
    let y = run(&mut heft(sr, 1, &[("freq", 120.0), ("harmonics", 0.0), ("original", 50.0)]), std::slice::from_ref(&x));
    let l = db(level(&y[0][24_000..], 50.0, sr)) - db(0.5);
    assert!(l < -3.0 && l > -9.0, "reduced: {l:.1} dB");
}

#[test]
fn the_same_at_every_rate() {
    let measure = |sr: f32| {
        let x = sine((sr * 1.0) as usize, 55.0, sr, 0.5);
        let y = run(&mut heft(sr, 1, &[("harmonics", 80.0), ("character", 60.0)]), &[x]);
        let tail = &y[0][y[0].len() / 2..];
        [55.0, 110.0, 165.0, 220.0].map(|hz| db(level(tail, hz, sr)))
    };
    let base = measure(48_000.0);
    for sr in [44_100.0, 96_000.0] {
        let m = measure(sr);
        for (a, b) in m.iter().zip(&base) {
            assert!(*b < -60.0 || (a - b).abs() < 0.5, "{sr}: {m:?} vs {base:?}");
        }
    }
}

/// At full Harmonics with loud highs in the input, nothing folds back: every part of the output
/// is the input's tones or a harmonic of the bass.
#[test]
fn no_aliasing_at_full_amount() {
    let sr = 48_000.0;
    let n = 1 << 15;
    let bin = |k: usize| k as f32 * sr / n as f32;
    // the bass on bin 41 (about 60 Hz), a loud tone on bin 7000 (about 10 kHz)
    let x: Vec<f32> = sine(2 * n, bin(41), sr, 0.6).iter().zip(sine(2 * n, bin(7000), sr, 0.4)).map(|(a, b)| a + b).collect();
    let y = run(&mut heft(sr, 1, &[("harmonics", 100.0), ("character", 50.0), ("freq", 100.0)]), &[x]);
    let tail = &y[0][n..];
    let mut buf: Vec<Complex<f32>> =
        tail.iter().enumerate().map(|(i, v)| Complex::new(v * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()), 0.0)).collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let mag: Vec<f32> = buf[..n / 2].iter().map(|c| c.norm()).collect();
    let full = mag.iter().cloned().fold(0.0, f32::max);
    let worst = (1..n / 2).filter(|i| (i % 41 > 3 && i % 41 < 38) && (*i < 6990 || *i > 7010)).map(|i| mag[i]).fold(0.0f32, f32::max);
    let d = 20.0 * (worst / full).log10();
    assert!(d < -80.0, "something folds back at {d:.1} dB");
}

#[test]
fn every_channel_mono_stereo_and_surround_no_allocation() {
    let sr = 48_000.0;
    for ch in [1usize, 2, 6] {
        let mut h = heft(sr, ch, &[("harmonics", 80.0)]);
        let src = sine(256, 50.0, sr, 0.5);
        let mut io: Vec<Vec<f32>> = (0..ch).map(|_| src.clone()).collect();
        let chans_ptr = h.chans.as_ptr();
        let host: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
        for blk in 0..400 {
            for c in &mut io {
                c.copy_from_slice(&src);
            }
            h.set_param("freq", 40.0 + (blk % 20) as f32 * 10.0);
            h.set_param("character", (blk % 100) as f32);
            if blk % 100 == 0 {
                h.reset();
            }
            h.process(&mut io, 256);
        }
        assert_eq!(chans_ptr, h.chans.as_ptr());
        assert_eq!(host, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
        assert!(io.iter().all(|c| c == &io[0]), "every channel processed alike ({ch})");
        assert!(io[0].iter().zip(&src).any(|(a, b)| (a - b).abs() > 1e-3), "and processed ({ch})");
    }
    // hostile values
    let mut h = heft(sr, 2, &[]);
    for bad in [f32::NAN, f32::INFINITY, -1e9] {
        for p in &HEFT_PARAMS {
            h.set_param(p.id, bad);
        }
        let mut io = vec![sine(512, 50.0, sr, 1.0), vec![f32::NAN; 512]];
        h.process(&mut io, 512);
        assert!(io.iter().flatten().all(|v| v.is_finite()));
    }
}

#[test]
fn readouts_meter_in_and_out() {
    let sr = 48_000.0;
    let mut h = heft(sr, 1, &[]);
    run(&mut h, &[sine(4800, 60.0, sr, 0.5)]);
    let mut r = [f32::NAN; 8];
    h.readouts(&mut r);
    assert!((r[0] - gain_to_db(0.5)).abs() < 0.2 && r[1].is_finite() && r[2] > -60.0, "{r:?}");
}

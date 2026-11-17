use super::*;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

fn sine(n: usize, hz: f32, sr: f32, amp: f32) -> Vec<f32> {
    (0..n).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / sr).sin()).collect()
}

fn keel(sr: f32, set: &[(&str, f32)]) -> Keel {
    let mut k = Keel::new();
    for (id, v) in set {
        assert!(k.set_param(id, *v), "{id}");
    }
    k.prepare(sr, 512, 2);
    k
}

fn run(k: &mut Keel, input: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len());
    for chunk in input.chunks(256) {
        let mut io = vec![chunk.to_vec(), chunk.to_vec()];
        k.process(&mut io, chunk.len());
        out.extend_from_slice(&io[0]);
    }
    out
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn registered_listed_with_the_engines_and_original() {
    let p = crate::create("keel").unwrap();
    assert_eq!(p.info().id, "keel");
    assert!(p.info().listed() && p.info().with_engines());
    assert_eq!(p.info().params.len(), KEEL_PARAMS.len());
}

/// The latency reported is the latency measured: an impulse through the DI (the clean signal the
/// amp is blended with, delayed to stay in time with the oversampled preamp) arrives exactly that
/// many samples late, at every rate. (The amp path's half-bands delay the low and middle of the
/// band by the same whole samples: `sat_halfband`'s own test measures them.)
#[test]
fn reported_latency_is_measured() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut k = keel(sr, &[("blend", 0.0)]);
        let l = k.latency();
        let coefs = [hb::coefs_for_stage(0), hb::coefs_for_stage(1)];
        assert_eq!(l, hb::round_trip_latency(&coefs, 2), "the preamp's half-bands");
        assert!(l > 0 && l < DI_LEN, "{l}");
        let mut x = vec![0.0f32; 64];
        x[0] = 1.0;
        let y = run(&mut k, &x);
        let at = y.iter().enumerate().fold((0, 0.0f32), |m, (i, v)| if v.abs() > m.1 { (i, v.abs()) } else { m }).0;
        assert_eq!(at, l, "{sr}: the DI peaks at the reported latency");
        assert!((y[at] - 1.0).abs() < 1e-3, "{sr}: and at full level");
    }
}

/// `process`, `set_param` and `reset` never allocate: the networks' buffers and the host's stay
/// where they are through 1000 blocks of every switch moving.
#[test]
fn process_does_not_allocate() {
    let mut k = keel(48_000.0, &[]);
    let heap = |k: &Keel| {
        let mut out = Vec::new();
        for s in &k.stacks {
            s.net.heap_blocks(&mut out);
        }
        out
    };
    let before = heap(&k);
    let src = sine(256, 110.0, 48_000.0, 0.6);
    let mut io = vec![src.clone(), src.clone()];
    let host: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        for c in &mut io {
            c.copy_from_slice(&src);
        }
        k.set_param("voicing", ((blk / 7) % 3) as f32);
        k.set_param("cab", ((blk / 5) % 4) as f32);
        k.set_param("bass", (blk % 11) as f32);
        k.set_param("treble", ((blk * 3) % 11) as f32);
        k.set_param("squeeze", ((blk / 3) % 11) as f32);
        k.set_param("bright", (blk % 2) as f32);
        if blk % 250 == 0 {
            k.reset();
        }
        k.process(&mut io, 256);
    }
    assert_eq!(before, heap(&k), "a network buffer moved");
    assert_eq!(host, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(io.iter().flatten().all(|v| v.is_finite()));
}

/// The same settings sound the same at 44.1, 48 and 96 kHz: level and the second and third
/// harmonics within a fraction of a dB, for every voicing.
#[test]
fn sample_rate_independent() {
    for voicing in 0..3 {
        let set = [("voicing", voicing as f32), ("drive", 7.0), ("cab", 0.0), ("blend", 100.0)];
        let measure = |sr: f32| {
            let n = (sr * 0.6) as usize;
            let y = run(&mut keel(sr, &set), &sine(n, 110.0, sr, 0.5));
            let tail = &y[n / 2..];
            // the levels of 110, 220 and 330 Hz by correlation
            let at = |hz: f32| {
                let (mut c, mut s) = (0.0f64, 0.0f64);
                for (i, v) in tail.iter().enumerate() {
                    let ph = std::f64::consts::TAU * f64::from(hz) * i as f64 / f64::from(sr);
                    c += f64::from(*v) * ph.cos();
                    s += f64::from(*v) * ph.sin();
                }
                20.0 * ((c * c + s * s).sqrt() * 2.0 / tail.len() as f64).max(1e-9).log10()
            };
            (20.0 * f64::from(rms(tail)).log10(), at(110.0), at(220.0), at(330.0))
        };
        let base = measure(48_000.0);
        for sr in [44_100.0, 96_000.0] {
            let m = measure(sr);
            assert!((m.0 - base.0).abs() < 0.5, "voicing {voicing}, {sr}: level {m:?} vs {base:?}");
            assert!((m.1 - base.1).abs() < 0.5, "voicing {voicing}, {sr}: fundamental {m:?} vs {base:?}");
            // the harmonics that are there (a symmetric clipper makes no even ones)
            for (a, b) in [(m.2, base.2), (m.3, base.3)] {
                assert!(b < -60.0 || (a - b).abs() < 1.5, "voicing {voicing}, {sr}: harmonics {m:?} vs {base:?}");
            }
        }
    }
}

/// At full drive, what folds back below the tone (anything not on a harmonic of it) stays far
/// under the tone itself, for every voicing: the oversampling does its job.
#[test]
fn little_aliasing_at_full_drive() {
    let sr = 48_000.0;
    let n = 1 << 15;
    // a tone that lands on an FFT bin, high enough that its harmonics pass Nyquist quickly
    let bin = 1365; // ~2 kHz
    let hz = bin as f32 * sr / n as f32;
    for voicing in 0..3 {
        let mut k = keel(sr, &[("voicing", voicing as f32), ("drive", 10.0), ("cab", 3.0), ("blend", 100.0), ("treble", 7.0)]);
        let x = sine(2 * n, hz, sr, 0.8);
        let y = run(&mut k, &x);
        let tail = &y[n..];
        let mut buf: Vec<Complex<f32>> =
            tail.iter().enumerate().map(|(i, v)| Complex::new(v * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()), 0.0)).collect();
        FftPlanner::new().plan_fft_forward(n).process(&mut buf);
        let mag: Vec<f32> = buf[..n / 2].iter().map(|c| c.norm()).collect();
        let fundamental = mag[bin - 2..=bin + 2].iter().cloned().fold(0.0, f32::max);
        // every bin more than 3 bins from a harmonic, above 60 Hz
        let worst = (40..n / 2)
            .filter(|i| {
                let r = i % bin;
                r > 3 && r < bin - 3
            })
            .map(|i| mag[i])
            .fold(0.0f32, f32::max);
        let db = 20.0 * (worst / fundamental).log10();
        assert!(db < -60.0, "voicing {voicing}: aliases at {db:.1} dB under the tone");
    }
}

#[test]
fn drive_bends_the_tone_and_squeeze_turns_peaks_down() {
    let sr = 48_000.0;
    let x = sine(24_000, 110.0, sr, 0.7);
    // the third harmonic against the fundamental, dB
    let h3 = |y: &[f32]| {
        let tail = &y[12_000..];
        let at = |hz: f64| {
            let (mut c, mut s) = (0.0f64, 0.0f64);
            for (i, v) in tail.iter().enumerate() {
                let ph = std::f64::consts::TAU * hz * i as f64 / f64::from(sr);
                c += f64::from(*v) * ph.cos();
                s += f64::from(*v) * ph.sin();
            }
            (c * c + s * s).sqrt()
        };
        20.0 * (at(330.0) / at(110.0)).log10()
    };
    for voicing in [0.0, 1.0, 2.0] {
        let clean = run(&mut keel(sr, &[("voicing", voicing), ("drive", 1.0), ("cab", 3.0), ("blend", 100.0)]), &x);
        let hot = run(&mut keel(sr, &[("voicing", voicing), ("drive", 9.0), ("cab", 3.0), ("blend", 100.0)]), &x);
        assert!(h3(&hot) > h3(&clean) + 12.0, "voicing {voicing}: driving bends the wave ({} vs {} dB)", h3(&hot), h3(&clean));
    }
    // Squeeze: a loud passage after a quiet one comes out closer in level
    let mut x = sine(48_000, 110.0, sr, 0.05);
    x.extend(sine(48_000, 110.0, sr, 0.8));
    let level = |set: &[(&str, f32)]| {
        let y = run(&mut keel(sr, set), &x);
        20.0 * (rms(&y[60_000..]) / rms(&y[24_000..48_000])).log10()
    };
    let open = level(&[("blend", 0.0)]);
    let squeezed = level(&[("blend", 0.0), ("squeeze", 8.0)]);
    assert!(squeezed < open - 6.0, "{squeezed} vs {open}");
    let mut k = keel(sr, &[("blend", 0.0), ("squeeze", 8.0)]);
    run(&mut k, &x);
    let mut r = [f32::NAN; 8];
    k.readouts(&mut r);
    assert!(r[2] > 3.0 && k.gain_reduction_db() == r[2], "{r:?}");
}

#[test]
fn the_tone_knobs_do_what_they_say() {
    let sr = 48_000.0;
    let level = |hz: f32, set: &[(&str, f32)]| {
        let mut s: Vec<(&str, f32)> = vec![("drive", 1.0), ("cab", 3.0), ("blend", 100.0)];
        s.extend_from_slice(set);
        let y = run(&mut keel(sr, &s), &sine(24_000, hz, sr, 0.05));
        20.0 * rms(&y[12_000..]).log10()
    };
    for voicing in [0.0, 1.0, 2.0] {
        let v = ("voicing", voicing);
        assert!(level(60.0, &[v, ("bass", 10.0)]) > level(60.0, &[v, ("bass", 0.0)]) + 6.0, "bass, voicing {voicing}");
        assert!(level(4000.0, &[v, ("treble", 10.0)]) > level(4000.0, &[v, ("treble", 0.0)]) + 6.0, "treble, voicing {voicing}");
        assert!(level(500.0, &[v, ("mid", 10.0)]) > level(500.0, &[v, ("mid", 0.0)]) + 3.0, "mid, voicing {voicing}");
    }
}

#[test]
fn hostile_values_and_switching_stay_finite() {
    let mut k = keel(44_100.0, &[]);
    for bad in [f32::NAN, f32::INFINITY, -1e9, 1e9] {
        for p in &KEEL_PARAMS {
            k.set_param(p.id, bad);
        }
        let mut io = vec![sine(512, 80.0, 44_100.0, 1.0), vec![f32::NAN; 512]];
        k.process(&mut io, 512);
        assert!(io.iter().flatten().all(|v| v.is_finite()), "{bad}");
    }
    // mono and wide buffers
    let mut k = keel(48_000.0, &[]);
    let mut mono = vec![sine(512, 80.0, 48_000.0, 0.5)];
    k.process(&mut mono, 512);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|_| sine(512, 80.0, 48_000.0, 0.5)).collect();
    k.process(&mut wide, 512);
    assert!(wide.iter().all(|c| c == &wide[0]), "the mono result on every channel");
}

/// Silence in is silence out at every rate and with each voicing, after `prepare` and after a
/// `reset` that follows loud playing: the tube stage starts from where it settles with no input,
/// not from its exact DC solution, which its per-sample solve lands a little away from.
#[test]
fn silence_in_silence_out_after_prepare_and_reset() {
    let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        for (v, name) in VOICINGS.iter().enumerate() {
            let mut k = keel(sr, &[("voicing", v as f32), ("drive", 10.0)]);
            let p = peak(&run(&mut k, &vec![0.0; sr as usize]));
            assert!(p < 1e-6, "{sr} Hz, {name}: {p} from silence");
            let _ = run(&mut k, &sine(sr as usize / 4, 60.0, sr, 0.9));
            k.reset();
            let p = peak(&run(&mut k, &vec![0.0; sr as usize / 2]));
            assert!(p < 1e-6, "{sr} Hz, {name}: {p} from silence after a reset");
        }
    }
    // the old reference, the stage's exact DC point, is measurably away from where it rests
    let k = keel(48_000.0, &[]);
    // (24 mV at 48 kHz when this was written; about 8e-4 at the output)
    let off = (k.tube.dc_plate() - k.tube_idle_plate).abs();
    assert!(off > 1e-4, "{off}");
}

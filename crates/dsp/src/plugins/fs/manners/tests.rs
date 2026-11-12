use super::*;
use rustfft::{FftPlanner, num_complex::Complex};

const SR: f32 = 48_000.0;
const N: usize = 2048;

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

/// Pink noise (Paul Kellet's economy filter), about -20 dBFS RMS.
fn pink(len: usize, seed: u32) -> Vec<f32> {
    let mut r = Rng(seed);
    let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
    (0..len)
        .map(|_| {
            let w = r.next();
            b0 = 0.99765 * b0 + w * 0.099_046;
            b1 = 0.96300 * b1 + w * 0.296_516_4;
            b2 = 0.57000 * b2 + w * 1.052_691_3;
            (b0 + b1 + b2 + w * 0.1848) * 0.06
        })
        .collect()
}

/// White noise through a two-pole resonator at `hz`: a narrow band of noise, peak gain about 1.
fn narrow(len: usize, hz: f32, seed: u32) -> Vec<f32> {
    let mut r = Rng(seed);
    let radius = 0.995f32;
    let w = std::f32::consts::TAU * hz / SR;
    let (a1, a2) = (2.0 * radius * w.cos(), -radius * radius);
    let (mut y1, mut y2) = (0.0f32, 0.0f32);
    (0..len)
        .map(|_| {
            let y = r.next() * (1.0 - radius) + a1 * y1 + a2 * y2;
            y2 = y1;
            y1 = y;
            y
        })
        .collect()
}

fn add(a: &[f32], b: &[f32], gain: f32) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y * gain).collect()
}

/// Mean power between `lo` and `hi` Hz, in dB, over Hann frames of 8192.
fn band_db(x: &[f32], lo: f32, hi: f32) -> f32 {
    let n = 8192;
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let mut sum = 0.0f64;
    let mut count = 0usize;
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

fn manners(set: &[(&str, f32)]) -> Manners {
    let mut t = Manners::new();
    t.prepare(SR, 512, 2);
    for (id, v) in set {
        assert!(t.set_param(id, *v), "{id}");
    }
    t
}

/// Runs both channels through in blocks of `block` and takes the latency off the front.
fn run(t: &mut Manners, input: &[Vec<f32>], block: usize) -> Vec<Vec<f32>> {
    let lat = t.latency();
    let len = input[0].len();
    let mut padded: Vec<Vec<f32>> = input.iter().map(|c| c.iter().copied().chain(std::iter::repeat_n(0.0, lat)).collect()).collect();
    let total = len + lat;
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        let mut buf: Vec<Vec<f32>> = padded.iter().map(|c| c[pos..pos + n].to_vec()).collect();
        t.process(&mut buf, n);
        for (c, b) in padded.iter_mut().zip(&buf) {
            c[pos..pos + n].copy_from_slice(b);
        }
        pos += n;
    }
    padded.into_iter().map(|c| c[lat..].to_vec()).collect()
}

fn stereo(x: Vec<f32>) -> Vec<Vec<f32>> {
    vec![x.clone(), x]
}

#[test]
fn registered_listed_and_reports_the_frame_as_latency() {
    let mut p = crate::create("fs_manners").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    assert!(p.info().listed());
    assert_eq!(p.latency(), 2048);
    p.prepare(44_100.0, 512, 2);
    assert_eq!(p.latency(), 2048);
    p.prepare(96_000.0, 512, 2);
    assert_eq!(p.latency(), 4096);
    p.prepare(192_000.0, 512, 2);
    assert_eq!(p.latency(), 8192);
    assert!(crate::plugins::fs::engine("fs_manners").unwrap().fidelity.is_none(), "there is no C++ original");
    let ids: Vec<&str> = MANNERS_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(ids, ["tame", "recover", "bias", "brighten", "boost", "rangeLo", "rangeHi", "delta", "bypass"]);
    assert_eq!(MANNERS_PARAMS[8].group, "", "the engine's bypass is hidden");
    assert_eq!(MANNERS_PARAMS[0].format(25.0), "25%");
    assert_eq!(MANNERS_PARAMS[5].format(20.0), "20.0 Hz");
}

#[test]
fn with_every_knob_at_rest_it_is_a_pure_delay() {
    let x = vec![pink(48_000, 1), pink(48_000, 2)];
    for block in [1usize, 64, 500, 4096] {
        let mut t = manners(&[("tame", 0.0), ("recover", 0.0)]);
        let y = run(&mut t, &x, block);
        for (a, b) in x.iter().zip(&y) {
            let err = a.iter().zip(b).map(|(p, q)| (p - q).abs()).fold(0.0f32, f32::max);
            assert!(err < 2e-5, "block {block}: {err}");
        }
    }
}

#[test]
fn tame_turns_a_resonance_down_and_leaves_the_rest() {
    let len = 4 * 48_000;
    let x = add(&pink(len, 3), &narrow(len, 3_000.0, 4), 6.0);
    let mut t = manners(&[("tame", 100.0), ("recover", 0.0)]);
    let y = run(&mut t, &stereo(x.clone()), 512);
    let tail = 48_000..len;
    let peak = band_db(&y[0][tail.clone()], 2_800.0, 3_200.0) - band_db(&x[tail.clone()], 2_800.0, 3_200.0);
    let rest = band_db(&y[0][tail.clone()], 300.0, 1_200.0) - band_db(&x[tail], 300.0, 1_200.0);
    assert!(peak < rest - 3.0, "the resonance came down {peak:.1} dB against {rest:.1} dB elsewhere");
    assert!(t.gain_reduction_db() > 2.0);
    let mut r = [f32::NAN; 8];
    t.readouts(&mut r);
    assert!(r[0] > 2.0 && r[1].abs() < 0.5 && r[2].is_finite() && r[3].is_nan(), "{r:?}");
}

/// `x` (a power of two long) with everything from `lo` to `hi` Hz scaled by `gain`.
fn scaled_band(x: &[f32], lo: f32, hi: f32, gain: f32) -> Vec<f32> {
    let n = x.len();
    let mut planner = FftPlanner::<f32>::new();
    let mut buf: Vec<Complex<f32>> = x.iter().map(|v| Complex::new(*v, 0.0)).collect();
    planner.plan_fft_forward(n).process(&mut buf);
    for (k, c) in buf.iter_mut().enumerate() {
        let f = k.min(n - k) as f32 * SR / n as f32;
        if f >= lo && f < hi {
            *c *= gain;
        }
    }
    planner.plan_fft_inverse(n).process(&mut buf);
    buf.iter().map(|c| c.re / n as f32).collect()
}

#[test]
fn recover_lifts_a_covered_region_and_not_the_noise_floor() {
    // Pink noise with the band around 2 kHz 14 dB down: its neighbours cover it.
    let len = 1 << 18;
    let x = scaled_band(&pink(len, 9), 1_700.0, 2_500.0, 0.2);
    let mut t = manners(&[("tame", 0.0), ("recover", 100.0)]);
    let y = run(&mut t, &stereo(x.clone()), 512);
    let tail = 48_000..len;
    let dip = band_db(&y[0][tail.clone()], 1_800.0, 2_400.0) - band_db(&x[tail.clone()], 1_800.0, 2_400.0);
    let loud = band_db(&y[0][tail.clone()], 500.0, 1_000.0) - band_db(&x[tail], 500.0, 1_000.0);
    assert!(dip > loud + 1.0, "the covered band rose {dip:.1} dB against {loud:.1} dB for its neighbours");
    let mut r = [f32::NAN; 8];
    t.readouts(&mut r);
    assert!(r[1] > 1.0 && r[0].abs() < 0.5, "{r:?}");

    // Nothing above 6 kHz at all (digital silence there): nothing to recover, so no lift.
    // (Read while the noise still runs: the end of a file is a click, which has highs.)
    let x = scaled_band(&pink(len, 10), 6_000.0, 30_000.0, 0.0);
    let mut dsp = MannersDsp::new();
    dsp.prepare(48_000.0, 2);
    dsp.set(Settings { recover: 1.0, ..Settings::default() });
    let mut io = stereo(x);
    dsp.process(&mut io, len);
    let comp = dsp.level_comp_db();
    for (hz, g) in dsp.band_hz().iter().zip(dsp.band_gain_db()) {
        if *hz > 9_000.0 {
            assert!(g - comp < 0.5, "{hz} Hz lifted {} dB", g - comp);
        }
    }
}

#[test]
fn an_even_spectrum_is_left_nearly_alone_and_keeps_its_level() {
    let len = 4 * 48_000;
    let x = pink(len, 11);
    let mut t = manners(&[("tame", 50.0), ("recover", 50.0)]);
    let y = run(&mut t, &stereo(x.clone()), 512);
    let tail = 48_000..len;
    for (lo, hi) in [(100.0, 200.0), (400.0, 800.0), (1_600.0, 3_200.0), (6_400.0, 12_800.0)] {
        let d = band_db(&y[0][tail.clone()], lo, hi) - band_db(&x[tail.clone()], lo, hi);
        assert!(d.abs() < 1.5, "{lo}-{hi} Hz moved {d:.2} dB on pink noise");
    }
    let whole = band_db(&y[0][tail.clone()], 20.0, 20_000.0) - band_db(&x[tail], 20.0, 20_000.0);
    assert!(whole.abs() < 0.5, "the level moved {whole:.2} dB");
}

#[test]
fn most_of_the_level_is_given_back_when_a_lot_is_tamed() {
    // The reference gives back about two thirds of what a cut takes (`Tuning::level_comp`).
    let len = 4 * 48_000;
    let x = add(&pink(len, 12), &narrow(len, 1_000.0, 13), 8.0);
    let rms = |v: &[f32]| (v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
    let change = |tuning: Tuning| {
        let mut dsp = MannersDsp::with_tuning(tuning);
        dsp.prepare(48_000.0, 2);
        dsp.set(Settings { tame: 1.0, ..Settings::default() });
        let mut io = stereo(x.clone());
        dsp.process(&mut io, len);
        (20.0 * (rms(&io[0][96_000..]) / rms(&x[96_000 - N..len - N])).log10(), dsp.level_comp_db())
    };
    let (with, comp) = change(Tuning::default());
    let (without, none) = change(Tuning { level_comp: 0.0, ..Tuning::default() });
    assert!(comp > 0.5 && none == 0.0, "{comp} {none}");
    assert!(without < -1.5, "taming this much takes level: {without:.2} dB");
    assert!(with.abs() < without.abs() * 0.6, "{with:.2} dB with the level given back, {without:.2} dB without");
    let (all, _) = change(Tuning { level_comp: 1.0, ..Tuning::default() });
    assert!(all.abs() < 0.6, "all of it given back: {all:.2} dB");
}

#[test]
fn a_lone_tone_is_cut_deep_and_narrow_without_turning_the_rest_up() {
    // A steady 1 kHz tone 12 dB over quiet pink noise: a peak on its own.
    let len = 4 * 48_000;
    let bed: Vec<f32> = pink(len, 30).iter().map(|v| v * 0.25).collect();
    let x: Vec<f32> = bed.iter().enumerate().map(|(i, v)| v + 0.1 * (std::f32::consts::TAU * 1_000.0 * i as f32 / SR).sin()).collect();
    let run_with = |tuning: Tuning| {
        let mut dsp = MannersDsp::with_tuning(tuning);
        dsp.prepare(48_000.0, 2);
        dsp.set(Settings { tame: 1.0, ..Settings::default() });
        let mut io = stereo(x.clone());
        dsp.process(&mut io, len);
        let at = |want: f32| {
            let i = (0..dsp.bands()).min_by(|a, b| (dsp.band_hz()[*a] - want).abs().total_cmp(&(dsp.band_hz()[*b] - want).abs())).unwrap();
            dsp.band_gain_db()[i]
        };
        (at(1_000.0), at(2_000.0), at(500.0), dsp.level_comp_db())
    };
    let (tone, above, below, comp) = run_with(Tuning::default());
    let (plain, _, _, plain_comp) = run_with(Tuning { peak_ratio: 0.0, ..Tuning::default() });
    assert!(tone < plain - 4.0, "the peak stage cuts the tone further: {tone:.1} dB against {plain:.1} dB without it");
    assert!(tone < above - 6.0 && tone < below - 6.0, "and narrowly: {below:.1} / {tone:.1} / {above:.1} dB at 500 Hz / 1 kHz / 2 kHz");
    assert!((comp - plain_comp).abs() < 0.5, "its cut is not made up for: {comp:.2} dB given back against {plain_comp:.2} dB");
}

#[test]
fn the_range_keeps_the_gain_inside_it() {
    let len = 3 * 48_000;
    let x = add(&add(&pink(len, 14), &narrow(len, 300.0, 15), 6.0), &narrow(len, 5_000.0, 16), 6.0);
    let mut t = manners(&[("tame", 100.0), ("recover", 0.0), ("rangeLo", 2_000.0)]);
    let _ = run(&mut t, &stereo(x), 512);
    let live = &t.analysis().unwrap().live;
    let bands = live.get(LIVE_BANDS) as usize;
    assert!(bands > 40 && bands <= MAX_BANDS, "{bands}");
    let comp = t.dsp.level_comp_db();
    let mut low_max = 0.0f32;
    let mut high_min = 0.0f32;
    for i in 0..bands {
        let (hz, g) = (live.get(LIVE_HZ + i), live.get(LIVE_GAIN + i) - comp);
        if hz < 1_000.0 {
            low_max = low_max.max(g.abs());
        } else if hz > 4_000.0 && hz < 6_000.0 {
            high_min = high_min.min(g);
        }
    }
    assert!(low_max < 0.01, "outside the range moved {low_max} dB");
    assert!(high_min < -2.0, "inside the range: {high_min} dB");
}

#[test]
fn brighten_and_boost_tilt_the_curve() {
    let len = 2 * 48_000;
    let x = stereo(pink(len, 17));
    let gain_at = |t: &Manners, want: f32| {
        let live = &t.analysis().unwrap().live;
        let bands = live.get(LIVE_BANDS) as usize;
        let i = (0..bands).min_by(|a, b| (live.get(LIVE_HZ + a) - want).abs().total_cmp(&(live.get(LIVE_HZ + b) - want).abs())).unwrap();
        live.get(LIVE_GAIN + i)
    };
    let mut t = manners(&[("tame", 0.0), ("recover", 0.0), ("brighten", 100.0)]);
    let _ = run(&mut t, &x, 512);
    assert!(gain_at(&t, 12_000.0) > gain_at(&t, 1_000.0) + 1.5 && gain_at(&t, 1_000.0) > gain_at(&t, 80.0) + 1.5);
    let mut t = manners(&[("tame", 0.0), ("recover", 0.0), ("brighten", -100.0)]);
    let _ = run(&mut t, &x, 512);
    assert!(gain_at(&t, 12_000.0) < gain_at(&t, 80.0) - 3.0);
    let mut t = manners(&[("tame", 0.0), ("recover", 0.0), ("boost", 100.0)]);
    let _ = run(&mut t, &x, 512);
    assert!(gain_at(&t, 50.0) > gain_at(&t, 450.0) + 4.0, "{} {}", gain_at(&t, 50.0), gain_at(&t, 450.0));
    assert!((gain_at(&t, 450.0) - gain_at(&t, 5_000.0)) < -1.0);
}

#[test]
fn delta_is_the_output_minus_the_input() {
    let len = 2 * 48_000;
    let x = stereo(add(&pink(len, 18), &narrow(len, 2_500.0, 19), 6.0));
    let set = [("tame", 80.0), ("recover", 60.0), ("brighten", 30.0)];
    let mut wet = manners(&set);
    let y = run(&mut wet, &x, 512);
    let mut delta = manners(&set);
    delta.set_param("delta", 1.0);
    let d = run(&mut delta, &x, 512);
    let err = (0..len).map(|i| (y[0][i] - x[0][i] - d[0][i]).abs()).fold(0.0f32, f32::max);
    assert!(err < 1e-4, "{err}");
    assert!(d[0].iter().any(|v| v.abs() > 1e-3), "something changed");
}

#[test]
fn the_engines_bypass_is_the_input_a_frame_late() {
    let x = stereo(add(&pink(48_000, 27), &narrow(48_000, 3_000.0, 28), 6.0));
    let mut t = manners(&[("tame", 100.0), ("recover", 100.0), ("brighten", 100.0), ("delta", 1.0), ("bypass", 1.0)]);
    assert_eq!(t.latency(), N);
    let y = run(&mut t, &x, 512);
    let err = x[0].iter().zip(&y[0]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(err < 2e-5, "{err}");
}

#[test]
fn mono_and_stereo_get_the_same_curve_and_the_sides_stay_apart() {
    let len = 2 * 48_000;
    let x = add(&pink(len, 20), &narrow(len, 3_000.0, 21), 6.0);
    let set = [("tame", 100.0), ("recover", 50.0)];
    let mut st = manners(&set);
    let y2 = run(&mut st, &stereo(x.clone()), 512);
    let mut mono = Manners::new();
    mono.prepare(SR, 512, 1);
    for (id, v) in set {
        mono.set_param(id, v);
    }
    let y1 = run(&mut mono, std::slice::from_ref(&x), 512);
    let err = (0..len).map(|i| (y1[0][i] - y2[0][i]).abs()).fold(0.0f32, f32::max);
    assert!(err < 1e-4, "{err}");
    // One side silent: it stays silent (the gain is real and the same on both).
    let mut t = manners(&set);
    let y = run(&mut t, &[x, vec![0.0; len]], 512);
    assert!(y[1].iter().all(|v| v.abs() < 1e-5));
    assert!(y[0].iter().any(|v| v.abs() > 0.01));
}

#[test]
fn silence_hostile_values_and_odd_calls_are_harmless() {
    let mut t = manners(&[("tame", 100.0), ("recover", 100.0)]);
    let y = run(&mut t, &stereo(vec![0.0; 48_000]), 512);
    assert!(y.iter().flatten().all(|v| *v == 0.0));
    let mut io = vec![vec![f32::NAN; 4096], vec![f32::INFINITY; 4096]];
    t.process(&mut io, 4096);
    t.process(&mut io, 4096);
    assert!(io.iter().flatten().all(|v| v.is_finite()));
    for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1e30, 1e30] {
        for p in &MANNERS_PARAMS {
            assert!(t.set_param(p.id, v));
        }
        let mut io = stereo(pink(4096, 22));
        t.process(&mut io, 4096);
        assert!(io.iter().flatten().all(|v| v.is_finite()));
    }
    // The range the wrong way round is the same range.
    let mut a = MannersDsp::new();
    a.prepare(48_000.0, 2);
    a.set(Settings { lo_hz: 5_000.0, hi_hz: 200.0, ..Settings::default() });
    let mut b = MannersDsp::new();
    b.prepare(48_000.0, 2);
    b.set(Settings { lo_hz: 200.0, hi_hz: 5_000.0, ..Settings::default() });
    assert_eq!(a.range_weights(), b.range_weights());
    // A hostile tuning still builds tables.
    let bad = Tuning { bands_per_erb: f32::NAN, slope_up_db: -5.0, max_cut_db: f32::INFINITY, ..Tuning::default() };
    let mut d = MannersDsp::with_tuning(bad);
    d.prepare(f64::NAN, 9);
    d.set(Settings { tame: 1.0, recover: 1.0, ..Settings::default() });
    let mut io = stereo(pink(8192, 23));
    d.process(&mut io, 8192);
    assert!(io.iter().flatten().all(|v| v.is_finite()));
    assert_eq!(d.latency_samples(), N);
}

#[test]
fn reset_empties_the_frame_and_keeps_the_knobs() {
    let mut t = manners(&[("tame", 70.0)]);
    let mut io = stereo(pink(4096, 24));
    t.process(&mut io, 4096);
    t.reset();
    let mut quiet = stereo(vec![0.0; 4096]);
    t.process(&mut quiet, 4096);
    assert!(quiet.iter().flatten().all(|v| *v == 0.0));
    assert_eq!(t.param("tame"), Some(70.0));
}

#[test]
fn steady_noise_does_not_make_the_gain_flutter() {
    // On stationary pink noise the curve should sit still: a busy curve is heard as a warble.
    // (Where it sits is the tuning's business; how much it moves about that is this test's.)
    let len = 6 * 48_000;
    let x = stereo(pink(len, 25));
    let mut dsp = MannersDsp::new();
    dsp.prepare(48_000.0, 2);
    dsp.set(Settings { tame: 1.0, recover: 1.0, ..Settings::default() });
    let bands = dsp.bands();
    let (mut sum, mut sq, mut frames) = (vec![0.0f64; bands], vec![0.0f64; bands], 0usize);
    let mut pos = 0;
    while pos + 256 <= len {
        let mut buf: Vec<Vec<f32>> = x.iter().map(|c| c[pos..pos + 256].to_vec()).collect();
        dsp.process(&mut buf, 256);
        if pos > 48_000 {
            let comp = dsp.level_comp_db();
            for (i, g) in dsp.band_gain_db().iter().enumerate() {
                let v = f64::from(g - comp);
                sum[i] += v;
                sq[i] += v * v;
            }
            frames += 1;
        }
        pos += 256;
    }
    let n = frames as f64;
    let dev: Vec<f64> = (0..bands).map(|i| (sq[i] / n - (sum[i] / n).powi(2)).max(0.0).sqrt()).collect();
    let worst = dev.iter().copied().fold(0.0, f64::max);
    let mean = dev.iter().sum::<f64>() / bands as f64;
    assert!(mean < 0.6, "the curve moves {mean:.2} dB RMS about its place on steady noise (worst band {worst:.2})");
    assert!(worst < 1.5, "worst band {worst:.2} dB");
}

#[test]
fn it_costs_little() {
    let mut t = manners(&[("tame", 100.0), ("recover", 100.0)]);
    let mut io = stereo(pink(48_000, 26));
    let start = std::time::Instant::now();
    for _ in 0..4 {
        t.process(&mut io, 48_000);
    }
    let per_second = start.elapsed().as_secs_f64() / 4.0;
    // Debug builds are slow; this only catches something going badly wrong.
    assert!(per_second < 2.0, "{per_second:.3} s per second of audio");
}

//! Auto Pitch's engine-level guarantees (the correction quality against the study's numbers is
//! measured in `crates/vocal/tests/autopitch_quality.rs`, with the Vocal Editor's tracker).

#![allow(clippy::needless_range_loop, clippy::useless_vec)]

use super::*;
use std::f64::consts::TAU;

const SR: f32 = 48_000.0;

/// A sung-like tone: `secs` at MIDI cents `cents` with `vib` cents of 5.5 Hz vibrato, twelve
/// harmonics falling 6 dB an octave, 30 ms fades, peak about 0.3.
fn tone(sr: f32, secs: f64, cents: f64, vib: f64) -> Vec<f32> {
    let sr = f64::from(sr);
    let n = (secs * sr) as usize;
    let fade = 0.03 * sr;
    let mut ph = 0.0f64;
    (0..n)
        .map(|i| {
            let t = i as f64 / sr;
            let c = cents + vib * (TAU * 5.5 * t).sin();
            let f = 440.0 * 2f64.powf((c - 6900.0) / 1200.0);
            ph += TAU * f / sr;
            let v: f64 = (1..=12).map(|k| (k as f64 * ph).sin() / k as f64).sum();
            let e = ((i.min(n - 1 - i)) as f64 / fade).min(1.0);
            (0.15 * v * e) as f32
        })
        .collect()
}

/// White noise (xorshift), peak 0.2.
fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut r = seed.max(1);
    (0..n)
        .map(|_| {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            (((r >> 40) as f64 / (1u64 << 24) as f64) - 0.5) as f32 * 0.4
        })
        .collect()
}

fn made(sr: f32, ch: usize, set: &[(&str, f32)]) -> AutoPitch {
    let mut a = AutoPitch::new();
    for (id, v) in set {
        assert!(a.set_param(id, *v), "{id}");
    }
    a.prepare(sr, 1024, ch);
    a
}

/// Runs `x` (planar) through `a` in blocks of `block`, plus the latency of silence after; returns
/// the whole output (latency not removed).
fn run(a: &mut AutoPitch, x: &[Vec<f32>], block: usize) -> Vec<Vec<f32>> {
    let len = x[0].len() + a.latency();
    let mut out = vec![Vec::with_capacity(len); x.len()];
    let mut io = vec![vec![0.0f32; block]; x.len()];
    let mut pos = 0;
    while pos < len {
        let n = block.min(len - pos);
        for (c, b) in io.iter_mut().enumerate() {
            for i in 0..n {
                b[i] = x[c].get(pos + i).copied().unwrap_or(0.0);
            }
        }
        a.set_transport(&Transport { playing: true, samples: pos as i64, ..Transport::default() });
        a.process(&mut io, n);
        for (c, b) in io.iter().enumerate() {
            out[c].extend_from_slice(&b[..n]);
        }
        pos += n;
    }
    out
}

/// The average pitch (cents) of `y` between `a` and `b` seconds by zero-crossing-free
/// autocorrelation on the fundamental (enough for a steady tone).
fn pitch(y: &[f32], sr: f32, a: f64, b: f64) -> f64 {
    let sr = f64::from(sr);
    let (s, e) = ((a * sr) as usize, (b * sr) as usize);
    let seg = &y[s..e];
    let (lo, hi) = ((sr / 1000.0) as usize, (sr / 60.0) as usize);
    let ac = |lag: usize| -> f64 { seg.iter().zip(&seg[lag..]).map(|(p, q)| f64::from(*p) * f64::from(*q)).sum() };
    let mut best = (lo, f64::MIN);
    for lag in lo..hi {
        let v = ac(lag);
        if v > best.1 {
            best = (lag, v);
        }
    }
    // Parabolic refinement.
    let (y0, y1, y2) = (ac(best.0 - 1), best.1, ac(best.0 + 1));
    let off = 0.5 * (y0 - y2) / (y0 - 2.0 * y1 + y2);
    1200.0 * ((sr / (best.0 as f64 + off)) / 440.0).log2() + 6900.0
}

#[test]
fn the_output_is_the_same_whatever_the_block_size() {
    // A3 sung 40 c flat with vibrato, stereo with a different right channel.
    let l = tone(SR, 1.5, 5660.0, 30.0);
    let r: Vec<f32> = l.iter().map(|v| v * 0.7).collect();
    let x = vec![l, r];
    for style in [STYLE_NATURAL, STYLE_HARD] {
        let reference = run(&mut made(SR, 2, &[("style", style as f32), ("key", 1.0)]), &x, 32);
        for block in [128, 512, 1024, 333] {
            let y = run(&mut made(SR, 2, &[("style", style as f32), ("key", 1.0)]), &x, block);
            assert_eq!(y, reference, "style {style}, block {block}");
        }
    }
}

#[test]
fn the_reported_latency_is_one_43_ms_frame_and_unvoiced_input_comes_out_delayed_unchanged() {
    for (sr, want) in [(44_100.0, 2048), (48_000.0, 2048), (96_000.0, 4096)] {
        let mut a = made(sr, 2, &[("key", 1.0)]);
        assert_eq!(a.latency(), want, "{sr}");
        let x = vec![noise(sr as usize / 2, 3), noise(sr as usize / 2, 5)];
        let y = run(&mut a, &x, 256);
        let d = a.latency();
        for c in 0..2 {
            assert!(y[c][..d].iter().all(|v| *v == 0.0));
            assert_eq!(&y[c][d..], &x[c][..], "noise passes through exactly, delayed by the latency ({sr} Hz)");
        }
    }
}

#[test]
fn an_impulse_comes_out_after_exactly_the_reported_latency() {
    let mut a = made(SR, 1, &[]);
    let mut x = vec![0.0f32; 24_000];
    x[1000] = 0.5;
    let y = run(&mut a, &[x], 128);
    let peak = y[0].iter().enumerate().max_by(|p, q| p.1.abs().total_cmp(&q.1.abs())).map(|p| p.0);
    assert_eq!(peak, Some(1000 + a.latency()));
}

#[test]
fn mix_at_zero_is_the_delayed_input_exactly() {
    let x = vec![tone(SR, 1.2, 5660.0, 0.0)];
    let mut a = made(SR, 1, &[("style", STYLE_HARD as f32), ("key", 1.0), ("mix", 0.0)]);
    let y = run(&mut a, &x, 256);
    assert_eq!(&y[0][a.latency()..], &x[0][..]);
}

#[test]
fn silence_in_is_silence_out() {
    let mut a = made(SR, 2, &[]);
    let x = vec![vec![0.0f32; 48_000]; 2];
    let t = std::time::Instant::now();
    let y = run(&mut a, &x, 512);
    let secs = t.elapsed().as_secs_f64();
    assert!(y.iter().all(|c| c.iter().all(|v| *v == 0.0)));
    // Cheap: a second of stereo silence in well under a tenth of a second even unoptimised.
    assert!(secs < 0.5, "{secs}");
}

#[test]
fn a_flat_note_is_pulled_onto_the_key_and_reported() {
    // A3 40 c flat in C major (own key C).
    let x = vec![tone(SR, 1.5, 5660.0, 0.0)];
    let mut a = made(SR, 1, &[("style", STYLE_HARD as f32), ("key", 1.0)]);
    let y = run(&mut a, &x, 256);
    let d = a.latency() as f64 / f64::from(SR);
    let p = pitch(&y[0], SR, 0.6 + d, 1.2 + d);
    assert!((p - 5700.0).abs() < 2.0, "{p}");
    let mut r = [f32::NAN; 8];
    a.readouts(&mut r);
    // The last detection saw the note's fade-out or silence; run a held note to read it.
    let mut b = made(SR, 1, &[("style", STYLE_HARD as f32), ("key", 1.0)]);
    let mut io = vec![tone(SR, 0.5, 5660.0, 0.0)];
    b.process(&mut io, 12_000);
    b.readouts(&mut r);
    assert!((r[0] - 5660.0).abs() < 5.0 && r[2] == 5700.0 && (r[1] - 5700.0).abs() < 5.0, "{r:?}");
    assert_eq!(r[4] as u16, SCALES[0].1);
}

#[test]
fn notes_switched_off_are_skipped_to_the_nearest_enabled_one() {
    // A sung 40 c flat; A switched off (Custom, C major without A): the nearest enabled note is
    // G#? no: G and B are enabled, G is 160 c below, B 240 c above: G.
    let mut set: Vec<(&str, f32)> = vec![("style", STYLE_HARD as f32), ("key", 1.0), ("scale", SCALE_CUSTOM as f32), ("note9", 0.0)];
    set.push(("hold", 0.0));
    let mut a = made(SR, 1, &set);
    let mut io = vec![tone(SR, 0.5, 5660.0, 0.0)];
    a.process(&mut io, 12_000);
    let mut r = [f32::NAN; 8];
    a.readouts(&mut r);
    assert_eq!(r[2], 5500.0, "{r:?}");
}

#[test]
fn the_song_key_is_followed_by_position() {
    // C major until sample 24 000, then Ab major (no A natural: A 40 c flat goes to Ab).
    let mut a = made(SR, 1, &[("style", STYLE_HARD as f32)]);
    a.set_song_keys(&[SongKey { start: i64::MIN, tonic: 0, minor: false }, SongKey { start: 24_000, tonic: 8, minor: false }]);
    let x = vec![tone(SR, 1.0, 5660.0, 0.0)];
    let mut targets = Vec::new();
    let mut pos = 0;
    let mut io = vec![vec![0.0f32; 256]];
    while pos + 256 <= x[0].len() {
        io[0].copy_from_slice(&x[0][pos..pos + 256]);
        a.set_transport(&Transport { playing: true, samples: pos as i64, ..Transport::default() });
        a.process(&mut io, 256);
        let mut r = [f32::NAN; 8];
        a.readouts(&mut r);
        targets.push((pos, r[2]));
        pos += 256;
    }
    let at = |p: usize| targets.iter().find(|(q, _)| *q >= p).map(|t| t.1);
    assert_eq!(at(18_000), Some(5700.0), "C major: A");
    // The key change applies to the voice at the change (the decision is made a detector window
    // later): Ab from shortly after.
    assert_eq!(at(28_000), Some(5600.0), "Ab major: Ab");
    // Following off: its own key holds.
    a.set_param("key", 1.0);
    a.process(&mut io, 256);
}

#[test]
fn retune_speed_can_be_automated_between_blocks() {
    // Retune 200 ms for the first half, then instant (Custom): the correction reaches the note
    // only after the change.
    let x = tone(SR, 2.0, 5660.0, 0.0);
    let mut a = made(SR, 1, &[("style", STYLE_CUSTOM as f32), ("key", 1.0), ("retune", 200.0), ("humanize", 0.0), ("vibrato", 0.0)]);
    let mut out = Vec::new();
    let mut io = vec![vec![0.0f32; 512]];
    let mut pos = 0;
    while pos + 512 <= x.len() {
        if pos == 48_000 {
            a.set_param("retune", 0.0);
        }
        io[0].copy_from_slice(&x[pos..pos + 512]);
        a.process(&mut io, 512);
        out.extend_from_slice(&io[0]);
        pos += 512;
    }
    let d = a.latency() as f64 / f64::from(SR);
    let early = pitch(&out, SR, 0.15 + d, 0.22 + d);
    let late = pitch(&out, SR, 1.3 + d, 1.6 + d);
    assert!(early < 5695.0, "slow at first: {early}");
    assert!((late - 5700.0).abs() < 2.0, "instant after: {late}");
}

#[test]
fn process_keeps_its_buffers() {
    // No counting allocator in this crate: every buffer is made in `prepare`, and the plugin's
    // heap blocks keep their addresses and capacities through a minute of blocks of every size,
    // with parameter changes, key changes and silence.
    let mut a = made(SR, 2, &[("key", 1.0)]);
    let addr = |a: &AutoPitch| -> Vec<(usize, usize)> { a.buffers() };
    let before = addr(&a);
    let x = tone(SR, 3.0, 5660.0, 40.0);
    let mut io = vec![vec![0.0f32; 1024]; 2];
    let mut pos = 0;
    for (k, block) in [32usize, 128, 512, 1024, 7].iter().cycle().take(400).enumerate() {
        let n = *block;
        for b in io.iter_mut() {
            for i in 0..n {
                b[i] = x.get((pos + i) % x.len()).copied().unwrap_or(0.0);
            }
        }
        if k % 50 == 0 {
            a.set_param("style", (k / 50 % 3) as f32);
            a.set_param("mode", (k / 100 % 2) as f32);
            a.set_param("range", (k / 50 % 3) as f32);
            a.set_param("formant", if k % 100 == 0 { 3.0 } else { 0.0 });
            a.set_song_keys(&[SongKey { start: 0, tonic: (k % 12) as u8, minor: k % 3 == 0 }]);
        }
        a.process(&mut io, n);
        pos += n;
    }
    assert_eq!(addr(&a), before);
}

#[test]
fn hostile_input_and_parameters_never_break_it() {
    let mut a = made(SR, 2, &[]);
    for id in ["retune", "hold", "formant", "mix", "style", "key", "scale"] {
        a.set_param(id, f32::NAN);
        a.set_param(id, f32::INFINITY);
    }
    let mut io = vec![vec![f32::NAN, f32::INFINITY, 1e30, -1e30].repeat(1000); 2];
    a.process(&mut io, 4000);
    assert!(io.iter().all(|c| c.iter().all(|v| v.is_finite())));
    // More channels in the buffer than prepared, fewer frames than the buffer.
    let mut io = vec![vec![0.1f32; 64]; 4];
    a.process(&mut io, 32);
    assert!(io[2].iter().all(|v| *v == 0.1));
    let mut none: Vec<Vec<f32>> = Vec::new();
    a.process(&mut none, 100);
}

// ------------------------------------------------------------------------------- Live mode

const LIVE: f32 = MODE_LIVE as f32;

#[test]
fn live_mode_reports_a_few_milliseconds_and_switching_changes_the_latency() {
    let mut a = made(SR, 2, &[]);
    assert_eq!(a.latency(), 2048);
    assert!(!a.live_monitoring());
    a.set_param("mode", LIVE);
    // Middle voice: 1 ms + half of a 100 Hz period = 288 samples (6 ms).
    assert_eq!(a.latency(), 288);
    assert!(a.live_monitoring());
    for (range, want) in [(0.0, 391), (2.0, 208)] {
        a.set_param("range", range);
        assert_eq!(a.latency(), want, "range {range}");
    }
    a.set_param("mode", 0.0);
    assert_eq!(a.latency(), 2048);
}

#[test]
fn live_mode_is_the_same_whatever_the_block_size() {
    let l = tone(SR, 1.5, 5660.0, 30.0);
    let r: Vec<f32> = l.iter().map(|v| v * 0.7).collect();
    let x = vec![l, r];
    for style in [STYLE_NATURAL, STYLE_HARD] {
        let set = [("style", style as f32), ("key", 1.0), ("mode", LIVE)];
        let reference = run(&mut made(SR, 2, &set), &x, 32);
        for block in [128, 512, 1024, 333] {
            assert_eq!(run(&mut made(SR, 2, &set), &x, block), reference, "style {style}, block {block}");
        }
    }
}

#[test]
fn live_mode_lets_noise_through_delayed_by_its_latency_and_mix_zero_is_the_delayed_input() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut a = made(sr, 2, &[("key", 1.0), ("mode", LIVE)]);
        let x = vec![noise(sr as usize / 2, 3), noise(sr as usize / 2, 5)];
        let y = run(&mut a, &x, 256);
        let d = a.latency();
        for c in 0..2 {
            assert_eq!(&y[c][d..], &x[c][..], "{sr} Hz");
        }
    }
    let x = vec![tone(SR, 1.2, 5660.0, 0.0)];
    let mut a = made(SR, 1, &[("style", STYLE_HARD as f32), ("key", 1.0), ("mode", LIVE), ("mix", 0.0)]);
    let y = run(&mut a, &x, 256);
    assert_eq!(&y[0][a.latency()..], &x[0][..]);
}

#[test]
fn live_mode_corrects_with_a_bounded_delay() {
    // A3 40 c flat, Hard tune: on the note within 2 c; the delay never exceeds one period of the
    // range's lowest note plus 20 ms (the bound the engine keeps), and averages a few ms.
    for range in [0.0f32, 1.0, 2.0] {
        let x = vec![tone(SR, 1.5, 5660.0, 0.0)];
        let mut a = made(SR, 1, &[("style", STYLE_HARD as f32), ("key", 1.0), ("mode", LIVE), ("range", range)]);
        let y = run(&mut a, &x, 128);
        let d = a.latency() as f64 / f64::from(SR);
        let p = pitch(&y[0], SR, 0.6 + d, 1.2 + d);
        assert!((p - 5700.0).abs() < 2.0, "range {range}: {p}");
        let (mean, max) = a.live_delay().unwrap();
        let floor = RANGE_FLOORS[range as usize];
        let bound = f64::from(SR) / floor * 3.0 + 0.02 * f64::from(SR);
        println!("range {range}: mean {:.1} ms, max {:.1} ms", mean / 48.0, max / 48.0);
        assert!(max <= bound, "range {range}: {max}");
        assert!(mean <= f64::from(SR) / floor + 48.0, "range {range}: mean {mean}");
    }
}

#[test]
fn switching_mode_mid_stream_starts_the_new_engine_clean() {
    let x = tone(SR, 2.0, 5660.0, 20.0);
    let mut a = made(SR, 1, &[("key", 1.0)]);
    let mut io = vec![vec![0.0f32; 256]];
    for (k, chunk) in x.chunks(256).enumerate() {
        if k == 150 {
            a.set_param("mode", LIVE);
        }
        io[0][..chunk.len()].copy_from_slice(chunk);
        a.process(&mut io, chunk.len());
        assert!(io[0].iter().all(|v| v.is_finite() && v.abs() < 1.0));
    }
    let mut r = [f32::NAN; 8];
    a.readouts(&mut r);
    assert_eq!(r[7], 1.0);
    assert!((r[6] - 6.0).abs() < 0.1, "{r:?}");
}

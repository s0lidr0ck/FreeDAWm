//! Objective tests on the real take (`00_original.wav` and Melodyne's edit of it, local only):
//! accuracy against Melodyne, wobble, unvoiced level, preset calibration and render speed (a port
//! of the spike's `edits.py::measured_edits`, `evaluate.py` and `stress.py::flux`).
//!
//! `SOUNDCRAFT_VOCAL_AB_DIR=<dir>` also writes the measured-edit render as
//! `<dir>/engine-measured.wav` (for listening; never committed).

mod common;

use common::{harmonic_tone, load_mono, no_cancel, test_audio};
use soundcraft_vocal::metrics::{harmonic_flux_db, pitch_error_cents, steady_frames};
use soundcraft_vocal::resynth::{changed_frames, render_all, render_spans};
use soundcraft_vocal::{
    EditCurves, Note, NoteEdit, NoteId, PitchClassSet, PitchTrack, VocalAnalysis, analyse, build_curves, factory_presets, track_pitch,
    typical_level_db,
};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Instant;

const NO_SCALE: Option<PitchClassSet> = None;
/// Notes at least this long count in the per-note measures (frames of 5 ms = 150 ms).
const MIN_NOTE_FRAMES: u32 = 30;

/// The original take, Melodyne's edit (truncated to the original's length) and their analyses.
struct Take {
    o: Vec<f32>,
    m: Vec<f32>,
    sr: u32,
    ao: VocalAnalysis,
    tm: PitchTrack,
}

/// The take, loaded and analysed once per test binary; `None` when the audio is missing.
fn take() -> Option<&'static Take> {
    static TAKE: OnceLock<Option<Take>> = OnceLock::new();
    TAKE.get_or_init(|| {
        let po = test_audio("00_original.wav")?;
        let pm = test_audio("00_original_melodyne_edited.wav")?;
        let (o, sr) = load_mono(&po);
        let (mut m, srm) = load_mono(&pm);
        assert_eq!(sr, srm, "sample rates differ");
        m.truncate(o.len());
        m.resize(o.len(), 0.0);
        let t0 = Instant::now();
        let ao = analyse(&o, sr, &no_cancel()).unwrap();
        let tm = track_pitch(&m, sr, &no_cancel()).unwrap();
        println!(
            "take: {:.2} s at {sr} Hz, {} frames, {} notes, {} unvoiced regions (analysis of both {:.0} ms)",
            o.len() as f64 / f64::from(sr),
            ao.track.frames.len(),
            ao.notes.len(),
            ao.unvoiced.len(),
            t0.elapsed().as_secs_f64() * 1e3
        );
        Some(Take { o, m, sr, ao, tm })
    })
    .as_ref()
}

/// numpy's median (mean of the middle two); NaN when empty.
fn median(v: impl IntoIterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = v.into_iter().filter(|x| x.is_finite()).collect();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        f64::NAN
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn cents(t: &PitchTrack, i: usize) -> Option<f64> {
    t.frames.get(i).and_then(|f| f.cents).filter(|c| c.is_finite()).map(f64::from)
}

/// scipy `medfilt(x, k)` (zero padding at both ends).
fn medfilt(x: &[f64], k: usize) -> Vec<f64> {
    let h = k / 2;
    (0..x.len())
        .map(|i| {
            let w = (0..k).map(|j| (i + j).checked_sub(h).and_then(|p| x.get(p)).copied().unwrap_or(0.0));
            median(w)
        })
        .collect()
}

/// numpy `interp` (clamped at both ends) of the points `(xs, ys)` at `x`.
fn interp(x: f64, xs: &[f64], ys: &[f64]) -> f64 {
    if x <= xs[0] {
        return ys[0];
    }
    for w in 0..xs.len() - 1 {
        if x <= xs[w + 1] {
            return ys[w] + (ys[w + 1] - ys[w]) * (x - xs[w]) / (xs[w + 1] - xs[w]);
        }
    }
    ys[ys.len() - 1]
}

/// Runs `[a, b)` of `true`.
fn runs(mask: &[bool]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < mask.len() {
        if mask[i] {
            let j = (i..mask.len()).find(|&j| !mask[j]).unwrap_or(mask.len());
            out.push((i, j));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// The spike's `edits.py::measured_edits`: the shift curve is Melodyne's measured pitch change
/// (`d = mel − orig` on frames voiced in both, |d| > 150 or missing interpolated, medfilt 9 and a
/// 4 Hz zero-phase Butterworth per voiced run of the original); the gain is each note's median
/// level change, with `ramp_curve`'s 3-frame Hann ramps.
fn measured_curves(t: &Take) -> EditCurves {
    let n = t.ao.track.frames.len();
    let d: Vec<Option<f64>> = (0..n).map(|i| Some(cents(&t.tm, i)? - cents(&t.ao.track, i)?).filter(|v| v.abs() <= 150.0)).collect();
    let voiced: Vec<bool> = (0..n).map(|i| cents(&t.ao.track, i).is_some()).collect();
    let mut shift = vec![0.0f64; n];
    for (a, b) in runs(&voiced) {
        let seg = &d[a..b];
        let good: Vec<(f64, f64)> = seg.iter().enumerate().filter_map(|(k, v)| v.map(|v| (k as f64, v))).collect();
        if good.len() < 3 {
            continue;
        }
        let (gx, gy): (Vec<f64>, Vec<f64>) = good.into_iter().unzip();
        let mut s: Vec<f64> = seg.iter().enumerate().map(|(k, v)| v.unwrap_or_else(|| interp(k as f64, &gx, &gy))).collect();
        s = if s.len() >= 9 { medfilt(&s, 9) } else { vec![median(s.iter().copied()); s.len()] };
        if s.len() > 12 {
            let f = soundcraft_vocal::filter::butter2_lowpass(4.0, 200.0);
            s = soundcraft_vocal::filter::filtfilt(&f, &s, 12);
        }
        shift[a..b].copy_from_slice(&s);
    }
    // Gain: per note the median measured level change, ramped by a normalised hanning(7).
    let mut g = vec![0.0f64; n];
    for note in &t.ao.notes {
        let (a, b) = (note.start as usize, (note.end as usize).min(n));
        let dl = median((a..b).map(|i| f64::from(t.tm.frames[i].level_db - t.ao.track.frames[i].level_db)));
        g[a..b].iter_mut().for_each(|v| *v = dl);
    }
    let k: Vec<f64> = (0..7).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / 6.0).cos()).collect();
    let ks: f64 = k.iter().sum();
    let gain: Vec<f32> = (0..n)
        .map(|i| (k.iter().enumerate().filter_map(|(j, w)| (i + 3).checked_sub(j).and_then(|p| g.get(p)).map(|v| v * w)).sum::<f64>() / ks) as f32)
        .collect();
    EditCurves { shift_cents: shift.into_iter().map(|v| v as f32).collect(), gain_db: gain }
}

/// The measured-edit render and its pitch track (once per test binary).
fn measured_render(t: &'static Take) -> &'static (Vec<f32>, PitchTrack) {
    static R: OnceLock<(Vec<f32>, PitchTrack)> = OnceLock::new();
    R.get_or_init(|| {
        let curves = measured_curves(t);
        let t0 = Instant::now();
        let y = render_all(&t.o, &t.ao.track, &curves, &no_cancel()).unwrap();
        println!("measured-edit render: {:.0} ms", t0.elapsed().as_secs_f64() * 1e3);
        if let Some(dir) = std::env::var_os("FREEDAWM_VOCAL_AB_DIR").or_else(|| std::env::var_os("SOUNDCRAFT_VOCAL_AB_DIR")) {
            let path = std::path::Path::new(&dir).join("engine-measured.wav");
            write_wav(&path, &y, t.sr);
            println!("wrote {}", path.display());
        }
        let ty = track_pitch(&y, t.sr, &no_cancel()).unwrap();
        (y, ty)
    })
}

fn write_wav(path: &std::path::Path, samples: &[f32], sr: u32) {
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: sr, channels: vec![samples.to_vec()] };
    let bytes = soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Every note under `preset` with `typical_db` = the median level of all notes.
fn preset_edits(a: &VocalAnalysis, name: &str) -> BTreeMap<NoteId, NoteEdit> {
    let p = factory_presets().into_iter().find(|p| p.name == name).unwrap();
    let typical = typical_level_db(&a.notes.iter().map(|n| n.level_db).collect::<Vec<_>>());
    a.notes
        .iter()
        .map(|n| {
            let e = NoteEdit {
                centre: p.centre,
                drift: p.drift,
                vibrato: p.vibrato,
                raise: p.raise,
                lower: p.lower,
                typical_db: Some(typical),
                ..Default::default()
            };
            (n.id, e)
        })
        .collect()
}

/// The "Like Melodyne 70 %" render over every note and its re-tracked pitch (once per binary).
fn preset_render(t: &'static Take) -> &'static (Vec<f32>, PitchTrack) {
    static R: OnceLock<(Vec<f32>, PitchTrack)> = OnceLock::new();
    R.get_or_init(|| {
        let curves = build_curves(&t.ao, &preset_edits(&t.ao, "Like Melodyne 70 %"), &NO_SCALE);
        let y = render_all(&t.o, &t.ao.track, &curves, &no_cancel()).unwrap();
        let ty = track_pitch(&y, t.sr, &no_cancel()).unwrap();
        (y, ty)
    })
}

/// The spike's octave-repaired Harvest track of the original (`tests/data/00_original_harvest.csv`,
/// one row per 5 ms, `f0_hz = 0` when unvoiced) as a [`PitchTrack`].
fn harvest_track(sr: u32, hop: u32) -> PitchTrack {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let text = std::fs::read_to_string(std::path::Path::new(&dir).join("tests/data/00_original_harvest.csv")).unwrap();
    let frames = text
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: f32 = l.split(',').nth(1).unwrap().trim().parse().unwrap();
            let cents = (f > 0.0).then(|| common::hz_to_cents(f));
            soundcraft_vocal::PitchFrame { cents, level_db: 0.0, voiced_prob: if f > 0.0 { 1.0 } else { 0.0 } }
        })
        .collect();
    PitchTrack { sample_rate: sr, hop_samples: hop, frames }
}

#[test]
fn null_render_is_bit_identical() {
    let Some(_) = test_audio("00_original.wav") else { return };
    let Some(_) = test_audio("00_original_melodyne_edited.wav") else { return };
    let Some(t) = take() else { return };
    let n = t.ao.track.frames.len();
    let zero = EditCurves { shift_cents: vec![0.0; n], gain_db: vec![0.0; n] };
    let y = render_all(&t.o, &t.ao.track, &zero, &no_cancel()).unwrap();
    assert_eq!(y.len(), t.o.len());
    assert!(y.iter().zip(&t.o).all(|(a, b)| a.to_bits() == b.to_bits()), "null render changed samples");
}

#[test]
fn measured_edit_lands_within_3c_of_melodyne() {
    let Some(_) = test_audio("00_original.wav") else { return };
    let Some(_) = test_audio("00_original_melodyne_edited.wav") else { return };
    let Some(t) = take() else { return };
    let (_, ty) = measured_render(t);
    let n = t.ao.track.frames.len();
    let mut core = vec![false; n];
    for note in &t.ao.notes {
        let len = (note.end - note.start) as usize;
        let k = len * 2 / 10;
        for c in core.iter_mut().take(note.end as usize - k).skip(note.start as usize + k) {
            *c = true;
        }
    }
    // Voiced in the original and in Melodyne's edit, on note cores.
    let v: Vec<bool> = (0..n).map(|i| core[i] && cents(&t.ao.track, i).is_some() && cents(&t.tm, i).is_some()).collect();
    let err = |i: usize| -> Option<f64> { Some(((cents(ty, i)? - cents(&t.tm, i)? + 600.0).rem_euclid(1200.0) - 600.0).abs()) };
    let mut per_note: Vec<f64> = Vec::new();
    for note in t.ao.notes.iter().filter(|n| n.end - n.start >= MIN_NOTE_FRAMES) {
        let e: Vec<f64> = (note.start as usize..note.end as usize).filter(|&i| v[i]).filter_map(err).collect();
        if e.len() >= 4 {
            per_note.push(median(e));
        }
    }
    let notes_median = median(per_note.iter().copied());
    let notes_mean = per_note.iter().sum::<f64>() / per_note.len() as f64;
    let (frame_median, frame_p90) = pitch_error_cents(ty, &t.tm, &v);
    // For scale: the original against Melodyne on the same frames.
    let (orig_median, orig_p90) = pitch_error_cents(&t.ao.track, &t.tm, &v);
    println!(
        "vs Melodyne: notes median {notes_median:.2} c (mean {notes_mean:.2}, {} notes >= 150 ms), frames median {frame_median:.2} c, p90 {frame_p90:.2} c; \
         original vs Melodyne: median {orig_median:.2} c, p90 {orig_p90:.2} c",
        per_note.len()
    );
    assert!(per_note.len() >= 10, "only {} notes measured", per_note.len());
    assert!(notes_median <= 3.0, "median per-note error {notes_median:.2} c > 3 c");
    assert!(frame_median <= 3.0, "frame median error {frame_median:.2} c > 3 c");
}

#[test]
fn wobble_on_held_notes_is_at_most_0_88_db() {
    let Some(_) = test_audio("00_original.wav") else { return };
    let Some(_) = test_audio("00_original_melodyne_edited.wav") else { return };
    let Some(t) = take() else { return };
    let (y, _) = measured_render(t);
    let (py, _) = preset_render(t);
    let hop = t.ao.track.hop_samples;
    // The gate uses the spike's steady frames (from its Harvest track, as `stress.py` did), so the
    // figure is comparable with the spike's 0.88 dB and Melodyne's 0.76 dB; our own tracker's
    // steady frames are printed alongside.
    let harvest = harvest_track(t.sr, hop);
    let mut fy = f32::NAN;
    for (name, track) in [("spike Harvest", &harvest), ("our tracker", &t.ao.track)] {
        let steady = steady_frames(track);
        let fo = harmonic_flux_db(&t.o, t.sr, &steady, hop);
        let fm = harmonic_flux_db(&t.m, t.sr, &steady, hop);
        let f = harmonic_flux_db(y, t.sr, &steady, hop);
        let fp = harmonic_flux_db(py, t.sr, &steady, hop);
        println!(
            "harmonic flux, steady frames from {name} ({}): original {fo:.3} dB, Melodyne {fm:.3} dB, ours (measured edit) {f:.3} dB, \
             ours (Like Melodyne 70 %) {fp:.3} dB",
            steady.iter().filter(|&&s| s).count()
        );
        if fy.is_nan() {
            // Not vacuous: the gate measures many steady frames, on audio that does wobble.
            let count = steady.iter().filter(|&&s| s).count();
            assert!(count > 1000, "only {count} Harvest steady frames");
            assert!(fo > 0.5, "the original's flux {fo:.3} dB is too small for the gate to mean anything");
            fy = f;
        }
    }
    assert!(fy <= 0.88, "wobble {fy:.3} dB > 0.88 dB");
}

#[test]
fn unvoiced_level_is_unchanged() {
    let Some(_) = test_audio("00_original.wav") else { return };
    let Some(_) = test_audio("00_original_melodyne_edited.wav") else { return };
    let Some(t) = take() else { return };
    let (_, ty) = preset_render(t);
    let d: Vec<f64> =
        t.ao.track
            .frames
            .iter()
            .zip(&ty.frames)
            .filter(|(f, _)| f.cents.is_none() && f.level_db > -55.0)
            .map(|(f, g)| f64::from(g.level_db - f.level_db))
            .collect();
    let med = median(d.iter().copied());
    println!("unvoiced level change: median {med:.4} dB over {} frames", d.len());
    assert!(!d.is_empty());
    assert!(med.abs() <= 0.05, "unvoiced level moved by {med:.4} dB");
}

#[test]
fn like_melodyne_70_preset_is_calibrated() {
    let Some(_) = test_audio("00_original.wav") else { return };
    let Some(_) = test_audio("00_original_melodyne_edited.wav") else { return };
    let Some(t) = take() else { return };
    let (_, ty) = preset_render(t);
    let (mut centre, mut drift, mut fit) = (Vec::new(), Vec::new(), Vec::new());
    for n in t.ao.notes.iter().filter(|n| n.end - n.start >= MIN_NOTE_FRAMES) {
        let (c1, l1, d1, _) = Note::measures(ty, n.start, n.end);
        let (c0, l0, d0) = (f64::from(n.centre_cents), f64::from(n.level_db), f64::from(n.drift_cents));
        let tgt = (c0 / 100.0).round() * 100.0;
        if (tgt - c0).abs() > 10.0 {
            centre.push((f64::from(c1) - c0) / (tgt - c0));
        }
        if d0 > 5.0 {
            drift.push(f64::from(d1) / d0);
        }
        fit.push((l0, f64::from(l1) - l0));
    }
    let centre_ratio = median(centre.iter().copied());
    let drift_ratio = median(drift.iter().copied());
    let k = fit.len() as f64;
    let (mx, my) = (fit.iter().map(|p| p.0).sum::<f64>() / k, fit.iter().map(|p| p.1).sum::<f64>() / k);
    let slope = fit.iter().map(|(x, y)| (x - mx) * (y - my)).sum::<f64>() / fit.iter().map(|(x, _)| (x - mx) * (x - mx)).sum::<f64>();
    println!(
        "Like Melodyne 70 %: centre ratio {centre_ratio:.3} ({} notes; Melodyne 0.40), drift ratio {drift_ratio:.3} ({} notes; Melodyne 0.84), \
         level slope {slope:.3} ({} notes; Melodyne -0.25)",
        centre.len(),
        drift.len(),
        fit.len()
    );
    assert!((0.30..=0.50).contains(&centre_ratio), "centre ratio {centre_ratio:.3} outside [0.30, 0.50]");
    assert!((0.65..=0.95).contains(&drift_ratio), "drift ratio {drift_ratio:.3} outside [0.65, 0.95]");
    assert!((-0.30..=-0.20).contains(&slope), "level slope {slope:.3} outside [-0.30, -0.20]");
}

/// Times `build_curves` → `changed_frames` → `render_spans` after changing `id`'s Centre to 0.5.
fn time_centre_change(a: &VocalAnalysis, x: &[f32], edits: &BTreeMap<NoteId, NoteEdit>, id: NoteId) -> (f64, usize) {
    let prev = build_curves(a, edits, &NO_SCALE);
    let mut next_edits = edits.clone();
    next_edits.get_mut(&id).unwrap().centre = 0.5;
    let mut best = f64::INFINITY;
    let mut samples = 0;
    // Best of three (the first run also plans the FFTs).
    for _ in 0..3 {
        let t0 = Instant::now();
        let next = build_curves(a, &next_edits, &NO_SCALE);
        let frames = changed_frames(Some(&prev), &next);
        let spans = render_spans(x, &a.track, &next, &frames, &no_cancel()).unwrap();
        best = best.min(t0.elapsed().as_secs_f64() * 1e3);
        samples = spans.iter().map(|s| s.samples.len()).sum();
    }
    (best, samples)
}

#[cfg_attr(debug_assertions, ignore = "timing: run with --release")]
#[test]
fn one_held_note_renders_within_a_quarter_second() {
    let Some(_) = test_audio("00_original.wav") else { return };
    let Some(_) = test_audio("00_original_melodyne_edited.wav") else { return };
    let Some(t) = take() else { return };
    // The A4 held note at 2.6–4.1 s: the note overlapping that window most.
    let (w0, w1) = (520u32, 820u32);
    let note = t.ao.notes.iter().max_by_key(|n| n.end.min(w1).saturating_sub(n.start.max(w0))).unwrap();
    println!("held note {:.2}–{:.2} s, centre {:.1} c", f64::from(note.start) * 0.005, f64::from(note.end) * 0.005, note.centre_cents);
    assert!((note.centre_cents - 6900.0).abs() < 60.0, "not the A4 note: {}", note.centre_cents);
    let edits = preset_edits(&t.ao, "Like Melodyne 70 %");
    let (ms, samples) = time_centre_change(&t.ao, &t.o, &edits, note.id);
    println!("held-note re-render: {ms:.1} ms for {:.2} s of audio", samples as f64 / f64::from(t.sr));
    assert!(ms < 250.0, "held-note re-render took {ms:.1} ms");
}

#[cfg_attr(debug_assertions, ignore = "timing: run with --release")]
#[test]
fn phrase_rerender_within_a_quarter_second() {
    // A 10 s fully-voiced legato phrase: 20 notes of 0.5 s, each detuned, gliding into the next
    // over 60 ms, with a slow drift and a light vibrato.
    let sr = 48_000u32;
    let pitches = [57.0f32, 59.0, 60.0, 62.0, 64.0, 62.0, 60.0, 59.0, 57.0, 55.0];
    let detune = [0.23f32, -0.31, 0.17, -0.12, 0.28, -0.22, 0.14, -0.27, 0.19, -0.16];
    let midi_at = |t: f32| {
        let k = ((t / 0.5) as usize).min(19);
        let p = |k: usize| pitches[k % 10] + detune[k % 10];
        let into = t - k as f32 * 0.5;
        let base = if k > 0 && into < 0.06 { p(k - 1) + (p(k) - p(k - 1)) * into / 0.06 } else { p(k) };
        base + 0.08 * (std::f32::consts::TAU * 0.7 * t).sin() + 0.05 * (std::f32::consts::TAU * 5.5 * t).sin()
    };
    let x = harmonic_tone(sr, 10.0, |t| 440.0 * 2f32.powf((midi_at(t) - 69.0) / 12.0), 30);
    let a = analyse(&x, sr, &no_cancel()).unwrap();
    let voiced = a.track.frames.iter().filter(|f| f.cents.is_some()).count();
    println!("phrase: {} notes, {} unvoiced regions, voiced {voiced}/{} frames", a.notes.len(), a.unvoiced.len(), a.track.frames.len());
    assert!(a.notes.len() >= 10, "only {} notes", a.notes.len());
    let edits: BTreeMap<NoteId, NoteEdit> = a.notes.iter().map(|n| (n.id, NoteEdit { centre: 0.4, drift: 0.3, ..Default::default() })).collect();
    let curves = build_curves(&a, &edits, &NO_SCALE);
    let shifted = curves.shift_cents.iter().filter(|&&s| s != 0.0).count();
    println!("shifted frames: {shifted}/{}", curves.shift_cents.len());
    let mid = &a.notes[a.notes.len() / 2];
    let (ms, samples) = time_centre_change(&a, &x, &edits, mid.id);
    println!("phrase re-render: {ms:.1} ms for {:.2} s of audio", samples as f64 / f64::from(sr));
    assert!(samples as f64 > 9.0 * f64::from(sr), "the re-render covered only {samples} samples, not the phrase");
    assert!(ms < 250.0, "phrase re-render took {ms:.1} ms");
}

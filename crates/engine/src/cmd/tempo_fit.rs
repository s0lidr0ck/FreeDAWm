//! Tempo from a MIDI performance (Event › Tempo from MIDI Performance; `event.tempo_from_midi`):
//! a clip played freely, without the click, gives the song its tempo. The clip's note onsets are
//! tracked as beats: an onset strength curve (200 frames a second, each note its velocity,
//! smoothed), a beat period from its autocorrelation between `min_bpm` and `max_bpm` (leaning to
//! 120 bpm, a log-normal prior an octave wide), then dynamic programming that picks the beats
//! near onsets keeping a steady period (Ellis, "Beat tracking by dynamic programming", 2007), each
//! beat moved onto the strongest onset within a fifth of a beat of it. A tempo point is written at
//! each beat so the beats land on the quarter-note grid: the first beat on the beat line after
//! where it now falls (the beat or two before it stretched or shrunk to meet it), the tempo
//! between each beat and the next from their distance, the last held. What was on the tempo map
//! between those lines goes; changes before and after stay.
//!
//! The performance keeps its sound: the clip stays at its samples and its notes and controllers
//! are given new ticks in the new map (the clip is kept out of the tick-track move of bug 3).
//! Other MIDI clips on tick tracks follow the new tempo as they do any tempo edit.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{ClipContent, ClipId};
use soundcraft_time::TICKS_PER_QUARTER;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "event.tempo_from_midi",
        "Tempo from MIDI Performance",
        [],
        None,
        "{clip?: the selected MIDI clip, min_bpm?: 60, max_bpm?: 200} -> {beats, bpm: [first beat's…], points} — tracks the clip's beats and writes a tempo point at each so they land on the quarter-note grid; the clip's notes keep their samples",
        always,
        tempo_from_midi
    )]
}

/// Frames a second of the onset curve.
const FPS: f64 = 200.0;

/// The beats of onsets `(seconds, weight)` (seconds from the first onset's frame base), in
/// seconds, tracked with periods between `min_bpm` and `max_bpm`. `None` with too few onsets or
/// no period.
pub fn track_beats(onsets: &[(f64, f64)], min_bpm: f64, max_bpm: f64) -> Option<Vec<f64>> {
    if onsets.len() < 4 {
        return None;
    }
    let last = onsets.iter().map(|o| o.0).fold(0.0f64, f64::max);
    let frames = ((last + 1.0) * FPS).ceil() as usize + 1;
    if frames > 200 * 60 * 30 {
        return None;
    }
    // The onset curve, smoothed with a Gaussian of 10 ms.
    let mut raw = vec![0.0f64; frames];
    for (t, w) in onsets {
        if let Some(f) = raw.get_mut((t * FPS).round().max(0.0) as usize) {
            *f += w.max(0.0);
        }
    }
    let sigma = 2.0f64;
    let kernel: Vec<f64> = (-6i64..=6).map(|k| (-(k as f64).powi(2) / (2.0 * sigma * sigma)).exp()).collect();
    let mut env = vec![0.0f64; frames];
    for (i, e) in env.iter_mut().enumerate() {
        for (j, k) in kernel.iter().enumerate() {
            let src = i as i64 + j as i64 - 6;
            if let Some(v) = usize::try_from(src).ok().and_then(|s| raw.get(s)) {
                *e += v * k;
            }
        }
    }
    let mean = env.iter().sum::<f64>() / frames as f64;
    let sd = (env.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / frames as f64).sqrt().max(1e-9);
    for e in env.iter_mut() {
        *e /= sd;
    }
    // The period: autocorrelation weighted towards 120 bpm.
    let (lo, hi) = ((60.0 / max_bpm * FPS).floor().max(2.0) as usize, (60.0 / min_bpm * FPS).ceil() as usize);
    if hi <= lo || hi >= frames {
        return None;
    }
    let ac = |lag: usize| env.iter().zip(env.iter().skip(lag)).map(|(a, b)| a * b).sum::<f64>();
    let centre = 60.0 / 120.0 * FPS;
    let mut best = (lo, f64::MIN);
    for lag in lo..=hi {
        let prior = (-0.5 * ((lag as f64 / centre).log2()).powi(2)).exp();
        let score = ac(lag) * prior;
        if score > best.1 {
            best = (lag, score);
        }
    }
    let period = best.0 as f64;
    // Dynamic programming: each frame's best score with a beat there and the beat before it.
    let alpha = 100.0;
    let mut score = env.clone();
    let mut back = vec![usize::MAX; frames];
    let (near, far) = ((period / 2.0).round() as usize, (period * 2.0).round() as usize);
    for t in 0..frames {
        let mut top = f64::MIN;
        for prev in t.saturating_sub(far)..t.saturating_sub(near).max(1).min(t) {
            let gap = (t - prev) as f64 / period;
            let s = score.get(prev).copied().unwrap_or(0.0) - alpha * gap.ln().powi(2);
            if s > top {
                top = s;
                if let Some(b) = back.get_mut(t) {
                    *b = prev;
                }
            }
        }
        if top > 0.0
            && let (Some(s), Some(e)) = (score.get_mut(t), env.get(t))
        {
            *s = e + top;
        } else if let Some(b) = back.get_mut(t) {
            *b = usize::MAX;
        }
    }
    // The last beat: the best score within a period of the end of the onsets.
    let end = ((last * FPS).round() as usize).min(frames - 1);
    let from = end.saturating_sub(period.round() as usize);
    let at = |f: usize| score.get(f).copied().unwrap_or(f64::MIN);
    let mut t = (from..=end).max_by(|a, b| at(*a).total_cmp(&at(*b))).unwrap_or(end);
    let mut beats = vec![t];
    while let Some(&p) = back.get(t) {
        if p == usize::MAX || beats.len() > frames {
            break;
        }
        beats.push(p);
        t = p;
    }
    beats.reverse();
    // Each beat onto the strongest onset near it.
    let reach = period / FPS / 5.0;
    let out: Vec<f64> = beats
        .into_iter()
        .map(|f| {
            let at = f as f64 / FPS;
            onsets.iter().filter(|o| (o.0 - at).abs() <= reach).max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.total_cmp(&a.0))).map_or(at, |o| o.0)
        })
        .collect();
    let mut dedup: Vec<f64> = Vec::with_capacity(out.len());
    for b in out {
        if dedup.last().is_none_or(|l| b > *l + 1e-4) {
            dedup.push(b);
        }
    }
    (dedup.len() >= 3).then_some(dedup)
}

fn tempo_from_midi(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "event.tempo_from_midi";
    let s = e.session();
    let id = match p.get("clip").and_then(Value::as_u64) {
        Some(c) => ClipId(c),
        None => s
            .edit
            .selected_clips
            .iter()
            .copied()
            .find(|c| s.find_clip(*c).is_some_and(|(_, c)| matches!(c.content, ClipContent::Midi { .. })))
            .ok_or_else(|| bad(C, "select a MIDI clip played without the click"))?,
    };
    let (min_bpm, max_bpm) = (f64_or(p, "min_bpm", 60.0).clamp(20.0, 400.0), f64_or(p, "max_bpm", 200.0).clamp(20.0, 400.0));
    if max_bpm <= min_bpm * 1.2 {
        return Err(bad(C, "`max_bpm` must be well above `min_bpm`"));
    }
    let Some((_, clip)) = s.find_clip(id) else { return Err(bad(C, "no such clip")) };
    let ClipContent::Midi { sequence } = &clip.content else { return Err(bad(C, "not a MIDI clip")) };
    let (sr, old) = (s.sample_rate, s.tempo.clone());
    let hz = f64::from(sr.hz());
    let base = old.samples_to_ticks_f(clip.start, sr);
    let at = |tick: i64| old.ticks_f_to_samples_f(base + tick as f64, sr);
    // The onsets in the clip's window, in samples.
    let mut onsets: Vec<(f64, f64)> = sequence
        .notes
        .iter()
        .filter(|n| !n.muted && n.start >= 0)
        .map(|n| (at(n.start), f64::from(n.velocity) / 127.0))
        .filter(|(t, _)| *t < clip.end() as f64)
        .collect();
    onsets.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some(first) = onsets.first().map(|o| o.0) else { return Err(bad(C, "the clip has no notes")) };
    let secs: Vec<(f64, f64)> = onsets.iter().map(|(t, w)| ((t - first) / hz, *w)).collect();
    let beats = track_beats(&secs, min_bpm, max_bpm).ok_or_else(|| bad(C, "too few notes, or no steady beat, to take a tempo from"))?;
    let beats: Vec<f64> = beats.into_iter().map(|b| first + b * hz).collect();
    let (Some(&b0), Some(&b_last)) = (beats.first(), beats.last()) else { return Err(bad(C, "no beats found")) };
    // Where the first beat lands: the beat line at or after where it now is.
    let q = TICKS_PER_QUARTER as f64;
    let t0 = old.samples_to_ticks_f(b0.round() as i64, sr);
    let mut start = (t0 / q).ceil() as i64 * TICKS_PER_QUARTER;
    let first_bpm = 60.0 * hz / beats.get(1).map_or(hz * 0.5, |b| b - b0);
    let mut new = old.clone();
    let mut lead: Option<(i64, f64)> = None;
    if (t0 - start as f64).abs() > 0.5 {
        // The beat line before it (or the one before that, so the stretch stays under twice
        // the performance's speed) is kept, and the time up to the first beat is its tempo.
        let mut from = start - TICKS_PER_QUARTER;
        let gap = |f: i64| b0 - old.ticks_f_to_samples_f(f as f64, sr);
        while from >= TICKS_PER_QUARTER && gap(from) * first_bpm < 0.5 * 60.0 * hz * ((start - from) as f64 / q) {
            from -= TICKS_PER_QUARTER;
        }
        if from < 0 || gap(from) <= 0.0 {
            // Nothing fits before it: the first beat moves to the next line instead.
            start += TICKS_PER_QUARTER;
            from = start - TICKS_PER_QUARTER;
        }
        let g = gap(from);
        if g <= 0.0 {
            return Err(bad(C, "the performance starts too close to the song start to fit"));
        }
        lead = Some((from, 60.0 * hz * ((start - from) as f64 / q) / g));
    }
    let end_tick = start.saturating_add(TICKS_PER_QUARTER.saturating_mul(beats.len() as i64 - 1));
    let clear_from = lead.map_or(start, |l| l.0);
    let gone: Vec<i64> = new.tempos().iter().map(|t| t.tick).filter(|t| *t > clear_from && *t <= end_tick).collect();
    for t in gone {
        new.remove_tempo(t);
    }
    let mut bpms = Vec::new();
    let put = |map: &mut soundcraft_time::TempoMap, tick: i64, bpm: f64| -> Result<()> {
        map.set_tempo(tick, bpm).map_err(|err| bad(C, format!("tempo {bpm:.1} at tick {tick}: {err}")))?;
        map.set_ramp(tick, false);
        Ok(())
    };
    if let Some((tick, bpm)) = lead {
        put(&mut new, tick, bpm)?;
    }
    for (k, w) in beats.windows(2).enumerate() {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        let bpm = 60.0 * hz / (b - a);
        put(&mut new, start + TICKS_PER_QUARTER * k as i64, bpm)?;
        bpms.push((bpm * 100.0).round() / 100.0);
    }
    // After the last beat the original tempo resumes where it was in force.
    let resume = old.tempo_at_tick(old.samples_to_ticks(b_last.round() as i64, sr));
    if !new.tempos().iter().any(|t| t.tick > end_tick) {
        put(&mut new, end_tick, *bpms.last().unwrap_or(&resume))?;
    }
    // The clip's notes and controllers keep their samples: new ticks in the new map.
    let new_base = new.samples_to_ticks_f(clip.start, sr);
    let retick = |tick: i64| -> i64 {
        let sample = old.ticks_f_to_samples_f(base + tick as f64, sr);
        (new.samples_to_ticks_f(sample.round() as i64, sr) - new_base).round() as i64
    };
    let mut seq = sequence.clone();
    for n in seq.notes.iter_mut() {
        let (a, b) = (retick(n.start), retick(n.end()));
        n.start = a;
        n.length = (b - a).max(1);
    }
    for c in seq.ctrls.iter_mut() {
        c.tick = retick(c.tick);
    }
    seq.sort();
    let n_points = beats.len() + usize::from(lead.is_some());
    let n_beats = beats.len();
    let doc = e.session_mut();
    doc.tempo = new;
    if let Some(c) = doc.find_clip_mut(id) {
        c.content = ClipContent::Midi { sequence: seq };
    }
    e.tempo_pinned.push(id);
    Ok(json!({"beats": n_beats, "bpm": bpms, "points": n_points}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;

    /// A clip played at a tempo drifting from 100 to 110 bpm (a note on every beat, a chord on
    /// the odd ones), recorded against the default 120 bpm map.
    fn drifting(e: &mut Engine) -> (u64, Vec<i64>) {
        e.execute("track.new", &json!({"name": "Free", "kind": "midi"})).unwrap();
        let sr = f64::from(e.session().sample_rate.hz());
        let clip =
            e.execute("midi.new_clip", &json!({"track": "Free", "start": 30_000, "end": 30_000 + 48_000 * 24})).unwrap()["clip"].as_u64().unwrap();
        let beats = 32;
        let mut t = 0.0f64;
        let mut samples = Vec::new();
        for k in 0..beats {
            let bpm = 100.0 + 10.0 * k as f64 / (beats - 1) as f64;
            samples.push(30_000 + (t * sr).round() as i64);
            t += 60.0 / bpm;
        }
        let map = e.session().tempo.clone();
        let base = map.samples_to_ticks_f(30_000, e.session().sample_rate);
        for (k, s) in samples.iter().enumerate() {
            let tick = (map.samples_to_ticks_f(*s, e.session().sample_rate) - base).round() as i64;
            let vel = if k % 2 == 0 { 110 } else { 70 };
            e.execute("midi.note_add", &json!({"clip": clip, "pitch": 48, "start_ticks": tick, "length_ticks": 200, "velocity": vel})).unwrap();
            if k % 2 == 1 {
                e.execute("midi.note_add", &json!({"clip": clip, "pitch": 55, "start_ticks": tick + 120, "length_ticks": 100, "velocity": 40}))
                    .unwrap();
            }
        }
        (clip, samples)
    }

    fn note_samples(e: &Engine, clip: u64) -> Vec<i64> {
        let s = e.session();
        let (_, c) = s.find_clip(ClipId(clip)).unwrap();
        let ClipContent::Midi { sequence } = &c.content else { panic!() };
        let base = s.tempo.samples_to_ticks_f(c.start, s.sample_rate);
        sequence
            .notes
            .iter()
            .filter(|n| n.pitch == 48)
            .map(|n| s.tempo.ticks_f_to_samples_f(base + n.start as f64, s.sample_rate).round() as i64)
            .collect()
    }

    #[test]
    fn a_drifting_performance_gives_its_drift_and_keeps_its_samples() {
        let mut e = Engine::default();
        let (clip, samples) = drifting(&mut e);
        let before = note_samples(&e, clip);
        let r = e.execute("event.tempo_from_midi", &json!({"clip": clip})).unwrap();
        assert_eq!(r["beats"], 32, "{r}");
        let bpm: Vec<f64> = r["bpm"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        for (k, b) in bpm.iter().enumerate() {
            let want = 100.0 + 10.0 * k as f64 / 31.0;
            assert!((b - want).abs() < 1.0, "beat {k}: {b} against {want}");
        }
        // Every note where it was played, give or take a sample of rounding per tick.
        let now = note_samples(&e, clip);
        for (a, b) in now.iter().zip(&samples) {
            assert!((a - b).abs() <= 30, "{a} against {b}");
        }
        // And every beat on a quarter-note line.
        let s = e.session();
        for b in &samples {
            let t = s.tempo.samples_to_ticks_f(*b, s.sample_rate);
            let off = (t / TICKS_PER_QUARTER as f64).fract();
            assert!(!(0.02..=0.98).contains(&off), "beat at tick {t}");
        }
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().tempo.tempos().len(), 1, "one undo step");
        assert_eq!(note_samples(&e, clip), before);
    }

    #[test]
    fn too_few_notes_are_refused() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "Few", "kind": "midi"})).unwrap();
        let clip = e.execute("midi.new_clip", &json!({"track": "Few", "start": 0, "end": 96_000})).unwrap()["clip"].clone();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": 0})).unwrap();
        assert!(e.execute("event.tempo_from_midi", &json!({"clip": clip})).is_err());
    }
}

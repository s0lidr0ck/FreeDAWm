//! Tools that write MIDI from a few choices (Event › More MIDI):
//!
//! - **Write Arpeggio** (`event.write_arpeggio`): a MIDI clip's held notes played through an
//!   arpeggiator and written back as the notes it plays. With MIDI effects on the clip's track
//!   (and not bypassed) those are what is printed, so the clip then sounds as it did and plays past
//!   them (`Clip::midi_fx_printed`, as Record Effects' Output does); without, an arpeggiator with
//!   the rate, pattern, octaves and gate given. The same effect code runs offline as the mix runs
//!   it live ([`soundcraft_mix::midi_fx::print`]), on the song's grid. One undo step.
//! - **Write Progression** (`event.write_progression`): a new MIDI clip on a MIDI or instrument
//!   track over the edit selection (else four bars from the insertion point) of voiced chords:
//!   the Band's chord lane where it has chords, or a progression in the key (`I-V-vi-IV` and the
//!   others in [`PROGRESSIONS`], or any degrees typed so) a chord a bar, played whole, in halves,
//!   in quarters or strummed. Chords are voiced close in the octave from C3 up with the root an
//!   octave below; each moves to the inversion nearest the one before.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{ClipContent, ClipId, MidiFx, MidiFxKind, TrackId};
use soundcraft_time::{Range, TICKS_PER_QUARTER};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "event.write_arpeggio",
            "Write Arpeggio",
            [],
            None,
            "{clip?: the selected MIDI clip, rate?: 1/16 (1/4 1/8 1/8T 1/16 1/16T 1/32 1/32T), pattern?: up|down|updown|played|random, octaves?: 1..4, gate?: 50 (percent)} -> {notes, printed: the track's own effects were used}",
            always,
            write_arpeggio
        ),
        cmd!(
            "event.write_progression",
            "Write Progression",
            [],
            None,
            "{track?: the selected MIDI track, start?, end?: samples (the edit selection; else 4 bars from the insertion point), progression?: lane|I-V-vi-IV|I-vi-IV-V|ii-V-I|I-IV-V-I|vi-IV-I-V|i-VI-III-VII|i-iv-v-i (lane: the chord lane, else I-V-vi-IV), rhythm?: whole|half|quarter|strum, velocity?: 90} -> {clip, chords}",
            always,
            write_progression
        ),
    ]
}

/// The progressions offered by name (roman numerals of the key's degrees, a chord a bar).
pub const PROGRESSIONS: [&str; 7] = ["I-V-vi-IV", "I-vi-IV-V", "ii-V-I", "I-IV-V-I", "vi-IV-I-V", "i-VI-III-VII", "i-iv-v-i"];

fn rate_index(name: &str) -> Option<u8> {
    soundcraft_model::MIDI_FX_RATES.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)).and_then(|i| u8::try_from(i).ok())
}

fn pattern_index(name: &str) -> Option<u8> {
    let i = match name.trim().to_ascii_lowercase().replace([' ', '/', '-', '_'], "").as_str() {
        "up" => 0,
        "down" => 1,
        "updown" => 2,
        "played" | "asplayed" => 3,
        "random" => 4,
        _ => return None,
    };
    Some(i)
}

fn midi_clip_param(e: &Engine, p: &Value, cmd: &str) -> Result<ClipId> {
    let s = e.session();
    let id = match p.get("clip").and_then(Value::as_u64) {
        Some(c) => ClipId(c),
        None => s
            .edit
            .selected_clips
            .iter()
            .copied()
            .find(|c| s.find_clip(*c).is_some_and(|(_, c)| matches!(c.content, ClipContent::Midi { .. })))
            .ok_or_else(|| bad(cmd, "select a MIDI clip"))?,
    };
    match s.find_clip(id) {
        Some((_, c)) if matches!(c.content, ClipContent::Midi { .. }) => Ok(id),
        Some(_) => Err(bad(cmd, "not a MIDI clip")),
        None => Err(bad(cmd, "no such clip")),
    }
}

fn write_arpeggio(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "event.write_arpeggio";
    let id = midi_clip_param(e, p, C)?;
    let s = e.session();
    let Some((track, clip)) = s.find_clip(id) else { return Err(bad(C, "no such clip")) };
    if clip.midi_fx_printed {
        return Err(bad(C, "the clip already holds its track's MIDI effects' output"));
    }
    let ClipContent::Midi { sequence } = &clip.content else { return Err(bad(C, "not a MIDI clip")) };
    let own: Vec<MidiFx> = s.track(track).map(|t| t.midi_fx.iter().copied().filter(|f| !f.bypass).collect()).unwrap_or_default();
    let printed = !own.is_empty();
    let fx = if printed {
        own
    } else {
        let mut arp = MidiFx::new(MidiFxKind::Arpeggiator);
        if let Some(r) = str_param(p, "rate") {
            arp.rate = rate_index(r).ok_or_else(|| bad(C, format!("unknown rate '{r}'")))?;
        }
        if let Some(pt) = str_param(p, "pattern") {
            arp.pattern = pattern_index(pt).ok_or_else(|| bad(C, format!("unknown pattern '{pt}': up, down, updown, played or random")))?;
        }
        arp.octaves = i64_or(p, "octaves", 1).clamp(1, 4) as u8;
        arp.gate = i64_or(p, "gate", 50).clamp(5, 100) as u8;
        vec![arp]
    };
    let range = Range::new(clip.start, clip.end());
    let keys = soundcraft_mix::midi_fx::sequence_events(s, range.start, range.end, sequence);
    let list: Vec<(soundcraft_time::Samples, [u8; 3], bool)> =
        soundcraft_mix::midi_fx::print(s, &fx, s.track(track).map(|t| t.midi_scripts.as_slice()).unwrap_or_default(), range, &keys)
            .into_iter()
            .map(|(at, m)| (at, m, true))
            .collect();
    let mut out = crate::band_print::events_to_sequence(s, range, &list);
    if out.notes.is_empty() {
        return Err(bad(C, "the clip has no notes to arpeggiate"));
    }
    // The controllers stay as they were (the effects pass them through unchanged).
    out.ctrls = sequence.ctrls.clone();
    out.sort();
    let n = out.notes.len();
    if let Some(c) = e.session_mut().find_clip_mut(id) {
        c.content = ClipContent::Midi { sequence: out };
        c.midi_fx_printed = printed;
    }
    Ok(json!({"notes": n, "printed": printed}))
}

/// A degree in roman numerals ("vi", "IV", "VII", "ii°") as (0-based degree, minor).
fn degree(r: &str) -> Option<i32> {
    let t = r.trim().trim_end_matches(['°', 'o', '+', '7']);
    let up = t.to_ascii_uppercase();
    ["I", "II", "III", "IV", "V", "VI", "VII"].iter().position(|n| *n == up).map(|d| d as i32)
}

/// The chords (root pitch class, semitones above it) of `prog` in the key `(tonic, scale)`: each
/// degree's triad in the key's scale, so "V" in a minor key is the scale's own.
pub fn progression(prog: &str, tonic: u8, scale: u8) -> Option<Vec<(i32, Vec<i32>)>> {
    let mut out = Vec::new();
    for r in prog.split(['-', ' ', ',']).filter(|x| !x.trim().is_empty()) {
        let d = degree(r)?;
        let at = |k: i32| soundcraft_band::chord_theory::scale_offset(i32::from(scale), k);
        let root = at(d);
        out.push(((i32::from(tonic) + root).rem_euclid(12), vec![0, at(d + 2) - root, at(d + 4) - root]));
        if out.len() > 64 {
            return None;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The chord's notes from C3 up, the inversion whose notes sit nearest `prev`'s (their mean), and
/// the root an octave below the lowest.
pub fn voice(root_pc: i32, tones: &[i32], prev: Option<&[u8]>) -> Vec<u8> {
    let base = 48 + root_pc;
    let close: Vec<i32> = tones.iter().map(|t| base + t).collect();
    let centre = |v: &[i32]| v.iter().sum::<i32>() as f64 / v.len().max(1) as f64;
    let target = prev.map(|p| p.iter().skip(1).map(|x| f64::from(*x)).sum::<f64>() / p.len().saturating_sub(1).max(1) as f64).unwrap_or(60.0);
    let mut best = close.clone();
    let mut best_d = f64::MAX;
    for inv in 0..tones.len() {
        for shift in [-12, 0] {
            let mut v: Vec<i32> = close.iter().enumerate().map(|(i, x)| if i < inv { x + 12 } else { *x }).map(|x| x + shift).collect();
            v.sort_unstable();
            let d = (centre(&v) - target).abs();
            if d < best_d {
                best_d = d;
                best = v;
            }
        }
    }
    let low = best.first().copied().unwrap_or(base);
    let mut bass = 36 + root_pc;
    while bass >= low {
        bass -= 12;
    }
    std::iter::once(bass).chain(best).filter_map(|x| u8::try_from(x.clamp(0, 127)).ok()).collect()
}

fn midi_track(e: &Engine, p: &Value, cmd: &str) -> Result<TrackId> {
    let s = e.session();
    if p.get("track").is_some() {
        let t = track_param(e, cmd, p, "track")?.ok_or_else(|| bad(cmd, "no track"))?;
        return match s.track(t) {
            Some(tr) if tr.kind.is_midi() => Ok(t),
            _ => Err(bad(cmd, "not a MIDI or instrument track")),
        };
    }
    s.edit
        .selected_tracks
        .iter()
        .copied()
        .find(|t| s.track(*t).is_some_and(|t| t.kind.is_midi()))
        .ok_or_else(|| bad(cmd, "select a MIDI or instrument track"))
}

fn write_progression(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "event.write_progression";
    let track = midi_track(e, p, C)?;
    let s = e.session();
    let sr = s.sample_rate;
    let sel = s.edit.selection;
    let start = p.get("start").and_then(Value::as_i64).unwrap_or(sel.start).max(0);
    let start_tick = s.tempo.samples_to_ticks(start, sr);
    let bar = s.tempo.meter_at_tick(start_tick).ticks_per_bar().max(1);
    let end = match p.get("end").and_then(Value::as_i64) {
        Some(x) => x,
        None if p.get("start").is_none() && !sel.is_empty() => sel.end,
        None => s.tempo.tick_to_samples(start_tick.saturating_add(bar.saturating_mul(4)), sr),
    };
    if end <= start {
        return Err(bad(C, "the range is empty"));
    }
    let end_tick = s.tempo.samples_to_ticks(end, sr);
    let rhythm = str_param(p, "rhythm").unwrap_or("whole").to_ascii_lowercase();
    let step = match rhythm.as_str() {
        "whole" | "strum" => bar,
        "half" => bar / 2,
        "quarter" => TICKS_PER_QUARTER,
        other => return Err(bad(C, format!("unknown rhythm '{other}': whole, half, quarter or strum"))),
    }
    .max(1);
    let velocity = i64_or(p, "velocity", 90).clamp(1, 127) as u8;
    let prog = str_param(p, "progression").unwrap_or("lane");
    let (tonic, scale) = crate::vocal::key_pc_at(s, start);
    // The chords: (from tick, root pitch class, tones), each held until the next.
    let lane: Vec<(i64, i32, Vec<i32>)> = if prog.eq_ignore_ascii_case("lane") {
        let mut cs: Vec<&soundcraft_model::ChordEvent> =
            s.band.chords.iter().filter(|c| c.tick < end_tick && c.tick.saturating_add(c.length_ticks.max(1)) > start_tick).collect();
        cs.sort_by_key(|c| c.tick);
        cs.iter()
            .map(|c| {
                let sh =
                    soundcraft_band::chord_theory::shape_for(i32::from(c.slot), i32::from(s.band.key), i32::from(s.band.scale), i32::from(c.bass));
                let n = usize::try_from(sh.count.clamp(1, 4)).unwrap_or(3);
                (c.tick.max(start_tick), sh.root_pc.rem_euclid(12), sh.tones.iter().take(n).copied().collect())
            })
            .collect()
    } else {
        Vec::new()
    };
    let chords: Vec<(i64, i32, Vec<i32>)> = if lane.is_empty() {
        let name = if prog.eq_ignore_ascii_case("lane") { "I-V-vi-IV" } else { prog };
        let list = progression(name, tonic, scale).ok_or_else(|| bad(C, format!("cannot read the progression '{name}'")))?;
        (0..)
            .map(|k: i64| start_tick.saturating_add(k.saturating_mul(bar)))
            .take_while(|t| *t < end_tick)
            .zip(list.iter().cycle())
            .map(|(t, (r, tones))| (t, *r, tones.clone()))
            .collect()
    } else {
        lane
    };
    let mut seq = Sequence::default();
    let mut prev: Option<Vec<u8>> = None;
    for (k, (from, root, tones)) in chords.iter().enumerate() {
        let to = chords.get(k + 1).map_or(end_tick, |c| c.0).min(end_tick);
        let pitches = voice(*root, tones, prev.as_deref());
        let mut t = *from;
        while t < to {
            let len = step.min(to - t).max(1);
            for (i, pitch) in pitches.iter().enumerate() {
                let delay = if rhythm == "strum" { (i as i64) * 30 } else { 0 };
                if delay >= len {
                    continue;
                }
                seq.notes.push(Note {
                    pitch: *pitch,
                    velocity,
                    release_velocity: 64,
                    channel: 0,
                    start: t - start_tick + delay,
                    length: (len - delay - 10).max(1),
                    muted: false,
                    probability: 100,
                });
            }
            t = t.saturating_add(step);
        }
        prev = Some(pitches);
    }
    seq.sort();
    let n_chords = chords.len();
    let doc = e.session_mut();
    let id = doc.new_clip_id();
    crate::edit::place_clip(doc, track, soundcraft_model::Clip::midi(id, "Progression".to_string(), start, end - start, seq));
    Ok(json!({"clip": id, "chords": n_chords}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;

    #[test]
    fn progressions_read_in_the_key() {
        let p = progression("I-V-vi-IV", 0, 0).unwrap();
        assert_eq!(p.iter().map(|c| c.0).collect::<Vec<_>>(), vec![0, 7, 9, 5]);
        assert_eq!(p[2].1, vec![0, 3, 7], "vi is minor in a major key");
        let m = progression("i-VI-III-VII", 9, 1).unwrap();
        assert_eq!(m.iter().map(|c| c.0).collect::<Vec<_>>(), vec![9, 5, 0, 7]);
        assert!(progression("I-X", 0, 0).is_none());
        // Voiced with the root below and moving little from chord to chord.
        let c = voice(0, &[0, 4, 7], None);
        assert_eq!(c[0] % 12, 0);
        let g = voice(7, &[0, 4, 7], Some(&c));
        let mean = |v: &[u8]| v.iter().skip(1).map(|x| f64::from(*x)).sum::<f64>() / 3.0;
        assert!((mean(&g) - mean(&c)).abs() <= 4.0, "{c:?} {g:?}");
        assert!(g[0] < g[1]);
    }

    #[test]
    fn a_progression_writes_a_chord_a_bar_on_the_selected_track() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "Keys", "kind": "midi"})).unwrap();
        e.execute("edit.select", &json!({"tracks": ["Keys"]})).unwrap();
        let r = e.execute("event.write_progression", &json!({"progression": "I-V-vi-IV", "rhythm": "half"})).unwrap();
        assert_eq!(r["chords"], 4);
        let notes = e.execute("midi.notes", &json!({"clip": r["clip"]})).unwrap();
        let notes = notes["notes"].as_array().unwrap();
        // Four bars, two chords of four notes a bar.
        assert_eq!(notes.len(), 4 * 2 * 4);
        let bass: Vec<i64> = notes
            .iter()
            .filter(|n| n["start"].as_i64().unwrap() % 3840 == 0)
            .map(|n| n["pitch"].as_i64().unwrap())
            .min_by_key(|p| *p)
            .into_iter()
            .collect();
        assert_eq!(bass[0] % 12, 0, "C in the bass of the first bar");
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().track_by_name("Keys").unwrap().clips().is_empty(), "one undo step");
    }

    #[test]
    fn write_arpeggio_turns_a_held_chord_into_steps() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "Arp", "kind": "midi"})).unwrap();
        let clip = e.execute("midi.new_clip", &json!({"track": "Arp", "start": 0, "end": 96_000})).unwrap()["clip"].clone();
        for p in [60, 64, 67] {
            e.execute("midi.note_add", &json!({"clip": clip, "pitch": p, "start_ticks": 0, "length_ticks": 3840})).unwrap();
        }
        let r = e.execute("event.write_arpeggio", &json!({"clip": clip, "rate": "1/16", "pattern": "up"})).unwrap();
        assert_eq!(r["printed"], false);
        let notes = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        let notes = notes["notes"].as_array().unwrap();
        // A bar of 16ths, C E G over and over.
        assert!((15..=17).contains(&notes.len()), "{}", notes.len());
        let pitches: Vec<i64> = notes.iter().take(4).map(|n| n["pitch"].as_i64().unwrap()).collect();
        assert_eq!(pitches, vec![60, 64, 67, 60]);
        assert_eq!(notes[1]["start"].as_i64().unwrap(), 240);
        assert!(e.execute("event.write_arpeggio", &json!({"clip": clip, "rate": "1/7"})).is_err());
    }
}

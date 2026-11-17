//! MIDI clip settings and edits on the timeline: a clip's own transpose and velocity offset
//! (applied as it plays, the notes left as written), reversing a clip, and Note Chase.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::{CtrlEvent, CtrlShape, Sequence};
use soundcraft_model::{ClipContent, ClipId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "clip.midi_offsets",
            "MIDI Clip Transpose and Velocity",
            [],
            None,
            "{clips?, transpose?: -48..48 semitones, velocity?: -127..127} — applied as the clip plays (with its track's Real-Time Properties); the notes stay as written",
            has_selection,
            offsets
        ),
        cmd!(
            "clip.midi_reverse",
            "Reverse MIDI Clip",
            [],
            None,
            "{clips?} — notes and controllers play backwards in the clip's window",
            has_selection,
            reverse
        ),
        cmd!(
            noundo "midi.note_chase",
            "Note Chase",
            [],
            None,
            "{on?: bool (default: toggle)} — a note already sounding where playback starts or a locate lands is started there",
            always,
            note_chase
        ),
        cmd!(
            noundo "midi.split_notes",
            "Split Notes at the Cut",
            [],
            None,
            "{on?: bool (default: toggle)} — splitting a MIDI clip cuts a note sounding across the cut in two, its rest starting the right clip (off: the note stays whole in the left clip, ending at the cut)",
            always,
            split_notes
        ),
        cmd!(
            "track.drum_map",
            "Drum Map",
            [],
            None,
            "{track, pitch?: 0..127, name?: text (empty: the pitch loses its name), gm?: true (General MIDI names for every pitch without one), clear?: true} — the note editor's names per pitch for the track; they override the instrument's and turn on the drum view for any instrument",
            always,
            drum_map
        ),
    ]
}

fn midi_clips(e: &Engine, p: &Value) -> Vec<ClipId> {
    let s = e.session();
    clip_ids_param(e, p).into_iter().filter(|id| s.find_clip(*id).is_some_and(|(_, c)| matches!(c.content, ClipContent::Midi { .. }))).collect()
}

fn offsets(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.midi_offsets";
    let ids = midi_clips(e, p);
    if ids.is_empty() {
        return Err(bad(C, "select a MIDI clip"));
    }
    let tr = p.get("transpose").and_then(Value::as_i64).map(|v| v.clamp(-48, 48) as i8);
    let ve = p.get("velocity").and_then(Value::as_i64).map(|v| v.clamp(-127, 127) as i8);
    if tr.is_none() && ve.is_none() {
        return Err(bad(C, "give transpose or velocity"));
    }
    let s = e.session_mut();
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            if let Some(v) = tr {
                c.midi_transpose = v;
            }
            if let Some(v) = ve {
                c.midi_velocity = v;
            }
        }
    }
    Ok(json!({"clips": ids.len()}))
}

/// `seq` played backwards in `[0, len)`: each note ends where it started from the end; each
/// controller lane's curve is mirrored. A held value holds over the mirror of the span it held (the
/// state before the window's first event, from an event before it, closes the reversed lane); a
/// ramp runs the other way, from where it ended to where it began, its curve bent the other way,
/// so the reversed lane is the original's mirror image. Events outside the window stay.
pub fn reversed(seq: &Sequence, len: i64) -> Sequence {
    let mut out = seq.clone();
    for n in out.notes.iter_mut().filter(|n| n.start >= 0 && n.start < len) {
        let end = n.end().min(len);
        n.start = len.saturating_sub(end).max(0);
    }
    let inside = |c: &CtrlEvent| c.tick >= 0 && c.tick < len;
    let mut lanes: Vec<(soundcraft_midi::CtrlKind, u8)> = Vec::new();
    for c in seq.ctrls.iter().filter(|c| inside(c)) {
        if !lanes.contains(&(c.kind, c.channel)) {
            lanes.push((c.kind, c.channel));
        }
    }
    // The reversed lanes first, so a ramp's end point on the window's edge comes before an event
    // already there (the sort below keeps equal ticks in order).
    let mut ctrls: Vec<CtrlEvent> = Vec::new();
    for (kind, ch) in lanes {
        let mut lane: Vec<&CtrlEvent> = seq.ctrls.iter().filter(|c| c.kind == kind && c.channel == ch).collect();
        lane.sort_by_key(|c| c.tick);
        let mut cuts: Vec<i64> = vec![0];
        cuts.extend(lane.iter().filter(|c| inside(c)).map(|c| c.tick));
        cuts.push(len);
        cuts.dedup();
        let segments: Vec<Segment> = cuts.windows(2).filter_map(|w| segment(&lane, *w.first()?, *w.get(1)?)).collect();
        for (k, s) in segments.iter().enumerate().rev() {
            let shape = match s.shape {
                CtrlShape::Ramp(c) => CtrlShape::Ramp(-c),
                CtrlShape::Step => CtrlShape::Step,
            };
            ctrls.push(CtrlEvent { tick: len.saturating_sub(s.to), value: round(s.b), shape, ..*s.event });
            // A ramp ends on a point with the value it started from, unless the next reversed
            // span starts there on that value already.
            if !s.shape.is_step() {
                let joined = k.checked_sub(1).and_then(|j| segments.get(j)).is_some_and(|n| n.to == s.from && round(n.b) == round(s.a));
                if !joined {
                    ctrls.push(CtrlEvent { tick: len.saturating_sub(s.from), value: round(s.a), shape: CtrlShape::Step, ..*s.event });
                }
            }
        }
    }
    ctrls.extend(seq.ctrls.iter().filter(|c| !inside(c)).copied());
    out.ctrls = ctrls;
    out.sort();
    out
}

/// A span `from..to` of a controller lane: its value `a` at the start going to `b` at the end,
/// held (`Step`) or by a ramp with the curve of that part of the ramp.
struct Segment<'a> {
    from: i64,
    to: i64,
    a: f64,
    b: f64,
    shape: CtrlShape,
    event: &'a CtrlEvent,
}

fn round(v: f64) -> i32 {
    v.round() as i32
}

/// The span `from..to` of `lane` (sorted by tick; no event strictly inside the span): the event
/// that sets it and what it does there. None before the lane's first event.
fn segment<'a>(lane: &[&'a CtrlEvent], from: i64, to: i64) -> Option<Segment<'a>> {
    if to <= from {
        return None;
    }
    let i = lane.iter().rposition(|c| c.tick <= from)?;
    let g = *lane.get(i)?;
    let held = Segment { from, to, a: f64::from(g.value), b: f64::from(g.value), shape: CtrlShape::Step, event: g };
    let (CtrlShape::Ramp(c), Some(h)) = (g.shape, lane.get(i + 1)) else { return Some(held) };
    if h.tick <= g.tick {
        return Some(held);
    }
    let span = (h.tick - g.tick) as f64;
    let (x0, x1) = ((from - g.tick) as f64 / span, ((to - g.tick) as f64 / span).min(1.0));
    let v = |x: f64| f64::from(g.value) + f64::from(h.value - g.value) * g.shape.at(x);
    // Part of an exponential ramp is the same ramp with its bend scaled by the part's share.
    Some(Segment { a: v(x0), b: v(x1), shape: CtrlShape::Ramp(c * (x1 - x0) as f32), ..held })
}

fn reverse(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.midi_reverse";
    let ids = midi_clips(e, p);
    if ids.is_empty() {
        return Err(bad(C, "select a MIDI clip"));
    }
    let s = e.session_mut();
    for id in &ids {
        let Some(c) = s.find_clip(*id).map(|(_, c)| c.clone()) else { continue };
        let ClipContent::Midi { sequence } = &c.content else { continue };
        let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
        let len = s.tempo.samples_to_ticks(c.end(), s.sample_rate).saturating_sub(base).max(1);
        let seq = reversed(sequence, len);
        if let Some(c) = s.find_clip_mut(*id) {
            c.content = ClipContent::Midi { sequence: seq };
        }
    }
    Ok(json!({"clips": ids.len()}))
}

fn note_chase(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    let on = p.get("on").and_then(Value::as_bool).unwrap_or(!s.edit.flag("midi.note_chase"));
    s.edit.set_flag("midi.note_chase", on);
    Ok(json!({"on": on}))
}

fn split_notes(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    let on = p.get("on").and_then(Value::as_bool).unwrap_or(!s.edit.flag(crate::edit::SPLIT_NOTES));
    s.edit.set_flag(crate::edit::SPLIT_NOTES, on);
    Ok(json!({"on": on}))
}

fn drum_map(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.drum_map";
    let id = track_param(e, C, p, "track")?.ok_or_else(|| bad(C, "`track` required"))?;
    let pitch = match p.get("pitch") {
        None => None,
        Some(v) => Some(v.as_u64().filter(|n| *n <= 127).ok_or_else(|| bad(C, "`pitch` must be 0..127"))? as u8),
    };
    let name = p.get("name").and_then(Value::as_str).map(|n| {
        let mut n: String = n.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string();
        soundcraft_model::truncate_chars(&mut n, soundcraft_model::MAX_NAME_CHARS);
        n
    });
    let gm = p.get("gm").and_then(Value::as_bool).unwrap_or(false);
    let clear = p.get("clear").and_then(Value::as_bool).unwrap_or(false);
    if name.is_some() && pitch.is_none() {
        return Err(bad(C, "`name` needs a `pitch`"));
    }
    if name.is_none() && !gm && !clear {
        return Err(bad(C, "give `pitch` and `name`, `gm` or `clear`"));
    }
    let Some(t) = e.session_mut().track_mut(id) else { return Err(bad(C, "no such track")) };
    if clear {
        t.drum_map.clear();
    }
    if gm {
        for p in 0..=127u8 {
            if let Some(n) = soundcraft_midi::gm_drum_name(p)
                && !t.drum_map.iter().any(|(q, _)| *q == p)
            {
                t.drum_map.push((p, n.to_string()));
            }
        }
    }
    if let (Some(p), Some(n)) = (pitch, name) {
        t.drum_map.retain(|(q, _)| *q != p);
        if !n.is_empty() {
            t.drum_map.push((p, n));
        }
    }
    t.drum_map.sort_by_key(|(p, _)| *p);
    Ok(json!({"track": id.0, "names": t.drum_map.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_midi::{CtrlKind, Note};
    use soundcraft_model::{ChannelFormat, TrackKind};

    #[test]
    fn a_clip_reverses_notes_and_controller_spans() {
        let n = |pitch, start, length| Note { probability: 100, pitch, velocity: 100, muted: false, release_velocity: 64, channel: 0, start, length };
        let c = |tick, value| CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(1), value, shape: Default::default() };
        let seq = Sequence {
            notes: vec![n(60, 0, 100), n(62, 300, 200), n(64, -50, 10)],
            ctrls: vec![c(-10, 5), c(200, 10), c(600, 20)],
            sysex: Vec::new(),
        };
        let r = reversed(&seq, 1000);
        let notes: Vec<(u8, i64, i64)> = r.notes.iter().map(|n| (n.pitch, n.start, n.length)).collect();
        assert_eq!(notes, vec![(64, -50, 10), (62, 500, 200), (60, 900, 100)]);
        let ctrls: Vec<(i64, i32)> = r.ctrls.iter().map(|c| (c.tick, c.value)).collect();
        // 10 held 200..600 -> 400..800; 20 held 600..1000 -> 0..400; 5 (from before) held 0..200 -> 800..
        assert_eq!(ctrls, vec![(-10, 5), (0, 20), (400, 10), (800, 5)]);
        let mut sorted = seq.clone();
        sorted.sort();
        assert_eq!(reversed(&r, 1000).notes, sorted.notes, "reversed twice, as it was");
    }

    /// The value a lane plays at `t`: the last event at or before it, ramping to the next.
    fn lane_value(seq: &Sequence, t: i64) -> Option<f64> {
        let i = seq.ctrls.iter().rposition(|c| c.tick <= t)?;
        let a = seq.ctrls.get(i)?;
        match seq.next_in_lane(i) {
            Some(b) if !a.shape.is_step() && b.tick > a.tick => {
                let x = (t - a.tick) as f64 / (b.tick - a.tick) as f64;
                Some(f64::from(a.value) + f64::from(b.value - a.value) * a.shape.at(x))
            }
            _ => Some(f64::from(a.value)),
        }
    }

    #[test]
    fn a_reversed_ramp_is_the_mirror_image() {
        let c = |tick, value, shape| CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(1), value, shape };
        // A curved ramp up, held, a straight ramp down, held; one ramp crosses the clip's end.
        let seq = Sequence {
            notes: vec![],
            ctrls: vec![
                c(100, 0, CtrlShape::Ramp(0.6)),
                c(400, 100, CtrlShape::Step),
                c(600, 90, CtrlShape::Ramp(0.0)),
                c(700, 20, CtrlShape::Step),
                c(900, 30, CtrlShape::Ramp(-0.4)),
                c(1200, 127, CtrlShape::Step),
            ],
            sysex: Vec::new(),
        };
        let len = 1000;
        let r = reversed(&seq, len);
        // (Where the original jumps, the mirror takes the other side of the jump: not compared.)
        for t in (1..len - 100).step_by(7).filter(|t| seq.ctrls.iter().all(|c| c.tick != len - t)) {
            let (want, got) = (lane_value(&seq, len - t), lane_value(&r, t));
            let (want, got) = (want.unwrap_or(f64::NAN), got.unwrap_or(f64::NAN));
            assert!((want - got).abs() <= 1.0, "at {t}: reversed {got}, mirror of the original {want}");
        }
        // Twice reversed plays as the original.
        let rr = reversed(&r, len);
        for t in (101..len).step_by(7) {
            let (want, got) = (lane_value(&seq, t).unwrap_or(f64::NAN), lane_value(&rr, t).unwrap_or(f64::NAN));
            assert!((want - got).abs() <= 1.0, "at {t}: twice reversed {got}, original {want}");
        }
    }

    /// Parity section 7: a track's drum map names pitches, fills in General MIDI names, is
    /// cleared, is undone and saved with the track.
    #[test]
    fn a_drum_map_names_pitches_on_the_track() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Kit"));
        e.execute("track.drum_map", &json!({"track": t.0, "pitch": 36, "name": "Big Kick"})).unwrap();
        e.execute("track.drum_map", &json!({"track": t.0, "pitch": 38, "name": "Snare\n"})).unwrap();
        assert_eq!(e.session().track(t).unwrap().drum_map, vec![(36, "Big Kick".to_string()), (38, "Snare".to_string())]);
        // GM names fill only the pitches without one.
        e.execute("track.drum_map", &json!({"track": t.0, "gm": true})).unwrap();
        let map = &e.session().track(t).unwrap().drum_map;
        assert_eq!(map.iter().find(|(p, _)| *p == 36).map(|x| x.1.as_str()), Some("Big Kick"));
        assert_eq!(map.iter().find(|(p, _)| *p == 42).map(|x| x.1.as_str()), soundcraft_midi::gm_drum_name(42));
        // An empty name removes one; a bad pitch is refused.
        e.execute("track.drum_map", &json!({"track": t.0, "pitch": 36, "name": ""})).unwrap();
        assert!(!e.session().track(t).unwrap().drum_map.iter().any(|(p, _)| *p == 36));
        assert!(e.execute("track.drum_map", &json!({"track": t.0, "pitch": 200, "name": "x"})).is_err());
        assert!(e.execute("track.drum_map", &json!({"track": t.0})).is_err());
        // Saved with the track; undone step by step.
        let saved = serde_json::to_string(e.session().track(t).unwrap()).unwrap();
        let back: soundcraft_model::Track = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.drum_map, e.session().track(t).unwrap().drum_map);
        e.execute("track.drum_map", &json!({"track": t.0, "clear": true})).unwrap();
        assert!(e.session().track(t).unwrap().drum_map.is_empty());
        assert!(e.undo());
        assert!(!e.session().track(t).unwrap().drum_map.is_empty());
    }

    #[test]
    fn offsets_reverse_and_chase_are_commands() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let c = e.execute("midi.new_clip", &json!({"track": t.0, "start": 0, "end": 96_000})).unwrap()["clip"].as_u64().unwrap();
        e.execute("clip.midi_offsets", &json!({"clips": [c], "transpose": 7, "velocity": -200})).unwrap();
        let clip = e.session().find_clip(ClipId(c)).unwrap().1.clone();
        assert_eq!((clip.midi_transpose, clip.midi_velocity), (7, -127));
        assert!(e.execute("clip.midi_offsets", &json!({"clips": [c]})).is_err());
        e.execute("midi.note_add", &json!({"clip": c, "pitch": 60, "start_ticks": 0, "length_ticks": 480})).unwrap();
        e.execute("clip.midi_reverse", &json!({"clips": [c]})).unwrap();
        let ClipContent::Midi { sequence } = &e.session().find_clip(ClipId(c)).unwrap().1.content else { panic!() };
        assert_eq!(sequence.notes[0].start, 3840 - 480);
        assert_eq!(e.execute("midi.note_chase", &json!({})).unwrap()["on"], true);
        assert!(e.session().edit.flag("midi.note_chase"));
    }
}

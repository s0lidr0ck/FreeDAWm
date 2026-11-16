//! MIDI clip settings and edits on the timeline: a clip's own transpose and velocity offset
//! (applied as it plays, the notes left as written), reversing a clip, and Note Chase.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::{CtrlEvent, Sequence};
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
/// controller's value holds over the mirror of the span it held (the state before its first
/// event in the window, from an event before it, closes the reversed lane). Events outside the
/// window stay.
pub fn reversed(seq: &Sequence, len: i64) -> Sequence {
    let mut out = seq.clone();
    for n in out.notes.iter_mut().filter(|n| n.start >= 0 && n.start < len) {
        let end = n.end().min(len);
        n.start = len.saturating_sub(end).max(0);
    }
    let inside = |c: &CtrlEvent| c.tick >= 0 && c.tick < len;
    let mut ctrls: Vec<CtrlEvent> = seq.ctrls.iter().filter(|c| !inside(c)).copied().collect();
    let mut lanes: Vec<(soundcraft_midi::CtrlKind, u8)> = Vec::new();
    for c in seq.ctrls.iter().filter(|c| inside(c)) {
        if !lanes.contains(&(c.kind, c.channel)) {
            lanes.push((c.kind, c.channel));
        }
    }
    for (kind, ch) in lanes {
        let lane: Vec<&CtrlEvent> = seq.ctrls.iter().filter(|c| c.kind == kind && c.channel == ch).collect();
        let within: Vec<&&CtrlEvent> = lane.iter().filter(|c| inside(c)).collect();
        for (k, c) in within.iter().enumerate() {
            let next = within.get(k + 1).map_or(len, |n| n.tick);
            ctrls.push(CtrlEvent { tick: len.saturating_sub(next).max(0), ..***c });
        }
        // The value before the window's first event, from the last one before the window.
        if let (Some(first), Some(prior)) = (within.first(), lane.iter().filter(|c| c.tick < 0).max_by_key(|c| c.tick))
            && first.tick > 0
        {
            ctrls.push(CtrlEvent { tick: len.saturating_sub(first.tick), ..**prior });
        }
    }
    out.ctrls = ctrls;
    out.sort();
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_midi::{CtrlKind, Note};
    use soundcraft_model::{ChannelFormat, TrackKind};

    #[test]
    fn a_clip_reverses_notes_and_controller_spans() {
        let n = |pitch, start, length| Note { pitch, velocity: 100, muted: false, release_velocity: 64, channel: 0, start, length };
        let c = |tick, value| CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(1), value, shape: Default::default() };
        let seq = Sequence { notes: vec![n(60, 0, 100), n(62, 300, 200), n(64, -50, 10)], ctrls: vec![c(-10, 5), c(200, 10), c(600, 20)] };
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

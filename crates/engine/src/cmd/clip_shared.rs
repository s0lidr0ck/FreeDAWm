//! Shared (alias) MIDI clips ([`crate::midi_shared`]): Duplicate as Shared Copy places a copy
//! after each selected MIDI clip that changes with it; Make Independent ends the sharing.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{ClipContent, ClipId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "clip.share",
            "Duplicate as Shared Copy",
            [],
            None,
            "{clips?} -> {clips: [new id]} — a copy right after each selected MIDI clip that shares its notes: a note edit in one is made in all",
            has_selection,
            share
        ),
        cmd!(
            "clip.make_independent",
            "Make Independent",
            [],
            None,
            "{clips?} -> {clips} — the selected shared MIDI clips keep their notes but no longer change with the others",
            has_selection,
            make_independent
        ),
    ]
}

fn midi_clips(e: &Engine, p: &Value) -> Vec<ClipId> {
    let s = e.session();
    clip_ids_param(e, p).into_iter().filter(|id| s.find_clip(*id).is_some_and(|(_, c)| matches!(c.content, ClipContent::Midi { .. }))).collect()
}

fn share(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.share";
    let ids = midi_clips(e, p);
    if ids.is_empty() {
        return Err(bad(C, "select a MIDI clip"));
    }
    let s = e.session_mut();
    let mut made = Vec::new();
    for id in ids {
        let Some((track, c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { continue };
        if c.pattern.is_some() {
            continue;
        }
        let group = c.shared.unwrap_or_else(|| s.alloc());
        if let Some(orig) = s.find_clip_mut(id) {
            orig.shared = Some(group);
        }
        let mut copy = c;
        copy.id = s.new_clip_id();
        copy.start = copy.end();
        copy.shared = Some(group);
        copy.loop_id = None;
        copy.group = None;
        made.push(copy.id.0);
        crate::edit::place_clip(s, track, copy);
    }
    if made.is_empty() {
        return Err(bad(C, "a Juke pattern clip cannot be shared"));
    }
    Ok(json!({"clips": made}))
}

fn make_independent(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = midi_clips(e, p);
    let s = e.session_mut();
    let mut n = 0;
    for id in ids {
        if let Some(c) = s.find_clip_mut(id).filter(|c| c.shared.is_some()) {
            c.shared = None;
            n += 1;
        }
    }
    if n == 0 {
        return Err(bad("clip.make_independent", "select a shared MIDI clip"));
    }
    Ok(json!({"clips": n}))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, ClipId, TrackKind};

    fn pitches(e: &mut crate::Engine, clip: u64) -> Vec<(i64, u64)> {
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        n["notes"].as_array().unwrap().iter().map(|n| (n["start"].as_i64().unwrap(), n["pitch"].as_u64().unwrap())).collect()
    }

    #[test]
    fn shared_copies_change_together_until_made_independent() {
        let mut e = crate::Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let a = e.execute("midi.new_clip", &json!({"track": t.0, "start": 0, "end": 96_000})).unwrap()["clip"].as_u64().unwrap();
        e.execute("midi.note_add", &json!({"clip": a, "pitch": 60, "start_ticks": 0})).unwrap();
        let b = e.execute("clip.share", &json!({"clips": [a]})).unwrap()["clips"][0].as_u64().unwrap();
        let start = |e: &crate::Engine, id: u64| e.session().find_clip(ClipId(id)).map(|(_, c)| (c.start, c.shared.is_some()));
        assert_eq!(start(&e, b), Some((96_000, true)));
        assert_eq!(pitches(&mut e, b), vec![(0, 60)]);
        // A note added to the copy is in the original, in one undo step.
        let steps = e.undo_history().len();
        e.execute("midi.note_add", &json!({"clip": b, "pitch": 64, "start_ticks": 960})).unwrap();
        assert_eq!(pitches(&mut e, a), vec![(0, 60), (960, 64)]);
        assert_eq!(e.undo_history().len(), steps + 1);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!((pitches(&mut e, a), pitches(&mut e, b)), (vec![(0, 60)], vec![(0, 60)]));
        // A Duplicate of a shared clip shares too; quantize in one reaches all three.
        e.execute("edit.select", &json!({"clips": [b]})).unwrap();
        e.execute("edit.duplicate", &json!({})).unwrap();
        let c = e.session().tracks[0].clips().iter().map(|c| c.id.0).find(|id| *id != a && *id != b).unwrap();
        assert_eq!(start(&e, c).map(|x| x.1), Some(true));
        e.execute("midi.notes_move", &json!({"clip": a, "indices": [0], "ticks": 30})).unwrap();
        e.execute("event.quantize", &json!({"clips": [c], "grid": 480})).unwrap();
        for id in [a, b, c] {
            assert_eq!(pitches(&mut e, id), vec![(0, 60)], "clip {id}");
        }
        // Moving a copy keeps it shared; Make Independent ends it for that one.
        e.execute("clip.make_independent", &json!({"clips": [c]})).unwrap();
        e.execute("midi.note_add", &json!({"clip": c, "pitch": 67, "start_ticks": 0})).unwrap();
        assert_eq!(pitches(&mut e, a), vec![(0, 60)]);
        // The last two: one made independent leaves the other on its own too.
        e.execute("clip.make_independent", &json!({"clips": [a]})).unwrap();
        assert_eq!(start(&e, b).map(|x| x.1), Some(false));
        // Sessions saved before load unshared; an unshared clip saves without the field.
        let saved = serde_json::to_string(&e.session().tracks[0].clips()[0]).unwrap();
        assert!(!saved.contains("shared"));
    }

    #[test]
    fn a_start_trim_leaves_a_shared_clip_on_its_own() {
        let mut e = crate::Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let a = e.execute("midi.new_clip", &json!({"track": t.0, "start": 0, "end": 96_000})).unwrap()["clip"].as_u64().unwrap();
        e.execute("midi.note_add", &json!({"clip": a, "pitch": 60, "start_ticks": 0})).unwrap();
        e.execute("midi.note_add", &json!({"clip": a, "pitch": 62, "start_ticks": 1920})).unwrap();
        let b = e.execute("clip.share", &json!({"clips": [a]})).unwrap()["clips"][0].as_u64().unwrap();
        e.execute("edit.trim_clip", &json!({"clip": b, "start": 96_000 + 24_000})).unwrap();
        let shared = |e: &crate::Engine, id: u64| e.session().find_clip(ClipId(id)).is_some_and(|(_, c)| c.shared.is_some());
        assert!(!shared(&e, b) && !shared(&e, a), "trimmed: on its own (and the other, alone, too)");
        assert_eq!(pitches(&mut e, a), vec![(0, 60), (1920, 62)], "the original keeps its notes");
    }
}

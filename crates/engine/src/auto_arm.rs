//! Auto-Arm Selected MIDI Track (Setup › MIDI Input Devices; `options.midi_auto_arm`): with it
//! on, selecting one instrument or MIDI track that listens to MIDI input arms it, so it is heard
//! (input monitoring Auto with MIDI Thru) and ready to record, and the track it armed before is
//! disarmed. Tracks armed by hand are left alone; nothing changes while recording.

use soundcraft_model::{Route, Session, TrackId};

/// The session flag that turns it on.
pub const FLAG: &str = "midi.auto_arm";
/// The edit value holding the track it armed (its id).
const ARMED: &str = "midi.auto_armed";

/// After a command changed the selection: arms the one selected MIDI track and disarms the one
/// armed before. A selection of several tracks, or of none, leaves things as they are; one
/// track that is not a MIDI track disarms the last.
pub(crate) fn follow_selection(s: &mut Session) {
    let one = match s.edit.selected_tracks.as_slice() {
        [t] => *t,
        _ => return,
    };
    let pick = s
        .track(one)
        .filter(|t| t.kind.is_midi() && !t.inactive && t.frozen_audio().is_none() && !t.edit_locked && matches!(t.mixer.input, Route::Midi(_)))
        .map(|t| t.id);
    let prev = s.edit.values.get(ARMED).copied().filter(|v| v.is_finite() && *v >= 0.0).map(|v| TrackId(v as u64));
    if pick.is_some() && pick == prev {
        return;
    }
    if let Some(p) = prev.and_then(|p| s.track_mut(p)) {
        p.mixer.record_arm = false;
    }
    s.edit.values.remove(ARMED);
    // One armed by hand stays the user's: it is not disarmed when the selection moves on.
    let Some(t) = pick.and_then(|p| s.track_mut(p)).filter(|t| !t.mixer.record_arm) else { return };
    t.mixer.record_arm = true;
    let id = t.id.0 as f64;
    s.edit.values.insert(ARMED.into(), id);
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::TrackId;

    #[test]
    fn selecting_a_midi_track_arms_it_and_disarms_the_last() {
        let mut e = Engine::default();
        let new = |e: &mut Engine, kind: &str| TrackId(e.execute("track.new", &json!({"kind": kind})).unwrap()["tracks"][0].as_u64().unwrap());
        let (a, b, audio) = (new(&mut e, "instrument"), new(&mut e, "instrument"), new(&mut e, "audio"));
        let armed = |e: &Engine, t: TrackId| e.session().track(t).unwrap().mixer.record_arm;
        let select = |e: &mut Engine, t: TrackId| e.execute("edit.select", &json!({"tracks": [t.0], "start": 0, "end": 0})).unwrap();
        // Off: selecting arms nothing.
        select(&mut e, a);
        assert!(!armed(&e, a));
        e.execute("options.midi_auto_arm", &json!({"value": true})).unwrap();
        select(&mut e, b);
        assert!(armed(&e, b) && !armed(&e, a));
        select(&mut e, a);
        assert!(armed(&e, a) && !armed(&e, b), "the last one it armed is disarmed");
        // One armed by hand stays armed when the selection moves on.
        e.execute("mix.record_arm", &json!({"tracks": [b.0], "value": true})).unwrap();
        select(&mut e, b);
        select(&mut e, audio);
        assert!(armed(&e, b) && !armed(&e, a) && !armed(&e, audio), "an audio track is not armed; the hand-armed one stays");
        // Several selected: nothing changes.
        select(&mut e, a);
        e.execute("edit.select", &json!({"tracks": [a.0, b.0], "start": 0, "end": 0})).unwrap();
        assert!(armed(&e, a));
    }
}

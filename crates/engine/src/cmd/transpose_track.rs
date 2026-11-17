//! The global transpose track (`Session::transpose_track`): changes of a number of semitones that
//! every MIDI and instrument track follows at playback, with the clip's and the track's own
//! transpose, except drum instruments and tracks set to Independent. Shown and set on the Key
//! ruler.

use super::*;
use crate::cmd;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "song.transpose",
            "Global Transpose",
            [],
            None,
            "{at, semitones: -24..24} — from `at` on, MIDI and instrument tracks play transposed by this much (replacing a change there); 0 goes back to as written",
            always,
            set
        ),
        cmd!("song.transpose_remove", "Remove Global Transpose Change", [], None, "{at}", always, remove),
        cmd!(
            "track.global_transpose",
            "Track Follows Global Transpose",
            [],
            None,
            "{tracks?, follow: bool} — a track set not to follow (Independent) plays as written; drum instruments never follow",
            has_tracks,
            follow
        ),
    ]
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "song.transpose";
    let at = position_param(e, C, p, "at")?.ok_or_else(|| bad(C, "`at` required"))?.max(0);
    let st = p.get("semitones").and_then(Value::as_i64).ok_or_else(|| bad(C, "`semitones` required"))?;
    if !(-24..=24).contains(&st) {
        return Err(bad(C, "`semitones` must be -24..24"));
    }
    let s = e.session_mut();
    if s.transpose_track.len() >= 10_000 && !s.transpose_track.iter().any(|(t, _)| *t == at) {
        return Err(bad(C, "the transpose track is full"));
    }
    s.transpose_track.retain(|(t, _)| *t != at);
    s.transpose_track.push((at, st as i8));
    s.transpose_track.sort_by_key(|c| c.0);
    Ok(json!({"at": at, "semitones": st}))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "song.transpose_remove";
    let at = position_param(e, C, p, "at")?.ok_or_else(|| bad(C, "`at` required"))?;
    let s = e.session_mut();
    let before = s.transpose_track.len();
    s.transpose_track.retain(|(t, _)| *t != at);
    if s.transpose_track.len() == before {
        return Err(bad(C, "no transpose change there"));
    }
    Ok(json!({"at": at}))
}

fn follow(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.global_transpose";
    let on = p.get("follow").and_then(Value::as_bool).ok_or_else(|| bad(C, "`follow` required"))?;
    let tracks = tracks_param(e, C, p)?;
    let s = e.session_mut();
    let mut n = 0;
    for id in &tracks {
        if let Some(t) = s.track_mut(*id).filter(|t| t.kind.is_midi()) {
            t.transpose_independent = !on;
            n += 1;
        }
    }
    if n == 0 {
        return Err(bad(C, "select a MIDI or instrument track"));
    }
    Ok(json!({"tracks": n, "follow": on}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    /// Parity section 7: the global transpose track: changes set, replaced and removed; a track
    /// follows unless Independent or a drum instrument; Insert and Cut Time move the changes; MIDI
    /// export writes the transposed notes, as they play.
    #[test]
    fn the_transpose_track_shifts_what_follows_it() {
        let mut e = Engine::default();
        let keys = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let kit = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Kit"));
        e.session_mut().track_mut(kit).unwrap().instrument = Some(soundcraft_model::Insert::new("drum_synth"));
        e.execute("song.transpose", &json!({"at": 96_000, "semitones": 2})).unwrap();
        e.execute("song.transpose", &json!({"at": 192_000, "semitones": -3})).unwrap();
        e.execute("song.transpose", &json!({"at": 192_000, "semitones": 5})).unwrap();
        assert_eq!(e.session().transpose_track, vec![(96_000, 2), (192_000, 5)]);
        assert!(e.execute("song.transpose", &json!({"at": 0, "semitones": 30})).is_err());
        let s = e.session();
        let gt = |t, at| s.global_transpose(s.track(t).unwrap(), at);
        assert_eq!((gt(keys, 0), gt(keys, 96_000), gt(keys, 200_000)), (0, 2, 5));
        assert_eq!(gt(kit, 200_000), 0, "drums don't follow");
        e.execute("track.global_transpose", &json!({"tracks": [keys.0], "follow": false})).unwrap();
        assert_eq!(e.session().global_transpose(e.session().track(keys).unwrap(), 200_000), 0);
        assert!(e.undo());
        // Insert Time moves the changes after the point; Cut Time keeps the one in force.
        e.execute("event.insert_time", &json!({"start": 48_000, "length": 48_000})).unwrap();
        assert_eq!(e.session().transpose_track, vec![(144_000, 2), (240_000, 5)]);
        e.execute("event.cut_time", &json!({"start": 200_000, "end": 300_000})).unwrap();
        assert_eq!(e.session().transpose_track, vec![(144_000, 2), (200_000, 5)]);
        e.execute("song.transpose_remove", &json!({"at": 200_000})).unwrap();
        assert!(e.execute("song.transpose_remove", &json!({"at": 1})).is_err());
        // MIDI export writes the note as it plays: up 2 from bar 2.
        let c = e.execute("midi.new_clip", &json!({"track": keys.0, "start": 0, "end": 384_000})).unwrap()["clip"].as_u64().unwrap();
        e.execute("midi.note_add", &json!({"clip": c, "pitch": 60, "start_ticks": 0})).unwrap();
        e.execute("midi.note_add", &json!({"clip": c, "pitch": 60, "start_ticks": 7_680})).unwrap();
        let bytes = crate::io::export_midi(&e, Some(&[keys])).unwrap();
        let smf = soundcraft_midi::read_smf(&bytes).unwrap();
        let pitches: Vec<u8> = smf.tracks.iter().flat_map(|t| t.sequence.notes.iter().map(|n| n.pitch)).collect();
        assert_eq!(pitches, vec![60, 62]);
    }
}

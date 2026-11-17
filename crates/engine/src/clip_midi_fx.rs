//! A MIDI clip's own MIDI effects (`Clip::midi_fx`): what they make of its notes is rendered here,
//! off the audio thread, after a command or an undo step and when a session is opened, into the
//! clip's `midi_fx_render`, which the mix plays straight to the instrument (past the track's
//! effects). The render is made by the same effects code the track's effects run on
//! (`soundcraft_mix::midi_fx::print`), on the song's grid where the clip lies, so moving the clip,
//! editing its notes or effects, or changing the tempo makes it anew.

use soundcraft_model::{Clip, FxRender, Session};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// The fingerprint of what `c`'s render is made from; `None` when it has no effect on (no render).
fn key(s: &Session, c: &Clip) -> Option<u64> {
    if c.muted || c.midi_fx.iter().all(|f| f.bypass) {
        return None;
    }
    let seq = c.midi_playing()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let text = serde_json::to_string(&(seq, &c.midi_fx, c.start, c.length, &s.tempo, s.sample_rate)).unwrap_or_default();
    text.hash(&mut h);
    Some(h.finish())
}

fn stale(s: &Session, c: &Clip) -> bool {
    if c.midi_fx.is_empty() {
        return c.midi_fx_render.0.is_some();
    }
    key(s, c) != c.midi_fx_render.0.as_ref().map(|r| r.key)
}

/// Whether some clip's render is missing or out of date.
pub fn any_stale(s: &Session) -> bool {
    s.tracks.iter().flat_map(|t| t.playlists.iter()).flat_map(|p| p.clips.iter()).any(|c| stale(s, c))
}

/// The render of `c` (its effects run over its range on the song's grid).
fn render(s: &Session, c: &Clip, key: u64) -> Option<FxRender> {
    let seq = c.midi_playing()?;
    let keys = soundcraft_mix::midi_fx::sequence_events(s, c.start, c.end(), seq);
    let events = soundcraft_mix::midi_fx::print(s, &c.midi_fx, c.range(), &keys);
    Some(FxRender { key, events })
}

/// Makes every stale render anew.
pub fn refresh(s: &mut Session) {
    let mut made: Vec<(usize, usize, usize, Option<Arc<FxRender>>)> = Vec::new();
    for (ti, t) in s.tracks.iter().enumerate() {
        for (pi, p) in t.playlists.iter().enumerate() {
            for (ci, c) in p.clips.iter().enumerate() {
                if stale(s, c) {
                    let r = key(s, c).and_then(|k| render(s, c, k)).map(Arc::new);
                    made.push((ti, pi, ci, r));
                }
            }
        }
    }
    for (ti, pi, ci, r) in made {
        if let Some(c) = s.tracks.get_mut(ti).and_then(|t| t.playlists.get_mut(pi)).and_then(|p| p.clips.get_mut(ci)) {
            c.midi_fx_render.0 = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::ClipId;

    /// A clip's own arpeggiator plays its held chord as steps, on the clip only: the render has
    /// the arpeggio, follows a move, goes when the effect is removed, and the track's other clip
    /// plays as it was.
    #[test]
    fn a_clips_own_effects_are_rendered_and_follow_its_edits() {
        let mut e = Engine::default();
        let t = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let mut seq = soundcraft_midi::Sequence::default();
        for p in [60, 64, 67] {
            seq.notes.push(soundcraft_midi::Note {
                probability: 100,
                pitch: p,
                velocity: 100,
                muted: false,
                release_velocity: 64,
                channel: 0,
                start: 0,
                length: 3840,
            });
        }
        let (a, b) = (e.session_mut().new_clip_id(), e.session_mut().new_clip_id());
        let tid = soundcraft_model::TrackId(t);
        crate::edit::place_clip(e.session_mut(), tid, soundcraft_model::Clip::midi(a, "a", 0, 96_000, seq.clone()));
        crate::edit::place_clip(e.session_mut(), tid, soundcraft_model::Clip::midi(b, "b", 96_000, 96_000, seq));
        e.execute("track.midi_fx_add", &json!({"clips": [a.0], "kind": "arpeggiator"})).unwrap();
        let clip = |e: &Engine, id: ClipId| e.session().find_clip(id).unwrap().1.clone();
        let r = clip(&e, a).midi_fx_render.0.expect("rendered");
        let ons: Vec<(i64, u8)> = r.events.iter().filter(|(_, m)| m[0] & 0xF0 == 0x90 && m[2] > 0).map(|(at, m)| (*at, m[1])).collect();
        assert!(ons.len() > 3, "an arpeggio, not the chord: {ons:?}");
        assert!(ons.windows(2).all(|w| w[0].0 < w[1].0), "one note at a time: {ons:?}");
        assert!(ons.iter().all(|(at, _)| (0..96_000).contains(at)));
        assert!(clip(&e, b).midi_fx_render.0.is_none(), "the other clip has no effects");
        assert_eq!(clip(&e, a).midi_fx.len(), 1);
        assert!(e.session().track(tid).unwrap().midi_fx.is_empty(), "the track's list is untouched");
        // Moved, it is made anew where it lies.
        e.execute("clip.move", &json!({"clips": [a.0], "to": 192_000})).ok();
        let moved = clip(&e, a);
        if moved.start != 0 {
            let first = moved.midi_fx_render.0.unwrap().events.first().map(|x| x.0).unwrap();
            assert!(first >= moved.start, "{first} before {}", moved.start);
        }
        // Bypassed or removed, nothing is rendered.
        e.execute("track.midi_fx_set", &json!({"clips": [a.0], "index": 0, "bypass": true})).unwrap();
        assert!(clip(&e, a).midi_fx_render.0.is_none());
        e.execute("track.midi_fx_remove", &json!({"clips": [a.0]})).unwrap();
        assert!(clip(&e, a).midi_fx.is_empty() && clip(&e, a).midi_fx_render.0.is_none());
        // Saved and opened: the effects are kept, the render is made again.
        e.execute("track.midi_fx_add", &json!({"clips": [a.0], "kind": "chord"})).unwrap();
        let text = serde_json::to_string(e.session()).unwrap();
        assert!(text.contains("\"midi_fx\""));
    }
}

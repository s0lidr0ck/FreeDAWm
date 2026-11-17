//! Stretching a MIDI clip (`clip.stretch_to` on a MIDI clip: the Edit window's trim with Alt
//! held, as for audio): the clip's edges go to the points given, the other edge staying, and its
//! notes' starts and lengths and its controllers scale with it, counted in ticks from its start
//! (so a stretch across a tempo change keeps the notes in proportion to the bars).

use crate::{Engine, EngineError, Result};
use serde_json::{Value, json};
use soundcraft_model::{ClipContent, ClipId};
use soundcraft_time::Samples;

const C: &str = "clip.stretch_to";

fn bad(why: &str) -> EngineError {
    EngineError::BadParams(C.into(), why.into())
}

/// `ticks` scaled by `k`, rounded.
fn scaled(ticks: i64, k: f64) -> i64 {
    let v = (ticks as f64 * k).round();
    if v.is_finite() { v.clamp(-9e15, 9e15) as i64 } else { ticks }
}

/// Stretches MIDI clip `id` so it runs from `start` to `end` (each defaulting to where it is).
pub fn stretch(e: &mut Engine, id: ClipId, start: Option<Samples>, end: Option<Samples>) -> Result<Value> {
    if start.is_none() && end.is_none() {
        return Err(bad("give a start or an end that leaves the clip some length"));
    }
    let s = e.session_mut();
    let (sr, tempo) = (s.sample_rate, s.tempo.clone());
    let c = s.find_clip_mut(id).ok_or_else(|| bad("no such clip"))?;
    if c.edit_locked {
        return Err(bad("the clip is edit-locked"));
    }
    if c.pattern.is_some() {
        return Err(bad("a Juke pattern clip follows its pattern: convert it to MIDI first"));
    }
    let (a, b) = (start.unwrap_or(c.start).max(0), end.unwrap_or(c.end()));
    let ticks = |x: Samples| tempo.samples_to_ticks(x, sr);
    let old = ticks(c.end()).saturating_sub(ticks(c.start));
    let new = ticks(b).saturating_sub(ticks(a));
    if b <= a || old <= 0 || new <= 0 {
        return Err(bad("give a start or an end that leaves the clip some length"));
    }
    let k = new as f64 / old as f64;
    if let ClipContent::Midi { sequence } = &mut c.content {
        for n in &mut sequence.notes {
            let (st, en) = (scaled(n.start, k), scaled(n.end(), k));
            n.start = st;
            n.length = en.saturating_sub(st).max(1);
        }
        for x in &mut sequence.ctrls {
            x.tick = scaled(x.tick, k);
        }
        sequence.sort();
    }
    // The chords a following clip was written against stretch with it.
    if let Some(chords) = &mut c.follow_chords {
        for ch in chords.iter_mut() {
            ch.start = scaled(ch.start, k);
            ch.end = scaled(ch.end, k);
        }
    }
    c.start = a;
    c.length = b - a;
    c.fade_in.len = c.fade_in.len.min(c.length);
    c.fade_out.len = c.fade_out.len.min(c.length);
    Ok(json!({"start": a, "end": b, "ratio": k}))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, TrackKind};

    fn notes(e: &mut crate::Engine, clip: u64) -> Vec<(i64, i64)> {
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        n["notes"].as_array().unwrap().iter().map(|n| (n["start"].as_i64().unwrap(), n["length"].as_i64().unwrap())).collect()
    }

    #[test]
    fn stretching_a_midi_clip_scales_its_notes_and_controllers() {
        let mut e = crate::Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let bar = 96_000; // 4/4 at 120 bpm, 48 kHz
        let clip = e.execute("midi.new_clip", &json!({"track": t.0, "start": bar, "end": 2 * bar})).unwrap()["clip"].as_u64().unwrap();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": 0, "length_ticks": 480})).unwrap();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 62, "start_ticks": 1920, "length_ticks": 960})).unwrap();
        e.execute("midi.ctrl_add", &json!({"clip": clip, "lane": "cc1", "tick": 2880, "value": 64})).unwrap();
        // Alt-drag the end out to two bars: everything twice as long and twice as far.
        e.execute("clip.stretch_to", &json!({"clip": clip, "end": 3 * bar})).unwrap();
        assert_eq!(notes(&mut e, clip), vec![(0, 960), (3840, 1920)]);
        let c = e.execute("midi.ctrls", &json!({"clip": clip})).unwrap();
        assert_eq!(c["events"][0]["tick"], 5760);
        let (_, cl) = e.session().find_clip(soundcraft_model::ClipId(clip)).unwrap();
        assert_eq!((cl.start, cl.length), (bar, 2 * bar));
        // The start dragged in by a bar: back to the first length, ending where it did.
        e.execute("clip.stretch_to", &json!({"clip": clip, "start": 2 * bar})).unwrap();
        assert_eq!(notes(&mut e, clip), vec![(0, 480), (1920, 960)]);
        let (_, cl) = e.session().find_clip(soundcraft_model::ClipId(clip)).unwrap();
        assert_eq!((cl.start, cl.length), (2 * bar, bar));
        // One undo step each.
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(notes(&mut e, clip), vec![(0, 960), (3840, 1920)]);
        assert!(e.execute("clip.stretch_to", &json!({"clip": clip, "end": bar})).is_err(), "no length left");
    }
}

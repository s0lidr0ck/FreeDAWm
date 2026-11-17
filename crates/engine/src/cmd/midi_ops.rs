//! More MIDI operations, from the MIDI Operations window: reverse, half and double time, strum,
//! humanise and fixing overlaps. Each acts on the selected MIDI clips, or with `indices` on those
//! notes of one clip (the MIDI Editor's selection), as Quantize does.

use super::*;
use crate::cmd;
use soundcraft_midi::ops::{self, DurationOp};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "event.reverse_notes",
            "Reverse Notes",
            [],
            None,
            "{clips?, indices?} — the notes play backwards in the span they cover",
            has_selection,
            |e, p| { super::event::midi_op(e, p, |n, _| ops::reverse(n)) }
        ),
        cmd!(
            "event.time_scale",
            "Half or Double Time",
            [],
            None,
            "{factor: 2 (half time) | 0.5 (double time) | 0.1..10, clips?, indices?} — starts from the first note and lengths scale",
            has_selection,
            time_scale
        ),
        cmd!(
            "event.strum",
            "Strum",
            [],
            None,
            "{ticks?: 30 (between strings), direction?: up|down, clips?, indices?} — each chord's notes start one after another, ends kept",
            has_selection,
            strum
        ),
        cmd!(
            "event.humanize",
            "Humanize",
            [],
            None,
            "{timing?: ticks (±, default 10), velocity?: ± (default 8), seed?, clips?, indices?}",
            has_selection,
            humanize
        ),
        cmd!(
            "event.fix_overlaps",
            "Fix Overlaps",
            [],
            None,
            "{clips?, indices?} — a note ends where the next of its pitch starts",
            has_selection,
            |e, p| { super::event::midi_op(e, p, |n, _| ops::change_duration(n, DurationOp::RemoveOverlap)) }
        ),
    ]
}

fn time_scale(e: &mut Engine, p: &Value) -> Result<Value> {
    let f = f64_or(p, "factor", f64::NAN);
    if !(0.1..=10.0).contains(&f) {
        return Err(bad("event.time_scale", "`factor` from 0.1 to 10 (2: half time, 0.5: double time)"));
    }
    super::event::midi_op(e, p, move |n, _| ops::scale_time(n, f))
}

fn strum(e: &mut Engine, p: &Value) -> Result<Value> {
    let step = i64_or(p, "ticks", 30).clamp(0, 960);
    let up = match str_param(p, "direction") {
        None | Some("up") => true,
        Some("down") => false,
        Some(other) => return Err(bad("event.strum", format!("`direction` is up or down, not `{other}`"))),
    };
    super::event::midi_op(e, p, move |n, _| ops::strum(n, step, up))
}

fn humanize(e: &mut Engine, p: &Value) -> Result<Value> {
    let timing = i64_or(p, "timing", 10).clamp(0, 960);
    let velocity = i64_or(p, "velocity", 8).clamp(0, 127) as u8;
    let seed = p.get("seed").and_then(Value::as_u64).unwrap_or(ops::DEFAULT_SEED);
    // A note in the clip is not moved before its start (where it would stop playing).
    super::event::midi_op(e, p, move |n, _| {
        let inside: Vec<bool> = n.iter().map(|x| x.start >= 0).collect();
        ops::humanize(n, timing, velocity, seed);
        for (x, was) in n.iter_mut().zip(inside) {
            if was && x.start < 0 {
                x.start = 0;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, TrackKind};

    fn notes(e: &mut Engine, clip: u64) -> Vec<(u64, i64, i64)> {
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        n["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["pitch"].as_u64().unwrap(), x["start"].as_i64().unwrap(), x["length"].as_i64().unwrap()))
            .collect()
    }

    #[test]
    fn operations_act_on_the_picked_notes_and_are_one_undo_step_each() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let clip = e.execute("midi.new_clip", &json!({"track": t.0, "start": 0, "end": 192_000})).unwrap()["clip"].as_u64().unwrap();
        for (pitch, start) in [(60, 0), (64, 0), (67, 0), (72, 1_920)] {
            e.execute("midi.note_add", &json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": 960})).unwrap();
        }
        let undo = e.undo_history().len();
        e.execute("event.strum", &json!({"clips": [clip], "indices": [0, 1, 2], "ticks": 40})).unwrap();
        assert_eq!(notes(&mut e, clip), vec![(60, 0, 960), (64, 40, 920), (67, 80, 880), (72, 1_920, 960)]);
        assert_eq!(e.undo_history().len(), undo + 1);
        e.execute("event.time_scale", &json!({"clips": [clip], "factor": 0.5})).unwrap();
        assert_eq!(notes(&mut e, clip).last(), Some(&(72, 960, 480)));
        assert!(e.execute("event.time_scale", &json!({"clips": [clip], "factor": 0})).is_err());
        e.execute("event.reverse_notes", &json!({"clips": [clip], "indices": [3]})).unwrap();
        assert_eq!(notes(&mut e, clip).last(), Some(&(72, 960, 480)), "one note reversed in its own span stays");
        e.execute("event.humanize", &json!({"clips": [clip], "timing": 0, "velocity": 0})).unwrap();
        assert_eq!(notes(&mut e, clip).last(), Some(&(72, 960, 480)));
        e.execute("event.fix_overlaps", &json!({"clips": [clip]})).unwrap();
    }
}

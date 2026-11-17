//! A track's MIDI input transform (`track.midi_input_transform`): transpose, velocity scale,
//! curve and fixed velocity, a key range and a channel, applied to what the track's instrument
//! hears live and to what a take records (`soundcraft_model::MidiTransform`), never to its clips.
//! Kept with the track's MIDI input (`Route::Midi`), so it stays when the port or channel
//! changes.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{MidiTransform, Route};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "track.midi_input_transform",
        "MIDI Input Transform",
        [],
        None,
        "{tracks?, transpose?: -48..48, velocity?: 1..200 (%), curve?: -100..100, fixed_velocity?: 0 (off)..127, key_low?: 0..127, key_high?: 0..127, channel?: 0 (as played)..16, reset?: bool} — what MIDI and instrument tracks do to their MIDI input before their instrument hears it and a take keeps it → {transform}",
        has_selection,
        set_transform
    )]
}

fn set_transform(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.midi_input_transform";
    let tracks = tracks_required(e, C, p)?;
    let int = |key: &str, lo: i64, hi: i64| -> Result<Option<i64>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_f64()
                .map(|x| x.round() as i64)
                .filter(|x| (lo..=hi).contains(x))
                .map(Some)
                .ok_or_else(|| bad(C, format!("`{key}` is {lo} to {hi}"))),
        }
    };
    let transpose = int("transpose", -48, 48)?;
    let velocity = int("velocity", 1, 200)?;
    let curve = int("curve", -100, 100)?;
    let fixed = int("fixed_velocity", 0, 127)?;
    let low = int("key_low", 0, 127)?;
    let high = int("key_high", 0, 127)?;
    let channel = int("channel", 0, 16)?;
    let reset = p.get("reset").and_then(Value::as_bool).unwrap_or(false);
    if let Some(t) = tracks.iter().filter_map(|t| e.session().track(*t)).find(|t| !matches!(t.mixer.input, Route::Midi(_))) {
        return Err(bad(C, format!("`{}` has no MIDI input (Track › Input: a MIDI port or All Inputs)", t.name)));
    }
    let mut last = MidiTransform::default();
    let s = e.session_mut();
    for t in &tracks {
        let Some(Route::Midi(m)) = s.track_mut(*t).map(|tr| &mut tr.mixer.input) else { continue };
        let x = &mut m.transform;
        if reset {
            *x = MidiTransform::default();
        }
        // (Each in range, checked above.)
        if let Some(v) = transpose {
            x.transpose = v as i8;
        }
        if let Some(v) = velocity {
            x.velocity = v as u8;
        }
        if let Some(v) = curve {
            x.curve = v as i8;
        }
        if let Some(v) = fixed {
            x.fixed_velocity = v as u8;
        }
        if let Some(v) = low {
            x.key_low = v as u8;
        }
        if let Some(v) = high {
            x.key_high = v as u8;
        }
        if let Some(v) = channel {
            x.channel = v as u8;
        }
        if x.key_low > x.key_high {
            std::mem::swap(&mut x.key_low, &mut x.key_high);
        }
        last = *x;
    }
    Ok(json!({"transform": last}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::MidiTransform;

    #[test]
    fn the_transform_maps_notes_consistently_and_checks_its_values() {
        let t = MidiTransform { transpose: -2, velocity: 50, ..MidiTransform::default() };
        assert_eq!(t.apply([0x91, 60, 100]), Some([0x91, 58, 50]));
        assert_eq!(t.apply([0x81, 60, 0]), Some([0x81, 58, 0]), "the note-off follows the note-on");
        assert_eq!(t.apply([0x90, 1, 100]), None, "moved below 0: left out");
        assert_eq!(t.apply([0xB0, 64, 127]), Some([0xB0, 64, 127]), "controllers kept");
        let hard = MidiTransform { curve: 50, ..MidiTransform::default() };
        assert!(hard.apply([0x90, 60, 64]).unwrap()[2] < 64, "a harder curve: soft playing softer");
        assert_eq!(hard.apply([0x90, 60, 127]).unwrap()[2], 127);
        let ch = MidiTransform { channel: 10, ..MidiTransform::default() };
        assert_eq!(ch.apply([0xE3, 0, 64]), Some([0xE9, 0, 64]));
        let mut e = Engine::default();
        let id = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        assert!(e.execute("track.midi_input_transform", &json!({"tracks": [id], "transpose": 60})).is_err());
        let r = e.execute("track.midi_input_transform", &json!({"tracks": [id], "key_low": 80, "key_high": 20})).unwrap();
        assert_eq!((r["transform"]["key_low"].as_u64(), r["transform"]["key_high"].as_u64()), (Some(20), Some(80)));
        let audio = e.execute("track.new", &json!({"kind": "audio"})).unwrap()["tracks"][0].as_u64().unwrap();
        assert!(e.execute("track.midi_input_transform", &json!({"tracks": [audio], "transpose": 1})).is_err());
    }
}

//! Setup › Session Rate: change the open session's sample rate later
//! ([`soundcraft_model::rate_change`]).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_audio_io::FileFormat;
use soundcraft_model::{ClipContent, SourceId};
use soundcraft_time::SampleRate;
use std::collections::HashMap;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "session.set_sample_rate",
        "Change Session Rate...",
        [],
        None,
        "{rate: Hz (44100, 48000, 88200, 96000, 176400, 192000, 352800 or 384000)} — converts the open session in one undo step: every source is resampled to new audio (the old files stay for Undo), every position in samples is moved to the same time, plug-ins are prepared again at the new rate → {rate, sources}",
        always,
        set_rate
    )]
}

fn set_rate(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "session.set_sample_rate";
    let hz = p.get("rate").and_then(Value::as_u64).and_then(|r| u32::try_from(r).ok()).ok_or_else(|| bad(C, "`rate` (Hz) required"))?;
    if !SampleRate::COMMON.contains(&hz) {
        return Err(bad(C, format!("{hz} Hz is not a session rate (44100 to 384000)")));
    }
    let to = SampleRate::new(hz).map_err(|err| bad(C, err.to_string()))?;
    if e.session().sample_rate == to {
        return Ok(json!({"rate": hz, "sources": 0}));
    }
    // The audio at the old rate, read before the session changes.
    let s = e.session();
    let audio: Vec<(SourceId, String, soundcraft_audio_io::AudioBuffer)> =
        s.sources.iter().filter_map(|src| s.pool.get(src.id).map(|a| (src.id, src.name.clone(), a.buffer().into_owned()))).collect();
    let s = e.session_mut();
    soundcraft_model::rate_change::scale_positions(s, to);
    // Each source again at the new rate (`add_source` converts it), as new audio: the old
    // files are left alone for Undo.
    let mut map: HashMap<SourceId, SourceId> = HashMap::new();
    for (id, name, buf) in audio {
        let new = crate::io::add_source(s, &name, buf, None, FileFormat::Wav);
        map.insert(id, new);
    }
    let swap = |id: &mut SourceId| {
        if let Some(n) = map.get(id) {
            *id = *n;
        }
    };
    for t in &mut s.tracks {
        for c in t.playlists.iter_mut().flat_map(|pl| pl.clips.iter_mut()) {
            if let ClipContent::Audio { source, .. } = &mut c.content {
                swap(source);
            }
            if let Some(alt) = &mut c.clean {
                swap(&mut alt.source);
                swap(&mut alt.original);
            }
        }
        if let Some(f) = &mut t.freeze {
            swap(&mut f.source);
        }
    }
    for c in &mut s.captured {
        if let ClipContent::Audio { source, .. } = &mut c.content {
            swap(source);
        }
    }
    Ok(json!({"rate": hz, "sources": map.len()}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::{Value, json};

    /// Every integer the demo session saves that does not change with the rate, as `parent.key`
    /// (MIDI notes are in ticks; the sources' own fields are replaced with the sources): a new
    /// field holding a position in samples must be scaled by `rate_change` (or listed here if it
    /// is not a position), so none is missed.
    const NOT_POSITIONS: &[&str] = &[
        ".format_version",
        ".next_id",
        "Audio.source",
        "busses.id",
        "clips.id",
        "edit.countoff_bars",
        "edit.selected_tracks",
        "input.Bus",
        "inserts.id",
        "markers.id",
        "markers.number",
        "markers.ruler",
        "meters.denominator",
        "meters.numerator",
        "mixer.next_module",
        "notes.length",
        "notes.pitch",
        "notes.release_velocity",
        "notes.start",
        "notes.velocity",
        "sources.channels",
        "sources.frames",
        "sources.id",
        "sources.sample_rate",
        "target.Bus",
        "tracks.color",
        "tracks.id",
    ];

    fn walk(v: &Value, key: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => m.iter().for_each(|(k, x)| walk(x, k, out)),
            Value::Array(a) => a.iter().for_each(|x| walk(x, key, out)),
            Value::Number(n) if n.is_i64() || n.is_u64() => out.push(key.to_string()),
            _ => {}
        }
    }

    /// The demo session at 48 kHz converted to 96 kHz: the mix renders the same (at the new rate),
    /// every position is twice what it was, the audio twice as long; Undo gives it all back.
    #[test]
    fn a_session_converted_to_another_rate_plays_the_same_at_the_same_times() {
        let mut e = soundcraft_engine_demo();
        let before = e.session().clone();
        let r = soundcraft_time::Range::new(480_000, 480_000 + 9_600);
        let old_mix = soundcraft_mix::render_range(&before, r, 512);
        let v = e.execute("session.set_sample_rate", &json!({"rate": 96_000})).unwrap();
        assert!(v["sources"].as_u64().unwrap() > 0);
        let s = e.session();
        assert_eq!(s.sample_rate.hz(), 96_000);
        for (a, b) in before.tracks.iter().zip(&s.tracks) {
            for (x, y) in a.clips().iter().zip(b.clips()) {
                assert_eq!((y.start, y.length), (x.start * 2, x.length * 2), "{}", x.name);
            }
        }
        for (a, b) in before.markers.iter().zip(&s.markers) {
            assert_eq!(b.start, a.start * 2);
        }
        let new_mix = soundcraft_mix::render_range(s, soundcraft_time::Range::new(r.start * 2, r.end * 2), 512);
        // The same sound: the old mix's energy, and the new mix decimated back lines up with it.
        let energy = |c: &[f32]| c.iter().map(|x| x * x).sum::<f32>() / c.len().max(1) as f32;
        let (eo, en) = (energy(&old_mix[0]), energy(&new_mix[0]));
        assert!(eo > 1e-6 && (en / eo - 1.0).abs() < 0.2, "{eo} vs {en}");
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().sample_rate.hz(), 48_000);
        assert_eq!(e.session().tracks[0].clips(), before.tracks[0].clips());
    }

    fn soundcraft_engine_demo() -> Engine {
        crate::demo::demo_engine()
    }

    /// Every integer field the demo session saves is either a position `rate_change` scales or
    /// one listed as not a position here.
    #[test]
    fn every_integer_field_is_known() {
        let mut e = crate::demo::demo_engine();
        let mut before = Vec::new();
        walk(&serde_json::to_value(e.session()).unwrap(), "", &mut before);
        let mut s = e.session().clone();
        soundcraft_model::rate_change::scale_positions(&mut s, soundcraft_time::SampleRate::new(96_000).unwrap());
        let (a, b) = (serde_json::to_value(e.session_mut().clone()).unwrap(), serde_json::to_value(&s).unwrap());
        let mut unscaled = Vec::new();
        same_ints(&a, &b, "", &mut unscaled);
        unscaled.sort();
        unscaled.dedup();
        let unknown: Vec<&String> = unscaled.iter().filter(|k| !NOT_POSITIONS.contains(&k.as_str())).collect();
        assert!(unknown.is_empty(), "integer fields left unscaled and not listed: {unknown:?}");
        assert!(!before.is_empty());
    }

    /// Keys whose integer value is the same (and not 0) before and after, as `parent.key` (the
    /// key of the object or list holding it, then its own).
    fn same_ints(a: &Value, b: &Value, key: &str, out: &mut Vec<String>) {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let parent = key.rsplit('.').next().unwrap_or("");
                for (k, v) in x {
                    if let Some(w) = y.get(k) {
                        same_ints(v, w, &format!("{parent}.{k}"), out);
                    }
                }
            }
            (Value::Array(x), Value::Array(y)) => x.iter().zip(y).for_each(|(v, w)| same_ints(v, w, key, out)),
            (Value::Number(x), Value::Number(y)) if (x.is_i64() || x.is_u64()) && x == y && x.as_i64() != Some(0) => out.push(key.to_string()),
            _ => {}
        }
    }
}

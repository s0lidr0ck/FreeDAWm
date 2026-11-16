//! Grooves and playback quantize: the grooves Quantize can follow (built in, or taken from a
//! clip), and quantize applied as a clip plays, with its notes left where they are.
//!
//! A groove repeats every `period` ticks from each bar line; its points are where the notes of
//! that span fall. Playback quantize is kept on the clip (`clip.play_quantize`) or on the track
//! with the Real-Time Properties (`event.midi_rtp` `quantize`); the clip's own wins. Applying it
//! writes the played positions into the notes and keeps the performance before it, so Restore
//! Performance brings it back (through trims and splits, as the stored performance follows them).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::ops::{Groove, PlayQuantize, builtin_grooves, groove_from_notes};
use soundcraft_model::{ClipContent, ClipId, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "event.grooves", "List Grooves", [], None, "{} -> {grooves: [{name, period, points, built_in}]}", always, list),
        cmd!(
            "event.groove_from_clip",
            "Groove from Clip",
            [],
            None,
            "{clip?, name?, grid?: '1/16', bars?: 1} — the clip's timing as a groove (each grid step where its notes fall, over `bars` bars), kept in the session -> {name, period, points}",
            has_selection,
            from_clip
        ),
        cmd!(
            "clip.play_quantize",
            "Playback Quantize",
            [],
            None,
            "{clips?, grid?: '1/16' | ticks | 'off', strength?: 100, swing?: 0, groove?: name, apply?: bool} — notes play quantized and stay where they are; `apply` writes it into the notes (Restore Performance undoes it) and clears it",
            has_selection,
            play_quantize
        ),
    ]
}

/// The groove called `name`: one taken from a clip in this session, else a built-in one.
pub fn groove_named(s: &Session, name: &str) -> Option<Groove> {
    let n = name.trim();
    s.grooves
        .iter()
        .find(|g| g.name.eq_ignore_ascii_case(n))
        .cloned()
        .or_else(|| builtin_grooves().into_iter().find(|g| g.name.eq_ignore_ascii_case(n)))
}

fn list(e: &mut Engine, _: &Value) -> Result<Value> {
    let s = e.session();
    let row = |g: &Groove, built_in: bool| json!({"name": g.name, "period": g.period, "points": g.points, "built_in": built_in});
    let mut v: Vec<Value> = builtin_grooves().iter().map(|g| row(g, true)).collect();
    v.extend(s.grooves.iter().map(|g| row(g, false)));
    Ok(json!({"grooves": v}))
}

/// The MIDI clips named by `clips` (or `clip`), else the selected ones.
fn midi_clips(e: &Engine, p: &Value) -> Vec<ClipId> {
    let s = e.session();
    clip_ids_param(e, p).into_iter().filter(|id| s.find_clip(*id).is_some_and(|(_, c)| matches!(c.content, ClipContent::Midi { .. }))).collect()
}

/// Most grooves a session keeps.
const MAX_GROOVES: usize = 64;

fn from_clip(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "event.groove_from_clip";
    let id = *midi_clips(e, p).first().ok_or_else(|| bad(C, "select a MIDI clip"))?;
    let slot = crate::cmd::event::grid_ticks(C, p)?;
    let bars = i64_or(p, "bars", 1).clamp(1, 4);
    let s = e.session();
    let Some((_, clip)) = s.find_clip(id) else { return Err(bad(C, "no such clip")) };
    let ClipContent::Midi { sequence } = &clip.content else { return Err(bad(C, "not a MIDI clip")) };
    let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
    let tpb = s.tempo.meter_at_tick(base).ticks_per_bar().max(1);
    let name = match str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => format!("Groove from {}", clip.name),
    };
    let tempo = &s.tempo;
    let bar_start = |x: i64| tempo.bar_start_tick(tempo.bar_beat_at_tick(base.saturating_add(x)).bar).saturating_sub(base);
    let g = groove_from_notes(&name, &sequence.notes, tpb.saturating_mul(bars), slot, bar_start)
        .ok_or_else(|| bad(C, "the clip has no notes near the grid to take a groove from"))?;
    let out = json!({"name": g.name, "period": g.period, "points": g.points});
    let s = e.session_mut();
    s.grooves.retain(|x| !x.name.eq_ignore_ascii_case(&g.name));
    if s.grooves.len() >= MAX_GROOVES {
        s.grooves.remove(0);
    }
    s.grooves.push(g);
    Ok(out)
}

fn play_quantize(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.play_quantize";
    let ids = midi_clips(e, p);
    if ids.is_empty() {
        return Err(bad(C, "select a MIDI clip"));
    }
    if bool_or(p, "apply", false) {
        return apply(e, &ids);
    }
    let off = matches!(p.get("grid").and_then(Value::as_str), Some(g) if g.trim().eq_ignore_ascii_case("off"));
    let setting = if off {
        None
    } else {
        let groove = match str_param(p, "groove").map(str::trim).filter(|g| !g.is_empty()) {
            Some(name) => Some(groove_named(e.session(), name).ok_or_else(|| bad(C, format!("no groove called '{name}'")))?),
            None => None,
        };
        Some(PlayQuantize {
            grid: crate::cmd::event::grid_ticks(C, p)?,
            strength: (f32_or(p, "strength", 100.0) / 100.0).clamp(0.0, 1.0),
            swing: (f32_or(p, "swing", 0.0) / 100.0).clamp(0.0, 1.0),
            groove,
        })
    };
    let s = e.session_mut();
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            c.play_quantize = setting.clone();
        }
    }
    Ok(json!({"clips": ids.len(), "on": setting.is_some()}))
}

/// Writes each clip's playback quantize (its own or its track's) into its notes, keeping the
/// performance before it for Restore Performance, and clears the clip's own.
fn apply(e: &mut Engine, ids: &[ClipId]) -> Result<Value> {
    let s = e.session_mut();
    let mut notes = 0;
    for id in ids {
        let Some((tid, clip)) = s.find_clip(*id).map(|(t, c)| (t, c.clone())) else { continue };
        let Some(track) = s.track(tid) else { continue };
        let Some(q) = soundcraft_mix::play_quantize(s, track, &clip).map(std::borrow::Cow::into_owned) else { continue };
        let ClipContent::Midi { sequence } = &clip.content else { continue };
        let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
        let mut seq = sequence.clone();
        for n in seq.notes.iter_mut() {
            n.start = soundcraft_mix::quantized_start(s, base, &q, n.start);
        }
        seq.sort();
        notes += seq.notes.len();
        s.midi_originals.entry(id.0).or_insert_with(|| sequence.clone());
        if let Some(c) = s.find_clip_mut(*id) {
            c.content = ClipContent::Midi { sequence: seq };
            c.play_quantize = None;
        }
    }
    Ok(json!({"notes": notes}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    /// A Keys clip of one bar with notes 30 ticks late on each 16th of beat 1 and on the
    /// off-beat 8th.
    fn setup() -> (Engine, u64) {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let c = e.execute("midi.new_clip", &json!({"track": t.0, "start": 0, "end": 96_000})).unwrap()["clip"].as_u64().unwrap();
        for start in [30, 270, 510, 750] {
            e.execute("midi.note_add", &json!({"clip": c, "pitch": 60, "start_ticks": start, "length_ticks": 100})).unwrap();
        }
        (e, c)
    }

    fn starts(e: &Engine, c: u64) -> Vec<i64> {
        match &e.session().find_clip(ClipId(c)).unwrap().1.content {
            ClipContent::Midi { sequence } => sequence.notes.iter().map(|n| n.start).collect(),
            _ => panic!("a MIDI clip"),
        }
    }

    #[test]
    fn a_groove_is_taken_from_a_clip_and_listed() {
        let (mut e, c) = setup();
        let r = e.execute("event.groove_from_clip", &json!({"clip": c, "name": "Late", "grid": "1/16"})).unwrap();
        assert_eq!(r["period"], 3840);
        let pts: Vec<i64> = r["points"].as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect();
        assert_eq!(&pts[..5], &[30, 270, 510, 750, 960], "the steps with notes are late, the others straight");
        let list = e.execute("event.grooves", &json!({})).unwrap();
        assert!(list["grooves"].as_array().unwrap().iter().any(|g| g["name"] == "Late" && g["built_in"] == false));
        assert!(list["grooves"].as_array().unwrap().iter().any(|g| g["name"] == "Swing 16ths 66%"));
        // Quantizing straight notes to it moves them late.
        let c2 = e.execute("midi.new_clip", &json!({"track": e.session().tracks[0].id.0, "start": 96_000, "end": 192_000})).unwrap()["clip"]
            .as_u64()
            .unwrap();
        e.execute("midi.note_add", &json!({"clip": c2, "pitch": 60, "start_ticks": 240})).unwrap();
        e.execute("event.quantize", &json!({"clips": [c2], "groove": "Late"})).unwrap();
        assert_eq!(starts(&e, c2), vec![270]);
        assert!(e.execute("event.groove_from_clip", &json!({"clip": c2, "grid": "1/7"})).is_err());
        assert!(e.execute("event.quantize", &json!({"clips": [c2], "groove": "Nope"})).is_err());
    }

    #[test]
    fn playback_quantize_keeps_the_notes_and_apply_can_be_restored() {
        let (mut e, c) = setup();
        e.execute("clip.play_quantize", &json!({"clips": [c], "grid": "1/16"})).unwrap();
        assert_eq!(starts(&e, c), vec![30, 270, 510, 750], "the notes stay where they are");
        let (t, clip) = e.session().find_clip(ClipId(c)).map(|(t, c)| (t, c.clone())).unwrap();
        let track = e.session().track(t).unwrap();
        let q = soundcraft_mix::play_quantize(e.session(), track, &clip).unwrap();
        assert_eq!(soundcraft_mix::quantized_start(e.session(), 0, &q, 30), 0);
        assert_eq!(soundcraft_mix::quantized_start(e.session(), 0, &q, 270), 240);
        // Apply writes it into the notes; Restore Performance undoes it.
        e.execute("clip.play_quantize", &json!({"clips": [c], "apply": true})).unwrap();
        assert_eq!(starts(&e, c), vec![0, 240, 480, 720]);
        assert!(e.session().find_clip(ClipId(c)).unwrap().1.play_quantize.is_none());
        e.execute("event.restore_performance", &json!({"clips": [c]})).unwrap();
        assert_eq!(starts(&e, c), vec![30, 270, 510, 750]);
        // Off: the clip plays as written; a track's quantize is used when the clip has none.
        e.execute("clip.play_quantize", &json!({"clips": [c], "grid": "off"})).unwrap();
        e.execute("event.midi_rtp", &json!({"tracks": [t.0], "quantize": 480, "q_strength": 50})).unwrap();
        let clip = e.session().find_clip(ClipId(c)).unwrap().1.clone();
        let q = soundcraft_mix::play_quantize(e.session(), e.session().track(t).unwrap(), &clip).unwrap();
        assert_eq!((q.grid, q.strength), (480, 0.5));
        assert_eq!(soundcraft_mix::quantized_start(e.session(), 0, &q, 270), 375);
    }
}

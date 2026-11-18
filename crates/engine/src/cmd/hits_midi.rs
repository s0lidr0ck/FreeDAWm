//! Drum hits in audio written as MIDI notes: the transients Beat Detective finds in the selected
//! audio tracks, a note each on a new instrument track playing the drum synth.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{ChannelFormat, Clip, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "event.hits_to_midi",
        "Drum Hits to MIDI",
        [],
        None,
        "{tracks?, start?, end?, sensitivity?: 0.5, pitch?: 36, velocity?: 100} — a note on `pitch` for each hit Beat Detective finds in the range, in a new MIDI clip on a new drum-synth track",
        has_range,
        hits_to_midi
    )]
}

fn hits_to_midi(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "event.hits_to_midi";
    let tracks = tracks_param(e, C, p)?;
    let r = range_param(e, C, p)?;
    if r.is_empty() {
        return Err(bad(C, "select a range to find hits in"));
    }
    if r.len() > e.session().sample_rate.samples(20.0 * 60.0) {
        return Err(bad(C, "at most 20 minutes at a time"));
    }
    let sens = f32_or(p, "sensitivity", 0.5).clamp(0.0, 1.0);
    let pitch = i64_or(p, "pitch", 36).clamp(0, 127) as u8;
    let velocity = i64_or(p, "velocity", 100).clamp(1, 127) as u8;
    let s = e.session();
    let audio: Vec<_> = tracks.iter().copied().filter(|t| s.track(*t).is_some_and(|x| x.kind.has_playlist() && !x.kind.is_midi())).collect();
    if audio.is_empty() {
        return Err(bad(C, "select an audio track"));
    }
    let mut hits: Vec<Samples> = audio.iter().flat_map(|t| crate::io::transients_in(s, *t, r, sens)).collect();
    hits.sort_unstable();
    // Hits closer than 30 ms are one hit (two tracks of one kit, a flam).
    let gap = s.sample_rate.samples(0.03);
    let mut kept: Vec<Samples> = Vec::new();
    for h in hits {
        if kept.last().is_none_or(|l| h - l >= gap) {
            kept.push(h);
        }
    }
    if kept.is_empty() {
        return Err(bad(C, "no hits found: try a higher sensitivity"));
    }
    let name = s.track(audio[0]).map_or_else(|| "Hits".to_string(), |t| format!("{} Hits", t.name));
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(r.start, sr);
    let mut seq = soundcraft_midi::Sequence::default();
    for h in &kept {
        let start = s.tempo.samples_to_ticks(*h, sr).saturating_sub(base).max(0);
        seq.notes.push(soundcraft_midi::Note {
            probability: 100,
            pitch,
            velocity,
            muted: false,
            release_velocity: 64,
            channel: 9,
            start,
            length: 120,
        });
    }
    seq.sort();
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(&name));
    if let Some(tr) = s.track_mut(t) {
        tr.instrument = Some(soundcraft_model::Insert::new("drum_synth"));
    }
    let id = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::midi(id, name, r.start, r.len(), seq));
    Ok(json!({"track": t.0, "clip": id.0, "notes": kept.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parity section 7: the demo's kick played as audio comes back as kick notes, on their hits.
    #[test]
    fn drum_hits_become_notes() {
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().clone();
        let c = kick.clips()[0].clone();
        let end = c.start + 48_000 * 8;
        let found =
            e.execute("event.beat_detective", &json!({"tracks": [kick.id.0], "start": c.start, "end": end})).unwrap()["count"].as_u64().unwrap();
        let r = e.execute("event.hits_to_midi", &json!({"tracks": [kick.id.0], "start": c.start, "end": end})).unwrap();
        let n = r["notes"].as_u64().unwrap();
        assert!(n > 4 && n <= found, "{r} against {found} hits");
        let t = e.session().track(soundcraft_model::TrackId(r["track"].as_u64().unwrap())).unwrap();
        assert_eq!(t.instrument.as_ref().unwrap().plugin, "drum_synth");
        let soundcraft_model::ClipContent::Midi { sequence } = &t.clips()[0].content else { panic!() };
        assert!(sequence.notes.iter().all(|x| x.pitch == 36 && x.channel == 9));
        assert!(e.undo());
        assert!(e.execute("event.hits_to_midi", &json!({"tracks": [kick.id.0], "start": 0, "end": 0})).is_err());
    }
}

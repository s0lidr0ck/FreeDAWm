//! MIDI Cleanup on a MIDI clip: its settings, the Original/Cleaned switch, Commit and Remove. The
//! cleaned notes are a preview derived from the take ([`soundcraft_model::MidiCleanup`]); the
//! take itself is untouched until Commit.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::cleanup::CleanupSettings;
use soundcraft_model::{ClipContent, ClipId, MidiCleanup, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "midi.cleanup",
            "MIDI Cleanup",
            [],
            None,
            "{clip? | clips?, wrong?, held?, legato?, extend?: 0..100 (% of each stage), protect_melody?: bool, original?: bool (true plays the untouched take), toggle?: bool (flips original), reset?: bool (settings back to defaults)} — non-destructive cleanup of MIDI clips against the band's chord lane and key: the first call gives the clip settings (defaults 40/70/30/40), the cleaned notes are derived (recomputed when notes, chords, key, tempo or settings change) and play instead of the take; undoable (the UI merges a drag into one step)",
            always,
            cleanup
        ),
        cmd!(
            "midi.cleanup_commit",
            "Commit MIDI Cleanup",
            [],
            None,
            "{clip? | clips?} — replaces the clip's notes with what plays (the cleaned notes, or the take when switched to Original) and drops its cleanup settings",
            always,
            commit
        ),
        cmd!(
            "midi.cleanup_remove",
            "Remove MIDI Cleanup",
            [],
            None,
            "{clip? | clips?} — drops the clip's cleanup settings; the take plays untouched",
            always,
            remove
        ),
    ]
}

/// The MIDI clips among `p`'s clips (or the selection), with a cleanup when `with_cleanup`.
fn targets(e: &Engine, p: &Value, with_cleanup: bool) -> Vec<ClipId> {
    clip_ids_param(e, p)
        .into_iter()
        .filter(|id| {
            e.session()
                .find_clip(*id)
                .is_some_and(|(_, c)| matches!(c.content, ClipContent::Midi { .. }) && (!with_cleanup || c.midi_cleanup.is_some()))
        })
        .collect()
}

/// An optional 0..=100 amount, rounded; `None` when absent.
fn amount(p: &Value, key: &str) -> Result<Option<u8>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64().filter(|m| m.is_finite() && (0.0..=100.0).contains(m)) {
            Some(m) => Ok(Some(m.round() as u8)),
            None => Err(bad("midi.cleanup", format!("`{key}` must be a number from 0 to 100 (%)"))),
        },
    }
}

fn cleanup(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "midi.cleanup";
    let ids = targets(e, p, false);
    if ids.is_empty() {
        return Err(bad(ID, "choose a MIDI clip"));
    }
    let wrong = amount(p, "wrong")?;
    let held = amount(p, "held")?;
    let legato = amount(p, "legato")?;
    let extend = amount(p, "extend")?;
    let protect = p.get("protect_melody").and_then(Value::as_bool);
    let original = p.get("original").and_then(Value::as_bool);
    let toggle = bool_or(p, "toggle", false);
    let reset = bool_or(p, "reset", false);
    // The new state of each clip's cleanup; the session is touched only where one differs.
    let mut changes: Vec<(ClipId, MidiCleanup)> = Vec::new();
    for id in &ids {
        let Some((_, c)) = e.session().find_clip(*id) else { continue };
        let mut m = c.midi_cleanup.clone().unwrap_or_default();
        if reset {
            m.settings = CleanupSettings::default();
        }
        let k = &mut m.settings;
        k.wrong = wrong.unwrap_or(k.wrong);
        k.held = held.unwrap_or(k.held);
        k.legato = legato.unwrap_or(k.legato);
        k.extend = extend.unwrap_or(k.extend);
        k.protect_melody = protect.unwrap_or(k.protect_melody);
        m.original = original.unwrap_or(m.original);
        if toggle {
            m.original = !m.original;
        }
        if c.midi_cleanup.as_ref() != Some(&m) {
            changes.push((*id, m));
        }
    }
    if changes.is_empty() {
        return Ok(report(e.session(), &ids));
    }
    let s = e.session_mut();
    for (id, m) in changes {
        if let Some(c) = s.find_clip_mut(id) {
            // Keep the preview already derived; refresh recomputes it if the settings changed.
            let preview = c.midi_cleanup.take().and_then(|o| o.preview);
            c.midi_cleanup = Some(MidiCleanup { preview, ..m });
        }
    }
    crate::midi_cleanup::refresh(s);
    Ok(report(s, &ids))
}

/// The result of `midi.cleanup` for `ids`.
fn report(s: &Session, ids: &[ClipId]) -> Value {
    let mut out = Vec::new();
    for id in ids.iter().copied() {
        let Some((_, c)) = s.find_clip(id) else { continue };
        let Some(m) = &c.midi_cleanup else { continue };
        let report = m.preview.as_ref().map(|v| v.out.report).unwrap_or_default();
        let k = m.settings;
        out.push(json!({
            "clip": id.0,
            "settings": {"wrong": k.wrong, "held": k.held, "legato": k.legato, "extend": k.extend, "protect_melody": k.protect_melody},
            "original": m.original,
            "report": {
                "total": report.total, "removed": report.removed, "held_trimmed": report.held_trimmed,
                "legato_trimmed": report.legato_trimmed, "legato_closed": report.legato_closed,
                "extended": report.extended, "pedal_changes": report.pedal_changes,
            },
            "summary": report.summary(),
            "chords": !crate::midi_cleanup::chord_spans(s, c).is_empty(),
        }));
    }
    json!({"clips": out})
}

fn commit(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "midi.cleanup_commit";
    let ids = targets(e, p, true);
    if ids.is_empty() {
        return Err(bad(ID, "choose a MIDI clip with MIDI Cleanup"));
    }
    let s = e.session_mut();
    // What plays must be what the settings give now.
    crate::midi_cleanup::refresh(s);
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            let playing = c.midi_playing().cloned();
            if let (Some(seq), ClipContent::Midi { sequence }) = (playing, &mut c.content) {
                *sequence = seq;
            }
            c.midi_cleanup = None;
        }
    }
    Ok(json!({"clips": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "midi.cleanup_remove";
    let ids = targets(e, p, true);
    if ids.is_empty() {
        return Err(bad(ID, "choose a MIDI clip with MIDI Cleanup"));
    }
    let s = e.session_mut();
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            c.midi_cleanup = None;
        }
    }
    Ok(json!({"clips": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_midi::{Note, Sequence};
    use soundcraft_model::{ChannelFormat, ChordEvent, SourceId, TrackKind};
    use std::sync::Arc;

    fn note(pitch: u8, velocity: u8, start: i64, length: i64) -> Note {
        Note { pitch, velocity, release_velocity: 64, channel: 0, start, length }
    }

    /// An instrument track with one MIDI clip at 0 (a C triad and a short quiet C#), a C chord
    /// over it, key C major.
    fn setup() -> (Engine, ClipId) {
        let mut e = Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let id = s.new_clip_id();
        let seq =
            Sequence { notes: vec![note(60, 90, 0, 900), note(64, 90, 0, 900), note(67, 90, 0, 900), note(61, 20, 100, 60)], ctrls: Vec::new() };
        let len = s.sample_rate.samples(4.0);
        crate::edit::place_clip(s, t, soundcraft_model::Clip::midi(id, "take", 0, len, seq));
        s.band.key = 0;
        s.band.scale = 0;
        s.band.chords.push(ChordEvent { id: 1, tick: 0, length_ticks: 3840, slot: 0, bass: -1 });
        s.band.next_id = 2;
        (e, id)
    }

    fn clip(e: &Engine, id: ClipId) -> &soundcraft_model::Clip {
        e.session().find_clip(id).map(|(_, c)| c).unwrap()
    }

    fn notes(e: &Engine, id: ClipId) -> Vec<Note> {
        match &clip(e, id).content {
            ClipContent::Midi { sequence } => sequence.notes.clone(),
            _ => Vec::new(),
        }
    }

    fn preview(e: &Engine, id: ClipId) -> Arc<soundcraft_model::CleanupPreview> {
        clip(e, id).midi_cleanup.as_ref().and_then(|m| m.preview.clone()).unwrap()
    }

    #[test]
    fn cleanup_creates_defaults_and_previews() {
        let (mut e, id) = setup();
        let r = e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        assert_eq!(r["clips"][0]["settings"]["wrong"], 40);
        assert_eq!(r["clips"][0]["settings"]["held"], 70);
        assert_eq!(r["clips"][0]["settings"]["protect_melody"], true);
        assert_eq!(r["clips"][0]["report"]["removed"], 1);
        assert_eq!(r["clips"][0]["chords"], true);
        assert!(r["clips"][0]["summary"].as_str().unwrap().starts_with("Removed 1"));
        assert_eq!(clip(&e, id).midi_playing().unwrap().notes.len(), 3);
        assert_eq!(notes(&e, id).len(), 4, "the take is untouched");
    }

    #[test]
    fn a_call_that_changes_nothing_adds_no_undo_step() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        let steps = e.undo_history().len();
        let doc = e.session_arc();
        let r = e.execute("midi.cleanup", &json!({"clip": id.0, "wrong": 40, "original": false})).unwrap();
        assert_eq!(r["clips"][0]["report"]["removed"], 1);
        assert_eq!(e.undo_history().len(), steps);
        assert!(Arc::ptr_eq(&doc, &e.session_arc()), "the session was not cloned");
    }

    #[test]
    fn splitting_a_cleaned_clip_rederives_each_half() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        let t = e.session().tracks[0].id;
        let at = e.session().sample_rate.samples(2.0);
        e.execute("edit.separate", &json!({"tracks": [t.0], "at": at})).unwrap();
        let clips: Vec<_> = e.session().tracks[0].clips().to_vec();
        assert_eq!(clips.len(), 2);
        for c in &clips {
            let m = c.midi_cleanup.as_ref().expect("settings follow the split");
            let p = m.preview.as_ref().expect("each half has a preview");
            let ClipContent::Midi { sequence } = &c.content else { panic!("not MIDI") };
            assert_eq!(p.out.report.total, sequence.notes.len());
        }
        assert_ne!(
            clips[0].midi_cleanup.as_ref().unwrap().preview.as_ref().unwrap().inputs,
            clips[1].midi_cleanup.as_ref().unwrap().preview.as_ref().unwrap().inputs
        );
    }

    #[test]
    fn defaults_from_the_selected_clip() {
        let (mut e, id) = setup();
        e.session_mut().edit.selected_clips = vec![id];
        assert!(e.execute("midi.cleanup", &json!({})).is_ok());
    }

    #[test]
    fn slider_change_is_undoable_and_undo_restores_preview() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        let before = preview(&e, id);
        e.execute("midi.cleanup", &json!({"clip": id.0, "wrong": 0})).unwrap();
        assert_eq!(clip(&e, id).midi_cleanup.as_ref().unwrap().settings.wrong, 0);
        assert_eq!(clip(&e, id).midi_playing().unwrap().notes.len(), 4, "0 % removes nothing");
        assert!(e.undo());
        assert_eq!(clip(&e, id).midi_cleanup.as_ref().unwrap().settings.wrong, 40);
        assert_eq!(*preview(&e, id), *before);
        assert_eq!(clip(&e, id).midi_playing().unwrap().notes.len(), 3);
        assert!(e.redo());
        assert_eq!(clip(&e, id).midi_playing().unwrap().notes.len(), 4);
    }

    #[test]
    fn bad_amount_refused() {
        let (mut e, id) = setup();
        for v in [json!(101), json!(-1), json!("x")] {
            let err = e.execute("midi.cleanup", &json!({"clip": id.0, "wrong": v})).unwrap_err().to_string();
            assert!(err.contains("`wrong` must be a number from 0 to 100 (%)"), "{err}");
        }
        assert!(e.execute("midi.cleanup", &json!({"clip": id.0, "extend": 100.5})).is_err());
        assert!(clip(&e, id).midi_cleanup.is_none(), "a refused call changes nothing");
    }

    #[test]
    fn toggle_plays_original() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        let r = e.execute("midi.cleanup", &json!({"clip": id.0, "toggle": true})).unwrap();
        assert_eq!(r["clips"][0]["original"], true);
        assert_eq!(clip(&e, id).midi_playing().unwrap().notes.len(), 4);
        e.execute("midi.cleanup", &json!({"clip": id.0, "original": false})).unwrap();
        assert_eq!(clip(&e, id).midi_playing().unwrap().notes.len(), 3);
    }

    #[test]
    fn reset_restores_defaults() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0, "wrong": 90, "held": 5, "legato": 6, "extend": 7, "protect_melody": false})).unwrap();
        let r = e.execute("midi.cleanup", &json!({"clip": id.0, "reset": true})).unwrap();
        assert_eq!(r["clips"][0]["settings"], json!({"wrong": 40, "held": 70, "legato": 30, "extend": 40, "protect_melody": true}));
    }

    #[test]
    fn commit_replaces_notes_and_clears_settings() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        let playing = clip(&e, id).midi_playing().unwrap().clone();
        e.execute("midi.cleanup_commit", &json!({"clip": id.0})).unwrap();
        assert!(clip(&e, id).midi_cleanup.is_none());
        assert_eq!(notes(&e, id), playing.notes);
        assert!(e.undo());
        assert!(clip(&e, id).midi_cleanup.is_some());
        assert_eq!(notes(&e, id).len(), 4);
        // Switched to Original, Commit keeps the take.
        e.execute("midi.cleanup", &json!({"clip": id.0, "original": true})).unwrap();
        e.execute("midi.cleanup_commit", &json!({"clip": id.0})).unwrap();
        assert_eq!(notes(&e, id).len(), 4);
        assert!(e.execute("midi.cleanup_commit", &json!({"clip": id.0})).is_err());
    }

    #[test]
    fn remove_drops_settings() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        e.execute("midi.cleanup_remove", &json!({"clip": id.0})).unwrap();
        assert!(clip(&e, id).midi_cleanup.is_none());
        assert_eq!(clip(&e, id).midi_playing().unwrap().notes.len(), 4);
        assert!(e.undo());
        assert!(clip(&e, id).midi_cleanup.is_some());
    }

    #[test]
    fn chord_lane_edit_rederives() {
        let (mut e, id) = setup();
        // The quiet short C# is wrong over C; a C/C# slash chord makes it a chord tone.
        let r = e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        assert_eq!(r["clips"][0]["report"]["removed"], 1);
        let before = preview(&e, id);
        e.execute("band.chord.set", &json!({"chord": 1, "bass": 1})).unwrap();
        let after = preview(&e, id);
        assert_ne!(before.inputs, after.inputs);
        assert_eq!(after.out.report.removed, 0, "{:?}", after.out.report);
        assert_eq!(clip(&e, id).midi_playing().unwrap().notes.len(), 4);
    }

    #[test]
    fn detected_chords_rederive_through_undo_and_redo() {
        // No chord lane: Wrong notes has nothing to judge, so the quiet C# stays.
        let (mut e, id) = setup();
        e.session_mut().band.chords.clear();
        let r = e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        assert_eq!(r["clips"][0]["report"]["removed"], 0);
        // Detect Chords lands through the UI path (`ml::apply`, outside `execute`): C over the clip.
        let t = e.session().tracks[0].id;
        let len = e.session().sample_rate.samples(4.0);
        let job = crate::ml::MlJob {
            command: "ml.chords",
            kind: crate::ml::JobKind::Chords(soundcraft_ml::ChordOptions { bass: false }),
            parts: vec![crate::ml::Part { target: crate::ml::Target::Track { track: t, start: 0, len, name: "Keys".into() }, audio: Vec::new() }],
            rate: e.session().sample_rate.hz(),
            device: soundcraft_ml::Device::Cpu,
            models_dir: std::path::PathBuf::new(),
            quality: soundcraft_ml::Quality::Fast,
            keep: Vec::new(),
            mute_original: false,
            lane: crate::cmd::chord_lane::LaneOptions { replace: true, ..Default::default() },
        };
        let c = soundcraft_ml::ChordSegment {
            start: 0.0,
            end: 4.0,
            label: "C".into(),
            class: 0,
            name: "C".into(),
            root_pc: 0,
            quality: "maj".into(),
            bass_pc: 0,
        };
        let out = soundcraft_ml::JobOutput { stems: Vec::new(), notes: Vec::new(), midi: None, chords: Some(vec![c]), lyrics: None };
        crate::ml::apply(&mut e, &job, crate::ml::MlResult { parts: vec![out] }).unwrap();
        assert!(!e.session().band.chords.is_empty());
        assert_eq!(preview(&e, id).out.report.removed, 1, "the preview follows the detected chords");
        assert_eq!(clip(&e, id).midi_playing().unwrap().notes.len(), 3);
        assert!(e.undo());
        assert_eq!(preview(&e, id).out.report.removed, 0);
        assert!(e.redo());
        assert_eq!(preview(&e, id).out.report.removed, 1, "redo brings back the refreshed preview");
    }

    #[test]
    fn note_edit_rederives() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        let before = preview(&e, id);
        // Make the quiet C# a loud, long one: a different take.
        e.execute("midi.note_edit", &json!({"clip": id.0, "index": 3, "velocity": 120, "length_ticks": 600})).unwrap();
        let after = preview(&e, id);
        assert_ne!(before.inputs, after.inputs);
        assert_eq!(after.out.report.total, 4);
    }

    #[test]
    fn unrelated_command_keeps_preview() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        let old = preview(&e, id);
        let t = e.session().tracks[0].id;
        e.execute("track.rename", &json!({"track": t.0, "name": "Piano"})).unwrap();
        assert!(Arc::ptr_eq(&old, &preview(&e, id)));
    }

    #[test]
    fn tempo_change_rederives() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0})).unwrap();
        let before = preview(&e, id).inputs;
        e.session_mut().tempo.set_tempo(0, 90.0).unwrap();
        let t = e.session().tracks[0].id;
        e.execute("track.rename", &json!({"track": t.0, "name": "P"})).unwrap();
        assert_ne!(before, preview(&e, id).inputs);
    }

    #[test]
    fn cleanup_command_on_huge_clip_is_fast() {
        let (mut e, id) = setup();
        let s = e.session_mut();
        let mut seq = Sequence::default();
        for i in 0..12_000i64 {
            seq.notes.push(note(48 + (i % 36) as u8, 70 + (i % 40) as u8, i * 240, 200 + (i % 5) * 60));
        }
        let len = s.sample_rate.samples(2000.0);
        if let Some(c) = s.find_clip_mut(id) {
            c.content = ClipContent::Midi { sequence: seq };
            c.length = len;
        }
        s.band.chords.clear();
        for i in 0..300i64 {
            s.band.chords.push(ChordEvent { id: i as u32 + 1, tick: i * 9600, length_ticks: 9600, slot: ((i % 6) * 16) as u8, bass: -1 });
        }
        s.band.next_id = 400;
        let t = std::time::Instant::now();
        e.execute("midi.cleanup", &json!({"clip": id.0, "wrong": 55})).unwrap();
        let took = t.elapsed();
        assert!(took.as_millis() < 250, "took {took:?}");
        assert_eq!(preview(&e, id).out.report.total, 12_000);
    }

    #[test]
    fn cleanup_round_trips_and_rederives() {
        let (mut e, id) = setup();
        e.execute("midi.cleanup", &json!({"clip": id.0, "wrong": 55, "original": true})).unwrap();
        let before = preview(&e, id);
        let json = serde_json::to_string(e.session()).unwrap();
        assert!(!json.contains("\"preview\""), "the preview is derived, not saved");
        let mut s: soundcraft_model::Session = serde_json::from_str(&json).unwrap();
        s.sanitize();
        let mut e2 = Engine::default();
        e2.replace_session(s);
        let m = clip(&e2, id).midi_cleanup.clone().unwrap();
        assert_eq!(m, clip(&e, id).midi_cleanup.clone().unwrap());
        assert_eq!(*preview(&e2, id), *before);
    }

    #[test]
    fn refused_on_audio_clip() {
        let mut e = Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let id = s.new_clip_id();
        crate::edit::place_clip(s, t, soundcraft_model::Clip::audio(id, "a", SourceId(0), 0, 0, 1000));
        for c in ["midi.cleanup", "midi.cleanup_commit", "midi.cleanup_remove"] {
            let err = e.execute(c, &json!({"clip": id.0})).unwrap_err().to_string();
            assert!(err.contains("choose a MIDI clip"), "{c}: {err}");
        }
    }
}

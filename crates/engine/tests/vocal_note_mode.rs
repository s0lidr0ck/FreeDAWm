//! The per-clip note detection mode (`vocal.note_mode`): Musical for new tunings (cutting a loose
//! legato take into fewer notes), Classic for tunings saved before modes existed, edits carried
//! across a switch, undo, saving, and the render inputs following the mode.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::vocal;
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, TrackKind};
use soundcraft_vocal::NoteMode;
use std::f64::consts::TAU;

const RATE: u32 = 48_000;

/// A sung line: the fundamental follows `f0(t)` (Hz), harmonics shaped by three formants, at
/// about −21 dBFS.
fn sung(secs: f64, f0: impl Fn(f64) -> f64) -> Vec<f32> {
    const FORMANTS: [(f64, f64, f64); 3] = [(700.0, 100.0, 1.0), (1200.0, 110.0, 0.6), (2600.0, 160.0, 0.3)];
    let n = (secs * f64::from(RATE)) as usize;
    let mut phase = 0.0f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / f64::from(RATE);
        let f = f0(t);
        let mut v = 0.0;
        for k in 1..=40 {
            let fk = f * k as f64;
            if fk >= f64::from(RATE) / 2.0 {
                break;
            }
            let a: f64 = FORMANTS.iter().map(|(fc, bw, g)| g / (1.0 + ((fk - fc) / bw).powi(2))).sum::<f64>() + 0.01 / k as f64;
            v += a * (phase * k as f64).sin();
        }
        out.push(v);
        phase = (phase + TAU * f / f64::from(RATE)) % (TAU * 1e6);
    }
    let rms = (out.iter().map(|v| v * v).sum::<f64>() / n.max(1) as f64).sqrt();
    let g = 0.09 / rms.max(1e-12);
    out.into_iter().map(|v| (v * g) as f32).collect()
}

fn hz(midi_cents: f64) -> f64 {
    440.0 * 2f64.powf((midi_cents - 6900.0) / 1200.0)
}

/// A loose legato line: A3 sung 35 c flat (0.45 s), then a scoop that lingers a moment 200 c
/// under B3 before reaching it 30 c flat (0.5 s).
fn loose_take() -> Vec<f32> {
    sung(1.05, |t| {
        let c = if t < 0.45 {
            5665.0
        } else if t < 0.49 {
            5665.0 + (5900.0 - 5665.0) * (t - 0.45) / 0.04
        } else if t < 0.57 {
            5900.0 + 40.0 * (t - 0.49) / 0.08
        } else if t < 0.6 {
            5940.0 + (6070.0 - 5940.0) * (t - 0.57) / 0.03
        } else {
            6070.0
        };
        hz(c)
    })
}

fn session_of(take: Vec<f32>) -> (Engine, ClipId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let len = take.len();
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![take] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Vox", src, 0, 0, len as i64));
    let mut e = Engine::new(s);
    e.execute("vocal.analyse", &json!({"clip": id.0})).unwrap();
    (e, id)
}

fn clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).map(|(_, c)| c.clone()).unwrap()
}

fn notes(e: &mut Engine, c: ClipId) -> Vec<Value> {
    e.execute("vocal.notes", &json!({"clip": c.0})).unwrap()["notes"].as_array().cloned().unwrap()
}

#[test]
fn musical_is_the_default_and_classic_cuts_more_notes() {
    let (mut e, c) = session_of(loose_take());
    assert!(clip(&e, c).tune.is_none());
    assert_eq!(vocal::note_mode(&clip(&e, c)), NoteMode::Musical, "an untuned clip shows the notes a new tuning gets");
    let r = e.execute("vocal.notes", &json!({"clip": c.0})).unwrap();
    assert_eq!(r["clips"][0]["note_mode"], json!("musical"));
    let ns = notes(&mut e, c);
    assert_eq!(ns.len(), 2, "A3 and B3: {ns:?}");
    // Centred on the held parts: 35 c flat and 30 c flat.
    assert!((ns[0]["offset"].as_f64().unwrap() + 35.0).abs() < 8.0, "{}", ns[0]["offset"]);
    assert!((ns[1]["offset"].as_f64().unwrap() + 30.0).abs() < 8.0, "{}", ns[1]["offset"]);
    assert!(ns.iter().all(|n| n["scoop"].is_null()), "the scoop is part of B3");
    // The first edit makes a Musical tuning.
    e.execute("vocal.edit", &json!({"notes": [{"clip": c.0, "note": ns[0]["note"]}], "centre": 50})).unwrap();
    assert_eq!(clip(&e, c).tune.as_ref().map(|t| t.note_mode), Some(NoteMode::Musical));
    assert!(e.undo());

    let undo = e.undo_history().len();
    let r = e.execute("vocal.note_mode", &json!({"clip": c.0, "mode": "classic"})).unwrap();
    assert_eq!(r["clips"][0]["note_mode"], json!("classic"));
    assert_eq!(e.undo_history().len(), undo + 1, "one undo step");
    assert_eq!(clip(&e, c).tune.as_ref().map(|t| t.note_mode), Some(NoteMode::Classic), "a tuning holds the switch");
    let classic = notes(&mut e, c).len();
    assert!(classic >= 3, "Classic makes a note of the lingering scoop: {classic}");
    assert_eq!(r["clips"][0]["notes"], json!(classic));

    assert!(e.undo());
    assert_eq!(vocal::note_mode(&clip(&e, c)), NoteMode::Musical);
    assert!(clip(&e, c).tune.is_none(), "undo leaves the clip untuned, as it was");
    // Switching back to Musical with nothing else in the tuning drops it again.
    e.execute("vocal.note_mode", &json!({"clip": c.0, "mode": "classic"})).unwrap();
    e.execute("vocal.note_mode", &json!({"clip": c.0, "mode": "musical"})).unwrap();
    assert!(clip(&e, c).tune.is_none());
}

#[test]
fn edits_follow_the_notes_across_a_switch() {
    let (mut e, c) = session_of(loose_take());
    let ns = notes(&mut e, c);
    let all: Vec<Value> = ns.iter().map(|n| json!({"clip": c.0, "note": n["note"]})).collect();
    e.execute("vocal.edit", &json!({"notes": all, "centre": 100})).unwrap();
    for n in &notes(&mut e, c) {
        assert!(n["tuned_offset"].as_f64().unwrap().abs() < 1.0, "tuned onto its semitone: {n}");
    }
    let musical_inputs = vocal::render_inputs(e.session(), &clip(&e, c)).unwrap();
    vocal::run_until_idle(&mut e, &[c]).unwrap();
    assert_eq!(e.session().vocal.renders.get(&c).map(|r| r.inputs), Some(musical_inputs), "rendered in Musical mode");
    // A switch with no mode (the menu's toggle): every target is Musical, so Classic.
    e.execute("vocal.note_mode", &json!({"clip": c.0})).unwrap();
    assert_eq!(vocal::note_mode(&clip(&e, c)), NoteMode::Classic);
    let ns = notes(&mut e, c);
    assert!(ns.len() >= 3);
    assert!(ns.iter().all(|n| n["edit"]["centre"] == json!(1.0)), "the edits came along: {ns:?}");
    let classic_inputs = vocal::render_inputs(e.session(), &clip(&e, c)).unwrap();
    assert_ne!(musical_inputs, classic_inputs, "a new mode is a new render");
    // And back.
    e.execute("vocal.note_mode", &json!({"clip": c.0})).unwrap();
    assert_eq!(vocal::note_mode(&clip(&e, c)), NoteMode::Musical);
    let ns = notes(&mut e, c);
    assert_eq!(ns.len(), 2);
    assert!(ns.iter().all(|n| n["edit"]["centre"] == json!(1.0)));
}

#[test]
fn the_mode_is_saved_and_old_sessions_stay_classic() {
    let (mut e, c) = session_of(loose_take());
    let ns = notes(&mut e, c);
    e.execute("vocal.edit", &json!({"notes": [{"clip": c.0, "note": ns[0]["note"]}], "centre": 50})).unwrap();
    let v = serde_json::to_value(clip(&e, c)).unwrap();
    assert_eq!(v["tune"]["note_mode"], json!("Musical"), "a new tuning saves its mode");
    let back: Clip = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(vocal::note_mode(&back), NoteMode::Musical);
    // A tuning saved before the mode existed (no key) is Classic: it sounds as it did.
    let mut old = v;
    old["tune"].as_object_mut().unwrap().remove("note_mode");
    let back: Clip = serde_json::from_value(old).unwrap();
    assert_eq!(vocal::note_mode(&back), NoteMode::Classic);
    // Classic writes no key, so it reads back as Classic.
    e.execute("vocal.note_mode", &json!({"clip": c.0, "mode": "classic"})).unwrap();
    let v = serde_json::to_value(clip(&e, c)).unwrap();
    assert!(v["tune"].get("note_mode").is_none(), "Classic is not written: {v}");
    let back: Clip = serde_json::from_value(v).unwrap();
    assert_eq!(vocal::note_mode(&back), NoteMode::Classic);
}

#[test]
fn bad_modes_are_refused() {
    let (mut e, c) = session_of(loose_take());
    for bad in [json!("melodyne"), json!(1), json!(null)] {
        let err = e.execute("vocal.note_mode", &json!({"clip": c.0, "mode": bad})).unwrap_err().to_string();
        assert!(err.contains("classic or musical"), "{err}");
    }
    assert!(e.execute("vocal.note_mode", &json!({"clip": 999, "mode": "musical"})).is_err());
    assert!(clip(&e, c).tune.is_none());
}

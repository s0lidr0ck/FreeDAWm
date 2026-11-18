//! AI › Tune Vocal › Fit to Lead, clicked in the menu on a tuned backing vocal under a lead. The
//! menu audit ran it on a drum track, where it changed nothing and said nothing, so it could not
//! tell whether it worked: here it moves the clashing note (one undo step) and, when there is
//! nothing to move, says why in the status line.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, TrackId, TrackKind};
use soundcraft_ui_egui::{Services, SoundApp, menus};
use std::f64::consts::TAU;

const RATE: u32 = 48_000;
const PATH: &str = "AI > Tune Vocal > Fit to Lead";
const ID: &str = "vocal.fit_to_lead";

/// A sung vowel (harmonics through three formants) at `f0` for `secs`.
fn vowel(f0: f64, secs: f64) -> Vec<f32> {
    let n = (secs * f64::from(RATE)) as usize;
    let formants = [(700.0, 100.0, 1.0), (1200.0, 110.0, 0.6), (2600.0, 160.0, 0.3)];
    let ks: Vec<(f64, f64)> = (1..=40)
        .map(|k| f64::from(k) * f0)
        .filter(|f| *f < f64::from(RATE) / 2.0)
        .map(|f| (f, formants.iter().map(|(c, b, a)| a / (1.0 + ((f - c) / b).powi(2))).sum::<f64>() + 0.01))
        .collect();
    let peak = 0.3 / ks.iter().map(|(_, a)| a).sum::<f64>().max(1e-9);
    (0..n).map(|i| (ks.iter().map(|(f, a)| a * (TAU * f * i as f64 / f64::from(RATE)).sin()).sum::<f64>() * peak) as f32).collect()
}

/// Notes (MIDI note, seconds) with 0.15 s of silence after each.
fn line(notes: &[(f64, f64)]) -> Vec<f32> {
    let mut x = Vec::new();
    for (m, d) in notes {
        x.extend(vowel(440.0 * 2f64.powf((m - 69.0) / 12.0), *d));
        x.extend(std::iter::repeat_n(0.0, (0.15 * f64::from(RATE)) as usize));
    }
    x
}

fn add(s: &mut Session, name: &str, take: Vec<f32>) -> (TrackId, ClipId) {
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
    let len = take.len() as i64;
    let src = soundcraft_engine::io::add_source(s, name, AudioBuffer { sample_rate: RATE, channels: vec![take] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(s, t, Clip::audio(id, name, src, 0, 0, len));
    (t, id)
}

#[test]
fn fit_to_lead_from_the_menu_moves_the_clashing_note_or_says_why_not() {
    let mut s = Session::default();
    s.band.key = 4; // E major
    // C#4 under E4 is fine; G4 under G#4 is a minor 2nd (and out of E major).
    let (_, lc) = add(&mut s, "Lead Vox", line(&[(64.0, 0.5), (68.0, 0.5)]));
    let (bv, bc) = add(&mut s, "BV 1", line(&[(61.0, 0.5), (67.0, 0.5)]));
    // A vocal never analysed: no tuned notes to fit.
    let (raw, _) = add(&mut s, "BV 2", line(&[(61.0, 0.5)]));
    let mut e = soundcraft_engine::Engine::new(s);
    soundcraft_engine::vocal::run_until_idle(&mut e, &[lc, bc]).unwrap();
    let mut app = SoundApp::new(e, None, Services::default());
    let notes = |app: &SoundApp| soundcraft_engine::vocal_studio::studio_notes(app.engine.session(), bv).iter().map(|n| n.midi()).collect::<Vec<_>>();
    assert_eq!(notes(&app), [61, 67]);

    app.run("edit.select", json!({"tracks": [bv.0]})).unwrap();
    assert_eq!(menus::availability(&app, ID), Ok(()));
    assert!(menus::item_state(&app, PATH, Some(ID)).0);
    let steps = app.engine.undo_history().len();
    menus::invoke_menu(&mut app, ID, PATH);
    // No chords: the nearest tone in the key that clashes with nothing, F#4 (down wins a tie).
    assert_eq!(notes(&app), [61, 66], "{}", app.ui.status);
    assert_eq!(app.engine.undo_history().len(), steps + 1, "one undo step");

    // Again: every note fits now, and the status line says so instead of nothing.
    menus::invoke_menu(&mut app, ID, PATH);
    assert_eq!(notes(&app), [61, 66]);
    assert!(app.ui.status.contains("fits Lead Vox already"), "{}", app.ui.status);

    // A vocal with no tuned notes: told to analyse it first.
    app.run("edit.select", json!({"tracks": [raw.0]})).unwrap();
    menus::invoke_menu(&mut app, ID, PATH);
    assert!(app.ui.status.contains("no tuned notes"), "{}", app.ui.status);
}

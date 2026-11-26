//! The keyboard's sound: a built-in synth played live from the keyboard, its preset, volume,
//! octave and knobs heard; kept with each idea and layer played on keys, rendered to a WAV, and
//! opened in FreeDAWm with the same instrument and settings.

mod common;

use common::{BLOCK, RATE, offline, temp_dir};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_recorder::files;
use soundcraft_recorder::keys::KeySound;
use soundcraft_recorder::layers::{self, Sidecar};
use soundcraft_recorder::rig::{Click, InputChoice};
use soundcraft_recorder::settings::Prefs;
use soundcraft_recorder::ui::{RecorderApp, Screen, daw_args, look};
use std::time::{Duration, Instant};

const MONO: InputChoice = InputChoice { on: true, channel: 0, stereo: false };

fn init() -> KeySound {
    KeySound::from_preset("subtractive_synth", "Init").unwrap()
}

/// Plays note `note` for `secs` on a rig with `sound`, the input silent: the output's left side.
fn play_note(sound: Option<KeySound>, note: u8, secs: f64) -> Vec<f32> {
    let mut o = offline(1, &["Keys"], MONO);
    o.rig.set_keys(sound);
    o.run(0.05, &|_, _| 0.0);
    o.keys.send("Keys", &[0x90, note, 100]);
    let mut out = Vec::new();
    let blocks = (secs * f64::from(RATE) / BLOCK as f64) as usize;
    for k in 0..blocks {
        out.extend(o.block(&|_, _| 0.0).chunks(2).map(|f| f[0]));
        if k % 4 == 0 {
            o.rig.step();
        }
    }
    o.keys.send("Keys", &[0x80, note, 0]);
    out
}

/// The pitch of `x` over its second half (Hz): the period (60..2000 Hz) at which it repeats best
/// (normalised autocorrelation; the first lag within 90% of the best, so not a multiple).
fn pitch(x: &[f32]) -> f64 {
    let x = &x[x.len() / 2..];
    let n = x.len().min(8192);
    let x = &x[..n];
    let lags = (RATE as usize / 2000)..(RATE as usize / 60).min(n / 2);
    let score: Vec<(usize, f64)> = lags
        .map(|l| {
            let (mut num, mut a, mut b) = (0f64, 0f64, 0f64);
            for i in 0..n - l {
                let (p, q) = (f64::from(x[i]), f64::from(x[i + l]));
                num += p * q;
                a += p * p;
                b += q * q;
            }
            (l, num / (a * b).sqrt().max(1e-12))
        })
        .collect();
    let best = score.iter().map(|s| s.1).fold(f64::MIN, f64::max);
    // The first peak near the best.
    let lag = score.windows(3).find(|w| w[1].1 >= 0.9 * best && w[1].1 >= w[0].1 && w[1].1 >= w[2].1).map_or(1, |w| w[1].0);
    f64::from(RATE) / lag as f64
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

#[test]
fn a_key_sounds_through_the_chosen_instrument_at_its_pitch() {
    let a = play_note(Some(init()), 69, 0.5);
    assert!(rms(&a) > 1e-3, "heard: {}", rms(&a));
    let f = pitch(&a);
    assert!((400.0..480.0).contains(&f), "A above middle C sounds at 440 Hz: {f}");
    // An octave up.
    let up = play_note(Some(KeySound { octave: 1, ..init() }), 69, 0.5);
    let f = pitch(&up);
    assert!((820.0..960.0).contains(&f), "an octave up: {f}");
    // Off: silent, as before.
    let off = play_note(None, 69, 0.3);
    assert!(off.iter().all(|x| *x == 0.0), "off is silent");
}

#[test]
fn volume_preset_and_knobs_are_heard() {
    let loud = rms(&play_note(Some(init()), 60, 0.4));
    let soft = rms(&play_note(Some(KeySound { volume_db: -12.0, ..init() }), 60, 0.4));
    let ratio = soft / loud;
    assert!((0.18..0.33).contains(&ratio), "−12 dB is a quarter as loud: {ratio}");
    let other = play_note(KeySound::from_preset("subtractive_synth", "Sync Lead"), 60, 0.4);
    let base = play_note(Some(init()), 60, 0.4);
    assert!(other.iter().zip(&base).any(|(a, b)| (a - b).abs() > 1e-3), "another preset sounds otherwise");
    let pad = KeySound::from_preset("subtractive_synth", "Breathing Pad").unwrap();
    let mut open = pad.clone();
    open.set_param("macro1", 100.0);
    let (a, b) = (play_note(Some(pad), 60, 0.6), play_note(Some(open), 60, 0.6));
    assert!(a.iter().zip(&b).any(|(x, y)| (x - y).abs() > 1e-3), "its knob is heard");
}

#[test]
fn the_choice_is_remembered() {
    let dir = temp_dir("keys-prefs");
    let path = dir.join("recorder.json");
    let p =
        Prefs { keys: KeySound::from_preset("fm_synth", "Tine Piano").map(|k| KeySound { octave: -1, volume_db: -3.0, ..k }), ..Prefs::default() };
    p.save(&path).unwrap();
    assert_eq!(Prefs::load(&path).keys, p.keys);
    assert_eq!(Prefs::default().key_sound().map(|k| k.preset), Some("Tine Piano".into()), "a piano to start with");
    assert_eq!(Prefs { keys_on: false, ..Prefs::default() }.key_sound(), None);
    let _ = std::fs::remove_dir_all(dir);
}

fn app(dir: &std::path::Path, f: impl FnOnce(&mut Prefs)) -> (RecorderApp, common::Offline) {
    let mut o = offline(1, &["Keys"], MONO);
    let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
    let mut prefs = Prefs { ideas_folder: dir.to_string_lossy().into_owned(), keys: Some(init()), ..Prefs::default() };
    f(&mut prefs);
    (RecorderApp::with_rig(rig, prefs, None), o)
}

fn harness(a: RecorderApp, height: f32) -> Harness<'static, RecorderApp> {
    let h = Harness::builder().with_size(egui::vec2(look::WIDTH, height)).build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), a);
    look::apply(&h.ctx);
    h
}

fn play(h: &mut Harness<'_, RecorderApp>, o: &mut common::Offline, secs: f64) {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        for _ in 0..4 {
            o.block(&|_, _| 0.0);
        }
        h.step();
    }
}

/// An idea played on the keys (a note at 0.2 s for 0.3 s), on the saved screen.
fn keys_idea(h: &mut Harness<'_, RecorderApp>, o: &mut common::Offline, lead: f64) {
    h.run_steps(2);
    h.state_mut().toggle_record();
    play(h, o, 0.2 + lead);
    o.keys.send("Keys", &[0x90, 69, 100]);
    play(h, o, 0.3);
    o.keys.send("Keys", &[0x80, 69, 0]);
    play(h, o, 0.3);
    h.state_mut().toggle_record();
    h.run_steps(3);
    assert_eq!(h.state().screen, Screen::Idea);
}

#[test]
fn an_idea_on_keys_keeps_its_sound_and_what_it_sounded_like() {
    let dir = temp_dir("keys-idea");
    let (a, mut o) = app(&dir, |_| {});
    let mut h = harness(a, look::HEIGHT);
    keys_idea(&mut h, &mut o, 0.0);
    let take = files::recent(&dir, 10).remove(0);
    assert!(take.mid.is_some());
    let side = Sidecar::of(&take);
    assert_eq!(side.keys.get(&1), Some(&init()), "the sound is kept beside the idea");
    // The keys as heard: their own WAV, the idea's length, the note at its pitch.
    let keys = take.keys.clone().expect("(keys).wav");
    assert!(keys.to_string_lossy().ends_with(" (keys).wav"));
    let (_, buf) = soundcraft_audio_io::decode(&std::fs::read(&keys).unwrap(), Some("wav")).unwrap();
    assert_eq!(buf.channels[0].len() as f64, (take.seconds.unwrap() * f64::from(RATE)).round(), "as long as the idea");
    let note: Vec<f32> = buf.channels[0].iter().skip(RATE as usize / 4).take(RATE as usize / 5).copied().collect();
    assert!((400.0..480.0).contains(&pitch(&note)), "{}", pitch(&note));
    // The microphone's WAV has none of it.
    let (_, mic) = soundcraft_audio_io::decode(&std::fs::read(take.wav.as_ref().unwrap()).unwrap(), Some("wav")).unwrap();
    assert!(mic.channels[0].iter().all(|x| *x == 0.0));
    // Played back: the keys as heard; without that WAV, the notes through the sound, the same.
    let with = layers::mix(&take, &side, RATE).unwrap();
    std::fs::remove_file(&keys).unwrap();
    let take2 = files::recent(&dir, 10).remove(0);
    let without = layers::mix(&take2, &side, RATE).unwrap();
    let diff = with.channels[0].iter().zip(&without.channels[0]).fold(0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(rms(&with.channels[0]) > 1e-3 && diff < 1e-3, "playback matches: {diff}");
    // Open in FreeDAWm: the MIDI file, not the render too.
    let args: Vec<String> = daw_args(&take).iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert!(!args.iter().any(|a| a.contains("(keys)")), "{args:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_keyboard_layer_has_its_own_sound() {
    let dir = temp_dir("keys-layer");
    let (a, mut o) = app(&dir, |p| p.count_in = false);
    let mut h = harness(a, look::HEIGHT);
    keys_idea(&mut h, &mut o, 0.0);
    // Another sound for the layer.
    let bass = KeySound::from_preset("fm_synth", "Tine Piano").unwrap();
    h.state_mut().prefs.keys = Some(bass.clone());
    h.run_steps(2);
    h.state_mut().add_layer();
    h.run_steps(2);
    assert!(h.state().recording());
    play(&mut h, &mut o, 0.1);
    o.keys.send("Keys", &[0x90, 48, 100]);
    play(&mut h, &mut o, 0.2);
    o.keys.send("Keys", &[0x80, 48, 0]);
    play(&mut h, &mut o, 0.1);
    h.state_mut().toggle_record();
    h.run_steps(3);
    let take = files::recent(&dir, 10).remove(0);
    assert_eq!(take.layer_count(), 2, "{take:?}");
    let side = Sidecar::of(&take);
    assert_eq!(side.keys.get(&1).map(|k| k.plugin.as_str()), Some("subtractive_synth"));
    assert_eq!(side.keys.get(&2), Some(&bass), "the layer keeps the sound it was played with");
    assert!(take.layers[0].keys.is_some(), "and its own (keys) WAV");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn freedawm_opens_a_keys_idea_with_its_instrument_and_an_idea_with_a_tempo_at_it() {
    let dir = temp_dir("keys-daw");
    let mut sound = KeySound::from_preset("wavetable_synth", "Init").unwrap_or_else(init);
    sound.volume_db = -6.0;
    let first = sound.params.keys().next().cloned().unwrap();
    sound.set_param(&first, 0.25);
    let (a, mut o) = app(&dir, |p| (p.keys, p.click_on, p.click_bpm, p.click_beats) = (Some(sound.clone()), true, 300.0, 3));
    let mut h = harness(a, look::HEIGHT);
    keys_idea(&mut h, &mut o, 0.6);
    let take = files::recent(&dir, 10).remove(0);
    // What FreeDAWm does with the files it is given.
    let mut e = Engine::default();
    for p in daw_args(&take) {
        let p = p.to_string_lossy().into_owned();
        let id = if p.ends_with(".mid") { "file.import_midi" } else { "file.import_audio" };
        e.execute(id, &json!({"path": p, "at": 0})).unwrap();
    }
    let s = e.session();
    let keys = s.tracks.iter().find(|t| t.instrument.is_some()).expect("an instrument track");
    let ins = keys.instrument.as_ref().unwrap();
    assert_eq!(ins.plugin, sound.plugin);
    assert_eq!(ins.preset, sound.preset);
    assert_eq!(ins.params.get(&first).copied(), sound.params.get(&first).copied(), "its settings");
    assert!((keys.mixer.volume_db - (-6.0)).abs() < 0.01, "its volume: {}", keys.mixer.volume_db);
    assert_eq!(s.tempo.tempo_at_tick(0).round(), 300.0);
    // A WAV alone with a tempo beside it, into an empty session: that tempo.
    let only = temp_dir("keys-daw-wav");
    std::fs::copy(take.wav.as_ref().unwrap(), only.join("Song.wav")).unwrap();
    Sidecar { tempo: Some(Click { bpm: 96.0, beats: 3, volume_db: -6.0 }), ..Sidecar::default() }.save(&only.join("Song.fdidea")).unwrap();
    let mut e = Engine::default();
    e.execute("file.import_audio", &json!({"path": only.join("Song.wav").to_string_lossy(), "at": 0})).unwrap();
    assert_eq!(e.session().tempo.tempo_at_tick(0).round(), 96.0);
    assert_eq!(e.session().tempo.meter_at_tick(0).numerator, 3);
    // Not over a session that has something in it.
    let before = e.session().tempo.tempo_at_tick(0);
    Sidecar { tempo: Some(Click { bpm: 150.0, beats: 4, volume_db: -6.0 }), ..Sidecar::default() }.save(&only.join("Song.fdidea")).unwrap();
    e.execute("file.import_audio", &json!({"path": only.join("Song.wav").to_string_lossy(), "at": 0})).unwrap();
    assert_eq!(e.session().tempo.tempo_at_tick(0), before);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(only);
}

#[test]
fn the_keyboard_sound_screen_changes_the_sound() {
    let dir = temp_dir("keys-screen");
    let (a, _o) = app(&dir, |_| {});
    let mut h = harness(a, 1600.0);
    h.run_steps(3);
    assert!(h.query_by_label_contains("Keyboard sound: Init").is_some(), "on the Recording from card");
    h.get_by_label("Keyboard sound").click();
    h.run_steps(2);
    assert_eq!(h.state().screen, Screen::Keys);
    assert!(h.query_by_label("Play the keyboard through a sound").is_some());
    let before = h.state().prefs.keys.clone().unwrap();
    h.get_by_label("Next sound").click();
    h.run_steps(2);
    let after = h.state().prefs.keys.clone().unwrap();
    assert_ne!((after.plugin.clone(), after.preset.clone()), (before.plugin.clone(), before.preset.clone()));
    assert_eq!(h.state().rig.as_ref().unwrap().keys(), Some(&after), "the rig plays it");
    h.get_by_label("Octave up").click();
    h.run_steps(2);
    assert_eq!(h.state().prefs.keys.as_ref().unwrap().octave, 1);
    assert!(h.query_by_label("Keyboard volume").is_some());
    assert!(h.query_by_label_contains("ms after you press it").is_some() || h.query_by_label_contains("smaller buffer").is_some());
    h.get_by_label("More settings").click();
    h.run_steps(2);
    assert!(h.state().keys_more);
    h.get_by_label("Play the keyboard through a sound").click();
    h.run_steps(2);
    assert_eq!(h.state().rig.as_ref().unwrap().keys(), None, "off: silent");
    let _ = std::fs::remove_dir_all(dir);
}

//! The click (metronome): heard at its tempo and beats, a bar of it before a take, never in any
//! file; Tap; the MIDI file at its tempo; a new idea made whole bars.

mod common;

use common::{BLOCK, RATE, offline, temp_dir};
use egui_kittest::{Harness, kittest::Queryable};
use soundcraft_recorder::files;
use soundcraft_recorder::layers::Sidecar;
use soundcraft_recorder::rig::{Click, InputChoice};
use soundcraft_recorder::settings::Prefs;
use soundcraft_recorder::ui::{RecorderApp, Screen, look};
use std::time::{Duration, Instant};

const MONO: InputChoice = InputChoice { on: true, channel: 0, stereo: false };

/// Where each click starts in `out` (a sound after at least 0.1 s of silence).
fn onsets(out: &[f32]) -> Vec<usize> {
    let mut v = Vec::new();
    let mut quiet = usize::MAX;
    for (i, x) in out.iter().enumerate() {
        if x.abs() > 1e-3 {
            if quiet >= 4800 {
                v.push(i);
            }
            quiet = 0;
        } else {
            quiet = quiet.saturating_add(1);
        }
    }
    v
}

#[test]
fn the_click_plays_at_its_tempo_after_a_bar_and_is_never_recorded() {
    let dir = temp_dir("click");
    let mut o = offline(1, &[], MONO);
    // 150 bpm: a beat every 19 200 frames; 3 beats a bar.
    o.rig.set_click(Some(Click { bpm: 150.0, beats: 3, volume_db: -6.0 }));
    o.rig.start(&dir, "Click take", false, 0).unwrap();
    let mut out = Vec::new();
    let mut said = None;
    let blocks = (3.0 * f64::from(RATE) / BLOCK as f64) as usize;
    for k in 0..blocks {
        let b = o.block(&|_, _| 0.0);
        out.extend(b.chunks(2).map(|f| f[0]));
        if k == 2 {
            said = o.rig.counting_off();
        }
        if k % 4 == 0 {
            assert!(o.rig.step().is_none());
        }
    }
    assert_eq!(said, Some((1, 3)), "the bar before: beat 1 of 3");
    let saved = o.rig.stop();
    let at = onsets(&out);
    assert!(at.len() >= 8, "{at:?}");
    for w in at.windows(2) {
        assert!(w[1].abs_diff(w[0] + 19_200) <= 2, "a click every beat: {at:?}");
    }
    // The take started after the bar (3 beats, 1.2 s): 3 s run, 1.8 s recorded.
    let wav = saved.wav.unwrap();
    let (info, buf) = soundcraft_audio_io::decode(&std::fs::read(&wav).unwrap(), Some("wav")).unwrap();
    let secs = info.frames as f64 / f64::from(RATE);
    assert!((1.7..1.85).contains(&secs), "the take starts after the count-in: {secs} s");
    assert!(buf.channels[0].iter().all(|x| *x == 0.0), "the click is not in the recording");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_midi_file_is_at_the_clicks_tempo_and_meter() {
    let dir = temp_dir("click-midi");
    let mut o = offline(0, &["Keys"], InputChoice { on: false, ..MONO });
    o.rig.set_click(Some(Click { bpm: 300.0, beats: 3, volume_db: -6.0 }));
    o.rig.start(&dir, "m", false, 0).unwrap();
    o.run(0.7, &|_, _| 0.0);
    o.keys.send("Keys", &[0x90, 60, 100]);
    o.run(0.2, &|_, _| 0.0);
    o.keys.send("Keys", &[0x80, 60, 0]);
    o.run(0.1, &|_, _| 0.0);
    let saved = o.rig.stop();
    let mid = saved.mid.unwrap();
    let smf = soundcraft_midi::read_smf(&std::fs::read(&mid).unwrap()).unwrap();
    assert_eq!(smf.tempos.first().map(|t| t.1.round()), Some(300.0));
    assert_eq!(smf.meters.first().map(|m| (m.1, m.2)), Some((3, 4)));
    // Opened in FreeDAWm (as its command line imports it): the session takes that tempo and meter.
    let mut e = soundcraft_engine::Engine::default();
    e.execute("file.import_midi", &serde_json::json!({"path": mid.to_string_lossy(), "at": 0})).unwrap();
    let s = e.session();
    assert_eq!(s.tempo.tempo_at_tick(0).round(), 300.0);
    assert_eq!(s.tempo.meter_at_tick(0).numerator, 3);
    let _ = std::fs::remove_dir_all(dir);
}

fn app(dir: &std::path::Path, f: impl FnOnce(&mut Prefs)) -> (RecorderApp, common::Offline) {
    let mut o = offline(1, &[], MONO);
    let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
    let mut prefs = Prefs { ideas_folder: dir.to_string_lossy().into_owned(), ..Prefs::default() };
    f(&mut prefs);
    (RecorderApp::with_rig(rig, prefs, None), o)
}

fn play(h: &mut Harness<'_, RecorderApp>, o: &mut common::Offline, secs: f64) {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        for _ in 0..4 {
            o.block(&|i, _| 0.2 * ((i as f32) * 0.01).sin());
        }
        h.step();
    }
}

#[test]
fn tap_sets_the_tempo() {
    let dir = temp_dir("click-tap");
    let (mut a, _o) = app(&dir, |p| p.click_on = true);
    for t in [10.0, 10.5, 11.0, 11.5] {
        a.tap(t);
    }
    assert_eq!(a.prefs.click_bpm, 120.0);
    // A pause starts again: two taps 0.4 s apart.
    a.tap(20.0);
    a.tap(20.4);
    assert_eq!(a.prefs.click_bpm, 150.0);
    for t in [30.0, 30.01] {
        a.tap(t);
    }
    assert_eq!(a.prefs.click_bpm, 300.0, "kept in range");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_new_idea_to_the_click_is_whole_bars_and_knows_its_tempo() {
    let dir = temp_dir("click-bars");
    // 240 bpm, 4 beats: a bar is a second.
    let (a, mut o) = app(&dir, |p| (p.click_on, p.click_bpm, p.click_beats) = (true, 240.0, 4));
    let mut h = Harness::builder().with_size(egui::vec2(look::WIDTH, look::HEIGHT)).build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), a);
    look::apply(&h.ctx);
    h.run_steps(2);
    h.state_mut().toggle_record();
    play(&mut h, &mut o, 0.5);
    assert!(h.query_by_label_contains("Count-in:").is_some(), "the bar before, on the screen");
    play(&mut h, &mut o, 0.5 + 1.2);
    h.state_mut().toggle_record();
    h.run_steps(3);
    assert_eq!(h.state().screen, Screen::Idea);
    assert!(h.query_by_label_contains("Looped as 1 bar").is_some());
    let take = files::recent(&dir, 10).remove(0);
    let (info, _) = soundcraft_audio_io::decode(&std::fs::read(take.wav.as_ref().unwrap()).unwrap(), Some("wav")).unwrap();
    assert_eq!(info.frames, u64::from(RATE), "exactly one bar");
    assert_eq!(Sidecar::of(&take).tempo.map(|t| (t.bpm, t.beats)), Some((240.0, 4)));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_settings_show_the_click_when_it_is_on() {
    let dir = temp_dir("click-settings");
    let (a, _o) = app(&dir, |_| {});
    let mut h = Harness::builder().with_size(egui::vec2(look::WIDTH, 2000.0)).build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), a);
    look::apply(&h.ctx);
    h.run_steps(2);
    h.state_mut().go(Screen::Settings);
    h.run_steps(2);
    assert!(h.query_by_label("Tap the tempo").is_none(), "off: nothing more");
    h.get_by_label("Play a click while recording").click();
    h.run_steps(2);
    assert!(h.state().prefs.click_on);
    h.get_by_label("Faster").click();
    h.run_steps(2);
    assert_eq!(h.state().prefs.click_bpm, 121.0);
    h.get_by_label("3 beats in a bar").click();
    h.run_steps(2);
    assert_eq!(h.state().prefs.click_beats, 3);
    assert!(h.query_by_label("Tap the tempo").is_some() && h.query_by_label("Click volume").is_some());
    assert_eq!(h.state().rig.as_ref().unwrap().click().map(|c| (c.bpm, c.beats)), Some((121.0, 3)), "the rig clicks as set");
    let _ = std::fs::remove_dir_all(dir);
}

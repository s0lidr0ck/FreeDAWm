//! Opening a project shows progress instead of freezing (spec 2026-10-28-open-progress): the
//! modal appears on the first frame, the phases advance in order with counts, the hosted plugin
//! instances are made on the UI thread across frames while the old session keeps playing, and
//! the new session is live after the swap. Cancel during Plugins leaves the old session as it was.
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_engine::open_job::OpenPhase;
use soundcraft_playback::Player;
use soundcraft_ui_egui::open_progress::OPENING;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Fake hosted plugins: `(id, thread made on)` and `(id, thread dropped on)`.
static MADE_ON: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static DROPPED_ON: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// How long one fake takes to create: longer than the per-frame budget, so one is made per frame.
const FAKE_LOAD: std::time::Duration = std::time::Duration::from_millis(20);

fn thread_name() -> String {
    std::thread::current().name().unwrap_or("?").to_string()
}

/// A "hosted" plugin that takes a while to make and records the threads it meets.
struct Fake {
    id: String,
    inner: Box<dyn soundcraft_dsp::Plugin>,
}

impl soundcraft_dsp::Plugin for Fake {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        DROPPED_ON.lock().unwrap().push((self.id.clone(), thread_name()));
    }
}

fn factory(id: &str) -> Option<Box<dyn soundcraft_dsp::Plugin>> {
    if !id.starts_with("clap:test.open.") {
        return None;
    }
    std::thread::sleep(FAKE_LOAD);
    MADE_ON.lock().unwrap().push((id.to_string(), thread_name()));
    Some(Box::new(Fake { id: id.to_string(), inner: soundcraft_dsp::create("gain")? }))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-ui-open-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The demo session with `n` fake hosted plugins (on its first three audio tracks, round robin),
/// saved under `tag` in `dir` (its audio files written beside it). Returns the path and the
/// number of audio files.
fn session_with_fakes(dir: &std::path::Path, tag: &str, n: usize) -> (String, usize) {
    let mut e = soundcraft_engine::demo::demo_engine();
    let tracks: Vec<soundcraft_model::TrackId> =
        e.session().tracks.iter().filter(|t| t.kind == soundcraft_model::TrackKind::Audio).map(|t| t.id).take(3).collect();
    assert_eq!(tracks.len(), 3);
    for i in 0..n {
        let t = tracks[i % tracks.len()];
        // (straight into the document: the fakes are not in any plugin catalog)
        e.session_mut().track_mut(t).unwrap().mixer.add_module(soundcraft_model::Insert::new(format!("clap:test.open.{tag}.{i}")), None).unwrap();
    }
    let path = dir.join(format!("{tag}.fdawm")).to_string_lossy().into_owned();
    e.execute("session.save_as", &json!({"path": path})).unwrap();
    // No waveform cache: the open builds them, so its Waveforms phase is seen.
    let _ = std::fs::remove_dir_all(dir.join("Waveforms"));
    let files = e.session().sources.len();
    assert!(files >= 4, "{files} audio files in the demo");
    (path, files)
}

/// The app over a player whose "audio thread" is a thread of ours, filling until stopped and
/// noting the session it renders (its `Arc` address) and its hosted instance count.
struct Rig {
    h: Harness<'static, SoundApp>,
    old: Arc<soundcraft_model::Session>,
    playing: Arc<AtomicUsize>,
    hosted: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    audio: Option<std::thread::JoinHandle<()>>,
}

impl Rig {
    fn new() -> Rig {
        soundcraft_mix::set_test_plugin_factory(Some(factory));
        let engine = soundcraft_engine::demo::demo_engine();
        let old = engine.session_arc();
        let (player, mut st) = Player::stalled(Arc::clone(&old), 48_000, 256, 2);
        let playing = Arc::new(AtomicUsize::new(0));
        let hosted = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (p2, h2, s2) = (Arc::clone(&playing), Arc::clone(&hosted), Arc::clone(&stop));
        let audio = std::thread::Builder::new()
            .name("fake-audio".into())
            .spawn(move || {
                let mut out = vec![0.0f32; 2 * 128];
                while !s2.load(Ordering::Relaxed) {
                    st.fill(&mut out, 2);
                    p2.store(Arc::as_ptr(st.session()) as usize, Ordering::Relaxed);
                    h2.store(st.hosted_instances(), Ordering::Relaxed);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
            .unwrap();
        let app = SoundApp::new(engine, Some(player), Services::default());
        let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_max_steps(5_000).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
        h.run_steps(5);
        Rig { h, old, playing, hosted, stop, audio: Some(audio) }
    }

    fn plays_old(&self) -> bool {
        self.playing.load(Ordering::Relaxed) == Arc::as_ptr(&self.old) as usize
    }

    /// One frame, then a moment for the worker and the audio thread.
    fn frame(&mut self) {
        self.h.step();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(a) = self.audio.take() {
            let _ = a.join();
        }
    }
}

/// `(phase, done, total)` of the open under way.
fn phase(app: &SoundApp) -> Option<(OpenPhase, usize, usize)> {
    app.open.as_ref().map(|j| (j.progress().phase, j.progress().done, j.progress().total))
}

fn index(p: OpenPhase) -> usize {
    OpenPhase::ALL.iter().position(|x| *x == p).unwrap()
}

fn made(tag: &str) -> Vec<String> {
    let prefix = format!("clap:test.open.{tag}.");
    MADE_ON.lock().unwrap().iter().filter(|(id, _)| id.starts_with(&prefix)).map(|(_, t)| t.clone()).collect()
}

fn dropped(tag: &str) -> Vec<String> {
    let prefix = format!("clap:test.open.{tag}.");
    DROPPED_ON.lock().unwrap().iter().filter(|(id, _)| id.starts_with(&prefix)).map(|(_, t)| t.clone()).collect()
}

#[test]
fn opening_shows_the_phases_in_order_makes_plugins_on_the_ui_thread_across_frames_and_swaps_last() {
    let dir = scratch("phases");
    let (path, files) = session_with_fakes(&dir, "phases", 3);
    let mut rig = Rig::new();
    let me = thread_name();
    let old_name = rig.h.state().engine.session().name.clone();

    let r = rig.h.state_mut().run("session.open", json!({"path": path})).unwrap();
    assert_eq!(r, json!({"opening": true}), "the command returns at once");
    // The modal is up on the first frame, over the old session (still the document).
    rig.h.step();
    assert!(rig.h.state().open.is_some());
    rig.h.get_by_label("Opening phases…");
    rig.h.get_by_label("Cancel");
    assert_eq!(rig.h.state().engine.session().name, old_name);

    // Frame by frame until the open is over: the phases, and what the audio thread plays.
    let mut seen: Vec<(OpenPhase, usize, usize)> = vec![phase(rig.h.state()).unwrap()];
    let mut plugin_frames = 0;
    let mut swapped_early = false;
    for _ in 0..3_000 {
        if rig.h.state().open.is_none() {
            break;
        }
        rig.frame();
        let Some(now) = phase(rig.h.state()) else { break };
        if now.0 == OpenPhase::Plugins {
            plugin_frames += 1;
            swapped_early |= !rig.plays_old();
            assert_eq!(rig.h.state().engine.session().name, old_name, "the document swaps only at Finishing");
        }
        if seen.last() != Some(&now) {
            seen.push(now);
        }
    }
    assert!(rig.h.state().open.is_none(), "{seen:?}");
    let order: Vec<usize> = seen.iter().map(|(p, _, _)| index(*p)).collect();
    assert!(order.is_sorted(), "phases in order: {seen:?}");
    let audio: Vec<_> = seen.iter().filter(|(p, _, _)| *p == OpenPhase::Audio).collect();
    assert!(audio.iter().all(|(_, _, total)| *total == files), "{audio:?}");
    assert!(seen.iter().any(|(p, _, _)| *p == OpenPhase::Waveforms), "{seen:?}");
    let plugins: Vec<_> = seen.iter().filter(|(p, _, _)| *p == OpenPhase::Plugins).collect();
    assert!(plugins.iter().all(|(_, _, total)| *total == 3), "{plugins:?}");
    assert!(plugins.windows(2).all(|w| w[1].1 == w[0].1 + 1), "one plugin per frame (each takes longer than the budget): {plugins:?}");
    assert!(plugin_frames >= 3, "{plugin_frames}");
    assert!(seen.iter().any(|(p, _, _)| *p == OpenPhase::Finishing), "{seen:?}");
    assert_eq!(made("phases"), vec![me.clone(), me.clone(), me.clone()], "made on the UI thread, never the audio thread");
    assert!(!swapped_early, "the audio thread played the old session until the swap");

    // After the swap the new session is live: the document, and the audio thread with its
    // three instances.
    assert_eq!(rig.h.state().engine.session().name, "phases");
    assert_eq!(rig.h.state().ui.status, "Opened phases");
    for _ in 0..200 {
        if !rig.plays_old() && rig.hosted.load(Ordering::Relaxed) == 3 {
            break;
        }
        rig.frame();
    }
    assert!(!rig.plays_old());
    assert_eq!(rig.hosted.load(Ordering::Relaxed), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_during_plugins_keeps_the_old_session_and_drops_the_instances_on_the_ui_thread() {
    let dir = scratch("cancel");
    let (path, _) = session_with_fakes(&dir, "cancel", 8);
    let mut rig = Rig::new();
    let me = thread_name();
    let snapshot = rig.h.state().engine.session_arc();
    rig.h.state_mut().run("session.open", json!({"path": path})).unwrap();
    for _ in 0..3_000 {
        if matches!(phase(rig.h.state()), Some((OpenPhase::Plugins, done, _)) if done >= 1) {
            break;
        }
        rig.frame();
    }
    assert!(matches!(phase(rig.h.state()), Some((OpenPhase::Plugins, 1..=3, 8))), "{:?}", phase(rig.h.state()));
    let made_so_far = made("cancel").len();
    assert!(made_so_far >= 1);
    rig.h.get_by_label("Cancel").click();
    rig.h.step();
    assert!(rig.h.state().open.is_none());
    assert_eq!(rig.h.state().ui.status, "Open cancelled");
    assert!(Arc::ptr_eq(&snapshot, &rig.h.state().engine.session_arc()), "the old session is untouched");
    assert!(!rig.h.state().engine.is_dirty());
    // (the click's frames made one or two more before the cancel took: those go too)
    let made_so_far = made("cancel").len();
    let dropped = dropped("cancel");
    assert_eq!(dropped.len(), made_so_far, "every instance made so far is dropped");
    assert!(dropped.iter().all(|t| *t == me), "dropped on the UI thread: {dropped:?}");
    rig.h.run_steps(5);
    std::thread::sleep(std::time::Duration::from_millis(10));
    assert!(rig.plays_old(), "no player replacement, no swap");
    assert_eq!(rig.hosted.load(Ordering::Relaxed), 0);
    assert_eq!(made("cancel").len(), made_so_far, "nothing more is made after the cancel");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn other_session_commands_renders_and_device_changes_are_refused_while_opening() {
    let dir = scratch("refuse");
    let (path, _) = session_with_fakes(&dir, "refuse", 3);
    let mut rig = Rig::new();
    rig.h.state_mut().run("session.open", json!({"path": path})).unwrap();
    rig.h.step();
    let app = rig.h.state_mut();
    for id in ["session.new", "session.open", "session.close", "session.revert", "file.bounce_mix"] {
        assert_eq!(app.run(id, json!({"path": path})).unwrap_err(), OPENING, "{id}");
    }
    assert_eq!(app.run("track.freeze", json!({"track": "Kick"})).unwrap_err(), OPENING);
    assert_eq!(app.apply_audio_config(soundcraft_playback::AudioConfig::default()).unwrap_err(), OPENING);
    assert!(app.open.is_some(), "the open goes on");
    // Esc cancels it; then the same commands are welcome again.
    rig.h.key_press(egui::Key::Escape);
    rig.h.step();
    assert!(rig.h.state().open.is_none());
    assert_eq!(rig.h.state().ui.status, "Open cancelled");
    assert!(rig.h.state_mut().run("session.close", json!({})).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_control_channels_open_stays_synchronous() {
    let dir = scratch("sync");
    let (path, _) = session_with_fakes(&dir, "sync", 3);
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let r = app.run_sync("session.open", json!({"path": path})).unwrap();
    assert_eq!(r["name"], "sync");
    assert!(app.open.is_none());
    assert_eq!(app.engine.session().name, "sync");
    let _ = std::fs::remove_dir_all(&dir);
}

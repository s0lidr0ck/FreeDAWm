//! The Sampler's instrument window: audio dropped on it loads the sample (by path on the
//! desktop, by bytes in the browser), and built-in instruments show their parameters.

use egui::{DroppedFile, Event, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_model::TrackId;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
struct Dropped {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.path
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(self.bytes.clone())
    }
}

fn wav(secs: f32) -> Vec<u8> {
    let n = (48_000.0 * secs) as usize;
    let one: Vec<f32> = (0..n).map(|i| 0.4 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![one; 2] };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
    soundcraft_audio_io::encode(&buf, &opts).unwrap()
}

fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn harness() -> (Harness<'static, SoundApp>, TrackId) {
    let engine = soundcraft_engine::demo::demo_engine();
    let keys = engine.session().tracks.iter().find(|t| t.name == "Keys").unwrap().id;
    let mut app = SoundApp::new(engine, None, Services::default());
    app.ui.plugin_windows.push((keys, soundcraft_mix::SlotKey::Instrument));
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            let _ = app.take_dropped_files(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    (h, keys)
}

fn instrument(h: &Harness<'_, SoundApp>, t: TrackId) -> soundcraft_model::Insert {
    h.state().engine.session().track(t).unwrap().instrument.clone().unwrap()
}

/// Drops `file` with the pointer at the centre of the Keys sampler window.
fn drop_on_window(h: &mut Harness<'_, SoundApp>, keys: TrackId, file: Dropped) {
    let r = h.state().sample_drop_targets.iter().rev().find(|(_, t)| *t == keys).map(|(r, _)| *r).expect("the sampler window is a drop target");
    h.state_mut().synthetic.push(Event::PointerMoved(r.center()));
    for _ in 0..3 {
        step(h);
    }
    h.input_mut().dropped_files.push(Arc::new(file));
    step(h);
    for _ in 0..3 {
        step(h);
    }
}

#[test]
fn built_in_instruments_show_their_parameters() {
    let (h, keys) = harness();
    assert_eq!(instrument(&h, keys).plugin, "subtractive_synth");
    let _ = h.get_by_label("Cutoff");
}

#[test]
fn dropping_audio_with_a_path_loads_the_sampler() {
    let dir = std::env::temp_dir().join(format!("sc-sampler-window-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pluck.wav");
    std::fs::write(&path, wav(0.5)).unwrap();
    let (mut h, keys) = harness();
    drop_on_window(&mut h, keys, Dropped { path: path.clone(), bytes: Vec::new() });
    let ins = instrument(&h, keys);
    assert_eq!(ins.plugin, "sampler");
    let src = h.state().engine.session().source(ins.sample.unwrap()).cloned().unwrap();
    assert_eq!(src.name, "pluck");
    assert_eq!(src.frames, 24_000);
    assert!(h.state().engine.session().pool.get(src.id).is_some());
    assert!(h.state().ui.status.contains("loaded into the sampler"), "{}", h.state().ui.status);
    // The window is now Lift's editor: the sample's buttons, the panels and the detected root.
    let _ = h.get_by_label("Load Sample…");
    let _ = h.get_by_label("From Selection");
    for knob in ["Level", "Cutoff", "Resonance", "Tune", "Glide", "Rate", "To Volume", "Pan", "Release"] {
        let _ = h.get_by_label(knob);
    }
    // A pill sets its choice, one undo step.
    h.get_by_label("Ping-pong").click();
    for _ in 0..3 {
        step(&mut h);
    }
    assert_eq!(instrument(&h, keys).params.get("loop_mode").copied(), Some(2.0));
    h.state_mut().engine.execute("edit.undo", &serde_json::json!({})).unwrap();
    // Slice… cuts the sample and switches to Slice mode, where the slice tools show.
    h.get_by_label("Slice…").click();
    for _ in 0..3 {
        step(&mut h);
    }
    let sliced = instrument(&h, keys);
    assert_eq!(sliced.params.get("play_mode").copied(), Some(2.0));
    assert!(sliced.params.get("slice_count").copied().unwrap_or(0.0) >= 1.0);
    let _ = h.get_by_label("To MIDI");
    let _ = h.get_by_label("Slice → Keys");
    h.state_mut().engine.execute("edit.undo", &serde_json::json!({})).unwrap();
    // Warp and the LFO's Sync are switches.
    h.get_by_label("On").click();
    h.get_by_label("Sync").click();
    for _ in 0..3 {
        step(&mut h);
    }
    let warped = instrument(&h, keys);
    assert_eq!((warped.params.get("warp").copied(), warped.params.get("lfo_sync").copied()), (Some(1.0), Some(1.0)));
    let _ = h.get_by_label("Division");
    let _ = h.get_by_label("Sample Tempo");
    h.state_mut().engine.execute("edit.undo", &serde_json::json!({})).unwrap();
    h.state_mut().engine.execute("edit.undo", &serde_json::json!({})).unwrap();
    // The 220 Hz tone is A (MIDI 57); the root is still middle C until Use as Root.
    assert_eq!(ins.params.get("root_note").copied(), Some(60.0));
    h.get_by_label("Use as Root").click();
    for _ in 0..3 {
        step(&mut h);
    }
    assert_eq!(instrument(&h, keys).params.get("root_note").copied(), Some(57.0));
    assert!(h.query_by_label("Use as Root").is_none(), "nothing left to apply");
    h.state_mut().engine.execute("edit.undo", &serde_json::json!({})).unwrap();
    assert_eq!(instrument(&h, keys).params.get("root_note").copied(), Some(60.0), "one undo step");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dropping_audio_without_a_path_reads_its_bytes() {
    let (mut h, keys) = harness();
    drop_on_window(&mut h, keys, Dropped { path: PathBuf::from("web drop.wav"), bytes: wav(0.25) });
    // Nothing loads until the shell has read the bytes (asynchronously, in the browser).
    assert_eq!(instrument(&h, keys).plugin, "subtractive_synth");
    let reads = h.state_mut().take_sample_reads();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].track, keys);
    for r in &reads {
        let bytes = r.file.bytes();
        h.state_mut().load_sample_bytes(r, bytes);
    }
    let ins = instrument(&h, keys);
    assert_eq!(ins.plugin, "sampler");
    assert_eq!(h.state().engine.session().source(ins.sample.unwrap()).map(|s| s.name.as_str()), Some("web drop"));
    assert_eq!(h.state().engine.undo_label(), Some("Load Sampler Sample"));
    // Unreadable bytes leave the track alone and say why.
    let before = h.state().engine.session().sources.len();
    h.state_mut().load_sample_bytes(&reads[0], Err("permission denied".into()));
    assert!(h.state().ui.status.contains("permission denied"));
    assert_eq!(h.state().engine.session().sources.len(), before);
}

#[test]
fn non_audio_drops_are_left_to_the_shell() {
    let (mut h, keys) = harness();
    drop_on_window(&mut h, keys, Dropped { path: PathBuf::from("/tmp/song.scraft"), bytes: Vec::new() });
    assert_eq!(instrument(&h, keys).plugin, "subtractive_synth");
    assert!(h.state_mut().take_sample_reads().is_empty());
}

#[test]
fn a_built_in_instruments_menu_opens_its_parameters() {
    let engine = soundcraft_engine::demo::demo_engine();
    let keys = engine.session().tracks.iter().find(|t| t.name == "Keys").unwrap().id;
    let app = SoundApp::new(engine, None, Services::default());
    let mut h = Harness::builder()
        .with_size(vec2(400.0, 400.0))
        .build_ui_state(move |ui, app: &mut SoundApp| soundcraft_ui_egui::mix_window::instrument_menu(app, ui, keys), app);
    h.step();
    assert!(h.state().ui.plugin_windows.is_empty());
    h.get_by_label("Edit Parameters…").click();
    h.step();
    assert_eq!(h.state().ui.plugin_windows, vec![(keys, soundcraft_mix::SlotKey::Instrument)]);
}

#[test]
fn the_instrument_window_command_opens_and_closes_it() {
    let engine = soundcraft_engine::demo::demo_engine();
    let keys = engine.session().tracks.iter().find(|t| t.name == "Keys").unwrap().id;
    let mut app = SoundApp::new(engine, None, Services::default());
    let r = app.run("ui.mix.instrument_window", serde_json::json!({"track": "Keys"})).unwrap();
    assert_eq!(r["plugin"], "subtractive_synth");
    assert_eq!(app.ui.plugin_windows, vec![(keys, soundcraft_mix::SlotKey::Instrument)]);
    app.run("ui.mix.instrument_window", serde_json::json!({"track": "Keys", "open": false})).unwrap();
    assert!(app.ui.plugin_windows.is_empty());
    assert!(app.run("ui.mix.instrument_window", serde_json::json!({"track": "Kick"})).is_err(), "an audio track has no instrument");
}

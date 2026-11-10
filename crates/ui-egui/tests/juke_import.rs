//! Drum takes dropped on the Juke window: on a pad card they become that pad's layers and
//! round-robins; anywhere else in the window they build the kit, each file to its pad by name.

use egui::{DroppedFile, Event, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_model::TrackId;
use soundcraft_ui_egui::juke_window::Tab;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
struct Dropped(PathBuf);

impl DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// A generated take (no recorded audio): hits of the given amplitudes, 300 ms apart.
fn take(path: &Path, amps: &[f32]) {
    let sr = 48_000.0f32;
    let mut x = vec![0.0f32; 4_800];
    for &a in amps {
        x.extend((0..(sr * 0.3) as usize).map(|i| {
            let t = i as f32 / sr;
            a * (-t / 0.05).exp() * (t * 180.0 * std::f32::consts::TAU).sin()
        }));
        x.extend(vec![0.0; 9_600]);
    }
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![x] };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, ..Default::default() };
    std::fs::write(path, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sc-juke-import-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn rig() -> (Harness<'static, SoundApp>, TrackId) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::new(soundcraft_model::Session::default()), None, Services::default());
    let v = app.run("track.new_beat", json!({})).unwrap();
    let tid: TrackId = serde_json::from_value(v["track"].clone()).unwrap();
    app.ui.juke_tabs.insert(tid, Tab::Kit);
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            let _ = app.take_dropped_files(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..4 {
        step(&mut h);
    }
    (h, tid)
}

fn drop_at(h: &mut Harness<'_, SoundApp>, at: egui::Pos2, files: &[PathBuf]) {
    h.state_mut().synthetic.push(Event::PointerMoved(at));
    for _ in 0..3 {
        step(h);
    }
    for f in files {
        h.input_mut().dropped_files.push(Arc::new(Dropped(f.clone())));
    }
    for _ in 0..3 {
        step(h);
    }
}

fn juke(h: &Harness<'_, SoundApp>, t: TrackId) -> soundcraft_model::Insert {
    h.state().engine.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap()
}

#[test]
fn takes_dropped_on_a_pad_become_its_layers() {
    let d = dir("pad");
    let f = d.join("Snare soft to hard.wav");
    take(&f, &[0.05, 0.1, 0.2, 0.4, 0.8, 0.06, 0.12, 0.25]);
    let (mut h, t) = rig();
    // Pad 3's card (the snare).
    let card =
        h.state().kit_import_targets.iter().find(|(_, tr, pad)| *tr == t && *pad == Some(2)).map(|(r, _, _)| *r).expect("pad cards take drops");
    drop_at(&mut h, card.center(), &[f]);
    // The picker opens on the hits found; Apply takes the ones it ticked.
    let picker = h.state().ui.hit_picker.clone().expect("the picker opens");
    assert_eq!((picker.pad, picker.hits.len()), (2, 8));
    assert!(juke(&h, t).samples.keys().all(|k| !k.starts_with("p3.")), "nothing changes before Apply");
    h.get_by_label("Apply 8 hits").click();
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(h.state().ui.hit_picker.is_none());
    let ins = juke(&h, t);
    let keys: Vec<&String> = ins.samples.keys().filter(|k| k.starts_with("p3.")).collect();
    assert_eq!(keys.len(), 8, "{keys:?}");
    assert!(
        ins.samples.keys().any(|k| k.starts_with("p3.v1.")) && ins.samples.keys().any(|k| k.starts_with("p3.v2.")),
        "soft and hard layers: {keys:?}"
    );
    assert!(h.state().ui.status.starts_with("Snare"), "{}", h.state().ui.status);
    // Other pads keep their sounds.
    assert!(!ins.samples.keys().any(|k| k.starts_with("p1.")));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn takes_dropped_on_the_window_build_the_kit() {
    let d = dir("kit");
    let kick = d.join("Kick.wav");
    let hat = d.join("hihat closed.wav");
    take(&kick, &[0.8, 0.8]);
    take(&hat, &[0.3, 0.3, 0.3]);
    let (mut h, t) = rig();
    let window =
        h.state().kit_import_targets.iter().find(|(_, tr, pad)| *tr == t && pad.is_none()).map(|(r, _, _)| *r).expect("the window takes drops");
    // A corner of the window, away from the pads.
    drop_at(&mut h, window.min + vec2(8.0, 8.0), &[kick, hat]);
    let ins = juke(&h, t);
    assert_eq!(ins.samples.keys().filter(|k| k.starts_with("p1.")).count(), 2, "the kick on pad 1");
    assert_eq!(ins.samples.keys().filter(|k| k.starts_with("p5.")).count(), 3, "the hat on pad 5");
    assert!(h.state().ui.status.starts_with("Imported 2 pads"), "{}", h.state().ui.status);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn from_selection_in_the_pad_editor_imports_the_selected_hits() {
    let d = dir("selection");
    let f = d.join("Kick.wav");
    take(&f, &[0.2, 0.5, 0.9, 0.3]);
    let (mut h, t) = rig();
    {
        let app = h.state_mut();
        app.ui.juke_tabs.insert(t, Tab::Pad);
        app.run("file.import_audio", json!({"path": f.to_string_lossy()})).unwrap();
    }
    for _ in 0..2 {
        step(&mut h);
    }
    // No selection yet: the button waits for one.
    assert!(h.get_by_label("From Selection").accesskit_node().is_disabled());
    let audio = h.state().engine.session().tracks.iter().find(|tr| tr.kind == soundcraft_model::TrackKind::Audio).map(|tr| tr.id).unwrap();
    h.state_mut().run("edit.select", json!({"tracks": [audio.0], "start": 0, "end": 48_000 * 3})).unwrap();
    for _ in 0..2 {
        step(&mut h);
    }
    // The selection is on the audio track; the Juke window still knows its own track.
    h.get_by_label("From Selection").click();
    for _ in 0..3 {
        step(&mut h);
    }
    h.get_by_label("Apply 4 hits").click();
    for _ in 0..3 {
        step(&mut h);
    }
    let ins = juke(&h, t);
    assert_eq!(ins.samples.keys().filter(|k| k.starts_with("p1.")).count(), 4, "{}", h.state().ui.status);
    assert!(h.state().ui.status.starts_with("Kick: 4 hits"), "{}", h.state().ui.status);
    let _ = std::fs::remove_dir_all(&d);
}

fn proof(h: &mut Harness<SoundApp>, name: &str) {
    if let Some(dir) = std::env::var_os("SOUNDCRAFT_JUKE_PROOF_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
    }
}

#[test]
fn the_picker_keeps_four_a_band_and_moves_hits_between_bands() {
    let d = dir("picker");
    // A take to pick from: `SOUNDCRAFT_JUKE_TAKE` (a real recording, for the proof picture), else
    // 24 generated hits, soft to hard.
    let f = match std::env::var_os("SOUNDCRAFT_JUKE_TAKE") {
        Some(p) => PathBuf::from(p),
        None => {
            let f = d.join("Snare.wav");
            take(&f, &(0..24).map(|i| 0.015 * 1.2f32.powi((i * 7) % 24)).collect::<Vec<_>>());
            f
        }
    };
    let mut app = SoundApp::new(soundcraft_engine::Engine::new(soundcraft_model::Session::default()), None, Services::default());
    let v = app.run("track.new_beat", json!({})).unwrap();
    let t: TrackId = serde_json::from_value(v["track"].clone()).unwrap();
    soundcraft_ui_egui::hit_picker::open(&mut app, t, 2, json!({"paths": [f.to_string_lossy()]}));
    let mut builder = Harness::builder().with_size(vec2(1280.0, 860.0)).with_step_dt(0.1);
    if std::env::var_os("SOUNDCRAFT_JUKE_PROOF_DIR").is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..4 {
        step(&mut h);
    }
    proof(&mut h, "juke-hit-picker");
    if std::env::var_os("SOUNDCRAFT_JUKE_TAKE").is_some() {
        return;
    }
    let picker = || h_picker(&h);
    let pk = picker();
    assert_eq!(pk.hits.len(), 24);
    assert!((0..4).all(|b| pk.kept(b) == 4), "the best sixteen, four a band");
    // A full band refuses a fifth.
    let spare = pk.hits.iter().find(|x| !x.keep && x.band == 3 && !x.off_pitch).map(|x| x.hit).unwrap();
    h.get_by_label(&format!("Keep hit {}", spare + 1)).click();
    step(&mut h);
    assert!(!h_picker(&h).hits.iter().any(|x| x.hit == spare && x.keep));
    assert!(h.state().ui.status.contains("Hard holds four"), "{}", h.state().ui.status);
    // Untick one of Hard's, tick the spare: still four.
    let kept = h_picker(&h).hits.iter().find(|x| x.keep && x.band == 3).map(|x| x.hit).unwrap();
    h.get_by_label(&format!("Keep hit {}", kept + 1)).click();
    step(&mut h);
    h.get_by_label(&format!("Keep hit {}", spare + 1)).click();
    step(&mut h);
    let pk = h_picker(&h);
    assert!(pk.hits.iter().any(|x| x.hit == spare && x.keep) && pk.kept(3) == 4);
    // Moving a hit to another band (as a drag ends there) keeps it there; Best 16 puts it back.
    let mv = pk.hits.iter().find(|x| x.band == 0 && !x.keep).map(|x| x.hit).unwrap();
    soundcraft_ui_egui::hit_picker::apply(h.state_mut(), vec![soundcraft_ui_egui::hit_picker::Ask::Move(mv, 2)]);
    assert_eq!(h_picker(&h).hits.iter().find(|x| x.hit == mv).unwrap().band, 2);
    h.get_by_label("Best 16").click();
    step(&mut h);
    assert_eq!(h_picker(&h).hits.iter().find(|x| x.hit == mv).unwrap().band, 0);
    // Clear, then keep two Ghost hits: Apply builds a one-layer pad from them.
    h.get_by_label("Clear").click();
    step(&mut h);
    let ghosts: Vec<usize> = h_picker(&h).hits.iter().filter(|x| x.band == 0).take(2).map(|x| x.hit).collect();
    for g in &ghosts {
        h.get_by_label(&format!("Keep hit {}", g + 1)).click();
        step(&mut h);
    }
    step(&mut h);
    h.get_by_label("Apply 2 hits").click();
    for _ in 0..3 {
        step(&mut h);
    }
    let keys: Vec<String> = juke(&h, t).samples.keys().filter(|k| k.starts_with("p3.")).cloned().collect();
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert!(keys.iter().all(|k| k.starts_with("p3.v1.")), "{keys:?}");
    assert_eq!(h.state().ui.juke_tabs.get(&t), Some(&Tab::Pad), "the pad shows");
    let _ = std::fs::remove_dir_all(&d);
}

fn h_picker(h: &Harness<'_, SoundApp>) -> soundcraft_ui_egui::hit_picker::HitPicker {
    h.state().ui.hit_picker.clone().expect("the picker is open")
}

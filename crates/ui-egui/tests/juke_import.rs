//! Drum takes dropped on the Juke window: on a pad card they become that pad's layers and
//! round-robins; anywhere else in the window they build the kit, each file to its pad by name.

use egui::{DroppedFile, Event, vec2};
use egui_kittest::Harness;
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
    use egui_kittest::kittest::{NodeT as _, Queryable as _};
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
    let ins = juke(&h, t);
    assert_eq!(ins.samples.keys().filter(|k| k.starts_with("p1.")).count(), 4, "{}", h.state().ui.status);
    assert!(h.state().ui.status.starts_with("Kick: 4 hits"), "{}", h.state().ui.status);
    let _ = std::fs::remove_dir_all(&d);
}

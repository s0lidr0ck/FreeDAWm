//! Sample-level views of audio: the Pencil at sample zoom redraws samples (one undo step), the
//! Sample Editor edits a selection, and the spectral view draws a spectrogram.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{Clip, TrackId};
use soundcraft_ui_egui::{Services, SoundApp};

fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn steps(h: &mut Harness<'_, SoundApp>, n: usize) {
    for _ in 0..n {
        step(h);
    }
}

fn drag(h: &mut Harness<'_, SoundApp>, from: Pos2, to: Pos2) {
    let m = Modifiers::NONE;
    let mut evs = vec![Event::PointerMoved(from), Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: m }];
    evs.extend((1..=16).map(|k| Event::PointerMoved(from + (to - from) * (k as f32 / 16.0))));
    evs.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: m });
    let n = evs.len();
    h.state_mut().synthetic.extend(evs);
    steps(h, n + 8);
}

#[test]
fn the_pencil_redraws_samples_at_sample_zoom() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let t = TrackId(app.run("track.new", json!({"name": "Wave", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
    app.run("track.delete", json!({"tracks": ["Kick", "Snare", "Hats", "Bass", "Pad", "Keys", "Lead", "Verb"]})).unwrap();
    app.run("track.height", json!({"tracks": [t.0], "height": "large"})).unwrap();
    let s = app.engine.session_mut();
    let src =
        soundcraft_engine::io::add_source(s, "zero", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 4_000]] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(s, t, Clip::audio(id, "zero", src, 0, 0, 4_000));
    app.run("edit.tool", json!({"tool": "pencil"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h, 6);
    {
        let z = &mut h.state_mut().engine.session_mut().edit.zoom;
        (z.scroll, z.samples_per_px) = (0, 0.5);
    }
    steps(&mut h, 4);
    let tl = h.state().edit_layout.timeline;
    let row = h.state().edit_layout.rows.iter().find(|(i, _)| *i == t.0).map(|(_, r)| *r).unwrap();
    let height = h.state().engine.session().track(t).unwrap().height.points();
    // Above the middle of the lane (positive values), from sample 100 to 300.
    let y = row[1] + height * 0.35;
    drag(&mut h, pos2(tl[0] + 200.0, y), pos2(tl[0] + 600.0, y));
    let s = h.state().engine.session();
    let c = s.track(t).unwrap().clips()[0].clone();
    assert_ne!(c.source(), Some(src), "the stroke made a new file");
    let buf = s.pool.get(c.source().unwrap()).unwrap().buffer().into_owned();
    let drawn = &buf.channels[0];
    assert!(drawn[200] > 0.05, "drawn above the line: {}", drawn[200]);
    assert_eq!(drawn[50], 0.0, "untouched before the stroke");
    assert_eq!(drawn[400], 0.0, "untouched after it");
    assert!(h.state_mut().engine.undo());
    assert_eq!(h.state().engine.session().track(t).unwrap().clips()[0].source(), Some(src), "one undo step");
}

#[test]
fn a_stroke_becomes_one_value_per_sample() {
    use soundcraft_ui_egui::sample_editor::stroke_values;
    let (from, v) = stroke_values(&[(10, 0.0), (14, 1.0), (12, -1.0)]).unwrap();
    assert_eq!(from, 10);
    assert_eq!(v, vec![0.0, 0.25, -1.0, 0.0, 1.0], "the later segment wins where they cross");
    assert_eq!(stroke_values(&[(5, 0.5)]), Some((5, vec![0.5])));
    assert_eq!(stroke_values(&[]), None);
}

#[test]
fn the_sample_editor_silences_a_selection() {
    use egui_kittest::kittest::Queryable;
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let t = TrackId(app.run("track.new", json!({"name": "Wave", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = app.engine.session_mut();
    let src =
        soundcraft_engine::io::add_source(s, "half", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(s, t, Clip::audio(id, "half", src, 0, 0, 48_000));
    app.run("edit.select", json!({"clips": [id.0]})).unwrap();
    soundcraft_ui_egui::menus::invoke_menu(&mut app, "window.sample_editor", "Process > More > Sample Editor");
    assert!(app.ui.show_sample_editor);
    let mut h = Harness::builder().with_size(vec2(1600.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h, 6);
    h.get_by_label("Silence");
    // A selection dragged across the middle of the view, then Silence.
    let r = h.get_by_label("Silence").rect();
    let from = pos2(r.min.x + 40.0, r.min.y - 80.0);
    drag(&mut h, from, from + vec2(200.0, 0.0));
    h.get_by_label("Silence").click();
    steps(&mut h, 4);
    let c = h.state().engine.session().track(t).unwrap().clips()[0].clone();
    assert_ne!(c.source(), Some(src), "a new file");
    let buf = h.state().engine.session().pool.get(c.source().unwrap()).unwrap().buffer().into_owned();
    let silent = buf.channels[0].iter().filter(|x| **x == 0.0).count();
    assert!(silent > 1_000 && silent < 48_000, "part of the clip silenced: {silent}");
}

#[test]
fn the_spectral_view_finds_a_tone_and_draws() {
    use soundcraft_ui_egui::spectral::{BANDS, FFT, band_of, column, heat};
    let rate = 48_000.0;
    let tone: Vec<f32> = (0..8_192).map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1_000.0 / rate).sin() * 0.5).collect();
    let audio = soundcraft_model::SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![tone.clone()] });
    let mut an = soundcraft_dsp::spectrum::Analyzer::default();
    let col = column(&mut an, &audio, 4_096, rate);
    let b = band_of((1_000.0 * FFT as f32 / rate).round() as usize, rate).unwrap();
    let loudest = col.iter().enumerate().max_by_key(|(_, v)| **v).map(|(i, _)| i).unwrap();
    assert!(loudest.abs_diff(b) <= 1, "1 kHz in band {b}, loudest {loudest}: {col:?}");
    assert!(col[BANDS - 1] < col[b] / 2, "the top band is quiet");
    assert_eq!(heat(0), egui::Color32::BLACK);

    // A track in the spectral view draws without trouble.
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let t = TrackId(app.run("track.new", json!({"name": "Tone", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = app.engine.session_mut();
    let src = soundcraft_engine::io::add_source(s, "tone", AudioBuffer { sample_rate: 48_000, channels: vec![tone] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(s, t, Clip::audio(id, "tone", src, 0, 0, 8_192));
    app.run("track.view", json!({"tracks": [t.0], "view": "spectral"})).unwrap();
    assert!(soundcraft_ui_egui::edit_window::view_options(app.engine.session().track(t).unwrap()).contains(&"spectral"));
    let mut h = Harness::builder().with_size(vec2(1600.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    {
        let z = &mut h.state_mut().engine.session_mut().edit.zoom;
        (z.scroll, z.samples_per_px) = (0, 8.0);
    }
    steps(&mut h, 6);
    assert_eq!(h.state().engine.session().track(t).unwrap().view, "spectral");
}

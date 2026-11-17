//! A stretch is heard at once through the real-time stretcher and its offline render, made by
//! the app's background worker, replaces it without an undo step.

use egui::{Pos2, RawInput, Rect, vec2};
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};
use soundcraft_ui_egui::{Services, SoundApp};

fn frame(ctx: &egui::Context, app: &mut SoundApp) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

#[test]
fn the_worker_renders_a_live_stretch_and_swaps_it_in() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Tone"));
    let v: Vec<f32> = (0..96_000).map(|i| (i as f32 * 0.0576).sin() * 0.5).collect();
    let src = soundcraft_engine::io::add_source(
        &mut s,
        "Tone",
        soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![v] },
        None,
        soundcraft_audio_io::FileFormat::Wav,
    );
    let cid = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(cid, "Tone", src, 0, 0, 96_000));
    s.track_mut(t).unwrap().elastic = Some("polyphonic".into());
    let mut e = Engine::new(s);
    e.defer_stretch = soundcraft_ui_egui::stretch_worker::StretchWorker::available();
    let mut app = SoundApp::new(e, None, Services::default());
    let ctx = egui::Context::default();
    frame(&ctx, &mut app);
    app.engine.execute("clip.elastic_properties", &json!({"clips": [cid.0], "ratio": 2.0})).unwrap();
    let c = app.engine.session().find_clip(cid).unwrap().1.clone();
    assert_eq!(c.length, 192_000, "the new length at once");
    let rendered = c.source().unwrap();
    assert!(app.engine.session().live_stretch.contains_key(&rendered), "heard live first");
    // Frames go by while the worker renders on its thread.
    for _ in 0..2_000 {
        frame(&ctx, &mut app);
        if app.engine.session().live_stretch.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let s = app.engine.session();
    assert!(s.live_stretch.is_empty(), "the render was swapped in");
    assert_eq!(s.pool.get(rendered).map(|a| a.frames()), Some(192_000));
    assert_eq!(s.find_clip(cid).unwrap().1.source(), Some(rendered), "the clip already pointed at it");
    // Not an undo step: one undo takes the stretch back.
    app.engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(app.engine.session().find_clip(cid).unwrap().1.length, 96_000);
}

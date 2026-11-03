//! Comping with take layers in the Edit window: the layers open and close under the track, the
//! comp's ranges are outlined on them, swipes and clicks comp, Up/Down cycles the takes.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{Clip, SourceId, Track, TrackId};
use soundcraft_time::Range;
use soundcraft_ui_egui::edit_window::comp_layers::{LAYER_H, used_ranges};
use soundcraft_ui_egui::{Services, SoundApp};

/// Each take covers 0..LEN on its layer.
const LEN: i64 = 480_000;

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

fn click(h: &mut Harness<'_, SoundApp>, pos: Pos2, modifiers: Modifiers) {
    h.state_mut().synthetic.extend([
        Event::ModifiersChanged(modifiers),
        Event::PointerMoved(pos),
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers },
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers },
        Event::ModifiersChanged(Modifiers::NONE),
    ]);
    steps(h, 8);
}

/// Press at `from`, move to `to` in steps, release (with `modifiers` held throughout). The app
/// plays one synthetic event a frame.
fn drag(h: &mut Harness<'_, SoundApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    let mut evs = vec![
        Event::ModifiersChanged(modifiers),
        Event::PointerMoved(from),
        Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers },
    ];
    evs.extend((1..=8).map(|k| Event::PointerMoved(from + (to - from) * (k as f32 / 8.0))));
    evs.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers });
    evs.push(Event::ModifiersChanged(Modifiers::NONE));
    let n = evs.len();
    h.state_mut().synthetic.extend(evs);
    steps(h, n + 8);
}

fn vox(h: &Harness<'_, SoundApp>) -> Track {
    h.state().engine.session().track_by_name("Vox").cloned().unwrap()
}

fn comp(h: &Harness<'_, SoundApp>) -> Vec<Option<usize>> {
    vox(h).clips().iter().map(|c| c.from_layer).collect()
}

/// The demo session plus a track "Vox" with four takes on layers 1-4 (0..LEN), its comp take 1.
fn harness() -> (Harness<'static, SoundApp>, TrackId) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let t = TrackId(app.run("track.new", json!({"name": "Vox", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = app.engine.session_mut();
    for k in 1..=4u64 {
        let layer = soundcraft_engine::cmd::comp::add_layer(s, t, None).unwrap();
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlists[layer].clips.push(Clip::audio(id, format!("take{k}"), SourceId(9000 + k), 0, 0, LEN));
    }
    s.track_mut(t).unwrap().record_layer = None;
    app.run("comp.swipe", json!({"track": t.0, "layer": 1, "start": 0, "end": LEN})).unwrap();
    // Only the new track, zoomed so the takes span most of the timeline.
    app.run("track.delete", json!({"tracks": ["Kick", "Snare", "Hats", "Bass", "Pad", "Keys", "Lead", "Verb"]})).unwrap();
    app.run("track.height", json!({"tracks": [t.0], "height": "medium"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h, 6);
    let tl = h.state().edit_layout.timeline;
    let z = &mut h.state_mut().engine.session_mut().edit.zoom;
    z.scroll = 0;
    z.samples_per_px = LEN as f64 * 1.2 / f64::from(tl[2] - tl[0]);
    steps(&mut h, 4);
    (h, t)
}

/// The row of track `t`: [x0, y0, x1, y1].
fn row(h: &Harness<'_, SoundApp>, t: TrackId) -> [f32; 4] {
    h.state().edit_layout.rows.iter().find(|(id, _)| *id == t.0).map(|(_, r)| *r).unwrap()
}

/// The point on layer `n` (1-based, top to bottom) at sample `at`.
fn on_layer(h: &Harness<'_, SoundApp>, t: TrackId, n: usize, at: i64) -> Pos2 {
    let r = row(h, t);
    let tl = h.state().edit_layout.timeline;
    let z = &h.state().engine.session().edit.zoom;
    let x = tl[0] + ((at - z.scroll) as f64 / z.samples_per_px) as f32;
    pos2(x, r[1] + vox(h).height.points() + (n as f32 - 0.5) * LAYER_H)
}

#[test]
fn layers_open_and_close_under_the_track() {
    let (mut h, t) = harness();
    let medium = vox(&h).height.points();
    assert!(vox(&h).layers_open, "a new layer opens the layers");
    // Four layers and the "+ New Layer" row.
    let r = row(&h, t);
    assert_eq!(r[3] - r[1], medium + 5.0 * LAYER_H);
    for n in 1..=4 {
        h.get_by_label(&format!("Vox layer Take {n}"));
    }
    let toggle = h.get_by_label("Vox layers").rect().center();
    click(&mut h, toggle, Modifiers::NONE);
    assert!(!vox(&h).layers_open);
    let r = row(&h, t);
    assert_eq!(r[3] - r[1], medium);
    assert!(h.query_by_label("Vox layer Take 1").is_none());
    let toggle = h.get_by_label("Vox layers").rect().center();
    click(&mut h, toggle, Modifiers::NONE);
    assert!(vox(&h).layers_open);
}

#[test]
fn the_comps_ranges_are_outlined_on_their_layers() {
    let (mut h, t) = harness();
    h.state_mut().run("comp.swipe", json!({"track": t.0, "layer": 3, "start": 100_000, "end": 200_000})).unwrap();
    let tr = vox(&h);
    // The 10 ms crossfades: the later clip of each join reaches 5 ms back (the takes' sources
    // aren't loaded here, so only the clip offsets give handles).
    let half = 240;
    assert_eq!(used_ranges(&tr, 3), [Range::new(100_000 - half, 200_000)]);
    assert_eq!(used_ranges(&tr, 1), [Range::new(0, 100_000), Range::new(200_000 - half, LEN)]);
    assert!(used_ranges(&tr, 2).is_empty());
    steps(&mut h, 2);
}

#[test]
fn a_swipe_is_one_undo_step_and_the_right_comp() {
    let (mut h, t) = harness();
    let before = h.state().engine.undo_history().len();
    let (a, b) = (on_layer(&h, t, 3, 100_000), on_layer(&h, t, 3, 200_000));
    drag(&mut h, a, b, Modifiers::NONE);
    assert_eq!(comp(&h), [Some(1), Some(3), Some(1)]);
    assert_eq!(h.state().engine.undo_history().len(), before + 1);
    assert_eq!(h.state().engine.undo_label(), Some("Comp"));
    // The take-3 piece covers the swiped range (within a pixel, plus the crossfade).
    let piece = vox(&h).clips()[1].range();
    assert!((piece.start - 100_000).abs() < 1_000 && (piece.end - 200_000).abs() < 1_000, "{piece:?}");
    // A click puts the whole take in the comp.
    let p = on_layer(&h, t, 2, 300_000);
    click(&mut h, p, Modifiers::NONE);
    assert_eq!(comp(&h), [Some(2)]);
    assert_eq!(h.state().engine.undo_history().len(), before + 2);
}

#[test]
fn ctrl_swipe_fills_only_where_the_comp_is_empty() {
    let (mut h, t) = harness();
    h.state_mut().run("edit.clear", json!({"tracks": [t.0], "start": 150_000, "end": 250_000})).unwrap();
    let (a, b) = (on_layer(&h, t, 4, 100_000), on_layer(&h, t, 4, 300_000));
    drag(&mut h, a, b, Modifiers::CTRL);
    assert_eq!(comp(&h), [Some(1), Some(4), Some(1)]);
    // The neighbours keep their ground, but for the crossfades.
    let c = vox(&h).clips().to_vec();
    assert!(c[0].end() >= 150_000 && c[0].end() < 151_000 && c[2].start <= 250_000 && c[2].start > 249_000);
}

fn key(h: &mut Harness<'_, SoundApp>, key: egui::Key) {
    h.state_mut().synthetic.extend([
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE },
        Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE },
    ]);
    steps(h, 6);
}

#[test]
fn up_and_down_cycle_the_selected_segment_through_the_takes() {
    let (mut h, t) = harness();
    h.state_mut().run("comp.swipe", json!({"track": t.0, "layer": 2, "start": 100_000, "end": 200_000})).unwrap();
    let seg = vox(&h).clips()[1].id.0;
    h.state_mut().run("edit.select", json!({"clips": [seg]})).unwrap();
    steps(&mut h, 2);
    let before = h.state().engine.undo_history().len();
    key(&mut h, egui::Key::ArrowDown);
    assert_eq!(comp(&h), [Some(1), Some(3), Some(1)]);
    key(&mut h, egui::Key::ArrowDown);
    assert_eq!(comp(&h), [Some(1), Some(4), Some(1)]);
    // Nothing below the last take: the comp stays.
    key(&mut h, egui::Key::ArrowDown);
    assert_eq!(comp(&h), [Some(1), Some(4), Some(1)]);
    key(&mut h, egui::Key::ArrowUp);
    assert_eq!(comp(&h), [Some(1), Some(3), Some(1)]);
    // One undo step each.
    assert_eq!(h.state().engine.undo_history().len(), before + 3);
}

#[test]
fn the_new_layer_row_and_the_layer_menu() {
    let (mut h, _) = harness();
    let add = h.get_by_label("Vox new layer").rect().center();
    click(&mut h, add, Modifiers::NONE);
    let tr = vox(&h);
    assert_eq!((tr.playlists.len(), tr.record_layer), (6, Some(5)));
    assert_eq!(tr.playlists[5].name, "Take 5");
    steps(&mut h, 2);
    h.get_by_label("Vox layer Take 5");
    // Right-click Take 1 (the comp uses it): no plain delete, Remove and Delete instead.
    let head = h.get_by_label("Vox layer Take 1").rect().center();
    h.state_mut().synthetic.extend([
        Event::PointerMoved(head),
        Event::PointerButton { pos: head, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos: head, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE },
    ]);
    steps(&mut h, 6);
    assert!(h.query_by_label("Delete Take").is_none());
    h.get_by_label("Remove it from the comp first");
    let rec = h.get_by_label("Record Into This Layer").rect().center();
    click(&mut h, rec, Modifiers::NONE);
    assert_eq!(vox(&h).record_layer, Some(1));
}

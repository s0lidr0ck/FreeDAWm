//! A closed folder edits as one clip: its lane shows the members' clips as blocks and a
//! selection made on it takes the members with it.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_time::Range;
use soundcraft_ui_egui::{Services, SoundApp};

fn steps(h: &mut Harness<'_, SoundApp>, n: usize) {
    for _ in 0..n {
        let mut raw = std::mem::take(h.input_mut());
        h.state_mut().raw_input_hook(&mut raw);
        *h.input_mut() = raw;
        h.step();
    }
}

fn drag(h: &mut Harness<'_, SoundApp>, from: Pos2, to: Pos2) {
    let m = Modifiers::NONE;
    let mut evs = vec![Event::PointerMoved(from), Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: m }];
    evs.extend((1..=8).map(|k| Event::PointerMoved(from + (to - from) * (k as f32 / 8.0))));
    evs.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: m });
    let n = evs.len();
    h.state_mut().synthetic.extend(evs);
    steps(h, n + 8);
}

#[test]
fn a_closed_folder_shows_blocks_and_a_selection_on_it_takes_the_members() {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"count": 2, "kind": "midi"})).unwrap();
    let members: Vec<TrackId> = engine.session().tracks.iter().map(|t| t.id).collect();
    for (t, (a, b)) in members.iter().zip([(0, 96_000), (48_000, 144_000)]) {
        engine.execute("midi.new_clip", &json!({"track": t.0, "start": a, "end": b})).unwrap();
    }
    engine.execute("midi.new_clip", &json!({"track": members[0].0, "start": 400_000, "end": 450_000})).unwrap();
    engine.execute("track.move_to_new_folder", &json!({"tracks": [members[0].0, members[1].0]})).unwrap();
    let folder = engine.session().tracks.iter().find(|t| t.is_folder()).unwrap().id;
    engine.execute("track.folder_toggle", &json!({"track": folder.0, "open": false})).unwrap();
    engine.execute("edit.tool", &json!({"tool": "selector"})).unwrap();
    // Two blocks: the overlapping clips joined, the later one alone.
    let blocks = soundcraft_ui_egui::edit_window::folder_blocks(engine.session(), folder, Range::new(0, 1_000_000));
    assert_eq!(blocks, vec![Range::new(0, 144_000), Range::new(400_000, 450_000)]);

    let app = SoundApp::new(engine, None, Services::default());
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
        (z.scroll, z.samples_per_px) = (0, 400.0);
    }
    steps(&mut h, 4);
    let tl = h.state().edit_layout.timeline;
    let row = h.state().edit_layout.rows.iter().find(|(i, _)| *i == folder.0).map(|(_, r)| *r).unwrap();
    assert!(h.state().edit_layout.rows.iter().all(|(i, _)| !members.iter().any(|m| m.0 == *i)), "the members are hidden");
    let y = (row[1] + row[3]) * 0.5;
    drag(&mut h, pos2(tl[0] + 50.0, y), pos2(tl[0] + 200.0, y));
    let ed = &h.state().engine.session().edit;
    assert!(ed.selected_tracks.contains(&folder));
    for m in &members {
        assert!(ed.selected_tracks.contains(m), "member {m:?} selected with the closed folder");
    }
    assert!(ed.selection.len() > 40_000, "{:?}", ed.selection);
}

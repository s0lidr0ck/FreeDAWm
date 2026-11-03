//! What a Mix strip shows about its track's state: inputs, activity, meters, groups.

use egui::{Color32, Shape, epaint::TextShape, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, theme::Tokens};

fn mix(engine: soundcraft_engine::Engine) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(engine, None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn texts(h: &Harness<SoundApp>, text: &str) -> Vec<TextShape> {
    h.output()
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == text => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn color(t: &TextShape) -> Color32 {
    t.override_text_color.unwrap_or(t.galley.job.sections[0].format.color)
}

fn click(h: &mut Harness<SoundApp>, at: egui::Pos2) {
    h.hover_at(at);
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
}

fn two_tracks() -> soundcraft_engine::Engine {
    let mut e = soundcraft_engine::Engine::default();
    e.execute("track.new", &json!({"name": "Gtr"})).unwrap();
    e.execute("track.new", &json!({"name": "Vox"})).unwrap();
    e
}

#[test]
fn an_unassigned_input_reads_no_input_dimmed() {
    let mut e = two_tracks();
    e.execute("track.input", &json!({"track": "Vox", "input": "none"})).unwrap();
    e.execute("track.input", &json!({"track": "Gtr", "input": "In 2"})).unwrap();
    let h = mix(e);
    let none = texts(&h, "No Input");
    assert_eq!(none.len(), 1, "only Vox has no input");
    assert_eq!(color(&none[0]), Tokens::current().text_dim);
    assert!(texts(&h, "In 1").is_empty(), "no made-up In 1");
    let gtr = texts(&h, "In 2");
    assert_eq!(gtr.len(), 1);
    assert_ne!(color(&gtr[0]), Tokens::current().text_dim);
}

#[test]
fn an_inactive_track_greys_its_strip_and_the_menu_offers_make_active() {
    let mut e = two_tracks();
    e.execute("track.make_inactive", &json!({"tracks": ["Vox"], "inactive": true})).unwrap();
    let h = mix(e);
    let plates = texts(&h, "INACTIVE");
    assert_eq!(plates.len(), 1, "only Vox's strip");
    // The plate sits on Vox's strip, the second one, right of Gtr's name.
    let gtr = texts(&h, "Gtr");
    let vox = texts(&h, "Vox");
    let x = plates[0].visual_bounding_rect().center().x;
    assert!(gtr.iter().all(|g| g.visual_bounding_rect().max.x < x), "on Vox, not Gtr");
    assert!(vox.iter().any(|v| (v.visual_bounding_rect().center().x - x).abs() < 40.0), "under Vox's name");
    // Its name's menu offers Make Active, and that wakes it up.
    let mut h = h;
    let at = vox.iter().map(|v| v.visual_bounding_rect().center()).max_by(|a, b| a.y.total_cmp(&b.y)).unwrap();
    h.hover_at(at);
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
    assert!(texts(&h, "Make Inactive").is_empty());
    let item = texts(&h, "Make Active");
    assert_eq!(item.len(), 1, "the menu shows Make Active");
    click(&mut h, item[0].visual_bounding_rect().center());
    assert!(!h.state().engine.session().track_by_name("Vox").unwrap().inactive);
    assert!(texts(&h, "INACTIVE").is_empty());
}

fn meter_of(h: &Harness<SoundApp>, name: &str) -> soundcraft_ui_egui::MeterDisplay {
    let id = h.state().engine.session().track_by_name(name).unwrap().id;
    h.state().meters.get(&id).copied().unwrap_or_default()
}

fn hit(h: &mut Harness<SoundApp>, name: &str, peak: f32) {
    let id = h.state().engine.session().track_by_name(name).unwrap().id;
    let m = h.state_mut().meters.entry(id).or_default();
    m.clip = true;
    m.peak = peak;
}

#[test]
fn meters_clear_their_clip_on_a_click_and_show_a_resettable_peak_hold() {
    let mut h = mix(two_tracks());
    hit(&mut h, "Gtr", 0.5);
    hit(&mut h, "Vox", 1.2);
    h.run_steps(2);
    // Peak-hold readouts: -6.0 on Gtr, +1.6 (over, in red) on Vox.
    let gtr = texts(&h, "-6.0");
    assert_eq!(gtr.len(), 1, "Gtr's peak readout");
    let vox = texts(&h, "1.6");
    assert_eq!(vox.len(), 1, "Vox's peak readout");
    assert_eq!(color(&vox[0]), Tokens::current().meter_red);
    // A click on Gtr's meter (just above its readout) clears its clip light and peak only.
    let meter = gtr[0].visual_bounding_rect().center() - vec2(0.0, 40.0);
    click(&mut h, meter);
    assert!(!meter_of(&h, "Gtr").clip);
    assert!(meter_of(&h, "Vox").clip, "only that track");
    assert!(!texts(&h, "-inf").is_empty() && texts(&h, "-6.0").is_empty(), "Gtr's peak reset");
    // Alt-click clears every track's.
    hit(&mut h, "Gtr", 0.5);
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
    click(&mut h, meter);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    assert!(!meter_of(&h, "Gtr").clip && !meter_of(&h, "Vox").clip);
    // A click on the readout resets the peak and leaves the clip light alone.
    hit(&mut h, "Vox", 0.5);
    h.run_steps(1);
    let readout = texts(&h, "-6.0");
    click(&mut h, readout[0].visual_bounding_rect().center());
    assert_eq!(meter_of(&h, "Vox").peak, 0.0);
    assert!(meter_of(&h, "Vox").clip);
}

fn group_box(h: &Harness<SoundApp>, strip: usize) -> egui::Pos2 {
    use egui_kittest::kittest::Queryable;
    let mut boxes: Vec<egui::Rect> = h.get_all_by_role_and_label(egui::accesskit::Role::ComboBox, "Group").map(|n| n.rect()).collect();
    boxes.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
    boxes[strip].center()
}

fn open_group_box(h: &mut Harness<SoundApp>, strip: usize) {
    let at = group_box(h, strip);
    click(h, at);
}

fn press_button(h: &mut Harness<SoundApp>, label: &str) {
    use egui_kittest::kittest::Queryable;
    // The widest match: a menu item rather than the Groups list's + of the same name.
    let rects: Vec<egui::Rect> = h.get_all_by_role_and_label(egui::accesskit::Role::Button, label).map(|n| n.rect()).collect();
    let at = rects.iter().max_by(|a, b| a.width().total_cmp(&b.width())).expect(label).center();
    click(h, at);
}

fn press_plus(h: &mut Harness<SoundApp>, label: &str) {
    use egui_kittest::kittest::Queryable;
    let rects: Vec<egui::Rect> = h.get_all_by_role_and_label(egui::accesskit::Role::Button, label).map(|n| n.rect()).collect();
    let at = rects.iter().find(|r| r.width() < 24.0).expect("the + key").center();
    click(h, at);
}

fn members(h: &Harness<SoundApp>, group: &str) -> Vec<String> {
    let s = h.state().engine.session();
    let g = s.groups.iter().find(|g| g.name == group);
    g.map(|g| g.members.iter().filter_map(|m| s.track(*m)).map(|t| t.name.clone()).collect()).unwrap_or_default()
}

#[test]
fn the_strip_group_box_joins_leaves_and_makes_groups_and_the_list_shows_them() {
    let mut h = mix(two_tracks());
    // Vox's box: New Group… opens Create Group with Vox ticked; OK makes it, and the list shows it.
    open_group_box(&mut h, 1);
    press_button(&mut h, "New Group…");
    assert_eq!(h.state().dialogs.open_name(), Some("group"));
    press_button(&mut h, "OK");
    assert_eq!(members(&h, "Group 1"), ["Vox"]);
    assert!(!texts(&h, "Group 1").is_empty(), "the Groups list and Vox's box show it");
    // Gtr's box: tick Group 1 to join it, tick again to leave.
    open_group_box(&mut h, 0);
    press_button(&mut h, "Group 1 (a)");
    assert_eq!(members(&h, "Group 1"), ["Vox", "Gtr"]);
    open_group_box(&mut h, 0);
    press_button(&mut h, "Group 1 (a)");
    assert_eq!(members(&h, "Group 1"), ["Vox"]);
    // The last member leaving deletes the group.
    open_group_box(&mut h, 1);
    press_button(&mut h, "Group 1 (a)");
    assert!(h.state().engine.session().groups.is_empty());
    // The Groups list's + makes one from the selection.
    h.state_mut().engine.execute("edit.select", &json!({"tracks": ["Gtr"], "exact": true})).unwrap();
    h.run_steps(1);
    press_plus(&mut h, "New Group…");
    assert_eq!(h.state().dialogs.open_name(), Some("group"));
    press_button(&mut h, "OK");
    assert_eq!(members(&h, "Group 1"), ["Gtr"]);
}

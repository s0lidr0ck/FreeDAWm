//! CleanQ's editor drawn with live values (as the plugin publishes them while it plays): each
//! dynamic band's ghost node, the dynamic ceiling's magenta cut, the lit "Dynamic" label.

use super::*;
use crate::Services;
use crate::editor_kit::palette::al;
use egui::epaint::{CircleShape, PathShape};
use egui_kittest::Harness;
use soundcraft_dsp::analysis::Analysis;

fn app_with_cleanq() -> (SoundApp, soundcraft_model::TrackId, ModuleId) {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Pad"})).unwrap_or_default();
    engine.execute("mix.module_add", &json!({"track": "Pad", "plugin": "fs_cleanq"})).unwrap_or_default();
    let t = engine.session().tracks[0].id;
    let m = engine.session().tracks[0].mixer.inserts[0].id;
    let params =
        json!({"b01_on": 2.0, "b01_type": 0.0, "b01_dyn": 1.0, "b01_freq": q::freq_to_knob(1000.0), "b01_gain": q::gain_to_knob(0.0), "ceilOn": 1.0});
    engine.execute("mix.module_params", &json!({"track": t.0, "module": m.0, "params": params})).unwrap_or_default();
    (SoundApp::new(engine, None, Services::default()), t, m)
}

fn harness(app: SoundApp, t: soundcraft_model::TrackId, m: ModuleId, meter: StripMeter) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(egui::vec2(1100.0, 700.0)).build_ui_state(
        move |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            if !app.fonts_ready {
                return;
            }
            let s = app.engine.session_arc();
            if let Some(track) = s.track(t) {
                crate::editor_kit::run::<Editor>(app, ui, track, m, Some(&meter));
            }
        },
        app,
    )
}

#[test]
fn dynamic_ghost_nodes_and_cut_are_drawn() {
    let (mut app, t, m) = app_with_cleanq();
    app.ui.drawer_tab = Some((t, m, "Ceiling".into()));
    let a = Arc::new(Analysis::with_live(cq::LIVE_LEN));
    a.live.set(cq::LIVE_DYN.start, -4.0);
    for i in cq::LIVE_CUT {
        a.live.set(i, -3.0);
    }
    a.live.set(cq::LIVE_CUTTING, 1.0);
    let meter = StripMeter { analysis: vec![(m, a)], ..StripMeter::default() };
    let mut h = harness(app, t, m, meter);
    h.run_steps(4);
    let f = crate::editor_kit::frame_rect(&h.ctx, t, m).unwrap_or(Rect::NOTHING);
    assert_eq!(f.size(), SIZE);
    // The geometry with the Ceiling strip open.
    let graph = Rect::from_min_max(pos2(f.left(), f.top() + TOP_BAR_H), pos2(f.right(), f.bottom() - CEIL_STRIP_H));
    let plot = Rect::from_min_max(pos2(graph.left(), graph.top() + EDGE_INSET), pos2(graph.right(), graph.bottom() - SCALE_H - EDGE_INSET));
    let geo = Geo { plot, range: 18.0 };
    let shapes: Vec<Shape> = h.output().shapes.iter().map(|c| c.shape.clone()).collect();

    // The ghost: a 3.5 px dot in band 1's colour at gain + offset, on its node's x.
    let ghost = shapes.iter().any(|s| match s {
        Shape::Circle(CircleShape { center, radius, fill, .. }) => {
            (*radius - 3.5).abs() < 0.01
                && (center.y - geo.gy(-4.0)).abs() < 0.5
                && (center.x - geo.fx(1000.0)).abs() < 0.5
                && *fill == al(band_colour(0), 0.55)
        }
        _ => false,
    });
    assert!(ghost, "a ghost node at the live offset");

    // The cut: its edge in CleanQGraph.h's magenta at 85 %, at -3 dB.
    let cut = shapes.iter().any(|s| match s {
        Shape::Path(PathShape { points, stroke, .. }) => {
            stroke.color == egui::epaint::ColorMode::Solid(al(CUT_COLOUR, 0.85))
                && points.len() == 256
                && points.iter().all(|p| (p.y - geo.gy(-3.0)).abs() < 0.5)
        }
        _ => false,
    });
    assert!(cut, "the dynamic ceiling's cut");

    // "Dynamic" lit in the caution colour while it cuts.
    let lit = shapes.iter().any(|s| matches!(s, Shape::Text(t) if t.galley.job.text == "Dynamic" && t.galley.job.sections.first().is_some_and(|sec| sec.format.color == WARN)));
    assert!(lit, "the Dynamic label is lit");
}

#[test]
fn hostile_live_values_draw() {
    let (app, t, m) = app_with_cleanq();
    let a = Arc::new(Analysis::with_live(cq::LIVE_LEN));
    for i in 0..cq::LIVE_LEN {
        a.live.set(i, if i % 2 == 0 { f32::NAN } else { f32::INFINITY });
    }
    let meter = StripMeter { analysis: vec![(m, a)], ..StripMeter::default() };
    let mut h = harness(app, t, m, meter);
    h.run_steps(6);
    assert!(crate::editor_kit::frame_rect(&h.ctx, t, m).is_some());
}

// ---------------------------------------------------------------------------- undo steps
use egui::{Modifiers, PointerButton};

fn plain_harness(app: SoundApp, t: soundcraft_model::TrackId, m: ModuleId) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(egui::vec2(1100.0, 700.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        move |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            if !app.fonts_ready {
                return;
            }
            let s = app.engine.session_arc();
            if let Some(track) = s.track(t) {
                crate::editor_kit::run::<Editor>(app, ui, track, m, None);
            }
        },
        app,
    )
}

fn ev(h: &mut Harness<SoundApp>, pos: Pos2, pressed: Option<bool>) {
    h.input_mut().events.push(egui::Event::PointerMoved(pos));
    if let Some(pressed) = pressed {
        h.input_mut().events.push(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.step();
}

fn click_text(h: &mut Harness<SoundApp>, want: &str) {
    let at = text_rect(h, |t| t == want).map(|r| r.center());
    let Some(at) = at else { panic!("no {want:?} on screen") };
    ev(h, at, None);
    ev(h, at, Some(true));
    ev(h, at, Some(false));
    h.run_steps(2);
}

fn text_rect(h: &Harness<SoundApp>, f: impl Fn(&str) -> bool) -> Option<Rect> {
    h.output().shapes.iter().find_map(|c| match &c.shape {
        Shape::Text(t) if f(&t.galley.job.text) => Some(t.visual_bounding_rect()),
        _ => None,
    })
}

fn state_of(h: &Harness<SoundApp>, t: soundcraft_model::TrackId, m: ModuleId) -> CeilingState {
    let s = h.state().engine.session();
    s.track(t).and_then(|x| x.mixer.module(m)).and_then(Insert::state_bytes).and_then(|b| CeilingState::read(&b)).unwrap_or_default()
}

#[test]
fn a_noise_level_drag_is_one_undo_step() {
    let (mut app, t, m) = app_with_cleanq();
    app.ui.drawer_tab = Some((t, m, "Ceiling".into()));
    let mut h = plain_harness(app, t, m);
    h.run_steps(4);
    click_text(&mut h, "White");
    assert!(state_of(&h, t, m).noise_on[0], "White noise guide on");
    let steps = h.state().engine.undo_history().len();
    let chip = text_rect(&h, |s| s == "White 0").map(|r| r.center());
    let Some(chip) = chip else { panic!("the White noise chip") };
    ev(&mut h, chip, None);
    ev(&mut h, chip, Some(true));
    for k in 1..=6 {
        ev(&mut h, chip + egui::vec2(0.0, 5.0 * k as f32), None);
    }
    ev(&mut h, chip + egui::vec2(0.0, 30.0), Some(false));
    h.run_steps(2);
    let st = state_of(&h, t, m);
    assert!(st.noise_manual[0], "the drag set the level by hand");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "one drag, one undo step");
    h.state_mut().engine.undo();
    h.run_steps(2);
    let st = state_of(&h, t, m);
    assert!(!st.noise_manual[0] && st.noise_on[0], "undo takes the drag back, not the toggle");
}

#[test]
fn conform_is_one_undo_step_with_its_bands() {
    let (mut app, t, m) = app_with_cleanq();
    // Free band 1 again so Conform starts from an empty EQ; a capture to conform.
    app.engine.execute("mix.module_params", &json!({"track": t.0, "module": m.0, "params": {"b01_on": 0.0, "ceilOn": 0.0}})).unwrap_or_default();
    let ring = soundcraft_dsp::analysis::AnalysisRing::new();
    let mut seed = 7u32;
    let noise: Vec<f32> = (0..8192)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            0.05 * ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0) + 0.5 * (std::f32::consts::TAU * 2000.0 * i as f32 / 48_000.0).sin()
        })
        .collect();
    let mut learn = cc::Learn::default();
    for k in 0..8 {
        ring.push_slice(&noise);
        learn.feed(&ring, f64::from(k), 48_000.0, 0.0, 3);
    }
    assert_eq!(learn.trace().len(), soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling::GRID_N, "a capture");
    app.ui.cq_learn.insert((t, m), learn);
    // Its drawer open (a capture is kept only for open drawers).
    app.ui.drawers.insert(t, m);
    let flat: Vec<[f64; 2]> = soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling::flat_ceiling(-60.0).pts.iter().map(|p| [p.hz, p.db]).collect();
    app.engine.execute("mix.module_ceiling", &json!({"track": t.0, "module": m.0, "points": flat})).unwrap_or_default();
    app.ui.drawer_tab = Some((t, m, "Ceiling".into()));
    let mut h = plain_harness(app, t, m);
    h.run_steps(4);
    let steps = h.state().engine.undo_history().len();
    click_text(&mut h, "Conform");
    let st = state_of(&h, t, m);
    assert!(!st.generated.is_empty(), "Conform wrote bands");
    let on = |h: &Harness<SoundApp>| {
        h.state()
            .engine
            .session()
            .track(t)
            .and_then(|x| x.mixer.module(m))
            .map_or(0, |i| (1..=24).filter(|b| i.params.get(&format!("b{b:02}_on")).copied().unwrap_or(0.0) > 0.5).count())
    };
    assert!(on(&h) > 0 && on(&h) <= 12, "at most 12 fixed bells");
    let module = |h: &Harness<SoundApp>, id: &str| {
        h.state().engine.session().track(t).and_then(|x| x.mixer.module(m)).and_then(|i| i.params.get(id).copied()).unwrap_or(-1.0)
    };
    for b in 1..=24 {
        if module(&h, &format!("b{b:02}_on")) > 0.5 {
            assert_eq!(module(&h, &format!("b{b:02}_type")), 0.0, "bells only");
            assert!(q::knob_to_q(module(&h, &format!("b{b:02}_q"))) <= 1.5);
        }
    }
    // The make-up went on the output, and the state remembers it as Conform's own.
    let st = state_of(&h, t, m);
    assert!(st.makeup_db != 0.0);
    assert!((q::knob_to_out_db(module(&h, "outGain")) - st.makeup_db).abs() < 0.01, "{} vs {}", module(&h, "outGain"), st.makeup_db);
    // The status says what was asked before the clamp (a flat ceiling is far from noise + a tone).
    assert!(text_rect(&h, |s| s.contains("asked") && s.contains(", EQ ") && s.contains("output")).is_some(), "the Conform status");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "bands, output and list: one undo step");
    h.state_mut().engine.undo();
    h.run_steps(3);
    assert!(state_of(&h, t, m).generated.is_empty(), "undo takes the list away");
    assert_eq!(on(&h), 0, "and the bands with it");
    assert_eq!(module(&h, "outGain"), 0.5, "and the make-up");
    assert_eq!(state_of(&h, t, m).makeup_db, 0.0);

    // The user's own trim (+3 dB), then Conform, a different Conform (range +-3 dB), Undo and
    // Remove: the make-up bookkeeping is undone with the bands, so the output ends at +3 dB.
    h.state_mut()
        .engine
        .execute("mix.module_params", &json!({"track": t.0, "module": m.0, "params": {"outGain": q::out_db_to_knob(3.0)}}))
        .unwrap_or_default();
    h.run_steps(2);
    click_text(&mut h, "Conform");
    let first = state_of(&h, t, m).makeup_db;
    assert!(first != 0.0);
    let mut half = state_of(&h, t, m);
    half.max_db = 3.0;
    h.state_mut()
        .engine
        .execute("mix.module_state_edit", &json!({"track": t.0, "module": m.0, "state": soundcraft_model::b64::encode(&half.write())}))
        .unwrap_or_default();
    h.run_steps(2);
    click_text(&mut h, "Conform");
    let second = state_of(&h, t, m).makeup_db;
    assert!((second - first).abs() > 0.1, "{first} vs {second}");
    assert!((q::knob_to_out_db(module(&h, "outGain")) - (3.0 + second)).abs() < 0.01);
    h.state_mut().engine.undo();
    h.run_steps(3);
    assert!((state_of(&h, t, m).makeup_db - first).abs() < 1e-9, "undo brings the first make-up back");
    assert!((q::knob_to_out_db(module(&h, "outGain")) - (3.0 + first)).abs() < 0.01);
    click_text(&mut h, "Remove");
    assert!(
        (q::knob_to_out_db(module(&h, "outGain")) - 3.0).abs() < 0.01,
        "Remove leaves the user's +3 dB: {}",
        q::knob_to_out_db(module(&h, "outGain"))
    );
    assert_eq!(state_of(&h, t, m).makeup_db, 0.0);
}

// ---------------------------------------------------------------------------- guide templates
fn guides_of(h: &Harness<SoundApp>, t: soundcraft_model::TrackId, m: ModuleId) -> Vec<Guide> {
    state_of(h, t, m).guides.list
}

fn texts(h: &Harness<SoundApp>) -> Vec<String> {
    h.output()
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            Shape::Text(t) => Some(t.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

fn module_params(h: &Harness<SoundApp>, t: soundcraft_model::TrackId, m: ModuleId) -> Vec<(String, f32)> {
    let s = h.state().engine.session();
    let mut v: Vec<(String, f32)> =
        s.track(t).and_then(|x| x.mixer.module(m)).map(|i| i.params.iter().map(|(k, v)| (k.clone(), *v)).collect()).unwrap_or_default();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

#[test]
fn the_templates_picker_places_guides_in_one_undo_step() {
    let (app, t, m) = app_with_cleanq();
    let mut h = plain_harness(app, t, m);
    h.run_steps(4);
    let steps = h.state().engine.undo_history().len();
    let params = module_params(&h, t, m);
    let at = text_rect(&h, |s| s.trim() == "Templates").map(|r| r.center());
    let Some(at) = at else { panic!("the Templates button") };
    ev(&mut h, at, None);
    ev(&mut h, at, Some(true));
    ev(&mut h, at, Some(false));
    h.run_steps(2);
    click_text(&mut h, "Female vocal");
    let g = guides_of(&h, t, m);
    assert_eq!(g.len(), 7, "the template's seven areas");
    assert_eq!((g[2].area.as_str(), g[2].freq_hz, g[2].lo_hz, g[2].hi_hz), ("Mud / box", 450.0, 350.0, 700.0));
    assert_eq!((g[2].note.as_str(), g[2].hint.as_str()), ("boxy, cardboard", "cut 2\u{2013}3 dB"));
    assert!(g.iter().all(|g| g.gain_db == 0.0));
    assert_eq!(module_params(&h, t, m), params, "guides only: no band, no gain");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "one undo step");
    // On the graph: each range's chip, labelled with its area.
    let shown = texts(&h);
    for want in ["3  Mud / box", "4  Nasal", "7  Air"] {
        assert!(shown.iter().any(|s| s == want), "{want} on the graph: {shown:?}");
    }
    // Hovering a chip shows the whole guide.
    let chip = text_rect(&h, |s| s == "3  Mud / box").map(|r| r.center());
    let Some(chip) = chip else { panic!("the Mud / box chip") };
    ev(&mut h, chip, None);
    h.run_steps(3);
    let shown = texts(&h);
    assert!(shown.iter().any(|s| s.contains("Usual move: cut 2\u{2013}3 dB")), "the tooltip: {shown:?}");
    // Applying it again adds nothing (all there); undo takes the guides away.
    h.state_mut().engine.execute("mix.module_guides", &json!({"track": t.0, "module": m.0, "template": "female_vocal"})).unwrap_or_default();
    assert_eq!(guides_of(&h, t, m).len(), 7);
    h.state_mut().engine.undo();
    h.state_mut().engine.undo();
    h.run_steps(2);
    assert!(guides_of(&h, t, m).is_empty(), "undo takes the template back");
}

fn reads_the_guide(slim: bool) {
    let (mut app, t, m) = app_with_cleanq();
    app.ui.slim_editors = slim;
    // Band 1 sits at 1 kHz: inside Female vocal's "Nasal" (900 Hz - 1.8 kHz).
    app.engine.execute("mix.module_guides", &json!({"track": t.0, "module": m.0, "template": "Female vocal"})).unwrap_or_default();
    app.ui.drawer_tab = Some((t, m, "Band 1".into()));
    let mut h = plain_harness(app, t, m);
    h.run_steps(5);
    let shown = texts(&h);
    let has = |f: &dyn Fn(&str) -> bool| shown.iter().any(|s| f(s));
    assert!(has(&|s| s == "Band 1"), "the band's card: {shown:?}");
    assert!(has(&|s| s == "Nasal"), "the area under it: {shown:?}");
    assert!(has(&|s| s == "Centre 1.2 kHz  \u{00B7}  900 Hz \u{2013} 1.8 kHz"), "centre and range: {shown:?}");
    assert!(has(&|s| s.starts_with("Sounds like") && s.ends_with("honky, nasal")), "what it sounds like: {shown:?}");
    assert!(has(&|s| s.starts_with("Usual move") && s.ends_with("narrow cut")), "the usual move: {shown:?}");
    // The text is drawn at a readable size.
    let size = h.output().shapes.iter().find_map(|c| match &c.shape {
        Shape::Text(t) if t.galley.job.text.ends_with("honky, nasal") => t.galley.job.sections.last().map(|s| s.format.font_id.size),
        _ => None,
    });
    assert!(size.is_some_and(|s| s >= 11.0), "{size:?}");
    // Not in a guide: no guide block.
    let mut h2 = {
        let (mut app, t, m) = app_with_cleanq();
        app.ui.slim_editors = slim;
        app.engine.execute("mix.module_guides", &json!({"track": t.0, "module": m.0, "template": "kick_out"})).unwrap_or_default();
        app.ui.drawer_tab = Some((t, m, "Band 1".into()));
        plain_harness(app, t, m)
    };
    h2.run_steps(5);
    assert!(!texts(&h2).iter().any(|s| s.starts_with("Usual move")), "1 kHz is in none of Kick out's areas");
}

#[test]
fn the_band_card_reads_the_guide_the_band_sits_in() {
    reads_the_guide(false);
}

#[test]
fn the_slim_drawer_reads_the_guide_the_band_sits_in() {
    reads_the_guide(true);
}

#[test]
fn guides_from_an_older_session_still_draw() {
    let (mut app, t, m) = app_with_cleanq();
    // A state written before guides had an area or a usual move.
    let old = "<ceiling/><guides><guide freq=\"1000.0\" lo=\"800.0\" hi=\"1250.0\" gain=\"-3.0\" dyn=\"0\" done=\"0\" note=\"honk\"/></guides>";
    app.engine
        .execute("mix.module_state_edit", &json!({"track": t.0, "module": m.0, "state": soundcraft_model::b64::encode(old.as_bytes())}))
        .unwrap_or_default();
    app.ui.drawer_tab = Some((t, m, "Band 1".into()));
    let mut h = plain_harness(app, t, m);
    h.run_steps(5);
    let g = guides_of(&h, t, m);
    assert_eq!(g.len(), 1);
    assert!(g[0].area.is_empty() && g[0].hint.is_empty());
    let shown = texts(&h);
    assert!(shown.iter().any(|s| s == "1  honk"), "its chip, labelled by its note: {shown:?}");
    assert!(shown.iter().any(|s| s.starts_with("Sounds like") && s.ends_with("honk")), "read under the band: {shown:?}");
    assert!(!shown.iter().any(|s| s.starts_with("Usual move")), "no usual move to show");
}

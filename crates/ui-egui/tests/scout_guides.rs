//! Scout guides: parsing Claude's guides block (hostile blocks dropped with a note), the overlay on
//! the Clean Q graph (visual only, never a panic), and the panel's suggestion rows.

use egui::{Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Guide, Insert, ScoutTurn, Session, TrackId, TrackKind};
use soundcraft_ui_egui::module_graphs::freq_to_x;
use soundcraft_ui_egui::scout::guides::{guide_rects, overlay, parse_guides};
use soundcraft_ui_egui::{Services, SoundApp};

fn known() -> Vec<String> {
    vec!["Lead Vocals".into(), "Bass".into()]
}

const GOOD: &str = r#"**Summary** The vocal is harsh around 3 kHz.

```json
{"format": "scout-guides", "version": 1, "guides": [
  {"kind": "eq", "track": "Lead Vocals", "lo_hz": 2500, "hi_hz": 4000, "gain_lo_db": -4, "gain_hi_db": -2, "dynamic": true, "q": 2.0, "label": "harshness"},
  {"kind": "module", "track": "Lead Vocals", "module": "Comp", "param": "Threshold", "current": "-18 dB", "range": "-22 to -24 dB", "reason": "peaks 9 dB over"}
]}
```"#;

#[test]
fn parses_the_guides_block() {
    let (g, notes) = parse_guides(GOOD, &known());
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(g.len(), 2);
    match &g[0] {
        Guide::Eq { track, lo_hz, hi_hz, gain_lo_db, gain_hi_db, dynamic, q, label } => {
            assert_eq!(track, "Lead Vocals");
            assert_eq!((*lo_hz, *hi_hz, *gain_lo_db, *gain_hi_db), (2500.0, 4000.0, -4.0, -2.0));
            assert!(*dynamic);
            assert_eq!(*q, Some(2.0));
            assert_eq!(label, "harshness");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(&g[1], Guide::Module { module, range, .. } if module == "Comp" && range == "-22 to -24 dB"));
    // The last scout-guides block wins; an unrelated json block before it is ignored.
    let two = format!("```json\n{{\"a\": 1}}\n```\n\n{GOOD}");
    assert_eq!(parse_guides(&two, &known()).0.len(), 2);
    // Track names match without regard to case, and come back as the package spells them.
    let lower = GOOD.replace("\"Lead Vocals\"", "\"lead vocals\"");
    assert!(matches!(&parse_guides(&lower, &known()).0[0], Guide::Eq { track, .. } if track == "Lead Vocals"));
}

fn eq(track: &str, lo: &str, hi: &str, glo: &str, ghi: &str) -> String {
    format!(
        r#"{{"kind": "eq", "track": "{track}", "lo_hz": {lo}, "hi_hz": {hi}, "gain_lo_db": {glo}, "gain_hi_db": {ghi}, "dynamic": false, "label": "x"}}"#
    )
}

fn block(guides: &[String]) -> String {
    format!("Answer.\n\n```json\n{{\"format\": \"scout-guides\", \"version\": 1, \"guides\": [{}]}}\n```\n", guides.join(","))
}

#[test]
fn hostile_guides_are_dropped_not_drawn() {
    // No block, or a malformed one: no guides, one note, the answer is still the answer.
    let (g, n) = parse_guides("Just words, no suggestions.", &known());
    assert!(g.is_empty() && n.is_empty(), "nothing suggested, nothing to say");
    let (g, n) = parse_guides("```json\n{\"format\": \"scout-guides\", \"guides\": [ {\"kind\": \n```", &known());
    assert!(g.is_empty());
    assert_eq!(n.len(), 1, "{n:?}");
    let (g, n) = parse_guides("```json\n{\"format\": \"scout-guides\", \"guides\": 7}\n```", &known());
    assert!(g.is_empty() && n.len() == 1, "{n:?}");
    // Each bad guide is dropped with its own note; the good one survives.
    let text = block(&[
        eq("Lead Vocals", "200", "400", "-3", "-1"),
        eq("Bass DI", "200", "400", "-3", "-1"),
        eq("Bass", "\"NaN\"", "400", "-3", "-1"),
        eq("Bass", "4000", "400", "-3", "-1"),
        eq("Bass", "10", "400", "-3", "-1"),
        eq("Bass", "200", "1000000", "-3", "-1"),
        eq("Bass", "200", "400", "-60", "-1"),
        eq("Bass", "200", "400", "\"inf\"", "-1"),
        r#"{"kind": "teleport", "track": "Bass"}"#.into(),
        "42".into(),
        r#"{"kind": "module", "track": "Nobody", "module": "Comp", "param": "Ratio"}"#.into(),
    ]);
    let (g, n) = parse_guides(&text, &known());
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(n.len(), 10, "{n:#?}");
    assert!(n[0].contains("Bass DI"), "{}", n[0]);
    // The overlay never panics, even on guides that slipped past (an old session, a hand edit).
    let hostile = vec![
        Guide::Eq {
            track: "Bass".into(),
            lo_hz: f32::NAN,
            hi_hz: 400.0,
            gain_lo_db: 0.0,
            gain_hi_db: 0.0,
            dynamic: false,
            q: None,
            label: "a".into(),
        },
        Guide::Eq {
            track: "Bass".into(),
            lo_hz: 4e6,
            hi_hz: -1.0,
            gain_lo_db: f32::INFINITY,
            gain_hi_db: 0.0,
            dynamic: false,
            q: None,
            label: String::new(),
        },
        Guide::Eq {
            track: "Bass".into(),
            lo_hz: 100.0,
            hi_hz: 200.0,
            gain_lo_db: -3.0,
            gain_hi_db: -1.0,
            dynamic: false,
            q: None,
            label: "ok".into(),
        },
    ];
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 176.0));
    let rects = guide_rects(&hostile, rect, freq_to_x);
    assert_eq!(rects.len(), 1, "only the drawable one");
    assert_eq!((rects[0].0, rects[0].3.as_str()), (2, "3  ok"), "numbered as listed, with its index");
    assert!(rects.iter().all(|(_, r, _, _)| r.is_finite() && rect.contains_rect(*r)));
    let (mut s, t) = session_with_cleanq("Bass");
    s.scout.turns.push(ScoutTurn { id: 9, question: "q".into(), answer: "a".into(), guides: hostile, ..ScoutTurn::default() });
    s.scout.guides_shown = Some(9);
    let app = SoundApp::new(Engine::new(s), None, Services::default());
    let ins = app.engine.session().tracks[0].mixer.inserts[0].clone();
    let mut h = Harness::new_ui(|ui| overlay(&app, ui, t, &ins, Rect::from_min_size(ui.min_rect().min, vec2(400.0, 176.0))));
    h.run_steps(2);
}

fn session_with_cleanq(name: &str) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
    if let Some(tr) = s.tracks.iter_mut().find(|x| x.id == t) {
        tr.mixer.inserts.push(Insert::new("fs_cleanq"));
    }
    (s, t)
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    )
}

fn turn_with(guides: Vec<Guide>) -> ScoutTurn {
    ScoutTurn {
        id: 5,
        question: "Too dull?".into(),
        captures: vec![],
        answer: "Lift the air.".into(),
        guides,
        model: "claude-opus-5-5".into(),
        ..ScoutTurn::default()
    }
}

#[test]
fn guides_never_change_the_session_mixer() {
    let (mut s, t) = session_with_cleanq("Bass");
    let g = Guide::Eq {
        track: "Bass".into(),
        lo_hz: 60.0,
        hi_hz: 120.0,
        gain_lo_db: 1.0,
        gain_hi_db: 3.0,
        dynamic: false,
        q: None,
        label: "weight".into(),
    };
    s.scout.turns.push(turn_with(vec![g]));
    let module = s.tracks[0].mixer.inserts[0].id;
    let before = serde_json::to_string(&s.tracks).unwrap();
    let mut h = harness(SoundApp::new(Engine::new(s), None, Services::default()));
    h.state_mut().ui.window = soundcraft_ui_egui::MainWindow::Mix;
    h.state_mut().ui.drawers.insert(t, module);
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.run_steps(3);
    h.get_by_label("Show guides on Clean Q").click();
    h.run_steps(4);
    assert_eq!(h.state().engine.session().scout.guides_shown, Some(5));
    assert_eq!(serde_json::to_string(&h.state().engine.session().tracks).unwrap(), before, "guides are visual only");
    h.get_by_label("Hide guides").click();
    h.run_steps(2);
    assert_eq!(h.state().engine.session().scout.guides_shown, None);
    assert_eq!(serde_json::to_string(&h.state().engine.session().tracks).unwrap(), before);
}

#[test]
fn a_track_without_clean_q_lists_the_guide_with_a_note() {
    let mut s = Session::default();
    s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Bass"));
    let g = Guide::Eq {
        track: "Bass".into(),
        lo_hz: 60.0,
        hi_hz: 120.0,
        gain_lo_db: 1.0,
        gain_hi_db: 3.0,
        dynamic: false,
        q: None,
        label: "weight".into(),
    };
    s.scout.turns.push(turn_with(vec![g]));
    let mut h = harness(SoundApp::new(Engine::new(s), None, Services::default()));
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Bass · Clean Q").is_some(), "the guide is listed");
    assert!(h.query_by_label_contains("Bass has no Clean Q").is_some(), "with a note");
    assert!(h.state().engine.session().tracks[0].mixer.inserts.is_empty(), "Scout never adds a module");
}

#[test]
fn guides_are_numbered_once_per_answer_across_tracks() {
    let g = |track: &str, lo: f32| Guide::Eq {
        track: track.into(),
        lo_hz: lo,
        hi_hz: lo * 2.0,
        gain_lo_db: -2.0,
        gain_hi_db: -1.0,
        dynamic: false,
        q: None,
        label: "x".into(),
    };
    let module = Guide::Module {
        track: "Bass".into(),
        module: "Comp".into(),
        param: "Ratio".into(),
        current: String::new(),
        range: String::new(),
        reason: String::new(),
    };
    let list = vec![g("Bass", 100.0), module, g("Kick", 300.0), g("Bass", 1000.0)];
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 176.0));
    let rects = guide_rects(&list, rect, freq_to_x);
    let got: Vec<(usize, String)> = rects.iter().map(|(i, _, _, t)| (*i, t.clone())).collect();
    assert_eq!(got, vec![(0, "1  x".to_string()), (2, "2  x".to_string()), (3, "3  x".to_string())]);
    use soundcraft_ui_egui::scout::guides::{colour_for, eq_number};
    assert_eq!((eq_number(&list, 1), eq_number(&list, 3)), (None, Some(2)));
    assert_eq!(rects[2].2, colour_for(2), "the colour follows the number");
}

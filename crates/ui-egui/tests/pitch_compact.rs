//! The Mix window's compact pitch box (spec 2026-11-02 item 1): only vocal or tuned strips show
//! one, the others give that room to their inserts; it is compact by default (green / amber /
//! red / grey), ▸ or a double-click expands it to the full Pitch panel and ▾ folds it back, kept
//! per track in the session.

mod vocal_support;

use egui::Rect;
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_engine::cmd::mix_view::pitch_expanded;
use soundcraft_model::{ChannelFormat, Session, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use soundcraft_ui_egui::vocal_mixer::{COMPACT_H, Level, compact_state, level_of};
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};
use vocal_support::*;

struct Mix {
    ctx: egui::Context,
    app: SoundApp,
    vox: TrackId,
    keys: TrackId,
}

/// Lead Vox (the two-note take at 0.25 s, analysed) beside a non-vocal "Keys" audio track.
fn mix() -> Mix {
    let mut s = Session::default();
    let keys = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Keys"));
    let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let clip = place(&mut s, vox, take2(), i64::from(RATE) / 4);
    let mut e = Engine::new(s);
    soundcraft_engine::vocal::run_until_idle(&mut e, &[clip]).unwrap();
    let mut app = SoundApp::new(e, None, Services::default());
    app.ui.window = MainWindow::Mix;
    let ctx = egui::Context::default();
    let mut m = Mix { ctx, app, vox, keys };
    settle(&m.ctx, &mut m.app);
    m
}

fn at(m: &mut Mix, p: Samples) {
    m.app.engine.session_mut().edit.selection = Range::new(p, p);
    settle(&m.ctx, &mut m.app);
}

fn ctl(m: &Mix, t: TrackId, name: &str) -> Rect {
    control(&m.ctx, &format!("mix-{}-{name}", t.0)).unwrap_or_else(|| panic!("no {name} on track {}", t.0))
}

/// Frames for about half a second of egui time (each frame is 1/60 s): past the colour's hold.
fn wait(m: &mut Mix) {
    for _ in 0..30 {
        frame(&m.ctx, &mut m.app, vec![]);
    }
}

fn mid_of_first_note(m: &Mix) -> Samples {
    let (a, b) = span(&m.app, notes(&m.app, m.vox)[0]);
    (a + b) / 2
}

#[test]
fn vocal_strips_show_the_compact_box_and_the_others_give_its_room_to_inserts() {
    let m = mix();
    let bx = ctl(&m, m.vox, "compact");
    assert!((bx.height() - COMPACT_H).abs() < 0.5, "{}", bx.height());
    assert!(control(&m.ctx, &format!("mix-{}-compact", m.keys.0)).is_none(), "Keys has no pitch box");
    // Keys's chain reaches down over the row the box takes on Lead Vox; the sends line up.
    let (kc, vc) = (ctl(&m, m.keys, "chain"), ctl(&m, m.vox, "chain"));
    assert!(kc.height() >= vc.height() + COMPACT_H, "Keys {} vs Lead Vox {}", kc.height(), vc.height());
    let (ks, vs) = (ctl(&m, m.keys, "sends_ae"), ctl(&m, m.vox, "sends_ae"));
    assert!((ks.min.y - vs.min.y).abs() <= 0.5, "Keys {} vs Lead Vox {}", ks.min.y, vs.min.y);
}

#[test]
fn no_vocal_shown_means_no_row_at_all() {
    let mut m = mix();
    let with = ctl(&m, m.keys, "sends_ae").min.y;
    m.app.run("track.vocal", json!({"tracks": [m.vox.0], "value": false})).unwrap();
    settle(&m.ctx, &mut m.app);
    let without = ctl(&m, m.keys, "sends_ae").min.y;
    assert!(without + COMPACT_H <= with + 0.5, "the row is gone: {without} vs {with}");
    let (kc, vc) = (ctl(&m, m.keys, "chain").height(), ctl(&m, m.vox, "chain").height());
    assert!((vc - kc).abs() <= 0.5, "{vc} vs {kc}");
}

#[test]
fn a_tuned_track_shows_the_box_even_when_not_named_a_vocal() {
    let mut m = mix();
    m.app.run("track.rename", json!({"track": m.vox.0, "name": "Guitar"})).unwrap();
    m.app.run("track.vocal", json!({"tracks": [m.vox.0], "value": false})).unwrap();
    let r = notes(&m.app, m.vox)[0];
    m.app.run("vocal.edit", json!({"notes": [r], "centre": 50})).unwrap();
    m.app.engine.session_mut().tracks.iter_mut().for_each(|t| {
        if t.id == m.vox {
            t.vocal = Some(false);
        }
    });
    settle(&m.ctx, &mut m.app);
    assert!(soundcraft_ui_egui::vocal_mixer::shows_pitch(m.app.engine.session().track(m.vox).unwrap()));
}

#[test]
fn the_colour_says_how_far_off_the_note_is() {
    let mut m = mix();
    let p = mid_of_first_note(&m);
    at(&mut m, p);
    wait(&mut m);
    // The take's first note sings 30 cents flat of A3.
    assert_eq!(text(&m.ctx, &format!("mix-{}-readout", m.vox.0)).as_deref(), Some("A3 \u{2212}30c"));
    assert_eq!(text(&m.ctx, &format!("mix-{}-level", m.vox.0)).as_deref(), Some("off"));
    let (cur, trace) = compact_state(m.app.engine.session(), m.vox, p);
    assert_eq!(level_of(cur.map(|c| c.offset)), Level::Off);
    assert!(!trace.is_empty(), "a trace of the note so far");
    assert!(trace.iter().flatten().all(|(x, _)| (0.0..=1.0).contains(x)));
    // The cents bar grows left (flat) from the centre.
    let bar = ctl(&m, m.vox, "cents_bar");
    let trace_r = ctl(&m, m.vox, "trace");
    assert!(bar.max.x <= trace_r.center().x + 0.5 && bar.width() > 10.0, "{bar:?}");
    // Centred fully, it is in tune; held briefly before the colour turns.
    let r = notes(&m.app, m.vox)[0];
    m.app.run("vocal.edit", json!({"notes": [r], "centre": 100, "drift": 100})).unwrap();
    settle(&m.ctx, &mut m.app);
    assert_eq!(text(&m.ctx, &format!("mix-{}-level", m.vox.0)).as_deref(), Some("off"), "held");
    wait(&mut m);
    assert_eq!(text(&m.ctx, &format!("mix-{}-level", m.vox.0)).as_deref(), Some("in tune"));
    // After the last note: silent, grey.
    at(&mut m, (2.2 * f64::from(RATE)) as Samples);
    wait(&mut m);
    assert_eq!(text(&m.ctx, &format!("mix-{}-level", m.vox.0)).as_deref(), Some("silent"));
    assert_eq!(text(&m.ctx, &format!("mix-{}-readout", m.vox.0)).as_deref(), Some("\u{2014}"));
}

#[test]
fn expand_and_fold_back_are_kept_per_track_in_the_session() {
    let mut m = mix();
    assert!(!pitch_expanded(m.app.engine.session(), m.vox));
    let before = m.app.engine.undo_history().len();
    let r = ctl(&m, m.vox, "expand");
    click(&m.ctx, &mut m.app, r.center());
    assert!(pitch_expanded(m.app.engine.session(), m.vox));
    assert_eq!(m.app.engine.undo_history().len(), before, "a view setting, not an undo step");
    // The full panel: its graph, and the row is the full panel's height on Keys's side too.
    let keys_chain = ctl(&m, m.keys, "chain").height();
    let vox_chain = ctl(&m, m.vox, "chain").height();
    assert!(keys_chain >= vox_chain + soundcraft_ui_egui::vocal_mixer::PITCH_H, "{keys_chain} vs {vox_chain}");
    let fold = ctl(&m, m.vox, "collapse");
    click(&m.ctx, &mut m.app, fold.center());
    assert!(!pitch_expanded(m.app.engine.session(), m.vox));
    // A double-click on the compact box expands it too.
    wait(&mut m);
    let bx = ctl(&m, m.vox, "compact");
    let pt = bx.center();
    for _ in 0..2 {
        frame(&m.ctx, &mut m.app, vec![egui::Event::PointerMoved(pt), button(pt, true, egui::Modifiers::NONE)]);
        frame(&m.ctx, &mut m.app, vec![button(pt, false, egui::Modifiers::NONE)]);
    }
    settle(&m.ctx, &mut m.app);
    assert!(pitch_expanded(m.app.engine.session(), m.vox));
    assert!(!pitch_expanded(m.app.engine.session(), m.keys));
}

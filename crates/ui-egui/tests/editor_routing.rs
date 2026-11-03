//! The wide editors keep the drawer's routing rows: the sidechain picker, the LINK row and the
//! reference rows show above the VST face (Shale's Kit Link status says "pick one under LINK").

mod editor_support;

use editor_support::*;
use egui::vec2;
use egui_kittest::kittest::Queryable;

#[test]
fn shale_in_a_wide_editor_offers_link_and_sidechain() {
    let (mut h, t, m) = open("fs_shale", vec2(1800.0, 1000.0), "dark");
    assert!(h.state().ui.wide_editors, "wide editors are the default");
    let f = frame(&h, t, m);
    let link = h.get_by_label("Off (not linked)").rect();
    assert!(link.max.y <= f.min.y + 0.5, "the LINK row sits above the editor: {link:?} vs {f:?}");
    assert!(h.query_by_label("None (own input)").is_some(), "Shale takes a sidechain: its picker shows");

    h.get_by_label("Off (not linked)").click();
    h.run_steps(3);
    h.get_by_label("Group 1").click();
    h.run_steps(3);
    let s = h.state().engine.session();
    assert_eq!(s.track(t).unwrap().mixer.module(m).unwrap().link_group, Some(1), "mix.module_link ran");
    assert_eq!(h.state().engine.undo_label(), Some("Set Module Link Group"));
}

#[test]
fn levelup_in_a_wide_editor_shows_link_and_reference_rows() {
    let (h, t, m) = open("fs_levelup", vec2(1800.0, 1000.0), "light");
    let f = frame(&h, t, m);
    assert!(h.query_by_label("Off (not linked)").is_some(), "LevelUp links");
    let refs = h.query_all_by_label("None · load a file…").count();
    assert_eq!(refs, 4, "one row per reference slot");
    assert!(h.query_by_label("None (own input)").is_none(), "LevelUp takes no sidechain");
    assert!(f.width() > 100.0);
}

#[test]
fn an_editor_without_routing_rows_starts_at_the_top() {
    let (h, t, m) = open("fs_cleanq", vec2(1800.0, 1000.0), "dark");
    assert!(h.query_by_label("Off (not linked)").is_none());
    let pad = h.get_by_label("Pad strip").rect();
    assert!(frame(&h, t, m).min.y <= pad.min.y + 1.0, "no strip above a plain editor");
}

//! Every leaf of the menu bar, in sessions that give it its context and in ones that don't: it
//! resolves to a command (an engine command or a UI-layer one), and when it is greyed out its
//! hover says why. Twelve UI-layer items were once greyed out for good, with no reason, because the
//! menu asked the engine about ids only the UI knows (New from Template…, Quantize Audio…, Tune
//! Vocal…, Capture MIDI, …); this walk keeps that class of bug from coming back.

use serde_json::json;
use soundcraft_ui_egui::menus::{self, MenuKind, MenuNode};
use soundcraft_ui_egui::{Services, SoundApp};

/// Every command leaf as (shown path, run path).
fn leaves(nodes: &[MenuNode], parent: &str, out: &mut Vec<(String, String)>) {
    for n in nodes {
        let shown = if parent.is_empty() { n.label.clone() } else { format!("{parent} > {}", n.label) };
        if n.children.is_empty() {
            if n.kind == MenuKind::Item {
                out.push((shown, n.path.clone()));
            }
        } else {
            leaves(&n.children, &shown, out);
        }
    }
}

fn all_leaves() -> Vec<(String, String)> {
    let mut v = Vec::new();
    leaves(&menus::tree(), "", &mut v);
    v
}

fn picker() -> Services {
    Services { pick_open: Some(Box::new(|_, _| None)), ..Services::default() }
}

/// The contexts the walk runs in: an empty session, and the demo with an audio, a MIDI and no
/// selection, with something on the clipboard and a vocal track.
fn contexts() -> Vec<(&'static str, SoundApp)> {
    let empty = SoundApp::new(soundcraft_engine::Engine::default(), None, picker());
    let demo = || SoundApp::new(soundcraft_engine::demo::demo_engine(), None, picker());
    let mut audio = demo();
    audio.run("edit.select", json!({"tracks": ["Kick"], "start": {"seconds": 12.0}, "end": {"seconds": 14.0}})).unwrap();
    audio.run("edit.copy", json!({})).unwrap();
    let mut midi = demo();
    midi.run("edit.select", json!({"tracks": ["Keys"], "start": {"seconds": 12.0}, "end": {"seconds": 14.0}})).unwrap();
    let mut clip = demo();
    clip.run("edit.select", json!({"tracks": ["Kick"], "start": {"seconds": 12.0}})).unwrap();
    clip.run("track.new", json!({"name": "Lead Vox"})).unwrap();
    let none = demo();
    vec![("empty", empty), ("audio", audio), ("midi", midi), ("clip", clip), ("none", none)]
}

#[test]
fn every_leaf_resolves_and_says_why_it_is_greyed_out() {
    let extra = menus::ui_aliases();
    let leaves = all_leaves();
    assert!(leaves.len() > 400, "the walk sees the whole menu bar ({} leaves)", leaves.len());
    let mut problems = Vec::new();
    for (name, app) in contexts() {
        for (shown, path) in &leaves {
            let Some(id) = menus::command_for_path(path, &extra) else {
                problems.push(format!("{shown}: runs `{path}`, which resolves to no command"));
                continue;
            };
            if soundcraft_engine::find_command(&id).is_none() && !menus::is_ui_command(&id) && !matches!(id.as_str(), "process.run") {
                problems.push(format!("{shown}: `{id}` is neither an engine nor a UI command"));
            }
            let (enabled, _) = menus::item_state(&app, path, Some(&id));
            let why = menus::availability(&app, &id);
            assert_eq!(enabled, why.is_ok(), "{shown}: the item's state and its availability agree");
            if let Err(why) = why
                && why.trim().is_empty()
            {
                problems.push(format!("{shown} (`{id}`, {name}): greyed out with no reason"));
            }
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}

fn state(app: &SoundApp, label: &str) -> Result<(), String> {
    let (_, path) = all_leaves().into_iter().find(|(shown, _)| shown.ends_with(&format!("> {label}"))).unwrap_or_else(|| panic!("no leaf {label}"));
    let id = menus::command_for_path(&path, &menus::ui_aliases()).unwrap();
    let (enabled, _) = menus::item_state(app, &path, Some(&id));
    let why = menus::availability(app, &id);
    assert_eq!(enabled, why.is_ok());
    why
}

/// The twelve UI-layer items the audit found always greyed out: clickable where they can act,
/// greyed out with a reason where they need something first.
#[test]
fn the_ui_layer_items_are_clickable_in_their_context() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, picker());
    app.run("edit.select", json!({"tracks": ["Kick"], "start": {"seconds": 12.0}, "end": {"seconds": 14.0}})).unwrap();
    for label in [
        "New from Template...",
        "Reference...",
        "Quantize Audio...",
        "Plugin Editors Larger",
        "Plugin Editors Smaller",
        "Capture MIDI",
        "Tune Vocal…",
        "MIDI Cleanup…",
    ] {
        assert_eq!(state(&app, label), Ok(()), "{label}");
    }
    // At actual size already: nothing to do, and the hover says so.
    assert!(state(&app, "Plugin Editors at Actual Size").is_err_and(|w| w.contains("actual size")));
    app.run("ui.mix.editors_larger", json!({})).unwrap();
    assert_eq!(state(&app, "Plugin Editors at Actual Size"), Ok(()));
    // Undo Last Pass and Erase Held Notes act on a MIDI loop recording.
    for label in ["Undo Last Pass", "Erase Held Notes"] {
        assert!(state(&app, label).is_err_and(|w| w.contains("loop recording")), "{label}");
    }
    // Make Harmony… needs a vocal track to harmonise.
    assert!(state(&app, "Make Harmony…").is_err_and(|w| w.contains("vocal track")));
    app.run("track.new", json!({"name": "Lead Vox"})).unwrap();
    assert_eq!(state(&app, "Make Harmony…"), Ok(()));
    // Without the desktop file picker, Import › Reference… cannot ask for its file.
    let web = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    assert!(state(&web, "Reference...").is_err_and(|w| w.contains("file picker")));
}

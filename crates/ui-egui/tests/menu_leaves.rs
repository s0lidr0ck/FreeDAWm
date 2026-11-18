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

/// A command clicked in a menu that fails says why in the status line: a UI-layer command's error
/// used to be dropped (only engine errors set the status), and engine errors read as the reason
/// alone, without "bad parameters for `…`".
#[test]
fn a_failing_menu_command_says_why_in_the_status_line() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    menus::invoke_menu(&mut app, "ui.capture_midi", "Event > More MIDI > Capture MIDI");
    assert!(app.ui.status.starts_with("Nothing was played to capture"), "{}", app.ui.status);
    assert_eq!(menus::readable_error("bad parameters for `edit.space_clips`: select two or more clips"), "Select two or more clips");
    assert_eq!(menus::readable_error("`edit.cut` is not available: make an edit selection first"), "Make an edit selection first");
    assert_eq!(menus::readable_error("APPDATA is not set"), "APPDATA is not set");
}

/// The items the menu audit found failing silently when their context was missing: each is greyed
/// out with a reason, or clicked anyway it changes something or says why in the status line (and
/// leaves no empty undo step).
#[test]
fn an_item_without_its_context_never_fails_silently() {
    const SILENT: &[&str] = &[
        "edit.clear_clip_gain",
        "edit.clear_clip_effects",
        "edit.clear_all_automation",
        "edit.clear_pan_automation",
        "edit.clear_plugin_automation",
        "edit.move_selection_left",
        "edit.halve_selection",
        "edit.double_selection",
        "edit.extend_up_to_members",
        "edit.extend_down_to_members",
        "edit.remove_selection_top",
        "edit.remove_selection_bottom",
        "edit.extend_to_start",
        "edit.nudge_start_earlier",
        "edit.nudge_start_later",
        "edit.nudge_end_earlier",
        "edit.select_track_clips",
        "edit.select_muted_clips",
        "edit.heal",
        "edit.fades_delete",
        "edit.trim_to_close_gaps",
        "edit.automation_trim_to_all",
        "edit.automation_glide_to_all",
        "automation.thin",
        "automation.thin_all",
        "automation.volume_to_clip_gain",
        "edit.automation_coalesce_clip_gain_to_volume",
        "clip.automation_from_track",
        "clip.automation_to_track",
        "clip.automation_clear",
        "edit.snap_next",
        "edit.snap_previous",
        "edit.space_clips",
        "track.split_into_mono",
        "track.bypass_inserts_fj",
        "track.bypass_reverb",
        "track.bypass_delay",
        "track.bypass_modulation",
        "track.mute_sends_fj",
        "track.show_target_playlist",
        "track.toggle_recent_playlist",
        "track.write_midi_rtp",
        "track.set_record_to_input_only",
        "track.coalesce_trim",
        "clip.send_to_back",
        "clip.ungroup",
        "clip.ungroup_all",
        "clip.quantize_to_grid",
        "clip.remove_warp",
        "event.remove_duplicate_notes",
        "event.restore_performance",
        "ui.mix.width.slim",
        "ui.mix.width.wide",
    ];
    let extra = menus::ui_aliases();
    let leaves = all_leaves();
    let demo = || SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let mut keys = demo();
    keys.run("edit.select", json!({"tracks": ["Keys"], "start": {"seconds": 12.0}, "end": {"seconds": 14.0}})).unwrap();
    let mut kick = demo();
    kick.run("edit.select", json!({"tracks": ["Kick"], "start": {"seconds": 12.0}})).unwrap();
    let mut problems = Vec::new();
    for (name, mut app) in [("no selection", demo()), ("Keys range", keys), ("Kick clip", kick)] {
        for id in SILENT {
            let (shown, path) = leaves
                .iter()
                .find(|(_, p)| menus::command_for_path(p, &extra).as_deref() == Some(id))
                .unwrap_or_else(|| panic!("no menu item runs {id}"));
            if menus::availability(&app, id).is_err() {
                continue;
            }
            app.ui.status.clear();
            let before = app.engine.session_arc();
            let steps = app.engine.undo_history().len();
            menus::invoke_menu(&mut app, id, path);
            let changed = *before != *app.engine.session();
            if !changed && app.ui.status.is_empty() {
                problems.push(format!("{shown} ({name}): changed nothing and said nothing"));
            }
            if !changed && app.engine.undo_history().len() != steps {
                problems.push(format!("{shown} ({name}): changed nothing but made an undo step"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));

    // With its context the edit acts, and nothing is said against it: Clear Special › Clip Gain.
    let mut app = demo();
    let clip = app.engine.session().track_by_name("Kick").unwrap().clips()[0].clone();
    app.run("clip.gain", json!({"clip": clip.id.0, "db": -6.0})).unwrap();
    app.run("edit.select", json!({"tracks": ["Kick"], "start": clip.start, "end": clip.end()})).unwrap();
    app.ui.status.clear();
    menus::invoke_menu(&mut app, "edit.clear_clip_gain", "Edit > Clear Special > Clip Gain");
    assert_eq!(app.engine.session().find_clip(clip.id).unwrap().1.gain_db, 0.0);
    assert!(!app.ui.status.contains("changed nothing"), "{}", app.ui.status);
    // Without it, the status line says what it needs.
    menus::invoke_menu(&mut app, "edit.clear_clip_gain", "Edit > Clear Special > Clip Gain");
    assert_eq!(app.ui.status, "Clear Special › Clip Gain changed nothing: select clips with clip gain");
    assert_eq!(app.engine.undo_label(), Some("Clip Gain"), "the empty step is gone; the clearing one stays");
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

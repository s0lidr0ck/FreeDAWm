//! Every user-facing engine command must be reachable from the GUI: through a menu, a shortcut,
//! or a widget that names it in `src/`. Commands that only work from the control channel / CLI /
//! MCP left users stuck (#282: `midi.new_clip` had no UI path, so an empty instrument track could
//! never get a MIDI clip).

use soundcraft_engine::{catalog, cmd::command_specs};
use soundcraft_ui_egui::menus::{self, MenuKind, MenuNode};
use std::collections::HashSet;
use std::path::Path;

/// Known gaps: commands with no menu, shortcut or literal reference in the UI source today.
/// Give each one a UI path and remove it from this list; don't add to it.
const KNOWN_UNREACHED: &[&str] = &[
    "edit.paste_clip_effects",
    "edit.paste_clip_gain",
    "event.midi_note",
    "markers.next",
    "markers.previous",
    "mix.clear_mutes",
    "mix.phase_invert",
    "mix.send_surround_pan",
    "setup.hardware",        // menu opens the Playback Engine window, which doesn't run it
    "setup.playback_engine", // same
    "setup.reset_edit_state",
    "track.pin",
    "track.trim_automation_write",
    "track.vca",
    "transport.pause",
    "transport.status",
    "video.offset",
    "view.sub_counter",
];

/// Commands the 2026-10 menus clean-up took off the menu bar (docs/superpowers/specs/
/// 2026-10-23-menus-cleanup-design.md) and that nothing else in the UI reaches (QuickPunch keeps
/// its shortcut, Link Track and Edit Selection its toolbar button, so they are not listed): they
/// change nothing a user can see yet, so a menu item only misled. They stay for agents and the control channel. Wiring one up (a reader for its
/// flag, a dialog) means giving it a menu-bar line again and removing it from here.
const OFF_THE_MENUS: &[(&str, &str)] = &[
    ("automation.trim", "for control surfaces and agents: the Mix fader in Trim mode calls Engine::automation_trim"),
    ("automation.write_range", "Glide to Current was mis-aliased to it: needs a param and value"),
    ("event.renumber_bars", "a no-op (bars start at 1)"),
    ("event.retrospective_record", "no live MIDI capture buffer feeds it"),
    ("options.auto_spot_clips", "stored flag nothing reads"),
    ("options.calibration_mode", "stored flag nothing reads"),
    ("options.destructive_punch", "stored flag nothing reads"),
    ("options.destructive_record", "stored flag nothing reads"),
    ("options.dynamic_transport", "stored flag nothing reads"),
    ("options.layered_editing", "stored flag nothing reads"),
    ("options.marker_target_follows_selection", "stored flag nothing reads"),
    ("options.markers_follow_edit", "stored flag nothing reads"),
    ("options.midi_live_mode", "stored flag nothing reads"),
    ("options.midi_thru", "stored flag nothing reads"),
    ("options.pre_fader_metering", "stored flag nothing reads"),
    ("options.transport_online", "stored flag nothing reads"),
    ("setup.click_countoff", "shown greyed out (coming soon): needs its own window"),
    ("setup.current_feet_frames", "needs a position and has no dialog"),
    ("setup.current_timecode", "needs a position and has no dialog"),
    ("setup.disk_allocation", "a stored setting with no window"),
    ("setup.external_timecode_offset", "a stored setting with no window; nothing reads it"),
    ("setup.midi_beat_clock", "a stored setting with no window; no clock output reads it"),
    ("setup.midi_input_devices", "shown greyed out (coming soon): needs its own window"),
    ("setup.midi_input_filter", "a stored setting with no window"),
    ("setup.midi_studio", "a stored setting with no window"),
    ("setup.peripherals", "a stored setting with no window"),
    ("setup.preferences", "Setup › Preferences… opens the Preferences window instead; this reads the session's settings for agents"),
    ("setup.transcription_settings", "a stored setting with no window; nothing reads it"),
    ("setup.video_sync_offset", "a stored setting with no window"),
    ("view.automation_composite_playlist", "stored flag nothing reads"),
    ("view.automation_trim_playlist", "stored flag nothing reads"),
    ("view.clip_all_channels", "stored flag nothing reads"),
    ("view.clip_ara_note_overlay", "stored flag nothing reads"),
    ("view.clip_channel_name", "stored flag nothing reads"),
    ("view.clip_copy_move_indicators", "stored flag nothing reads"),
    ("view.clip_overwrite_indicator", "stored flag nothing reads"),
    ("view.clip_processing_state", "stored flag nothing reads"),
    ("view.clip_scene_take", "stored flag nothing reads"),
    ("view.dropped_video_frame_indicator", "stored flag nothing reads"),
    ("view.edit_marker_controls", "stored flag nothing reads"),
    ("view.edit_mic_preamps", "stored flag nothing reads"),
    ("view.edit_object", "stored flag nothing reads"),
    ("view.edit_pinned_tracks", "stored flag nothing reads"),
    ("view.edit_realtime_properties", "stored flag nothing reads"),
    ("view.edit_track_color", "stored flag nothing reads"),
    ("view.marker_folder_lines", "stored flag nothing reads"),
    ("view.marker_ruler_lane_color", "stored flag nothing reads"),
    ("view.marker_track_lane", "stored flag nothing reads"),
    ("view.marker_track_lane_color", "stored flag nothing reads"),
    ("view.midi_editor_clip_effects", "stored flag nothing reads"),
    ("view.midi_input_display", "stored flag nothing reads"),
    ("view.ruler_key_staff", "stored flag nothing reads"),
    ("view.track_transcription_lane", "stored flag nothing reads"),
    ("view.transport_ableton_link", "stored flag nothing reads"),
    ("view.transport_synchronization", "stored flag nothing reads"),
];

/// Position-based aliases of the channel-chain commands: the UI acts by module id, so an alias is
/// reached when the command it stands for is.
const ALIASES: &[(&str, &str)] = &[
    ("mix.insert", "mix.module_add"),
    ("mix.insert_remove", "mix.module_remove"),
    ("mix.insert_move", "mix.module_move"),
    ("mix.insert_bypass", "mix.module_bypass"),
    ("mix.insert_param", "mix.module_param"),
    ("mix.insert_params", "mix.module_params"),
];

fn ui_source() -> String {
    fn walk(dir: &Path, out: &mut String) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push_str(&std::fs::read_to_string(&p).unwrap());
            }
        }
    }
    let mut s = String::new();
    walk(&Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into())).join("src"), &mut s);
    s
}

/// Command ids behind the live leaves of the menu bar actually built by the app (greyed-out
/// "coming soon" items don't count). A spec's own `menu` field is not enough: the bar is built
/// from `menubar.txt`, so a path it doesn't list never shows.
fn menu_commands() -> HashSet<String> {
    fn walk(nodes: &[MenuNode], extra: &[(&str, &str)], out: &mut HashSet<String>) {
        for n in nodes {
            if n.children.is_empty() && n.kind == MenuKind::Item {
                out.extend(catalog::implemented_by(&n.path, extra));
            }
            walk(&n.children, extra, out);
        }
    }
    let mut out = HashSet::new();
    walk(&menus::tree(), &menus::ui_aliases(), &mut out);
    out
}

#[test]
fn every_command_has_a_ui_path() {
    let src = ui_source();
    let in_menus = menu_commands();
    let mut unreached = Vec::new();
    let mut stale = Vec::new();
    for c in command_specs() {
        // Queries and view-only commands are for agents / the UI's own reads.
        if !c.journal || c.view {
            continue;
        }
        let direct = |id: &str| in_menus.contains(id) || src.contains(&format!("\"{id}\""));
        let alias_of = ALIASES.iter().find(|(alias, _)| *alias == c.id).map(|(_, target)| *target);
        let reached = direct(c.id) || c.shortcut.is_some() || alias_of.is_some_and(direct);
        let known = KNOWN_UNREACHED.contains(&c.id) || OFF_THE_MENUS.iter().any(|(id, _)| *id == c.id);
        if !reached && !known {
            unreached.push(c.id);
        } else if reached && known {
            stale.push(c.id);
        }
    }
    assert!(unreached.is_empty(), "commands with no menu, shortcut or UI reference: {unreached:?}");
    assert!(stale.is_empty(), "now reachable, remove from KNOWN_UNREACHED: {stale:?}");
}

#[test]
fn new_midi_clip_is_in_the_clip_menu() {
    let extra = menus::ui_aliases();
    assert_eq!(catalog::implemented_by("Clip > New MIDI Clip", &extra).as_deref(), Some("midi.new_clip"));
    assert!(menu_commands().contains("midi.new_clip"));
    let tree = menus::tree();
    let clip = tree.iter().find(|n| n.label == "Clip").unwrap();
    let first = clip.children.iter().find(|n| n.kind == MenuKind::Item).unwrap();
    assert_eq!(first.label, "New MIDI Clip", "creates rather than edits, so it comes first");
}

#[test]
fn remove_sync_point_sits_in_clip_remove_and_is_greyed_out_without_a_point() {
    let tree = menus::tree();
    let clip = tree.iter().find(|n| n.label == "Clip").unwrap();
    let remove = clip.children.iter().find(|n| n.label == "Remove").unwrap();
    let labels: Vec<&str> = remove.children.iter().map(|n| n.label.as_str()).collect();
    assert_eq!(labels, ["Sync Point", "Warp", "Pitch Shift"]);
    let path = &remove.children[0].path;
    let id = menus::command_for_path(path, &menus::ui_aliases());
    assert_eq!(id.as_deref(), Some("clip.remove_sync_point"));

    let mut app = soundcraft_ui_egui::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, soundcraft_ui_egui::Services::default());
    app.engine.execute("edit.select", &serde_json::json!({"tracks": ["Kick"], "start": {"seconds": 12.0}})).unwrap();
    assert!(!menus::item_state(&app, path, id.as_deref()).0, "the selected Kick clip has no sync point yet");
    app.engine.execute("clip.identify_sync_point", &serde_json::json!({})).unwrap();
    assert!(menus::item_state(&app, path, id.as_deref()).0);
}

#[test]
fn the_window_sync_options_are_check_items_in_window_sync() {
    let tree = menus::tree();
    let window = tree.iter().find(|n| n.label == "Window").unwrap();
    let sync = window.children.iter().find(|n| n.label == "Sync").expect("a Window > Sync submenu");
    let labels: Vec<&str> = sync.children.iter().map(|n| n.label.as_str()).collect();
    assert_eq!(labels, ["Sync Selection", "Scroll to Selected Track", "Follow Folder Collapse", "Sync Track Visibility"]);
    let commands = menu_commands();
    for id in ["window.sync_selection", "window.sync_scroll", "window.sync_folders", "window.sync_visibility"] {
        assert!(menus::UI_COMMANDS.iter().any(|c| c.0 == id), "{id} is a UI command");
        assert!(commands.contains(id), "{id} is reachable from the menu bar");
    }
}

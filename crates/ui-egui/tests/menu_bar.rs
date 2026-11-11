//! The menu bar after the 2026-10 clean-up (docs/superpowers/specs/2026-10-23-menus-cleanup-design.md
//! and its approved mockups): the menus, their top-level rows, nothing dead, one item per action,
//! check marks on the working toggles, and the few owner-approved "coming soon" items.

use serde_json::json;
use soundcraft_ui_egui::menus::{self, MenuKind, MenuNode};
use soundcraft_ui_egui::{Services, SoundApp};
use std::collections::HashMap;

/// Top-level rows per menu, headings and separators not counted: the approved mockups' counts,
/// plus the owner's greyed-out "coming soon" items (Event: Add Key Change…; Setup: Click/Countoff…,
/// Preferences…). The FreeDAWm menu (About, Session Info, Session Audio
/// Health, Quit) is drawn by hand ahead of these.
const ROWS: [(&str, usize); 11] = [
    ("File", 14),
    ("Edit", 27),
    ("View", 11),
    ("Track", 31),
    ("Clip", 23),
    ("Event", 15),
    ("Process", 14),
    ("AI", 11),
    ("Options", 15),
    ("Setup", 9),
    ("Window", 33),
];

/// The only greyed-out items: shown so they are not forgotten (label, hover text).
const GREYED: [(&str, &str); 3] = [("Change Meter...", menus::SOON), ("Add Key Change...", menus::SOON), ("Click/Countoff...", menus::SOON)];

fn rows(n: &MenuNode) -> usize {
    n.children.iter().filter(|c| !matches!(c.kind, MenuKind::Heading | MenuKind::Separator)).count()
}

/// Every leaf, with the shown path of its parent.
fn leaves(nodes: &[MenuNode], parent: &str, out: &mut Vec<(String, MenuNode)>) {
    for n in nodes {
        if n.children.is_empty() {
            out.push((parent.to_string(), MenuNode { label: n.label.clone(), path: n.path.clone(), kind: n.kind.clone(), children: Vec::new() }));
        } else {
            let shown = if parent.is_empty() { n.label.clone() } else { format!("{parent} > {}", n.label) };
            leaves(&n.children, &shown, out);
        }
    }
}

fn all_leaves() -> Vec<(String, MenuNode)> {
    let mut v = Vec::new();
    leaves(&menus::tree(), "", &mut v);
    v
}

fn demo_app() -> SoundApp {
    SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default())
}

#[test]
fn the_menus_and_their_top_level_rows_match_the_mockups() {
    let tree = menus::tree();
    let names: Vec<&str> = tree.iter().map(|n| n.label.as_str()).collect();
    assert_eq!(names, ROWS.map(|(m, _)| m), "menu bar order (AI sits between Process and Options)");
    for (menu, want) in ROWS {
        let n = tree.iter().find(|n| n.label == menu).unwrap();
        assert_eq!(rows(n), want, "{menu}: top-level rows");
        assert!(n.children.first().is_some_and(|c| c.kind == MenuKind::Heading), "{menu} opens with a heading");
    }
}

#[test]
fn every_item_runs_something_except_the_greyed_out_ones() {
    let extra = menus::ui_aliases();
    let mut greyed = Vec::new();
    for (parent, n) in all_leaves() {
        let shown = format!("{parent} > {}", n.label);
        match n.kind {
            MenuKind::Item => {
                assert!(menus::command_for_path(&n.path, &extra).is_some(), "{shown} (runs `{}`) maps to no command", n.path);
            }
            MenuKind::Soon(text) => greyed.push((n.label.clone(), text)),
            MenuKind::Special(name) => assert!(matches!(name, "instrument" | "recent"), "{shown}: unknown special `{name}`"),
            MenuKind::Heading | MenuKind::Separator => {}
        }
    }
    let mut want: Vec<(String, &str)> = GREYED.iter().map(|(l, t)| (l.to_string(), *t)).collect();
    want.sort();
    greyed.sort();
    assert_eq!(greyed, want);
}

/// Their path is still the catalog path of the command they will run, so wiring one up means
/// only dropping `= soon` from its `menubar.txt` line (and giving it its window).
#[test]
fn greyed_out_items_say_why() {
    for (parent, n) in all_leaves() {
        if let MenuKind::Soon(text) = n.kind {
            assert!(text.starts_with("Coming soon"), "{parent} > {}: {text}", n.label);
        }
    }
}

/// Ids whose menu item names its parameter in the label (a ruler, a width, a rating…).
const PARAMETERISED: [&str; 8] = [
    "view.ruler",
    "view.main_counter",
    "options.scrolling",
    "options.solo_mode",
    "clip.rating",
    "track.change_width",
    "view.mix_section",
    "process.run",
];

#[test]
fn each_action_has_one_item_and_dead_ones_are_gone() {
    let extra = menus::ui_aliases();
    let mut seen: HashMap<String, String> = HashMap::new();
    for (parent, n) in all_leaves() {
        if n.kind != MenuKind::Item {
            continue;
        }
        let Some(id) = menus::command_for_path(&n.path, &extra) else { continue };
        let key = if PARAMETERISED.contains(&id.as_str()) { format!("{id} {}", n.path) } else { id.clone() };
        let shown = format!("{parent} > {}", n.label);
        // Track › Color… opens the color palette that Window › More lists too.
        if id != "window.color_palette"
            && let Some(first) = seen.insert(key, shown.clone())
        {
            panic!("{shown} and {first} both run `{id}`");
        }
    }
    // The duplicates the clean-up merged, and the items that did nothing.
    for dead in [
        "file.send_to_sibelius",
        "file.get_info",
        "track.scroll_to",
        "event.move_song_start",
        "event.renumber_bars",
        "event.retrospective_record",
        "event.tempo_constant",
        "event.tempo_linear",
        "event.tempo_scale",
        "automation.write_range",
        "options.quickpunch",
        "options.trackpunch",
        "options.layered_editing",
        "options.midi_thru",
        "view.edit_object",
        "view.clip_channel_name",
        "view.mix_slim",
        "window.arrange_cascade",
        "window.hide_floating",
        "window.midi_front",
        "window.workspace_sounds",
        "window.clip_list_view",
        "setup.disk_allocation",
        "setup.midi_studio",
    ] {
        assert!(!seen.keys().any(|k| k == dead || k.starts_with(&format!("{dead} "))), "`{dead}` is still on the menu bar");
    }
    for gone in ["View > Mix Window Views > EQ Curve", "View > Mix Window Views > Meters and Faders", "Options > Solo Mode > Latch"] {
        assert!(!all_leaves().iter().any(|(_, n)| n.path == gone), "{gone} is still shown");
    }
}

#[test]
fn items_ending_in_dots_open_a_dialog_or_window() {
    let extra = menus::ui_aliases();
    let mut at_once = Vec::new();
    for (parent, n) in all_leaves() {
        if n.kind != MenuKind::Item || !menus::opens_window(&n.label) {
            continue;
        }
        let id = menus::command_for_path(&n.path, &extra).unwrap_or_default();
        if !menus::opens_dialog(&id, &n.path) {
            at_once.push(format!("{parent} > {} (`{id}`)", n.label));
        }
    }
    assert!(at_once.is_empty(), "these act at once: drop their dots: {at_once:#?}");
}

#[test]
fn the_ai_menu_holds_every_ml_tool() {
    let tree = menus::tree();
    let ai = tree.iter().find(|n| n.label == "AI").unwrap();
    let shown: Vec<&str> = ai.children.iter().filter(|c| rows_kind(c)).map(|c| c.label.as_str()).collect();
    assert_eq!(
        shown,
        [
            "Separate Stems…",
            "Make Karaoke…",
            "Split Drums…",
            "Clean Vocal…",
            "Clean Vocal",
            "Tune Vocal…",
            "Tune Vocal",
            "Convert to MIDI…",
            "Detect Chords…",
            "Transcribe Lyrics…",
            "MIDI Cleanup…"
        ]
    );
    let extra = menus::ui_aliases();
    let clean = ai.children.iter().find(|c| c.label == "Clean Vocal").unwrap();
    let ids: Vec<String> = clean.children.iter().filter_map(|c| menus::command_for_path(&c.path, &extra)).collect();
    assert_eq!(ids, ["clip.clean_toggle", "clip.clean_mix", "clip.clean_commit", "clip.clean_remove"]);
    let tune = ai.children.iter().find(|c| c.label == "Tune Vocal").unwrap();
    let ids: Vec<String> = tune.children.iter().filter_map(|c| menus::command_for_path(&c.path, &extra)).collect();
    assert_eq!(ids, ["vocal.analyse", "vocal.toggle", "vocal.note_mode", "vocal.fit_to_lead", "ui.vocal_harmony", "vocal.commit", "vocal.remove"]);
    let track = tree.iter().find(|n| n.label == "Track").unwrap();
    assert!(!track.children.iter().any(|c| c.label.contains("Stems") || c.label.contains("Karaoke")), "the ML tools left Track");
}

fn rows_kind(n: &MenuNode) -> bool {
    !matches!(n.kind, MenuKind::Heading | MenuKind::Separator)
}

#[test]
fn shown_paths_resolve_like_catalog_paths() {
    let extra = menus::ui_aliases();
    assert_eq!(menus::source_path("Window > More > Search"), "Window > Pro Tools Search");
    assert_eq!(menus::command_for_path("Window > More > Search", &extra).as_deref(), Some("window.search"));
    assert_eq!(menus::command_for_path("Track > Rename...", &extra).as_deref(), Some("track.rename"));
    assert_eq!(menus::command_for_path("File > Export > Stems...", &extra).as_deref(), Some("file.bounce_stems"));
    assert_eq!(menus::menu_location("window.track_list").as_deref(), Some("Window"));
    assert_eq!(menus::menu_location("track.freeze").as_deref(), Some("Track"));
    assert_eq!(menus::menu_location("ui.ml.separate").as_deref(), Some("AI"));
    // Shortcuts stay next to their items.
    let id = menus::command_for_path("Track > New...", &extra).unwrap();
    assert!(menus::shortcut_for(&id).is_some(), "Track › New… keeps its shortcut");
}

/// How a toggle's check mark answers a click: it flips, or (a radio item) it comes on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Answer {
    Flips,
    ComesOn,
}

fn answer(id: &str, label: &str) -> Option<Answer> {
    const UI_TOGGLES: [&str; 33] = [
        "window.mix_separate",
        "window.vocal_separate",
        "window.midi_editor_separate",
        "window.transport",
        "window.big_counter",
        "window.memory_locations",
        "window.automation",
        "window.undo_history",
        "window.clip_list",
        "window.track_list",
        "window.clip_effects",
        "window.plugin_browser",
        "window.midi_editor",
        "window.score",
        "window.event_list",
        "window.midi_keyboard",
        "window.studio",
        "window.scout",
        "window.configurations",
        "window.config_auto_update",
        "window.video",
        "window.video_universe",
        "window.universe",
        "window.search",
        "window.renderer",
        "window.metadata",
        "window.color_palette",
        "window.ui_customization",
        "window.task_manager",
        "window.disk_usage",
        "window.system_usage",
        "window.narrow_mix",
        "ui.mix.wide_editors",
    ];
    if menus::opens_window(label) {
        return None;
    }
    if UI_TOGGLES.contains(&id) || matches!(id, "clip.edit_lock" | "clip.time_lock" | "ui.mix.slim_editors") {
        return Some(Answer::Flips);
    }
    if matches!(id, "window.mix" | "window.edit" | "window.workspace" | "options.scrolling" | "options.solo_mode" | "view.main_counter")
        || id.starts_with("view.waveform_p")
        || id.starts_with("view.clip_time_")
    {
        return Some(Answer::ComesOn);
    }
    if id == "view.mix_section" {
        return (!matches!(label, "All" | "Minimal")).then_some(Answer::Flips);
    }
    if id.starts_with("view.ruler") && !matches!(id, "view.ruler_all" | "view.ruler_minimal") {
        return Some(Answer::Flips);
    }
    let spec = soundcraft_engine::find_command(id)?;
    spec.params.starts_with("{value?: bool}").then_some(Answer::Flips)
}

#[test]
fn working_toggles_show_a_check_mark() {
    let mut app = demo_app();
    // A clip to lock and unlock.
    app.engine.execute("edit.select", &json!({"tracks": ["Kick"], "start": {"seconds": 12.0}})).unwrap();
    let extra = menus::ui_aliases();
    let mut tried = 0;
    for (parent, n) in all_leaves() {
        if n.kind != MenuKind::Item {
            continue;
        }
        let Some(id) = menus::command_for_path(&n.path, &extra) else { continue };
        let Some(answer) = answer(&id, &n.label) else { continue };
        let shown = format!("{parent} > {}", n.label);
        let (enabled, before) = menus::item_state(&app, &n.path, Some(&id));
        if !enabled {
            // Slim Plugin Editors waits for Plugin Editors as in the VST, which runs first.
            assert_eq!(id, "ui.mix.slim_editors", "{shown} is greyed out in the demo session");
            continue;
        }
        menus::invoke_menu(&mut app, &id, &n.path);
        let after = menus::item_state(&app, &n.path, Some(&id)).1;
        match answer {
            Answer::Flips => {
                assert_ne!(before, after, "{shown} (`{id}`): the check mark doesn't follow the toggle");
                menus::invoke_menu(&mut app, &id, &n.path);
                assert_eq!(menus::item_state(&app, &n.path, Some(&id)).1, before, "{shown} (`{id}`) toggles back");
            }
            Answer::ComesOn => assert!(after, "{shown} (`{id}`) is checked once chosen"),
        }
        tried += 1;
    }
    assert!(tried >= 110, "only {tried} toggles were tried");
}

#[test]
fn freeze_reads_unfreeze_for_a_frozen_selected_track() {
    let mut app = demo_app();
    let extra = menus::ui_aliases();
    let freeze = all_leaves().into_iter().find(|(p, n)| p == "Track" && n.label == "Freeze").map(|(_, n)| n).unwrap();
    let id = menus::command_for_path(&freeze.path, &extra).unwrap();
    assert_eq!(id, "track.freeze");
    app.engine.execute("edit.select", &json!({"tracks": ["Kick"]})).unwrap();
    assert_eq!(menus::live_label(&app, Some(&id)).as_deref(), Some("Freeze"));
    menus::invoke_menu(&mut app, &id, &freeze.path);
    assert!(app.engine.session().track_by_name("Kick").unwrap().frozen_audio().is_some(), "Kick froze");
    assert_eq!(menus::live_label(&app, Some(&id)).as_deref(), Some("Unfreeze"));
    // Another, unfrozen track in the selection: Freeze would freeze it, so the label says Freeze.
    app.engine.execute("edit.select", &json!({"tracks": ["Kick", "Snare"]})).unwrap();
    assert_eq!(menus::live_label(&app, Some(&id)).as_deref(), Some("Freeze"));
    app.engine.execute("edit.select", &json!({"tracks": ["Kick"]})).unwrap();
    menus::invoke_menu(&mut app, &id, &freeze.path);
    assert!(app.engine.session().track_by_name("Kick").unwrap().frozen_audio().is_none(), "Unfreeze thawed Kick");
    assert_eq!(menus::live_label(&app, Some(&id)).as_deref(), Some("Freeze"));
}

#[test]
fn pre_post_roll_opens_its_dialog_and_the_shortcut_still_toggles() {
    let mut app = demo_app();
    let extra = menus::ui_aliases();
    let id = menus::command_for_path("Options > Pre/Post-Roll", &extra).unwrap();
    menus::invoke_menu(&mut app, &id, "Options > Pre/Post-Roll");
    assert_eq!(app.dialogs.open_name(), Some("pre_post_roll"));
    app.dialogs.open = None;
    assert!(!menus::invoke_shortcut_dialog(&mut app, &id), "Cmd+K runs the toggle, no dialog");
}

#[test]
fn rename_opens_on_the_selected_track_and_greys_out_without_one() {
    let mut app = demo_app();
    let extra = menus::ui_aliases();
    let id = menus::command_for_path("Track > Rename...", &extra).unwrap();
    app.engine.execute("edit.select", &json!({"tracks": []})).unwrap();
    assert!(!menus::item_state(&app, "Track > Rename...", Some(&id)).0);
    app.engine.execute("edit.select", &json!({"tracks": ["Bass"]})).unwrap();
    assert!(menus::item_state(&app, "Track > Rename...", Some(&id)).0);
    menus::invoke_menu(&mut app, &id, "Track > Rename...");
    assert_eq!(app.dialogs.open_name(), Some("rename_track"));
}

/// Review I1: an `= @id` line runs that id as it stands, so a typo or a renamed command would ship
/// a dead, unexplained item. Every `@` id must be an engine command or a UI command.
#[test]
fn every_at_id_in_menubar_names_a_real_command() {
    let mut app = demo_app();
    let mut unknown = Vec::new();
    for line in menus::layout() {
        let menus::Target::Id(id) = line.target else { continue };
        let known = soundcraft_engine::find_command(id).is_some()
            || menus::UI_COMMANDS.iter().any(|c| c.0 == id)
            || menus::run_ui_command(&mut app, id, &json!({"value": false})).is_some();
        if !known {
            unknown.push(format!("{} = @{id}", line.display));
        }
    }
    assert!(unknown.is_empty(), "menubar.txt names commands that don't exist: {unknown:#?}");
}

/// Every `menubar.txt` line parses into one of the documented shapes (review M10): a typo in the
/// markers would otherwise end up inside a label or a path.
#[test]
fn every_menubar_line_has_a_known_shape() {
    for line in menus::layout() {
        let d = line.display;
        assert!(d.split(" > ").count() >= 2, "`{d}`: needs `Menu > Item`");
        assert!(!d.contains('[') && !d.contains(" = ") && !d.contains('@'), "`{d}`: a marker ended up in the shown path");
        match line.target {
            menus::Target::Path(p) => {
                assert!(p.contains(" > ") && !p.contains('[') && !p.starts_with("soon"), "`{d}`: odd target `{p}`");
            }
            menus::Target::Id(id) => assert!(!id.is_empty() && !id.contains(' '), "`{d}`: odd id `{id}`"),
            menus::Target::Soon(text) => assert!(text.starts_with("Coming soon"), "`{d}`: {text}"),
            menus::Target::Special(name) => assert!(matches!(name, "instrument" | "recent"), "`{d}`: unknown `!{name}`"),
        }
    }
}

#[test]
fn greyed_items_are_refused_by_path() {
    assert_eq!(menus::soon_reason("Setup > Click/Countoff..."), Some(menus::SOON));
    // MIDI Input Devices… has its window now (hardware MIDI input).
    assert_eq!(menus::soon_reason("Setup > MIDI Input Devices..."), None);
    assert_eq!(menus::soon_reason("Setup > Preferences..."), None);
    // Change Velocity… has its dialog now, under More MIDI and its catalog path.
    assert_eq!(menus::soon_reason("Event > MIDI Operations > Change Velocity..."), None);
    assert_eq!(menus::soon_reason("AI > MIDI Cleanup…"), None);
    assert_eq!(menus::soon_reason("Setup > Session"), None);
    assert_eq!(menus::soon_reason("Event > MIDI Operations > Change Duration..."), None);
}

#[test]
fn hide_hides_even_when_the_selection_is_hidden_already() {
    let mut app = demo_app();
    let extra = menus::ui_aliases();
    let id = menus::command_for_path("Track > Hide", &extra).unwrap();
    app.engine.execute("edit.select", &json!({"tracks": ["Bass"]})).unwrap();
    let hidden = |app: &SoundApp| app.engine.session().track_by_name("Bass").unwrap().hidden;
    menus::invoke_menu(&mut app, &id, "Track > Hide");
    assert!(hidden(&app));
    menus::invoke_menu(&mut app, &id, "Track > Hide");
    assert!(hidden(&app), "a second Hide must not unhide");
}

fn harness(app: SoundApp) -> egui_kittest::Harness<'static, SoundApp> {
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

fn click_ok(h: &mut egui_kittest::Harness<'static, SoundApp>) {
    use egui_kittest::kittest::Queryable;
    h.get_by_role_and_label(egui::accesskit::Role::Button, "OK").click();
    h.run_steps(3);
}

#[test]
fn pre_post_roll_ok_sets_both_amounts_and_the_switch() {
    let mut app = demo_app();
    menus::invoke_menu(&mut app, "options.pre_post_roll", "Options > Pre/Post-Roll");
    let Some(soundcraft_ui_egui::dialogs::Dialog::PrePostRoll { on, pre, post }) = &mut app.dialogs.open else {
        panic!("Pre/Post-Roll… did not open its dialog");
    };
    *on = true;
    *pre = 1.5;
    *post = 0.25;
    let mut h = harness(app);
    click_ok(&mut h);
    let s = h.state().engine.session();
    assert!(s.edit.pre_post_roll);
    assert_eq!(s.edit.pre_roll, s.sample_rate.samples(1.5));
    assert_eq!(s.edit.post_roll, s.sample_rate.samples(0.25));
    assert!(h.state().dialogs.open.is_none());
}

#[test]
fn rename_ok_renames_the_selected_track() {
    let mut app = demo_app();
    app.engine.execute("edit.select", &json!({"tracks": ["Bass"]})).unwrap();
    menus::invoke_menu(&mut app, "track.rename", "Track > Rename...");
    let Some(soundcraft_ui_egui::dialogs::Dialog::RenameTrack { name, .. }) = &mut app.dialogs.open else {
        panic!("Rename… did not open its dialog");
    };
    assert_eq!(name, "Bass");
    *name = "Low End".into();
    let mut h = harness(app);
    click_ok(&mut h);
    assert!(h.state().engine.session().track_by_name("Low End").is_some());
}

#[test]
fn clean_vocal_amount_ok_sets_the_cleaned_share() {
    let mut app = demo_app();
    // Give one of Bass's clips a cleaned alternate, as Clean Vocal would.
    let (clip_id, original) = {
        let s = app.engine.session();
        let t = s.track_by_name("Bass").unwrap();
        let c = t.clips().first().unwrap();
        (c.id, c.source().unwrap())
    };
    let s = app.engine.session_mut();
    let rate = s.sample_rate.hz();
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: rate, channels: vec![vec![0.0; 4_800]] };
    let cleaned = soundcraft_engine::io::add_source(s, "Bass-clean", buf, None, soundcraft_audio_io::FileFormat::Wav);
    let clip = s.find_clip_mut(clip_id).unwrap();
    clip.clean = Some(soundcraft_model::CleanAlt { source: cleaned, original, origin: 0, mix: 0.4, bypass: false });
    app.engine.execute("edit.select", &json!({"clips": [clip_id.0]})).unwrap();

    let extra = menus::ui_aliases();
    let id = menus::command_for_path("AI > Clean Vocal > Amount...", &extra).unwrap();
    assert!(menus::item_state(&app, "AI > Clean Vocal > Amount...", Some(&id)).0, "enabled on a cleaned clip");
    menus::invoke_menu(&mut app, &id, "AI > Clean Vocal > Amount...");
    let Some(soundcraft_ui_egui::dialogs::Dialog::Number { value, .. }) = &mut app.dialogs.open else {
        panic!("Amount… did not open its dialog");
    };
    assert_eq!(*value, 40.0, "starts at the clip's amount");
    *value = 75.0;
    let mut h = harness(app);
    click_ok(&mut h);
    let mix = h.state().engine.session().find_clip(clip_id).and_then(|(_, c)| c.clean_alt().map(|a| a.mix)).unwrap();
    assert!((mix - 0.75).abs() < 1e-6, "{mix}");
}

#[test]
fn midi_cleanup_opens_the_midi_editor_with_its_cleanup_panel() {
    let mut app = demo_app();
    app.ui.show_midi_editor = false;
    app.midi.show_cleanup = false;
    menus::run_ui_command(&mut app, "ui.midi_cleanup", &serde_json::json!({})).unwrap().unwrap();
    assert!(app.ui.show_midi_editor && app.midi.show_cleanup);
}

#[test]
fn tune_vocal_opens_the_vocal_editor() {
    let mut app = demo_app();
    app.ui.show_vocal_editor = false;
    menus::run_ui_command(&mut app, "ui.vocal_editor", &serde_json::json!({})).unwrap().unwrap();
    assert!(app.ui.show_vocal_editor);
    // On a clip: the editor follows its track.
    let (track, clip) = {
        let s = app.engine.session();
        let t = s.tracks.iter().find(|t| t.clips().iter().any(|c| c.is_audio())).unwrap();
        (t.id, t.clips().iter().find(|c| c.is_audio()).unwrap().id)
    };
    menus::run_ui_command(&mut app, "ui.vocal_editor", &serde_json::json!({"clip": clip.0})).unwrap().unwrap();
    assert_eq!(app.vocal.track, Some(track));
    let extra = menus::ui_aliases();
    assert_eq!(menus::command_for_path("Window > Vocal Editor", &extra).as_deref(), Some("window.vocal_editor"));
}

#[test]
fn the_first_item_is_the_wordmark_named_freedawm_and_opens_about() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(demo_app());
    h.get_by_role_and_label(egui::accesskit::Role::Button, "FreeDAWm").click();
    h.run_steps(3);
    h.get_by_label("About FreeDAWm").click();
    h.run_steps(3);
    assert!(h.state().ui.show_about, "About FreeDAWm opens the About box");
}

/// The wordmark is 80 % of the menu text's height, centred on the menu bar's text line.
#[test]
fn the_wordmark_is_four_fifths_of_the_menu_text_height() {
    use egui_kittest::kittest::Queryable;
    assert_eq!(menus::wordmark_size(15.0, 5.12), egui::vec2(12.0 * 5.12, 12.0));
    let h = harness(demo_app());
    let text_h = h.ctx.global_style().text_styles.get(&egui::TextStyle::Button).map(|f| h.ctx.fonts_mut(|fonts| fonts.row_height(f))).unwrap();
    let spacing = h.ctx.global_style().spacing.clone();
    let wordmark = h.get_by_role_and_label(egui::accesskit::Role::Button, "FreeDAWm").rect();
    // A button is never shorter than the interact height: with a small menu font (egui's own,
    // where the system UI font isn't found, as on Linux) that is what sets the height.
    let want = (menus::WORDMARK_SCALE * text_h + 2.0 * spacing.button_padding.y).max(spacing.interact_size.y);
    assert!((wordmark.height() - want).abs() < 1.0, "{wordmark:?}, text {text_h}, want {want}");
    // Centred on the line of the menu titles next to it.
    let file = h.get_by_role_and_label(egui::accesskit::Role::Button, "File").rect();
    assert!((wordmark.center().y - file.center().y).abs() < 1.0, "{wordmark:?} vs {file:?}");
}

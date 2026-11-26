//! The Vocal Editor header's MIDI menu: Notes to MIDI Track and Export Notes as MIDI File...,
//! greyed out without detected notes, acting on the selected notes when there are any.

mod vocal_support;

use egui::Id;
use serde_json::json;
use soundcraft_model::{ClipContent, TrackKind};
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::vocal_editor::NO_NOTES_FOR_MIDI;
use vocal_support::*;

fn midi_enabled(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new("soundcraft-vocal-midi-enabled"))).expect("the MIDI menu was drawn")
}

/// Opens the MIDI menu and clicks `item` in it.
fn choose(ctx: &egui::Context, app: &mut soundcraft_ui_egui::SoundApp, item: &str) {
    click(ctx, app, control(ctx, "midi").expect("the header's MIDI menu").center());
    let at = control(ctx, item).unwrap_or_else(|| panic!("{item} in the MIDI menu")).center();
    click(ctx, app, at);
}

#[test]
fn notes_to_midi_track_from_the_header() {
    let Setup { ctx, mut app, track, .. } = setup();
    assert!(midi_enabled(&ctx));
    let n = notes(&app, track).len();
    assert_eq!(n, 2);
    let tracks = app.engine.session().tracks.len();
    choose(&ctx, &mut app, "midi-to-track");
    let s = app.engine.session();
    assert_eq!(s.tracks.len(), tracks + 1, "{}", app.ui.status);
    let i = s.track_index(track).unwrap();
    let made = &s.tracks[i + 1];
    assert_eq!((made.name.as_str(), made.kind), ("Lead Vox MIDI", TrackKind::Instrument), "under the vocal");
    let ClipContent::Midi { sequence } = &made.clips()[0].content else { panic!("a MIDI clip") };
    // 220 Hz −30 c and 247 Hz +20 c: A3 and B3 (MIDI 57, 59).
    assert_eq!(sequence.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![57, 59]);
    assert_eq!(app.engine.undo_label(), Some("Notes to MIDI Track"));
    assert!(app.ui.status.contains("2 notes copied to Lead Vox MIDI"), "{}", app.ui.status);
}

#[test]
fn the_selected_notes_only_and_the_export_prompt() {
    let Setup { ctx, mut app, track, .. } = setup();
    let second = notes(&app, track)[1];
    app.vocal.selected.insert(second);
    settle(&ctx, &mut app);
    let p = soundcraft_ui_egui::vocal_editor::midi_params(&app, track);
    assert_eq!(p, json!({"track": track.0, "notes": [{"clip": second.clip.0, "note": second.note.0}]}));
    choose(&ctx, &mut app, "midi-export");
    match &app.dialogs.open {
        Some(Dialog::PathPrompt { cmd, path, .. }) => {
            assert_eq!(cmd, "vocal.export_midi");
            assert!(path.ends_with("Lead Vox notes.mid"), "{path}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(app.dialogs.path_extra, Some(("vocal.export_midi".to_string(), p)), "the track and notes go with the prompt");
    // The prompt's OK runs the command with them: as it would, straight to a file.
    let dir = std::env::temp_dir().join(format!("sc-vocal-midi-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("one.mid");
    let mut params = app.dialogs.path_extra.clone().unwrap().1;
    params["path"] = json!(file.to_string_lossy());
    let r = app.run("vocal.export_midi", params).unwrap();
    assert_eq!(r["notes"], 1, "{r}");
    assert!(std::fs::metadata(&file).is_ok_and(|m| m.len() > 20));
    let _ = std::fs::remove_dir_all(dir);
    // From the menu bar, the prompt carries nothing over.
    assert!(app.dialogs.open_for_command(&app.engine, "vocal.export_midi"));
    assert_eq!(app.dialogs.path_extra, None);
}

#[test]
fn greyed_out_without_detected_notes() {
    // An analysed clip of silence: nothing sung.
    let Setup { ctx, mut app, track, .. } = setup_with(vec![(vec![0.0; RATE as usize], 0)], true);
    assert!(notes(&app, track).is_empty());
    assert!(!midi_enabled(&ctx));
    let tracks = app.engine.session().tracks.len();
    click(&ctx, &mut app, control(&ctx, "midi").expect("the menu is shown, greyed out").center());
    assert!(control(&ctx, "midi-to-track").is_none(), "a greyed-out menu does not open");
    assert_eq!(app.engine.session().tracks.len(), tracks);
    assert!(NO_NOTES_FOR_MIDI.starts_with("No detected notes"));
    // The menu bar's items say why too.
    let why = soundcraft_ui_egui::menus::availability(&app, "vocal.to_midi_track").unwrap_err();
    assert!(why.contains("no detected notes"), "{why}");
    assert!(soundcraft_ui_egui::menus::availability(&app, "vocal.export_midi").is_err());
}

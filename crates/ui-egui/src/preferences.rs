//! Setup › Preferences…: the settings people look for in one place, in five tabs (Display,
//! Operation, Editing, Mixing, Metering), as Pro Tools and Studio One group them. Every control
//! runs the command its menu item runs (or sets the same UI state the Appearance window does), so
//! the menus, the toolbar and this window always agree. Settings saved with the session are under
//! "This session"; the rest belong to this computer (`ui.json`).

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Ui, vec2};
use serde_json::{Value, json};

/// The tabs, in order: (id, label).
pub const TABS: [(&str, &str); 5] =
    [("display", "Display"), ("operation", "Operation"), ("editing", "Editing"), ("mixing", "Mixing"), ("metering", "Metering")];

/// Peak hold at or above this many seconds holds until cleared (a click on the meter).
pub const HOLD_UNTIL_CLEARED: f32 = 1000.0;

/// The meter ballistics Preferences › Metering sets: release (dB a second) and peak hold
/// (seconds; 0 = none; [`HOLD_UNTIL_CLEARED`] = until a click on the meter).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ballistics {
    pub release_db_s: f32,
    pub hold_secs: f32,
}

impl Ballistics {
    pub fn of(ui: &crate::UiState) -> Ballistics {
        let release = if ui.meter_release_db_s.is_finite() { ui.meter_release_db_s.clamp(3.0, 120.0) } else { DEFAULT_RELEASE };
        let hold = if ui.meter_hold_secs.is_finite() { ui.meter_hold_secs.clamp(0.0, HOLD_UNTIL_CLEARED) } else { DEFAULT_HOLD };
        Ballistics { release_db_s: release, hold_secs: hold }
    }
}

/// Meter release, dB a second (Pro Tools' "Medium" sits near it).
pub const DEFAULT_RELEASE: f32 = 26.0;
/// Peak hold, seconds.
pub const DEFAULT_HOLD: f32 = 2.0;

/// The release choices: (label, dB a second).
const RELEASES: [(&str, f32); 4] = [("Slow (12 dB/s)", 12.0), ("Medium (26 dB/s)", 26.0), ("Fast (48 dB/s)", 48.0), ("Very fast (96 dB/s)", 96.0)];
/// The peak hold choices: (label, seconds).
const HOLDS: [(&str, f32); 5] = [("None", 0.0), ("1 second", 1.0), ("2 seconds", 2.0), ("3 seconds", 3.0), ("Until cleared", HOLD_UNTIL_CLEARED)];

/// UI commands handled here: `window.preferences {value?, tab?}`, `ui.meter_ballistics
/// {release_db_s?, hold_secs?}`, `ui.copy_imported_audio {value?}` (this computer's choice; the
/// engine's `setup.copy_imported_audio` follows it).
pub fn run(app: &mut SoundApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "window.preferences" => {
            if let Some(tab) = p.get("tab").and_then(Value::as_str) {
                let Some((id, _)) = TABS.iter().find(|(id, label)| id.eq_ignore_ascii_case(tab) || label.eq_ignore_ascii_case(tab)) else {
                    return Some(Err(format!("unknown tab `{tab}` (display, operation, editing, mixing or metering)")));
                };
                app.ui.preferences_tab = (*id).to_string();
                app.ui.show_preferences = p.get("value").and_then(Value::as_bool).unwrap_or(true);
            } else {
                app.ui.show_preferences = p.get("value").and_then(Value::as_bool).unwrap_or(!app.ui.show_preferences);
            }
            Ok(json!({"value": app.ui.show_preferences, "tab": current_tab(app)}))
        }
        "ui.meter_ballistics" => {
            let num = |k: &str| p.get(k).and_then(Value::as_f64).map(|v| v as f32).filter(|v| v.is_finite());
            if let Some(r) = num("release_db_s") {
                app.ui.meter_release_db_s = r.clamp(3.0, 120.0);
            }
            if let Some(h) = num("hold_secs") {
                app.ui.meter_hold_secs = h.clamp(0.0, HOLD_UNTIL_CLEARED);
            }
            let b = Ballistics::of(&app.ui);
            Ok(json!({"release_db_s": b.release_db_s, "hold_secs": b.hold_secs}))
        }
        // The engine's `setup.copy_imported_audio` too: in the app it is this computer's setting.
        "ui.copy_imported_audio" | "setup.copy_imported_audio" => {
            let on = p.get("value").and_then(Value::as_bool).unwrap_or(!app.ui.copy_imported_audio);
            app.ui.copy_imported_audio = on;
            app.engine.copy_imported_audio = on;
            Ok(json!({"value": on}))
        }
        _ => return None,
    })
}

fn current_tab(app: &SoundApp) -> &'static str {
    TABS.iter().find(|(id, _)| *id == app.ui.preferences_tab).map_or(TABS[0].0, |(id, _)| id)
}

/// The Preferences window.
pub fn window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_preferences;
    if !open {
        return;
    }
    egui::Window::new(egui::RichText::new("Preferences").font(bold(13.0)))
        .id(egui::Id::new("preferences_window"))
        .open(&mut open)
        .default_size(vec2(520.0, 420.0))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (id, label) in TABS {
                    let on = current_tab(app) == id;
                    if ui.add(egui::Button::selectable(on, egui::RichText::new(label).font(bold(12.0)))).clicked() {
                        app.ui.preferences_tab = id.to_string();
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| match current_tab(app) {
                "operation" => operation(app, ui),
                "editing" => editing(app, ui),
                "mixing" => mixing(app, ui),
                "metering" => metering(app, ui),
                _ => display(app, ui),
            });
        });
    app.ui.show_preferences = open && app.ui.show_preferences;
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text).font(bold(11.0)).color(Tokens::current().text_dim));
}

/// A checkbox that runs `cmd {value}` when changed.
fn flag(app: &mut SoundApp, ui: &mut Ui, label: &str, cmd: &str, on: bool, tip: &str) {
    let mut v = on;
    let resp = crate::widgets::checkbox(ui, &mut v, label);
    let resp = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    if resp.changed() {
        // `on` for the UI toggles that take it (Plugin Editors as in the VST, Slim).
        report(app, cmd, json!({"value": v, "on": v}));
    }
}

fn report(app: &mut SoundApp, cmd: &str, params: Value) {
    if let Err(e) = app.run(cmd, params) {
        app.ui.status = e;
    }
}

fn display(app: &mut SoundApp, ui: &mut Ui) {
    heading(ui, "THIS COMPUTER");
    egui::Grid::new("prefs_display").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
        ui.label("Theme");
        let mut chosen = None;
        egui::ComboBox::from_id_salt("prefs_theme").selected_text(app.ui.theme.label()).show_ui(ui, |ui| {
            for mode in crate::theme::ThemeMode::ALL {
                if ui.selectable_label(app.ui.theme == mode, mode.label()).clicked() {
                    chosen = Some(mode);
                }
            }
        });
        if let Some(mode) = chosen {
            report(app, "ui.theme", json!({"mode": mode.id()}));
        }
        ui.end_row();
        ui.label("Interface scale");
        ui.add(egui::Slider::new(&mut app.extra.ui_scale, 0.75..=1.5).step_by(0.05).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
        ui.end_row();
        ui.label("Track colour in the Edit window");
        ui.add(egui::Slider::new(&mut app.ui.track_tint, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)))
            .on_hover_text("How strongly tracks take on their colour (0: only the colour strip)");
        ui.end_row();
    });
    crate::widgets::checkbox(ui, &mut app.extra.reduce_motion, "Reduce motion");
    crate::widgets::checkbox(ui, &mut app.ui.show_splash, "Show splash screen at launch")
        .on_hover_text("The six-second opening animation (a click or Esc skips it). Never shown for automation runs.");
    heading(ui, "THIS SESSION");
    let e = app.engine.session().edit.clone();
    let power = e.flag("waveform.power") && !e.flag("waveform.peak");
    ui.horizontal(|ui| {
        ui.label("Waveforms");
        if ui.radio(!power, "Peak").clicked() {
            report(app, "view.waveform_peak", json!({}));
        }
        if ui.radio(power, "Power").clicked() {
            report(app, "view.waveform_power", json!({}));
        }
    });
    flag(app, ui, "Rectified waveforms", "view.waveform_rectified", e.flag("waveform.rectified"), "");
    flag(app, ui, "Waveform outlines", "view.waveform_outlines", e.flag("waveform.outlines"), "");
}

fn operation(app: &mut SoundApp, ui: &mut Ui) {
    let e = app.engine.session().edit.clone();
    let sr = app.engine.session().sample_rate;
    heading(ui, "THIS SESSION: TRANSPORT");
    flag(app, ui, "Click", "options.click", e.click, "The metronome: Setup › Click/Countoff… sets its sound");
    flag(app, ui, "Loop Record", "options.loop_record", e.loop_record, "");
    flag(app, ui, "Pre/Post-Roll", "options.pre_post_roll", e.pre_post_roll, "Play from before the selection and on past its end");
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        for (label, cmd, len) in [("Pre-roll", "options.pre_roll", e.pre_roll), ("Post-roll", "options.post_roll", e.post_roll)] {
            ui.label(label);
            let mut secs = sr.seconds(len);
            let resp = ui.add(egui::DragValue::new(&mut secs).range(0.0..=60.0).speed(0.05).suffix(" s").max_decimals(2));
            if resp.changed() {
                report(app, cmd, json!({"length": {"seconds": secs}}));
            }
        }
    });
    heading(ui, "THIS SESSION: FOLLOWING & SCROLLING");
    flag(app, ui, "Link Timeline and Edit Selection", "options.link_timeline_edit", e.link_timeline_edit, "");
    flag(app, ui, "Link Track and Edit Selection", "options.link_track_edit", e.link_track_edit, "");
    flag(app, ui, "Insertion Follows Playback", "options.insertion_follows_playback", e.insertion_follows_playback, "");
    ui.horizontal(|ui| {
        ui.label("Edit window scrolling");
        let modes = [
            ("none", "No Scrolling"),
            ("after_playback", "After Playback"),
            ("page", "Page"),
            ("continuous", "Continuous"),
            ("center", "Center Playhead"),
        ];
        let shown = modes.iter().find(|(id, _)| *id == e.scrolling).map_or("Page", |(_, l)| l);
        let mut chosen = None;
        egui::ComboBox::from_id_salt("prefs_scrolling").selected_text(shown).show_ui(ui, |ui| {
            for (id, label) in modes {
                if ui.selectable_label(e.scrolling == id, label).clicked() {
                    chosen = Some(id);
                }
            }
        });
        if let Some(mode) = chosen {
            report(app, "options.scrolling", json!({"mode": mode}));
        }
    });
    heading(ui, "THIS COMPUTER: PROJECT FILES");
    let on = app.ui.copy_imported_audio;
    flag(
        app,
        ui,
        "Copy imported audio into the project",
        "ui.copy_imported_audio",
        on,
        "When the session is saved, imported audio is copied into its Audio Files folder, so the project folder holds everything it needs. The original file is never moved or deleted. Off: the project refers to the original where it is.",
    );
}

fn editing(app: &mut SoundApp, ui: &mut Ui) {
    let e = app.engine.session().edit.clone();
    heading(ui, "THIS SESSION");
    flag(app, ui, "Tab to Transient", "options.tab_to_transient", e.tab_to_transient, "Tab moves to the next transient");
    flag(app, ui, "Automation Follows Edit", "options.automation_follows_edit", e.automation_follows_edit, "Moving clips carries their automation");
    flag(app, ui, "Mirrored MIDI Editing", "options.mirrored_midi", e.mirrored_midi, "Editing a MIDI clip edits its copies too");
    flag(app, ui, "Show grid lines", "view.grid_lines", e.grid_lines, "");
}

fn mixing(app: &mut SoundApp, ui: &mut Ui) {
    let e = app.engine.session().edit.clone();
    heading(ui, "THIS SESSION");
    flag(app, ui, "Delay Compensation", "options.delay_compensation", e.delay_compensation, "Line up tracks whose plugins add latency");
    flag(
        app,
        ui,
        "Low-latency monitoring",
        "options.low_latency_monitoring",
        e.flag("options.low_latency_monitoring"),
        "Bypass latency-adding plugins on live tracks",
    );
    let xor = e.solo_mode.contains("xor");
    let mut v = xor;
    if crate::widgets::checkbox(ui, &mut v, "X-OR solo (a new solo cancels the previous one)").changed() {
        report(app, "options.solo_mode", json!({"mode": "sip", "xor": v}));
    }
    heading(ui, "THIS COMPUTER");
    let narrow = app.ui.narrow_mix;
    flag(app, ui, "Narrow Mix", "window.narrow_mix", narrow, "Narrow strips in the Mix window");
    let wide = app.ui.wide_editors;
    flag(app, ui, "Plugin editors as in the VST", "ui.mix.wide_editors", wide, "FireStation modules open their VST-faithful editors in the drawer");
    let slim = app.ui.slim_editors;
    ui.add_enabled_ui(wide, |ui| {
        flag(app, ui, "Slim plugin editors", "ui.mix.slim_editors", slim, "The VST editors' slim layout, at the drawer's width")
    });
    heading(ui, "EDIT AND MIX WINDOWS (WINDOW › SYNC)");
    let sync = app.ui.sync;
    flag(app, ui, "Sync selection", "window.sync_selection", sync.selection, "");
    flag(app, ui, "Scroll to the selected track", "window.sync_scroll", sync.scroll, "");
    flag(app, ui, "Follow folder collapse", "window.sync_folders", sync.folders, "");
    flag(app, ui, "Sync track visibility", "window.sync_visibility", sync.visibility, "");
}

fn metering(app: &mut SoundApp, ui: &mut Ui) {
    let b = Ballistics::of(&app.ui);
    heading(ui, "THIS COMPUTER: TRACK AND MASTER METERS");
    egui::Grid::new("prefs_metering").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
        ui.label("Release");
        let shown = RELEASES
            .iter()
            .find(|(_, v)| (v - b.release_db_s).abs() < 0.01)
            .map_or_else(|| format!("{:.0} dB/s", b.release_db_s), |(l, _)| (*l).to_string());
        let mut chosen = None;
        egui::ComboBox::from_id_salt("prefs_release").selected_text(shown).show_ui(ui, |ui| {
            for (label, v) in RELEASES {
                if ui.selectable_label((v - b.release_db_s).abs() < 0.01, label).clicked() {
                    chosen = Some(v);
                }
            }
        });
        if let Some(v) = chosen {
            report(app, "ui.meter_ballistics", json!({"release_db_s": v}));
        }
        ui.end_row();
        ui.label("Peak hold");
        let shown =
            HOLDS.iter().find(|(_, v)| (v - b.hold_secs).abs() < 0.01).map_or_else(|| format!("{:.1} s", b.hold_secs), |(l, _)| (*l).to_string());
        let mut chosen = None;
        egui::ComboBox::from_id_salt("prefs_hold").selected_text(shown).show_ui(ui, |ui| {
            for (label, v) in HOLDS {
                if ui.selectable_label((v - b.hold_secs).abs() < 0.01, label).clicked() {
                    chosen = Some(v);
                }
            }
        });
        if let Some(v) = chosen {
            report(app, "ui.meter_ballistics", json!({"hold_secs": v}));
        }
        ui.end_row();
    });
    ui.label(
        egui::RichText::new("A click on a meter clears its clip light and peak (Alt-click: every track's).")
            .font(regular(10.5))
            .color(Tokens::current().text_dim),
    );
}

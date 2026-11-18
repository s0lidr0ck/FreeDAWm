//! The monitor stage in the app: the master fader's EAR and SIM keys, their commands, and the
//! Ear Profile window.
//!
//! The stage (`soundcraft_dsp::monitor`) sits between the mix and the audio device, in the player
//! only: bounces, exports, freezes and every other offline render never pass through it, and the
//! meters read the mix before it. EAR is a hearing correction (one curve per ear); SIM plays the
//! mix through a simulated output (a phone, a car…). Which are on, and with which profile, is this
//! computer's (`ui.json`): no session holds it, and changing it is no edit. The profiles are
//! files in the Library folder's `Monitor Profiles` (`soundcraft_engine::monitor_profiles`), so
//! a Library synced between computers carries them.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Sense, Ui, Vec2};
use serde_json::{Value, json};
use soundcraft_engine::monitor_profiles as lib;
use soundcraft_model::monitor::HearingProfile;
use std::path::PathBuf;

/// The frequencies the Ear Profile window's sliders set, Hz (the hearing test's).
pub const EDIT_HZ: [f32; 10] = soundcraft_dsp::hearing_test::TEST_HZ;
/// The sliders' range, dB.
pub const EDIT_DB: std::ops::RangeInclusive<f32> = -20.0..=30.0;

/// What the player was last told, so it is told again only on a change (or when it is replaced).
#[derive(Debug, Clone, PartialEq)]
struct Sent {
    player: usize,
    rate: u32,
    ear_on: bool,
    sim_on: bool,
    ear: String,
    sim: String,
    dir: Option<PathBuf>,
    revision: u64,
}

/// The Ear Profile window's work in progress.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Draft {
    pub name: String,
    pub headphones: String,
    /// Gain per [`EDIT_HZ`] frequency, dB.
    pub left: [f32; 10],
    pub right: [f32; 10],
}

impl Draft {
    fn of(p: &HearingProfile) -> Draft {
        let at = |points: &[[f32; 2]]| {
            let clean = soundcraft_dsp::monitor::clean_points(points);
            EDIT_HZ.map(|hz| soundcraft_dsp::monitor::curve_db(&clean, hz).clamp(*EDIT_DB.start(), *EDIT_DB.end()))
        };
        Draft { name: p.name.clone(), headphones: p.headphones.clone(), left: at(&p.left), right: at(&p.right) }
    }

    pub fn profile(&self) -> HearingProfile {
        let points = |g: &[f32; 10]| EDIT_HZ.iter().zip(g).map(|(hz, db)| [*hz, *db]).collect();
        let mut p =
            HearingProfile { name: self.name.clone(), headphones: self.headphones.clone(), left: points(&self.left), right: points(&self.right) };
        p.sanitize();
        p
    }
}

/// The app's side of the monitor stage.
#[derive(Debug, Default)]
pub struct MonitorUi {
    sent: Option<Sent>,
    /// Bumped whenever a profile's contents may have changed (a save, a slider).
    revision: u64,
    /// The Ear Profile window's profile while it is open: heard instead of the saved one.
    pub draft: Option<Draft>,
}

/// The `Monitor Profiles` folder of the Library in use (none in a bare test app).
pub fn dir(app: &SoundApp) -> Option<PathBuf> {
    app.dialogs.dirs.root.as_deref().map(lib::dir_in)
}

fn hearing_names(app: &SoundApp) -> Vec<HearingProfile> {
    dir(app).map(|d| lib::hearing(&d)).unwrap_or_default()
}

/// Tells the player what the stage does, when that changed or the player is a new one. A stage
/// whose profile is gone (deleted, or not synced to this computer) is turned off, with a note.
pub(crate) fn sync(app: &mut SoundApp) {
    let Some(player) = app.player.as_ref().map(soundcraft_playback::Player::id) else { return };
    let rate = app.engine.session().sample_rate.hz();
    let folder = dir(app);
    let same = app.monitor.sent.as_ref().is_some_and(|s| {
        s.player == player
            && s.rate == rate
            && s.ear_on == app.ui.monitor_ear
            && s.sim_on == app.ui.monitor_sim
            && s.ear == app.ui.monitor_ear_profile
            && s.sim == app.ui.monitor_sim_profile
            && s.dir == folder
            && s.revision == app.monitor.revision
    });
    if same {
        return;
    }
    let ear = match &app.monitor.draft {
        Some(d) => Some(d.profile()),
        None => folder.as_deref().and_then(|d| lib::find_hearing(d, &app.ui.monitor_ear_profile)),
    };
    let sim = lib::find_device(folder.as_deref(), &app.ui.monitor_sim_profile);
    if app.ui.monitor_ear && ear.is_none() {
        app.ui.monitor_ear = false;
        app.ui.status = format!("Ear correction is off: the hearing profile “{}” was not found", app.ui.monitor_ear_profile);
    }
    if app.ui.monitor_sim && sim.is_none() {
        app.ui.monitor_sim = false;
        app.ui.status = format!("Simulated output is off: the profile “{}” was not found", app.ui.monitor_sim_profile);
    }
    let plan = lib::plan(rate as f32, ear.as_ref(), app.ui.monitor_ear, sim.as_ref(), app.ui.monitor_sim);
    if let Some(p) = &app.player {
        p.set_monitor(plan);
    }
    app.monitor.sent = Some(Sent {
        player,
        rate,
        ear_on: app.ui.monitor_ear,
        sim_on: app.ui.monitor_sim,
        ear: app.ui.monitor_ear_profile.clone(),
        sim: app.ui.monitor_sim_profile.clone(),
        dir: folder,
        revision: app.monitor.revision,
    });
}

fn state(app: &SoundApp) -> Value {
    json!({
        "ear": {"value": app.ui.monitor_ear, "profile": app.ui.monitor_ear_profile},
        "sim": {"value": app.ui.monitor_sim, "profile": app.ui.monitor_sim_profile},
    })
}

fn points(p: &Value, key: &str) -> Result<Vec<[f32; 2]>, String> {
    let Some(v) = p.get(key) else { return Ok(Vec::new()) };
    let bad = || format!("`{key}` is a list of [Hz, dB] pairs");
    v.as_array()
        .ok_or_else(bad)?
        .iter()
        .take(soundcraft_model::monitor::MAX_MONITOR_POINTS)
        .map(|pt| match pt.as_array().map(Vec::as_slice) {
            Some([hz, db]) => Ok([hz.as_f64().ok_or_else(bad)? as f32, db.as_f64().ok_or_else(bad)? as f32]),
            _ => Err(bad()),
        })
        .collect()
}

/// UI commands handled here: `monitor.ear {value?, profile?}`, `monitor.sim {value?, profile?}`,
/// `monitor.profiles`, `monitor.ear_profile.save {name, headphones?, left?, right?, select?}` and
/// `window.ear_profile {value?}`. None of them edits the session.
pub fn run(app: &mut SoundApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    let want = |cur: bool| p.get("value").and_then(Value::as_bool).unwrap_or(!cur);
    Some(match id {
        "monitor.ear" => {
            let named = p.get("profile").and_then(Value::as_str).map(str::trim);
            let all = hearing_names(app);
            if let Some(name) = named {
                let Some(found) = all.iter().find(|h| h.name.eq_ignore_ascii_case(name)) else {
                    return Some(Err(format!("no hearing profile named `{name}`")));
                };
                app.ui.monitor_ear_profile = found.name.clone();
            }
            // A profile named without a value turns the stage on.
            let on = p.get("value").and_then(Value::as_bool).unwrap_or(named.is_some() || !app.ui.monitor_ear);
            if on && app.monitor.draft.is_none() && !all.iter().any(|h| h.name.eq_ignore_ascii_case(&app.ui.monitor_ear_profile)) {
                match all.first() {
                    Some(first) => app.ui.monitor_ear_profile = first.name.clone(),
                    None => {
                        // Nothing to correct with yet: the test that makes one opens instead.
                        let _ = crate::hearing_ui::run(app, "window.hearing_test", &json!({"value": true}));
                        app.ui.status = "No hearing profile yet: the Hearing Test makes one".into();
                        return Some(Err(app.ui.status.clone()));
                    }
                }
            }
            app.ui.monitor_ear = on;
            Ok(state(app))
        }
        "monitor.sim" => {
            let named = p.get("profile").and_then(Value::as_str).map(str::trim);
            let all = lib::devices(dir(app).as_deref());
            if let Some(name) = named {
                let Some(found) = all.iter().find(|d| d.name.eq_ignore_ascii_case(name)) else {
                    return Some(Err(format!("no simulated output named `{name}`")));
                };
                app.ui.monitor_sim_profile = found.name.clone();
            }
            let on = p.get("value").and_then(Value::as_bool).unwrap_or(named.is_some() || !app.ui.monitor_sim);
            if on && !all.iter().any(|d| d.name.eq_ignore_ascii_case(&app.ui.monitor_sim_profile)) {
                app.ui.monitor_sim_profile = all.first().map(|d| d.name.clone()).unwrap_or_default();
            }
            app.ui.monitor_sim = on && !app.ui.monitor_sim_profile.is_empty();
            Ok(state(app))
        }
        "monitor.profiles" => {
            let builtin = lib::builtin_devices();
            let folder = dir(app);
            let devices: Vec<Value> = lib::devices(folder.as_deref())
                .into_iter()
                // Built in unless a file of the Library replaces it.
                .map(|d| json!({"name": d.name, "notes": d.notes, "builtin": builtin.contains(&d), "width": d.width}))
                .collect();
            let hearing: Vec<Value> = hearing_names(app).into_iter().map(|h| json!({"name": h.name, "headphones": h.headphones})).collect();
            let mut v = state(app);
            if let Some(o) = v.as_object_mut() {
                o.insert("hearing".into(), json!(hearing));
                o.insert("devices".into(), json!(devices));
                o.insert("folder".into(), json!(folder.map(|d| d.display().to_string())));
            }
            Ok(v)
        }
        "monitor.ear_profile.save" => {
            let Some(folder) = dir(app) else { return Some(Err("there is no Library folder to save the profile in".into())) };
            let name = p.get("name").and_then(Value::as_str).unwrap_or_default();
            let (left, right) = match (points(p, "left"), points(p, "right")) {
                (Ok(l), Ok(r)) => (l, r),
                (Err(e), _) | (_, Err(e)) => return Some(Err(e)),
            };
            let headphones = p.get("headphones").and_then(Value::as_str).unwrap_or_default().to_string();
            let mut prof = HearingProfile { name: name.to_string(), headphones, left, right };
            prof.sanitize();
            match lib::save_hearing(&folder, &prof) {
                Ok(file) => {
                    app.monitor.revision = app.monitor.revision.wrapping_add(1);
                    if p.get("select").and_then(Value::as_bool).unwrap_or(false) {
                        app.ui.monitor_ear_profile = prof.name.clone();
                    }
                    Ok(json!({"name": prof.name, "file": file.display().to_string()}))
                }
                Err(e) => Err(e),
            }
        }
        "window.ear_profile" => {
            if want(app.ui.show_ear_profile) {
                open_editor(app);
            } else {
                close_editor(app);
            }
            Ok(json!({"value": app.ui.show_ear_profile}))
        }
        _ => return None,
    })
}

/// Opens the Ear Profile window on `draft` (the hearing test's result), in place of whatever it
/// was editing.
pub(crate) fn load_draft(app: &mut SoundApp, draft: Draft) {
    app.monitor.draft = Some(draft);
    app.monitor.revision = app.monitor.revision.wrapping_add(1);
    app.ui.show_ear_profile = true;
}

fn open_editor(app: &mut SoundApp) {
    if app.monitor.draft.is_none() {
        let current = dir(app).and_then(|d| lib::find_hearing(&d, &app.ui.monitor_ear_profile));
        app.monitor.draft = Some(current.as_ref().map(Draft::of).unwrap_or_default());
        app.monitor.revision = app.monitor.revision.wrapping_add(1);
    }
    app.ui.show_ear_profile = true;
}

fn close_editor(app: &mut SoundApp) {
    app.ui.show_ear_profile = false;
    if app.monitor.draft.take().is_some() {
        app.monitor.revision = app.monitor.revision.wrapping_add(1);
    }
}

/// A master strip key with a short word on it, lit in the accent colour when on.
pub fn word_key(ui: &mut Ui, size: Vec2, on: bool, word: &str, label: &str) -> egui::Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), on, label));
    crate::widgets::key(ui.painter(), r, &resp, if on { t.accent } else { crate::widgets::key_fill(&resp) });
    let font = (size.y * 0.5).clamp(7.0, 10.0);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, word, bold(font), if on { t.text_dark } else { t.text });
    resp
}

fn report(app: &mut SoundApp, cmd: &str, params: Value) {
    if let Err(e) = app.run(cmd, params) {
        app.ui.status = e;
    }
}

/// The EAR and SIM keys of the master strip (`size` each). A click turns the stage on or off; a
/// right-click picks its profile.
pub fn strip_keys(app: &mut SoundApp, ui: &mut Ui, size: Vec2) {
    let ear_tip = if app.ui.monitor_ear_profile.is_empty() {
        "Ear correction (monitoring only, never in a bounce)\nRight-click to choose or edit a hearing profile".to_string()
    } else {
        format!(
            "Ear correction: {} (monitoring only, never in a bounce)\nRight-click to choose or edit a hearing profile",
            app.ui.monitor_ear_profile
        )
    };
    let ear = word_key(ui, size, app.ui.monitor_ear, "EAR", "Ear correction").on_hover_text(ear_tip);
    if ear.clicked() {
        report(app, "monitor.ear", json!({}));
    }
    ear.context_menu(|ui| {
        for h in hearing_names(app) {
            let on = app.ui.monitor_ear_profile.eq_ignore_ascii_case(&h.name);
            let text = if h.headphones.is_empty() { h.name.clone() } else { format!("{} ({})", h.name, h.headphones) };
            if ui.radio(on, text).clicked() {
                report(app, "monitor.ear", json!({"profile": h.name}));
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Hearing Test…").clicked() {
            report(app, "window.hearing_test", json!({"value": true}));
            ui.close();
        }
        if ui.button("Ear Profile…").clicked() {
            report(app, "window.ear_profile", json!({"value": true}));
            ui.close();
        }
    });
    let sim_tip = if app.ui.monitor_sim_profile.is_empty() {
        "Simulated output (monitoring only, never in a bounce)\nRight-click to choose what the mix is heard through".to_string()
    } else {
        format!(
            "Simulated output: {} (monitoring only, never in a bounce)\nRight-click to choose what the mix is heard through",
            app.ui.monitor_sim_profile
        )
    };
    let sim = word_key(ui, size, app.ui.monitor_sim, "SIM", "Simulated output").on_hover_text(sim_tip);
    if sim.clicked() {
        report(app, "monitor.sim", json!({}));
    }
    sim.context_menu(|ui| {
        for d in lib::devices(dir(app).as_deref()) {
            let on = app.ui.monitor_sim_profile.eq_ignore_ascii_case(&d.name);
            let mut item = ui.radio(on, d.name.as_str());
            if !d.notes.is_empty() {
                item = item.on_hover_text(d.notes.as_str());
            }
            if item.clicked() {
                report(app, "monitor.sim", json!({"profile": d.name}));
                ui.close();
            }
        }
    });
}

/// The Ear Profile window: a hearing correction set by hand, heard as it is changed.
pub fn window(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.ui.show_ear_profile {
        return;
    }
    let Some(mut draft) = app.monitor.draft.clone() else {
        open_editor(app);
        return;
    };
    let before = draft.clone();
    let mut open = true;
    let (mut save, mut retest) = (false, false);
    let t = Tokens::current();
    egui::Window::new(egui::RichText::new("Ear Profile").font(bold(13.0)))
        .id(egui::Id::new("ear_profile_window"))
        .open(&mut open)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "Raise a slider where that ear hears less. The correction is only ever heard while monitoring: bounces and exports never include it. Nothing is boosted above the mix, so turn your headphones up to taste.",
                )
                .font(regular(11.0))
                .color(t.text_dim),
            );
            ui.add_space(4.0);
            egui::Grid::new("ear_profile_names").num_columns(2).show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(240.0).hint_text("Me on my headphones"));
                ui.end_row();
                ui.label("Headphones");
                ui.add(egui::TextEdit::singleline(&mut draft.headphones).desired_width(240.0));
                ui.end_row();
            });
            ui.add_space(6.0);
            for (side, gains) in [("Left ear", &mut draft.left), ("Right ear", &mut draft.right)] {
                ui.label(egui::RichText::new(side).font(bold(12.0)));
                ui.horizontal(|ui| {
                    for (hz, db) in EDIT_HZ.iter().zip(gains.iter_mut()) {
                        ui.vertical(|ui| {
                            let name = if *hz >= 1000.0 { format!("{}k", hz / 1000.0) } else { format!("{hz}") };
                            ui.add(egui::Slider::new(db, EDIT_DB).vertical().step_by(0.5).show_value(false))
                                .on_hover_text(format!("{side}, {name} Hz: {db:+.1} dB"));
                            ui.label(egui::RichText::new(name).font(regular(10.0)).color(t.text_dim));
                            ui.label(egui::RichText::new(format!("{db:+.0}")).font(regular(10.0)));
                        });
                    }
                });
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                if ui.button("Copy Left to Right").clicked() {
                    draft.right = draft.left;
                }
                if ui.button("Hearing Test…").on_hover_text("Measure the profile instead of setting it by hand").clicked() {
                    retest = true;
                }
                if ui.button("Flat").clicked() {
                    draft.left = [0.0; 10];
                    draft.right = [0.0; 10];
                }
                let named = !draft.name.trim().is_empty();
                if ui.add_enabled(named, egui::Button::new("Save")).on_disabled_hover_text("Give the profile a name first").clicked() {
                    save = true;
                }
            });
        });
    if draft != before {
        app.monitor.revision = app.monitor.revision.wrapping_add(1);
    }
    app.monitor.draft = Some(draft.clone());
    if save {
        let p = draft.profile();
        match app
            .run("monitor.ear_profile.save", json!({"name": p.name, "headphones": p.headphones, "left": p.left, "right": p.right, "select": true}))
        {
            Ok(_) => app.ui.status = format!("Saved the hearing profile “{}”", p.name),
            Err(e) => app.ui.status = e,
        }
    }
    if retest {
        let _ = app.run("window.hearing_test", json!({"value": true, "name": draft.name, "headphones": draft.headphones}));
        open = false;
    }
    if !open {
        close_editor(app);
    }
}

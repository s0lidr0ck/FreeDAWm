//! The automation lane picker: what a track can show as a lane (under the track, or as its main
//! view), grouped as Pro Tools lists it, each marked when it already has automation or is shown.

use crate::SoundApp;
use egui::Ui;
use serde_json::json;
use soundcraft_model::{AutoParam, Route, Session, Track, TrackKind};

/// One parameter the picker offers: the submenu it sits in (`None`: the top level), the lane and
/// its name there.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub group: Option<String>,
    pub param: AutoParam,
    pub label: String,
}

/// Everything a lane can show on `track`, in menu order: the fader section (volume, both
/// panners of a stereo track, mute, trim), each send's level, pan and mute, then each module's
/// parameters, built-in or hosted (VST3, CLAP, AU: their own names and ranges; a built-in lists
/// the drawer's own, see [`crate::edit_window::lane_params`]). `s` names the sends' busses.
pub fn choices(s: &Session, track: &Track) -> Vec<Choice> {
    let top = |param: AutoParam, label: &str| Choice { group: None, param, label: label.to_string() };
    let mut v = Vec::new();
    if track.kind != TrackKind::Folder {
        v.push(top(AutoParam::Volume, "volume"));
        if track.kind != TrackKind::Vca {
            match track.mixer.pan.len() {
                0 => {}
                1 => v.push(top(AutoParam::Pan(0), "pan")),
                _ => {
                    v.push(top(AutoParam::Pan(0), "pan left"));
                    v.push(top(AutoParam::Pan(1), "pan right"));
                }
            }
        }
        if track.kind != TrackKind::Master {
            v.push(top(AutoParam::Mute, "mute"));
        }
        v.push(top(AutoParam::Trim, "trim"));
    }
    // Sends: every assigned one, and any slot that still has automation.
    for (i, snd) in track.mixer.sends.iter().enumerate() {
        let Ok(slot) = u8::try_from(i) else { continue };
        let lanes = [AutoParam::SendLevel(slot), AutoParam::SendPan(slot), AutoParam::SendMute(slot)];
        if snd.is_none() && !lanes.iter().any(|p| has_data(track, p)) {
            continue;
        }
        let letter = char::from(b'A'.saturating_add(slot.min(25)));
        let bus = match snd.as_ref().map(|x| &x.target) {
            Some(Route::Bus(b)) => s.bus(*b).map(|b| b.name.clone()),
            _ => None,
        };
        let group = match bus {
            Some(b) => format!("Send {letter} ({b})"),
            None => format!("Send {letter}"),
        };
        for (p, label) in lanes.into_iter().zip(["level", "pan", "mute"]) {
            v.push(Choice { group: Some(group.clone()), param: p, label: format!("send {} {label}", letter.to_ascii_lowercase()) });
        }
    }
    for ins in &track.mixer.inserts {
        let Some(info) = crate::mix_window::plugin_info(&ins.plugin) else { continue };
        for p in crate::edit_window::lane_params(track, ins, info) {
            let name = crate::module_drawer::dial_name(ins, p.id).unwrap_or(p.name);
            v.push(Choice {
                group: Some(info.name.to_string()),
                param: AutoParam::Plugin { module: ins.id, param: p.id.to_string() },
                label: name.to_string(),
            });
        }
    }
    v
}

/// Whether the track has automation on `p` (a lane with points).
pub fn has_data(track: &Track, p: &AutoParam) -> bool {
    track.lane(p).is_some_and(|l| !l.points.is_empty())
}

/// Whether `p` is on screen for `track`: its main view or one of its lanes.
pub fn is_shown(track: &Track, p: &AutoParam) -> bool {
    AutoParam::parse(&track.view).as_ref() == Some(p) || track.auto_lanes.iter().any(|v| AutoParam::parse(v).as_ref() == Some(p))
}

/// The label of a choice in the menu: a dot after the name when it has automation.
pub fn menu_label(track: &Track, c: &Choice) -> String {
    if has_data(track, &c.param) { format!("{}  \u{2022}", c.label) } else { c.label.clone() }
}

/// The picker's rows into `ui`; returns the parameter picked. `checked` says which rows show a
/// check mark.
pub fn picker(ui: &mut Ui, s: &Session, track: &Track, checked: &dyn Fn(&AutoParam) -> bool) -> Option<AutoParam> {
    let mut picked = None;
    let all = choices(s, track);
    let mut groups: Vec<&str> = Vec::new();
    for c in &all {
        match &c.group {
            None => {
                if ui.selectable_label(checked(&c.param), menu_label(track, c)).clicked() {
                    picked = Some(c.param.clone());
                    ui.close();
                }
            }
            Some(g) if !groups.contains(&g.as_str()) => groups.push(g),
            Some(_) => {}
        }
    }
    for g in groups {
        let any_data = all.iter().any(|c| c.group.as_deref() == Some(g) && has_data(track, &c.param));
        let title = if any_data { format!("{g}  \u{2022}") } else { g.to_string() };
        ui.menu_button(title, |ui| {
            for c in all.iter().filter(|c| c.group.as_deref() == Some(g)) {
                if ui.selectable_label(checked(&c.param), menu_label(track, c)).clicked() {
                    picked = Some(c.param.clone());
                    ui.close();
                }
            }
        });
    }
    picked
}

/// The lanes shown under `track`, top to bottom: each resolves on the track (a lane whose module
/// is gone is dropped), once, and not the parameter the main view already shows.
pub fn shown_lanes(track: &Track) -> Vec<AutoParam> {
    let main = AutoParam::parse(&track.view);
    let mut v: Vec<AutoParam> = Vec::new();
    for id in &track.auto_lanes {
        let Some(p) = AutoParam::parse(id) else { continue };
        if let AutoParam::Plugin { module, .. } = &p
            && track.mixer.module(*module).is_none()
        {
            continue;
        }
        if main.as_ref() != Some(&p) && !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

/// The "+" picker under a lane (or the track): shows the parameter picked right under `after`.
pub fn add_lane_menu(app: &mut SoundApp, ui: &mut Ui, track: &Track, after: Option<&AutoParam>) {
    ui.label(egui::RichText::new("Show automation lane").weak());
    let s = app.engine.session_arc();
    if let Some(p) = picker(ui, &s, track, &|p| is_shown(track, p)) {
        let mut params = json!({"track": track.id.0, "param": p.id(), "show": true});
        if let Some(a) = after {
            params["after"] = json!(a.id());
        }
        let _ = app.run("track.auto_lane", params);
    }
}

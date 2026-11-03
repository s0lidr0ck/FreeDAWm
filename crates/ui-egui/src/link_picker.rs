//! The drawer header's instance-link and reference rows.
//!
//! - **Link** (plugins that declare `PluginInfo::link`, or a module still in a group): the
//!   session-level link group the module is in, through `mix.module_link` (one undo step). The
//!   field shows the group, the module's role and how many modules share it, or why it can't
//!   talk to its group in the same block ("alone", "late (cycle)"). The menu offers groups 1–8
//!   plus any other in use, each with its member count; one the engine would refuse is greyed
//!   with the reason on hover.
//! - **Reference** (plugins that declare `PluginInfo::references`): one row per slot with the
//!   file's name, through `mix.module_reference` (Load file…, Clear).
//!
//! Statuses are worked out once per document revision (`sidechain_picker::cached`), not per frame.

use crate::SoundApp;
use crate::sidechain_picker::{ROW_H, cached};
use crate::theme::{Tokens, bold};
use egui::{Align2, Rect, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::{LinkRole, PluginInfo};
use soundcraft_mix::{LinkProblem, link_refusal, link_routes, short_link_problem};
use soundcraft_model::{Insert, ModuleId, REFERENCE_KEYS, Session, Track, TrackId};
use std::sync::Arc;

/// Groups the menu always offers.
const MENU_GROUPS: u32 = 8;

/// Whether the drawer shows the link row: the plugin links, or the module is still in a group
/// (so it can leave it).
pub fn shows_link(info: &PluginInfo, ins: &Insert) -> bool {
    info.link.is_some() || ins.link_group.is_some()
}

/// A role as the field names it.
pub fn role_name(role: Option<LinkRole>) -> &'static str {
    match role {
        Some(LinkRole::Producer) => "sends",
        Some(LinkRole::Consumer) => "receives",
        Some(LinkRole::Peer) => "shares",
        None => "idle",
    }
}

/// The module's link as the engine resolves it: (role, problem, modules in its group).
pub type LinkStatus = Option<(Option<LinkRole>, Option<LinkProblem>, usize)>;

/// [`LinkStatus`] of module `module` on `track`.
pub fn link_status(s: &Session, track: TrackId, module: ModuleId) -> LinkStatus {
    let routes = link_routes(s);
    let r = routes.iter().find(|r| r.track == track && r.module == module)?;
    let n = routes.iter().filter(|x| x.group == r.group).count();
    Some((r.role, r.result.err(), n))
}

/// What the link field reads.
pub fn link_label(ins: &Insert, status: LinkStatus) -> String {
    let Some(g) = ins.link_group else { return "Off (not linked)".into() };
    match status {
        None => format!("Group {g}"),
        Some((role, None, n)) => format!("Group {g} · {} · {n} linked", role_name(role)),
        Some((role, Some(p), _)) if p.is_ordering() => format!("Group {g} · {} · late ({})", role_name(role), short_link_problem(p)),
        Some((role, Some(p), _)) => format!("Group {g} · {} · {}", role_name(role), short_link_problem(p)),
    }
}

/// The groups the menu offers (1–8, then any other in use), each with how many modules are in it
/// and the reason the engine would refuse this module there.
pub fn group_choices(s: &Session, track: TrackId, module: ModuleId) -> Vec<(u32, usize, Option<String>)> {
    let mut used: Vec<u32> = s.tracks.iter().flat_map(|t| t.mixer.inserts.iter()).filter_map(|i| i.link_group).collect();
    used.sort_unstable();
    used.dedup();
    let mut groups: Vec<u32> = (1..=MENU_GROUPS).collect();
    groups.extend(used.iter().filter(|g| !(1..=MENU_GROUPS).contains(*g)));
    groups
        .into_iter()
        .map(|g| {
            let n = s
                .tracks
                .iter()
                .flat_map(|t| t.mixer.inserts.iter().map(move |i| (t.id, i)))
                .filter(|(t, i)| i.link_group == Some(g) && !(*t == track && i.id == module))
                .count();
            (g, n, link_refusal(s, track, module, Some(g)))
        })
        .collect()
}

/// Draws the link row at `y` across `inner`; returns the y under it.
pub(crate) fn link_row(app: &mut SoundApp, ui: &mut Ui, track: &Track, ins: &Insert, info: &PluginInfo, inner: Rect, y: f32) -> f32 {
    let t = Tokens::current();
    let label_w = 66.0;
    ui.painter().text(pos2(inner.min.x, y + ROW_H / 2.0), Align2::LEFT_CENTER, "LINK", bold(9.0), t.text_dim);
    let r = Rect::from_min_size(pos2(inner.min.x + label_w, y), vec2(inner.width() - label_w, ROW_H));
    let rev = app.engine.revision;
    let status = cached(ui, egui::Id::new(("link_status", track.id.0, ins.id.0)), rev, || link_status(app.engine.session(), track.id, ins.id));
    let text = link_label(ins, status);
    let resp = crate::module_drawer::menu_field(ui, r, &text, "link", true)
        .on_hover_text("Link this module with other instances (on any track) through a shared group");
    let links = info.link.is_some();
    egui::Popup::menu(&resp).show(|ui| link_menu(app, ui, track.id, ins, links));
    y + ROW_H
}

/// `links`: the plugin links (a module whose plugin does not, still in a group, can only leave).
fn link_menu(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, links: bool) {
    let mut pick: Option<Value> = None;
    ui.label(egui::RichText::new("Link group").small().weak());
    if ui.add(egui::Button::selectable(ins.link_group.is_none(), "Off (not linked)")).clicked() {
        pick = Some(Value::Null);
    }
    let rev = app.engine.revision;
    let choices =
        cached(ui, egui::Id::new(("link_choices", track.0, ins.id.0)), rev, || Arc::new(group_choices(app.engine.session(), track, ins.id)));
    for (g, n, why) in choices.iter() {
        let on = ins.link_group == Some(*g);
        let text = if *n == 0 { format!("Group {g}") } else { format!("Group {g}  ({n})") };
        let resp = ui.add_enabled(on || (links && why.is_none()), egui::Button::selectable(on, text));
        let resp = match why {
            _ if !links => resp.on_disabled_hover_text("This plugin does not link with other instances"),
            Some(w) => resp.on_disabled_hover_text(w.as_str()),
            None => resp,
        };
        if resp.clicked() {
            pick = Some(json!(g));
        }
    }
    if let Some(group) = pick {
        // A refusal shows in the status line (`SoundApp::run`).
        let _ = app.run("mix.module_link", json!({"track": track.0, "module": ins.id.0, "group": group}));
    }
}

/// What a reference slot's field reads: the file's name, "(missing)" when it could not be read.
pub fn reference_label(s: &Session, ins: &Insert, slot: usize) -> String {
    let Some(key) = REFERENCE_KEYS.get(slot) else { return String::new() };
    let Some(r) = ins.assets.get(*key) else { return "None · load a file…".into() };
    let name = std::path::Path::new(r).file_name().and_then(|n| n.to_str()).unwrap_or(r.as_str()).to_string();
    if s.assets.contains_for(key, r, s.sample_rate.hz()) { name } else { format!("{name} (missing)") }
}

/// Draws one row per reference slot of the plugin at `y`; returns the y under them.
pub(crate) fn reference_rows(app: &mut SoundApp, ui: &mut Ui, track: &Track, ins: &Insert, info: &PluginInfo, inner: Rect, mut y: f32) -> f32 {
    let t = Tokens::current();
    let label_w = 66.0;
    let slots = usize::from(info.references).min(REFERENCE_KEYS.len());
    for slot in 0..slots {
        ui.painter().text(pos2(inner.min.x, y + ROW_H / 2.0), Align2::LEFT_CENTER, format!("REF {}", slot + 1), bold(9.0), t.text_dim);
        let r = Rect::from_min_size(pos2(inner.min.x + label_w, y), vec2(inner.width() - label_w, ROW_H));
        let text = reference_label(app.engine.session(), ins, slot);
        let resp = crate::module_drawer::menu_field(ui, r, &text, &format!("ref{slot}"), true)
            .on_hover_text("Reference audio this module plays along with the timeline");
        let loaded = REFERENCE_KEYS.get(slot).is_some_and(|k| ins.assets.contains_key(*k));
        let mut pick: Option<Value> = None;
        egui::Popup::menu(&resp).show(|ui| {
            let can_pick = app.services.pick_open.is_some();
            if ui.add_enabled(can_pick, egui::Button::new("Load file…")).clicked() {
                if let Some(open) = &app.services.pick_open
                    && let Some(path) = open("mix.module_reference", &["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg"])
                {
                    pick = Some(json!(path));
                }
                ui.close();
            }
            if loaded && ui.button("Clear").clicked() {
                pick = Some(Value::Null);
                ui.close();
            }
        });
        if let Some(file) = pick {
            let _ = app.run("mix.module_reference", json!({"track": track.id.0, "module": ins.id.0, "slot": slot, "file": file}));
        }
        y += ROW_H + 4.0;
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    #[test]
    fn labels_say_group_role_and_status() {
        let mut s = Session::default();
        let a = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("A"));
        let mut ins = Insert::new("test_link");
        ins.params.insert("role".into(), 2.0);
        let probe = soundcraft_dsp::plugin_info("test_link").unwrap();
        assert!(shows_link(probe, &ins));
        assert!(!shows_link(soundcraft_dsp::plugin_info("compressor").unwrap(), &ins));
        assert_eq!(link_label(&ins, None), "Off (not linked)");
        ins.link_group = Some(3);
        let m = s.track_mut(a).unwrap().mixer.add_module(ins.clone(), None).unwrap();
        assert_eq!(link_label(&ins, link_status(&s, a, m)), "Group 3 · receives · alone");
        let mut p = Insert::new("test_link");
        p.params.insert("role".into(), 1.0);
        p.link_group = Some(3);
        let b = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("B"));
        s.track_mut(b).unwrap().mixer.add_module(p, None).unwrap();
        assert_eq!(link_label(&ins, link_status(&s, a, m)), "Group 3 · receives · 2 linked");
        let choices = group_choices(&s, a, m);
        assert_eq!(choices.len(), 8);
        assert_eq!(choices[2], (3, 1, None));
        assert_eq!(reference_label(&s, &ins, 0), "None · load a file…");
    }
}

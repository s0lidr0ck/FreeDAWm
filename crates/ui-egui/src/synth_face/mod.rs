//! The built-in synths drawn as instruments: Burley (the subtractive synth), Cicada (the
//! wavetable synth) and Steeple (the FM synth). Each is a printed panel between wood end cheeks
//! with its sections framed in signal order, real knobs and envelope faders, inset displays and
//! a preset display, built from one kit ([`kit`]) and coloured by its own [`kit::Face`].
//!
//! A panel is drawn at its own size at the top of the instrument's window (the window scrolls
//! when it is smaller); every parameter of the synth is on it. Below the sections, the macro knobs
//! and the modulation sources ([`patch`]); the Back button turns the panel round to its cables.

pub mod burley;
pub mod cicada;
pub mod kit;
pub mod patch;
pub mod steeple;

use crate::SoundApp;
use egui::{Color32, Rect, Sense, Ui, Vec2, pos2, vec2};
use kit::Face;
use serde_json::json;
use soundcraft_model::{Insert, TrackId};
use std::cell::RefCell;

const fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Burley: a cream panel, walnut cheeks, an amber display, a burnt-orange accent.
pub const BURLEY: Face = Face {
    panel: &[(0.0, c(0xe9dfc9)), (1.0, c(0xe0d5bc))],
    brushed: false,
    ink: c(0x2b2520),
    ink2: c(0x7a6e5e),
    line: c(0xbfb193),
    key: c(0xf3ebd9),
    well: c(0x1d1710),
    well_ink: c(0xffc46b),
    glow: c(0xffb347),
    grid: c(0x2c2318),
    cap_hi: c(0x4a443d),
    cap_lo: c(0x1c1915),
    pointer: c(0xf6efe2),
    accent: c(0xd9641e),
    on_accent: c(0xfff6e8),
    wood: (c(0x4a2c17), c(0x7a4b28)),
    rear: &[(0.0, c(0x3b332a)), (1.0, c(0x332c24))],
    plates: false,
};

/// Cicada: a dark green-charcoal panel, a large green display, a green accent.
pub const CICADA: Face = Face {
    panel: &[(0.0, c(0x262b27)), (1.0, c(0x1f2420))],
    brushed: false,
    ink: c(0xe4eadf),
    ink2: c(0x8f9a8c),
    line: c(0x444d45),
    key: c(0x333a34),
    well: c(0x0e120f),
    well_ink: c(0xcfe8c9),
    glow: c(0x86e07e),
    grid: c(0x1a211b),
    cap_hi: c(0x565f57),
    cap_lo: c(0x121512),
    pointer: c(0xf2f7ee),
    accent: c(0x86e07e),
    on_accent: c(0x0d130d),
    wood: (c(0x5e412b), c(0x8e6a48)),
    rear: &[(0.0, c(0x191c1a)), (1.0, c(0x141715))],
    plates: false,
};

/// Steeple: a galvanised steel-grey panel, pine cheeks, a blue display and accent.
pub const STEEPLE: Face = Face {
    panel: &[(0.0, c(0xc9d0d4)), (0.3, c(0xdfe4e6)), (0.62, c(0xc3cacf)), (1.0, c(0xd8dee1))],
    brushed: true,
    ink: c(0x1f262b),
    ink2: c(0x5d6a72),
    line: c(0x9aa5ac),
    key: c(0xeef1f3),
    well: c(0x101519),
    well_ink: c(0xbfe1ff),
    glow: c(0x6cc0ff),
    grid: c(0x1a2229),
    cap_hi: c(0x3f4850),
    cap_lo: c(0x14181c),
    pointer: c(0xf3f7fa),
    accent: c(0x1f6fb5),
    on_accent: c(0xf0f7ff),
    wood: (c(0x8a6a3e), c(0xc39a5e)),
    rear: &[(0.0, c(0x34393d)), (1.0, c(0x2c3135))],
    plates: false,
};

/// Which synth a panel is, and how its front is drawn.
pub struct Synth {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: &'static str,
    pub face: &'static Face,
    /// The front's height between the top bar and the macros and sources row.
    pub body_h: f32,
    /// Draws the front (displays and sections) into the rectangle given.
    pub body: fn(&mut SoundApp, &mut Ui, &kit::Ctx, Rect),
}

/// The three synths with panels.
pub const SYNTHS: [Synth; 3] = [
    Synth { id: burley::ID, name: "Burley", kind: "Subtractive Synthesizer", face: &BURLEY, body_h: burley::BODY_H, body: burley::body },
    Synth { id: cicada::ID, name: "Cicada", kind: "Wavetable Synthesizer", face: &CICADA, body_h: cicada::BODY_H, body: cicada::body },
    Synth { id: steeple::ID, name: "Steeple", kind: "FM Synthesizer", face: &STEEPLE, body_h: steeple::BODY_H, body: steeple::body },
];

/// The panels' width.
pub const WIDTH: f32 = 760.0;
/// The panel's margins (top and bottom inside the unit), the gap under the top bar.
const MARGINS: f32 = 16.0;
const UNDER_TOP: f32 = 8.0;

fn synth(plugin: &str) -> Option<&'static Synth> {
    SYNTHS.iter().find(|s| s.id == plugin)
}

/// The size `plugin`'s panel is drawn at, when it has one.
pub fn size(plugin: &str) -> Option<Vec2> {
    synth(plugin).map(|s| vec2(WIDTH, MARGINS + TOP_H + UNDER_TOP + s.body_h + kit::SECTION_H + 2.0))
}

/// Draws `ins` (the instrument of `track`) as its panel, inside a scroll area as large as the
/// window gives. `frozen`: the track is frozen (the panel is shown, its controls greyed).
pub fn draw(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, frozen: bool) {
    let (Some(s), Some(sz)) = (synth(&ins.plugin), size(&ins.plugin)) else { return };
    let want = sz + vec2(8.0, 12.0);
    egui::ScrollArea::both().id_salt(("synth-face", track.0)).auto_shrink([true, true]).min_scrolled_width(want.x).min_scrolled_height(want.y).show(
        ui,
        |ui| {
            let (rect, _) = ui.allocate_exact_size(want, Sense::hover());
            let r = Rect::from_min_size(rect.min + vec2(2.0, 2.0), sz);
            let mut pui = ui.new_child(egui::UiBuilder::new().id_salt(("synth-face-ui", track.0)).max_rect(r));
            if frozen {
                pui.disable();
            }
            panel(app, &mut pui, track, ins, s, r);
        },
    );
}

/// One synth's panel in `r`: the unit, the top bar, the front (or the Back), the macros and the
/// sources, and the patching gestures across them.
fn panel(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, s: &Synth, r: Rect) {
    let Some(info) = soundcraft_dsp::plugin_info(s.id) else { return };
    let mut st = patch::state(ui.ctx(), track);
    // The Back is the rear of the same instrument: the same layout on a darker plate.
    let rear = kit::rear(s.face);
    let face = if st.back { &rear } else { s.face };
    let p = ui.painter().clone();
    let inner = kit::unit(&p, r, face);
    // The Back's cables go here, under every jack and label drawn after.
    let cable_slot = p.add(egui::Shape::Noop);
    let (arcs, kinds) = patch::arcs(ins, s.face);
    let hooks = patch::Hooks { track, ins, kinds: &kinds };
    let dests = patch::dests(s.id);
    let patchable = |d: &str| dests.contains(&d);
    // What lights up: every destination while a source or macro is dragged, else what the hovered
    // source drives.
    let lit: Vec<String> = match (st.drag, st.hover_source, st.hover_macro) {
        (Some((patch::Drag::Source(_) | patch::Drag::Macro(_), _)), _, _) => dests.iter().map(|d| (*d).to_string()).collect(),
        (_, Some(i), _) => {
            let id = soundcraft_dsp::plugins::synth_mod::SOURCES.get(i).map_or("", |x| x.id);
            ins.modulation
                .slots
                .iter()
                .filter(|x| soundcraft_dsp::plugins::synth_mod::source_index(&x.source) == Some(i) || x.source == id)
                .map(|x| x.dest.clone())
                .collect()
        }
        (_, _, Some(m)) => patch::macro_targets(ins, m),
        _ => Vec::new(),
    };
    // Live modulation: what the synth reads back from the audio thread this frame (the atomics
    // behind its analysis handle, read now, not at the meters' rate), unless Reduce motion keeps
    // the arcs static.
    let now = ui.input(|i| i.time);
    let motion = !app.extra.reduce_motion;
    let live = if motion { live_read(app, track) } else { None };
    // A macro just turned lights the arcs of everything it drives a moment.
    let flash: Vec<String> = match st.macro_touch {
        Some((m, at)) if motion && now - at < patch::MACRO_FLASH_S => patch::macro_targets(ins, m),
        _ => Vec::new(),
    };
    if motion && (live.is_some_and(|l| l.sounding) || !flash.is_empty()) {
        // Repaint only while a voice sounds or a lit macro fades; a moved control repaints by
        // itself.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    }
    let centres = RefCell::new(Vec::new());
    let outputs = RefCell::new(Vec::new());
    let cx = kit::Ctx {
        track,
        ins,
        info,
        face,
        arcs: &arcs,
        lit: &lit,
        patchable: &patchable,
        patching: Some(&hooks),
        centres: &centres,
        back: st.back,
        outputs: &outputs,
        motion,
        live,
        flash: &flash,
    };
    let top = Rect::from_min_size(inner.min, vec2(inner.width(), TOP_H));
    let clicked = top_bar(app, ui, &cx, top, s.name, s.kind, &[("Surprise", false), ("Front", !st.back), ("Back", st.back)]);
    match clicked {
        Some(0) => {
            if let Err(e) = app.run("mix.mod_surprise", json!({"track": track.0})) {
                app.ui.status = e;
            }
            st.moved_at = ui.input(|i| i.time);
        }
        Some(1) => st.back = false,
        Some(2) => st.back = true,
        _ => {}
    }
    let area = Rect::from_min_max(pos2(inner.left(), top.bottom() + UNDER_TOP), inner.max);
    let body = Rect::from_min_size(area.min, vec2(area.width(), s.body_h));
    (s.body)(app, ui, &cx, body);
    let row = Rect::from_min_max(pos2(area.left(), area.bottom() - kit::SECTION_H), area.max);
    if cx.back {
        // On the Back the sources' outputs sit on their modules: the macros take the row.
        patch::macros(app, ui, &cx, row, &mut st);
    } else {
        let parts = kit::row(row, &[kit::Share::Fixed(4.0 * 60.0 + 16.0), kit::Share::Knobs(1)]);
        if let [m, src] = parts.as_slice() {
            patch::macros(app, ui, &cx, *m, &mut st);
            st.hover_source = patch::sources(ui, &cx, *src, &mut st);
        }
    }
    if cx.back {
        let reduce = app.extra.reduce_motion;
        patch::back(app, ui, &cx, area, &mut st, reduce, cable_slot);
    } else {
        patch::front_drop(app, ui, &cx, &mut st);
    }
    patch::set_state(ui.ctx(), track, st);
}

/// The live modulation values the instrument of `track` publishes (see
/// `soundcraft_dsp::plugins::synth_mod::LIVE_SOUNDING`), read from its analysis handle on the
/// track's meter: `None` while the audio is off or the instance has none.
fn live_read(app: &SoundApp, track: TrackId) -> Option<soundcraft_dsp::plugins::synth_mod::LiveRead> {
    // (An offscreen render or a test feeds `preview_meters` instead of a player.)
    let m = app.preview_meters.as_ref().and_then(|p| p.tracks.get(&track)).or_else(|| app.meter_snapshot.tracks.get(&track))?;
    let (_, a) = m.analysis.iter().find(|(id, _)| *id == soundcraft_mix::INSTRUMENT_ANALYSIS)?;
    (a.live.len() >= soundcraft_dsp::plugins::synth_mod::LIVE_LEN).then(|| soundcraft_dsp::plugins::synth_mod::LiveRead::of(&a.live))
}

/// Whether `plugin` is drawn as an instrument panel.
pub fn has_panel(plugin: &str) -> bool {
    synth(plugin).is_some()
}

/// The height of a panel's top bar (lamp, badge, preset display, buttons).
pub const TOP_H: f32 = 28.0;

/// A panel's top bar across `r`: the lamp, the name badge, the preset display, and the panel
/// buttons named in `buttons` (right to left from the edge); returns which button was clicked.
pub fn top_bar(app: &mut SoundApp, ui: &mut Ui, cx: &kit::Ctx, r: Rect, name: &str, kind: &str, buttons: &[(&str, bool)]) -> Option<usize> {
    let face = cx.face;
    let p = ui.painter().clone();
    kit::lamp(&p, egui::pos2(r.left() + 5.0, r.center().y), face);
    let bw = kit::badge(&p, egui::pos2(r.left() + 16.0, r.center().y), name, kind, face);
    let mut x = r.right() - window_keys(app, ui, cx, r.right(), r.center().y) - 10.0;
    let mut clicked = None;
    for (i, (label, on)) in buttons.iter().enumerate().rev() {
        let w = kit::button_width(&p, label);
        let b = Rect::from_min_size(egui::pos2(x - w, r.center().y - 10.0), vec2(w, 20.0));
        if kit::button(ui, b, (cx.track.0, *label), label, *on, face).clicked() {
            clicked = Some(i);
        }
        x -= w + 5.0;
    }
    let lcd = Rect::from_min_max(egui::pos2(r.left() + 26.0 + bw, r.center().y - 11.0), egui::pos2(x - 5.0, r.center().y + 11.0));
    let out = kit::lcd(ui, lcd, cx.track.0, &preset_name(cx.ins), face);
    if out.prev {
        step_preset(app, cx.track, cx.info, cx.ins, -1);
    }
    if out.next {
        step_preset(app, cx.track, cx.info, cx.ins, 1);
    }
    if let Some(resp) = out.name {
        let info = cx.info;
        let track = cx.track;
        egui::Popup::menu(&resp).show(|ui| preset_menu(app, ui, track, info));
    }
    clicked
}

/// The window's own switches, small, in the panel's top bar (so the window reads as one
/// instrument): Detach (or Dock when it has a window of its own), Pin and Follow, as the plug-in
/// windows' row has them. Returns the width used, drawn right-aligned at `right`.
fn window_keys(app: &mut SoundApp, ui: &mut Ui, cx: &kit::Ctx, right: f32, y: f32) -> f32 {
    let key = (cx.track, soundcraft_mix::SlotKey::Instrument);
    let detached = app.ui.detached_plugins.contains(&key);
    let pinned = app.ui.pinned_plugins.contains(&key);
    let following = app.ui.following_plugins.contains(&key);
    let items: [(&str, bool, &str); 3] = [
        (
            if detached { "Dock" } else { "Detach" },
            false,
            if detached {
                "Put this window back inside the main window"
            } else {
                "Move this window into a window of its own that you can put anywhere (another screen too)"
            },
        ),
        ("Pin", pinned, "Keep this window on top of the others"),
        ("Follow", following, "Show this instrument on whichever track is selected"),
    ];
    let p = ui.painter().clone();
    let mut x = right;
    let mut clicked = None;
    for (i, (label, on, tip)) in items.iter().enumerate().rev() {
        let w = kit::small_key_width(&p, label);
        let r = Rect::from_min_size(pos2(x - w, y - 8.0), vec2(w, 16.0));
        if kit::small_key(ui, r, (cx.track.0, "win", i), label, *on, cx.face).on_hover_text(*tip).clicked() {
            clicked = Some(i);
        }
        x -= w + 3.0;
    }
    match clicked {
        Some(0) if detached => app.ui.detached_plugins.retain(|k| *k != key),
        Some(0) => app.ui.detached_plugins.push(key),
        Some(1) if pinned => app.ui.pinned_plugins.retain(|k| *k != key),
        Some(1) => app.ui.pinned_plugins.push(key),
        Some(2) if following => app.ui.following_plugins.retain(|k| *k != key),
        Some(2) => app.ui.following_plugins.push(key),
        _ => {}
    }
    right - x
}

/// The preset display's name: the preset loaded last, or "Init" for the defaults.
pub fn preset_name(ins: &Insert) -> String {
    if ins.preset.trim().is_empty() || ins.preset.starts_with('<') { "Init".to_string() } else { ins.preset.clone() }
}

/// The factory presets of `info`'s instrument in menu order: `(name, json)`.
fn factory_list(info: &soundcraft_dsp::PluginInfo) -> Vec<(&'static str, &'static str)> {
    crate::fs_presets::factory_groups(info).iter().flat_map(|(_, l)| l.iter().copied()).collect()
}

/// Loads factory preset `name` (its JSON `json`) into the instrument of `track`: every parameter
/// set, the name shown, one undo step.
pub fn recall(app: &mut SoundApp, track: TrackId, info: &soundcraft_dsp::PluginInfo, name: &str, json: &str) {
    let Some((preset, params)) = crate::fs_presets::factory_recall(info, name, json) else { return };
    let mut p = json!({"track": track.0, "plugin": info.id, "params": params, "preset": preset});
    // Its modulation patch too (a recall replaces the synth's patch; none clears it).
    if let (Some(m), Some(o)) = (crate::fs_presets::factory_modulation(json), p.as_object_mut()) {
        o.insert("modulation".into(), m);
    }
    if let Err(e) = app.run("mix.instrument", p) {
        app.ui.status = e;
    }
}

/// The preset display's previous (`step` -1) or next (+1) factory preset, from the one loaded.
pub fn step_preset(app: &mut SoundApp, track: TrackId, info: &soundcraft_dsp::PluginInfo, ins: &Insert, step: i32) {
    let list = factory_list(info);
    if list.is_empty() {
        return;
    }
    let n = list.len() as i32;
    let at = list.iter().position(|(name, _)| *name == ins.preset).map_or(if step > 0 { -1 } else { 0 }, |i| i as i32);
    let i = (at + step).rem_euclid(n) as usize;
    if let Some((name, json)) = list.get(i) {
        recall(app, track, info, name, json);
    }
}

/// The preset menu under the display's name: the factory presets by group, the user's own
/// presets, and Save as User Preset (the settings and the modulation patch).
pub fn preset_menu(app: &mut SoundApp, ui: &mut Ui, track: TrackId, info: &soundcraft_dsp::PluginInfo) {
    for (group, list) in crate::fs_presets::factory_groups(info) {
        ui.menu_button(group, |ui| {
            egui::ScrollArea::vertical().max_height((ui.ctx().content_rect().height() - 80.0).max(160.0)).show(ui, |ui| {
                crate::fs_presets::tiered_rows(ui, list, |ui, name, json| {
                    let b = ui.button(name);
                    let b = match crate::fs_presets::preset_field(json, "desc") {
                        Some(d) => b.on_hover_text(d.replace("\\\"", "\"")),
                        None => b,
                    };
                    if b.clicked() {
                        recall(app, track, info, name, json);
                        ui.close();
                    }
                });
            });
        });
    }
    let users = crate::preset_house::user_names(app, info.id);
    if !users.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("User").small().strong());
        for name in users {
            if ui.button(&name).clicked() {
                recall_user(app, track, info, &name);
                ui.close();
            }
        }
    }
    ui.separator();
    let id = egui::Id::new(("synth-save-preset", track.0));
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_default();
    ui.horizontal(|ui| {
        let edit = ui.add(egui::TextEdit::singleline(&mut text).hint_text("New preset name").desired_width(150.0));
        let go = ui.add_enabled(!text.trim().is_empty(), egui::Button::new("Save")).clicked()
            || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if go && !text.trim().is_empty() {
            match save_user(app, track, info, text.trim()) {
                Ok(n) => app.ui.status = format!("Saved the preset {n}"),
                Err(e) => app.ui.status = e,
            }
            text.clear();
            ui.close();
        }
    });
    ui.data_mut(|d| d.insert_temp(id, text));
}

/// Saves the synth of `track` as user preset `name`: every parameter and its modulation patch.
pub fn save_user(app: &mut SoundApp, track: TrackId, info: &soundcraft_dsp::PluginInfo, name: &str) -> Result<String, String> {
    if crate::editor_kit::presets::is_factory_name(info, name) {
        return Err(format!("{name} is a factory preset: choose another name"));
    }
    let ins = app.engine.session().track(track).and_then(|t| t.instrument.clone()).ok_or("The track has no instrument")?;
    let value = |id: &str| ins.params.get(id).copied().or_else(|| info.param(id).map(|p| p.default)).unwrap_or(0.0);
    let patch = serde_json::to_value(&ins.modulation).map_err(|e| e.to_string())?;
    let body = crate::editor_kit::presets::user_preset_body(info, &[], value, name, "", Some(("modulation", patch)));
    crate::preset_house::write_user(app, info.id, name, &body)
}

/// Loads user preset `name` into the synth of `track`: its parameters and its patch (one undo
/// step).
pub fn recall_user(app: &mut SoundApp, track: TrackId, info: &soundcraft_dsp::PluginInfo, name: &str) {
    let Some(params) = crate::preset_house::user_params(app, info, name) else {
        app.ui.status = format!("The preset {name} could not be read");
        return;
    };
    let patch = crate::preset_house::user_dir(app, info.id)
        .and_then(|d| std::fs::read_to_string(d.join(format!("{name}.json"))).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("modulation").filter(|m| !m.is_null()).cloned());
    let mut p = json!({"track": track.0, "plugin": info.id, "params": params, "preset": name});
    if let (Some(m), Some(o)) = (patch, p.as_object_mut()) {
        o.insert("modulation".into(), m);
    }
    if let Err(e) = app.run("mix.instrument", p) {
        app.ui.status = e;
    }
}

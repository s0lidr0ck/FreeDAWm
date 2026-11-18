//! The built-in synths drawn as instruments: Burley (the subtractive synth), Cicada (the
//! wavetable synth) and Steeple (the FM synth). Each is a printed panel between wood end cheeks
//! with its sections framed in signal order, real knobs and envelope faders, inset displays and
//! a preset display, built from one kit ([`kit`]) and coloured by its own [`kit::Face`].
//!
//! A panel is drawn at its own size at the top of the instrument's window (the window scrolls
//! when it is smaller); every parameter of the synth is on it.

pub mod burley;
pub mod kit;

use crate::SoundApp;
use egui::{Color32, Rect, Sense, Ui, Vec2, vec2};
use kit::Face;
use serde_json::json;
use soundcraft_model::{Insert, TrackId};

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
    wood: (c(0x2a211a), c(0x4a3a2c)),
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
};

/// The size `plugin`'s panel is drawn at, when it has one.
pub fn size(plugin: &str) -> Option<Vec2> {
    match plugin {
        burley::ID => Some(burley::SIZE),
        _ => None,
    }
}

/// Whether `plugin` is drawn as an instrument panel.
pub fn has_panel(plugin: &str) -> bool {
    size(plugin).is_some()
}

/// Draws `ins` (the instrument of `track`) as its panel, inside a scroll area as large as the
/// window gives. `frozen`: the track is frozen (the panel is shown, its controls greyed).
pub fn draw(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, frozen: bool) {
    let Some(sz) = size(&ins.plugin) else { return };
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
            if ins.plugin == burley::ID {
                burley::draw(app, &mut pui, track, ins, r);
            }
        },
    );
}

/// The height of a panel's top bar (lamp, badge, preset display, buttons).
pub const TOP_H: f32 = 36.0;

/// A panel's top bar across `r`: the lamp, the name badge, the preset display, and the panel
/// buttons named in `buttons` (right to left from the edge); returns which button was clicked.
pub fn top_bar(app: &mut SoundApp, ui: &mut Ui, cx: &kit::Ctx, r: Rect, name: &str, kind: &str, buttons: &[(&str, bool)]) -> Option<usize> {
    let face = cx.face;
    let p = ui.painter().clone();
    kit::lamp(&p, egui::pos2(r.left() + 6.0, r.center().y), face);
    let bw = kit::badge(&p, egui::pos2(r.left() + 20.0, r.center().y), name, kind, face);
    let mut x = r.right();
    let mut clicked = None;
    for (i, (label, on)) in buttons.iter().enumerate().rev() {
        let w = kit::button_width(&p, label);
        let b = Rect::from_min_size(egui::pos2(x - w, r.center().y - 13.0), vec2(w, 26.0));
        if kit::button(ui, b, (cx.track.0, *label), label, *on, face).clicked() {
            clicked = Some(i);
        }
        x -= w + 8.0;
    }
    let lcd = Rect::from_min_max(egui::pos2(r.left() + 32.0 + bw, r.center().y - 14.0), egui::pos2(x - 6.0, r.center().y + 14.0));
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
    if let Some((preset, params)) = crate::fs_presets::factory_recall(info, name, json)
        && let Err(e) = app.run("mix.instrument", json!({"track": track.0, "plugin": info.id, "params": params, "preset": preset}))
    {
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

/// The preset menu under the display's name: the factory presets by group.
pub fn preset_menu(app: &mut SoundApp, ui: &mut Ui, track: TrackId, info: &soundcraft_dsp::PluginInfo) {
    for (group, list) in crate::fs_presets::factory_groups(info) {
        ui.menu_button(group, |ui| {
            for (name, json) in list {
                if ui.button(*name).clicked() {
                    recall(app, track, info, name, json);
                    ui.close();
                }
            }
        });
    }
}

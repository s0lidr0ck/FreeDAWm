//! Juke's effect slots: four on each effects bus and on the output, each holding a FireStation
//! (or other built-in) effect. A filled slot opens the effect's full editor in a window of its
//! own (its parameter window when it has no full editor); an empty one offers the effects.

use crate::theme::{bold, regular};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2};
use serde_json::{Value, json};
use soundcraft_model::{Insert, KitBus, KitFx, ModuleId, TrackId};

const LABEL: Color32 = Color32::from_rgb(0xD9, 0xCC, 0xB6);
const CREAM: Color32 = Color32::from_rgb(0xF6, 0xEE, 0xDF);
const AMBER: Color32 = Color32::from_rgb(0xF0, 0xB2, 0x4A);

/// What a slot asked for.
pub enum FxAction {
    /// Open the effect's editor.
    Open(ModuleId),
    /// Run a command (`mix.kit_fx`).
    Run(&'static str, Value),
}

/// The effects a slot offers: built-in, not instruments, FireStation's first.
pub fn effects() -> Vec<&'static soundcraft_dsp::PluginInfo> {
    let mut v: Vec<_> = soundcraft_dsp::listed_plugins().filter(|p| !p.is_instrument).collect();
    v.sort_by_key(|p| (!p.with_engines(), p.name));
    v
}

fn pick_menu(ui: &mut Ui, tid: TrackId, bus: KitBus, slot: usize, out: &mut Option<FxAction>) {
    let (fs, rest): (Vec<_>, Vec<_>) = effects().into_iter().partition(|p| p.with_engines());
    let mut item = |ui: &mut Ui, p: &soundcraft_dsp::PluginInfo| {
        if ui.button(crate::chain_view::plugin_name(p.id)).clicked() {
            *out = Some(FxAction::Run("mix.kit_fx", json!({"track": tid.0, "bus": bus.id(), "slot": slot + 1, "plugin": p.id})));
            ui.close();
        }
    };
    for p in fs {
        item(ui, p);
    }
    if !rest.is_empty() {
        ui.separator();
        ui.menu_button("More", |ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for p in rest {
                    item(ui, p);
                }
            });
        });
    }
}

/// Draws effect slot `slot` (from 0) of `bus`; returns what it asked for.
pub fn slot(ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, bus: KitBus, slot: usize) -> Option<FxAction> {
    let mut out = None;
    let id = egui::Id::new(("juke-fx", tid.0, bus.id(), slot));
    let resp = ui.interact(r, id, Sense::click());
    let p = ui.painter().clone();
    match ins.kit_effect(bus, slot) {
        Some(fx) => {
            let name = crate::chain_view::plugin_name(&fx.insert.plugin);
            let on = !fx.insert.bypass;
            let fill = if resp.hovered() { Color32::from_black_alpha(80) } else { Color32::from_black_alpha(55) };
            p.rect_filled(r, 3.0, fill);
            p.rect_stroke(r, 3.0, Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Inside);
            p.circle_filled(pos2(r.min.x + 7.0, r.center().y), 3.0, if on { AMBER } else { LABEL.gamma_multiply(0.35) });
            let short = name.strip_prefix("FireStation ").unwrap_or(&name).to_string();
            let galley = p.layout_no_wrap(short, bold(9.5), if on { CREAM } else { LABEL.gamma_multiply(0.6) });
            let at = pos2(r.min.x + 13.0, r.center().y - galley.size().y * 0.5);
            p.with_clip_rect(r.shrink(1.0)).galley(at, galley, CREAM);
            let module = KitFx::module_id(KitFx::index(bus, slot));
            let resp = resp.on_hover_text(format!("{name}: click to open it, right-click for more"));
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Bus {} effect {}", bus.id(), slot + 1)));
            if resp.clicked() {
                out = Some(FxAction::Open(module));
            }
            resp.context_menu(|ui| {
                if ui.button("Open").clicked() {
                    out = Some(FxAction::Open(module));
                    ui.close();
                }
                if ui.button(if on { "Bypass" } else { "Turn On" }).clicked() {
                    out = Some(FxAction::Run("mix.kit_fx", json!({"track": tid.0, "bus": bus.id(), "slot": slot + 1, "bypass": on})));
                    ui.close();
                }
                ui.menu_button("Replace", |ui| pick_menu(ui, tid, bus, slot, &mut out));
                ui.separator();
                if ui.button("Remove").clicked() {
                    out = Some(FxAction::Run("mix.kit_fx", json!({"track": tid.0, "bus": bus.id(), "slot": slot + 1, "plugin": ""})));
                    ui.close();
                }
            });
        }
        None => {
            let c = if resp.hovered() { LABEL } else { LABEL.gamma_multiply(0.45) };
            dashed(&p, r, c);
            p.text(r.center(), Align2::CENTER_CENTER, "Add effect", regular(9.5), c);
            let resp = resp.on_hover_text("Put an effect on this bus");
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Add effect on bus {} {}", bus.id(), slot + 1)));
            egui::Popup::menu(&resp).show(|ui| pick_menu(ui, tid, bus, slot, &mut out));
        }
    }
    out
}

/// A dashed outline: an empty slot.
fn dashed(p: &egui::Painter, r: Rect, c: Color32) {
    let r = r.shrink(0.5);
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in corners.windows(2) {
        if let [a, b] = w {
            p.add(egui::Shape::dashed_line(&[*a, *b], Stroke::new(1.0, c), 3.0, 2.0));
        }
    }
}

/// Opens kit effect `module`'s editor on track `tid`: its full editor, else its parameter window.
pub fn open(app: &mut crate::SoundApp, tid: TrackId, module: ModuleId) {
    let plugin = app.engine.session().track(tid).and_then(|t| t.module(module)).map(|i| i.plugin.clone()).unwrap_or_default();
    if crate::editors::has_wide(&plugin) {
        app.ui.full_editors.retain(|k| *k != (tid, module));
        app.ui.full_editors.push((tid, module));
        app.module_focus = Some((tid, soundcraft_mix::SlotKey::Module(module)));
    } else {
        crate::chain_view::show_parameters(app, tid, soundcraft_mix::SlotKey::Module(module));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_offer_effects_firestation_first() {
        let v = effects();
        assert!(v.iter().all(|p| !p.is_instrument));
        assert!(v.first().is_some_and(|p| p.category == soundcraft_dsp::Category::FireStation));
        assert!(v.iter().any(|p| p.id == "fs_reverb"));
    }
}

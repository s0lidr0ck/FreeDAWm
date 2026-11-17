//! The Modulators window: a track's LFOs and envelope followers and the plug-in parameters they
//! move (`mix.modulator_add`, `mix.modulator_set`, `mix.modulator_target`,
//! `mix.modulator_remove`; a drag is one undo step). Opened from an insert's menu (Modulators…)
//! for that insert's track.

use crate::SoundApp;
use crate::theme::Tokens;
use egui::vec2;
use serde_json::{Value, json};
use soundcraft_model::{LFO_SHAPES, LFO_SYNC, MAX_MOD_TARGETS, MAX_MODULATORS, ModulatorKind, ModuleId, TrackId};

/// A command to run after the window is drawn, merged into one undo step by `key` while a
/// control is dragged.
type Ask = (&'static str, Value, Option<String>);

/// Opens the window on `track`'s modulators.
pub(crate) fn open(app: &mut SoundApp, track: TrackId) {
    app.ui.modulators_window = Some(track);
}

pub(crate) fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(tid) = app.ui.modulators_window else { return };
    let Some(track) = app.engine.session().track(tid).cloned() else {
        app.ui.modulators_window = None;
        return;
    };
    let mut open = true;
    let mut asks: Vec<Ask> = Vec::new();
    let mut stop_merge = false;
    // The chain's modules and their parameters, for the target pickers.
    let modules: Vec<(ModuleId, String, Vec<(&'static str, &'static str)>)> = track
        .mixer
        .inserts
        .iter()
        .filter_map(|i| {
            let info = crate::mix_window::plugin_info(&i.plugin)?;
            let params = info.params.iter().filter(|p| p.id != "bypass").map(|p| (p.id, p.name)).collect();
            Some((i.id, crate::chain_view::tile_name(i), params))
        })
        .collect();
    egui::Window::new(format!("Modulators · {}", track.name))
        .id(egui::Id::new("modulators-window"))
        .open(&mut open)
        .default_size(vec2(400.0, 420.0))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                if track.modulators.is_empty() {
                    ui.label("No modulators yet. An LFO or an envelope follower moves any plug-in parameter on this track.");
                }
                for m in &track.modulators {
                    let base = |k: &str, v: Value| {
                        let mut p = serde_json::Map::new();
                        p.insert("track".into(), json!(tid.0));
                        p.insert("modulator".into(), json!(m.id));
                        p.insert(k.into(), v);
                        Value::Object(p)
                    };
                    ui.horizontal(|ui| {
                        ui.strong(format!("{} {}", m.kind.name(), m.id));
                        let mut on = !m.bypass;
                        if ui.checkbox(&mut on, "On").changed() {
                            asks.push(("mix.modulator_set", base("bypass", json!(!on)), None));
                        }
                        if ui.small_button("Remove").clicked() {
                            asks.push(("mix.modulator_remove", json!({"track": tid.0, "modulator": m.id}), None));
                        }
                    });
                    match m.kind {
                        ModulatorKind::Lfo => {
                            ui.horizontal(|ui| {
                                let mut shape = usize::from(m.shape).min(LFO_SHAPES.len() - 1);
                                egui::ComboBox::from_id_salt(("mod-shape", m.id)).selected_text(LFO_SHAPES[shape]).show_ui(ui, |ui| {
                                    for (i, s) in LFO_SHAPES.iter().enumerate() {
                                        if ui.selectable_value(&mut shape, i, *s).clicked() {
                                            asks.push(("mix.modulator_set", base("shape", json!(i)), None));
                                        }
                                    }
                                });
                                let sync = usize::from(m.sync);
                                let shown = if sync == 0 { "Free" } else { LFO_SYNC.get(sync - 1).map_or("Free", |s| s.0) };
                                egui::ComboBox::from_id_salt(("mod-sync", m.id)).selected_text(shown).show_ui(ui, |ui| {
                                    if ui.selectable_label(sync == 0, "Free").clicked() {
                                        asks.push(("mix.modulator_set", base("sync", json!(0)), None));
                                    }
                                    for (i, (name, _)) in LFO_SYNC.iter().enumerate() {
                                        if ui.selectable_label(sync == i + 1, *name).clicked() {
                                            asks.push(("mix.modulator_set", base("sync", json!(i + 1)), None));
                                        }
                                    }
                                });
                            });
                            if m.sync == 0 {
                                let mut rate = m.rate_hz;
                                let r = ui.add(egui::Slider::new(&mut rate, 0.01..=40.0).logarithmic(true).suffix(" Hz").text("Rate"));
                                if r.changed() {
                                    asks.push(("mix.modulator_set", base("rate", json!(rate)), Some(format!("mod-rate.{}.{}", tid.0, m.id))));
                                }
                                stop_merge |= r.drag_stopped();
                            }
                        }
                        ModulatorKind::Envelope => {
                            for (key, label, v) in [("attack", "Attack", m.attack_ms), ("release", "Release", m.release_ms)] {
                                let mut x = v;
                                let r = ui.add(egui::Slider::new(&mut x, 0.1..=2000.0).logarithmic(true).suffix(" ms").text(label));
                                if r.changed() {
                                    asks.push(("mix.modulator_set", base(key, json!(x)), Some(format!("mod-{key}.{}.{}", tid.0, m.id))));
                                }
                                stop_merge |= r.drag_stopped();
                            }
                            ui.label(
                                egui::RichText::new("Follows the track's signal where it enters the inserts")
                                    .small()
                                    .color(Tokens::current().text_dim),
                            );
                        }
                    }
                    // What it moves.
                    for t in &m.targets {
                        let (mname, pname) = modules
                            .iter()
                            .find(|(id, _, _)| *id == t.module)
                            .map(|(_, n, ps)| (n.clone(), ps.iter().find(|(id, _)| *id == t.param).map_or(t.param.clone(), |p| p.1.to_string())))
                            .unwrap_or_else(|| ("(removed)".into(), t.param.clone()));
                        ui.horizontal(|ui| {
                            let mut depth = (t.depth * 100.0).round();
                            let r = ui.add(egui::Slider::new(&mut depth, -100.0..=100.0).suffix(" %").text(format!("{mname} · {pname}")));
                            let aim = |d: f32| json!({"track": tid.0, "modulator": m.id, "module": t.module.0, "param": t.param, "depth": d});
                            if r.changed() {
                                asks.push((
                                    "mix.modulator_target",
                                    aim(depth / 100.0),
                                    Some(format!("mod-depth.{}.{}.{}.{}", tid.0, m.id, t.module.0, t.param)),
                                ));
                            }
                            stop_merge |= r.drag_stopped();
                            if ui.small_button("✕").on_hover_text("Stop moving this parameter").clicked() {
                                asks.push(("mix.modulator_target", aim(0.0), None));
                            }
                        });
                    }
                    if m.targets.len() < MAX_MOD_TARGETS && !modules.is_empty() {
                        ui.menu_button("Modulate a Parameter", |ui| {
                            for (id, name, params) in &modules {
                                ui.menu_button(name.as_str(), |ui| {
                                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                        for (pid, pname) in params {
                                            if ui.button(*pname).clicked() {
                                                let p = json!({"track": tid.0, "modulator": m.id, "module": id.0, "param": pid, "depth": 0.25});
                                                asks.push(("mix.modulator_target", p, None));
                                                ui.close();
                                            }
                                        }
                                    });
                                });
                            }
                        });
                    }
                    ui.separator();
                }
                if track.modulators.len() < MAX_MODULATORS {
                    ui.horizontal(|ui| {
                        for k in ModulatorKind::ALL {
                            if ui.button(format!("Add {}", k.name())).clicked() {
                                asks.push(("mix.modulator_add", json!({"track": tid.0, "kind": k.key()}), None));
                            }
                        }
                    });
                }
            });
        });
    for (cmd, p, key) in asks {
        let r = match key {
            Some(k) => app.engine.execute_merged(cmd, &p, &k).map(|_| ()).map_err(|e| e.to_string()),
            None => app.run(cmd, p).map(|_| ()),
        };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
    if stop_merge {
        app.engine.end_merge();
    }
    if !open {
        app.ui.modulators_window = None;
    }
}

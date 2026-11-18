//! Window › Transport & Session › Control Room: the monitor speakers (sets with level, dim, mono and mute), the listen
//! level of AFL/PFL solo, the cue mixes with their sends, and talkback (the Talk button, or F11
//! held while the session has a control room with a talkback input). Every control runs a
//! `monitor.*` command.

use crate::SoundApp;
use egui::{Key, vec2};
use serde_json::{Value, json};

/// The key held to talk (see [`talk_key`]).
pub const TALK_KEY: Key = Key::F11;

/// F11 pressed (no modifiers): talkback on while it is held, when the session has a control room
/// with a talkback input. Returns false otherwise (the key then means whatever it is bound to).
pub fn talk_key(app: &mut SoundApp) -> bool {
    let ready = app.engine.session().control_room.as_ref().is_some_and(|r| !r.talkback.input.is_empty());
    if ready {
        let _ = app.run("monitor.talkback", json!({"on": true}));
        app.ui.talk_held = true;
    }
    ready
}

/// The window, and the end of a held talk key (every frame).
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if app.ui.talk_held && !ctx.input(|i| i.key_down(TALK_KEY)) {
        app.ui.talk_held = false;
        let _ = app.run("monitor.talkback", json!({"on": false}));
    }
    if !app.ui.show_control_room {
        return;
    }
    let mut open = true;
    let mut run: Vec<(&'static str, Value)> = Vec::new();
    let s = app.engine.session();
    let outputs: Vec<String> = s.outputs.iter().map(|o| o.name.clone()).collect();
    let mut inputs: Vec<String> = s.inputs.iter().map(|i| i.name.clone()).collect();
    if inputs.is_empty() {
        inputs.extend((1..=8).map(|n| format!("In {n}")));
    }
    let room = s.control_room.clone();
    let solo_mode = s.edit.solo_mode.clone();
    egui::Window::new("Control Room").open(&mut open).default_size(vec2(360.0, 420.0)).show(ctx, |ui| {
        let Some(r) = room else {
            ui.label("This session has no control room: the main mix plays on the main output.");
            if ui.button("Use a Control Room").clicked() {
                run.push(("monitor.control_room", json!({"on": true})));
            }
            return;
        };
        ui.heading("Monitor");
        ui.horizontal(|ui| {
            for (i, set) in r.sets.iter().enumerate() {
                if ui.selectable_label(i == r.active_set, &set.name).on_hover_text("Play the monitor on these speakers").clicked() {
                    run.push(("monitor.select_set", json!({"set": i})));
                }
            }
        });
        let mut level = r.set().map_or(0.0, |x| x.level_db);
        if ui.add(egui::Slider::new(&mut level, -60.0..=12.0).text("Level dB")).changed() {
            run.push(("monitor.level", json!({"db": level})));
        }
        ui.horizontal(|ui| {
            if ui.selectable_label(r.dim, "Dim").clicked() {
                run.push(("monitor.dim", json!({"on": !r.dim})));
            }
            if ui.selectable_label(r.mono, "Mono").clicked() {
                run.push(("monitor.mono", json!({"on": !r.mono})));
            }
            if ui.selectable_label(r.mute, "Mute").clicked() {
                run.push(("monitor.mute", json!({"on": !r.mute})));
            }
            let mut dim_db = r.dim_db;
            if ui.add(egui::DragValue::new(&mut dim_db).range(-60.0..=0.0).suffix(" dB dim")).changed() {
                run.push(("monitor.dim", json!({"db": dim_db})));
            }
        });
        ui.horizontal(|ui| {
            let mut listen = r.listen_db;
            if ui.add(egui::Slider::new(&mut listen, -60.0..=12.0).text("Listen dB")).changed() {
                run.push(("monitor.listen_level", json!({"db": listen})));
            }
        });
        let listening = solo_mode.starts_with("afl") || solo_mode.starts_with("pfl");
        ui.horizontal(|ui| {
            ui.label("Solo:");
            for (mode, label) in [("sip", "In Place"), ("afl", "AFL"), ("pfl", "PFL")] {
                if ui.selectable_label(solo_mode.starts_with(mode), label).clicked() {
                    run.push(("options.solo_mode", json!({"mode": mode, "xor": solo_mode.contains("xor")})));
                }
            }
        });
        if listening {
            ui.label(egui::RichText::new("A solo is heard on the listen bus; the mix and bounces play on unchanged.").weak());
        }
        ui.separator();
        ui.collapsing("Monitor Sets", |ui| {
            for (i, set) in r.sets.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(&set.name);
                    let shown = if set.output.is_empty() { "Main output".to_string() } else { set.output.clone() };
                    egui::ComboBox::from_id_salt(("cr_set_out", i)).selected_text(shown).show_ui(ui, |ui| {
                        if ui.selectable_label(set.output.is_empty(), "Main output").clicked() {
                            run.push(("monitor.set", json!({"set": i, "output": ""})));
                        }
                        for o in outputs.iter().skip(1) {
                            if ui.selectable_label(o.eq_ignore_ascii_case(&set.output), o).clicked() {
                                run.push(("monitor.set", json!({"set": i, "output": o})));
                            }
                        }
                    });
                    let mut trim = set.trim_db;
                    if ui.add(egui::DragValue::new(&mut trim).range(-24.0..=24.0).suffix(" dB trim")).changed() {
                        run.push(("monitor.set", json!({"set": i, "trim_db": trim})));
                    }
                    if r.sets.len() > 1 && ui.small_button("Remove").clicked() {
                        run.push(("monitor.set_remove", json!({"set": i})));
                    }
                });
            }
            if ui.button("Add Monitor Set").clicked() {
                run.push(("monitor.set", json!({})));
            }
        });
        ui.collapsing("Cue Mixes", |ui| {
            for (i, cue) in r.cues.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(&cue.name);
                    egui::ComboBox::from_id_salt(("cr_cue_out", i)).selected_text(cue.output.clone()).show_ui(ui, |ui| {
                        for o in outputs.iter().skip(1) {
                            if ui.selectable_label(o.eq_ignore_ascii_case(&cue.output), o).clicked() {
                                run.push(("monitor.cue", json!({"cue": i, "output": o})));
                            }
                        }
                    });
                    let mut lvl = cue.level_db;
                    if ui.add(egui::DragValue::new(&mut lvl).range(-60.0..=12.0).suffix(" dB")).changed() {
                        run.push(("monitor.cue", json!({"cue": i, "level_db": lvl})));
                    }
                    if ui.selectable_label(cue.mute, "Mute").clicked() {
                        run.push(("monitor.cue", json!({"cue": i, "mute": !cue.mute})));
                    }
                    if ui.selectable_label(cue.talkback, "TB").on_hover_text("Talkback is heard in this cue").clicked() {
                        run.push(("monitor.cue", json!({"cue": i, "talkback": !cue.talkback})));
                    }
                });
                ui.horizontal(|ui| {
                    if ui
                        .small_button("Add Cue Sends")
                        .on_hover_text("A pre-fader send to this cue on every track that has none, at the track's fader level")
                        .clicked()
                    {
                        run.push(("monitor.cue_sends", json!({"cue": i, "copy_fader": true})));
                    }
                    if ui.small_button("Remove Cue").clicked() {
                        run.push(("monitor.cue_remove", json!({"cue": i})));
                    }
                });
            }
            match outputs.get(1) {
                Some(first) => {
                    if ui.button("Add Cue Mix").clicked() {
                        run.push(("monitor.cue", json!({"output": first})));
                    }
                }
                None => {
                    ui.label(egui::RichText::new("A cue needs a hardware output of its own (Setup › I/O).").weak());
                }
            }
        });
        ui.collapsing("Talkback", |ui| {
            let tb = &r.talkback;
            ui.horizontal(|ui| {
                ui.label("Input");
                let shown = if tb.input.is_empty() { "None".to_string() } else { tb.input.clone() };
                egui::ComboBox::from_id_salt("cr_tb_in").selected_text(shown).show_ui(ui, |ui| {
                    for name in &inputs {
                        if ui.selectable_label(name.eq_ignore_ascii_case(&tb.input), name).clicked() {
                            run.push(("monitor.talkback_setup", json!({"input": name})));
                        }
                    }
                });
                let mut lvl = tb.level_db;
                if ui.add(egui::DragValue::new(&mut lvl).range(-60.0..=12.0).suffix(" dB")).changed() {
                    run.push(("monitor.talkback_setup", json!({"level_db": lvl})));
                }
                let mut dims = tb.dims;
                if ui.checkbox(&mut dims, "Dims monitor").changed() {
                    run.push(("monitor.talkback_setup", json!({"dims": dims})));
                }
            });
            ui.horizontal(|ui| {
                let talk = ui.add_enabled(!tb.input.is_empty(), egui::Button::selectable(tb.on, "Talk"));
                // Held: on while the button is down (unless latched on anyway).
                let down = talk.is_pointer_button_down_on();
                if down != app.ui.talk_button {
                    app.ui.talk_button = down;
                    if !app.ui.talk_latched {
                        run.push(("monitor.talkback", json!({"on": down})));
                    }
                }
                let mut latch = app.ui.talk_latched;
                if ui.checkbox(&mut latch, "Latch").on_hover_text("Talkback stays on until switched off").changed() {
                    app.ui.talk_latched = latch;
                    run.push(("monitor.talkback", json!({"on": latch})));
                }
                ui.label(egui::RichText::new("Hold Talk or F11 to talk").weak());
            });
        });
        ui.separator();
        if ui.small_button("Stop Using a Control Room").clicked() {
            run.push(("monitor.control_room", json!({"on": false})));
        }
    });
    app.ui.show_control_room = open;
    for (cmd, p) in run {
        let _ = app.run(cmd, p);
    }
}

//! The Edit window toolbar, and the Mix window's slim top bar.

use crate::theme::{Tokens, bold, mono, regular};
use crate::widgets::{icon_button, key, key_fill, text_toggle};
use crate::{SoundApp, icons};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::edit::{SHUFFLE_LOCK, SNAP_TO_GRID};
use soundcraft_model::{EditMode, Tool};
use soundcraft_time::{TimeFormat, format_length};

pub const HEIGHT: f32 = 78.0;

fn group(ui: &mut Ui, w: f32, h: f32, add: impl FnOnce(&mut Ui, Rect)) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.painter().rect(r, CornerRadius::same(4), t.toolbar_group, Stroke::new(1.0, t.group_border), StrokeKind::Inside);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(5.0)).layout(egui::Layout::left_to_right(egui::Align::Min)));
    add(&mut child, r);
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, t.toolbar_bg);
    ui.add_space(4.0);
    egui::ScrollArea::horizontal()
        .id_salt("edit_toolbar_scroll")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.spacing_mut().item_spacing.x = 8.0;
                edit_modes(app, ui);
                zoom_group(app, ui);
                tools(app, ui);
                counters(app, ui);
                grid_nudge(app, ui);
                transport(app, ui);
                #[cfg(not(target_arch = "wasm32"))]
                follow_badge(app, ui);
                scout_button(app, ui);
                tempo_meter(app, ui);
                output_meter(app, ui);
            });
        });
}

fn edit_modes(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let ed = &app.engine.session().edit;
    let (cur, snap, locked) = (ed.edit_mode, ed.flag(SNAP_TO_GRID), ed.flag(SHUFFLE_LOCK));
    let cmd = crate::shortcuts::shortcut_label("Cmd", ui.ctx().os() == egui::os::OperatingSystem::Mac);
    let modes = [
        (EditMode::Shuffle, 0, 0, "SHUFFLE", "F1"),
        (EditMode::Spot, 1, 0, "SPOT", "F3"),
        (EditMode::Slip, 0, 1, "SLIP", "F2"),
        (EditMode::Grid, 1, 1, "GRID", "F4"),
    ];
    let widest =
        modes.iter().map(|m| m.3).chain(["REL GRID"]).map(|l| ui.painter().layout_no_wrap(l.into(), bold(10.5), t.text).size().x).fold(0.0, f32::max);
    group(ui, 2.0 * widest + 44.0, 52.0, |ui, r| {
        let inner = r.shrink(6.0);
        let cw = inner.width() / 2.0;
        let ch = inner.height() / 2.0;
        for (m, cx, cy, label, fkey) in modes {
            let cell = Rect::from_min_size(pos2(inner.min.x + cw * cx as f32, inner.min.y + ch * cy as f32), vec2(cw, ch)).shrink(1.0);
            let resp = ui.interact(cell, ui.id().with(("mode", label)), Sense::click());
            let on = cur == m || (m == EditMode::Grid && cur == EditMode::GridRelative);
            key(ui.painter(), cell, &resp, if on { t.mode_on } else { t.mode_off });
            let fg = if on { t.mode_on_text } else { t.mode_off_text };
            if m == EditMode::Grid && snap {
                ui.painter().rect_stroke(cell, 1.0, Stroke::new(1.5, t.mode_on), StrokeKind::Inside);
            }
            if m == EditMode::Shuffle && locked {
                icons::draw(ui.painter(), Rect::from_center_size(cell.center(), vec2(15.0, 15.0)), "lock", fg);
            } else {
                let txt = if m == EditMode::Grid && cur == EditMode::GridRelative { "REL GRID" } else { label };
                ui.painter().text(cell.center(), Align2::CENTER_CENTER, txt, bold(10.5), fg);
            }
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, label));
            if resp.clicked() {
                mode_click(app, m, ui.input(|i| i.modifiers));
            }
            resp.on_hover_text(match m {
                EditMode::Shuffle if locked => format!("Shuffle is locked ({cmd}-click to unlock)"),
                EditMode::Shuffle => format!("Shuffle mode ({fkey}); {cmd}-click locks it out"),
                EditMode::Grid => format!("Grid mode ({fkey}; again for Relative); Shift-click: Snap to Grid in the other modes"),
                _ => format!("{} mode ({fkey}); Shift-click adds Snap to Grid", if m == EditMode::Slip { "Slip" } else { "Spot" }),
            });
        }
    });
}

/// Shift adds Snap to Grid to Shuffle, Slip and Spot (on Grid it toggles it for the current mode);
/// Cmd/Ctrl on Shuffle toggles Shuffle Lock.
fn mode_click(app: &mut SoundApp, m: EditMode, mods: egui::Modifiers) {
    let cur = app.engine.session().edit.edit_mode;
    let _ = if m == EditMode::Shuffle && mods.command {
        app.run("edit.shuffle_lock", json!({}))
    } else if m == EditMode::Grid && mods.shift && !cur.is_grid() {
        app.run("edit.snap_to_grid", json!({}))
    } else {
        let id = match m {
            EditMode::Shuffle => "shuffle",
            EditMode::Spot => "spot",
            EditMode::Slip => "slip",
            _ => crate::shortcuts::grid_mode(app),
        };
        app.run("edit.mode", json!({"mode": id, "snap_to_grid": mods.shift}))
    };
}

fn zoom_group(app: &mut SoundApp, ui: &mut Ui) {
    group(ui, 128.0, 52.0, |ui, _| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 3.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if icon_button(ui, vec2(28.0, 22.0), "minus", false, "Zoom out (Cmd+[)").clicked() {
                    let _ = app.run("view.zoom_out", json!({}));
                }
                if icon_button(ui, vec2(30.0, 22.0), "wave", false, "Waveform zoom in (Cmd+Alt+])").clicked() {
                    let _ = app.run("view.waveform_zoom", json!({"factor": 2.0}));
                }
                if icon_button(ui, vec2(28.0, 22.0), "plus", false, "Zoom in (Cmd+])").clicked() {
                    let _ = app.run("view.zoom_in", json!({}));
                }
                if icon_button(ui, vec2(28.0, 22.0), "zoom", false, "Fill window with session (Alt+A)").clicked() {
                    let w = ui.ctx().content_rect().width() - 420.0;
                    let _ = app.run("view.zoom_fit", json!({"width_px": w}));
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for i in 1..=5 {
                    let r = crate::widgets::text_toggle(
                        ui,
                        vec2(22.0, 18.0),
                        &i.to_string(),
                        false,
                        Color32::GRAY,
                        "Zoom preset (click to recall, Cmd-click to store)",
                    );
                    if r.clicked() {
                        let store = ui.input(|x| x.modifiers.command);
                        let _ = app.run("view.zoom_preset", json!({"preset": i, "store": store}));
                    }
                }
            });
        });
    });
}

fn tools(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let cur = app.engine.session().edit.tool;
    group(ui, 268.0 + 38.0, 52.0, |ui, r| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 3.0);
            let row = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if icon_button(ui, vec2(36.0, 26.0), "zoom_toggle", app.engine.session().edit.zoom_toggle.is_some(), "Zoom Toggle (E)").clicked() {
                    crate::edit_window::zoom_toggle(app);
                }
                for (tool, icon) in [
                    (Tool::Zoom, "zoom"),
                    (Tool::Trim, "trim"),
                    (Tool::Selector, "selector"),
                    (Tool::Grabber, "grabber"),
                    (Tool::Scrubber, "scrubber"),
                    (Tool::Pencil, "pencil"),
                ] {
                    let smart_member = cur == Tool::Smart && matches!(tool, Tool::Trim | Tool::Selector | Tool::Grabber);
                    let tip = format!("{}{}", tool.label(), tool.fkey().map(|k| format!(" ({k})")).unwrap_or_default());
                    if icon_button(ui, vec2(36.0, 26.0), icon, cur == tool || smart_member, &tip).clicked() {
                        let _ = app.run("edit.tool", json!({"tool": tool.id()}));
                    }
                }
            });
            // Smart tool bracket above Trim/Selector/Grabber.
            let rr = row.response.rect;
            let bx0 = rr.min.x + 76.0;
            let bx1 = bx0 + 38.0 * 3.0 - 2.0;
            let bracket = Rect::from_min_max(pos2(bx0 - 2.0, r.min.y + 1.0), pos2(bx1 + 2.0, r.min.y + 5.0));
            let resp = ui.interact(bracket.expand2(vec2(0.0, 2.0)), ui.id().with("smart"), Sense::click());
            ui.painter().rect_filled(bracket, 1.0, if cur == Tool::Smart { t.accent } else { t.tool_bracket_off });
            if resp.on_hover_text("Smart Tool (F6+F7 / F7+F8)").clicked() {
                let _ = app.run("edit.tool", json!({"tool": "smart"}));
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let e = app.engine.session().edit.clone();
                let toggles: [(&str, &str, bool, &str); 5] = [
                    ("options.tab_to_transient", "tab_transient", e.tab_to_transient, "Tab to Transient"),
                    ("options.link_timeline_edit", "link", e.link_timeline_edit, "Link Timeline and Edit Selection"),
                    ("options.link_track_edit", "link", e.link_track_edit, "Link Track and Edit Selection"),
                    ("options.mirrored_midi", "note", e.mirrored_midi, "Mirrored MIDI Editing"),
                    ("options.automation_follows_edit", "wave", e.automation_follows_edit, "Automation Follows Edit"),
                ];
                for (id, icon, on, tip) in toggles {
                    if icon_button(ui, vec2(36.0, 18.0), icon, on, tip).clicked() {
                        let _ = app.run(id, json!({}));
                    }
                }
                let focus = e.keyboard_focus == "commands";
                if crate::widgets::text_toggle(ui, vec2(36.0, 18.0), "a-z", focus, t.accent, "Commands Keyboard Focus (single-key editing)").clicked()
                {
                    let _ = app.run("options.keyboard_focus", json!({}));
                }
            });
        });
    });
}

fn fmt_pos(app: &SoundApp, at: i64, f: TimeFormat) -> String {
    let s = app.engine.session();
    s.format_position(at, f)
}

fn counters(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let (main_fmt, sub_fmt, sel) = {
        let s = app.engine.session();
        (s.edit.main_counter, s.edit.sub_counter.unwrap_or(TimeFormat::Samples), s.edit.selection)
    };
    let pos = app.position();
    let (r, _) = ui.allocate_exact_size(vec2(318.0, 64.0), Sense::hover());
    ui.painter().rect(r, CornerRadius::same(4), t.counter_bg, Stroke::new(1.0, t.counter_border), StrokeKind::Inside);
    let main_r = Rect::from_min_max(r.min, pos2(r.min.x + 180.0, r.min.y + 44.0));
    // A count-off before the take: the bars still to count (negative, Pro Tools style) and the
    // beat, in the record colour, and `COUNT-OFF bar/bars` under it: recording has not started.
    let counting = app.countoff_status();
    let txt = match counting {
        Some((bar, beat, bars)) => format!("-{}|{beat}", bars.saturating_sub(bar) + 1),
        None => fmt_pos(app, pos, main_fmt),
    };
    // The sticky loop: `LOOP start – end` under the main counter, lit while the playhead is inside.
    let lp = app.engine.session().edit.loop_range.filter(|r| !r.is_empty());
    // Recording: `REC m:ss` (the take's length so far) in the record colour, and the booth when
    // this PC records with one.
    let rec_line = (counting.is_none() && app.engine.transport.recording).then(|| record_line(app, pos));
    let main_y = if lp.is_some() || counting.is_some() || rec_line.is_some() { main_r.center().y - 4.0 } else { main_r.center().y };
    let main_col = if counting.is_some() { t.rec } else { t.counter_text };
    ui.painter().text(pos2(main_r.max.x - 18.0, main_y), Align2::RIGHT_CENTER, txt, mono(26.0), main_col);
    if let Some((bar, _, bars)) = counting {
        let label = format!("COUNT-OFF {bar}/{bars} · not recording yet");
        ui.painter().text(pos2(main_r.max.x - 18.0, main_r.max.y - 5.0), Align2::RIGHT_CENTER, label, mono(8.5), t.rec);
    } else if let Some(label) = rec_line {
        ui.painter().text(pos2(main_r.max.x - 18.0, main_r.max.y - 5.0), Align2::RIGHT_CENTER, label, mono(8.5), t.rec);
    } else if let Some(lp) = lp {
        let inside = lp.contains(pos);
        let col = if inside { crate::edit_window::LOOP_COLOR } else { t.text_dim };
        let label = format!("LOOP {} – {}", fmt_pos(app, lp.start, main_fmt), fmt_pos(app, lp.end, main_fmt));
        ui.painter().text(pos2(main_r.max.x - 18.0, main_r.max.y - 5.0), Align2::RIGHT_CENTER, label, mono(8.5), col);
    }
    let resp = ui.interact(main_r, ui.id().with("main_counter"), Sense::click());
    icons::draw(ui.painter(), Rect::from_center_size(pos2(main_r.max.x - 9.0, main_r.center().y), vec2(10.0, 10.0)), "triangle_down", t.text_dim);
    egui::Popup::menu(&resp).show(|ui| {
        for f in TimeFormat::ALL {
            if ui.selectable_label(main_fmt == f, f.label()).clicked() {
                let _ = app.run("view.main_counter", json!({"format": f.id()}));
            }
        }
    });
    // Start / End / Length.
    let s = app.engine.session();
    let rows = [
        ("Start", fmt_pos(app, sel.start, main_fmt)),
        ("End", fmt_pos(app, sel.end, main_fmt)),
        ("Length", format_length(sel.start, sel.len(), main_fmt, s.sample_rate, &s.tempo, s.frame_rate)),
    ];
    for (i, (label, val)) in rows.iter().enumerate() {
        let y = r.min.y + 10.0 + i as f32 * 13.0;
        ui.painter().text(pos2(r.min.x + 214.0, y), Align2::RIGHT_CENTER, *label, regular(11.0), t.counter_label);
        ui.painter().text(pos2(r.max.x - 8.0, y), Align2::RIGHT_CENTER, val, mono(11.0), t.counter_text);
    }
    // Sub counter row.
    let y = r.max.y - 11.0;
    ui.painter().line_segment([pos2(r.min.x + 6.0, r.max.y - 21.0), pos2(r.max.x - 6.0, r.max.y - 21.0)], Stroke::new(1.0, t.counter_rule));
    ui.painter().text(pos2(r.min.x + 10.0, y), Align2::LEFT_CENTER, "Sub", regular(11.0), t.counter_label);
    ui.painter().text(pos2(r.min.x + 150.0, y), Align2::RIGHT_CENTER, fmt_pos(app, pos, sub_fmt), mono(11.0), t.counter_text);
    let st = if app.engine.is_dirty() { "modified" } else { "saved" };
    let dev = app.player.as_ref().map_or("no audio engine", |p| if p.silent { "silent clock" } else { "audio on" });
    ui.painter().text(pos2(r.max.x - 8.0, y), Align2::RIGHT_CENTER, format!("{st} · {dev}"), regular(10.0), t.text_dim);
}

fn grid_nudge(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let (grid, nudge, lines) = {
        let e = &app.engine.session().edit;
        (e.grid, e.nudge, e.grid_lines)
    };
    let (r, _) = ui.allocate_exact_size(vec2(170.0, 64.0), Sense::hover());
    ui.painter().rect(r, CornerRadius::same(4), t.counter_bg, Stroke::new(1.0, t.counter_border), StrokeKind::Inside);
    let gl = Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 8.0), vec2(48.0, 16.0));
    let resp = ui.interact(gl, ui.id().with("grid_lines"), Sense::click());
    key(ui.painter(), gl, &resp, if lines { t.mode_on } else { t.counter_off });
    ui.painter().text(gl.center(), Align2::CENTER_CENTER, "Grid", bold(11.0), if lines { t.mode_on_text } else { t.mode_off_text });
    if resp.on_hover_text("Show grid lines").clicked() {
        let _ = app.run("view.grid_lines", json!({}));
    }
    let gr = Rect::from_min_max(pos2(r.min.x + 60.0, r.min.y + 6.0), pos2(r.max.x - 4.0, r.min.y + 26.0));
    ui.painter().text(pos2(gr.max.x - 14.0, gr.center().y), Align2::RIGHT_CENTER, grid.label(), mono(11.0), t.counter_text);
    let gresp = ui.interact(gr, ui.id().with("grid_value"), Sense::click());
    value_menu(app, &gresp, "view.grid");
    ui.painter().text(pos2(r.min.x + 10.0, r.min.y + 44.0), Align2::LEFT_CENTER, "Nudge", regular(11.0), t.counter_label);
    let nr = Rect::from_min_max(pos2(r.min.x + 60.0, r.min.y + 34.0), pos2(r.max.x - 4.0, r.min.y + 54.0));
    ui.painter().text(pos2(nr.max.x - 14.0, nr.center().y), Align2::RIGHT_CENTER, nudge.label(), mono(11.0), t.counter_text);
    let nresp = ui.interact(nr, ui.id().with("nudge_value"), Sense::click());
    value_menu(app, &nresp, "view.nudge");
    for rr in [gr, nr] {
        icons::draw(ui.painter(), Rect::from_center_size(pos2(rr.max.x - 6.0, rr.center().y), vec2(10.0, 10.0)), "triangle_down", t.text_dim);
    }
}

fn value_menu(app: &mut SoundApp, resp: &egui::Response, cmd: &str) {
    egui::Popup::menu(resp).show(|ui| {
        for v in ["1 bar", "1/2", "1/4", "1/8", "1/16", "1/32", "1/64", "1/8t", "1/16t", "1/4."] {
            if ui.button(v).clicked() {
                let _ = app.run(cmd, json!({"value": v}));
            }
        }
        ui.separator();
        for (label, v) in [
            ("1 msec", json!({"seconds": 0.001})),
            ("10 msec", json!({"seconds": 0.01})),
            ("100 msec", json!({"seconds": 0.1})),
            ("1 second", json!({"seconds": 1.0})),
            ("1 frame", json!({"frames": 1})),
            ("100 samples", json!({"samples": 100})),
        ] {
            if ui.button(label).clicked() {
                let _ = app.run(cmd, json!({"value": v}));
            }
        }
    });
}

/// The counter's line while recording: `● REC 1:23`, `● REC with Booth · 1:23` on a remote
/// recorder following one.
pub fn record_line(app: &SoundApp, pos: soundcraft_time::Samples) -> String {
    let start = app.take_plan.as_ref().map_or(app.record_start, |p| p.start);
    let secs = pos.saturating_sub(start).max(0) as f64 / app.engine.session().sample_rate.as_f64().max(1.0);
    let t = secs as u64;
    let (h, m, s) = (t / 3600, (t / 60) % 60, t % 60);
    let clock = if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") };
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(soundcraft_net::glue::FollowBadge::Recording(booth)) = crate::net_ui::badge(app) {
        return format!("● REC with {booth} · {clock}");
    }
    format!("● REC {clock}")
}

/// While following a booth (Setup › Network): what this PC is doing for it, in the record colour
/// while it records a take for the booth, amber while a take waits to be sent.
#[cfg(not(target_arch = "wasm32"))]
fn follow_badge(app: &mut SoundApp, ui: &mut Ui) {
    use soundcraft_net::glue::FollowBadge;
    let Some(badge) = crate::net_ui::badge(app) else { return };
    let t = Tokens::current();
    let (fill, fg, border) = match &badge {
        FollowBadge::Recording(_) => (Color32::from_rgb(110, 30, 30), Color32::from_rgb(255, 170, 160), t.rec),
        FollowBadge::Waiting(_) | FollowBadge::Away(_) => (t.toolbar_group, Color32::from_rgb(240, 198, 116), Color32::from_rgb(160, 120, 50)),
        FollowBadge::Sent(_) => (t.toolbar_group, Color32::from_rgb(120, 220, 140), Color32::from_rgb(60, 130, 70)),
        FollowBadge::Sending(..) | FollowBadge::Preparing(_) | FollowBadge::Following(_) => (t.toolbar_group, t.counter_text, t.group_border),
    };
    let text = badge.text();
    let w = (ui.fonts_mut(|f| f.layout_no_wrap(text.clone(), bold(11.0), fg).size().x) + 34.0).clamp(120.0, 260.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w, 64.0), Sense::click());
    let p = ui.painter();
    p.rect(r, CornerRadius::same(4), fill, Stroke::new(1.0, border), StrokeKind::Inside);
    p.text(pos2(r.min.x + 10.0, r.min.y + 14.0), Align2::LEFT_CENTER, "REMOTE RECORDER", regular(9.0), t.text_dim);
    let dot = pos2(r.min.x + 14.0, r.center().y + 4.0);
    if badge.recording() {
        // Blinks once a second, as a recording light does.
        let on = (ui.input(|i| i.time) % 1.0) < 0.6;
        p.circle_filled(dot, 5.0, if on { t.rec } else { t.rec.gamma_multiply(0.35) });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    } else {
        p.circle_filled(dot, 4.0, fg.gamma_multiply(0.8));
    }
    p.text(pos2(dot.x + 12.0, dot.y), Align2::LEFT_CENTER, &text, bold(11.0), fg);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &text));
    if resp.on_hover_text("Following a booth (Setup › Network). Click for the Network window.").clicked() {
        app.ui.show_network = true;
    }
}

fn transport(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let playing = app.is_playing();
    let recording = app.engine.transport.recording;
    let looped = app.engine.session().edit.loop_playback;
    let llm = app.engine.session().edit.flag("options.low_latency_monitoring");
    group(ui, 217.0, 64.0, |ui, g| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(3.0, 4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let tip = match app.engine.session().edit.loop_range.filter(|r| !r.is_empty()) {
                    Some(lp) => {
                        let f = app.engine.session().edit.main_counter;
                        format!("Loop Playback (Cmd+Shift+L): loops {} – {}", fmt_pos(app, lp.start, f), fmt_pos(app, lp.end, f))
                    }
                    None => "Loop Playback (Cmd+Shift+L): select a range to set the loop".to_string(),
                };
                if icon_button(ui, vec2(38.0, 28.0), "loop", looped, &tip).clicked() {
                    let _ = app.run("options.loop_playback", json!({}));
                }
                // Low-latency monitoring: a monitoring mode (no undo step), also in Options.
                const LLM_TIP: &str = "Low-latency monitoring: bypass latency-adding plugins on live tracks";
                let resp = text_toggle(ui, vec2(38.0, 28.0), "LLM", llm, t.input, LLM_TIP);
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, llm, LLM_TIP));
                if resp.clicked() {
                    let _ = app.run("options.low_latency_monitoring", json!({}));
                }
                let (r, resp) = ui.allocate_exact_size(vec2(38.0, 28.0), Sense::click());
                key(ui.painter(), r, &resp, key_fill(&resp));
                icons::draw(ui.painter(), r.shrink(5.0), "stop", if playing { t.text } else { t.accent });
                if resp.on_hover_text("Stop (Space)").clicked() {
                    let _ = app.run("transport.stop", json!({}));
                }
                let (r, resp) = ui.allocate_exact_size(vec2(38.0, 28.0), Sense::click());
                key(ui.painter(), r, &resp, if playing { Color32::from_rgb(40, 90, 50) } else { key_fill(&resp) });
                icons::draw(ui.painter(), r.shrink(5.0), "play", if playing { Color32::from_rgb(120, 255, 140) } else { t.counter_text });
                if resp.on_hover_text("Play (Space)").clicked() {
                    let _ = app.run("transport.play", json!({}));
                }
                let (r, resp) = ui.allocate_exact_size(vec2(38.0, 28.0), Sense::click());
                key(ui.painter(), r, &resp, if recording { Color32::from_rgb(110, 30, 30) } else { key_fill(&resp) });
                icons::draw(ui.painter(), r.shrink(5.0), "record", t.rec);
                // The offset text is built only while the tooltip shows.
                let resp = resp.on_hover_ui(|ui| {
                    ui.label(format!("Record (Cmd+Space) · {}", app.record_offset_summary()));
                });
                if let Some(why) = app.record_hint() {
                    // Below the whole group, so the hint covers none of its buttons.
                    let below = Rect::from_min_max(r.min, pos2(r.max.x, g.max.y));
                    egui::Tooltip::always_open(ui.ctx().clone(), resp.layer_id, resp.id, below).show(|ui| ui.label(why));
                }
                if resp.clicked() {
                    let _ = app.run("transport.record", json!({}));
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for (icon, cmd, tip) in [
                    ("rtz", "transport.rtz", "Return to Zero (Home)"),
                    ("rewind", "transport.rewind", "Rewind"),
                    ("ffwd", "transport.fast_forward", "Fast Forward"),
                    ("end", "transport.go_to_end", "Go to End (End)"),
                ] {
                    // Four keys under the five above.
                    if icon_button(ui, vec2(48.0, 22.0), icon, false, tip).clicked() {
                        let _ = app.run(cmd, json!({}));
                    }
                }
            });
        });
    });
}

/// The Scout button: idle it reads "Scout" (with a loop of 30 s or less it offers "Capture loop");
/// capturing it reads "SCOUT — capturing m:ss" in red and a click stops it.
fn scout_button(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let capturing = crate::scout::capturing(app);
    let sr = app.engine.session().sample_rate.as_f64();
    let label = if capturing { format!("● SCOUT {}", crate::scout::clock(app.scout.frames().unwrap_or(0), sr)) } else { "Scout".to_string() };
    let selected = app.engine.session().edit.selected_tracks.len();
    let lp = (!capturing).then(|| crate::scout::capturable_loop(app)).flatten();
    let last_capture = app.engine.session().scout.captures.last().map(|c| c.id);
    group(ui, 128.0, 64.0, |ui, _| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(3.0, 5.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(3.0, 0.0);
                let (r, resp) = ui.allocate_exact_size(vec2(118.0, 28.0), Sense::click());
                let fill = if capturing { Color32::from_rgb(110, 30, 30) } else { key_fill(&resp) };
                key(ui.painter(), r, &resp, fill);
                if capturing {
                    ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, t.rec), StrokeKind::Inside);
                }
                let fg = if capturing { Color32::from_rgb(255, 150, 140) } else { t.counter_text };
                ui.painter().text(r.center(), Align2::CENTER_CENTER, &label, bold(11.0), fg);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, if capturing { "Scout stop" } else { "Scout" }));
                let resp = resp.on_hover_text(if capturing {
                    "Stop the Scout capture (it also ends at 30 seconds, at the loop end, or when the transport stops)"
                } else {
                    "Capture the selected tracks, buses or master before and after their modules (Window ▸ Scout)"
                });
                if capturing {
                    if resp.clicked() {
                        let _ = app.run("scout.capture", json!({"action": "stop"}));
                    }
                } else if let Some(lp) = lp {
                    let len = crate::scout::clock(u64::try_from(lp.len()).unwrap_or(0), sr);
                    egui::Popup::menu(&resp).show(|ui| {
                        if ui.button("Capture").clicked() {
                            let _ = app.run("scout.capture", json!({"action": "start"}));
                        }
                        if ui.button(format!("Capture loop ({len})")).clicked() {
                            let _ = app.run("scout.capture", json!({"action": "start", "loop": true}));
                        }
                    });
                } else if resp.clicked() {
                    let _ = app.run("scout.capture", json!({"action": "start"}));
                }
            });
            let hint = if capturing {
                "capturing · click to stop".to_string()
            } else if selected == 0 {
                "select strips".to_string()
            } else {
                format!("{selected} selected")
            };
            ui.allocate_ui_with_layout(vec2(118.0, 16.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing = vec2(3.0, 0.0);
                if small_key(ui, "Open", "Open Scout window", true, "Open the Scout window") {
                    let _ = app.run("window.scout", json!({"value": true}));
                }
                let copy_ok = last_capture.filter(|_| !capturing);
                let tip =
                    if last_capture.is_none() { "No Scout capture yet" } else { "Copy the last Scout capture (prompt + package) to the clipboard" };
                if small_key(ui, "Copy", "Copy last Scout capture", copy_ok.is_some(), tip)
                    && let Some(id) = copy_ok
                    && let Err(e) = app.run("ui.scout.copy", json!({"captures": [id]}))
                {
                    app.ui.status = e;
                }
                ui.add(egui::Label::new(egui::RichText::new(hint).font(regular(10.0)).color(t.text_dim)).truncate());
            });
        });
    });
}

/// A small text key (28x16); returns true when clicked. A disabled key is dimmed and never fires,
/// but still shows its hover text.
fn small_key(ui: &mut Ui, text: &str, name: &str, enabled: bool, tip: &str) -> bool {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(28.0, 16.0), Sense::click());
    key(ui.painter(), r, &resp, if enabled { key_fill(&resp) } else { t.button });
    let fg = if enabled { t.counter_text } else { t.text_dim.gamma_multiply(0.6) };
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, bold(9.0), fg);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
    let resp = resp.on_hover_text(tip);
    enabled && resp.clicked()
}

fn tempo_meter(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let (bpm, meter, countoff, click, bars) = {
        let s = app.engine.session();
        let tick = s.tempo.samples_to_ticks(app.position(), s.sample_rate);
        let m = s.tempo.meter_at_tick(tick);
        (s.tempo.tempo_at_tick(tick), format!("{}/{}", m.numerator, m.denominator), s.edit.countoff, s.edit.click, s.edit.countoff_bars)
    };
    let (r, _) = ui.allocate_exact_size(vec2(150.0, 64.0), Sense::hover());
    ui.painter().rect(r, CornerRadius::same(4), t.counter_bg, Stroke::new(1.0, t.counter_border), StrokeKind::Inside);
    let co = Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 6.0), vec2(64.0, 15.0));
    let resp = ui.interact(co, ui.id().with("countoff"), Sense::click());
    key(ui.painter(), co, &resp, if countoff { t.mode_on } else { t.counter_off });
    ui.painter().text(co.center(), Align2::CENTER_CENTER, "Count Off", bold(10.0), if countoff { t.mode_on_text } else { t.mode_off_text });
    if resp.clicked() {
        let _ = app.run("options.countoff", json!({}));
    }
    ui.painter().text(pos2(r.max.x - 8.0, co.center().y), Align2::RIGHT_CENTER, format!("{bars} bars"), mono(11.0), t.counter_text);
    ui.painter().text(pos2(r.min.x + 70.0, r.min.y + 32.0), Align2::RIGHT_CENTER, "Meter", regular(11.0), t.counter_label);
    ui.painter().text(pos2(r.max.x - 8.0, r.min.y + 32.0), Align2::RIGHT_CENTER, meter, mono(11.0), t.counter_text);
    ui.painter().text(pos2(r.min.x + 70.0, r.min.y + 48.0), Align2::RIGHT_CENTER, "Tempo", regular(11.0), t.counter_label);
    icons::draw(ui.painter(), Rect::from_center_size(pos2(r.min.x + 82.0, r.min.y + 48.0), vec2(13.0, 13.0)), "note", t.counter_text);
    tempo_value(app, ui, r.max.x - 8.0, r.min.y + 48.0, bpm);
    let clk = Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 40.0), vec2(18.0, 18.0));
    icons::draw(ui.painter(), clk, "click", if click { t.counter_text } else { t.text_dim });
    if ui.interact(clk, ui.id().with("click"), Sense::click()).on_hover_text("Click").clicked() {
        let _ = app.run("options.click", json!({}));
    }
}

/// The tempo in effect at the playhead, right-aligned at `right`: drag it (Shift for fine steps),
/// or click and type.
fn tempo_value(app: &mut SoundApp, ui: &mut Ui, right: f32, y: f32, bpm: f64) {
    let t = Tokens::current();
    let w = ui.painter().layout_no_wrap(format!("{bpm:.4}"), mono(11.0), t.counter_text).size().x;
    let rect = Rect::from_min_max(pos2(right - w - 4.0, y - 8.0), pos2(right + 4.0, y + 8.0));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    c.style_mut().override_font_id = Some(mono(11.0));
    let v = c.visuals_mut();
    v.override_text_color = Some(t.counter_text);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.weak_bg_fill = Color32::TRANSPARENT;
        w.bg_stroke = Stroke::NONE;
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.counter_border);
    let mut value = bpm;
    let resp = c.add_sized(rect.size(), egui::DragValue::new(&mut value).speed(0.1).range(5.0..=1000.0).fixed_decimals(4));
    if resp.changed() {
        let s = app.engine.session();
        let tick = s.tempo.samples_to_ticks(app.position(), s.sample_rate);
        let event = s.tempo.tempos().iter().rev().find(|e| e.tick <= tick).map_or(0, |e| e.tick);
        let at = s.tempo.tick_to_samples(event, s.sample_rate);
        let _ = app.engine.execute_merged("event.tempo", &json!({"bpm": value, "at": at}), "toolbar_tempo");
    }
    if resp.drag_stopped() || resp.lost_focus() {
        app.engine.end_merge();
    }
    resp.on_hover_text("Drag to change the tempo (Shift: fine steps), or click to type it");
}

fn output_meter(app: &mut SoundApp, ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(26.0, 64.0), Sense::hover());
    let m = app.main_meter;
    let w = (r.width() - 6.0) / 2.0;
    for c in 0..2 {
        let rr = Rect::from_min_size(pos2(r.min.x + 2.0 + c as f32 * (w + 2.0), r.min.y), vec2(w, r.height()));
        crate::widgets::meter(ui, rr, m.level[c], m.hold[c], m.clip);
    }
}

/// Height of the Mix window's slim top bar.
pub const MIX_HEIGHT: f32 = 40.0;

/// The Mix window's slim top bar, as Studio One keeps one over its console: the transport keys,
/// the main counter (click for its format) and the edit selection's start, end and length, with
/// the loop under the counter when there is one. The same commands as the Edit toolbar.
pub fn mix_bar(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, t.toolbar_bg);
    ui.painter().hline(full.x_range(), full.max.y - 0.5, Stroke::new(1.0, t.border));
    let playing = app.is_playing();
    let recording = app.engine.transport.recording;
    let looped = app.engine.session().edit.loop_playback;
    let mut row =
        ui.new_child(egui::UiBuilder::new().max_rect(full.shrink2(vec2(8.0, 5.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    row.spacing_mut().item_spacing.x = 3.0;
    let ui = &mut row;
    let key_size = vec2(30.0, 26.0);
    for (icon, cmd, tip) in [("rtz", "transport.rtz", "Return to Zero (Home)"), ("rewind", "transport.rewind", "Rewind")] {
        if icon_button(ui, key_size, icon, false, tip).clicked() {
            let _ = app.run(cmd, json!({}));
        }
    }
    let (r, resp) = ui.allocate_exact_size(key_size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Stop"));
    key(ui.painter(), r, &resp, key_fill(&resp));
    icons::draw(ui.painter(), r.shrink(5.0), "stop", if playing { t.text } else { t.accent });
    if resp.on_hover_text("Stop (Space)").clicked() {
        let _ = app.run("transport.stop", json!({}));
    }
    let (r, resp) = ui.allocate_exact_size(key_size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, playing, "Play"));
    key(ui.painter(), r, &resp, if playing { Color32::from_rgb(40, 90, 50) } else { key_fill(&resp) });
    icons::draw(ui.painter(), r.shrink(5.0), "play", if playing { Color32::from_rgb(120, 255, 140) } else { t.counter_text });
    if resp.on_hover_text("Play (Space)").clicked() {
        let _ = app.run("transport.play", json!({}));
    }
    let (r, resp) = ui.allocate_exact_size(key_size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, recording, "Record"));
    key(ui.painter(), r, &resp, if recording { Color32::from_rgb(110, 30, 30) } else { key_fill(&resp) });
    icons::draw(ui.painter(), r.shrink(5.0), "record", t.rec);
    let resp = resp.on_hover_ui(|ui| {
        ui.label(format!("Record (Cmd+Space) · {}", app.record_offset_summary()));
        if let Some(why) = app.record_hint() {
            ui.label(why);
        }
    });
    if resp.clicked() {
        let _ = app.run("transport.record", json!({}));
    }
    for (icon, cmd, tip) in [("ffwd", "transport.fast_forward", "Fast Forward"), ("end", "transport.go_to_end", "Go to End (End)")] {
        if icon_button(ui, key_size, icon, false, tip).clicked() {
            let _ = app.run(cmd, json!({}));
        }
    }
    if icon_button(ui, key_size, "loop", looped, "Loop Playback (Cmd+Shift+L)").clicked() {
        let _ = app.run("options.loop_playback", json!({}));
    }
    ui.add_space(8.0);
    // The main counter: a click picks its format.
    let (main_fmt, sel) = {
        let s = app.engine.session();
        (s.edit.main_counter, s.edit.selection)
    };
    let pos = app.position();
    let (cr, cresp) = ui.allocate_exact_size(vec2(190.0, 30.0), Sense::click());
    // A count-off before the take shows instead (see `counters`).
    let counting = app.countoff_status();
    let text = match counting {
        Some((bar, beat, bars)) => format!("-{}|{beat}", bars.saturating_sub(bar) + 1),
        None => fmt_pos(app, pos, main_fmt),
    };
    cresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Main counter {text}")));
    ui.painter().rect(cr, CornerRadius::same(3), t.counter_bg, Stroke::new(1.0, t.counter_border), StrokeKind::Inside);
    let lp = app.engine.session().edit.loop_range.filter(|r| !r.is_empty());
    let cy = if lp.is_some() || counting.is_some() { cr.center().y - 5.0 } else { cr.center().y };
    ui.painter().text(pos2(cr.max.x - 16.0, cy), Align2::RIGHT_CENTER, text, mono(16.0), if counting.is_some() { t.rec } else { t.counter_text });
    icons::draw(ui.painter(), Rect::from_center_size(pos2(cr.max.x - 8.0, cr.center().y), vec2(8.0, 8.0)), "triangle_down", t.text_dim);
    if let Some((bar, _, bars)) = counting {
        ui.painter().text(pos2(cr.max.x - 16.0, cr.max.y - 6.0), Align2::RIGHT_CENTER, format!("COUNT-OFF {bar}/{bars}"), mono(8.0), t.rec);
    } else if let Some(lp) = lp {
        let col = if lp.contains(pos) { crate::edit_window::LOOP_COLOR } else { t.text_dim };
        let label = format!("LOOP {} – {}", fmt_pos(app, lp.start, main_fmt), fmt_pos(app, lp.end, main_fmt));
        ui.painter().text(pos2(cr.max.x - 16.0, cr.max.y - 6.0), Align2::RIGHT_CENTER, label, mono(8.0), col);
    }
    egui::Popup::menu(&cresp.on_hover_text("Main counter: click to change its format")).show(|ui| {
        for f in TimeFormat::ALL {
            if ui.selectable_label(main_fmt == f, f.label()).clicked() {
                let _ = app.run("view.main_counter", json!({"format": f.id()}));
            }
        }
    });
    ui.add_space(6.0);
    // The edit selection: Start, End, Length.
    let s = app.engine.session();
    let fields = [
        ("Start", fmt_pos(app, sel.start, main_fmt)),
        ("End", fmt_pos(app, sel.end, main_fmt)),
        ("Length", format_length(sel.start, sel.len(), main_fmt, s.sample_rate, &s.tempo, s.frame_rate)),
    ];
    for (label, value) in fields {
        let lw = ui.painter().layout_no_wrap(label.to_string(), regular(10.0), t.counter_label).size().x;
        let vw = ui.painter().layout_no_wrap(value.clone(), mono(11.0), t.counter_text).size().x;
        let (r, resp) = ui.allocate_exact_size(vec2(lw + vw + 18.0, 26.0), Sense::hover());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, format!("Selection {label} {value}")));
        ui.painter().rect(r, CornerRadius::same(3), t.counter_bg, Stroke::new(1.0, t.counter_border), StrokeKind::Inside);
        ui.painter().text(pos2(r.min.x + 6.0, r.center().y), Align2::LEFT_CENTER, label, regular(10.0), t.counter_label);
        ui.painter().text(pos2(r.max.x - 6.0, r.center().y), Align2::RIGHT_CENTER, value, mono(11.0), t.counter_text);
    }
}

//! DelayEngine's editor in the classic drawer's width, about a mixer strip tall: the full
//! editor's sections, names and order, folded to fit. Header (title and preset strip; Undo, A, B,
//! Bypass), the style (a menu of the six, each with its blurb), the Heads (Tape and Drum), the
//! four main dials two to a row, the foot (Sync, Tap, Reverse, Freeze and the tempo readout), and
//! a More section holding the drawer's six tabs and their panels. The gestures are the full
//! editor's own (`super::dial_ctl`, `super::tap_ui`, the family's attachments).

use super::{
    Editor, FAM, PLAIN, TAB_DYNAMICS, TAB_MOTION, TAB_RHYTHM, TAB_SETUP, TAB_STEREO, TAB_TONE, TABS, applies, dial_ctl, effective_bpm, help_text,
    host_bpm, model, tap_ui, tempo_readout,
};
use crate::SoundApp;
use crate::editor_kit::controls::{PillStyle, SelectionStyle, led, pill_height, segmented};
use crate::editor_kit::family::{self, pill_param, segmented_param};
use crate::editor_kit::look::{CAPTION_SIZE, PANEL_CORNER, font, paint_background, paint_title};
use crate::editor_kit::slim::{self as sk, MenuEntry, PAD_X, PAD_Y};
use crate::editor_kit::{Host, param, readouts};
use egui::{Align2, Rect, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::delay::delay_config as dc;

const GAP: f32 = 8.0;
const ROW_H: f32 = 22.0;
const MODEL_H: f32 = 28.0;
const BIG_KNOB: f32 = 64.0;
const SMALL_KNOB: f32 = 36.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host) {
    let pal = &super::PALETTE;
    let ed = host.rect;
    st.tab = st.tab.min(TABS.len() - 1);
    if let Some(tab) = family::take_drawer_tab(app, host, &mut st.fam) {
        if let Some(i) = TABS.iter().position(|t| t.eq_ignore_ascii_case(&tab)) {
            st.tab = i;
            st.slim_more = true;
        }
        match tab.as_str() {
            "More" => st.slim_more = true,
            "Styles" => st.styles_open = true,
            _ => {}
        }
    }
    if family::browser(app, ui, host, pal, &mut st.fam, &FAM, "") {
        ui.ctx().request_repaint();
        return;
    }
    let p = ui.painter().clone();
    paint_background(&p, ed);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;
    let ins = host.ins;
    let a = applies(ins);
    let style = model(ins, "style");
    let ro = readouts(host);
    let bpm = effective_bpm(host, ro);

    // ---- 1. header: the title and the preset strip; then Undo, A, B, Bypass
    let head = row(y, ROW_H + 2.0);
    let title_w = paint_title(&p, head, "Delay", "Engine", pal);
    let (prev, name, next) = sk::preset_strip(head, title_w);
    y += ROW_H + 2.0 + 6.0;
    let widths = [family::small_pill_w(ui, "Undo"), 28.0, 28.0, family::small_pill_w(ui, "Bypass")];
    let cells = sk::pill_row(x0, w, y, &widths, &[1]);
    let rects = family::HeaderRects { prev, name, next, undo: cells[0], a: cells[1], b: cells[2], bypass: cells[3] };
    family::header(app, ui, host, pal, &mut st.fam, &FAM, &rects);
    y += pill_height(PillStyle::Small) + GAP + 2.0;

    // ---- 2. the style: one menu of the six (the full editor's row and blurb)
    let items: Vec<MenuEntry> =
        dc::STYLES.iter().enumerate().map(|(i, s)| MenuEntry { name: s, tag: "", blurb: dc::style_blurb(i as i32), credit: "" }).collect();
    let sel = usize::try_from(style).unwrap_or(0);
    if let Some(i) = sk::model_menu(ui, row(y, MODEL_H), "Style", "Style", pal, &items, sel, std::mem::take(&mut st.styles_open)) {
        family::set(app, host, &mut st.fam, &FAM, "style", i as f32);
    }
    y += MODEL_H + GAP;

    // ---- 3. the Heads (Tape and Drum)
    if a.heads {
        segmented_param(app, ui, host, &mut st.fam, &FAM, row(y, 2.0 * ROW_H), pal, "heads", None, 2, SelectionStyle::Filled, false);
        y += 2.0 * ROW_H + GAP;
    }

    // ---- 4. the main dials, two to a row: Time (or Note), Feedback, Mix, Character
    let cell = vec2((w / 3.0).floor(), BIG_KNOB + 32.0);
    let cells = sk::grid(x0, w, y, 4, 2, cell, 2.0);
    let first = if a.division { ("division", "Note") } else { ("time", "Time") };
    let mains = [first, ("feedback", "Feedback"), ("mix", "Mix"), ("character", dc::character_label(style))];
    for ((id, label), r) in mains.into_iter().zip(&cells) {
        if id == "time" && !a.time {
            continue;
        }
        dial_ctl(app, ui, host, &mut st.fam, *r, id, label, false, bpm);
    }
    y = sk::bottom_of(&cells, y) + GAP;

    // ---- 5. the foot: Sync, Tap; Reverse, Freeze; the tempo readout under them
    let widths = [54.0, 46.0, 70.0, 62.0];
    let cells = sk::pill_row(x0, w, y, &widths, &[0, 2]);
    pill_param(app, ui, host, &mut st.fam, &FAM, cells[0], pal, "sync", "Sync", PLAIN);
    tap_ui(st, app, ui, host, cells[1]);
    pill_param(app, ui, host, &mut st.fam, &FAM, cells[2], pal, "reverse", "Reverse", PLAIN);
    pill_param(app, ui, host, &mut st.fam, &FAM, cells[3], pal, "freeze", "Freeze", PLAIN);
    y += pill_height(PillStyle::Small) + 4.0;
    let text = tempo_readout(param(host, "tempoSrc") < 0.5, host_bpm(host), bpm);
    p.text(pos2(x0 + w / 2.0, y + 7.0), Align2::CENTER_CENTER, text, font(CAPTION_SIZE, false), pal.text_faint);
    y += 14.0 + GAP;

    // ---- 6. More: the drawer's tabs and their panels
    if sk::more_header(ui, row(y, 20.0), "delay", pal, st.slim_more, &TABS.join(" \u{00B7} ")) {
        st.slim_more = !st.slim_more;
    }
    y += 20.0;
    if st.slim_more {
        y += 6.0;
        if let Some(i) = segmented(ui, row(y, 2.0 * ROW_H), "tabs", pal, &TABS, 2, Some(st.tab), Default::default()) {
            st.tab = i;
        }
        y += 2.0 * ROW_H + 8.0;
        let top = y;
        let slot = sk::panel_slot(&p);
        y = tab_panel(st, app, ui, host, row(y + 10.0, 0.0), bpm, ro) + 6.0;
        let help = help_text(st.tab);
        if !help.is_empty() {
            y += sk::wrapped(ui, Rect::from_min_size(pos2(x0 + 10.0, y), vec2(w - 20.0, 200.0)), help, CAPTION_SIZE, pal.text_faint, 8);
        }
        y += 10.0;
        sk::panel_under(&p, slot, Rect::from_min_max(pos2(x0, top), pos2(x0 + w, y)), PANEL_CORNER);
        y += 2.0;
    }
    y += PAD_Y;

    if ro.is_some() || st.fam.presets.message.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
    sk::settle(ui, ed.top(), y, &mut st.slim_h);
}

/// The open tab's controls from `r`'s top; returns their bottom.
fn tab_panel(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, bpm: f32, ro: Option<[f32; 8]>) -> f32 {
    let pal = &super::PALETTE;
    let (x0, w, y) = (r.left() + 6.0, r.width() - 12.0, r.top());
    let a = applies(host.ins);
    let cell = vec2((w / 3.0).floor(), SMALL_KNOB + 32.0);
    let dials = |st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, y: f32, list: &[(&str, &str)]| -> Vec<Rect> {
        let cells = sk::grid(x0, w, y, list.len(), sk::per_row(list.len()), cell, 2.0);
        for ((id, label), c) in list.iter().zip(&cells) {
            dial_ctl(app, ui, host, &mut st.fam, *c, id, label, true, bpm);
        }
        cells
    };
    let full = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    match st.tab {
        TAB_TONE => {
            let c = dials(st, app, ui, y, &[("lowCut", "Low Cut"), ("highCut", "High Cut"), ("filtMod", "Filter Sweep"), ("noise", "Noise")]);
            sk::bottom_of(&c, y)
        }
        TAB_DYNAMICS => {
            let c = dials(st, app, ui, y, &[("duck", "Duck"), ("threshold", "Threshold"), ("focus", "Attack Focus")]);
            // the gate lamp beside Threshold's knob, lit by the engine's gate lamp
            if let Some(t) = c.get(1) {
                let lamp = Rect::from_min_size(pos2(t.center().x.floor() + 22.0, t.top() + 2.0), vec2(10.0, 10.0));
                led(ui.painter(), lamp, pal, ro.is_some_and(|r| r[3] > 0.5), pal.accent);
            }
            sk::bottom_of(&c, y)
        }
        TAB_MOTION => {
            let c = dials(st, app, ui, y, &[("modRate", "Mod Rate"), ("modDepth", "Mod Depth"), ("diffusion", "Diffusion")]);
            let mut b = sk::bottom_of(&c, y);
            if let Some(rate) = c.first() {
                let sync_r = Rect::from_center_size(pos2(rate.center().x, b + 12.0), vec2(52.0, 20.0));
                pill_param(app, ui, host, &mut st.fam, &FAM, sync_r, pal, "modSync", "Sync", PLAIN);
            }
            b += 28.0;
            segmented_param(app, ui, host, &mut st.fam, &FAM, full(b, 24.0), pal, "pitch", None, 1, SelectionStyle::Filled, false);
            b + 24.0
        }
        TAB_RHYTHM => {
            segmented_param(app, ui, host, &mut st.fam, &FAM, full(y, 2.0 * ROW_H), pal, "pattern", None, 2, SelectionStyle::Filled, false);
            y + 2.0 * ROW_H
        }
        TAB_STEREO => {
            segmented_param(app, ui, host, &mut st.fam, &FAM, full(y, 24.0), pal, "routing", None, 1, SelectionStyle::Filled, false);
            let mut shown: Vec<(&str, &str)> = vec![("width", "Width")];
            if a.offset {
                shown.push(("offset", "R Offset"));
            }
            if a.division_r {
                shown.push(("divisionR", "R Note"));
            }
            if a.swing {
                shown.push(("swing", "Swing"));
            }
            let top = y + 24.0 + 8.0;
            let c = dials(st, app, ui, top, &shown);
            sk::bottom_of(&c, top)
        }
        TAB_SETUP => {
            let c = dials(st, app, ui, y, &[("trim", "Input"), ("level", "Level"), ("tempo", "Manual BPM")]);
            let b = sk::bottom_of(&c, y) + 4.0;
            let ph = 21.0;
            let cells = sk::pill_row(x0 + 10.0, w - 20.0, b, &[124.0, 70.0], &[]);
            pill_param(app, ui, host, &mut st.fam, &FAM, cells[0].with_max_y(cells[0].top() + ph), pal, "tempoSrc", "Use Manual BPM", PLAIN);
            pill_param(app, ui, host, &mut st.fam, &FAM, cells[1].with_max_y(cells[1].top() + ph), pal, "trails", "Trails", PLAIN);
            b + ph
        }
        _ => y,
    }
}

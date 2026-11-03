//! Reverb's editor in the classic drawer's width ([`crate::module_drawer::DRAWER_W`]), about a
//! mixer strip tall: the full editor's sections, names and order, folded to fit. Header (title and
//! preset strip; Undo, A, B, Bypass), the machine (a menu of the six, each with its
//! description), the era row (every machine but the Spring) and its blurb, the four main dials
//! two to a row, the five drawer tabs with their panel (the full editor's own secondary section,
//! so it is not folded again into a More), and the foot (the tempo readout, Freeze). The tab's
//! help line is the tabs' tooltip. The gestures are the full editor's own code (`super::dial_ui`,
//! the family attachments).

use super::{Editor, FAM, PALETTE, PLAIN, TAB_DYNAMICS, TAB_MOTION, TAB_SETUP, TAB_TIME, TAB_TONE, TABS};
use super::{applies, effective_bpm, help_text, host_bpm, model, prelude, tone_dials};
use crate::SoundApp;
use crate::editor_kit::controls::{PillStyle, SelectionStyle, pill_height, segmented, segmented_id, tip};
use crate::editor_kit::family::{pill_param, segmented_param};
use crate::editor_kit::look::{CAPTION_SIZE, PANEL_CORNER, font, paint_background, paint_panel};
use crate::editor_kit::slim::{
    self, AbPlace, BIG_KNOB, GAP, MODEL_H, MenuEntry, MenuLook, PAD_X, PAD_Y, ROW_H, SMALL_KNOB, caption, family_header, grid, model_menu_ex,
};
use crate::editor_kit::{Host, param, readouts};
use egui::{Align2, Rect, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::reverb::reverb_config as rc;

/// The main dials, in the full editor's order.
const MAIN: [&str; 4] = ["decay", "size", "mix", "character"];

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let pal = &PALETTE;
    let ed = host.rect;
    let p = ui.painter().clone();
    paint_background(&p, ed);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;
    let ins = host.ins;
    let a = applies(ins);
    let machine = model(ins, "machine");
    let bpm = effective_bpm(host, readouts(host));

    // ---- 1. header
    y = family_header(app, ui, host, pal, &mut st.fam, &FAM, ("Reverb", "Engine"), (x0, w, y), AbPlace::Centred(3.0));

    // ---- 2. the machine: one menu of the six (the full editor's segmented row and its blurb)
    let entries: Vec<MenuEntry> =
        rc::MACHINES.iter().enumerate().map(|(i, m)| MenuEntry { name: m, tag: "", blurb: rc::machine_blurb(i as i32), credit: "" }).collect();
    let cur = machine.clamp(0, 5) as usize;
    if let Some(i) = model_menu_ex(
        ui,
        row(y, MODEL_H),
        "reverb-machine",
        "Machine",
        pal,
        &entries,
        cur,
        std::mem::take(&mut st.machines_open),
        MenuLook::BASELINE_10,
    ) {
        crate::editor_kit::family::set(app, host, &mut st.fam, &FAM, "machine", i as f32);
    }
    y += MODEL_H + 6.0;

    // ---- 3. the era (not on the Spring), its blurb when it is not "Now"
    if a.color {
        segmented_param(app, ui, host, &mut st.fam, &FAM, row(y, ROW_H + 2.0), pal, "color", None, 1, SelectionStyle::Filled, false);
        y += ROW_H + 2.0 + 4.0;
        let blurb = rc::color_blurb(model(ins, "color"));
        if !blurb.is_empty() {
            y += caption(ui, row(y, 0.0), blurb, pal.text_body, 3) + 2.0;
        }
    }
    y += GAP - 4.0;

    // ---- 4. the main dials, two to a row
    let cell = vec2((w / 3.0).floor(), BIG_KNOB + 32.0);
    let cells = grid(x0, w, y, MAIN.len(), 2, cell, 10.0);
    for (id, r) in MAIN.iter().zip(&cells) {
        let label = match *id {
            "decay" => "Decay",
            "size" => "Size",
            "mix" => "Mix",
            _ => rc::character_label(machine),
        };
        super::dial_ui(app, ui, host, &mut st.fam, *r, id, label, false, bpm);
    }
    if let Some(last) = cells.last() {
        y = last.bottom() + GAP;
    }

    // ---- 5. the drawer: its tabs (the help line is their tooltip) and the open tab's panel
    if let Some(i) = segmented(ui, row(y, 24.0), "tabs", pal, &TABS, 1, Some(st.tab), Default::default()) {
        st.tab = i;
    }
    let help = help_text(st.tab);
    if !help.is_empty() {
        tip(ui, segmented_id(ui, "tabs"), &help.replace('\n', " "));
    }
    y += 24.0 + 6.0;
    y = tab_panel(st, app, ui, host, (x0, w, y), bpm);
    y += GAP;

    // ---- 6. the foot: the tempo readout left, Freeze right
    let pill_h = pill_height(PillStyle::Small);
    let fr = Rect::from_min_size(pos2(x0 + w - 62.0, y), vec2(62.0, pill_h));
    super::freeze_ui(app, ui, host, &mut st.fam, fr);
    let readout = Rect::from_min_max(pos2(x0, y), pos2(fr.left() - 8.0, y + pill_h));
    let text = super::tempo_readout(param(host, "tempoSrc") < 0.5, host_bpm(host), bpm);
    p.with_clip_rect(readout).text(pos2(readout.left(), readout.center().y), Align2::LEFT_CENTER, text, font(CAPTION_SIZE, false), pal.text_faint);
    y += pill_h;

    slim::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
    if readouts(host).is_some() || st.fam.presets.message.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// The open tab's panel from `y` across `[x0, x0 + w]`: its dials three to a row, its switches
/// under them. Returns the y under it.
fn tab_panel(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, (x0, w, y): (f32, f32, f32), bpm: f32) -> f32 {
    let pal = &PALETTE;
    let a = applies(host.ins);
    let inner_x = x0 + 6.0;
    let inner_w = w - 12.0;
    let cw = (inner_w / 3.0).floor();
    let cell_h = SMALL_KNOB + 32.0;
    let top = y + 8.0;
    // The three columns of the panel (the full editor's `columns (panel, 3)`), `rows` tall.
    let col = |i: usize, rows: f32| Rect::from_min_size(pos2(inner_x + cw * i as f32, top), vec2(cw, cell_h * rows));
    let pill_h = 21.0;
    // The panel goes under what it holds: its height first.
    let body_h = match st.tab {
        TAB_TONE => tone_dials(&a).len().div_ceil(3) as f32 * (cell_h + 2.0) - 2.0,
        TAB_TIME => cell_h + 4.0 + pill_h,
        TAB_MOTION => cell_h + if a.interval { 6.0 + 26.0 } else { 0.0 },
        TAB_SETUP => cell_h + 6.0 + pill_h,
        _ => cell_h,
    };
    let panel = Rect::from_min_size(pos2(x0, y), vec2(w, body_h + 16.0));
    paint_panel(ui.painter(), panel, PANEL_CORNER);
    let dial = |app: &mut SoundApp, ui: &mut Ui, st: &mut Editor, r: Rect, id: &str, label: &str| {
        super::dial_ui(app, ui, host, &mut st.fam, r, id, label, true, bpm);
    };
    match st.tab {
        TAB_TONE => {
            let shown = tone_dials(&a);
            let cells = grid(inner_x, inner_w, top, shown.len(), 3, vec2(cw, cell_h), 2.0);
            for ((id, label), r) in shown.into_iter().zip(cells) {
                dial(app, ui, st, r, id, label);
            }
        }
        TAB_TIME => {
            let first = col(0, 1.0);
            if a.pre_delay {
                dial(app, ui, st, first, "preDelay", "Pre-Delay");
            }
            if a.division {
                dial(app, ui, st, first, "division", "Pre-Delay Note");
            }
            let sync_r = Rect::from_center_size(pos2(first.center().x, first.bottom() + 4.0 + pill_h / 2.0), vec2(52.0, 20.0));
            pill_param(app, ui, host, &mut st.fam, &FAM, sync_r, pal, "sync", "Sync", PLAIN);
            if a.attack {
                dial(app, ui, st, col(1, 1.0), "attack", "Attack");
            }
        }
        TAB_MOTION => {
            dial(app, ui, st, col(0, 1.0), "modRate", "Mod Rate");
            dial(app, ui, st, col(1, 1.0), "modDepth", "Mod Depth");
            if a.interval {
                let ir = Rect::from_min_size(pos2(inner_x, top + cell_h + 6.0), vec2(inner_w, 26.0));
                segmented_param(app, ui, host, &mut st.fam, &FAM, ir, pal, "interval", None, 1, SelectionStyle::Filled, false);
            }
        }
        TAB_DYNAMICS => {
            dial(app, ui, st, col(0, 1.0), "duck", "Duck");
            dial(app, ui, st, col(1, 1.0), "swell", "Swell");
        }
        TAB_SETUP => {
            dial(app, ui, st, col(0, 1.0), "width", "Width");
            dial(app, ui, st, col(1, 1.0), "level", "Level");
            dial(app, ui, st, col(2, 1.0), "tempo", "Manual BPM");
            let py = top + cell_h + 6.0;
            let total = 124.0 + 8.0 + 70.0;
            let px = (inner_x + (inner_w - total) / 2.0).round();
            pill_param(
                app,
                ui,
                host,
                &mut st.fam,
                &FAM,
                Rect::from_min_size(pos2(px, py), vec2(124.0, pill_h)),
                pal,
                "tempoSrc",
                "Use Manual BPM",
                PLAIN,
            );
            pill_param(
                app,
                ui,
                host,
                &mut st.fam,
                &FAM,
                Rect::from_min_size(pos2(px + 132.0, py), vec2(70.0, pill_h)),
                pal,
                "trails",
                "Trails",
                PLAIN,
            );
        }
        _ => {}
    }
    panel.bottom()
}

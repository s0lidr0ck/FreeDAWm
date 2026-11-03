//! DistEngine's editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections in its order, stacked. Header (title and preset strip; Undo, A, B, Bypass), the
//! Circuit as one menu, Input / Drive / Tone in a row and Level with Link, its set points and
//! Calibrate beside it, the calibration wizard under them while it runs, and a More section with
//! the circuit's switch rows (a menu, each option with its description, where a switch has more
//! than four options; a segmented switch with the description under it otherwise). The gestures
//! are the full editor's own code.

use super::{Editor, FAMILY, PALETTE, cal_spec, choices, circuit_of, dial_defs, dial_ui, link_text, link_ui, prelude, switch_state};
use crate::SoundApp;
use crate::editor_kit::controls::{PillStyle, SelectionStyle, pill_height, segmented_ex};
use crate::editor_kit::look::{CAPTION_SIZE, font, paint_background};
use crate::editor_kit::slim::{self as kit, GAP, MODEL_H, MORE_H, MenuEntry, MenuLook, PAD_X, PAD_Y, ROW_H};
use crate::editor_kit::{Host, family, link_cal};
use egui::{Align2, Rect, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::dist::wrapper as cfg;

const KNOB: f32 = 60.0;
const INPUT_KNOB: f32 = 46.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let ed = host.rect;
    let pal = &PALETTE;
    let ins = host.ins;
    let (circuit, c) = circuit_of(ins);
    paint_background(ui.painter(), ed);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header
    y = kit::family_header(app, ui, host, pal, &mut st.fam, &FAMILY, ("Dist", "Engine"), (x0, w, y), kit::AbPlace::Centred(4.0));

    // ---- 2. the circuit: one menu of the twelve, each with what its dials are called
    let tags: Vec<String> = cfg::CIRCUITS.iter().map(|c| format!("{} \u{00B7} {} \u{00B7} {}", c.dial_drive, c.dial_tone, c.dial_level)).collect();
    let items: Vec<MenuEntry> = cfg::CIRCUITS.iter().zip(&tags).map(|(c, t)| MenuEntry { name: c.name, tag: t, blurb: "", credit: "" }).collect();
    let open = std::mem::take(&mut st.circuit_open);
    if let Some(i) = kit::model_menu_ex(ui, row(y, MODEL_H), "circuit", "Circuit", pal, &items, circuit as usize, open, MenuLook::CENTRED) {
        family::set(app, host, &mut st.fam, &FAMILY, "circuit", i as f32);
    }
    y += MODEL_H + GAP;

    // ---- 3. Input, Drive and Tone; Level with Link, its set points and Calibrate beside it
    let defs = dial_defs(c);
    let col = (w / 3.0).floor();
    for (i, (pid, label, small)) in defs.iter().take(3).enumerate() {
        let k = if *small { INPUT_KNOB } else { KNOB };
        let cx = x0 + col * i as f32 + col / 2.0;
        let r = Rect::from_min_size(pos2((cx - (k + 30.0) / 2.0).round(), y + (KNOB - k) / 2.0), vec2(k + 30.0, k + 30.0));
        dial_ui(app, ui, host, st, r, pid, label, *small);
    }
    y += KNOB + 30.0 + 4.0;
    let (pid, label, small) = defs[3];
    let level_r = Rect::from_min_size(pos2((x0 + col / 2.0 - (KNOB + 30.0) / 2.0).round(), y), vec2(KNOB + 30.0, KNOB + 30.0));
    dial_ui(app, ui, host, st, level_r, pid, label, small);
    let btn_h = pill_height(PillStyle::Small);
    let stack_h = btn_h + 4.0 + 14.0 + 6.0 + btn_h;
    let sx = x0 + col + 8.0;
    let sy = y + ((KNOB - stack_h) / 2.0).max(0.0).trunc();
    let link_r = Rect::from_min_size(pos2(sx, sy), vec2(family::small_pill_w(ui, "Link").trunc().max(64.0), btn_h));
    let readout = Rect::from_min_size(pos2(sx, link_r.bottom() + 4.0), vec2(x0 + w - sx, 14.0));
    let cal_r = Rect::from_min_size(pos2(sx, readout.bottom() + 6.0), vec2(family::small_pill_w(ui, "Calibrate").trunc().max(78.0), btn_h));
    ui.painter().text(pos2(readout.left(), readout.center().y), Align2::LEFT_CENTER, link_text(ins, c), font(CAPTION_SIZE, false), pal.text_body);
    link_ui(app, ui, host, st, c, link_r, cal_r);
    y += KNOB + 30.0 + GAP;

    // ---- 4. the calibration wizard, while it runs
    if st.cal.active {
        let spec = cal_spec(c);
        let cal_h = link_cal::panel_height(ui, &st.cal, &spec, w);
        link_cal::panel(app, ui, host, pal, &mut st.cal, &spec, row(y, cal_h));
        y += cal_h + GAP;
    }

    // ---- 5. More: the circuit's switch rows
    let holds: Vec<&str> = c.switches.iter().map(|(_, l)| *l).collect();
    if !c.switches.is_empty() {
        if kit::more_header(ui, row(y, MORE_H), "dist", pal, st.slim_more, &holds.join(" \u{00B7} ")) {
            st.slim_more = !st.slim_more;
        }
        y += MORE_H;
        if st.slim_more {
            y += 4.0;
            for (pid, label) in c.switches {
                y = switch_row(app, ui, host, st, x0, w, y, pid, label) + 6.0;
            }
        }
    }
    kit::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

/// A switch row stacked: its name, then the switch (a menu past four options, each option with its
/// description; a segmented switch with the description under it otherwise). Returns its bottom.
#[allow(clippy::too_many_arguments)]
fn switch_row(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut Editor, x0: f32, w: f32, y: f32, pid: &'static str, label: &str) -> f32 {
    let pal = &PALETTE;
    let opts = choices(pid);
    let (sel, desc) = switch_state(host.ins, pid);
    kit::switch_label(ui, Rect::from_min_size(pos2(x0, y), vec2(w, 14.0)), label, pal);
    let r = Rect::from_min_size(pos2(x0, y + 16.0), vec2(w, ROW_H));
    if opts.len() > 4 {
        let descs = super::descriptions::DESCRIPTIONS.iter().find(|(id, _)| *id == pid).map_or(&[][..], |(_, d)| *d);
        let items: Vec<MenuEntry> =
            opts.iter().enumerate().map(|(i, o)| MenuEntry { name: o, tag: "", blurb: descs.get(i).copied().unwrap_or(""), credit: "" }).collect();
        if let Some(n) = kit::model_menu_ex(ui, r, pid, label, pal, &items, sel, false, MenuLook::CENTRED) {
            family::set(app, host, &mut st.fam, &FAMILY, pid, n as f32);
        }
        return r.bottom();
    }
    if let Some(n) = segmented_ex(ui, r, ("row", pid), pal, opts, 1, Some(sel), SelectionStyle::Filled, false) {
        family::set(app, host, &mut st.fam, &FAMILY, pid, n as f32);
    }
    r.bottom() + 4.0 + kit::wrapped(ui, Rect::from_min_size(pos2(x0 + 2.0, r.bottom() + 4.0), vec2(w - 4.0, 0.0)), desc, 9.5, pal.text_dim, 3)
}

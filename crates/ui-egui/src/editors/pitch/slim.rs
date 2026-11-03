//! PitchEngine's editor in the classic drawer's width, about a mixer strip tall: the full
//! editor's sections in its order, stacked. Header (title and preset strip; Undo, A, B, Bypass),
//! the style as one menu (each style with its blurb), the main knobs in rows of three, the switch
//! row (Whammy's Interval as a menu, POG's 5th Mode), the foot line with the OC-2 lamp, and a More
//! section holding the shaping row. Controls that do not apply to the style are hidden, as in the
//! full editor. The gestures are the full editor's own code.

use super::{Editor, FAMILY, PALETTE, dial, fifth_mode_ui, foot_text, lamp_of, main_ids, prelude, pv, shape_ids, style_of, timer};
use crate::SoundApp;
use crate::editor_kit::look::{CAPTION_SIZE, paint_background};
use crate::editor_kit::slim::{self as kit, GAP, MODEL_H, MORE_H, MenuEntry, MenuLook, PAD_X, PAD_Y, ROW_H};
use crate::editor_kit::{Host, controls, family};
use egui::{Rect, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::pitch::pitch_config as cfg;
use soundcraft_dsp::plugins::fs::pitch::pitch_engine_dsp as d;
use soundcraft_dsp::plugins::fs::pitch::{INTERVALS, PITCH_PARAMS};

const MAIN_KNOB: f32 = 56.0;
const SHAPE_KNOB: f32 = 38.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let ed = host.rect;
    let pal = &PALETTE;
    let ins = host.ins;
    let style = style_of(ins);
    let a = cfg::what_applies(style, pv(ins, d::P_FIFTH));
    paint_background(ui.painter(), ed);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header
    y = kit::family_header(app, ui, host, pal, &mut st.fam, &FAMILY, ("Pitch", "Engine"), (x0, w, y), kit::AbPlace::Centred(4.0));

    // ---- 2. the style: one menu, each style with its blurb
    let items: Vec<MenuEntry> =
        cfg::STYLES.iter().enumerate().map(|(i, name)| MenuEntry { name, tag: "", blurb: cfg::style_blurb(i as i32), credit: "" }).collect();
    let open = std::mem::take(&mut st.style_open);
    if let Some(i) = kit::model_menu_ex(ui, row(y, MODEL_H), "style", "Style", pal, &items, style as usize, open, MenuLook::CENTRED) {
        family::set(app, host, &mut st.fam, &FAMILY, "style", i as f32);
    }
    y += MODEL_H + GAP;

    // ---- 3. the main knobs, three to a row
    let main = main_ids(style);
    let n = kit::per_row(main.len());
    let cells = kit::grid(x0, w, y, main.len(), n, vec2((w / n.max(3) as f32).floor(), MAIN_KNOB + 32.0), 2.0);
    for (&i, r) in main.iter().zip(&cells) {
        dial(app, ui, host, st, *r, i, style, false);
    }
    y = kit::bottom_of(&cells, y) + GAP;

    // ---- 4. the switch row: Whammy's Interval (a menu of ten), POG's 5th Mode
    if a.interval {
        let items: Vec<MenuEntry> = INTERVALS.iter().map(|name| MenuEntry { name, tag: "Interval", blurb: "", credit: "" }).collect();
        let sel = pv(ins, d::P_INTERVAL).round().max(0.0) as usize;
        let open = std::mem::take(&mut st.interval_open);
        if let Some(i) = kit::model_menu_ex(ui, row(y, ROW_H + 2.0), "interval", "Interval", pal, &items, sel, open, MenuLook::CENTRED) {
            family::set(app, host, &mut st.fam, &FAMILY, "interval", i as f32);
        }
        y += ROW_H + 2.0 + GAP;
    }
    if a.fifth_mode {
        fifth_mode_ui(app, ui, host, st, row(y, ROW_H));
        y += ROW_H + GAP;
    }

    // ---- 5. the foot line, the OC-2 lamp before it
    let lamp = lamp_of(host);
    let mut tx = x0 + 2.0;
    if a.track {
        controls::led(ui.painter(), Rect::from_min_size(pos2(tx, y), vec2(14.0, 14.0)), pal, lamp > 0.5, pal.accent);
        tx += 14.0 + 6.0;
    }
    let text = foot_text(ins, style, lamp).replace("   \u{00B7}   ", " \u{00B7} ");
    y += kit::wrapped(ui, Rect::from_min_size(pos2(tx, y), vec2(x0 + w - tx - 2.0, 0.0)), &text, CAPTION_SIZE, pal.text_faint, 4).max(14.0) + GAP;

    // ---- 6. More: the shaping row
    let shape = shape_ids(style, &a);
    let label = |i: usize| cfg::knob_label(style, i).unwrap_or_else(|| PITCH_PARAMS.get(i).map_or("", |p| p.name));
    let holds: Vec<&str> = shape.iter().map(|i| label(*i)).collect();
    if kit::more_header(ui, row(y, MORE_H), "pitch", pal, st.slim_more, &holds.join(" \u{00B7} ")) {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        let n = kit::per_row(shape.len());
        let cells = kit::grid(x0, w, y, shape.len(), n, vec2((w / n.max(3) as f32).floor(), SHAPE_KNOB + 32.0), 2.0);
        for (&i, r) in shape.iter().zip(&cells) {
            dial(app, ui, host, st, *r, i, style, true);
        }
        y = kit::bottom_of(&cells, y);
    }
    timer(ui, host, style);
    kit::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

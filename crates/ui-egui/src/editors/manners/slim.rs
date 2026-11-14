//! Manners' editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections in its order, stacked. Header (title and preset strip; Undo, A, B, Bypass), the
//! display (its range edges drag here too), Tame and Recover, Delta, and a More section holding
//! Bias, Brighten and Boost and the working range in words. The gestures are the full editor's
//! own code.

use super::{Editor, FAMILY, PALETTE, delta_ui, dial_ui, display_ui, hz_text, prelude};
use crate::SoundApp;
use crate::editor_kit::look::{CAPTION_SIZE, paint_background, paint_panel};
use crate::editor_kit::slim::{self as kit, GAP, MORE_H, PAD_X, PAD_Y};
use crate::editor_kit::{Host, param};
use crate::theme::regular;
use egui::{Align2, Rect, Ui, pos2, vec2};

const DISPLAY_H: f32 = 170.0;
const BIG_KNOB: f32 = 70.0;
const SMALL_KNOB: f32 = 44.0;
/// A dial's label and readout under its knob.
const DIAL_TEXT: f32 = 30.0;
const DELTA_H: f32 = 22.0;

/// The dials in More: (parameter, label).
const MORE_DIALS: [(&str, &str); 3] = [("bias", "Bias"), ("brighten", "Brighten"), ("boost", "Boost")];

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let ed = host.rect;
    let pal = &PALETTE;
    paint_background(ui.painter(), ed);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header
    y = kit::family_header(app, ui, host, pal, &mut st.fam, &FAMILY, ("Manners", ""), (x0, w, y), kit::AbPlace::Centred(4.0));

    // ---- 2. the display
    display_ui(st, app, ui, host, row(y, DISPLAY_H), true);
    y += DISPLAY_H + GAP;

    // ---- 3. Tame and Recover side by side, Delta under them
    let half = (w / 2.0).floor();
    let panel_h = 8.0 + BIG_KNOB + DIAL_TEXT + 8.0 + DELTA_H + 8.0;
    paint_panel(ui.painter(), row(y, panel_h), 10.0);
    let top = y + 8.0;
    for (i, (id, label)) in [("tame", "Tame"), ("recover", "Recover")].into_iter().enumerate() {
        let col = Rect::from_min_size(pos2(x0 + half * i as f32, top), vec2(half, BIG_KNOB + DIAL_TEXT));
        let cell = Rect::from_center_size(col.center(), vec2(BIG_KNOB + 40.0, BIG_KNOB + DIAL_TEXT));
        dial_ui(app, ui, host, &mut st.fam, cell, id, label, false, true);
    }
    let delta_y = top + BIG_KNOB + DIAL_TEXT + 8.0 + DELTA_H * 0.5;
    let note = delta_ui(app, ui, host, &mut st.fam, pos2(x0 + 8.0 + 32.0, delta_y));
    ui.painter().text(pos2(x0 + 8.0 + 64.0 + 8.0, delta_y), Align2::LEFT_CENTER, note, regular(CAPTION_SIZE), pal.text_faint);
    y += panel_h + GAP;

    // ---- 4. More: the three small dials and the range in words
    let holds: Vec<&str> = MORE_DIALS.iter().map(|(_, l)| *l).collect();
    if kit::more_header(ui, row(y, MORE_H), "manners", pal, st.slim_more, &holds.join(" \u{00B7} ")) {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        let cells = kit::grid(x0, w, y, MORE_DIALS.len(), 3, vec2((w / 3.0).floor(), SMALL_KNOB + DIAL_TEXT + 2.0), 2.0);
        for ((id, label), r) in MORE_DIALS.iter().zip(&cells) {
            dial_ui(app, ui, host, &mut st.fam, *r, id, label, true, true);
        }
        y = kit::bottom_of(&cells, y) + 6.0;
        let (lo, hi) = (param(host, "rangeLo"), param(host, "rangeHi"));
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        let text = format!("Works from {} to {}: drag the display's edges", hz_text(lo), hz_text(hi));
        y += kit::wrapped(ui, Rect::from_min_size(pos2(x0 + 2.0, y), vec2(w - 4.0, 0.0)), &text, CAPTION_SIZE, pal.text_faint, 2);
    }
    kit::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

//! Sift's editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections in its order, stacked. Header (title and preset strip; Undo, A, B, Bypass), the
//! timeline, the two Amounts side by side, Listen, and a More section holding the ranges, the
//! sibilant band, the plosive ceiling and Mode. The gestures are the full editor's own code.

use super::{Editor, FAMILY, PALETTE, dial_ui, display_ui, listen_note, prelude, seg_ui};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::look::{CAPTION_SIZE, paint_background, paint_panel};
use crate::editor_kit::slim::{self as kit, GAP, MORE_H, PAD_X, PAD_Y, ROW_H};
use egui::{Rect, Ui, pos2, vec2};

const DISPLAY_H: f32 = 150.0;
const BIG_KNOB: f32 = 70.0;
const SMALL_KNOB: f32 = 44.0;
/// A dial's label and readout under its knob.
const DIAL_TEXT: f32 = 30.0;

/// The dials in More: (parameter, label).
const MORE_DIALS: [(&str, &str); 5] = [
    ("sibRange", "Sibilance Range"),
    ("sibLow", "Sibilance Low"),
    ("sibHigh", "Sibilance High"),
    ("ploRange", "Plosive Range"),
    ("ploHigh", "Plosive Ceiling"),
];

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
    y = kit::family_header(app, ui, host, pal, &mut st.fam, &FAMILY, ("Sift", ""), (x0, w, y), kit::AbPlace::Centred(4.0));

    // ---- 2. the timeline
    display_ui(ui, host, row(y, DISPLAY_H), true);
    y += DISPLAY_H + GAP;

    // ---- 3. the two Amounts side by side
    let half = (w / 2.0).floor();
    let panel_h = 8.0 + BIG_KNOB + DIAL_TEXT + 8.0;
    paint_panel(ui.painter(), row(y, panel_h), 10.0);
    for (i, (id, label)) in [("sibAmount", "Sibilance"), ("ploAmount", "Plosives")].into_iter().enumerate() {
        let col = Rect::from_min_size(pos2(x0 + half * i as f32, y + 8.0), vec2(half, BIG_KNOB + DIAL_TEXT));
        let cell = Rect::from_center_size(col.center(), vec2(BIG_KNOB + 40.0, BIG_KNOB + DIAL_TEXT));
        dial_ui(app, ui, host, &mut st.fam, cell, id, label, false);
    }
    y += panel_h + GAP;

    // ---- 4. Listen
    kit::row_label(ui, row(y, 14.0), "Listen", pal, false);
    y += 16.0;
    seg_ui(app, ui, host, &mut st.fam, row(y, ROW_H), "listen");
    y += ROW_H + 4.0;
    y += kit::wrapped(ui, Rect::from_min_size(pos2(x0 + 2.0, y), vec2(w - 4.0, 0.0)), listen_note(host), CAPTION_SIZE, pal.text_faint, 1) + GAP;

    // ---- 5. More: the ranges, the band, the ceiling and Mode
    if kit::more_header(ui, row(y, MORE_H), "sift", pal, st.slim_more, "Range \u{00B7} Band \u{00B7} Ceiling \u{00B7} Mode") {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        let cells = kit::grid(x0, w, y, MORE_DIALS.len(), 3, vec2((w / 3.0).floor(), SMALL_KNOB + DIAL_TEXT + 14.0), 2.0);
        for ((id, label), r) in MORE_DIALS.iter().zip(&cells) {
            dial_ui(app, ui, host, &mut st.fam, *r, id, label, true);
        }
        y = kit::bottom_of(&cells, y) + 8.0;
        kit::row_label(ui, row(y, 14.0), "Mode", pal, false);
        y += 16.0;
        seg_ui(app, ui, host, &mut st.fam, row(y, ROW_H), "mode");
        y += ROW_H;
    }
    kit::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

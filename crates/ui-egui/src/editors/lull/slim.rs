//! Lull's editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections in its order, stacked. The title and the preset strip, then Undo, A, B and Listen;
//! the scope with the threshold to drag and the line saying what the settings come to;
//! Threshold with Range and Release beside it over Expand/Gate; and a More section holding the
//! rest of the shape and timing, the detector (filters, key, Guard) and the output. The gestures
//! are the full editor's own code.

use super::{Editor, FAMILY, PALETTE, dial_ui, feed_history, header_pills, inactive, prelude, preset_strip_ui, scope_plot, scope_ui, summary};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::controls::{PillStyle, SelectionStyle, pill_height};
use crate::editor_kit::family;
use crate::editor_kit::look::{CAPTION_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::slim::{self as kit, GAP, MORE_H, PAD_X, PAD_Y, ROW_H};
use egui::{Rect, Ui, pos2, vec2};

const SCOPE_H: f32 = 150.0;
/// Two rows of the caption under the scope.
const READOUT_H: f32 = 26.0;
const BIG_KNOB: f32 = 70.0;
const SMALL_KNOB: f32 = 44.0;
/// The title row, then the pill row: where the scope starts, from the editor's top.
const SCOPE_TOP: f32 = PAD_Y + ROW_H + 2.0 + 6.0 + 21.0 + GAP;

/// The More section's dials, in rows of three, and the detector's, in rows of two.
const MORE_DIALS: [(&str, &str); 6] =
    [("ratio", "Ratio"), ("knee", "Knee"), ("hysteresis", "Hysteresis"), ("attack", "Attack"), ("hold", "Hold"), ("lookahead", "Lookahead")];
const DETECTOR_DIALS: [(&str, &str); 4] = [("sc_hp", "High-Pass"), ("sc_lp", "Low-Pass"), ("guard", "Guard"), ("guard_delay", "Guard Delay")];
const OUTPUT_DIALS: [(&str, &str); 3] = [("link", "Link"), ("mix", "Mix"), ("trim", "Trim")];

fn scope_panel(frame: Rect) -> Rect {
    Rect::from_min_size(pos2(frame.left() + PAD_X, frame.top() + SCOPE_TOP), vec2(frame.width() - 2.0 * PAD_X, SCOPE_H))
}

/// The scope's plot area in a slim editor drawn at `frame` (for tests and scripts).
pub fn slim_scope_rect(frame: Rect) -> Rect {
    scope_plot(scope_panel(frame))
}

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    // Scripts: `ui.mix.drawer {tab: "More"}` opens the More section ("Presets" opens the browser).
    if family::take_drawer_tab(app, host, &mut st.fam).as_deref() == Some("More") {
        st.slim_more = true;
    }
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

    // ---- 1. the header: the title and the preset strip, then Undo, A, B and Listen
    let head = row(y, ROW_H + 2.0);
    let title_w = paint_title(ui.painter(), head, "Lull", "Gate", pal);
    let (prev, name, next) = kit::preset_strip(head, title_w);
    preset_strip_ui(st, app, ui, host, [prev, name, next]);
    y += ROW_H + 2.0 + 6.0;
    let widths = [
        family::small_pill_w(ui, "Undo"),
        family::small_pill_w(ui, "A").max(28.0),
        family::small_pill_w(ui, "B").max(28.0),
        family::small_pill_w(ui, "Listen"),
    ];
    if let [undo, a, b, listen] = kit::pill_row(x0, w, y, &widths, &[1]).as_slice() {
        header_pills(st, app, ui, host, [*undo, *a, *b, *listen]);
    }
    y += pill_height(PillStyle::Small) + GAP;

    // ---- 2. the scope, and what the settings come to
    let panel = scope_panel(ed);
    let now = feed_history(st, host);
    scope_ui(st, app, ui, host, panel, now, true);
    let readout = Rect::from_min_size(pos2(x0 + 2.0, panel.bottom() + 4.0), vec2(w - 4.0, READOUT_H));
    kit::wrapped(ui, readout, &summary(host, true), CAPTION_SIZE, pal.text_faint, 2);
    y = y.max(readout.bottom()) + GAP;

    // ---- 3. the main dials: Threshold, with Range and Release beside it, over Expand / Gate
    let panel_h = 8.0 + BIG_KNOB + 30.0 + 6.0 + ROW_H + 10.0;
    paint_panel(ui.painter(), row(y, panel_h), 10.0);
    let top = y + 8.0;
    let big_w = BIG_KNOB + 30.0;
    let threshold = Rect::from_min_size(pos2(x0 + 4.0, top), vec2(big_w, BIG_KNOB + 30.0));
    dial_ui(app, ui, host, &mut st.fam, threshold, "threshold", "Threshold", true, false);
    let side_w = ((w - big_w - 8.0) / 2.0).floor();
    for (i, (id, label)) in [("range", "Range"), ("release", "Release")].into_iter().enumerate() {
        let cell =
            Rect::from_min_size(pos2(threshold.right() + side_w * i as f32, top + (BIG_KNOB - SMALL_KNOB) * 0.5), vec2(side_w, SMALL_KNOB + 30.0));
        dial_ui(app, ui, host, &mut st.fam, cell, id, label, false, false);
    }
    let seg = Rect::from_min_size(pos2(x0 + 8.0, top + BIG_KNOB + 30.0 + 6.0), vec2(w - 16.0, ROW_H));
    family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, seg, pal, "mode", None, 1, SelectionStyle::Filled, false);
    y += panel_h + GAP;

    // ---- 4. More: the rest of the shape and timing, the detector, the output
    if kit::more_header(ui, row(y, MORE_H), "lull", pal, st.slim_more, "Shape \u{00B7} Time \u{00B7} Detector \u{00B7} Key \u{00B7} Output") {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        let dials = |app: &mut SoundApp, ui: &mut Ui, st: &mut Editor, list: &[(&str, &str)], per_row: usize, y: f32| {
            let cell = vec2((w / per_row as f32).floor(), SMALL_KNOB + 32.0);
            let cells = kit::grid(x0, w, y, list.len(), per_row, cell, 2.0);
            for ((id, label), r) in list.iter().zip(&cells) {
                dial_ui(app, ui, host, &mut st.fam, *r, id, label, false, inactive(host, id));
            }
            kit::bottom_of(&cells, y)
        };
        y = dials(app, ui, st, &MORE_DIALS, 3, y) + GAP;
        let seg = Rect::from_min_size(pos2(x0 + 8.0, y), vec2(w - 16.0, ROW_H));
        family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, seg, pal, "key", None, 1, SelectionStyle::Filled, false);
        y += ROW_H + 6.0;
        y = dials(app, ui, st, &DETECTOR_DIALS, 2, y) + GAP;
        y = dials(app, ui, st, &OUTPUT_DIALS, 3, y);
    }
    kit::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

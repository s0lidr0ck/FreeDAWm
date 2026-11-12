//! Rez's editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections in its order, stacked. The title, then Undo, A, B and Delta; the graph (the same
//! bands, range handles and live cut, with fewer labels) and the line about the band in hand;
//! Depth with Sharpness and Selectivity beside it over Soft/Hard; and a More section holding
//! Attack and Release, the stereo group (L/R or M/S, Link, Balance) and Mix and Trim. The gestures
//! are the full editor's own code.

use super::{Editor, FAMILY, PALETTE, dial_ui, graph_ui, header_pills, sens_y};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::controls::{PillStyle, SelectionStyle, pill_height};
use crate::editor_kit::family;
use crate::editor_kit::look::{paint_background, paint_panel, paint_title};
use crate::editor_kit::slim::{self as kit, GAP, MORE_H, PAD_X, PAD_Y, ROW_H};
use crate::module_graphs::freq_to_x;
use egui::{Pos2, Rect, Ui, pos2, vec2};

const GRAPH_H: f32 = 176.0;
/// Two rows of the caption under the graph.
const READOUT_H: f32 = 26.0;
const BIG_KNOB: f32 = 70.0;
const SMALL_KNOB: f32 = 44.0;
/// The title row, then the pill row: where the graph starts, from the editor's top.
const GRAPH_TOP: f32 = PAD_Y + ROW_H + 2.0 + 6.0 + 21.0 + GAP;

/// The dials of the More section, in rows of two.
const MORE_DIALS: [(&str, &str); 4] = [("attack", "Attack"), ("release", "Release"), ("mix", "Mix"), ("trim", "Trim")];
const STEREO_DIALS: [(&str, &str); 2] = [("link", "Link"), ("balance", "Balance")];

fn graph_panel(frame: Rect) -> Rect {
    Rect::from_min_size(pos2(frame.left() + PAD_X, frame.top() + GRAPH_TOP), vec2(frame.width() - 2.0 * PAD_X, GRAPH_H))
}

/// The graph's plot area in a slim editor drawn at `frame` (for tests and scripts).
pub fn slim_plot_rect(frame: Rect) -> Rect {
    let panel = graph_panel(frame);
    Rect::from_min_max(pos2(panel.left() + 6.0, panel.top() + 6.0), pos2(panel.right() - 6.0, panel.bottom() - 16.0))
}

/// Where a band at `freq` Hz and `gain` dB of sensitivity has its dot in a slim editor drawn at
/// `frame` (for tests and scripts).
pub fn slim_band_pos(frame: Rect, freq: f32, gain: f32) -> Pos2 {
    let plot = slim_plot_rect(frame);
    pos2(freq_to_x(plot, freq), sens_y(plot, gain))
}

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    // Scripts: `ui.mix.drawer {tab: "More"}` opens the More section.
    if family::take_drawer_tab(app, host, &mut st.fam).as_deref() == Some("More") {
        st.slim_more = true;
    }
    let ed = host.rect;
    let pal = &PALETTE;
    paint_background(ui.painter(), ed);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. the header: the title, then Undo, A, B and Delta
    paint_title(ui.painter(), row(y, ROW_H + 2.0), "Rez", "Resonance", pal);
    y += ROW_H + 2.0 + 6.0;
    let widths = [
        family::small_pill_w(ui, "Undo"),
        family::small_pill_w(ui, "A").max(28.0),
        family::small_pill_w(ui, "B").max(28.0),
        family::small_pill_w(ui, "Delta"),
    ];
    if let [undo, a, b, delta] = kit::pill_row(x0, w, y, &widths, &[1]).as_slice() {
        header_pills(st, app, ui, host, [*undo, *a, *b, *delta]);
    }
    y += pill_height(PillStyle::Small) + GAP;

    // ---- 2. the graph and the line about the band in hand
    let panel = graph_panel(ed);
    let readout = Rect::from_min_size(pos2(x0 + 2.0, panel.bottom() + 4.0), vec2(w - 4.0, READOUT_H));
    graph_ui(st, app, ui, host, (panel, slim_plot_rect(ed), readout), true);
    y = y.max(readout.bottom()) + GAP;

    // ---- 3. the main dials: Depth, with Sharpness and Selectivity beside it, over Soft / Hard
    let panel_h = 8.0 + BIG_KNOB + 30.0 + 6.0 + ROW_H + 10.0;
    paint_panel(ui.painter(), row(y, panel_h), 10.0);
    let top = y + 8.0;
    let big_w = BIG_KNOB + 30.0;
    let depth = Rect::from_min_size(pos2(x0 + 4.0, top), vec2(big_w, BIG_KNOB + 30.0));
    dial_ui(app, ui, host, &mut st.fam, depth, "depth", "Depth", true);
    let side_w = ((w - big_w - 8.0) / 2.0).floor();
    for (i, (id, label)) in [("sharpness", "Sharpness"), ("selectivity", "Selectivity")].into_iter().enumerate() {
        let cell = Rect::from_min_size(pos2(depth.right() + side_w * i as f32, top + (BIG_KNOB - SMALL_KNOB) * 0.5), vec2(side_w, SMALL_KNOB + 30.0));
        dial_ui(app, ui, host, &mut st.fam, cell, id, label, false);
    }
    let seg = Rect::from_min_size(pos2(x0 + 8.0, top + BIG_KNOB + 30.0 + 6.0), vec2(w - 16.0, ROW_H));
    family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, seg, pal, "mode", None, 1, SelectionStyle::Filled, false);
    y += panel_h + GAP;

    // ---- 4. More: timing, the stereo group, the output
    if kit::more_header(ui, row(y, MORE_H), "rez", pal, st.slim_more, "Attack \u{00B7} Release \u{00B7} Stereo \u{00B7} Mix \u{00B7} Trim") {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        let cell = vec2((w / 2.0).floor(), SMALL_KNOB + 32.0);
        let cells = kit::grid(x0, w, y, MORE_DIALS.len(), 2, cell, 2.0);
        for ((id, label), r) in MORE_DIALS.iter().zip(&cells) {
            dial_ui(app, ui, host, &mut st.fam, *r, id, label, false);
        }
        y = kit::bottom_of(&cells, y) + GAP;
        let seg = Rect::from_min_size(pos2(x0 + 8.0, y), vec2(w - 16.0, ROW_H));
        family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, seg, pal, "stereo", None, 1, SelectionStyle::Filled, false);
        y += ROW_H + 6.0;
        let cells = kit::grid(x0, w, y, STEREO_DIALS.len(), 2, cell, 2.0);
        for ((id, label), r) in STEREO_DIALS.iter().zip(&cells) {
            dial_ui(app, ui, host, &mut st.fam, *r, id, label, false);
        }
        y = kit::bottom_of(&cells, y);
    }
    kit::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

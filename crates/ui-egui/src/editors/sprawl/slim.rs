//! Sprawl's editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections in its order, stacked. The title and the preset strip, then Undo, A, B and Mono Safe;
//! the scope over its correlation meter; the bands (the same bars, splits and live lane, with
//! fewer labels) and the line about the band in hand; Width with Stereoize and Trim beside it;
//! and a More section holding the four bands' widths as dials. The gestures are the full
//! editor's own code.

use super::{AXIS_H, Editor, LANE_H, PALETTE, SCALE_W, band_dials, band_pos_in, bands_ui, dial_ui, header_pills, prelude, preset_strip_ui, scope_ui};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::controls::{PillStyle, pill_height};
use crate::editor_kit::family;
use crate::editor_kit::look::{paint_background, paint_panel, paint_title};
use crate::editor_kit::slim::{self as kit, GAP, MORE_H, PAD_X, PAD_Y, ROW_H};
use egui::{Pos2, Rect, Ui, pos2, vec2};
use soundcraft_dsp::plugins::sprawl::SPLITS;

const SCOPE_H: f32 = 176.0;
const BANDS_H: f32 = 150.0;
/// Two rows of the caption under the bands.
const READOUT_H: f32 = 26.0;
const BIG_KNOB: f32 = 70.0;
const SMALL_KNOB: f32 = 44.0;
/// The title row, then the pill row, then the scope: where the bands start, from the editor's top.
const BANDS_TOP: f32 = PAD_Y + ROW_H + 2.0 + 6.0 + 21.0 + GAP + SCOPE_H + GAP;

fn bands_panel(frame: Rect) -> Rect {
    Rect::from_min_size(pos2(frame.left() + PAD_X, frame.top() + BANDS_TOP), vec2(frame.width() - 2.0 * PAD_X, BANDS_H))
}

/// The bands' plot area in a slim editor drawn at `frame` (for tests and scripts).
pub fn slim_bands_rect(frame: Rect) -> Rect {
    let panel = bands_panel(frame);
    Rect::from_min_max(pos2(panel.left() + 6.0, panel.top() + 6.0), pos2(panel.right() - 2.0 - SCALE_W, panel.bottom() - LANE_H - AXIS_H - 4.0))
}

/// Where band `i`'s bar is grabbed when its Width is `width` percent and the splits are `at` Hz,
/// in a slim editor drawn at `frame` (for tests and scripts).
pub fn slim_band_pos(frame: Rect, at: &[f32; SPLITS], i: usize, width: f32) -> Pos2 {
    band_pos_in(slim_bands_rect(frame), at, i, width)
}

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    // Scripts: `ui.mix.drawer {tab: "More"}` opens the More section.
    if family::take_drawer_tab(app, host, &mut st.fam).as_deref() == Some("More") {
        st.slim_more = true;
    }
    // "Presets" opens the browser, which covers the editor while it is open.
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

    // ---- 1. the header: the title, then Undo, A, B and Mono Safe
    let head = row(y, ROW_H + 2.0);
    let title_w = paint_title(ui.painter(), head, "Sprawl", "Imager", pal);
    let (prev, name, next) = kit::preset_strip(head, title_w);
    preset_strip_ui(st, app, ui, host, [prev, name, next]);
    y += ROW_H + 2.0 + 6.0;
    let widths = [
        family::small_pill_w(ui, "Undo"),
        family::small_pill_w(ui, "A").max(28.0),
        family::small_pill_w(ui, "B").max(28.0),
        family::small_pill_w(ui, "Mono Safe"),
    ];
    if let [undo, a, b, safe] = kit::pill_row(x0, w, y, &widths, &[1]).as_slice() {
        header_pills(st, app, ui, host, [*undo, *a, *b, *safe]);
    }
    y += pill_height(PillStyle::Small) + GAP;

    // ---- 2. the scope over its correlation meter
    scope_ui(st, ui, host, row(y, SCOPE_H), true);

    // ---- 3. the bands and the line about the band in hand
    let panel = bands_panel(ed);
    let readout = Rect::from_min_size(pos2(x0 + 2.0, panel.bottom() + 4.0), vec2(w - 4.0, READOUT_H));
    bands_ui(st, app, ui, host, (panel, slim_bands_rect(ed), readout), true);
    y = readout.bottom() + GAP;

    // ---- 4. the main dials: Width, with Stereoize and Trim beside it
    let panel_h = 8.0 + BIG_KNOB + 30.0 + 8.0;
    paint_panel(ui.painter(), row(y, panel_h), 10.0);
    let top = y + 8.0;
    let big_w = BIG_KNOB + 30.0;
    let width = Rect::from_min_size(pos2(x0 + 4.0, top), vec2(big_w, BIG_KNOB + 30.0));
    dial_ui(app, ui, host, &mut st.fam, width, "width", "Width", true);
    let side_w = ((w - big_w - 8.0) / 2.0).floor();
    for (i, (id, label)) in [("stereoize", "Stereoize"), ("trim", "Trim")].into_iter().enumerate() {
        let cell = Rect::from_min_size(pos2(width.right() + side_w * i as f32, top + (BIG_KNOB - SMALL_KNOB) * 0.5), vec2(side_w, SMALL_KNOB + 30.0));
        dial_ui(app, ui, host, &mut st.fam, cell, id, label, false);
    }
    y += panel_h + GAP;

    // ---- 5. More: the bands' widths as dials
    if kit::more_header(ui, row(y, MORE_H), "sprawl", pal, st.slim_more, "The four bands' widths") {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        let dials = band_dials();
        let cell = vec2((w / 2.0).floor(), SMALL_KNOB + 32.0);
        let cells = kit::grid(x0, w, y, dials.len(), 2, cell, 2.0);
        for ((id, label), r) in dials.iter().zip(&cells) {
            dial_ui(app, ui, host, &mut st.fam, *r, id, label, false);
        }
        y = kit::bottom_of(&cells, y);
    }
    kit::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

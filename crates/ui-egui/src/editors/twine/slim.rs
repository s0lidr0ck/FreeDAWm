//! Twine's editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections, names and order, folded to fit. Header (TWINE and the preset strip; "clip + limit",
//! Undo, Bypass), the clip / limit display kept large (the owner's favourite: it stays the editor's
//! centrepiece), the four bars as one compact row under it, the crossovers (in Multiband only),
//! Drive, Clip, Ceiling and Release two to a row, Match / Chaff / Multiband, and a More section
//! holding the clip shape, the limiter's character and its note. The gestures are the full
//! editor's own (`super::knob`, `super::seg_ui`, `super::toggle_ui`, `super::crossover_ui`,
//! `super::header_controls`).

use super::{ACCENT, BYPASS_RED, CLIP_COL, Editor, HeaderRects, IN_COL, LIM_COL, PALETTE, brighter, character_note, display, prelude, tooltip};
use crate::SoundApp;
use crate::editor_kit::controls::{PillStyle, pill_height};
use crate::editor_kit::look::{tracked_text, tracked_width};
use crate::editor_kit::palette::al;
use crate::editor_kit::slim::{self as sk, PAD_X, PAD_Y};
use crate::editor_kit::{Host, param};
use crate::theme::regular;
use egui::{Align2, Color32, CornerRadius, Rect, Stroke, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::twine as dsp;

const GAP: f32 = 8.0;
const ROW_H: f32 = 22.0;
const DISPLAY_H: f32 = 200.0;
const BARS_H: f32 = 34.0;
const XOVER_H: f32 = 50.0;
const KNOB: f32 = 60.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let pal = &PALETTE;
    let ed = host.rect;
    if prelude(st, app, ui, host) {
        return;
    }
    let p = ui.painter().clone();
    p.rect_filled(ed, CornerRadius::ZERO, super::ROOT_BACK);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header: TWINE and the preset strip; then "clip + limit", Undo and Bypass
    let head = row(y, ROW_H + 2.0);
    let tw = tracked_width(&p, "TWINE", 15.0, 0.24, true) + 4.0;
    tracked_text(&p, Rect::from_min_size(head.min, vec2(tw, head.height())), Align2::LEFT_CENTER, "TWINE", 15.0, 0.24, true, true, pal.text_bright);
    let (prev, name, next) = sk::preset_strip(head, tw);
    y += ROW_H + 2.0 + 6.0;
    let ph = pill_height(PillStyle::Small);
    let sub = Rect::from_min_size(pos2(x0, y), vec2(110.0, ph));
    tracked_text(&p, sub, Align2::LEFT_CENTER, "clip + limit", 10.0, 0.12, true, false, ACCENT);
    let bypass = Rect::from_min_size(pos2(x0 + w - 62.0, y), vec2(62.0, ph));
    let undo = Rect::from_min_size(pos2(bypass.left() - 6.0 - 52.0, y), vec2(52.0, ph));
    super::header_controls(st, app, ui, host, &HeaderRects { bypass, undo, prev, next, name });
    y += ph + GAP + 2.0;

    // ---- 2. the display: the last ten seconds of what went in, what came out, and what each
    // stage took off
    let ceiling_db = dsp::knob_to_ceiling_db(param(host, "ceiling"));
    display::paint(&p, row(y, DISPLAY_H), &mut st.history, ceiling_db);
    y += DISPLAY_H + 6.0;

    // ---- 3. the four bars as one row: In, Clip, Limit, Out TP
    bars(st, ui, row(y, BARS_H), ceiling_db);
    y += BARS_H + GAP;

    // ---- 4. the crossovers, in Multiband
    if param(host, "multi") > 0.5 {
        super::crossover_ui(st, app, ui, host, row(y, XOVER_H));
        y += XOVER_H + GAP;
    }

    // ---- 5. Drive, Clip, Ceiling, Release, two to a row
    let cell = vec2((w / 3.0).floor() + 10.0, KNOB + 34.0);
    let cells = sk::grid(x0, w, y, 4, 2, cell, 0.0);
    for (id, r) in ["drive", "clip", "ceiling", "release"].into_iter().zip(&cells) {
        if let Some(resp) = super::knob(st, app, ui, host, *r, id) {
            let _ = resp.on_hover_text(tooltip(id));
        }
    }
    y = sk::bottom_of(&cells, y) + GAP;

    // ---- 6. Match, Chaff, Multiband
    let cells = sk::pill_row(x0, w, y, &[80.0, 80.0, 96.0], &[]);
    for (id, r) in ["match", "chaff", "multi"].into_iter().zip(&cells) {
        super::toggle_ui(app, ui, host, r.with_max_y(r.top() + 24.0), id);
    }
    y += 24.0 + GAP + 2.0;

    // ---- 7. More: the clip shape, the limiter's character and what it does
    if sk::more_header(ui, row(y, 20.0), "twine", pal, st.slim_more, "Clip shape \u{00B7} Limiter") {
        st.slim_more = !st.slim_more;
    }
    y += 20.0;
    if st.slim_more {
        y += 8.0;
        for (id, label) in [("shape", "Clip shape"), ("character", "Limiter")] {
            tracked_text(&p, row(y, 14.0), Align2::LEFT_CENTER, label, 9.5, 0.1, true, false, pal.text_label);
            y += 14.0;
            super::seg_ui(st, app, ui, host, row(y, 26.0), id);
            y += 26.0 + 10.0;
        }
        let note = character_note(param(host, "character").round() as i32);
        y += sk::wrapped(ui, row(y, 40.0), note, 10.0, pal.text_faint, 3) + 2.0;
    }
    y += PAD_Y;
    sk::settle(ui, ed.top(), y, &mut st.slim_h);
}

/// The four bars as one row in `r`: each its name, its held value and a thin bar (the gain
/// reductions grow from the left over 12 dB, the levels over the 48 dB the tall bars show; Out TP
/// marks the ceiling and turns red over it). The same meters as the full editor's.
fn bars(st: &Editor, ui: &Ui, r: Rect, ceiling_db: f32) {
    let p = ui.painter();
    let defs = [("In", brighter(IN_COL, 0.6), false), ("Clip", CLIP_COL, true), ("Limit", LIM_COL, true), ("Out TP", ACCENT, false)];
    let cw = (r.width() / 4.0).floor();
    for (i, (name, colour, reduction)) in defs.into_iter().enumerate() {
        let col = Rect::from_min_size(pos2(r.left() + cw * i as f32, r.top()), vec2(cw, r.height())).shrink2(vec2(4.0, 0.0));
        let Some(m) = st.meters.get(i) else { continue };
        let h = m.hold;
        let txt = if reduction {
            if h < 0.05 { "0.0".to_string() } else { format!("-{h:.1}") }
        } else if h <= -119.0 {
            "-inf".to_string()
        } else {
            format!("{h:.1}")
        };
        let ink = if i == 3 && h > ceiling_db + 0.05 { BYPASS_RED } else { PALETTE.text_bright };
        tracked_text(p, Rect::from_min_size(col.min, vec2(col.width(), 12.0)), Align2::LEFT_CENTER, name, 8.5, 0.08, true, false, PALETTE.text_label);
        p.text(pos2(col.left(), col.top() + 19.0), Align2::LEFT_CENTER, txt, regular(10.5), ink);
        let bar = Rect::from_min_size(pos2(col.left(), col.bottom() - 5.0), vec2(col.width(), 4.0));
        p.rect_filled(bar, CornerRadius::same(2), al(Color32::BLACK, 0.45));
        let frac = if reduction { (m.now / 12.0).clamp(0.0, 1.0) } else { ((m.now + 48.0) / 48.0).clamp(0.0, 1.0) };
        if frac > 0.0 {
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), CornerRadius::same(2), colour);
        }
        if i == 3 {
            let x = (bar.left() + bar.width() * ((ceiling_db + 48.0) / 48.0).clamp(0.0, 1.0)).round() + 0.5;
            p.line_segment([pos2(x, bar.top() - 3.0), pos2(x, bar.bottom() + 1.0)], Stroke::new(1.0, ACCENT));
        }
    }
}

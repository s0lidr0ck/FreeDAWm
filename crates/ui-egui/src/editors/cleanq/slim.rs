//! CleanQ's editor in the classic drawer's width ([`crate::module_drawer::DRAWER_W`]), about a
//! mixer strip tall: the full editor's sections, names and order, folded to fit. The top bar on
//! two rows (CLEANQ, + Band, Presets, the fold; Undo / Redo, A / A->B, polarity, Byp), the graph
//! with its analyser and every edge button (Range, RTA, Ceiling, Guides and +, the four corner
//! shortcuts), the Ceiling strip under it while it is open (its rows wrapped), the bands: a chip
//! per band in use (a press selects it, as a press on its node does) and the selected band's card
//! docked under them (the floating card's controls, names and order, at full width), or the
//! guide being written, and a More section holding the rest of the bar: Pre / Post / Hold,
//! Copy / Paste / Clear, the Out box and Auto. Everything is the full editor's own code.

use super::band_card::{band_guide, card_body, card_content_h};
use super::guide_card::{guide_card_at, guide_card_h};
use super::*;
use crate::editor_kit::slim::{GAP, PAD_Y, more_header, settle};

const SIDE: f32 = crate::editor_kit::slim::PAD_X;
/// Two rows of buttons.
const BAR_H: f32 = 6.0 + BTN_H + 6.0 + BTN_H + 6.0;
const GRAPH_H: f32 = 236.0;
const CHIP_W: f32 = 30.0;
const CHIP_H: f32 = 20.0;

/// The bar, the graph and (while it is open) the Ceiling strip, sized by its last layout.
pub(super) fn layout(st: &Editor, ed: Rect) -> (Rect, Rect, Option<Rect>) {
    let bar = Rect::from_min_size(ed.min, vec2(ed.width(), BAR_H));
    let graph = Rect::from_min_size(pos2(ed.left(), bar.bottom()), vec2(ed.width(), GRAPH_H));
    let strip = st.ceiling_open.then(|| Rect::from_min_size(pos2(ed.left(), graph.bottom()), vec2(ed.width(), st.slim_ceil_h.max(80.0))));
    (bar, graph, strip)
}

/// The top bar on two rows: CLEANQ, + Band ... Presets, the fold; then Undo / Redo, A / A->B,
/// the polarity and Byp.
pub(super) fn top_bar(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, bar: Rect) {
    let p = ui.painter().clone();
    paint_bar(&p, bar);
    let (x0, x1) = (bar.left() + SIDE, bar.right() - SIDE);
    let y1 = bar.top() + 6.0;
    let at = |x: f32, y: f32, w: f32| Rect::from_min_size(pos2(x, y), vec2(w, BTN_H));

    let tw = title_w(&p);
    paint_cq_title(&p, at(x0, y1, tw));
    add_band_button(app, ui, cx, st, at(x0 + tw, y1, pref_w(ui, "+ Band", 18.0)));
    let fold = at(x1 - 20.0, y1, 20.0);
    let pw = pref_w(ui, PRESETS_LABEL, 18.0);
    presets_button(app, ui, cx, st, at(fold.left() - BTN_GAP - 4.0 - pw, y1, pw));
    fold_button(app, ui, cx, fold);

    let y2 = y1 + BTN_H + 6.0;
    undo_redo(app, ui, at(x0, y2, 26.0), at(x0 + 26.0, y2, 26.0));
    let ab_x = ((x0 + x1) / 2.0 - (28.0 + 38.0) / 2.0).round();
    ab_buttons(app, ui, cx, st, at(ab_x, y2, 28.0), at(ab_x + 28.0, y2, 38.0));
    let bw = pref_w(ui, "Byp", 18.0);
    let byp = at(x1 - bw, y2, bw);
    invert_button(app, ui, cx, at(byp.left() - BTN_GAP - 28.0, y2, 28.0));
    bypass_button(app, ui, cx, byp);
}

/// Everything under the graph: the Ceiling strip (while open), the bands and their card (or the
/// guide being written), and More. Records the editor's height.
pub(super) fn under_graph(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, strip: Option<Rect>) {
    let ed = cx.host.rect;
    let x0 = ed.left() + SIDE;
    let w = ed.width() - 2.0 * SIDE;
    let mut y = cx.graph.bottom();

    // ---- the Ceiling strip, its rows wrapped
    if let Some(s) = strip {
        let bottom = ceiling_strip::ceiling_strip(app, ui, cx, st, s, true);
        let h = (bottom - s.top()).ceil();
        if (h - st.slim_ceil_h).abs() > 0.5 {
            st.slim_ceil_h = h;
            ui.ctx().request_repaint();
        }
        y = s.top() + h;
    }
    y += GAP;

    // ---- the bands: a chip each, then the selected one's card (or the guide being written)
    y = band_chips(ui, cx, st, x0, w, y) + 6.0;
    if st.guide_edit.is_some() {
        let h = guide_card_h(ui, st, w);
        guide_card_at(app, ui, cx, st, Rect::from_min_size(pos2(x0, y), vec2(w, h)));
        y += h;
    } else if let Some(b) = st.selected.and_then(|s| cx.band(s)) {
        // The band's controls, then the guide it sits in (read under them).
        let guide = band_guide(ui, cx, st, &b, w - 20.0);
        let info_h = guide.as_ref().map_or(0.0, |(.., h)| *h);
        let h = 10.0 + card_content_h(&b) + info_h + 10.0;
        let card = Rect::from_min_size(pos2(x0, y), vec2(w, h));
        let p = ui.painter();
        p.rect_filled(card, CornerRadius::same(10), CARD_BG);
        p.rect_stroke(card.shrink(0.5), CornerRadius::same(10), Stroke::new(1.0, al(band_colour(b.slot), 0.45)), StrokeKind::Middle);
        let inner = card.shrink(10.0);
        card_body(app, ui, cx, st, b, inner);
        if let Some((k, g, _)) = &guide {
            let r = Rect::from_min_max(pos2(inner.left(), inner.top() + card_content_h(&b)), inner.max);
            guide_info::draw_info(ui.painter(), r, *k, g);
        }
        y += h;
    } else {
        let any = cx.bands.iter().any(BandV::used);
        let hint = if any { "Click a band or its node to edit it." } else { "+ Band, a corner shortcut, or click the graph to add one." };
        ui.painter().text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, hint, regular(10.5), TEXT_FAINT);
        y += 16.0;
    }
    y += GAP;

    // ---- More: the rest of the bar
    if more_header(
        ui,
        Rect::from_min_size(pos2(x0, y), vec2(w, 20.0)),
        "cleanq",
        &PALETTE,
        st.slim_more,
        "Pre \u{00B7} Post \u{00B7} Hold \u{00B7} Copy \u{00B7} Paste \u{00B7} Clear \u{00B7} Out \u{00B7} Auto",
    ) {
        st.slim_more = !st.slim_more;
    }
    y += 20.0;
    if st.slim_more {
        y += 6.0;
        let at = |x: f32, y: f32, w: f32| Rect::from_min_size(pos2(x, y), vec2(w, BTN_H));
        // Pre | Post | Hold, then Copy | Paste | Clear
        let (wp, wo, wh) = (pref_w(ui, "Pre", 16.0), pref_w(ui, "Post", 16.0), pref_w(ui, "Hold", 16.0));
        analyser_buttons(ui, st, [at(x0, y, wp), at(x0 + wp, y, wo), at(x0 + wp + wo, y, wh)]);
        let (wc, wpa, wcl) = (pref_w(ui, "Copy", 18.0), pref_w(ui, "Paste", 18.0), pref_w(ui, "Clear", 18.0));
        let cx0 = x0 + w - (wc + wpa + wcl);
        clipboard_buttons(app, ui, cx, st, [at(cx0, y, wc), at(cx0 + wc, y, wpa), at(cx0 + wc + wpa, y, wcl)]);
        y += BTN_H + 6.0;
        // The Out box and Auto
        let aw = pref_w(ui, "Auto", 18.0);
        out_box(app, ui, cx, at(x0, y, w - aw - BTN_GAP));
        auto_button(app, ui, cx, at(x0 + w - aw, y, aw));
        y += BTN_H;
    }
    settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

/// A chip per band in use, in its colour, wrapping: a press selects the band (as a press on its
/// node does). Returns the y under them.
fn band_chips(ui: &mut Ui, cx: &Cx, st: &mut Editor, x0: f32, w: f32, y: f32) -> f32 {
    let p = ui.painter().clone();
    let lw = 44.0;
    p.text(pos2(x0, y + CHIP_H * 0.5), Align2::LEFT_CENTER, "BANDS", bold(9.5), TEXT_FAINT);
    let per_row = (((w - lw) + 4.0) / (CHIP_W + 4.0)).floor().max(1.0) as usize;
    let used: Vec<BandV> = cx.bands.iter().copied().filter(BandV::used).collect();
    if used.is_empty() {
        p.text(pos2(x0 + lw, y + CHIP_H * 0.5), Align2::LEFT_CENTER, "none yet", regular(10.0), TEXT_FAINT);
        return y + CHIP_H;
    }
    let mut bottom = y + CHIP_H;
    for (i, b) in used.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(x0 + lw + (i % per_row) as f32 * (CHIP_W + 4.0), y + (i / per_row) as f32 * (CHIP_H + 4.0)),
            vec2(CHIP_W, CHIP_H),
        );
        bottom = r.bottom();
        let resp = ui.interact(r, ui.id().with(("cq-slim-chip", b.slot)), Sense::click());
        let sel = st.selected == Some(b.slot);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, sel, format!("Band {}", b.slot + 1)));
        let col = band_colour(b.slot);
        let live = if b.active() { 1.0 } else { 0.45 };
        p.rect_filled(r, CornerRadius::same(4), if sel { al(col, 0.22) } else { PALETTE.field });
        let edge = if sel {
            col
        } else if resp.hovered() {
            PALETTE.border_hover
        } else {
            BORDER
        };
        p.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, edge), StrokeKind::Inside);
        p.circle_filled(pos2(r.left() + 7.0, r.center().y), 3.5, al(col, live));
        p.text(
            pos2(r.left() + 19.0, r.center().y),
            Align2::CENTER_CENTER,
            format!("{}", b.slot + 1),
            bold(10.5),
            al(if sel { TEXT_BRIGHT } else { TEXT_DIM }, live),
        );
        if resp.clicked() {
            st.guide_edit = None;
            select(st, Some(b.slot));
        }
    }
    bottom
}

//! `CleanQGuideCard.h`: the card that writes or edits a guide. Save, Delete and the Done toggle
//! change the guides in the module state (`uiState/guides`), each one undo step.

use super::band_card::field;
use super::*;

const PAD_Y: f32 = 10.0;

/// The card's controls' height (no padding, no guide information).
fn controls_h() -> f32 {
    let (head_h, value_h, note_h, row_h) = (18.0, 34.0, 24.0, 24.0);
    head_h + 6.0 + value_h + 6.0 + value_h + 6.0 + note_h + 8.0 + row_h
}

/// The guide's information under the controls, for a guide that has an area or a usual move (a
/// template's), at the card's inner width `w`; else nothing.
fn info(ui: &Ui, ge: &GuideEdit, w: f32) -> Option<(usize, f32)> {
    let k = ge.index?;
    (!ge.g.area.is_empty() || !ge.g.hint.is_empty()).then(|| (k, guide_info::info_h(ui.painter(), &ge.g, w)))
}

/// The card's height at width `w`.
pub(super) fn guide_card_h(ui: &Ui, st: &Editor, w: f32) -> f32 {
    let extra = st.guide_edit.as_ref().and_then(|ge| info(ui, ge, w - 20.0)).map_or(0.0, |(_, h)| h);
    PAD_Y * 2.0 + controls_h() + extra
}

/// The card over the graph, centred on the guide's frequency.
pub(super) fn guide_card(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor) {
    let Some(freq) = st.guide_edit.as_ref().map(|ge| ge.g.freq_hz) else { return };
    let h = guide_card_h(ui, st, GUIDE_W);
    let ga = cx.graph;
    let centre = cx.geo.fx(freq);
    let x = (centre - GUIDE_W * 0.5).round().clamp(ga.left() + 4.0, (ga.right() - GUIDE_W - 4.0).max(ga.left() + 4.0));
    let y = (ga.top() + 92.0).clamp(ga.top() + 4.0, (ga.bottom() - h - 4.0).max(ga.top() + 4.0));
    guide_card_at(app, ui, cx, st, Rect::from_min_size(pos2(x, y), vec2(GUIDE_W, h)));
}

/// The card in `card` (the slim layout docks it under the graph).
pub(super) fn guide_card_at(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, card: Rect) {
    let Some(mut ge) = st.guide_edit.clone() else { return };
    let (pad_y, head_h, value_h, note_h, row_h) = (PAD_Y, 18.0, 34.0, 24.0, 24.0);
    let col = guide_colour(ge.index.unwrap_or(cx.guides().len()));
    let _ = ui.interact(card, ui.id().with("cqw-guide-swallow"), Sense::click_and_drag());
    let p = ui.painter().clone();
    p.rect_filled(card.translate(vec2(0.0, 3.0)), CornerRadius::same(12), al(Color32::BLACK, 0.45));
    p.rect_filled(card, CornerRadius::same(12), al(CARD_BG, 0.98));
    p.rect_stroke(card.shrink(0.5), CornerRadius::same(12), Stroke::new(1.0, al(col, 0.45)), StrokeKind::Middle);

    let inner = card.shrink2(vec2(10.0, pad_y));
    let mut yy = inner.top();
    let head = Rect::from_min_size(pos2(inner.left(), yy), vec2(inner.width(), head_h));
    let dot = pos2(head.left() + 6.0, head.center().y);
    p.circle_filled(dot, 8.0, al(col, 0.45));
    p.circle_filled(dot, 5.0, col);
    let title = ge.index.map_or_else(|| "New guide".to_string(), |i| format!("Guide {}", i + 1));
    let tw = p.layout_no_wrap(title.clone(), bold(12.0), TEXT_BRIGHT).size().x + 8.0;
    p.text(pos2(head.left() + 18.0, head.center().y), Align2::LEFT_CENTER, title, bold(12.0), TEXT_BRIGHT);
    p.text(pos2(head.left() + 18.0 + tw, head.center().y), Align2::LEFT_CENTER, note_name(ge.g.freq_hz), regular(10.5), TEXT_FAINT);
    yy += head_h + 6.0;

    let w = (inner.width() / 3.0).floor();
    let cell = |i: usize, y: f32| Rect::from_min_size(pos2(inner.left() + w * i as f32, y), vec2(w, value_h)).shrink2(vec2(2.0, 0.0));
    if let Some(v) = field(ui, st, cell(0, yy), Field::GCentre, ge.g.freq_hz).changed {
        // Moving the centre carries the region with it.
        let ratio = v / ge.g.freq_hz.max(1.0);
        ge.g.lo_hz = (ge.g.lo_hz * ratio).clamp(FREQ_MIN, FREQ_MAX);
        ge.g.hi_hz = (ge.g.hi_hz * ratio).clamp(FREQ_MIN, FREQ_MAX);
        ge.g.freq_hz = v;
    }
    if let Some(v) = field(ui, st, cell(1, yy), Field::GFrom, ge.g.lo_hz).changed {
        ge.g.lo_hz = v;
    }
    if let Some(v) = field(ui, st, cell(2, yy), Field::GTo, ge.g.hi_hz).changed {
        ge.g.hi_hz = v;
    }
    yy += value_h + 6.0;
    let half = Rect::from_min_size(pos2(inner.left(), yy), vec2(inner.width() * 0.5, value_h)).shrink2(vec2(2.0, 0.0));
    if let Some(v) = field(ui, st, half, Field::GGain, ge.g.gain_db).changed {
        ge.g.gain_db = v;
    }
    let toggles = Rect::from_min_max(pos2(inner.left() + inner.width() * 0.5 + 2.0, yy + 5.0), pos2(inner.right() - 2.0, yy + value_h - 5.0));
    let tw2 = toggles.width() * 0.5;
    if card_button(
        ui,
        Rect::from_min_size(toggles.min, vec2(tw2, toggles.height())),
        "g-dyn",
        &PALETTE,
        Btn::new("Dynamic").on(ge.g.dynamic),
        Sense::click(),
    )
    .clicked()
    {
        ge.g.dynamic = !ge.g.dynamic;
    }
    if card_button(
        ui,
        Rect::from_min_max(pos2(toggles.left() + tw2 + 4.0, toggles.top()), toggles.max),
        "g-done",
        &PALETTE,
        Btn::new("Done").on(ge.g.done),
        Sense::click(),
    )
    .clicked()
    {
        ge.g.done = !ge.g.done;
    }
    yy += value_h + 6.0;

    // The note: Enter saves.
    let nr = Rect::from_min_size(pos2(inner.left() + 2.0, yy), vec2(inner.width() - 4.0, note_h));
    let mut commit = false;
    {
        let id = ui.id().with("cqw-guide-note");
        let focused = ui.memory(|m| m.has_focus(id));
        p.rect_filled(nr, CornerRadius::same(3), PALETTE.field);
        p.rect_stroke(nr, CornerRadius::same(3), Stroke::new(1.0, if focused { ACCENT } else { BORDER }), StrokeKind::Inside);
        let te = egui::TextEdit::singleline(&mut ge.g.note)
            .id(id)
            .font(egui::FontId::proportional(11.5))
            .text_color(TEXT_BRIGHT)
            .hint_text(egui::RichText::new("what is going on here").color(TEXT_FAINT))
            .frame(egui::Frame::NONE)
            .char_limit(80);
        let tr = ui.put(nr.shrink2(vec2(4.0, 3.0)), te);
        // Focus once the gesture that opened the card is over (the release of a press that
        // landed elsewhere would take the focus straight back).
        if ge.focus_note && !ui.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
            tr.request_focus();
            ge.focus_note = false;
        }
        if tr.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            commit = true;
        }
    }
    yy += note_h + 8.0;

    if let Some((k, _)) = info(ui, &ge, inner.width()) {
        let r = Rect::from_min_max(pos2(inner.left(), inner.top() + controls_h()), inner.max);
        guide_info::draw_info(&p, r, k, &ge.g);
    }

    let n = if ge.index.is_some() { 3.0 } else { 2.0 };
    let bw = inner.width() / n;
    let br = |i: f32| Rect::from_min_size(pos2(inner.left() + bw * i, yy), vec2(bw, row_h)).shrink2(vec2(2.0, 0.0));
    if card_button(ui, br(0.0), "g-save", &PALETTE, Btn::new("Save").bold(), Sense::click()).clicked() {
        commit = true;
    }
    let mut close = false;
    if let Some(i) = ge.index
        && card_button(ui, br(1.0), "g-delete", &PALETTE, Btn::new("Delete").danger(), Sense::click()).clicked()
    {
        let mut guides = cx.ceil.guides.clone();
        guides.remove(i);
        write_guides(app, cx, guides);
        close = true;
    }
    if card_button(ui, br(n - 1.0), "g-cancel", &PALETTE, Btn::new("Cancel"), Sense::click()).clicked() {
        close = true;
    }
    if commit {
        let mut guides = cx.ceil.guides.clone();
        match ge.index {
            Some(i) if i < guides.list.len() => guides.replace(i, ge.g.clone()),
            _ => {
                let _ = guides.add(ge.g.clone());
            }
        }
        write_guides(app, cx, guides);
        close = true;
    }
    st.guide_edit = if close { None } else { Some(ge) };
}

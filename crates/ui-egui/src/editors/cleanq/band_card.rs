//! `CleanQBandCard.h` (the band card, `GlyphButton`) and the type / place glyphs of
//! `CleanQIcons.h`.

use super::*;
use crate::editor_kit::cards::{CardStyle, POINTER_R, floating_card};
use crate::editor_kit::controls::{ValueOut, value_box};
use crate::editor_kit::icons::cubic;
use egui::Mesh;

// ============================================================================ CleanQIcons.h
/// The type glyph in the original's 32 x 20 box.
fn type_path(kind: i32) -> Vec<Pos2> {
    let pt = |x: f32, y: f32| pos2(x, y);
    let mut v = Vec::new();
    match kind {
        0 => {
            v.push(pt(2.0, 15.0));
            cubic(&mut v, pt(2.0, 15.0), pt(8.0, 15.0), pt(10.0, 4.0), pt(16.0, 4.0));
            cubic(&mut v, pt(16.0, 4.0), pt(22.0, 4.0), pt(24.0, 15.0), pt(30.0, 15.0));
        }
        1 => {
            v.push(pt(2.0, 5.0));
            cubic(&mut v, pt(2.0, 5.0), pt(10.0, 5.0), pt(12.0, 14.0), pt(18.0, 14.0));
            v.push(pt(30.0, 14.0));
        }
        2 => {
            v.push(pt(2.0, 14.0));
            v.push(pt(14.0, 14.0));
            cubic(&mut v, pt(14.0, 14.0), pt(20.0, 14.0), pt(22.0, 5.0), pt(30.0, 5.0));
        }
        3 => {
            v.push(pt(4.0, 18.0));
            cubic(&mut v, pt(4.0, 18.0), pt(8.0, 18.0), pt(10.0, 4.0), pt(16.0, 4.0));
            v.push(pt(30.0, 4.0));
        }
        4 => {
            v.push(pt(2.0, 4.0));
            v.push(pt(16.0, 4.0));
            cubic(&mut v, pt(16.0, 4.0), pt(22.0, 4.0), pt(24.0, 18.0), pt(28.0, 18.0));
        }
        5 => v.extend([pt(2.0, 6.0), pt(12.0, 6.0), pt(16.0, 17.0), pt(20.0, 6.0), pt(30.0, 6.0)]),
        _ => v.extend([pt(2.0, 16.0), pt(12.0, 16.0), pt(16.0, 4.0), pt(20.0, 16.0), pt(30.0, 16.0)]),
    }
    v
}

/// `getTransformToScaleToFit` of a path's bounds into `area`, keeping its aspect, centred.
fn fit(points: &[Pos2], area: Rect) -> Vec<Pos2> {
    let b = Rect::from_points(points);
    let s = (area.width() / b.width().max(1e-3)).min(area.height() / b.height().max(1e-3));
    let off = area.center() - b.center() * s;
    points.iter().map(|p| (p.to_vec2() * s + off).to_pos2()).collect()
}

/// `icons::drawType`.
pub(super) fn draw_type(p: &egui::Painter, kind: i32, area: Rect, col: Color32) {
    let pts = fit(&type_path(kind), area.shrink(1.0));
    let w = (2.0 * area.width() / 32.0).max(1.0);
    p.add(Shape::line(pts, Stroke::new(w, col)));
}

/// `icons::drawPlace`: two r=6 circles at (12,10) and (20,10) in the 32 x 20 box.
fn draw_place(p: &egui::Painter, place: i32, area: Rect, col: Color32) {
    let a = area.shrink(1.0);
    let s = (a.width() / 32.0).min(a.height() / 20.0);
    let off = a.center() - vec2(16.0, 10.0) * s;
    let tp = |x: f32, y: f32| pos2(off.x + x * s, off.y + y * s);
    let stroke = (2.0 * s).max(1.0);
    let (l, r, rad) = (tp(12.0, 10.0), tp(20.0, 10.0), 6.0 * s);
    // Filled regions, column by column: the circles' vertical extents at x.
    let half = |c: Pos2, x: f32| {
        let d = rad * rad - (x - c.x) * (x - c.x);
        if d > 0.0 { d.sqrt() } else { -1.0 }
    };
    let mut mesh = Mesh::default();
    let mut col_quad = |x0: f32, x1: f32, y0: f32, y1: f32| {
        if y1 <= y0 {
            return;
        }
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(x0, y0), col);
        mesh.colored_vertex(pos2(x1, y0), col);
        mesh.colored_vertex(pos2(x1, y1), col);
        mesh.colored_vertex(pos2(x0, y1), col);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    };
    let step = 0.5;
    let mut x = l.x - rad;
    while x < r.x + rad {
        let xm = x + step * 0.5;
        let (hl, hr) = (half(l, xm), half(r, xm));
        let cy = l.y;
        match place {
            1 if hl > 0.0 => col_quad(x, x + step, cy - hl, cy + hl),
            2 if hr > 0.0 => col_quad(x, x + step, cy - hr, cy + hr),
            3 if hl > 0.0 && hr > 0.0 => {
                let h = hl.min(hr);
                col_quad(x, x + step, cy - h, cy + h);
            }
            4 => {
                let (hi, lo) = (hl.max(hr), hl.min(hr));
                if hi > 0.0 {
                    if lo > 0.0 {
                        col_quad(x, x + step, cy - hi, cy - lo);
                        col_quad(x, x + step, cy + lo, cy + hi);
                    } else {
                        col_quad(x, x + step, cy - hi, cy + hi);
                    }
                }
            }
            _ => {}
        }
        x += step;
    }
    p.add(Shape::mesh(mesh));
    let (la, ra) = match place {
        1 => (1.0, 0.35),
        2 => (0.35, 1.0),
        _ => (1.0, 1.0),
    };
    p.circle_stroke(l, rad, Stroke::new(stroke, al(col, la)));
    p.circle_stroke(r, rad, Stroke::new(stroke, al(col, ra)));
}

/// `GlyphButton::paint`.
fn glyph_button(ui: &mut Ui, r: Rect, salt: impl std::hash::Hash + std::fmt::Debug, place: bool, index: i32, selected: bool) -> egui::Response {
    let resp = ui.interact(r, ui.id().with(("cqw-glyph", salt)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, if place { "Place" } else { "Type" }));
    let hovering = resp.hovered();
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(5), if selected { ACCENT } else { PALETTE.field });
    p.rect_stroke(
        r,
        CornerRadius::same(5),
        Stroke::new(
            1.0,
            if selected {
                ACCENT
            } else if hovering {
                PALETTE.border_hover
            } else {
                BORDER
            },
        ),
        StrokeKind::Inside,
    );
    let ink = if selected {
        PALETTE.on_accent
    } else if hovering {
        TEXT_BRIGHT
    } else {
        TEXT_DIM
    };
    let g = r.shrink2(vec2(r.width() * 0.17, r.height() * 0.22));
    if place {
        draw_place(p, index, g, ink)
    } else {
        draw_type(p, index, g, ink)
    }
    resp
}

// ============================================================================ BandCard
/// The card's controls' height (its padding excluded): more with the dynamics open.
pub(super) fn card_content_h(b: &BandV) -> f32 {
    let (head_h, value_h, row_h, sub_h) = (18.0, 34.0, 24.0, 12.0);
    let mut h = head_h + 6.0 + value_h + 8.0 + row_h + 6.0 + row_h + 6.0 + row_h;
    if b.has_gain() && b.dyn_ {
        h += 6.0 + sub_h + 4.0 + value_h + 6.0 + value_h;
    }
    h
}

pub(super) fn band_card(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, b: BandV) {
    let slot = b.slot;
    let col = band_colour(slot);
    let pad_top = 10.0;
    // The guide the band sits in, read under the band's controls.
    let guide = band_guide(ui, cx, st, &b, CARD_W - 20.0);
    let info_h = guide.as_ref().map_or(0.0, |(.., h)| *h);
    let h = pad_top + card_content_h(&b) + info_h + 10.0 + POINTER_R;
    let node = cx.geo.node(&b);
    let faded = st.drag.is_some_and(|d| d.slot == slot);
    let style = CardStyle { bg: CARD_BG, edge: rgb(0x2F3842), bubble: col };
    floating_card(ui, ("cqw-card", slot), &PALETTE, &style, node, vec2(CARD_W, h), cx.graph, faded, |ui, body| {
        let inner = Rect::from_min_max(pos2(body.left() + 10.0, body.top() + pad_top), pos2(body.right() - 10.0, body.bottom()));
        card_body(app, ui, cx, st, b, inner);
        if let Some((k, g, _)) = &guide {
            let r = Rect::from_min_max(pos2(inner.left(), inner.top() + card_content_h(&b)), inner.max);
            guide_info::draw_info(ui.painter(), r, *k, g);
        }
    });
}

/// The guide `b` sits in and its block's height at width `w` (the docked card's), if any.
pub(super) fn band_guide(ui: &Ui, cx: &Cx, st: &Editor, b: &BandV, w: f32) -> Option<(usize, Guide, f32)> {
    let k = guide_info::guide_at_freq(cx, st, b.freq)?;
    let g = cx.guides().get(k)?.clone();
    let h = guide_info::info_h(ui.painter(), &g, w);
    Some((k, g, h))
}

/// The card's controls in `inner`, from its top: the head (dot, name, note, close), Freq | Gain
/// (or the slope) | Q, the type and place rows, On | Solo | Sweep | Dyn | X, and the dynamics.
/// The floating card and the slim layout's docked one both draw these.
pub(super) fn card_body(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, b: BandV, inner: Rect) {
    let slot = b.slot;
    let col = band_colour(slot);
    let dyn_on = b.has_gain() && b.dyn_;
    let (head_h, value_h, row_h, sub_h) = (18.0, 34.0, 24.0, 12.0);
    {
        let p = ui.painter().clone();
        let mut yy = inner.top();
        // Head: dot, name, the note it sits on; close.
        let head = Rect::from_min_size(pos2(inner.left(), yy), vec2(inner.width(), head_h));
        let close = Rect::from_center_size(pos2(head.right() - 9.0, head.center().y), vec2(18.0, 16.0));
        let cresp = card_button(ui, close, ("close", slot), &PALETTE, Btn::new(""), Sense::click());
        cross(&p, close.center(), 3.5, ink_of(&PALETTE, &cresp, false));
        if cresp.clicked() {
            select(st, None);
        }
        let dot = pos2(head.left() + 6.0, head.center().y);
        p.circle_filled(dot, 8.0, al(col, 0.45));
        p.circle_filled(dot, 5.0, col);
        let name = format!("Band {}", slot + 1);
        let nw = p.layout_no_wrap(name.clone(), bold(12.0), TEXT_BRIGHT).size().x + 6.0;
        p.text(pos2(head.left() + 18.0, head.center().y), Align2::LEFT_CENTER, name, bold(12.0), TEXT_BRIGHT);
        p.text(pos2(head.left() + 18.0 + nw, head.center().y), Align2::LEFT_CENTER, note_name(b.freq), regular(11.0), TEXT_FAINT);
        yy += head_h + 6.0;

        // Freq | Gain (or slope) | Q
        let thirds = |y: f32| -> [Rect; 3] {
            let w = (inner.width() / 3.0).floor();
            [0, 1, 2].map(|i| {
                let r = Rect::from_min_size(pos2(inner.left() + w * i as f32, y), vec2(if i == 2 { inner.width() - 2.0 * w } else { w }, value_h));
                r.shrink2(vec2(2.0, 0.0))
            })
        };
        let row = thirds(yy);
        let o = field(ui, st, row[0], Field::Freq, b.freq);
        apply_band_field(app, cx, slot, Field::Freq, o);
        if b.has_gain() {
            let o = field(ui, st, row[1], Field::Gain, b.gain);
            apply_band_field(app, cx, slot, Field::Gain, o);
        } else {
            slope_box(app, ui, cx, slot, b.slope, row[1].shrink2(vec2(0.0, 6.0)));
        }
        let o = field(ui, st, row[2], Field::Q, b.q);
        apply_band_field(app, cx, slot, Field::Q, o);
        yy += value_h + 8.0;

        // Type row, place row.
        let tw = inner.width() / 7.0;
        for k in 0..7 {
            let r = Rect::from_min_size(pos2(inner.left() + tw * k as f32, yy), vec2(tw, row_h)).shrink2(vec2(1.0, 0.0));
            if glyph_button(ui, r, ("type", slot, k), false, k, b.kind == k).clicked() {
                let mut list = vec![(bp(slot, cq::F_TYPE), k as f32)];
                if k > q::TYPE_HIGH_SHELF {
                    list.push((bp(slot, cq::F_DYN), 0.0));
                }
                params(app, cx, &list);
            }
        }
        yy += row_h + 6.0;
        let pw = inner.width() / 5.0;
        for k in 0..5 {
            let r = Rect::from_min_size(pos2(inner.left() + pw * k as f32, yy), vec2(pw, row_h)).shrink2(vec2(1.0, 0.0));
            if glyph_button(ui, r, ("place", slot, k), true, k, b.place == k).clicked() {
                params(app, cx, &[(bp(slot, cq::F_PLACE), k as f32)]);
            }
        }
        yy += row_h + 6.0;

        // On | Solo | Sweep | Dyn | X -- only the ones that apply.
        let soloed = pv(cx.ins(), "solo").round() as usize == slot + 1;
        let sweeping = st.sweeps.get(slot).is_some_and(|s| s.active);
        let mut items: Vec<u8> = vec![0, 1];
        if b.has_gain() {
            items.push(2);
            items.push(3);
        }
        items.push(4);
        let bw = inner.width() / items.len() as f32;
        for (i, it) in items.iter().enumerate() {
            let r = Rect::from_min_size(pos2(inner.left() + bw * i as f32, yy), vec2(bw, row_h)).shrink2(vec2(2.0, 0.0));
            match it {
                0 => {
                    if card_button(ui, r, ("on", slot), &PALETTE, Btn::new(if b.active() { "On" } else { "Off" }).on(b.active()), Sense::click())
                        .clicked()
                    {
                        params(app, cx, &[(bp(slot, cq::F_ON), if b.active() { q::ON_BYPASSED as f32 } else { q::ON_ACTIVE as f32 })]);
                    }
                }
                1 => {
                    if card_button(ui, r, ("solo", slot), &PALETTE, Btn::new("Solo").on(soloed), Sense::click()).clicked() {
                        toggle_solo(app, cx, slot);
                    }
                }
                2 => {
                    if card_button(ui, r, ("sweep", slot), &PALETTE, Btn::new(if sweeping { "Cut it" } else { "Sweep" }).on(sweeping), Sense::click())
                        .clicked()
                    {
                        toggle_sweep(app, cx, st, slot);
                    }
                }
                3 => {
                    if card_button(ui, r, ("dyn", slot), &PALETTE, Btn::new("Dyn").on(b.dyn_), Sense::click()).clicked() {
                        toggle_dyn(app, cx, slot);
                    }
                }
                _ => {
                    let resp = card_button(ui, r, ("remove", slot), &PALETTE, Btn::new("").danger(), Sense::click());
                    cross(&p, r.center(), 3.5, if resp.hovered() { rgb(0xFFB4B4) } else { TEXT_DIM });
                    if resp.clicked() {
                        remove_band(app, cx, st, slot);
                    }
                }
            }
        }
        yy += row_h;

        if dyn_on {
            yy += 6.0;
            p.text(pos2(inner.left(), yy + sub_h * 0.5), Align2::LEFT_CENTER, "DYNAMICS", regular(9.0), TEXT_FAINT);
            yy += sub_h + 4.0;
            let row = thirds(yy);
            let thr = f64::from(pv(cx.ins(), bp(slot, cq::F_THRESHOLD)));
            let rng = f64::from(pv(cx.ins(), bp(slot, cq::F_RANGE)));
            let att = q::knob_to_attack_ms(pv(cx.ins(), "dynAttack"));
            let rel = q::knob_to_release_ms(pv(cx.ins(), "dynRelease"));
            let o = field(ui, st, row[0], Field::Thr, thr);
            apply_band_field(app, cx, slot, Field::Thr, o);
            let o = field(ui, st, row[1], Field::Range, rng);
            apply_band_field(app, cx, slot, Field::Range, o);
            let o = field(ui, st, row[2], Field::Att, att);
            apply_band_field(app, cx, slot, Field::Att, o);
            yy += value_h + 6.0;
            let row2 = thirds(yy);
            let o = field(ui, st, row2[0], Field::Rel, rel);
            apply_band_field(app, cx, slot, Field::Rel, o);
        }
    }
}

/// One of the editor's value boxes (`NumField`).
pub(super) fn field(ui: &mut Ui, st: &mut Editor, r: Rect, f: Field, value: f64) -> ValueOut {
    value_box(ui, &mut st.fields, r, f, &PALETTE, &spec(f), value, false)
}

/// A card field's value into its parameter (`card.onFreq` and friends). A drag is one undo step;
/// a typed value or a wheel step is one each.
fn apply_band_field(app: &mut SoundApp, cx: &Cx, slot: usize, f: Field, o: ValueOut) {
    if o.started {
        end_gesture(app);
    }
    if let Some(v) = o.changed {
        let (id, knob) = match f {
            Field::Freq => (bp(slot, cq::F_FREQ), q::freq_to_knob(v)),
            Field::Gain => (bp(slot, cq::F_GAIN), q::gain_to_knob(v)),
            Field::Q => (bp(slot, cq::F_Q), q::q_to_knob(v)),
            Field::Thr => (bp(slot, cq::F_THRESHOLD), v as f32),
            Field::Range => (bp(slot, cq::F_RANGE), v as f32),
            Field::Att => ("dynAttack", q::attack_to_knob(v)),
            Field::Rel => ("dynRelease", q::release_to_knob(v)),
            _ => return,
        };
        let knob = if matches!(f, Field::Thr | Field::Range) { knob } else { knob.clamp(0.0, 1.0) };
        params_merged(app, cx, &[(id, knob)], &format!("field-{f:?}-{slot}"));
    }
    if o.ended {
        end_gesture(app);
    }
}

/// The slope ComboBox the cuts show in place of the gain field.
fn slope_box(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, slot: usize, slope: i32, r: Rect) {
    let resp = ui.interact(r, ui.id().with(("cqw-slope", slot)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Slope"));
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(3), PALETTE.field);
    p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, BORDER), StrokeKind::Inside);
    p.text(pos2(r.left() + 6.0, r.center().y), Align2::LEFT_CENTER, format!("{}", q::slope_db_per_oct(slope)), regular(12.0), TEXT_BRIGHT);
    tri_down(p, pos2(r.right() - 9.0, r.center().y), TEXT_DIM);
    egui::Popup::menu(&resp).show(|ui| {
        for i in 0..q::NUM_SLOPES {
            if ui.add(egui::Button::selectable(i == slope, format!("{}", q::slope_db_per_oct(i)))).clicked() {
                params(app, cx, &[(bp(slot, cq::F_SLOPE), i as f32)]);
                ui.close();
            }
        }
    });
}

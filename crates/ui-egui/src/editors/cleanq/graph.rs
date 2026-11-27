//! `CleanQGraph.h`: the display (`paint` and its parts, in the C++ order) and its mouse.

use super::*;
use crate::editor_kit::icons::{dashed_closed, fill_strip, radial_bg};
use crate::editor_kit::{consume_wheel, juce_wheel_delta_y};
use egui::{CursorIcon, PointerButton};
use soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling as cl;

/// The radial lift towards the bottom centre: 1A2530 at (cx, bottom + 0.2 h), 12161B at 0.45,
/// 0D1013 at the top centre.
fn background(p: &egui::Painter, r: Rect) {
    let centre = pos2(r.center().x, r.bottom() + r.height() * 0.2);
    radial_bg(p, r, centre, r.height() * 1.2, &[(0.0, rgb(0x1A2530)), (0.45, rgb(0x12161B)), (1.0, rgb(0x0D1013))]);
}

/// `spectrumNorm`: the loudest bin in pixel column `px` of `w`.
fn spectrum_norm(bins: &[f32], px: f32, w: f32, sr: f64) -> f32 {
    if bins.len() < 2 || w <= 0.0 {
        return 0.0;
    }
    let bin_hz = sr / q::FFT_SIZE as f64;
    let f0 = norm_freq(f64::from(px / w));
    let f1 = norm_freq(f64::from((px + 1.0) / w));
    let n = bins.len() - 1;
    let k0 = ((f0 / bin_hz).floor() as usize).clamp(1, n);
    let k1 = ((f1 / bin_hz).ceil() as usize).clamp(k0, n);
    bins.get(k0..=k1).map_or(0.0, |s| s.iter().copied().fold(0.0, f32::max))
}

fn spectrum_y(cx: &Cx, bins: &[f32], px: f32) -> f32 {
    let pl = cx.geo.plot;
    pl.bottom() - spectrum_norm(bins, px, pl.width(), cx.sr()) * pl.height()
}

/// The per-band curves and their sum (`paintCurves`). Each band's curve is cached by its own
/// settings, its live dynamic offset (`gainNow`), the width and the rate, so a moving dynamic
/// band recomputes only itself; the sum is re-added from the cached curves (no allocation once
/// the buffers have grown).
fn curves(cx: &Cx, st: &mut Editor) {
    use std::hash::{Hash, Hasher};
    let w = cx.geo.plot.width().max(2.0) as usize;
    let steps = w.min(600);
    let c = &mut st.curves;
    c.per.resize_with(q::NUM_BANDS, BandCurve::default);
    for (b, bc) in cx.bands.iter().zip(c.per.iter_mut()) {
        bc.used = b.used();
        if !bc.used {
            continue;
        }
        let mut h = std::hash::DefaultHasher::new();
        (b.kind, b.slope).hash(&mut h);
        b.freq.to_bits().hash(&mut h);
        b.gain_now().to_bits().hash(&mut h);
        b.q.to_bits().hash(&mut h);
        (steps, cx.sr().to_bits()).hash(&mut h);
        let key = h.finish();
        if bc.key == key && bc.db.len() == steps + 1 {
            continue;
        }
        bc.key = key;
        bc.db.clear();
        let d = q::design_filter(b.kind, q::slope_to_order(b.slope), b.freq, b.gain_now(), b.q, cx.sr());
        for i in 0..=steps {
            let f = norm_freq(i as f64 / steps as f64);
            let wr = 2.0 * std::f64::consts::PI * f.min(cx.sr() * 0.499) / cx.sr();
            let db = q::magnitude_db(&d, wr);
            bc.db.push(if db.is_finite() { db } else { 0.0 });
        }
    }
    c.total.clear();
    c.total.resize(steps + 1, 0.0);
    for (b, bc) in cx.bands.iter().zip(c.per.iter()) {
        if bc.used && b.active() {
            for (t, v) in c.total.iter_mut().zip(&bc.db) {
                *t += v;
            }
        }
    }
}

pub(super) fn paint_graph(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor) {
    let _ = app;
    // The capture is read while the rest of the state is written (the curve cache, the chips).
    let trace = std::mem::take(&mut st.trace);
    paint_graph_with(ui, cx, st, &trace);
    st.trace = trace;
}

fn paint_graph_with(ui: &mut Ui, cx: &Cx, st: &mut Editor, trace: &[f64]) {
    let g = cx.graph;
    let geo = cx.geo;
    let pl = geo.plot;
    let p = ui.painter_at(g.intersect(ui.clip_rect()));
    background(&p, g);

    // ---- paintGrid: 1-2-5 per decade, labelled at the decades and the 2s and 5s
    for decade in 1..=4 {
        let base = 10f64.powi(decade);
        for mult in 1..=9 {
            let f = base * f64::from(mult);
            if !(FREQ_MIN..=FREQ_MAX).contains(&f) {
                continue;
            }
            let x = geo.fx(f).round();
            let major = mult == 1;
            p.line_segment(
                [pos2(x + 0.5, pl.top()), pos2(x + 0.5, pl.bottom())],
                Stroke::new(1.0, al(Color32::WHITE, if major { 0.10 } else { 0.04 })),
            );
            let room = x > pl.left() + 16.0 && x < pl.right() - 16.0;
            if room && (major || mult == 2 || mult == 5) {
                let label = if f >= 1000.0 {
                    if f >= 10_000.0 { format!("{:.0}k", f / 1000.0) } else { format!("{:.1}k", f / 1000.0) }
                } else {
                    format!("{}", f.round() as i64)
                };
                p.text(
                    pos2(x, pl.bottom() + 1.0 + (SCALE_H - 2.0) * 0.5),
                    Align2::CENTER_CENTER,
                    label,
                    regular(9.0),
                    al(TEXT_FAINT, if major { 0.9 } else { 0.55 }),
                );
            }
        }
    }
    let step = db_step(st.range);
    let mut db = -st.range;
    while db <= st.range {
        let y = geo.gy(f64::from(db)).round();
        let zero = db == 0;
        p.line_segment([pos2(pl.left(), y + 0.5), pos2(pl.right(), y + 0.5)], Stroke::new(1.0, al(Color32::WHITE, if zero { 0.18 } else { 0.06 })));
        if !zero {
            p.text(
                pos2(pl.left() + 3.0, y),
                Align2::LEFT_CENTER,
                format!("{}{db}", if db > 0 { "+" } else { "" }),
                regular(9.0),
                al(TEXT_FAINT, 0.7),
            );
        }
        db += step;
    }

    // ---- paintSpectrum
    if st.analyser && !st.held_post.is_empty() {
        let w = pl.width().max(1.0) as usize;
        let path = |bins: &[f32]| -> Vec<Pos2> { (0..=w).map(|px| pos2(pl.left() + px as f32, spectrum_y(cx, bins, px as f32))).collect() };
        if st.pre && !st.held_pre.is_empty() {
            fill_strip(&p, &path(&st.held_pre), |_| pl.bottom(), al(Color32::WHITE, 0.055));
        }
        if st.post {
            let post = path(&st.held_post);
            fill_strip(&p, &post, |_| pl.bottom(), al(ACCENT, 0.13));
            p.add(Shape::line(post, Stroke::new(1.0, al(ACCENT, 0.30))));
        }
    }

    // ---- paintCeilingLines, behind the curves
    let overlay = st.ceiling_open || cx.ceil.pinned;
    let pc = ui.painter_at(pl.intersect(ui.clip_rect()));
    if overlay {
        for (i, n) in cl::NOISES.iter().enumerate() {
            if !cx.ceil.noise_on[i] {
                continue;
            }
            let level = noise_level(cx, st, i, trace);
            let a = pos2(pl.left(), geo.sy(cl::noise_line_db(n.slope_db_per_oct, level, FREQ_MIN)));
            let b = pos2(pl.right(), geo.sy(cl::noise_line_db(n.slope_db_per_oct, level, FREQ_MAX)));
            dashed_closed(&pc, &[a, b], 6.0, 4.0, Stroke::new(1.2, al(noise_colour(n.argb), 0.55)));
        }
        let shape = &cx.ceil.shape;
        if shape.is_valid() {
            let (lo, hi) = (shape.lo_hz(), shape.hi_hz());
            let steps = (((geo.fx(hi) - geo.fx(lo)) / 3.0).round() as usize).clamp(2, 2000);
            let line: Vec<Pos2> = (0..=steps)
                .map(|k| {
                    let hz = lo * (hi / lo).powf(k as f64 / steps as f64);
                    pos2(geo.fx(hz), geo.sy(shape.at(hz)))
                })
                .collect();
            fill_strip(&pc, &line, |_| pl.top(), al(WARN, 0.03));
            pc.add(Shape::line(line, Stroke::new(2.0, al(WARN, 0.9))));
            for i in 0..shape.pts.len().saturating_sub(1) {
                let (a, b) = (shape.pts[i], shape.pts[i + 1]);
                if geo.fx(b.hz) - geo.fx(a.hz) < 60.0 {
                    continue;
                }
                let mid = (a.hz * b.hz).sqrt();
                let s = shape.slope(i);
                pc.text(
                    pos2(geo.fx(mid), geo.sy(shape.at(mid)) - 11.0),
                    Align2::CENTER_CENTER,
                    format!("{}{s:.1} dB/oct", if s > 0.05 { "+" } else { "" }),
                    regular(9.5),
                    al(WARN, 0.85),
                );
            }
        }
        if trace.len() == cl::GRID_N {
            let eq = eq_on_grid(cx, true);
            if eq.iter().any(|v| v.abs() > 0.05) {
                let after: Vec<f64> = trace.iter().zip(&eq).map(|(t, e)| if *t > -110.0 { t + e } else { *t }).collect();
                grid_line(&pc, &geo, &after, Stroke::new(1.6, al(rgb(0x7FB4FF), 0.85)));
            }
            grid_line(&pc, &geo, trace, Stroke::new(1.6, al(rgb(0xF3F6F8), if st.hold { 0.95 } else { 0.75 })));
        }
        if st.ceiling_open {
            let hint = if !st.analyser {
                "Turn the analyser on (RTA) to capture"
            } else if trace.is_empty() {
                "Play something \u{2014} the capture builds up as it plays. Hold freezes it."
            } else {
                ""
            };
            if !hint.is_empty() {
                pc.text(pos2(pl.center().x, pl.top() + pl.height() / 3.0 + 10.0), Align2::CENTER_CENTER, hint, regular(11.0), TEXT_DIM);
            }
        }
    }

    // ---- paintGuideRegions (behind the curves)
    if st.guides_on {
        for (k, m) in cx.guides().iter().enumerate() {
            let col = guide_colour(k);
            let fade = if m.done { 0.3 } else { 1.0 };
            let (x0, x1) = (geo.fx(m.lo_hz), geo.fx(m.hi_hz));
            let span = Rect::from_min_size(pos2(x0, pl.top()), vec2((x1 - x0).max(2.0), pl.height()));
            p.rect_filled(span, CornerRadius::ZERO, al(col, 0.07 * fade));
            p.line_segment([pos2(x0.round(), span.top()), pos2(x0.round(), span.bottom())], Stroke::new(1.0, al(col, 0.20 * fade)));
            p.line_segment([pos2(x1.round(), span.top()), pos2(x1.round(), span.bottom())], Stroke::new(1.0, al(col, 0.20 * fade)));
            let xc = geo.fx(m.freq_hz);
            dashed_closed(&p, &[pos2(xc, span.top()), pos2(xc, span.bottom())], 3.0, 4.0, Stroke::new(1.0, al(col, 0.45 * fade)));
        }
    }

    // ---- Scout's guides for this track, behind the curves as the editor's own are
    let mut scout = std::mem::take(&mut st.scout);
    scout_guides(cx, &mut scout);
    for (_, span, col, _) in &scout {
        let span = Rect::from_x_y_ranges(span.x_range(), pl.y_range());
        p.rect_filled(span, CornerRadius::ZERO, al(*col, 0.07));
        for x in [span.left().round(), span.right().round()] {
            p.line_segment([pos2(x, span.top()), pos2(x, span.bottom())], Stroke::new(1.0, al(*col, 0.20)));
        }
        let xc = (span.left() + span.right()) * 0.5;
        dashed_closed(&p, &[pos2(xc, span.top()), pos2(xc, span.bottom())], 3.0, 4.0, Stroke::new(1.0, al(*col, 0.45)));
    }

    // ---- paintDynCut: the dynamic ceiling's cut right now, magenta, from 0 dB down
    paint_dyn_cut(&pc, cx);

    // ---- paintCurves: one thin one per band, then the sum
    curves(cx, st);
    let total = &st.curves.total;
    let steps = total.len().saturating_sub(1).max(1);
    let xs = |i: usize| pl.left() + i as f32 / steps as f32 * pl.width();
    let pg = ui.painter_at(g.intersect(ui.clip_rect()));
    for (slot, bc) in st.curves.per.iter().enumerate().filter(|(_, bc)| bc.used) {
        let v = &bc.db;
        let Some(b) = cx.band(slot) else { continue };
        let sel = st.selected == Some(slot);
        let mut col = band_colour(slot);
        if !b.active() {
            col = al(col, 0.25);
        }
        let pts: Vec<Pos2> = v.iter().enumerate().map(|(i, db)| pos2(xs(i), geo.gy(*db))).collect();
        pg.add(Shape::line(pts, Stroke::new(if sel { 1.6 } else { 1.1 }, al(col, if sel { 0.95 } else { 0.5 }))));
    }
    let sum: Vec<Pos2> = total.iter().enumerate().map(|(i, db)| pos2(xs(i), geo.gy(*db))).collect();
    let zero = geo.gy(0.0);
    fill_strip(&pg, &sum, |_| zero, al(Color32::WHITE, 0.07));
    pg.add(Shape::line(sum, Stroke::new(2.0, place_colour(q::PLACE_STEREO))));

    // ---- paintNodes
    for b in cx.bands.iter().filter(|b| b.used()) {
        let at = geo.node(b);
        let col = band_colour(b.slot);
        let sel = st.selected == Some(b.slot);
        let r = 8.0;
        let alpha = if b.active() { 1.0 } else { 0.4 };
        if b.dyn_ && b.has_gain() {
            // dynamic: dashed ring at the node, plus a ghost at the offset it is applying
            st.ring.clear();
            st.ring.extend((0..=48).map(|i| i as f32 / 48.0 * std::f32::consts::TAU).map(|a| at + vec2(a.cos(), a.sin()) * 13.0));
            dashed_closed(&pg, &st.ring, 3.0, 3.0, Stroke::new(1.0, al(col, 0.8 * alpha)));
            if b.dyn_off.abs() > 0.05 {
                let gy = geo.gy(b.gain + b.dyn_off);
                pg.line_segment([at, pos2(at.x, gy)], Stroke::new(1.0, al(col, 0.55 * alpha)));
                pg.circle_filled(pos2(at.x, gy), 3.5, al(col, 0.55 * alpha));
            }
        }
        if sel {
            pg.circle_filled(at, r + 4.0, al(Color32::WHITE, 0.18));
        }
        for i in (1..=3).rev() {
            pg.circle_filled(at, r + i as f32 * 2.0, al(col, 0.10 * alpha / i as f32));
        }
        let square = b.place == q::PLACE_MID || b.place == q::PLACE_SIDE;
        let bx = Rect::from_center_size(at, vec2(r * 2.0, r * 2.0));
        let ring_col = if st.soloing.is_some_and(|(s, _)| s == b.slot) { WARN } else { al(Color32::WHITE, 0.85) };
        if square {
            pg.rect_filled(bx, CornerRadius::same(4), al(col, alpha));
            pg.rect_stroke(bx.shrink(1.0), CornerRadius::same(4), Stroke::new(2.0, al(ring_col, alpha)), StrokeKind::Middle);
        } else {
            pg.circle_filled(at, r, al(col, alpha));
            pg.circle_stroke(at, r - 1.0, Stroke::new(2.0, al(ring_col, alpha)));
        }
        if b.place != q::PLACE_STEREO {
            pg.circle_filled(at, 2.5, al(place_colour(b.place), alpha));
        }
    }

    // ---- paintGuideChips, in front: the part you click
    st.chips.clear();
    if st.guides_on {
        let row_ends = &mut st.row_ends;
        row_ends.clear();
        // A label is at most a third of the graph (and 130 px) wide, cut with an ellipsis; chips
        // stagger over CHIP_ROWS rows; one with no room left shows its number alone, or nothing
        // (its range is still drawn). Hovering a chip shows the whole guide.
        let max_w = (pl.width() / 3.0).min(130.0);
        let h = 15.0;
        for (k, m) in cx.guides().iter().enumerate() {
            let col = guide_colour(k);
            let fade = if m.done { 0.42 } else { 1.0 };
            let label = if m.label().is_empty() { guide_info::short_freq(m.freq_hz) } else { m.label().to_string() };
            let tick_w = if m.done { 11.0 } else { 0.0 };
            let full = fit_label(&pg, &format!("{}  {label}", k + 1), max_w - 14.0 - tick_w);
            let x0 = geo.fx(m.lo_hz) + 2.0;
            let mut placed = None;
            for text in [full, format!("{}", k + 1)] {
                let tw = pg.layout_no_wrap(text.clone(), regular(10.0), col).size().x;
                let w = tw + 14.0 + tick_w;
                let x = x0.clamp(pl.left() + 2.0, (pl.right() - w - 4.0).max(pl.left() + 2.0));
                if let Some(row) = (0..CHIP_ROWS).find(|r| row_ends.get(*r).is_none_or(|end| x >= *end)) {
                    placed = Some((text, x, w, row));
                    break;
                }
            }
            let Some((text, x, w, row)) = placed else {
                st.chips.push(Rect::NOTHING);
                continue;
            };
            if row_ends.len() <= row {
                row_ends.resize(row + 1, f32::NEG_INFINITY);
            }
            row_ends[row] = x + w + 6.0;
            let y = pl.top() + 30.0 + row as f32 * (h + 3.0);
            let chip = Rect::from_min_size(pos2(x, y), vec2(w, h));
            st.chips.push(chip);
            pg.rect_filled(chip, CornerRadius::same(3), al(BACKGROUND, 0.88 * if m.done { 0.7 } else { 1.0 }));
            pg.rect_stroke(chip.shrink(0.5), CornerRadius::same(3), Stroke::new(1.0, al(col, 0.75 * fade)), StrokeKind::Middle);
            let ink = al(col, fade);
            let inner = chip.shrink2(vec2(6.0, 0.0));
            if m.done {
                let bx = Rect::from_center_size(pos2(inner.right() - tick_w * 0.5, inner.center().y), vec2(8.0, 8.0));
                pg.add(Shape::line(
                    vec![pos2(bx.left(), bx.center().y), pos2(bx.center().x - 1.0, bx.bottom()), pos2(bx.right(), bx.top())],
                    Stroke::new(1.6, ink),
                ));
            }
            pg.text(pos2(inner.left(), inner.center().y), Align2::LEFT_CENTER, text, regular(10.0), ink);
        }
        // The whole guide on hover: its chip, or its range in the chips' rows (where a click takes it).
        if st.drag.is_none()
            && st.guide_edit.is_none()
            && let Some(at) = ui.input(|i| i.pointer.hover_pos())
            && ui.rect_contains_pointer(pl)
            && let Some(k) = guide_at(cx, st, at)
            && let Some(m) = cx.guides().get(k)
        {
            let anchor = st.chips.get(k).copied().filter(|c| c.is_positive()).unwrap_or(Rect::from_center_size(at, vec2(1.0, 1.0)));
            egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), ui.id().with(("cq-guide-tip", k)), anchor)
                .show(|ui| guide_info::tooltip(ui, k, m));
        }
    }

    // ---- Scout's chips: the same chip, a "SCOUT" tag in front, read-only; in their own rows
    // under the editor's guides so neither hides the other.
    st.scout_chips.clear();
    let own_rows =
        st.chips.iter().filter(|c| c.is_positive()).map(|c| ((c.top() - (pl.top() + 30.0)) / 18.0).round() as usize + 1).max().unwrap_or(0);
    let row_ends = &mut st.row_ends;
    row_ends.clear();
    for (_, span, col, text) in &scout {
        let tag_w = pg.layout_no_wrap("SCOUT".into(), regular(8.0), *col).size().x + 8.0;
        let tw = pg.layout_no_wrap(text.clone(), regular(10.0), *col).size().x;
        let (w, h) = (tag_w + 4.0 + tw + 12.0, 15.0);
        let x = (span.left() + 2.0).clamp(pl.left(), (pl.right() - w).max(pl.left()));
        let row = row_ends.iter().position(|end| x >= *end).unwrap_or(row_ends.len());
        if row == row_ends.len() {
            row_ends.push(0.0);
        }
        if let Some(end) = row_ends.get_mut(row) {
            *end = x + w + 6.0;
        }
        let y = pl.top() + 30.0 + (own_rows + row) as f32 * (h + 3.0);
        let chip = Rect::from_min_size(pos2(x, y), vec2(w, h));
        st.scout_chips.push(chip);
        pg.rect_filled(chip, CornerRadius::same(3), al(BACKGROUND, 0.88));
        dashed_closed(
            &pg,
            &[chip.left_top(), chip.right_top(), chip.right_bottom(), chip.left_bottom(), chip.left_top()],
            3.0,
            2.0,
            Stroke::new(1.0, al(*col, 0.75)),
        );
        let tag = Rect::from_min_size(chip.min + vec2(2.0, 2.0), vec2(tag_w, h - 4.0));
        pg.rect_filled(tag, CornerRadius::same(2), *col);
        pg.text(tag.center(), Align2::CENTER_CENTER, "SCOUT", regular(8.0), BACKGROUND);
        pg.text(pos2(tag.right() + 4.0, chip.center().y), Align2::LEFT_CENTER, text, regular(10.0), *col);
    }

    st.scout = scout;

    // ---- paintCeilingHandles: noise labels, and the points while the panel is open
    if overlay {
        for (i, n) in cl::NOISES.iter().enumerate() {
            if !cx.ceil.noise_on[i] {
                continue;
            }
            let chip = noise_chip(&pg, cx, st, i, trace);
            pg.rect_filled(chip, CornerRadius::same(3), al(BACKGROUND, 0.9));
            pg.rect_stroke(chip.shrink(0.5), CornerRadius::same(3), Stroke::new(1.0, al(noise_colour(n.argb), 0.8)), StrokeKind::Middle);
            pg.text(chip.center(), Align2::CENTER_CENTER, noise_name(i), regular(9.5), al(noise_colour(n.argb), 0.8));
        }
        if st.ceiling_open {
            for (i, pt) in cx.ceil.shape.pts.iter().enumerate() {
                let at = pos2(geo.fx(pt.hz), geo.sy(pt.db));
                let dragging = st.ceil_drag.as_ref().is_some_and(|(d, ..)| *d == i);
                let on = dragging || st.ceiling_node == Some(i);
                pg.circle_filled(at, 5.5, BACKGROUND);
                pg.circle_stroke(at, 5.0, Stroke::new(if on { 2.5 } else { 1.8 }, WARN));
                if dragging {
                    pg.text(at + vec2(0.0, 15.0), Align2::CENTER_CENTER, format!("{}  {:.1} dB", format_freq(pt.hz), pt.db), regular(9.5), WARN);
                }
            }
        }
    }
}

/// `paintDynCut`: the area between 0 dB and the cut on the ceiling grid, 28 % magenta, and its
/// edge at 85 %, clipped to the plot. Only while the dynamic ceiling is on (`refreshDynamic`
/// hands the graph an empty cut otherwise) and the plugin is playing.
fn paint_dyn_cut(p: &egui::Painter, cx: &Cx) {
    let Some(live) = cx.live else { return };
    if pv(cx.ins(), "ceilOn") <= 0.5 || cx.geo.plot.width() <= 1.0 {
        return;
    }
    let geo = cx.geo;
    let zero = geo.gy(0.0);
    let edge: Vec<Pos2> = (0..cl::GRID_N)
        .map(|i| {
            let v = live.get(cq::LIVE_CUT.start + i);
            pos2(geo.fx(cl::grid_hz(i)), geo.gy(if v.is_finite() { f64::from(v) } else { 0.0 }))
        })
        .collect();
    fill_strip(p, &edge, |_| zero, al(CUT_COLOUR, 0.28));
    p.add(Shape::line(edge, Stroke::new(1.4, al(CUT_COLOUR, 0.85))));
}

/// Scout's EQ guides for this track (the shown answer's), placed on this editor's frequency
/// scale: `(index, lo..hi column, Scout's badge colour, chip text)`, into `out`. Visual only:
/// they never become bands.
fn scout_guides(cx: &Cx, out: &mut Vec<(usize, Rect, Color32, String)>) {
    out.clear();
    let session = &cx.host.session;
    let guides = crate::scout::guides::shown(session);
    if guides.is_empty() {
        return;
    }
    let Some(name) = session.track(cx.host.track).map(|t| t.name.as_str()) else { return };
    let geo = cx.geo;
    out.extend(
        crate::scout::guides::guide_rects(guides, geo.plot, |_, f| geo.fx(f64::from(f)))
            .into_iter()
            .filter(|(i, ..)| matches!(guides.get(*i), Some(soundcraft_model::Guide::Eq { track, .. }) if track == name)),
    );
}

fn noise_colour(argb_v: u32) -> Color32 {
    rgb(argb_v & 0x00FF_FFFF)
}
fn noise_name(i: usize) -> String {
    let n = &cl::NOISES[i];
    format!("{} {}{}", n.name, if n.slope_db_per_oct > 0.0 { "+" } else { "" }, n.slope_db_per_oct as i32)
}
/// A guide nobody has dragged sits on the capture (`fitNoiseLevel`).
pub(super) fn noise_level(cx: &Cx, st: &Editor, i: usize, trace: &[f64]) -> f64 {
    if let Some(l) = st.noise_levels {
        return l[i];
    }
    if !cx.ceil.noise_manual[i] && trace.len() == cl::GRID_N {
        cl::fit_noise_level(trace, cl::NOISES[i].slope_db_per_oct, 40.0, 12000.0, -100.0)
    } else {
        cx.ceil.noise_level[i]
    }
}
fn noise_chip_x(cx: &Cx, i: usize) -> f32 {
    cx.geo.plot.left() + cx.geo.plot.width() * (0.22 + 0.075 * i as f32)
}
fn noise_chip(p: &egui::Painter, cx: &Cx, st: &Editor, i: usize, trace: &[f64]) -> Rect {
    let x = noise_chip_x(cx, i);
    let n = &cl::NOISES[i];
    let y = cx.geo.sy(cl::noise_line_db(n.slope_db_per_oct, noise_level(cx, st, i, trace), cx.geo.xf(x)));
    let w = p.layout_no_wrap(noise_name(i), regular(9.5), TEXT_DIM).size().x + 12.0;
    Rect::from_min_size(pos2(x - w * 0.5, y - 7.5), vec2(w, 15.0))
}
fn grid_line(p: &egui::Painter, geo: &Geo, db: &[f64], stroke: Stroke) {
    let mut run: Vec<Pos2> = Vec::new();
    for (i, v) in db.iter().enumerate() {
        if *v <= -110.0 {
            if run.len() > 1 {
                p.add(Shape::line(std::mem::take(&mut run), stroke));
            }
            run.clear();
            continue;
        }
        run.push(pos2(geo.fx(cl::grid_hz(i)), geo.sy(*v)));
    }
    if run.len() > 1 {
        p.add(Shape::line(run, stroke));
    }
}

/// `eqOnGrid`: the static response of the active bands (not Side) on the ceiling grid.
pub(super) fn eq_on_grid(cx: &Cx, include_generated: bool) -> Vec<f64> {
    let mut out = vec![0.0; cl::GRID_N];
    for b in cx.bands.iter().filter(|b| b.active()) {
        if !include_generated && cx.ceil.generated.contains(&(b.slot as i32)) {
            continue;
        }
        if b.place == q::PLACE_SIDE {
            continue;
        }
        let d = q::design_filter(b.kind, q::slope_to_order(b.slope), b.freq, b.gain, b.q, cx.sr());
        for (i, o) in out.iter_mut().enumerate() {
            *o += q::magnitude_db(&d, 2.0 * std::f64::consts::PI * cl::grid_hz(i).min(cx.sr() * 0.499) / cx.sr());
        }
    }
    out
}

// ============================================================================ mouse
/// The most rows the guide chips stagger over (the rows a click in a guide's range reaches).
const CHIP_ROWS: usize = 3;

/// `text` cut to `max_w` (at 10 pt) with an ellipsis.
fn fit_label(p: &egui::Painter, text: &str, max_w: f32) -> String {
    let w = |s: &str| p.layout_no_wrap(s.to_string(), regular(10.0), Color32::WHITE).size().x;
    if w(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 1 {
        n -= 1;
        let s = format!("{}\u{2026}", chars.iter().take(n).collect::<String>().trim_end());
        if w(&s) <= max_w {
            return s;
        }
    }
    chars.first().map(|c| c.to_string()).unwrap_or_default()
}

fn hit_node(cx: &Cx, p: Pos2) -> Option<usize> {
    let mut best = None;
    let mut dist = 13.0;
    for b in cx.bands.iter().filter(|b| b.used()) {
        let d = cx.geo.node(b).distance(p);
        if d < dist {
            dist = d;
            best = Some(b.slot);
        }
    }
    best
}

fn guide_at(cx: &Cx, st: &Editor, p: Pos2) -> Option<usize> {
    if !st.guides_on {
        return None;
    }
    if let Some(k) = st.chips.iter().position(|c| c.contains(p)) {
        return Some(k);
    }
    let pl = cx.geo.plot;
    if p.y > pl.top() + 26.0 && p.y < pl.top() + 86.0 {
        return cx.guides().iter().position(|g| p.x >= cx.geo.fx(g.lo_hz) && p.x <= cx.geo.fx(g.hi_hz));
    }
    None
}

pub(super) fn open_guide_editor(st: &mut Editor, g: Guide, index: Option<usize>) {
    select(st, None);
    st.guides_on = true;
    st.guide_edit = Some(GuideEdit { index, g, focus_note: true });
}

/// `acceptGuide`: a band at the guide (Q from its width; a dynamic guide makes a dynamic band,
/// threshold -24, range = its gain, gain 0) and the guide marked done, as ONE undo step; a done
/// guide clicked again is undone.
fn accept_guide(app: &mut SoundApp, cx: &Cx, st: &mut Editor, k: usize) {
    let mut guides = cx.ceil.guides.clone();
    let Some(mut g) = guides.list.get(k).cloned() else { return };
    if g.done {
        g.done = false;
        guides.replace(k, g);
        write_guides(app, cx, guides);
        return;
    }
    let Some(slot) = cx.first_free() else { return };
    let ratio = (g.hi_hz / g.lo_hz.max(1.0)).max(1.0001);
    let qv = (1.0 / ratio.log2().max(0.05)).clamp(0.025, 40.0);
    let mut list = vec![
        (bp(slot, cq::F_FREQ), q::freq_to_knob(g.freq_hz).clamp(0.0, 1.0)),
        (bp(slot, cq::F_GAIN), q::gain_to_knob(g.gain_db).clamp(0.0, 1.0)),
        (bp(slot, cq::F_Q), q::q_to_knob(qv).clamp(0.0, 1.0)),
        (bp(slot, cq::F_TYPE), q::TYPE_BELL as f32),
        (bp(slot, cq::F_PLACE), q::PLACE_STEREO as f32),
        (bp(slot, cq::F_SLOPE), 1.0),
        (bp(slot, cq::F_DYN), 0.0),
        (bp(slot, cq::F_ON), q::ON_ACTIVE as f32),
    ];
    if g.dynamic {
        list[6].1 = 1.0;
        list[1].1 = q::gain_to_knob(0.0);
        list.push((bp(slot, cq::F_THRESHOLD), -24.0));
        list.push((bp(slot, cq::F_RANGE), g.gain_db.clamp(-30.0, 30.0) as f32));
    }
    g.done = true;
    guides.replace(k, g);
    crate::editor_kit::one_step(app, |app| {
        set_params_merged(app, cx.host, &list, "accept-guide");
        write_guides_merged(app, cx.host, &guides, "accept-guide");
    });
    select(st, Some(slot));
}

/// `onPlaceGuide (hz, db)`: `Guide::around (hz, db < 0 ? db : -3)`.
fn placed_guide(hz: f64, db: f64) -> Guide {
    Guide::around(hz, if db < 0.0 { db } else { -3.0 })
}

pub(super) fn graph_mouse(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor) {
    let geo = cx.geo;
    let pl = geo.plot;
    let resp = ui.interact(cx.graph, ui.id().with("cqw-graph"), Sense::click_and_drag());
    let (pos, mods, pressed_l, pressed_r, down, released) = ui.input(|i| {
        (
            i.pointer.interact_pos(),
            i.modifiers,
            i.pointer.button_pressed(PointerButton::Primary),
            i.pointer.button_pressed(PointerButton::Secondary),
            i.pointer.primary_down(),
            i.pointer.any_released(),
        )
    });
    if st.placing && resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    }
    if st.drag.is_some_and(|d| d.gain_only) {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    let overlay = st.ceiling_open || cx.ceil.pinned;

    let now = ui.input(|i| i.time);
    let mut double = false;
    if resp.hovered()
        && pressed_l
        && let Some(p) = pos
    {
        double = st.last_press.is_some_and(|(t, at)| now - t <= 0.5 && at.distance(p) < 6.0);
        st.last_press = if double { None } else { Some((now, p)) };
    }
    // ---- mouseDown
    if resp.hovered()
        && (pressed_l || pressed_r)
        && let Some(p) = pos
    {
        let popup = pressed_r;
        'down: {
            // Scout's guides are read-only: a press on one does nothing.
            if st.scout_chips.iter().any(|c| c.contains(p)) {
                break 'down;
            }
            if st.placing {
                st.placing = false;
                if !popup && pl.contains(p) {
                    open_guide_editor(st, placed_guide(geo.xf(p.x), geo.yg(p.y).clamp(-30.0, 30.0)), None);
                }
                break 'down;
            }
            // Ceiling points and noise chips, while the panel is open.
            if overlay && st.ceiling_open {
                if let Some(i) = cx.ceil.shape.pts.iter().position(|pt| pos2(geo.fx(pt.hz), geo.sy(pt.db)).distance(p) <= 8.0) {
                    if popup {
                        break 'down;
                    }
                    end_gesture(app);
                    st.ceil_drag = Some((i, cx.ceil.shape.clone(), p));
                    st.ceiling_node = Some(i);
                    break 'down;
                }
                if !popup {
                    let pc = ui.painter().clone();
                    if let Some(i) =
                        (0..cl::NUM_NOISES).filter(|i| cx.ceil.noise_on[*i]).find(|i| noise_chip(&pc, cx, st, *i, &st.trace).expand(2.0).contains(p))
                    {
                        st.noise_drag = Some(i);
                        let mut levels = [0.0; cl::NUM_NOISES];
                        for (k, l) in levels.iter_mut().enumerate() {
                            *l = noise_level(cx, st, k, &st.trace);
                        }
                        st.noise_levels = Some(levels);
                        break 'down;
                    }
                }
            }
            let hit = hit_node(cx, p);
            let guide = guide_at(cx, st, p);
            if popup {
                if let Some(k) = guide {
                    if let Some(g) = cx.guides().get(k).cloned() {
                        open_guide_editor(st, g, Some(k));
                    }
                } else if let Some(h) = hit {
                    remove_band(app, cx, st, h);
                } else {
                    open_guide_editor(st, placed_guide(geo.xf(p.x), geo.yg(p.y).clamp(-30.0, 30.0)), None);
                }
                break 'down;
            }
            if let Some(k) = guide {
                accept_guide(app, cx, st, k);
                break 'down;
            }
            if hit.is_none() && spectrum_grab(app, cx, st, p, mods.alt) {
                break 'down;
            }
            let Some(h) = hit else {
                select(st, None);
                break 'down;
            };
            select(st, Some(h));
            if mods.alt {
                let prev = pv(cx.ins(), "solo");
                st.soloing = Some((h, prev));
                params(app, cx, &[("solo", (h + 1) as f32)]);
                break 'down;
            }
            let b = cx.bands[h];
            end_gesture(app);
            st.drag = Some(NodeDrag { slot: h, start: p, freq: b.freq, gain: b.gain, axis: 0, gain_only: false, has_gain: b.has_gain() });
        }
    }

    // ---- mouseDrag
    if down && let Some(p) = pos {
        if let Some((i, from, start)) = st.ceil_drag.clone() {
            let c = drag_ceiling(&geo, &from, i, p - start, cx.ceil.snap && !mods.shift, mods.alt);
            set_ceiling(app, cx, &c, true);
        } else if let Some(i) = st.noise_drag {
            let f = geo.xf(noise_chip_x(cx, i));
            let db = geo.ys(p.y).clamp(-96.0, 0.0);
            if let Some(l) = st.noise_levels.as_mut() {
                l[i] = db - cl::NOISES[i].slope_db_per_oct * cl::octaves_from_1k(f);
            }
        } else if let Some(mut d) = st.drag {
            let fine = if mods.shift { 0.25 } else { 1.0 };
            let mut delta = (p - d.start) * fine;
            if d.gain_only {
                delta.x = 0.0;
            }
            if mods.ctrl || mods.command {
                if d.axis == 0 && delta.length() > 4.0 {
                    d.axis = if delta.x.abs() > delta.y.abs() { 1 } else { 2 };
                }
                if d.axis == 1 {
                    delta.y = 0.0;
                } else if d.axis == 2 {
                    delta.x = 0.0;
                }
            }
            st.drag = Some(d);
            if delta.length() > 0.0 || d.gain_only {
                let f = geo.xf(geo.fx(d.freq) + delta.x).clamp(FREQ_MIN, 20_000.0);
                let gdb = geo.yg(geo.gy(d.gain) + delta.y).clamp(-30.0, 30.0);
                let gdb = if d.has_gain { gdb } else { 0.0 };
                params_merged(
                    app,
                    cx,
                    &[(bp(d.slot, cq::F_FREQ), q::freq_to_knob(f).clamp(0.0, 1.0)), (bp(d.slot, cq::F_GAIN), q::gain_to_knob(gdb).clamp(0.0, 1.0))],
                    "node",
                );
            }
        }
    }

    // ---- mouseUp
    if released {
        if st.ceil_drag.take().is_some() {
            end_gesture(app);
        }
        if st.noise_drag.take().is_some()
            && let Some(levels) = st.noise_levels.take()
        {
            let mut s = (*cx.ceil).clone();
            for (i, l) in levels.iter().enumerate() {
                if s.noise_on[i] && (s.noise_level[i] - l).abs() > 1e-9 {
                    s.noise_level[i] = *l;
                    s.noise_manual[i] = true;
                }
            }
            write_state(app, cx, &s);
        }
        if st.drag.take().is_some() {
            end_gesture(app);
        }
        if let Some((_, prev)) = st.soloing.take() {
            params(app, cx, &[("solo", prev)]);
        }
    }

    // ---- mouseDoubleClick: JUCE calls it on the second press within the system's double-click
    // time (Windows: 500 ms)
    if double
        && let Some(p) = pos
        && !st.scout_chips.iter().any(|c| c.contains(p))
        // Nor inside a Scout guide's column: Scout's guides never become bands.
        && !st.scout.iter().any(|(_, col, ..)| p.x >= col.left() && p.x <= col.right() && cx.geo.plot.contains(p))
    {
        let on_ceiling = st.ceiling_open && cx.ceil.shape.pts.iter().any(|pt| pos2(geo.fx(pt.hz), geo.sy(pt.db)).distance(p) <= 8.0);
        if !on_ceiling {
            if let Some(h) = hit_node(cx, p) {
                toggle_dyn(app, cx, h);
            } else if guide_at(cx, st, p).is_none() {
                add_band(app, cx, st, q::TYPE_BELL, geo.xf(p.x), geo.yg(p.y).clamp(-30.0, 30.0));
            }
        }
    }

    // ---- mouseWheelMove: Q on the node under the pointer, x1.08 a notch, a burst one undo step
    if resp.hovered()
        && let Some(p) = pos
        && let Some(h) = hit_node(cx, p)
    {
        let dy = juce_wheel_delta_y(ui);
        consume_wheel(ui);
        if dy != 0.0 {
            let qv = (cx.bands[h].q * 1.08f64.powf(if dy > 0.0 { 1.0 } else { -1.0 })).clamp(0.025, 40.0);
            params_merged(app, cx, &[(bp(h, cq::F_Q), q::q_to_knob(qv).clamp(0.0, 1.0))], "wheel");
            st.wheel_at = ui.input(|i| i.time);
        }
    }
}

/// `trySpectrumGrab`.
fn spectrum_grab(app: &mut SoundApp, cx: &Cx, st: &mut Editor, p: Pos2, alt: bool) -> bool {
    if !st.analyser || !st.post || st.held_post.is_empty() || alt || !cx.geo.plot.contains(p) {
        return false;
    }
    let pl = cx.geo.plot;
    let px = p.x.round() - pl.left();
    let mut best = f32::MAX;
    let mut d = -6.0;
    while d <= 6.0 {
        best = best.min((spectrum_y(cx, &st.held_post, (px + d).clamp(0.0, pl.width())) - p.y).abs());
        d += 2.0;
    }
    if best >= 9.0 {
        return false;
    }
    let f = cx.geo.xf(p.x);
    let Some(slot) = cx.first_free() else { return false };
    end_gesture(app);
    let list = [
        (bp(slot, cq::F_FREQ), q::freq_to_knob(f).clamp(0.0, 1.0)),
        (bp(slot, cq::F_GAIN), q::gain_to_knob(0.0)),
        (bp(slot, cq::F_Q), q::q_to_knob(4.0).clamp(0.0, 1.0)),
        (bp(slot, cq::F_TYPE), q::TYPE_BELL as f32),
        (bp(slot, cq::F_PLACE), q::PLACE_STEREO as f32),
        (bp(slot, cq::F_SLOPE), 1.0),
        (bp(slot, cq::F_DYN), 0.0),
        (bp(slot, cq::F_ON), q::ON_ACTIVE as f32),
    ];
    params(app, cx, &list);
    select(st, Some(slot));
    st.drag = Some(NodeDrag { slot, start: p, freq: f, gain: 0.0, axis: 0, gain_only: true, has_gain: true });
    true
}

/// `ceilingMouseDrag` on this editor's scale (10 Hz..30 kHz across, -96..0 dBFS up).
fn drag_ceiling(geo: &Geo, from: &Ceiling, i: usize, delta: egui::Vec2, snap: bool, alone: bool) -> Ceiling {
    let mut c = from.clone();
    let n = from.pts.len();
    let Some(start) = from.pts.get(i).copied() else { return c };
    let mut hz = start.hz;
    if i > 0 && i + 1 < n {
        let gap = 2f64.powf(1.0 / 6.0);
        let (lo, hi) = (from.pts[i - 1].hz * gap, from.pts[i + 1].hz / gap);
        if lo < hi {
            hz = geo.xf(geo.fx(hz) + delta.x).clamp(lo, hi);
        }
    }
    c.pts[i].hz = hz;
    let mut db = geo.ys(geo.sy(start.db) + delta.y).clamp(-96.0, 0.0);
    if snap && i > 0 {
        let r = c.pts[i - 1];
        let oct = (hz / r.hz).log2();
        if oct.abs() > 1.0e-3 {
            let slope = (((db - r.db) / oct) / 1.5).round() * 1.5;
            db = (r.db + slope * oct).clamp(-96.0, 0.0);
        }
    }
    cl::move_node(&mut c, i, db, !alone);
    c
}

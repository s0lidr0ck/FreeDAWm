//! Comp's editor in the classic drawer's width ([`crate::module_drawer::DRAWER_W`]), about a
//! mixer strip tall: the full editor's sections, names and order, folded to fit. Header (title and
//! preset strip; Channels, Undo, 1, 2, Bypass), the model (a menu of the ten circuits, each with
//! its description), one display that flips between the gain-reduction meter and the transfer
//! curve, the split switch with the dry and lane slots as knobs (and the crossovers in
//! Multiband), the big knobs in rows of three, and a More section holding the style's switches,
//! the small knobs and Auto gain. The gestures are the full editor's own code (`super::*_ui`,
//! `super::knob_laid`, `super::header_controls`); the slots send what the lane mixer sends.

use super::lane_mixer::level_text;
use super::model::{self, STYLES};
use super::{
    ACCENT, BIG_IDS, Editor, HeaderRects, LANE_ON, PALETTE, SMALL_IDS, curve, gr_meter, header_controls, knob_laid, lane_param, param, pick_style,
    prelude, small_w, style_of, style_short_name, values,
};
use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, LabelLayout, PillStyle, dial, pill_height};
use crate::editor_kit::look::{paint_background, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::slim::{self, BIG_KNOB, GAP, MODEL_H, MORE_H, MenuEntry, PAD_X, PAD_Y, ROW_H, SMALL_KNOB, grid};
use crate::editor_kit::{Host, end_gesture, set_params, set_params_merged};
use crate::theme::{bold, regular};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::comp as dsp;

pub use slim::size;

const DISPLAY_H: f32 = 158.0;
const SLOT_KNOB: f32 = 38.0;
const XOVER_H: f32 = 50.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let ed = host.rect;
    if prelude(st, app, ui, host) {
        return;
    }
    let p = ui.painter().clone();
    paint_background(&p, ed);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header: the title and the preset strip; then Channels, Undo, 1, 2, Bypass
    let btn_h = pill_height(PillStyle::Small);
    let head = row(y, ROW_H + 2.0);
    let title_w = paint_title(&p, head, "Comp", "Engine", &PALETTE);
    let (prev, name, next) = slim::preset_strip(head, title_w);
    y += ROW_H + 2.0 + 6.0;
    let chan_text = model::channel_mode_name(param(host, "chanMode").round() as i32);
    let mut widths =
        [small_w(ui, chan_text).max(74.0), small_w(ui, "Undo"), small_w(ui, "1").max(28.0), small_w(ui, "2").max(28.0), small_w(ui, "Bypass")];
    // the pills spread over the row in three groups and Bypass, as the header orders them; when
    // they do not fit, Channels gives up its minimum, then all of them narrow alike (pill_row)
    if widths.iter().sum::<f32>() > w - 4.0 - 3.0 * 4.0 {
        widths[0] = small_w(ui, chan_text);
    }
    let cells = slim::pill_row(x0, w, y, &widths, &[2]);
    let rects = HeaderRects { chan: cells[0], undo: cells[1], slot_1: cells[2], slot_2: cells[3], bypass: cells[4], prev, next, name };
    header_controls(st, app, ui, host, &rects);
    y += btn_h + GAP + 2.0;

    let v = values(host, st.lane);

    // ---- 2. the model: one menu of the ten circuits (the full editor's strip and blurb)
    let entries: Vec<MenuEntry> = STYLES.iter().map(|s| MenuEntry { name: s.name, tag: s.tag, blurb: s.blurb, credit: s.credit }).collect();
    let open_now = std::mem::take(&mut st.models_open);
    if let Some(i) = slim::model_menu(ui, row(y, MODEL_H), "comp", "Model", &PALETTE, &entries, v.style as usize, open_now) {
        pick_style(st, app, host, i);
    }
    y += MODEL_H + GAP;

    // ---- 3. one display: the gain-reduction meter or the transfer curve with its meters
    let display = row(y, DISPLAY_H);
    if st.slim_curve {
        curve::paint(&p, display, ACCENT, &v, &mut st.curve, st.meter.input, st.meter.output, st.meter.gr);
    } else {
        gr_meter::paint(&p, display, ACCENT, &st.meter);
    }
    slim::display_label(ui, display, "comp", if st.slim_curve { "Transfer curve" } else { "Gain reduction meter" });
    let toggle = Rect::from_min_size(display.left_top() + vec2(6.0, 6.0), vec2(70.0, 16.0));
    if let Some(i) = slim::display_toggle(ui, toggle, "comp", &PALETTE, &["GR", "Curve"], usize::from(st.slim_curve)) {
        st.slim_curve = i == 1;
    }
    y += DISPLAY_H + GAP;

    // ---- 4. Parallel / Multiband / Serial, the dry and lane slots, the crossovers (Multiband)
    let split = super::split_ui(st, app, ui, host, row(y, ROW_H));
    y += ROW_H + 4.0;
    y = slots(st, app, ui, host, row(y, 0.0), split);
    if split == dsp::comp_engine_dsp::SPLIT_MULTIBAND {
        y += 4.0;
        super::crossover_ui(st, app, ui, host, row(y, XOVER_H), split);
        y += XOVER_H;
    }
    y += GAP + 4.0;

    // ---- 5. the big knobs, three to a row
    let bigs: Vec<&'static str> = BIG_IDS.iter().copied().filter(|id| model::visible(id, v.style)).collect();
    let cell = vec2((w / 3.0).floor(), BIG_KNOB + 32.0);
    let cells = grid(x0, w, y, bigs.len(), 3, cell, 2.0);
    for (id, r) in bigs.iter().zip(&cells) {
        knob_laid(st, app, ui, host, *r, id, &v, false, LabelLayout::Below);
    }
    if let Some(last) = cells.last() {
        y = last.bottom() + GAP;
    }

    // ---- 6. More: the style's switches, the small knobs and Auto gain
    let smalls: Vec<&'static str> = SMALL_IDS.iter().copied().filter(|id| model::visible(id, v.style)).collect();
    let switches: Vec<&'static str> = ["variant", "detMode"].into_iter().filter(|b| model::switch_def(b, v.style).1.len() > 1).collect();
    let mut summary: Vec<&str> = switches.iter().map(|b| model::switch_def(b, v.style).0).collect();
    summary.extend(smalls.iter().map(|id| model::label(id, v.style)));
    if slim::more_header(ui, row(y, MORE_H), "comp", &PALETTE, st.slim_more, &summary.join(" \u{00B7} ")) {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 8.0;
        for base in switches {
            super::switch_ui(st, app, ui, host, row(y, ROW_H), base, &v);
            y += ROW_H + 2.0 + 12.0 + 6.0;
        }
        let cell = vec2((w / 3.0).floor(), SMALL_KNOB + 32.0);
        let cells = grid(x0, w, y, smalls.len(), 3, cell, 2.0);
        for (id, r) in smalls.iter().zip(&cells) {
            knob_laid(st, app, ui, host, *r, id, &v, true, LabelLayout::Below);
        }
        if let Some(last) = cells.last() {
            y = last.bottom() + 4.0;
        }
        let auto_w = small_w(ui, "Auto gain").max(92.0);
        let ph = pill_height(PillStyle::Small);
        super::auto_gain_ui(st, app, ui, host, Rect::from_min_size(pos2((x0 + (w - auto_w) / 2.0).round(), y), vec2(auto_w, ph)));
        y += ph + 2.0 + 12.0;
    }
    y += PAD_Y;
    slim::settle(ui, ed.top(), y, &mut st.slim_h);
}

/// The dry and lane slots, side by side in `r` (its top; returns the bottom): the lane's letter
/// (a press selects it, as the mixer's header), its On button, its level as a knob, and its
/// circuit and level under it. The same commands and undo steps as the lane mixer.
fn slots(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, split: i64) -> f32 {
    let multiband = split == dsp::comp_engine_dsp::SPLIT_MULTIBAND;
    let serial = split == dsp::comp_engine_dsp::SPLIT_SERIAL;
    let col_w = (r.width() / 4.0).floor();
    let (letter_h, on_h, foot_h) = (15.0, 15.0, 34.0);
    let h = 4.0 + letter_h + 2.0 + on_h + 4.0 + SLOT_KNOB + foot_h;
    let area = Rect::from_min_size(r.min, vec2(r.width(), h));
    let p = ui.painter().clone();
    p.rect_filled(area, CornerRadius::same(8), al(Color32::BLACK, 0.22));
    p.rect_stroke(area.shrink(0.5), CornerRadius::same(8), Stroke::new(1.0, al(Color32::WHITE, 0.05)), StrokeKind::Middle);
    for s in 0..4usize {
        let col = Rect::from_min_size(pos2(area.left() + (r.width() - 4.0 * col_w) / 2.0 + col_w * s as f32, area.top()), vec2(col_w, h));
        let is_dry = s == 0;
        let (level_id, on_id) = if is_dry { ("dry", "dryOn") } else { (lane_param("level", s - 1), LANE_ON[(s - 1).min(2)]) };
        let sel = !is_dry && st.lane + 1 == s;
        let live = param(host, on_id) > 0.5;
        let dim = if live { 1.0 } else { 0.45 };
        if sel {
            p.rect_filled(col.shrink(2.0), CornerRadius::same(6), al(PALETTE.accent, 0.10));
            p.rect_stroke(col.shrink(2.5), CornerRadius::same(6), Stroke::new(1.0, al(PALETTE.accent, 0.45)), StrokeKind::Middle);
        }
        // the letter: a press selects the lane
        let letter_r = Rect::from_min_size(col.min + vec2(0.0, 4.0), vec2(col_w, letter_h));
        let letter = ["A", "B", "C"].get(s.wrapping_sub(1)).copied().unwrap_or("");
        let band = ["LOW", "MID", "HIGH"].get(s.wrapping_sub(1)).copied().unwrap_or("");
        let text = if is_dry {
            "DRY".to_string()
        } else if multiband {
            format!("{letter} {band}")
        } else if serial && s < 3 {
            format!("{letter} \u{2192}")
        } else {
            letter.to_string()
        };
        let small_text = is_dry || multiband;
        p.text(
            letter_r.center(),
            Align2::CENTER_CENTER,
            text,
            bold(if small_text { 10.5 } else { 13.0 }),
            al(if sel { PALETTE.accent } else { PALETTE.text_bright }, dim),
        );
        if !is_dry {
            let lr = ui.interact(letter_r, ui.id().with(("comp-slot-letter", s)), Sense::click());
            lr.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Lane {letter}")));
            if lr.clicked() {
                st.lane = s - 1;
            }
        }
        // On / Off (Byp where a lane is in the path)
        let on_r = Rect::from_min_size(pos2(col.left() + 6.0, letter_r.bottom() + 2.0), vec2(col_w - 12.0, on_h));
        let on_resp = ui.interact(on_r, ui.id().with(("comp-slot-on", s)), Sense::click());
        on_resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, live, format!("{} on", if is_dry { "Dry" } else { letter }))
        });
        p.rect_filled(on_r, CornerRadius::same(3), if live { PALETTE.accent } else { al(Color32::BLACK, 0.25) });
        let edge = if live {
            PALETTE.accent
        } else if on_resp.hovered() {
            PALETTE.border_hover
        } else {
            PALETTE.border
        };
        p.rect_stroke(on_r.shrink(0.5), CornerRadius::same(3), Stroke::new(1.0, edge), StrokeKind::Middle);
        let label = if live {
            "ON"
        } else if (multiband || serial) && !is_dry {
            "BYP"
        } else {
            "OFF"
        };
        p.text(on_r.center(), Align2::CENTER_CENTER, label, bold(9.5), if live { PALETTE.on_accent } else { PALETTE.text_faint });
        if on_resp.clicked() {
            st.push_undo(host);
            set_params(app, host, &[(on_id, if live { 0.0 } else { 1.0 })]);
        }
        // the level knob: a drag is one undo step, a double-click is unity, as the fader
        let level = param(host, level_id);
        let knob_r = Rect::from_min_size(pos2(col.center().x - col_w / 2.0, on_r.bottom() + 4.0), vec2(col_w, SLOT_KNOB + 30.0));
        let name = if is_dry { "Dry level".to_string() } else { format!("{letter} level") };
        let o = DialOpts { label: &name, value_text: "", default: 0.75, layout: LabelLayout::Below, inactive: false, ..DialOpts::default() };
        // the dial paints its own (empty) readout rows; the foot below is the slot's
        let out = {
            let mut kui = ui.new_child(egui::UiBuilder::new().max_rect(knob_r).id_salt(("comp-slot", s)));
            kui.set_clip_rect(Rect::from_min_size(knob_r.min, vec2(col_w, SLOT_KNOB + 1.0)).intersect(ui.clip_rect()));
            let mut pal = PALETTE;
            if !live {
                pal.accent = al(PALETTE.accent, 0.45);
            }
            dial(&mut kui, knob_r, ("slot", s), &pal, level, &o)
        };
        if out.started {
            if !is_dry && !sel {
                st.lane = s - 1;
            }
            st.push_undo(host);
            end_gesture(app);
        }
        if let Some(nv) = out.value {
            set_params_merged(app, host, &[(level_id, nv.clamp(0.0, 1.0))], &format!("lane-level-{s}"));
        }
        if out.ended {
            end_gesture(app);
        }
        // the circuit and the level; the lane's gain reduction under them
        let foot = Rect::from_min_size(pos2(col.left(), on_r.bottom() + 4.0 + SLOT_KNOB + 2.0), vec2(col_w, 12.0));
        let circuit = if is_dry { "no comp" } else { style_short_name(style_of(param(host, lane_param("style", s - 1)))) };
        let shown = out.value.unwrap_or(level);
        let pc = p.with_clip_rect(foot.intersect(p.clip_rect()));
        let mut job = egui::text::LayoutJob::default();
        let fmt = |c: Color32| egui::TextFormat { font_id: regular(9.0), color: al(c, dim), ..Default::default() };
        job.append(circuit, 0.0, fmt(PALETTE.text_faint));
        job.append(" \u{00B7} ", 0.0, fmt(PALETTE.text_faint));
        job.append(&level_text(shown), 0.0, fmt(if sel { PALETTE.text_bright } else { PALETTE.text_dim }));
        let g = ui.fonts_mut(|f| f.layout_job(job));
        pc.galley(pos2(foot.center().x - g.size().x / 2.0, foot.top()), g, PALETTE.text_dim);
        if !is_dry {
            // the lane's gain reduction: a bar (0..20 dB, as the mixer's) and its value
            let gr = st.lane_gr.get(s - 1).copied().filter(|g| g.is_finite() && live).unwrap_or(0.0);
            let row = Rect::from_min_size(pos2(col.left() + 7.0, foot.bottom() + 4.0), vec2(col_w - 14.0, 10.0));
            let text_w = 24.0;
            let bar = Rect::from_min_size(pos2(row.left(), row.center().y - 2.0), vec2(row.width() - text_w - 3.0, 4.0));
            p.rect_filled(bar, CornerRadius::same(2), al(Color32::BLACK, 0.45));
            let on = gr > 0.05;
            if on {
                let f = (gr / 20.0).clamp(0.0, 1.0).sqrt();
                p.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(2.0), bar.height())), CornerRadius::same(2), PALETTE.accent);
            }
            let txt = if on { format!("-{gr:.1}") } else { "0.0".to_string() };
            p.text(
                pos2(row.right(), row.center().y),
                Align2::RIGHT_CENTER,
                txt,
                regular(9.0),
                if on { PALETTE.accent } else { al(PALETTE.text_faint, dim) },
            );
        }
    }
    area.bottom()
}

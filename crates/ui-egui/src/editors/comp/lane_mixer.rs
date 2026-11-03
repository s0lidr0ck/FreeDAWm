//! `CompLaneMixer.h` `LaneMixer`: the dry strip and the three lanes, as a mixer beside the
//! displays. Pressing a lane's header selects it for editing; the On button toggles it; the fader
//! follows the pointer from the press (selecting its lane); a double-click on a fader returns it
//! to unity.

use crate::editor_kit::VstPalette;
use crate::editor_kit::palette::{al, argb};
use crate::theme::{bold, regular};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::comp::knob_to_lane_db;

pub const NUM_STRIPS: usize = 4;
pub const DRY_STRIP: usize = 0;

const LETTER_H: f32 = 15.0;
const STYLE_H: f32 = 13.0;
const ON_H: f32 = 17.0;
const HEADER_H: f32 = LETTER_H + STYLE_H + ON_H + 4.0;
const FOOT_H: f32 = 18.0;
const PAD: f32 = 6.0;

/// One strip's state (`LaneState`).
#[derive(Clone, Debug)]
pub struct Strip {
    pub level: f32,
    pub gr: f32,
    pub on: bool,
    pub style: &'static str,
}

/// What the mixer shows.
pub struct View {
    pub strips: [Strip; NUM_STRIPS],
    pub selected: usize,
    pub multiband: bool,
    pub serial: bool,
}

/// What the mixer did this frame (`onSelect`, `onToggleOn`, `onGestureStart`, `onLevel`,
/// `onGestureEnd`).
#[derive(Default, Debug)]
pub struct Out {
    pub select: Option<usize>,
    pub toggle_on: Option<usize>,
    pub gesture_start: Option<usize>,
    pub level: Option<(usize, f32)>,
    pub gesture_end: Option<usize>,
}

fn cell_of(r: Rect, lane: usize) -> Rect {
    let w = (r.width() / NUM_STRIPS as f32).floor();
    Rect::from_min_size(pos2(r.left() + w * lane.min(NUM_STRIPS - 1) as f32, r.top()), vec2(w, r.height()))
}
fn header(r: Rect, lane: usize) -> Rect {
    let c = cell_of(r, lane);
    Rect::from_min_size(c.min, vec2(c.width(), HEADER_H)).shrink(2.0)
}
fn on_button(r: Rect, lane: usize) -> Rect {
    let c = cell_of(r, lane);
    let h = Rect::from_min_size(c.min, vec2(c.width(), HEADER_H)).shrink2(vec2(4.0, 0.0));
    Rect::from_min_size(pos2(h.left(), h.top() + LETTER_H + STYLE_H + 2.0), vec2(h.width(), ON_H))
}
fn body(r: Rect, lane: usize) -> Rect {
    let c = cell_of(r, lane);
    Rect::from_min_max(pos2(c.left(), c.top() + HEADER_H), pos2(c.right(), c.bottom() - FOOT_H)).shrink(PAD)
}
/// The fader's travel, excluding the gain-reduction bar beside it.
fn track(r: Rect, lane: usize) -> Rect {
    let c = body(r, lane);
    let w = (c.width() / 2.0 - 2.0).floor().max(10.0);
    Rect::from_min_size(c.min, vec2(w, c.height()))
}
fn gr_bar(r: Rect, lane: usize) -> Rect {
    let c = body(r, lane);
    let w = (c.width() / 2.0 - 2.0).floor().max(10.0);
    let rest = Rect::from_min_max(pos2(c.left() + w, c.top()), c.max);
    let bw = (rest.width() - 4.0).max(6.0);
    Rect::from_min_max(pos2(rest.right() - bw, rest.top()), rest.max)
}
fn lane_at(r: Rect, p: Pos2) -> Option<usize> {
    let w = (r.width() / NUM_STRIPS as f32).floor().max(1.0);
    let i = ((p.x - r.left()) / w).floor();
    (i >= 0.0 && (i as usize) < NUM_STRIPS).then_some(i as usize)
}
fn level_at(r: Rect, lane: usize, y: f32) -> f32 {
    let t = track(r, lane);
    (1.0 - (y - t.top()) / t.height().max(1.0)).clamp(0.0, 1.0)
}

/// The mixer in `r`. `dragging` is the strip being dragged (kept by the editor).
pub fn lane_mixer(ui: &mut Ui, r: Rect, pal: &VstPalette, v: &View, dragging: &mut Option<usize>) -> Out {
    let mut out = Out::default();
    let resp = ui.interact(r, ui.id().with("comp-lane-mixer"), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Lane mixer"));
    let (pressed, released, pointer) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.interact_pos()));

    // mouseDown
    if pressed
        && resp.hovered()
        && let Some(p) = pointer
        && let Some(lane) = lane_at(r, p)
    {
        if on_button(r, lane).contains(p) {
            out.toggle_on = Some(lane);
        } else if header(r, lane).contains(p) {
            if lane != DRY_STRIP {
                out.select = Some(lane);
            }
        } else if track(r, lane).expand2(vec2(6.0, 2.0)).contains(p) {
            // selecting the lane you are about to balance is what you meant
            if lane != DRY_STRIP && lane != v.selected {
                out.select = Some(lane);
            }
            *dragging = Some(lane);
            out.gesture_start = Some(lane);
            out.level = Some((lane, level_at(r, lane, p.y)));
        }
    }
    // mouseDrag
    if let Some(lane) = *dragging
        && !pressed
        && ui.input(|i| i.pointer.primary_down())
        && let Some(p) = pointer
    {
        out.level = Some((lane, level_at(r, lane, p.y)));
    }
    // mouseUp
    if released && let Some(lane) = dragging.take() {
        out.gesture_end = Some(lane);
    }
    // mouseDoubleClick: back to unity
    if resp.double_clicked()
        && let Some(p) = resp.interact_pointer_pos()
        && let Some(lane) = lane_at(r, p)
        && track(r, lane).expand2(vec2(6.0, 2.0)).contains(p)
    {
        out.gesture_start = Some(lane);
        out.level = Some((lane, 0.75));
        out.gesture_end = Some(lane);
    }

    let painter = ui.painter();
    painter.rect_filled(r, CornerRadius::same(8), al(Color32::BLACK, 0.22));
    painter.rect_stroke(r.shrink(0.5), CornerRadius::same(8), Stroke::new(1.0, al(Color32::WHITE, 0.05)), StrokeKind::Middle);
    for lane in 0..NUM_STRIPS {
        let mut s = v.strips[lane].clone();
        if let Some((l, lv)) = out.level
            && l == lane
        {
            s.level = lv;
        }
        paint_lane(painter, r, pal, v, lane, &s);
    }
    out
}

fn paint_lane(g: &egui::Painter, r: Rect, pal: &VstPalette, v: &View, lane: usize, s: &Strip) {
    let sel = lane == v.selected;
    let live = s.on;
    let dim = if live { 1.0 } else { 0.4 };
    let accent = pal.accent;
    let cell = cell_of(r, lane);
    if sel {
        g.rect_filled(cell.shrink(2.0), CornerRadius::same(6), al(accent, 0.10));
        g.rect_stroke(cell.shrink(2.5), CornerRadius::same(6), Stroke::new(1.0, al(accent, 0.45)), StrokeKind::Middle);
    }

    // ---- header: the letter, then the circuit, then the button
    let h = Rect::from_min_size(cell.min, vec2(cell.width(), HEADER_H)).shrink(2.0);
    let is_dry = lane == DRY_STRIP;
    let band = v.multiband && !is_dry;
    let letter = ["A", "B", "C"].get(lane.wrapping_sub(1)).copied().unwrap_or("");
    let band_name = ["LOW", "MID", "HIGH"].get(lane.wrapping_sub(1)).copied().unwrap_or("");
    let text = if is_dry {
        "DRY".to_string()
    } else if band {
        format!("{letter} {band_name}")
    } else if v.serial && lane < 3 {
        format!("{letter} \u{2192}")
    } else {
        letter.to_string()
    };
    let letter_r = Rect::from_min_size(h.min, vec2(h.width(), LETTER_H));
    g.text(
        letter_r.center(),
        Align2::CENTER_CENTER,
        text,
        bold(if is_dry || band { 10.5 } else { 13.0 }),
        al(if sel { accent } else { pal.text_bright }, dim),
    );
    let style_r = Rect::from_min_size(pos2(h.left(), letter_r.bottom()), vec2(h.width(), STYLE_H));
    g.text(style_r.center(), Align2::CENTER_CENTER, if is_dry { "no comp" } else { s.style }, regular(9.5), al(pal.text_faint, dim));
    let btn = on_button(r, lane);
    g.rect_filled(btn, CornerRadius::same(3), if live { accent } else { al(Color32::BLACK, 0.25) });
    g.rect_stroke(btn.shrink(0.5), CornerRadius::same(3), Stroke::new(1.0, if live { accent } else { pal.border }), StrokeKind::Middle);
    let label = if live {
        "ON"
    } else if (v.multiband || v.serial) && !is_dry {
        "BYP"
    } else {
        "OFF"
    };
    g.text(btn.center(), Align2::CENTER_CENTER, label, bold(9.5), if live { pal.on_accent } else { pal.text_faint });

    // ---- the fader
    let t = track(r, lane);
    g.rect_filled(Rect::from_center_size(t.center(), vec2(4.0, t.height())), CornerRadius::same(2), al(Color32::BLACK, 0.35));
    let unity_y = (t.bottom() - 0.75 * t.height()).round();
    g.line_segment([pos2(t.left() - 2.0, unity_y + 0.5), pos2(t.right() + 2.0, unity_y + 0.5)], Stroke::new(1.0, al(pal.text_faint, 0.5)));
    let level = if s.level.is_finite() { s.level.clamp(0.0, 1.0) } else { 0.0 };
    let y = t.bottom() - level * t.height();
    g.rect_filled(
        Rect::from_min_max(pos2(t.center().x - 2.0, y), pos2(t.center().x + 2.0, t.bottom())),
        CornerRadius::same(2),
        al(accent, dim * 0.8),
    );
    let thumb = Rect::from_min_size(pos2(t.center().x - 7.0, y - 4.0), vec2(14.0, 8.0));
    g.rect_filled(thumb, CornerRadius::same(2), argb(0xFF2F3136));
    g.rect_stroke(
        thumb.shrink(0.5),
        CornerRadius::same(2),
        Stroke::new(1.0, al(if sel { accent } else { pal.border_hover }, dim)),
        StrokeKind::Middle,
    );

    // ---- gain reduction beside its own fader (the dry strip has none)
    if !is_dry {
        let bar = gr_bar(r, lane);
        g.rect_filled(bar, CornerRadius::same(2), al(Color32::BLACK, 0.35));
        if live && s.gr.is_finite() && s.gr > 0.02 {
            let f = (s.gr / 20.0).clamp(0.0, 1.0);
            g.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width(), f * bar.height())), CornerRadius::same(2), al(accent, 0.55));
        }
    }

    // ---- the level, in dB
    let foot = Rect::from_min_max(pos2(cell.left() + 2.0, cell.bottom() - FOOT_H), pos2(cell.right() - 2.0, cell.bottom()));
    g.text(foot.center(), Align2::CENTER_CENTER, level_text(level), regular(10.0), al(if sel { pal.text_bright } else { pal.text_dim }, dim));
}

/// A strip's level as its foot shows it: "off", "0.0", "+2.5", "-6.0".
pub fn level_text(level: f32) -> String {
    let level = if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 };
    let db = knob_to_lane_db(level);
    if level <= 0.0 {
        "off".to_string()
    } else if db.abs() < 0.05 {
        "0.0".to_string()
    } else {
        format!("{}{db:.1}", if db > 0.0 { "+" } else { "" })
    }
}

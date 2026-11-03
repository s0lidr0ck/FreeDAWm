//! `fs::filter::StepStrip`: the sequencer steps as one strip of draggable bars, the running step
//! lit. A press (`onGestureStart`) then every drag position sets the step under the pointer to
//! its height (`1 - y / height`), the column picked from the pointer's x.

use crate::editor_kit::VstPalette;
use crate::editor_kit::palette::al;
use egui::{Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// What the strip did this frame.
#[derive(Debug, Default)]
pub struct StripOut {
    /// A press began a gesture (`onGestureStart`).
    pub started: bool,
    /// `onStepChanged (index, 0..1)`.
    pub changed: Option<(usize, f32)>,
    /// The press was let go.
    pub ended: bool,
}

/// `drag`: the step and value under `p` in `r` with `count` columns.
pub fn hit(r: Rect, count: usize, p: egui::Pos2) -> (usize, f32) {
    let count = count.clamp(2, 16);
    let w = r.width() / count as f32;
    let i = (((p.x - r.left()) / w.max(1.0)).floor().max(0.0) as usize).min(count - 1);
    let v = (1.0 - (p.y - r.top()) / r.height().max(1.0)).clamp(0.0, 1.0);
    (i, v)
}

/// Paints the strip and handles its gesture. `values` are the sixteen steps, `current` the
/// running step (or none).
pub fn step_strip(ui: &mut Ui, r: Rect, pal: &VstPalette, values: &[f32; 16], count: usize, current: Option<usize>) -> StripOut {
    let count = count.clamp(2, 16);
    let resp = ui.interact(r, ui.id().with("filter-step-strip"), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Step strip"));
    let mut out = StripOut::default();
    let pressed = resp.is_pointer_button_down_on();
    // `mouseDown`: the press itself starts the gesture (and sets the step under it).
    out.started = pressed && ui.input(|i| i.pointer.primary_pressed());
    if pressed && let Some(p) = resp.interact_pointer_pos() {
        out.changed = Some(hit(r, count, p));
    }
    if resp.drag_stopped() || resp.clicked() {
        out.ended = true;
    }

    let p = ui.painter();
    let w = r.width() / count as f32;
    for (i, stored) in values.iter().enumerate().take(count) {
        let cell = Rect::from_min_size(pos2(r.left() + w * i as f32, r.top()), vec2(w, r.height())).shrink2(vec2(1.5, 0.0));
        p.rect_filled(cell, CornerRadius::same(2), al(Color32::BLACK, 0.35));
        let v = out.changed.filter(|(c, _)| *c == i).map_or(*stored, |(_, v)| v);
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let bar = Rect::from_min_max(pos2(cell.left(), cell.bottom() - cell.height() * v), cell.max);
        let lit = current == Some(i);
        if bar.height() > 1.0 {
            p.rect_filled(bar, CornerRadius::same(2), if lit { pal.accent } else { al(pal.accent, 0.55) });
        }
        if lit {
            p.rect_stroke(cell.shrink(0.5), CornerRadius::same(2), Stroke::new(1.0, al(pal.accent, 0.8)), StrokeKind::Middle);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_picks_the_column_and_height() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(160.0, 100.0));
        assert_eq!(hit(r, 8, pos2(25.0, 25.0)), (1, 0.75));
        assert_eq!(hit(r, 8, pos2(-5.0, 200.0)), (0, 0.0));
        assert_eq!(hit(r, 8, pos2(500.0, -5.0)), (7, 1.0));
        assert_eq!(hit(r, 99, pos2(159.0, 50.0)), (15, 0.5));
    }
}

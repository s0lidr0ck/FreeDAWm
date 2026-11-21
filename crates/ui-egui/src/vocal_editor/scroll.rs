//! The Vocal Editor's scroll bars: time under the roll, pitch at its right. They look and act as
//! the MIDI Editor's bar under its roll (a rounded trough, the thumb the border colour, the accent
//! while hovered or dragged, at least 12 px long, dragging anywhere on the bar moves the view with
//! the pointer); a click beside the thumb pages one view that way.

use crate::theme::Tokens;
use egui::{Rect, Sense, Ui, pos2};

/// A bar's thickness: the MIDI Editor's.
pub const BAR_W: f32 = crate::midi_editor::BAR_H;
/// The shortest thumb (px), as the MIDI Editor's.
pub const MIN_THUMB: f32 = 12.0;

/// What a bar shows, in any unit along its axis: the range `lo..hi` and the part in view,
/// `start..start + len`. [`Span::new`] widens the range to hold the view, so the thumb never
/// leaves the bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub lo: f64,
    pub hi: f64,
    pub start: f64,
    pub len: f64,
}

impl Span {
    pub fn new(lo: f64, hi: f64, start: f64, len: f64) -> Span {
        let ok = |v: f64, d: f64| if v.is_finite() { v } else { d };
        let start = ok(start, 0.0);
        let len = ok(len, 0.0).max(0.0);
        let lo = ok(lo, start).min(start);
        let hi = ok(hi, start + len).max(start + len).max(lo + 1e-9);
        Span { lo, hi, start, len }
    }

    /// The share of the range in view, 0..=1: the thumb's share of the bar.
    pub fn fraction(&self) -> f64 {
        (self.len / (self.hi - self.lo).max(1e-9)).clamp(0.0, 1.0)
    }

    /// The thumb along a bar from `a` to `b` (px): its two ends, at least [`MIN_THUMB`] apart
    /// (or the bar's length when shorter), inside the bar.
    pub fn thumb(&self, a: f32, b: f32) -> (f32, f32) {
        let len = (b - a).max(0.0);
        let range = (self.hi - self.lo).max(1e-9);
        let size = ((self.len / range) as f32 * len).clamp(MIN_THUMB.min(len), len);
        let from = (a + ((self.start - self.lo) / range) as f32 * len).clamp(a, (b - size).max(a));
        (from, from + size)
    }

    /// Units per px along a bar `len` px long.
    fn per_px(&self, len: f32) -> f64 {
        (self.hi - self.lo) / f64::from(len.max(1.0))
    }
}

/// Draws a bar in `rect` (`vertical`: along y) for `span` and handles it; returns how far the
/// view should move, in the span's unit (positive: later, or down), and the thumb drawn.
pub fn bar(ui: &Ui, rect: Rect, id: egui::Id, span: Span, vertical: bool) -> (f64, Rect) {
    let t = Tokens::current();
    if !rect.is_positive() {
        return (0.0, Rect::NOTHING);
    }
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 3.0, t.panel_bg2);
    let (a, b) = if vertical { (rect.top(), rect.bottom()) } else { (rect.left(), rect.right()) };
    let (from, to) = span.thumb(a, b);
    let thumb = if vertical {
        Rect::from_min_max(pos2(rect.left() + 1.0, from), pos2(rect.right() - 1.0, to))
    } else {
        Rect::from_min_max(pos2(from, rect.top() + 1.0), pos2(to, rect.bottom() - 1.0))
    };
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let fill = if resp.dragged() || resp.hovered() { t.accent } else { t.border };
    p.rect_filled(thumb, 3.0, fill);
    let per_px = span.per_px(b - a);
    let mut by = 0.0;
    if resp.dragged() {
        let d = resp.drag_delta();
        by = f64::from(if vertical { d.y } else { d.x }) * per_px;
    } else if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        // Beside the thumb: one view that way.
        let at = if vertical { at.y } else { at.x };
        if at < from {
            by = -span.len;
        } else if at > to {
            by = span.len;
        }
    }
    (if by.is_finite() { by } else { 0.0 }, thumb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thumb_is_the_share_in_view() {
        let s = Span::new(0.0, 100.0, 25.0, 50.0);
        assert!((s.fraction() - 0.5).abs() < 1e-9);
        assert_eq!(s.thumb(0.0, 200.0), (50.0, 150.0));
    }

    #[test]
    fn the_range_holds_the_view_and_odd_input_is_safe() {
        let s = Span::new(10.0, 20.0, 0.0, 40.0);
        assert_eq!((s.lo, s.hi), (0.0, 40.0));
        assert_eq!(s.fraction(), 1.0);
        let z = Span::new(f64::NAN, f64::INFINITY, 5.0, -3.0);
        assert!(z.fraction().is_finite());
        let (a, b) = z.thumb(0.0, 0.0);
        assert!(a.is_finite() && b.is_finite());
        // A tiny view keeps a thumb that can be grabbed, inside the bar.
        let tiny = Span::new(0.0, 1e9, 1e9 - 1.0, 1.0);
        let (a, b) = tiny.thumb(0.0, 100.0);
        assert!((b - a - MIN_THUMB).abs() < 1e-3 && b <= 100.0 + 1e-3, "{a}..{b}");
    }
}

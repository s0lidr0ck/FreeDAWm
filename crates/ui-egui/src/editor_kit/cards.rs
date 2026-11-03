//! Cards that float over an editor's display (CleanQ's band card: a body with a pointer bubble
//! that tracks its anchor and flips below when there is no room above) and strips docked along
//! an edge (CleanQ's Ceiling strip).

use super::palette::{VstPalette, al};
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::hash::Hash;

/// A floating card's colours.
#[derive(Clone, Copy, Debug)]
pub struct CardStyle {
    /// The body's fill.
    pub bg: Color32,
    /// The body's outline.
    pub edge: Color32,
    /// The pointer bubble's halo and ring (a band's colour).
    pub bubble: Color32,
}

/// The pointer bubble's radius (`BandCard`'s `ptr`).
pub const POINTER_R: f32 = 12.0;
/// The gap between the anchor and the card.
pub const ANCHOR_GAP: f32 = 4.0;

/// A card `size` (pointer bubble included) floating above `anchor` inside `bounds` (below it when
/// there is no room above and there is room below), x centred on the anchor and kept inside.
/// Paints the bubble, the shadow and the body, swallows clicks on it, then calls `body` with a
/// child `Ui` (faded to 38 % while `faded`, as the card fades while its node is dragged) and the
/// body's rect (the card without the bubble's band). Returns the card's rect.
#[allow(clippy::too_many_arguments)]
pub(crate) fn floating_card(
    ui: &mut Ui,
    salt: impl Hash + std::fmt::Debug,
    pal: &VstPalette,
    style: &CardStyle,
    anchor: Pos2,
    size: egui::Vec2,
    bounds: Rect,
    faded: bool,
    body: impl FnOnce(&mut Ui, Rect),
) -> Rect {
    let _ = pal;
    let (w, h) = (size.x, size.y);
    let mut below = anchor.y - h - ANCHOR_GAP < bounds.top();
    if below && anchor.y + ANCHOR_GAP + h > bounds.bottom() {
        below = false;
    }
    let y = if below { anchor.y + ANCHOR_GAP } else { anchor.y - h - ANCHOR_GAP };
    let y = y.clamp(bounds.top(), (bounds.bottom() - h).max(bounds.top()));
    let x = (anchor.x - w * 0.5).clamp(bounds.left() + 4.0, (bounds.right() - w - 4.0).max(bounds.left() + 4.0));
    let card = Rect::from_min_size(pos2(x, y), vec2(w, h));

    let mut cui = ui.new_child(egui::UiBuilder::new().id_salt(("vst-card", salt)).max_rect(card));
    if faded {
        cui.multiply_opacity(0.38);
    }
    let _ = cui.interact(card, cui.id().with("swallow"), Sense::click_and_drag());
    let p = cui.painter().clone();
    let px = (anchor.x - x).clamp(POINTER_R, w - POINTER_R) + x;
    let py = if below { card.top() + POINTER_R } else { card.bottom() - POINTER_R };
    for i in (1..=4).rev() {
        p.circle_filled(pos2(px, py), POINTER_R + i as f32 * 3.0, al(style.bubble, 0.16 / i as f32));
    }
    p.circle_filled(pos2(px, py), POINTER_R, style.bg);
    p.circle_stroke(pos2(px, py), POINTER_R - 0.5, Stroke::new(1.0, al(style.bubble, 0.55)));
    let b = if below {
        Rect::from_min_max(pos2(card.left(), card.top() + POINTER_R), card.max)
    } else {
        Rect::from_min_max(card.min, pos2(card.right(), card.bottom() - POINTER_R))
    };
    p.rect_filled(b.translate(vec2(0.0, 3.0)), CornerRadius::same(12), al(Color32::BLACK, 0.45));
    p.rect_filled(b, CornerRadius::same(12), al(style.bg, 0.98));
    p.rect_stroke(b.shrink(0.5), CornerRadius::same(12), Stroke::new(1.0, style.edge), StrokeKind::Middle);
    body(&mut cui, b);
    card
}

/// A strip docked along an edge: filled, with a hairline along its top.
pub(crate) fn docked_strip(p: &egui::Painter, rect: Rect, bg: Color32, pal: &VstPalette) {
    p.rect_filled(rect, CornerRadius::ZERO, bg);
    p.line_segment([pos2(rect.left(), rect.top() + 0.5), pos2(rect.right(), rect.top() + 0.5)], Stroke::new(1.0, pal.border));
}

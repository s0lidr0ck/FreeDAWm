//! `CompControls.h` `StyleStrip`: the ten compressors, name and tag inline, the selected one
//! filled; a click (mouse up) picks one.

use super::baseline_text;
use super::model::STYLES;
use crate::editor_kit::VstPalette;
use crate::editor_kit::palette::al;
use crate::theme::{bold, regular};
use egui::{Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

fn cell(r: Rect, i: usize) -> Rect {
    let w = r.width() / STYLES.len() as f32;
    Rect::from_min_size(pos2(r.left() + w * i as f32, r.top()), vec2(w, r.height()))
}

fn index_at(r: Rect, x: f32) -> Option<usize> {
    let i = ((x - r.left()) / (r.width() / STYLES.len() as f32)).floor();
    (i >= 0.0 && (i as usize) < STYLES.len()).then_some(i as usize)
}

/// Paints the strip with `selected` and returns the style clicked.
pub fn style_strip(ui: &mut Ui, r: Rect, pal: &VstPalette, selected: usize) -> Option<usize> {
    let resp = ui.interact(r, ui.id().with("comp-style-strip"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::RadioGroup, true, "Style strip"));
    let hover = resp.hover_pos().and_then(|p| index_at(r, p.x));
    let p = ui.painter();
    for (i, s) in STYLES.iter().enumerate() {
        let c = cell(r, i).shrink2(vec2(1.5, 0.0));
        let on = i == selected;
        let hot = hover == Some(i);
        p.rect_filled(c, CornerRadius::same(5), if on { pal.accent } else { al(Color32::BLACK, 0.22) });
        let edge = if on {
            pal.accent
        } else if hot {
            al(pal.accent, 0.6)
        } else {
            al(Color32::WHITE, 0.09)
        };
        p.rect_stroke(c.shrink(0.5), CornerRadius::same(5), Stroke::new(1.0, edge), StrokeKind::Middle);
        // name and tag sit on one baseline, the tag smaller and dimmer
        let (nf, tf) = (bold(12.0), regular(9.0));
        let nw = p.layout_no_wrap(s.name.to_string(), nf.clone(), Color32::WHITE).size().x;
        let tw = p.layout_no_wrap(s.tag.to_string(), tf.clone(), Color32::WHITE).size().x;
        let gap = 5.0;
        let x = (c.center().x - (nw + gap + tw) * 0.5).round();
        let baseline = (c.center().y + 4.0).round();
        baseline_text(p, pos2(x, baseline), s.name, nf, if on { pal.on_accent } else { pal.text_dim });
        baseline_text(p, pos2(x + nw + gap, baseline), s.tag, tf, if on { al(pal.on_accent, 0.7) } else { pal.text_faint });
    }
    if resp.clicked() { resp.interact_pointer_pos().and_then(|p| index_at(r, p.x)) } else { None }
}

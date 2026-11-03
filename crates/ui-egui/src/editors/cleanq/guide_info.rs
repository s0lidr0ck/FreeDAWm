//! The guide's information, read under the band's (not in the C++ editor): the area's name, its
//! centre and search range, what it sounds like and the usual move as a hint. Shown in the band
//! card under the band's controls when the selected band sits inside a guide's range, and in the
//! guide card for the guide being edited; the graph shows the same guide as its shaded range and
//! labelled chip.

use super::*;
use egui::text::{LayoutJob, TextFormat};

const GAP: f32 = 6.0;
const HEAD_H: f32 = 18.0;
const LINE_GAP: f32 = 3.0;
const BODY: f32 = 11.5;

/// The guide the band at `freq` sits in: the one whose range holds it, nearest its centre (on the
/// octave scale) when several do. None while the guides are hidden.
pub(super) fn guide_at_freq(cx: &Cx, st: &Editor, freq: f64) -> Option<usize> {
    if !st.guides_on || !freq.is_finite() || freq <= 0.0 {
        return None;
    }
    cx.guides()
        .iter()
        .enumerate()
        .filter(|(_, g)| g.lo_hz <= freq && freq <= g.hi_hz)
        .min_by(|(_, a), (_, b)| {
            let d = |g: &Guide| (freq / g.freq_hz.max(1e-9)).ln().abs();
            d(a).total_cmp(&d(b))
        })
        .map(|(k, _)| k)
}

/// A frequency as the templates write it: "20 Hz", "450 Hz", "1.2 kHz", "13 kHz".
pub(super) fn short_freq(hz: f64) -> String {
    if !hz.is_finite() {
        return String::new();
    }
    if hz >= 1000.0 {
        let k = format!("{:.1}", hz / 1000.0);
        format!("{} kHz", k.strip_suffix(".0").unwrap_or(&k))
    } else {
        format!("{} Hz", hz.round() as i64)
    }
}

/// "Centre 450 Hz · 350 Hz – 700 Hz".
pub(super) fn range_text(g: &Guide) -> String {
    format!("Centre {}  \u{00B7}  {} \u{2013} {}", short_freq(g.freq_hz), short_freq(g.lo_hz), short_freq(g.hi_hz))
}

/// A labelled line ("Sounds like  boxy, cardboard"), wrapped to `w`.
fn labelled(p: &egui::Painter, label: &str, text: &str, ink: Color32, w: f32) -> Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    job.append(label, 0.0, TextFormat { font_id: regular(10.5), color: TEXT_FAINT, ..TextFormat::default() });
    job.append(text, 6.0, TextFormat { font_id: regular(BODY), color: ink, ..TextFormat::default() });
    job.wrap.max_width = w.max(40.0);
    p.layout_job(job)
}

/// The lines under the head: the range, then (when there is one) the sound and the usual move.
fn lines(p: &egui::Painter, g: &Guide, w: f32) -> Vec<Arc<egui::Galley>> {
    let mut v = vec![p.layout(range_text(g), regular(BODY), TEXT_DIM, w.max(40.0))];
    if !g.note.is_empty() {
        v.push(labelled(p, "Sounds like", &g.note, TEXT_BRIGHT, w));
    }
    if !g.hint.is_empty() {
        v.push(labelled(p, "Usual move", &g.hint, WARN, w));
    }
    v
}

/// The block's height at width `w` (its gap above included).
pub(super) fn info_h(p: &egui::Painter, g: &Guide, w: f32) -> f32 {
    GAP + 1.0 + GAP + HEAD_H + lines(p, g, w).iter().map(|l| l.size().y + LINE_GAP).sum::<f32>()
}

/// Draws guide `k`'s block from the top of `r` (as tall as [`info_h`]): a rule, the head (the
/// guide's dot and number, its area, "Dynamic" when it wants a dynamic band), then the lines.
pub(super) fn draw_info(p: &egui::Painter, r: Rect, k: usize, g: &Guide) {
    let col = guide_colour(k);
    let mut y = r.top() + GAP;
    p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0, BORDER));
    y += 1.0 + GAP;
    let head = Rect::from_min_size(pos2(r.left(), y), vec2(r.width(), HEAD_H));
    let dot = pos2(head.left() + 6.0, head.center().y);
    p.circle_filled(dot, 7.0, al(col, 0.35));
    p.circle_filled(dot, 4.5, col);
    let tag = format!("Guide {}", k + 1);
    let tag_w = p.layout_no_wrap(tag.clone(), regular(10.5), TEXT_FAINT).size().x;
    p.text(pos2(head.left() + 17.0, head.center().y), Align2::LEFT_CENTER, tag, regular(10.5), TEXT_FAINT);
    let badge_w = if g.dynamic {
        let bw = p.layout_no_wrap("Dynamic".into(), regular(10.0), col).size().x + 10.0;
        let b = Rect::from_min_size(pos2(head.right() - bw, head.top() + 2.0), vec2(bw, HEAD_H - 4.0));
        p.rect_stroke(b, CornerRadius::same(3), Stroke::new(1.0, al(col, 0.7)), StrokeKind::Inside);
        p.text(b.center(), Align2::CENTER_CENTER, "Dynamic", regular(10.0), col);
        bw + 6.0
    } else {
        0.0
    };
    let name = if g.area.is_empty() { short_freq(g.freq_hz) } else { g.area.clone() };
    let x = head.left() + 17.0 + tag_w + 6.0;
    let name_w = (head.right() - badge_w - x).max(10.0);
    let galley = p.layout(name, bold(12.5), col, name_w);
    let row = galley.rows.first().map(|r| r.rect().height()).unwrap_or(14.0);
    // One line: a long area is cut (the graph's chip tooltip shows it whole).
    let clip = Rect::from_min_size(pos2(x, head.top()), vec2(name_w, HEAD_H));
    p.with_clip_rect(clip).galley(pos2(x, head.center().y - row * 0.5), galley, col);
    y += HEAD_H + LINE_GAP;
    for l in lines(p, g, r.width()) {
        let h = l.size().y;
        p.galley(pos2(r.left(), y), l, TEXT_BRIGHT);
        y += h + LINE_GAP;
    }
}

/// The chip's tooltip: everything the guide says, in one block.
pub(super) fn tooltip(ui: &mut Ui, k: usize, g: &Guide) {
    ui.set_max_width(260.0);
    let title = if g.area.is_empty() { format!("Guide {}", k + 1) } else { format!("Guide {}: {}", k + 1, g.area) };
    ui.label(egui::RichText::new(title).strong());
    ui.label(range_text(g));
    if !g.note.is_empty() {
        ui.label(format!("Sounds like: {}", g.note));
    }
    if !g.hint.is_empty() {
        ui.label(format!("Usual move: {}", g.hint));
    }
    if g.dynamic {
        ui.label("Better as a dynamic band.");
    }
    if g.done {
        ui.label("Done.");
    }
    ui.label(egui::RichText::new("Click: a band here. Right-click: edit.").weak());
}

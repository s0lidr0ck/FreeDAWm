//! The amp's face (`paint`): a metal strip with its name and its spec line, dressed per amp
//! (silverface metal, blackface black, JTM gold, Vox copper, Orange orange).

use super::{AMP_AC30, AMP_DELUXE, AMP_ORANGE, AMP_PRINCETON, AMP_SUPER, AMP_TWIN};
use crate::editor_kit::look::tracked_text;
use crate::editor_kit::palette::{al, argb};
use crate::theme::bold;
use egui::{Align2, Color32, Mesh, Painter, Rect, Shape, Stroke, pos2};

/// How the strip is dressed for an amp: its ink, its name (in script, or tracked capitals
/// `name_w` wide) and its spec line (right of `name_w`).
struct Dress {
    ink: Color32,
    name: &'static str,
    script: bool,
    name_w: f32,
    spec: String,
}

fn dress(amp: usize, normal: bool) -> Dress {
    let twin = amp == AMP_TWIN;
    let (deluxe, super_, princeton, vox, orange) = (amp == AMP_DELUXE, amp == AMP_SUPER, amp == AMP_PRINCETON, amp == AMP_AC30, amp == AMP_ORANGE);
    let blackface = deluxe || super_ || princeton;
    let ink = if twin {
        argb(0xFF1A2530)
    } else if blackface {
        argb(0xFFEDE8DE)
    } else {
        argb(0xFF221A0C)
    };
    if orange {
        let spec = "OR120 graphic  -  early 1970s  -  120 W  -  4 x EL34  -  cathodyne  -  4 x 12\" closed back".to_string();
        Dress { ink: argb(0xFF1B1008), name: "ORANGE", script: false, name_w: 150.0, spec }
    } else if vox {
        let spec = "1964  -  JMI  -  top boost  -  30 W  -  4 x EL84, self-biased  -  2 x 12\" alnico".to_string();
        Dress { ink: argb(0xFF24160B), name: "VOX  AC30", script: false, name_w: 190.0, spec }
    } else if blackface {
        let name = if deluxe {
            "Deluxe Reverb"
        } else if super_ {
            "Super Reverb"
        } else {
            "Princeton Reverb"
        };
        let mut spec = String::from(if deluxe {
            "1965  -  AB763  -  22 W  -  2 x 6V6GT  -  1 x 12\""
        } else if super_ {
            "1965  -  AB763  -  45 W  -  2 x 6L6GC  -  4 x 10\""
        } else {
            "1965  -  AB1270  -  12 W  -  2 x 6V6GT  -  1 x 10\"  -  bias tremolo"
        });
        if !princeton {
            spec += if normal { "  -  normal channel" } else { "  -  vibrato channel" };
        }
        Dress { ink, name, script: true, name_w: 250.0, spec }
    } else if twin {
        let spec = format!("1977  -  135 W  -  4 x 6L6GC  -  2 x 12\"  -  {}", if normal { "normal channel" } else { "vibrato channel" });
        Dress { ink, name: "Twin Reverb", script: true, name_w: 220.0, spec }
    } else {
        let spec = "1966  -  model 1962 \"Bluesbreaker\"  -  2 x KT66  -  2 x 12\"  -  both channels".to_string();
        Dress { ink, name: "JTM 45", script: false, name_w: 150.0, spec }
    }
}

/// The metal: the vertical gradient and its brushed lines.
fn metal(p: &Painter, strip: Rect, amp: usize) {
    let twin = amp == AMP_TWIN;
    let (vox, orange) = (amp == AMP_AC30, amp == AMP_ORANGE);
    let blackface = amp == AMP_DELUXE || amp == AMP_SUPER || amp == AMP_PRINCETON;
    let pick = |tw: u32, bf: u32, vx: u32, or: u32, jtm: u32| {
        argb(if twin {
            tw
        } else if blackface {
            bf
        } else if vox {
            vx
        } else if orange {
            or
        } else {
            jtm
        })
    };
    let top = pick(0xFFB9C0C6, 0xFF2A2A2C, 0xFFC0824E, 0xFFF28A2A, 0xFFC9A24A);
    let bot = pick(0xFF8C949B, 0xFF121213, 0xFF7E4E28, 0xFFC4621A, 0xFF9C7A2E);
    // the vertical metal gradient (the C++ rounds the top corners by 5 px; the panel's own
    // rounded outline covers them here)
    let mut mesh = Mesh::default();
    mesh.colored_vertex(strip.left_top(), top);
    mesh.colored_vertex(strip.right_top(), top);
    mesh.colored_vertex(strip.right_bottom(), bot);
    mesh.colored_vertex(strip.left_bottom(), bot);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(Shape::mesh(mesh));
    // brushed lines
    let mut y = strip.top() + 2.0;
    while y < strip.bottom() {
        let yy = y.trunc() + 0.5;
        p.line_segment([pos2(strip.left(), yy), pos2(strip.right(), yy)], Stroke::new(1.0, al(Color32::WHITE, 0.06)));
        y += 3.0;
    }
}

/// The strip in `strip` for `amp`; `normal` is the channel switch on Normal (Twin, Deluxe, Super).
pub fn paint_strip(p: &Painter, strip: Rect, amp: usize, normal: bool) {
    metal(p, strip, amp);
    let d = dress(amp, normal);
    let t = strip.shrink2(egui::vec2(16.0, 0.0));
    let rest = Rect::from_min_max(pos2(t.left() + d.name_w, t.top()), t.max);
    if d.script {
        // `Font (22, bold | italic)`: the UI font has no italic; the script is drawn bold
        p.text(pos2(t.left(), t.center().y), Align2::LEFT_CENTER, d.name, bold(22.0), d.ink);
    } else {
        let name_r = Rect::from_min_size(t.min, egui::vec2(d.name_w, t.height()));
        tracked_text(p, name_r, Align2::LEFT_CENTER, d.name, 20.0, 0.3, true, true, d.ink);
    }
    tracked_text(p, rest, Align2::RIGHT_CENTER, &d.spec, 10.0, 0.1, true, false, al(d.ink, 0.8));
}

/// The strip folded for the slim layout: the name on top, the spec line under it (cut where it
/// does not fit).
pub fn paint_strip_slim(p: &Painter, strip: Rect, amp: usize, normal: bool) {
    metal(p, strip, amp);
    let d = dress(amp, normal);
    let t = strip.shrink2(egui::vec2(10.0, 4.0));
    let name_r = Rect::from_min_size(t.min, egui::vec2(t.width(), 24.0));
    if d.script {
        p.text(pos2(t.left(), name_r.center().y), Align2::LEFT_CENTER, d.name, bold(19.0), d.ink);
    } else {
        tracked_text(p, name_r, Align2::LEFT_CENTER, d.name, 17.0, 0.3, true, true, d.ink);
    }
    let spec_r = Rect::from_min_max(pos2(t.left(), name_r.bottom()), t.max);
    let ink = al(d.ink, 0.8);
    let mut job = egui::text::LayoutJob::simple_singleline(d.spec.replace("  -  ", " \u{00B7} "), bold(9.5), ink);
    job.wrap = egui::text::TextWrapping::truncate_at_width(spec_r.width());
    let g = p.layout_job(job);
    p.with_clip_rect(spec_r.intersect(p.clip_rect())).galley(pos2(spec_r.left(), spec_r.center().y - g.size().y / 2.0), g, ink);
}

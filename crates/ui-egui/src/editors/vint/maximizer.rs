//! The Maximizer's own panel: the BBE 882's front, for one stereo unit. Not in the C++ editor
//! (there the Enhancer is a column of dials for all three types); FreeDAWm's own, since its
//! Maximizer is the 882's circuit (`vint_maximizer`).
//!
//! A black single-space rack front with a blue sweep across it: "882 SONIC MAXIMIZER" at the left,
//! the five output level lamps (-20 to CLIP), LO CONTOUR and PROCESS on silver-capped knobs with
//! 0 to 10 scales, and the FUNCTION button (IN / OUT: the unit's bypass). Where the hardware has
//! its second channel there is one control of ours, TUNE (the filter's capacitors), and above
//! the unit the Enhancer's Type selector, to get back to the other two.

use super::config::{FaceStyle, Mark};
use super::draw::{Fill, Jc, fill_ellipse, fill_round_rect, line, round_rect, stroke_closed, text_c};
use super::{FAMILY, PALETTE, apply_knob, choices, id_of, knob, pv, text_left};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::controls::{SelectionStyle, segmented};
use crate::editor_kit::family::{self, FamilyState};
use crate::editor_kit::look::LABEL_SIZE;
use crate::theme::regular;
use egui::{Mesh, Pos2, Rect, Sense, Shape, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::vint::vint_engine_dsp as d;
use soundcraft_dsp::plugins::fs::vint::vint_maximizer as circuit;

pub(super) const BLURB: &str =
    "The BBE 882, solved from its schematic: the highs leave ahead of the lows. Lo Contour and Process each add up to 15 dB.";
pub(super) const CREDIT: &str = "Component-level model of the BBE 882 Sonic Maximizer's filter and summing network. No affiliation; BBE and Sonic Maximizer are trademarks of their owner.";

const PLATE: Jc = Jc::argb(0xFF0C0C0E);
const SWEEP: Jc = Jc::argb(0xFF1F5FAE);
const INK: Jc = Jc::argb(0xFFF4F5F6);
const CAP: Jc = Jc::argb(0xFFE9EAEC);

// The unit on the 1000 x 300 panel.
const UNIT_TOP: f32 = 66.0;
const UNIT_BOTTOM: f32 = 288.0;
const KNOB_Y: f32 = 166.0;
const KNOB_W: f32 = 124.0;
const LO_X: f32 = 432.0;
const PROCESS_X: f32 = 566.0;
const TUNE_X: f32 = 858.0;
const BUTTON: (f32, f32) = (712.0, 178.0);
/// The level lamps, left to right as the 882's channel A has them, with their dBu.
const LAMPS: [(f32, &str, f32, u32); 5] = [
    (222.0, "CLIP", 20.0, 0xFFFF3B2F),
    (262.0, "+10", 10.0, 0xFF3BE06A),
    (290.0, "0", 0.0, 0xFF3BE06A),
    (318.0, "-10", -10.0, 0xFFF2C94C),
    (346.0, "-20", -20.0, 0xFFF2C94C),
];
const LAMP_Y: f32 = 176.0;
/// A full-scale sine is +18 dBu at the engine's 8.7 V, and its RMS is 3 dB under full scale.
const DBU_OF_RMS_DBFS: f32 = 21.0;

/// The Enhancer on its Maximizer type: this panel is shown.
pub(super) fn shown(host: &Host) -> bool {
    (pv(host, d::P_CIRCUIT).round() as i32) == d::C_ENHANCER && (pv(host, d::P_ENH_TYPE).round() as i32) == 1
}

fn unit(origin: Pos2, scale: f32, x: f32, y: f32) -> Pos2 {
    pos2(origin.x + x * scale, origin.y + y * scale)
}

fn plate_rect(panel: Rect, origin: Pos2, scale: f32) -> Rect {
    Rect::from_min_max(pos2(panel.left() + 4.0, origin.y + UNIT_TOP * scale), pos2(panel.right() - 4.0, origin.y + UNIT_BOTTOM * scale))
}

/// The sweep's two edges at panel x (0..1000), as heights 0 (the unit's bottom) to 1 (its top).
fn sweep_edges(x: f32) -> Option<(f32, f32)> {
    if x < 300.0 {
        return None;
    }
    let upper = if x < 680.0 { ((x - 300.0) / 380.0).powf(0.9) } else { 1.0 };
    let lower = if x < 540.0 { 0.0 } else { 0.78 * (std::f32::consts::PI * ((x - 540.0) / 460.0).min(1.0)).sin().max(0.0).powf(0.5) };
    Some((upper, lower.min(upper)))
}

/// The lamps' level, in dBu: up at once, down 30 dB a second.
pub(super) fn level_step(level: &mut f32, rms: f32, dt: f32) {
    let dbu = if rms.is_finite() && rms > 0.0 { (20.0 * rms.log10()).max(-90.0) + DBU_OF_RMS_DBFS } else { -90.0 };
    let fallen = if level.is_finite() { *level - 30.0 * dt } else { -90.0 };
    *level = dbu.max(fallen).clamp(-90.0, 40.0);
}

/// The unit's front: plate, sweep, rack ears, printing, lamps.
pub(super) fn paint(g: &egui::Painter, host: &Host, panel: Rect, scale: f32, origin: Pos2, level_dbu: f32) {
    let k = scale / 0.95;
    let plate = plate_rect(panel, origin, scale);
    let at = |x: f32, y: f32| unit(origin, scale, x, y);
    fill_round_rect(g, plate, 4.0, Fill::linear(PLATE.brighter(0.10), plate.left_top(), PLATE, plate.left_bottom()));

    // the blue sweep: up from the bottom edge, along the top, and down again at the far right
    let gc = g.with_clip_rect(plate.intersect(g.clip_rect()));
    let mut mesh = Mesh::default();
    let (h, steps) = (plate.height(), 140);
    let lit = SWEEP.brighter(0.12).c32();
    let deep = SWEEP.darker(0.10).c32();
    for i in 0..=steps {
        let x = 300.0 + (1000.0 - 300.0) * i as f32 / steps as f32;
        let (up, low) = sweep_edges(x).unwrap_or((0.0, 0.0));
        let px = origin.x + x * scale;
        mesh.colored_vertex(pos2(px, plate.bottom() - up * h), lit);
        mesh.colored_vertex(pos2(px, plate.bottom() - low * h), deep);
        if i > 0 {
            let b = (2 * i) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b, b + 1);
        }
    }
    gc.add(Shape::mesh(mesh));
    stroke_closed(g, &round_rect(plate, 4.0), 1.0, Jc::WHITE.alpha(0.10));

    // rack ears: the mounting slots and a screw beside each pair
    for x in [30.0, 970.0] {
        for y in [UNIT_TOP + 30.0, UNIT_BOTTOM - 30.0] {
            let slot = Rect::from_center_size(at(x, y), vec2(26.0 * k, 13.0 * k));
            fill_round_rect(g, slot, 6.0 * k, Fill::Solid(Jc::argb(0xFF050506)));
            stroke_closed(g, &round_rect(slot, 6.0 * k), 1.0, Jc::WHITE.alpha(0.22));
        }
    }
    for (x, y) in [(66.0, UNIT_TOP + 30.0), (66.0, UNIT_BOTTOM - 30.0), (936.0, UNIT_TOP + 30.0), (936.0, UNIT_BOTTOM - 30.0)] {
        let c = at(x, y);
        fill_ellipse(
            g,
            Rect::from_center_size(c, vec2(13.0 * k, 13.0 * k)),
            Fill::radial(Jc::argb(0xFF3A3B3E), c - vec2(2.0, 2.0), Jc::argb(0xFF101012), c + vec2(6.0 * k, 6.0 * k)),
        );
        line(g, c - vec2(3.5 * k, 0.0), c + vec2(3.5 * k, 0.0), 1.2, Jc::BLACK.alpha(0.7));
        line(g, c - vec2(0.0, 3.5 * k), c + vec2(0.0, 3.5 * k), 1.2, Jc::BLACK.alpha(0.7));
    }

    // the model, and what the knobs and lamps are
    let label = |x: f32, y: f32, text: &str, size: f32, c: Jc| text_c(g, Rect::from_center_size(at(x, y), vec2(260.0, 20.0)), text, size * k, c);
    label(142.0, 158.0, "882", 40.0, INK);
    label(142.0, 192.0, "SONIC MAXIMIZER", 10.0, INK);
    label(BUTTON.0, 126.0, "PROCESS", 11.5, INK);
    label(BUTTON.0, 246.0, "FUNCTION", 9.5, INK);
    label(264.0, 252.0, "OUTPUT LEVEL", 9.5, INK);
    // the bracket under the lamps and the two knobs, as under each of the 882's channels
    let (x0, x1, y) = (204.0, 640.0, 266.0);
    line(g, at(x0, y - 16.0), at(x0, y), 1.6, INK);
    line(g, at(x0, y), at(x1, y), 1.6, INK);

    for (x, text, dbu, colour) in LAMPS {
        let c = at(x, LAMP_Y);
        let r = 5.2 * k;
        let on = level_dbu >= dbu;
        let hot = Jc::argb(colour);
        if on {
            fill_ellipse(
                g,
                Rect::from_center_size(c, vec2(r * 6.0, r * 6.0)),
                Fill::radial(hot.alpha(0.5), c, hot.alpha(0.0), c + vec2(r * 3.0, 0.0)),
            );
        }
        let (c1, c2) = if on { (hot.brighter(0.5), hot.darker(0.25)) } else { (Jc::argb(0xFF6C6460), Jc::argb(0xFF2E2A28)) };
        fill_ellipse(g, Rect::from_center_size(c, vec2(2.0 * r + 2.0, 2.0 * r + 2.0)), Fill::Solid(Jc::BLACK.alpha(0.6)));
        fill_ellipse(g, Rect::from_center_size(c, vec2(2.0 * r, 2.0 * r)), Fill::radial(c1, c - vec2(r * 0.4, r * 0.4), c2, c + vec2(r, r)));
        label(x, LAMP_Y + 20.0, text, 8.5, INK.alpha(0.9));
    }

    // the FUNCTION button between IN and OUT: the word the unit is at is lit
    let inn = pv(host, d::P_BYPASS) < 0.5;
    label(BUTTON.0 - 46.0, BUTTON.1, "IN", 9.5, if inn { Jc::argb(0xFF6DF08E) } else { INK.alpha(0.55) });
    label(BUTTON.0 + 48.0, BUTTON.1, "OUT", 9.5, if inn { INK.alpha(0.55) } else { Jc::argb(0xFFF2C94C) });
    let b = Rect::from_center_size(at(BUTTON.0, BUTTON.1), vec2(24.0 * k, 24.0 * k));
    fill_round_rect(g, b.expand(2.0), 3.0, Fill::Solid(Jc::BLACK.alpha(0.7)));
    let (c1, c2) = if inn { (Jc::argb(0xFFB9BCC0), Jc::argb(0xFF54575B)) } else { (Jc::argb(0xFF8E9195), Jc::argb(0xFF3C3E42)) };
    fill_round_rect(g, b, 2.0, Fill::linear(c1, b.left_top(), c2, b.right_bottom()));
    let core = b.shrink(6.0 * k);
    fill_round_rect(g, core, 1.5, Fill::Solid(if inn { Jc::argb(0xFF6DF08E).alpha(0.85) } else { Jc::argb(0xFF2A2B2E) }));
}

fn zero_to_ten() -> [Mark; 11] {
    const T: [&str; 11] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
    std::array::from_fn(|i| Mark { pos: i as f32 / 10.0, text: T[i] })
}

/// A pot's boost over its minimum, as the circuit has it.
fn pot_db(series: f64, track: f64, rotation: f32) -> String {
    let db = 20.0 * ((series + track) / (series + circuit::pot_ohms(track, f64::from(rotation)))).log10();
    format!("+{db:.1} dB")
}

fn hz(f: f64) -> String {
    if f >= 1000.0 { format!("{:.1} kHz", f / 1000.0) } else { format!("{} Hz", f.round() as i64) }
}

/// The Type selector's place above the unit.
fn type_rects(panel: Rect) -> (Rect, Rect) {
    let top = panel.top() + 14.0;
    (Rect::from_min_size(pos2(panel.left() + 18.0, top), vec2(40.0, 26.0)), Rect::from_min_size(pos2(panel.left() + 62.0, top), vec2(330.0, 26.0)))
}

/// The unit's controls: the two pots, TUNE, the FUNCTION button, and the Type selector.
pub(super) fn controls(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut FamilyState, panel: Rect, scale: f32, origin: Pos2) {
    let pal = &PALETTE;
    let (type_label, type_seg) = type_rects(panel);
    text_left(ui.painter(), type_label, "Type", regular(LABEL_SIZE), pal.text_body);
    let opts = choices(host, d::P_ENH_TYPE);
    if let Some(i) = segmented(ui, type_seg, "vint-maximizer-type", pal, opts, 1, Some(1), SelectionStyle::Filled)
        && i != 1
    {
        family::set(app, host, st, &FAMILY, id_of(d::P_ENH_TYPE), i as f32);
    }

    let scale_marks = zero_to_ten();
    // TUNE reads the filter's upper crossover: 3.2 kHz with the 882's own capacitors.
    let tune_marks = [
        Mark { pos: 0.0, text: "1k" },
        Mark { pos: 0.25, text: "1.8" },
        Mark { pos: 0.5, text: "3.2" },
        Mark { pos: 0.75, text: "5.7" },
        Mark { pos: 1.0, text: "10k" },
    ];
    let knobs: [(usize, f32, &str, &[Mark], i32); 3] = [
        (d::P_ENH_LOW, LO_X, "Lo Contour", &scale_marks, 20),
        (d::P_ENH_AMOUNT, PROCESS_X, "Process", &scale_marks, 20),
        (d::P_ENH_TUNE, TUNE_X, "Tune", &tune_marks, 8),
    ];
    for (param, x, name, marks, minor) in knobs {
        let v = pv(host, param);
        let readout = match param {
            d::P_ENH_LOW => pot_db(circuit::R14, circuit::VR2, v),
            d::P_ENH_AMOUNT => pot_db(circuit::R1, circuit::VR1, v),
            _ => hz(3208.0 / circuit::cap_scale(v)),
        };
        let w = (KNOB_W * scale).round();
        let c = unit(origin, scale, x, KNOB_Y);
        let top = (c.y - knob::centre_offset(w, FaceStyle::Silver, false, true)).round();
        let r = Rect::from_min_size(pos2((c.x - w * 0.5).round(), top), vec2(w, knob::height_for(w, FaceStyle::Silver, false, true)));
        let face = knob::KnobFace {
            style: FaceStyle::Silver,
            cap: CAP,
            ink: INK,
            marks,
            positions: 0,
            minor,
            travel: None,
            dots: false,
            name,
            name_below: true,
        };
        let default = host.info.param(id_of(param)).map_or(0.5, |p| p.default);
        let out = knob::vint_knob(ui, r, param, &face, v, default, &readout, true);
        apply_knob(app, host, st, param, 0, &out);
    }

    // FUNCTION: the button, and the words either side of it
    let k = scale / 0.95;
    let hit = Rect::from_center_size(unit(origin, scale, BUTTON.0, BUTTON.1), vec2(34.0 * k, 34.0 * k));
    let inn = pv(host, d::P_BYPASS) < 0.5;
    let resp = ui.interact(hit, ui.id().with("vint-maximizer-function"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, inn, "Function"));
    if resp.clicked() {
        family::pill_set(app, host, st, &FAMILY, id_of(d::P_BYPASS), if inn { 1.0 } else { 0.0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_stays_inside_the_unit() {
        assert!(sweep_edges(299.0).is_none());
        for i in 0..=700 {
            let (up, low) = sweep_edges(300.0 + i as f32).unwrap();
            assert!((0.0..=1.0).contains(&up) && (0.0..=1.0).contains(&low) && low <= up, "{i}: {up} {low}");
        }
        // along the top over the black arch, back to the bottom edge at the far right
        let (up, low) = sweep_edges(800.0).unwrap();
        assert!(up == 1.0 && low > 0.6 && low < 0.8);
        assert!(sweep_edges(1000.0).unwrap().1 < 0.01);
    }

    #[test]
    fn readouts_are_the_circuits() {
        assert_eq!(pot_db(circuit::R1, circuit::VR1, 0.0), "+0.0 dB");
        assert_eq!(pot_db(circuit::R1, circuit::VR1, 1.0), "+15.6 dB");
        assert_eq!(hz(3208.0 / circuit::cap_scale(0.5)), "3.2 kHz");
        assert_eq!(hz(157.0), "157 Hz");
    }

    #[test]
    fn the_lamps_rise_at_once_and_fall_slowly() {
        let mut level = -90.0;
        // a full-scale sine's RMS: +18 dBu
        level_step(&mut level, std::f32::consts::FRAC_1_SQRT_2, 1.0 / 30.0);
        assert!((level - 18.0).abs() < 0.1, "{level}");
        level_step(&mut level, 0.0, 0.5);
        assert!((level - 3.0).abs() < 0.1, "{level}");
        level_step(&mut level, f32::NAN, 10.0);
        assert_eq!(level, -90.0);
    }
}

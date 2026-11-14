//! The Maximizer's own panel: the BBE 882's front, for one stereo unit. Not in the C++ editor
//! (there the Enhancer is a column of dials for all three types); FreeDAWm's own, since its
//! Maximizer is the 882's circuit (`vint_maximizer`).
//!
//! A black single-space rack front with a blue sweep across it: "882 SONIC MAXIMIZER" at the left,
//! the five output level lamps (-20 to CLIP), LO CONTOUR and PROCESS on silver-capped knobs with
//! 0 to 10 scales, and the FUNCTION button (IN / OUT: the unit's bypass). Where the hardware has
//! its second channel there is one control of ours, TUNE (the filter's capacitors), and above
//! the unit the Enhancer's Type selector, to get back to the other two.
//!
//! Two layouts ([`Lay`]): across the wide editor's panel, and for the slim editor the same unit
//! turned on its side, its left end at the top, everything on it still upright.

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

// The unit on the wide editor's 1000 x 300 panel.
const UNIT_TOP: f32 = 66.0;
const UNIT_BOTTOM: f32 = 288.0;
const KNOB_Y: f32 = 166.0;
const KNOB_W: f32 = 124.0;
/// The level lamps, left to right as the 882's channel A has them, with their dBu.
const LAMPS: [(f32, &str, f32, u32); 5] = [
    (222.0, "CLIP", 20.0, 0xFFFF3B2F),
    (262.0, "+10", 10.0, 0xFF3BE06A),
    (290.0, "0", 0.0, 0xFF3BE06A),
    (318.0, "-10", -10.0, 0xFFF2C94C),
    (346.0, "-20", -20.0, 0xFFF2C94C),
];
/// A full-scale sine is +18 dBu at the engine's 8.7 V, and its RMS is 3 dB under full scale.
const DBU_OF_RMS_DBFS: f32 = 21.0;
/// The slim layout: the turned unit's length, and the Type selector's row above it.
const SLIM_H: f32 = 556.0;
const SLIM_TYPE_H: f32 = 26.0;

/// The Enhancer on its Maximizer type: this panel is shown.
pub(super) fn shown(host: &Host) -> bool {
    (pv(host, d::P_CIRCUIT).round() as i32) == d::C_ENHANCER && (pv(host, d::P_ENH_TYPE).round() as i32) == 1
}

/// Where everything on the unit is, in points.
pub(super) struct Lay {
    plate: Rect,
    /// the printing's and hardware's size (1 at the wide editor's default size)
    k: f32,
    /// the unit runs top to bottom (the slim editor)
    turned: bool,
    /// "882" (SONIC MAXIMIZER under it)
    title: Pos2,
    lamps: [Pos2; 5],
    level_label: Pos2,
    /// the rule under the lamps and the two knobs (the wide layout)
    bracket: Option<[Pos2; 3]>,
    /// LO CONTOUR, PROCESS, TUNE
    knobs: [Pos2; 3],
    knob_w: f32,
    process_label: Pos2,
    button: Pos2,
    function_label: Pos2,
    slots: [Pos2; 4],
    screws: [Pos2; 4],
}

impl Lay {
    /// Across the wide editor's panel.
    pub(super) fn wide(panel: Rect, scale: f32, origin: Pos2) -> Lay {
        let at = |x: f32, y: f32| pos2(origin.x + x * scale, origin.y + y * scale);
        let (ear_top, ear_bottom) = (UNIT_TOP + 30.0, UNIT_BOTTOM - 30.0);
        Lay {
            plate: Rect::from_min_max(
                pos2(panel.left() + 4.0, origin.y + UNIT_TOP * scale),
                pos2(panel.right() - 4.0, origin.y + UNIT_BOTTOM * scale),
            ),
            k: scale / 0.95,
            turned: false,
            title: at(142.0, 158.0),
            lamps: LAMPS.map(|l| at(l.0, 176.0)),
            level_label: at(264.0, 252.0),
            bracket: Some([at(204.0, 250.0), at(204.0, 266.0), at(640.0, 266.0)]),
            knobs: [at(432.0, KNOB_Y), at(566.0, KNOB_Y), at(858.0, KNOB_Y)],
            knob_w: (KNOB_W * scale).round(),
            process_label: at(712.0, 126.0),
            button: at(712.0, 178.0),
            function_label: at(712.0, 246.0),
            slots: [at(30.0, ear_top), at(30.0, ear_bottom), at(970.0, ear_top), at(970.0, ear_bottom)],
            screws: [at(66.0, ear_top), at(66.0, ear_bottom), at(936.0, ear_top), at(936.0, ear_bottom)],
        }
    }

    /// Turned on its side for the slim editor: `w` wide from `top_left`, the unit's left end up.
    pub(super) fn slim(top_left: Pos2, w: f32) -> Lay {
        let at = |x: f32, y: f32| pos2(top_left.x + x, top_left.y + y);
        let mid = w / 2.0;
        let lamp_x = [-62.0, -22.0, 6.0, 34.0, 62.0];
        Lay {
            plate: Rect::from_min_size(top_left, vec2(w, SLIM_H)),
            k: 1.0,
            turned: true,
            title: at(mid, 62.0),
            lamps: lamp_x.map(|dx| at(mid + dx, 128.0)),
            level_label: at(mid, 170.0),
            bracket: None,
            knobs: [at(w * 0.27, 250.0), at(w * 0.73, 250.0), at(mid, 468.0)],
            knob_w: 118.0,
            process_label: at(mid, 338.0),
            button: at(mid, 366.0),
            function_label: at(mid, 396.0),
            slots: [at(26.0, 16.0), at(w - 26.0, 16.0), at(26.0, SLIM_H - 16.0), at(w - 26.0, SLIM_H - 16.0)],
            screws: [at(60.0, 16.0), at(w - 60.0, 16.0), at(60.0, SLIM_H - 16.0), at(w - 60.0, SLIM_H - 16.0)],
        }
    }

    /// A point of the sweep: `u` along the unit (0..1 from its left end), `v` across it (0 at its
    /// bottom edge, 1 at its top).
    fn across(&self, u: f32, v: f32) -> Pos2 {
        let p = self.plate;
        if self.turned {
            pos2(p.right() - v * p.width(), p.top() + u * p.height())
        } else {
            pos2(p.left() + u * p.width(), p.bottom() - v * p.height())
        }
    }

    fn knob_rect(&self, i: usize) -> Rect {
        let (c, w) = (self.knobs[i], self.knob_w);
        let top = (c.y - knob::centre_offset(w, FaceStyle::Silver, false, true)).round();
        Rect::from_min_size(pos2((c.x - w * 0.5).round(), top), vec2(w, knob::height_for(w, FaceStyle::Silver, false, true)))
    }
}

/// The sweep's two edges at `x` along the unit (0..1000), as heights 0 (its bottom) to 1 (its top).
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
pub(super) fn paint(g: &egui::Painter, host: &Host, lay: &Lay, level_dbu: f32) {
    let (k, plate) = (lay.k, lay.plate);
    let shade_to = if lay.turned { plate.right_top() } else { plate.left_bottom() };
    fill_round_rect(g, plate, 4.0, Fill::linear(PLATE.brighter(0.10), plate.left_top(), PLATE, shade_to));

    // the blue sweep: up from the bottom edge, along the top, and down again at the far end
    let gc = g.with_clip_rect(plate.intersect(g.clip_rect()));
    let mut mesh = Mesh::default();
    let steps = 140;
    let lit = SWEEP.brighter(0.12).c32();
    let deep = SWEEP.darker(0.10).c32();
    for i in 0..=steps {
        let x = 300.0 + (1000.0 - 300.0) * i as f32 / steps as f32;
        let (up, low) = sweep_edges(x).unwrap_or((0.0, 0.0));
        mesh.colored_vertex(lay.across(x / 1000.0, up), lit);
        mesh.colored_vertex(lay.across(x / 1000.0, low), deep);
        if i > 0 {
            let b = (2 * i) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b, b + 1);
        }
    }
    gc.add(Shape::mesh(mesh));
    stroke_closed(g, &round_rect(plate, 4.0), 1.0, Jc::WHITE.alpha(0.10));

    // rack ears: the mounting slots and a screw beside each
    for c in lay.slots {
        let slot = Rect::from_center_size(c, vec2(26.0 * k, 13.0 * k));
        fill_round_rect(g, slot, 6.0 * k, Fill::Solid(Jc::argb(0xFF050506)));
        stroke_closed(g, &round_rect(slot, 6.0 * k), 1.0, Jc::WHITE.alpha(0.22));
    }
    for c in lay.screws {
        fill_ellipse(
            g,
            Rect::from_center_size(c, vec2(13.0 * k, 13.0 * k)),
            Fill::radial(Jc::argb(0xFF3A3B3E), c - vec2(2.0, 2.0), Jc::argb(0xFF101012), c + vec2(6.0 * k, 6.0 * k)),
        );
        line(g, c - vec2(3.5 * k, 0.0), c + vec2(3.5 * k, 0.0), 1.2, Jc::BLACK.alpha(0.7));
        line(g, c - vec2(0.0, 3.5 * k), c + vec2(0.0, 3.5 * k), 1.2, Jc::BLACK.alpha(0.7));
    }

    // the model, and what the knobs and lamps are
    let label = |at: Pos2, text: &str, size: f32, c: Jc| text_c(g, Rect::from_center_size(at, vec2(260.0, 20.0)), text, size * k, c);
    label(lay.title, "882", 40.0, INK);
    label(lay.title + vec2(0.0, 32.5 * k), "SONIC MAXIMIZER", 10.0, INK);
    label(lay.process_label, "PROCESS", 11.5, INK);
    label(lay.function_label, "FUNCTION", 9.5, INK);
    label(lay.level_label, "OUTPUT LEVEL", 9.5, INK);
    // the bracket under the lamps and the two knobs, as under each of the 882's channels
    if let Some([a, b, c]) = lay.bracket {
        line(g, a, b, 1.6, INK);
        line(g, b, c, 1.6, INK);
    }

    for ((_, text, dbu, colour), c) in LAMPS.into_iter().zip(lay.lamps) {
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
        label(c + vec2(0.0, 19.0 * k), text, 8.5, INK.alpha(0.9));
    }

    // the FUNCTION button between IN and OUT: the word the unit is at is lit
    let inn = pv(host, d::P_BYPASS) < 0.5;
    label(lay.button - vec2(44.0 * k, 0.0), "IN", 9.5, if inn { Jc::argb(0xFF6DF08E) } else { INK.alpha(0.55) });
    label(lay.button + vec2(46.0 * k, 0.0), "OUT", 9.5, if inn { INK.alpha(0.55) } else { Jc::argb(0xFFF2C94C) });
    let b = Rect::from_center_size(lay.button, vec2(24.0 * k, 24.0 * k));
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

/// The Enhancer's Type selector for a unit that has the panel to itself (`current`: its type),
/// in `seg`, with "Type" printed in `label` when there is room for it.
pub(super) fn type_selector(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut FamilyState, label: Option<Rect>, seg: Rect, current: usize) {
    let pal = &PALETTE;
    if let Some(label) = label {
        text_left(ui.painter(), label, "Type", regular(LABEL_SIZE), pal.text_body);
    }
    let opts = choices(host, d::P_ENH_TYPE);
    if let Some(i) = segmented(ui, seg, "vint-enhancer-type", pal, opts, 1, Some(current), SelectionStyle::Filled)
        && i != current
    {
        family::set(app, host, st, &FAMILY, id_of(d::P_ENH_TYPE), i as f32);
    }
}

/// The Type selector's place above the unit in the wide editor: (its label, the selector).
pub(super) fn wide_type_rects(panel: Rect) -> (Option<Rect>, Rect) {
    let top = panel.top() + 14.0;
    (
        Some(Rect::from_min_size(pos2(panel.left() + 18.0, top), vec2(40.0, 26.0))),
        Rect::from_min_size(pos2(panel.left() + 62.0, top), vec2(330.0, 26.0)),
    )
}

/// The unit's controls: the two pots, TUNE and the FUNCTION button.
pub(super) fn controls(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut FamilyState, lay: &Lay) {
    let scale_marks = zero_to_ten();
    // TUNE reads the filter's upper crossover: 3.2 kHz with the 882's own capacitors.
    let tune_marks = [
        Mark { pos: 0.0, text: "1k" },
        Mark { pos: 0.25, text: "1.8" },
        Mark { pos: 0.5, text: "3.2" },
        Mark { pos: 0.75, text: "5.7" },
        Mark { pos: 1.0, text: "10k" },
    ];
    let knobs: [(usize, &str, &[Mark], i32); 3] =
        [(d::P_ENH_LOW, "Lo Contour", &scale_marks, 20), (d::P_ENH_AMOUNT, "Process", &scale_marks, 20), (d::P_ENH_TUNE, "Tune", &tune_marks, 8)];
    for (i, (param, name, marks, minor)) in knobs.into_iter().enumerate() {
        let v = pv(host, param);
        let readout = match param {
            d::P_ENH_LOW => pot_db(circuit::R14, circuit::VR2, v),
            d::P_ENH_AMOUNT => pot_db(circuit::R1, circuit::VR1, v),
            _ => hz(3208.0 / circuit::cap_scale(v)),
        };
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
        let out = knob::vint_knob(ui, lay.knob_rect(i), param, &face, v, default, &readout, true);
        apply_knob(app, host, st, param, 0, &out);
    }

    // FUNCTION: the button, and the words either side of it
    let hit = Rect::from_center_size(lay.button, vec2(34.0 * lay.k, 34.0 * lay.k));
    let inn = pv(host, d::P_BYPASS) < 0.5;
    let resp = ui.interact(hit, ui.id().with("vint-maximizer-function"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, inn, "Function"));
    if resp.clicked() {
        family::pill_set(app, host, st, &FAMILY, id_of(d::P_BYPASS), if inn { 1.0 } else { 0.0 });
    }
}

/// The slim editor's section: the Type selector, then the unit turned on its side, in a column
/// `w` wide from `top_left`. Returns its bottom.
pub(super) fn slim(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut FamilyState, top_left: Pos2, w: f32, level_dbu: f32) -> f32 {
    type_selector(app, ui, host, st, None, Rect::from_min_size(top_left, vec2(w, SLIM_TYPE_H)), 1);
    let lay = Lay::slim(top_left + vec2(0.0, SLIM_TYPE_H + 8.0), w);
    paint(&ui.painter().clone(), host, &lay, level_dbu);
    controls(app, ui, host, st, &lay);
    lay.plate.bottom()
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
    fn both_layouts_keep_everything_on_the_plate() {
        let wide = Lay::wide(Rect::from_min_size(pos2(24.0, 120.0), vec2(952.0, 301.0)), 0.952, pos2(24.0, 127.2));
        let slim = Lay::slim(pos2(10.0, 200.0), 260.0);
        for (name, lay) in [("wide", &wide), ("slim", &slim)] {
            let inside = |p: Pos2| lay.plate.shrink(4.0).contains(p);
            assert!(lay.lamps.iter().all(|p| inside(*p)) && inside(lay.title) && inside(lay.button) && inside(lay.function_label), "{name}");
            for i in 0..3 {
                let r = lay.knob_rect(i);
                assert!(lay.plate.contains_rect(r.shrink(2.0)), "{name}: knob {i} {r:?} in {:?}", lay.plate);
                assert!((0..i).all(|j| !lay.knob_rect(j).shrink(6.0).intersects(r.shrink(6.0))), "{name}: knobs {i} overlap");
            }
            // the sweep's corners are the plate's
            assert!(lay.plate.expand(0.5).contains(lay.across(0.0, 0.0)) && lay.plate.expand(0.5).contains(lay.across(1.0, 1.0)), "{name}");
        }
        assert!(!wide.turned && slim.turned && slim.plate.width() == 260.0);
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

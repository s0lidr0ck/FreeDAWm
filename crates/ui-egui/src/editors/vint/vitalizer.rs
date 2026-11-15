//! The Vitalizer's own panel: the SPL Vitalizer Mk3-T's front. Not in the C++ editor; FreeDAWm's
//! own, since its Vitalizer has the hardware's eight controls (`vint_vitalizer`).
//!
//! A black single-space rack front, the knobs in the hardware's order and sections, each with
//! its printed scale: DRIVE (dB) with its OVL lamp | BASS SOUND (Soft, 0, Tight) and BASS COMP
//! with its GR lamp | MID-HI TUNE (kHz) and PROCESS | LC-EQ and INTENSITY | STEREO EXPANDER |
//! the name over the valve window, and the ON button (the unit's bypass). Dotted rules part the
//! sections. Above the unit, the Enhancer's Type selector.
//!
//! Two layouts ([`Lay`]): across the wide editor's panel, and for the slim editor the same unit
//! turned on its side: a section to a row, its two knobs side by side, everything upright.

use super::config::{FaceStyle, Mark, format_knob_for};
use super::draw::{Fill, Jc, fill_ellipse, fill_round_rect, line, round_rect, stroke_closed, text_c};
use super::{FAMILY, apply_knob, id_of, knob, maximizer, pv};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::family::{self, FamilyState};
use egui::{Pos2, Rect, Sense, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::vint::vint_engine_dsp as d;

const PLATE: Jc = Jc::argb(0xFF101011);
const INK: Jc = Jc::argb(0xFFF2F2F0);
const CAP: Jc = Jc::argb(0xFF1B1B1D);
const ORANGE: Jc = Jc::argb(0xFFFF8A1F);

// The unit on the wide editor's 1000 x 300 panel.
const UNIT_TOP: f32 = 66.0;
const UNIT_BOTTOM: f32 = 288.0;
const KNOB_Y: f32 = 190.0;
const KNOB_W: f32 = 104.0;
/// The printed scale's radius past the knob's edge (points): its dots, then its figures.
const DOT_PAST: f32 = 6.0;
const FIGURE_PAST: f32 = 15.0;
const WINDOW: (f32, f32) = (853.0, 176.0);
const ON: (f32, f32) = (946.0, 196.0);
/// The dotted rules between the sections.
const RULES: [f32; 6] = [112.0, 306.0, 500.0, 694.0, 790.0, 916.0];
/// The lamps beside Drive and Bass Comp, from their knobs' centres.
const LAMP_FROM_KNOB: Vec2 = vec2(38.0, -82.0);
/// The slim layout: a row's height, the margin above the first, and the Type selector's row.
const SLIM_ROW_H: f32 = 124.0;
const SLIM_MARGIN: f32 = 10.0;
const SLIM_TYPE_H: f32 = 26.0;

#[derive(Clone, Copy, PartialEq)]
enum Scale {
    DriveDb,
    Bass,
    OffMax,
    TuneKhz,
    LowHigh,
}

/// The eight knobs, left to right as on the hardware: parameter, x, name, scale.
const KNOBS: [(usize, f32, &str, Scale); 8] = [
    (d::P_VIT_DRIVE, 58.0, "Drive", Scale::DriveDb),
    (d::P_VIT_BASS, 162.0, "Bass Sound", Scale::Bass),
    (d::P_VIT_COMP, 256.0, "Bass Comp", Scale::OffMax),
    (d::P_ENH_TUNE, 356.0, "Mid-Hi Tune", Scale::TuneKhz),
    (d::P_ENH_AMOUNT, 450.0, "Process", Scale::OffMax),
    (d::P_VIT_LC, 550.0, "LC-EQ", Scale::LowHigh),
    (d::P_VIT_INTENSITY, 644.0, "Intensity", Scale::OffMax),
    (d::P_VIT_WIDTH, 742.0, "Stereo Exp.", Scale::OffMax),
];

fn marks(scale: Scale) -> Vec<Mark> {
    let even = |t: [&'static str; 11]| t.iter().enumerate().map(|(i, text)| Mark { pos: i as f32 / 10.0, text }).collect();
    match scale {
        Scale::DriveDb => even(["-20", "-13", "-8", "-5", "-2", "0", "1.5", "3", "4", "5", "6"]),
        Scale::TuneKhz => even(["22", "14", "8.5", "6", "4", "2.8", "2.2", "1.8", "1.5", "1.3", "1.1"]),
        Scale::OffMax => even(["OFF", "1", "2", "3", "4", "5", "6", "7", "8", "9", "MAX"]),
        Scale::LowHigh => even(["LOW", "1", "2", "3", "4", "5", "6", "7", "8", "9", "HIGH"]),
        Scale::Bass => vec![Mark { pos: 0.5, text: "0" }],
    }
}

/// The Enhancer on its Vitalizer type: this panel is shown.
pub(super) fn shown(host: &Host) -> bool {
    (pv(host, d::P_CIRCUIT).round() as i32) == d::C_ENHANCER && (pv(host, d::P_ENH_TYPE).round() as i32) == 2
}

/// Where everything on the unit is, in points.
pub(super) struct Lay {
    plate: Rect,
    /// the printing's and hardware's size (1 at the wide editor's default size)
    k: f32,
    /// the eight knobs, in [`KNOBS`]' order
    knobs: [Pos2; 8],
    knob_w: f32,
    ovl: Pos2,
    gr: Pos2,
    /// the dotted rules between the sections
    rules: Vec<(Pos2, Pos2)>,
    /// "dB" under Drive, "kHz" under Mid-Hi Tune
    units: [Pos2; 2],
    name: Pos2,
    model: Pos2,
    window: Rect,
    on: Pos2,
}

impl Lay {
    /// Across the wide editor's panel.
    pub(super) fn wide(panel: Rect, scale: f32, origin: Pos2) -> Lay {
        let at = |x: f32, y: f32| pos2(origin.x + x * scale, origin.y + y * scale);
        let knobs = KNOBS.map(|k| at(k.1, KNOB_Y));
        Lay {
            plate: Rect::from_min_max(
                pos2(panel.left() + 4.0, origin.y + UNIT_TOP * scale),
                pos2(panel.right() - 4.0, origin.y + UNIT_BOTTOM * scale),
            ),
            k: scale / 0.95,
            knobs,
            knob_w: (KNOB_W * scale).round(),
            ovl: knobs[0] + LAMP_FROM_KNOB * scale,
            gr: knobs[2] + LAMP_FROM_KNOB * scale,
            rules: RULES.iter().map(|x| (at(*x, UNIT_TOP + 12.0), at(*x, UNIT_BOTTOM - 12.0))).collect(),
            units: [at(KNOBS[0].1, KNOB_Y + 46.0), at(KNOBS[3].1, KNOB_Y + 46.0)],
            name: at(WINDOW.0, 112.0),
            model: at(WINDOW.0, 246.0),
            window: Rect::from_center_size(at(WINDOW.0, WINDOW.1), vec2(112.0 * scale, 62.0 * scale)),
            on: at(ON.0, ON.1),
        }
    }

    /// Turned on its side for the slim editor: `w` wide from `top_left`. Five rows, a section
    /// each: Drive and the ON button; Bass Sound, Bass Comp; Mid-Hi Tune, Process; LC-EQ,
    /// Intensity; Stereo Expander and the valve window.
    pub(super) fn slim(top_left: Pos2, w: f32) -> Lay {
        let (left, right) = (top_left.x + (w * 0.27).round(), top_left.x + (w * 0.73).round());
        let row = |i: usize| top_left.y + SLIM_MARGIN + SLIM_ROW_H * i as f32 + 78.0;
        let knobs = [
            pos2(left, row(0)),
            pos2(left, row(1)),
            pos2(right, row(1)),
            pos2(left, row(2)),
            pos2(right, row(2)),
            pos2(left, row(3)),
            pos2(right, row(3)),
            pos2(left, row(4)),
        ];
        let lamp = vec2(46.0, -48.0);
        let height = 2.0 * SLIM_MARGIN + 5.0 * SLIM_ROW_H;
        Lay {
            plate: Rect::from_min_size(top_left, vec2(w, height)),
            k: 1.0,
            knobs,
            knob_w: 99.0,
            ovl: knobs[0] + lamp,
            gr: knobs[2] + lamp,
            rules: (1..5)
                .map(|i| {
                    let y = top_left.y + SLIM_MARGIN + SLIM_ROW_H * i as f32 + 2.0;
                    (pos2(top_left.x + 12.0, y), pos2(top_left.x + w - 12.0, y))
                })
                .collect(),
            units: [knobs[0] + vec2(0.0, 40.0), knobs[3] + vec2(0.0, 40.0)],
            name: pos2(right, row(4) - 50.0),
            model: pos2(right, row(4) + 44.0),
            window: Rect::from_center_size(pos2(right, row(4) - 2.0), vec2(104.0, 54.0)),
            on: pos2(right, row(0) + 10.0),
        }
    }

    fn knob_rect(&self, i: usize) -> Rect {
        let (c, w) = (self.knobs[i], self.knob_w);
        let top = (c.y - knob::centre_offset(w, FaceStyle::Round, false, false)).round();
        Rect::from_min_size(pos2((c.x - w * 0.5).round(), top), vec2(w, knob::height_for(w, FaceStyle::Round, false, false)))
    }
}

/// A lamp is lit: (OVL, GR) from the engine's readouts. OVL comes on 3 dB before full scale, as
/// the hardware's does before an overload; GR while the bass compressor is holding anything.
pub(super) fn lamps(readouts: Option<[f32; 8]>) -> (bool, bool) {
    let r = readouts.unwrap_or([f32::NAN; 8]);
    (r[5].is_finite() && r[5] >= -3.0, r[4].is_finite() && r[4] >= 0.5)
}

fn lamp(g: &egui::Painter, c: Pos2, r: f32, hot: Jc, on: bool) {
    if on {
        fill_ellipse(g, Rect::from_center_size(c, vec2(r * 6.0, r * 6.0)), Fill::radial(hot.alpha(0.5), c, hot.alpha(0.0), c + vec2(r * 3.0, 0.0)));
    }
    let (c1, c2) = if on { (hot.brighter(0.5), hot.darker(0.2)) } else { (hot.darker(0.55), hot.darker(0.8)) };
    fill_ellipse(g, Rect::from_center_size(c, vec2(2.0 * r + 2.0, 2.0 * r + 2.0)), Fill::Solid(Jc::BLACK.alpha(0.7)));
    fill_ellipse(g, Rect::from_center_size(c, vec2(2.0 * r, 2.0 * r)), Fill::radial(c1, c - vec2(r * 0.4, r * 0.4), c2, c + vec2(r, r)));
}

/// The unit's front: plate, rules, printing, lamps, the valve window and the ON button.
pub(super) fn paint(g: &egui::Painter, host: &Host, lay: &Lay, lit: (bool, bool)) {
    let (k, plate) = (lay.k, lay.plate);
    // black, with the faint sheen across the middle the anodised front has
    fill_round_rect(g, plate, 5.0, Fill::Solid(PLATE));
    let mid = plate.center().y;
    for (a, b) in [(plate.top() + 6.0, mid), (mid, plate.bottom() - 6.0)] {
        let band = Rect::from_min_max(pos2(plate.left(), a.min(b)), pos2(plate.right(), a.max(b)));
        let (edge, centre) = (Jc::WHITE.alpha(0.0), Jc::WHITE.alpha(0.055));
        let (c1, c2) = if a < mid { (edge, centre) } else { (centre, edge) };
        fill_round_rect(g, band, 0.0, Fill::linear(c1, band.left_top(), c2, band.left_bottom()));
    }
    line(g, pos2(plate.left() + 4.0, plate.top() + 2.0), pos2(plate.right() - 4.0, plate.top() + 2.0), 2.0, Jc::WHITE.alpha(0.12));
    stroke_closed(g, &round_rect(plate, 5.0), 1.0, Jc::WHITE.alpha(0.10));

    for (a, b) in &lay.rules {
        let n = ((*b - *a).length() / (6.0 * k)).round().max(1.0) as usize;
        for i in 0..=n {
            let c = *a + (*b - *a) * (i as f32 / n as f32);
            fill_ellipse(g, Rect::from_center_size(c, vec2(1.6 * k, 1.6 * k)), Fill::Solid(INK.alpha(0.55)));
        }
    }

    let label = |at: Pos2, text: &str, size: f32, c: Jc| text_c(g, Rect::from_center_size(at, vec2(260.0, 20.0)), text, size * k, c);
    // each knob's name, and its scale printed small around it: a dot, then the figure
    let r = knob::radius_for(lay.knob_w, FaceStyle::Round, false);
    let (dot_r, figure_r) = (r + DOT_PAST * k, r + FIGURE_PAST * k);
    for ((_, _, name, kind), c) in KNOBS.into_iter().zip(lay.knobs) {
        let on_ring = |pos: f32, rad: f32| {
            let a = (-135.0 + 270.0 * pos).to_radians();
            pos2(c.x + a.sin() * rad, c.y - a.cos() * rad)
        };
        text_c(g, Rect::from_center_size(pos2(c.x, c.y - figure_r - 17.0 * k), vec2(200.0, 16.0)), &name.to_uppercase(), 10.0 * k, INK);
        for m in marks(kind) {
            if kind != Scale::Bass {
                fill_ellipse(g, Rect::from_center_size(on_ring(m.pos, dot_r), vec2(2.2 * k, 2.2 * k)), Fill::Solid(INK));
            }
            text_c(g, Rect::from_center_size(on_ring(m.pos, figure_r), vec2(40.0, 14.0)), m.text, 8.0 * k, INK);
        }
        if kind == Scale::Bass {
            // Soft in growing dots to the left, Tight in growing squares to the right
            for (i, deg) in [40.0f32, 72.0, 104.0, 135.0].into_iter().enumerate() {
                let size = (2.4 + 1.3 * i as f32) * k;
                for side in [-1.0f32, 1.0] {
                    let a = (side * deg).to_radians();
                    let dot = Rect::from_center_size(pos2(c.x + a.sin() * (dot_r + 3.0 * k), c.y - a.cos() * (dot_r + 3.0 * k)), vec2(size, size));
                    if side < 0.0 {
                        fill_ellipse(g, dot, Fill::Solid(INK));
                    } else {
                        fill_round_rect(g, dot, 0.5, Fill::Solid(INK));
                    }
                }
            }
            for (dx, word) in [(-30.0, "SOFT"), (30.0, "TIGHT")] {
                text_c(g, Rect::from_center_size(pos2(c.x + dx * k, c.y - figure_r - 4.0 * k), vec2(60.0, 12.0)), word, 7.5 * k, INK.alpha(0.9));
            }
        }
    }
    label(lay.units[0], "dB", 8.5, INK.alpha(0.9));
    label(lay.units[1], "kHz", 8.5, INK.alpha(0.9));

    label(lay.ovl - vec2(0.0, 13.5 * k), "OVL", 8.5, INK);
    lamp(g, lay.ovl, 4.6 * k, Jc::argb(0xFFFF2E24), lit.0);
    label(lay.gr - vec2(0.0, 13.5 * k), "GR", 8.5, INK);
    lamp(g, lay.gr, 4.6 * k, ORANGE, lit.1);

    // the name over the valve window; the valve glows while the unit is in
    let inn = pv(host, d::P_BYPASS) < 0.5;
    label(lay.name, "Vitalizer", 19.0, INK);
    label(lay.model, "Mk3-T", 12.0, INK.alpha(0.9));
    let win = lay.window;
    let round = win.height() * 0.5;
    fill_round_rect(g, win.expand(3.0), round + 3.0, Fill::linear(Jc::argb(0xFF2A2A2C), win.left_top(), Jc::argb(0xFF0A0A0B), win.left_bottom()));
    fill_round_rect(g, win, round, Fill::Solid(Jc::argb(0xFF060606)));
    if inn {
        let hot = win.center() + vec2(win.width() * 0.1, 0.0);
        fill_ellipse(
            g,
            Rect::from_center_size(hot, vec2(win.height() * 1.5, win.height() * 0.95)),
            Fill::radial(ORANGE.alpha(0.42), hot, ORANGE.alpha(0.0), hot + vec2(win.height() * 0.75, 0.0)),
        );
        for dx in [-0.02f32, 0.09, 0.2] {
            let p = win.center() + vec2(win.width() * dx, 0.0);
            line(g, p - vec2(0.0, win.height() * 0.22), p + vec2(0.0, win.height() * 0.22), 1.4, ORANGE.brighter(0.4).alpha(0.75));
        }
    }
    // the grille over it
    let gc = g.with_clip_rect(win.shrink(1.0).intersect(g.clip_rect()));
    let step = 6.0 * k;
    let mut x = win.left() - win.height();
    while x < win.right() {
        line(&gc, pos2(x, win.bottom()), pos2(x + win.height(), win.top()), 1.0, Jc::WHITE.alpha(0.10));
        line(&gc, pos2(x, win.top()), pos2(x + win.height(), win.bottom()), 1.0, Jc::WHITE.alpha(0.10));
        x += step;
    }
    // the window is a slot with round ends: the plate again over the grille's corners, then its rim
    stroke_closed(g, &round_rect(win.expand(5.5), round + 5.5), 11.0, PLATE);
    stroke_closed(g, &round_rect(win.expand(1.5), round + 1.5), 2.5, Jc::argb(0xFF2A2A2C));
    stroke_closed(g, &round_rect(win, round), 1.0, Jc::BLACK.alpha(0.8));

    // ON: lit orange while the unit is in
    label(lay.on - vec2(0.0, 32.0 * k), "ON", 11.0, INK);
    let b = Rect::from_center_size(lay.on, vec2(34.0 * k, 34.0 * k));
    fill_round_rect(g, b.expand(2.0), 5.0, Fill::Solid(Jc::argb(0xFF050505)));
    fill_round_rect(g, b, 4.0, Fill::linear(Jc::argb(0xFF2C2C2E), b.left_top(), Jc::argb(0xFF131314), b.left_bottom()));
    let core = b.shrink(9.0 * k);
    if inn {
        fill_ellipse(
            g,
            Rect::from_center_size(b.center(), vec2(b.width() * 1.9, b.width() * 1.9)),
            Fill::radial(ORANGE.alpha(0.35), b.center(), ORANGE.alpha(0.0), b.center() + vec2(b.width() * 0.95, 0.0)),
        );
    }
    fill_round_rect(g, core, 1.5, Fill::Solid(if inn { ORANGE } else { Jc::argb(0xFF4A2A10) }));
}

/// The unit's controls: the eight knobs and the ON button.
pub(super) fn controls(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut FamilyState, lay: &Lay) {
    for (i, (param, _, _, _)) in KNOBS.into_iter().enumerate() {
        let v = pv(host, param);
        // the panel prints the name and the scale itself (`paint`): the knob is only the knob
        let face = knob::KnobFace {
            style: FaceStyle::Round,
            cap: CAP,
            ink: INK,
            marks: &[],
            positions: 0,
            minor: 0,
            travel: None,
            dots: false,
            name: "",
            name_below: false,
        };
        let default = host.info.param(id_of(param)).map_or(0.5, |p| p.default);
        let out = knob::vint_knob(ui, lay.knob_rect(i), param, &face, v, default, &format_knob_for(param, v, 2), true);
        apply_knob(app, host, st, param, 0, &out);
    }
    let hit = Rect::from_center_size(lay.on, vec2(40.0 * lay.k, 40.0 * lay.k));
    let inn = pv(host, d::P_BYPASS) < 0.5;
    let resp = ui.interact(hit, ui.id().with("vint-vitalizer-on"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, inn, "On"));
    if resp.clicked() {
        family::pill_set(app, host, st, &FAMILY, id_of(d::P_BYPASS), if inn { 1.0 } else { 0.0 });
    }
}

/// The slim editor's section: the Type selector, then the unit turned on its side, in a column
/// `w` wide from `top_left`. Returns its bottom.
pub(super) fn slim(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut FamilyState, top_left: Pos2, w: f32, lit: (bool, bool)) -> f32 {
    maximizer::type_selector(app, ui, host, st, None, Rect::from_min_size(top_left, vec2(w, SLIM_TYPE_H)), 2);
    let lay = Lay::slim(top_left + vec2(0.0, SLIM_TYPE_H + 8.0), w);
    paint(&ui.painter().clone(), host, &lay, lit);
    controls(app, ui, host, st, &lay);
    lay.plate.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scales_are_the_hardwares() {
        use soundcraft_dsp::plugins::fs::vint::vint_vitalizer as v;
        // The printed figures are what the knobs do.
        for (m, db) in marks(Scale::DriveDb).iter().zip(v::DRIVE_SCALE_DB) {
            assert!((v::drive_db(m.pos) - db).abs() < 1e-3, "{}", m.text);
            assert_eq!(m.text.parse::<f32>().ok(), Some(db));
        }
        for (m, khz) in marks(Scale::TuneKhz).iter().zip(v::TUNE_SCALE_KHZ) {
            assert!((v::tune_hz(m.pos) / 1000.0 - khz).abs() < 1e-3, "{}", m.text);
            assert_eq!(m.text.parse::<f32>().ok(), Some(khz));
        }
        assert_eq!(marks(Scale::OffMax).len(), 11);
        assert_eq!(marks(Scale::Bass).len(), 1);
    }

    #[test]
    fn the_knobs_keep_to_their_sections() {
        // In the hardware's order, each between the rules of its section, none over another.
        let xs: Vec<f32> = KNOBS.iter().map(|k| k.1).collect();
        assert!(xs.windows(2).all(|w| w[1] - w[0] >= 88.0), "{xs:?}");
        let section = |x: f32| RULES.iter().filter(|r| **r < x).count();
        assert_eq!(xs.iter().map(|x| section(*x)).collect::<Vec<_>>(), [0, 1, 1, 2, 2, 3, 3, 4]);
        assert_eq!((section(WINDOW.0), section(ON.0)), (5, 6));
    }

    #[test]
    fn both_layouts_keep_everything_on_the_plate() {
        let wide = Lay::wide(Rect::from_min_size(pos2(24.0, 120.0), vec2(952.0, 301.0)), 0.952, pos2(24.0, 127.2));
        let slim = Lay::slim(pos2(10.0, 200.0), 260.0);
        for (name, lay) in [("wide", &wide), ("slim", &slim)] {
            let inside = |p: Pos2| lay.plate.shrink(4.0).contains(p);
            assert!(inside(lay.ovl) && inside(lay.gr) && inside(lay.on) && inside(lay.name) && inside(lay.model), "{name}");
            assert!(lay.plate.contains_rect(lay.window.expand(6.0)), "{name}: the window");
            let reach = knob::radius_for(lay.knob_w, FaceStyle::Round, false) + (FIGURE_PAST + 8.0) * lay.k;
            for (i, c) in lay.knobs.iter().enumerate() {
                assert!(lay.plate.contains_rect(Rect::from_center_size(*c, Vec2::splat(2.0 * reach))), "{name}: knob {i}'s scale");
                assert!(lay.knobs[..i].iter().all(|o| o.distance(*c) >= 2.0 * reach - 10.0 * lay.k), "{name}: knob {i} crowds another");
            }
            assert!(lay.rules.iter().all(|(a, b)| lay.plate.contains(*a) && lay.plate.contains(*b)), "{name}");
        }
        // the slim unit: two knobs to a row, a section to a row
        assert_eq!(slim.plate.width(), 260.0);
        assert_eq!(slim.knobs[1].y, slim.knobs[2].y);
        assert!(slim.knobs[3].y > slim.knobs[1].y && slim.rules.len() == 4);
    }

    #[test]
    fn the_lamps_follow_the_readouts() {
        assert_eq!(lamps(None), (false, false));
        let mut r = [f32::NAN; 8];
        r[4] = 0.0;
        r[5] = -12.0;
        assert_eq!(lamps(Some(r)), (false, false));
        r[4] = 3.0;
        r[5] = -2.0;
        assert_eq!(lamps(Some(r)), (true, true));
    }
}

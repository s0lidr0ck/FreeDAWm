//! FireStation ChamberMade's graph: a top-down plan of the room, as the C++ editor's
//! `ChamberPlan.h` draws it (`drawRoom()` in `ChamberMadeUI.js`), simplified for a tile: the
//! outline (walls in the wall material's colour over a floor tinted by the floor material), the
//! objects, the direct sound (and, large, the first reflections) to each microphone, the speaker
//! with its aim, and the two microphones; large, the RT60 per band and the volume.
//!
//! Everything drawn comes out of the engine's own geometry ([`build_chamber_for`], the call the
//! DSP makes), so the plan is the room the audio is in. It is rebuilt only when a parameter
//! changes: one cached room per module, keyed on the parameter values.

use crate::theme::{Tokens, bold, is_light, regular};
use egui::{Align2, Color32, Id, Pos2, Rect, Shape, Stroke, pos2, vec2};
use soundcraft_dsp::ParamInfo;
use soundcraft_dsp::plugins::fs::chamber::{CHAMBER_PARAMS, build_chamber_for, chamber_geometry as geo, feet_string};
use soundcraft_model::Insert;
use std::sync::Arc;

/// The plan's palette (`plan::` in `ChamberPlan.h`).
const MIC_A: Color32 = Color32::from_rgb(0x4F, 0xC3, 0xF7);
const MIC_B: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x6B);
const CONE: Color32 = Color32::from_rgb(0xFF, 0xC8, 0x57);
const OBJ: Color32 = Color32::from_rgb(0xCF, 0xD4, 0xDA);

/// `MAT[].c` in `ChamberMadeUI.js`: each material's colour (walls are drawn in it).
const MATERIAL_COLOURS: [u32; 15] = [
    0xB9C2C9, 0xE6E1D3, 0x9FD8E6, 0xC9714F, 0x8F958E, 0x7FA0C4, 0xC8F2FF, 0xC89A5B, 0xA87942, 0xD5D5CF, 0x7D6248, 0x8C6FB0, 0xA0527A, 0x55606B,
    0x9AA3A0,
];

fn rgb(c: u32) -> Color32 {
    let [_, r, g, b] = c.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

/// A plan colour on the current palette: as given on the dark one, deepened on the light one so
/// pale materials still read on a pale tile.
fn on_palette(c: Color32) -> Color32 {
    if is_light() { c.lerp_to_gamma(Color32::BLACK, 0.45) } else { c }
}

fn material_colour(index: f32) -> Color32 {
    let i = if index.is_finite() { (index.round().max(0.0) as usize).min(MATERIAL_COLOURS.len() - 1) } else { 0 };
    on_palette(rgb(MATERIAL_COLOURS[i]))
}

/// One parameter's value as the engine sees it (clamped, the default where unset), or `over`
/// when it names this parameter (a knob being dragged).
fn knob(ins: &Insert, p: &ParamInfo, over: Option<(&str, f32)>) -> f32 {
    let v = match over {
        Some((id, v)) if id == p.id => v,
        _ => ins.params.get(p.id).copied().unwrap_or(p.default),
    };
    p.clamp(v)
}

/// The module's parameter values in the engine's order (clamped, defaults where unset).
pub fn knob_values(ins: &Insert) -> Vec<f32> {
    CHAMBER_PARAMS.iter().map(|p| knob(ins, p, None)).collect()
}

type PlanCache = (u64, Arc<geo::Chamber>);

/// The room the module's parameters make, from the engine's geometry; rebuilt only when a value
/// changes (one entry per module, shared by its tile and its drawer).
pub fn chamber_for(ctx: &egui::Context, module: Id, ins: &Insert) -> Arc<geo::Chamber> {
    chamber_with(ctx, module.with("chamber-plan"), ins, None)
}

/// [`chamber_for`] under `key`, with one value overridden. The cache key is hashed straight from
/// the parameters; the value list is only built on a miss.
fn chamber_with(ctx: &egui::Context, key: Id, ins: &Insert, over: Option<(&str, f32)>) -> Arc<geo::Chamber> {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    CHAMBER_PARAMS.iter().for_each(|p| knob(ins, p, over).to_bits().hash(&mut h));
    let hash = h.finish();
    if let Some((_, c)) = ctx.data(|d| d.get_temp::<PlanCache>(key)).filter(|(k, _)| *k == hash) {
        return c;
    }
    let values: Vec<f32> = CHAMBER_PARAMS.iter().map(|p| knob(ins, p, over)).collect();
    let c = Arc::new(build_chamber_for(&values));
    ctx.data_mut(|d| d.insert_temp::<PlanCache>(key, (hash, Arc::clone(&c))));
    c
}

/// `lengthString (metres, true)` in feet: one decimal under 100, none above, no unit.
fn length_bare(metres: f32) -> String {
    let x = metres * 3.280_84;
    if x < 100.0 { format!("{x:.1}") } else { format!("{x:.0}") }
}

/// What a control says in the drawer: `formatParam` in `Parameters.h`, from the room the
/// module's values build (with `shown`, the value under the knob, for this one), through the
/// cached engine geometry. Parameters that need no room keep their own text.
pub fn value_text(ctx: &egui::Context, module: Id, ins: &Insert, info: &ParamInfo, shown: f32) -> String {
    use soundcraft_dsp::plugins::fs::chamber::chamber_made_dsp as d;
    let Some(index) = CHAMBER_PARAMS.iter().position(|p| p.id == info.id) else { return info.format(shown) };
    let object_field = (d::P_OBJ1_TYPE..=d::P_OBJ8_SIZE).contains(&index) && !(index - d::P_OBJ1_TYPE).is_multiple_of(4);
    let needs_room = object_field
        || matches!(
            index,
            d::P_DECAY
                | d::P_ROOM_W
                | d::P_ROOM_L
                | d::P_ROOM_H
                | d::P_CEIL_SLOPE
                | d::P_SPK_X
                | d::P_SPK_Y
                | d::P_SPK_Z
                | d::P_MIC_AX
                | d::P_MIC_AY
                | d::P_MIC_BX
                | d::P_MIC_BY
                | d::P_MIC_Z
        );
    if !needs_room {
        return info.format(shown);
    }
    let v = info.clamp(shown);
    let ch = chamber_with(ctx, module.with("chamber-values"), ins, Some((info.id, v)));
    let pl = &ch.placement;
    match index {
        d::P_DECAY => format!("\u{d7}{:.2} \u{b7} {:.1} s", d::knob_to_decay_mul(v), ch.acoustics.rt60.mid),
        d::P_ROOM_W => feet_string(ch.room.width_m),
        d::P_ROOM_L => feet_string(ch.room.length_m),
        d::P_ROOM_H => feet_string(ch.room.height_m),
        d::P_CEIL_SLOPE => {
            if v < 0.01 {
                "flat".to_string()
            } else {
                format!("{}\u{2192}{}", length_bare(ch.room.ceiling_at(0.0)), feet_string(ch.room.ceiling_at(ch.room.length_m)))
            }
        }
        d::P_SPK_X => feet_string(pl.speaker.x),
        d::P_SPK_Y => feet_string(pl.speaker.y),
        d::P_SPK_Z => format!("{} up", feet_string(pl.speaker.z)),
        d::P_MIC_AX => feet_string(pl.mic_a.x),
        d::P_MIC_AY => feet_string(pl.mic_a.y),
        d::P_MIC_BX => feet_string(pl.mic_b.x),
        d::P_MIC_BY => feet_string(pl.mic_b.y),
        d::P_MIC_Z => format!("{} up", feet_string(pl.mic_a.z)),
        _ => {
            // An object slot's x, y or size: the confined position and the diameter once
            // something stands there, else the percentage (the C++'s own fallback).
            let slot = (index - d::P_OBJ1_TYPE) / 4;
            let Some(o) = ch.objects.get(slot).filter(|o| o.on) else { return info.format(shown) };
            match (index - d::P_OBJ1_TYPE) % 4 {
                1 => feet_string(o.pos.x),
                2 => feet_string(o.pos.y),
                _ => format!("\u{d8} {}", feet_string(o.radius * 2.0)),
            }
        }
    }
}

/// `layout()`: room metres to screen, turned on its side when that is bigger.
#[derive(Debug, Clone, Copy)]
struct Layout {
    sc: f32,
    ox: f32,
    oy: f32,
    rot: bool,
}

impl Layout {
    fn new(area: Rect, w: f32, l: f32) -> Layout {
        let w = w.max(0.1);
        let l = l.max(0.1);
        let s0 = (area.width() / w).min(area.height() / l);
        let s1 = (area.width() / l).min(area.height() / w);
        let rot = s1 > s0 * 1.02;
        let sc = (if rot { s1 } else { s0 }).max(0.01);
        let ox = area.left() + (area.width() - (if rot { l } else { w }) * sc) * 0.5;
        let mid = area.center().y;
        let oy = if rot { mid - w * sc * 0.5 } else { mid + l * sc * 0.5 };
        Layout { sc, ox, oy, rot }
    }

    fn p(&self, x: f32, y: f32) -> Pos2 {
        if self.rot { pos2(self.ox + y * self.sc, self.oy + x * self.sc) } else { pos2(self.ox + x * self.sc, self.oy - y * self.sc) }
    }

    /// A direction in room space on screen.
    fn d(&self, vx: f32, vy: f32) -> egui::Vec2 {
        if self.rot { vec2(vy, vx) } else { vec2(vx, -vy) }
    }
}

/// "6,330 ft³".
fn cubic_feet(m3: f32) -> String {
    let v = (m3 * 3.280_84f32.powi(3)).round().max(0.0) as u64;
    let s = v.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{out} ft\u{b3}")
}

fn rt_text(x: f32) -> String {
    if x < 10.0 { format!("{x:.1}") } else { format!("{x:.0}") }
}

/// Draws the plan into `rect`.
pub fn draw(ctx: &egui::Context, p: &egui::Painter, rect: Rect, ins: &Insert, module: Id, large: bool) {
    let t = Tokens::current();
    let c = chamber_for(ctx, module, ins);
    let room = &c.room;
    let ac = &c.acoustics;
    let pl = &c.placement;

    let head = if large { 16.0 } else { 0.0 };
    let pad = if large { 10.0 } else { 4.0 };
    let foot = if large { 12.0 } else { 0.0 };
    let area = Rect::from_min_max(pos2(rect.left() + pad, rect.top() + pad + head), pos2(rect.right() - pad, rect.bottom() - pad - foot));
    if area.width() < 4.0 || area.height() < 4.0 {
        return;
    }
    let lay = Layout::new(area, room.width_m, room.length_m);
    let p = p.with_clip_rect(rect);

    // The drafting grid (large only): 1 / 2 / 5 / 10 / 25 ft, from the size of one foot.
    if large {
        let px_ft = lay.sc * 0.3048;
        let step_ft = if px_ft >= 18.0 {
            1.0
        } else if px_ft >= 9.0 {
            2.0
        } else if px_ft >= 3.6 {
            5.0
        } else if px_ft >= 1.8 {
            10.0
        } else {
            25.0
        };
        let step = step_ft * 0.3048;
        let grid = Stroke::new(1.0, t.graph_grid);
        let mut x = 0.0;
        while x <= room.width_m + 1e-4 {
            p.line_segment([lay.p(x, 0.0), lay.p(x, room.length_m)], grid);
            x += step;
        }
        let mut y = 0.0;
        while y <= room.length_m + 1e-4 {
            p.line_segment([lay.p(0.0, y), lay.p(room.width_m, y)], grid);
            y += step;
        }
        p.text(pos2(rect.right() - 6.0, rect.top() + 4.0), Align2::RIGHT_TOP, format!("{step_ft:.0} ft grid"), regular(8.5), t.text_dim);
    }

    // Floor and walls, coloured by material.
    let walls = room.num_walls.min(geo::MAX_WALLS);
    let outline: Vec<Pos2> = (0..walls).map(|i| lay.p(room.vx[i], room.vy[i])).collect();
    let wall = material_colour(ins.params.get("wallMat").copied().unwrap_or(0.0));
    let floor = material_colour(ins.params.get("floorMat").copied().unwrap_or(0.0));
    p.add(Shape::convex_polygon(outline.clone(), floor.gamma_multiply(0.12), Stroke::NONE));
    p.add(Shape::closed_line(outline, Stroke::new(if large { 3.0 } else { 2.0 }, wall)));

    // Sound paths: the direct sound solid; large, the first reflections dashed. Blue for mic A,
    // red for B, as bright as they are loud.
    let last = if large { geo::TAP_SECOND } else { geo::TAP_FIRST };
    for (mi, (mic, colour)) in [(pl.mic_a, MIC_A), (pl.mic_b, MIC_B)].into_iter().enumerate() {
        for i in geo::TAP_DIRECT..last {
            let Some(tap) = c.taps.get(mi * geo::TAPS_PER_MIC + i) else { continue };
            let gm = tap.gain.mid.abs();
            if !tap.live || gm < 1.0e-4 || !gm.is_finite() {
                continue;
            }
            let alpha = (0.10 + gm * 2.0).min(0.8);
            let col = colour.gamma_multiply(alpha);
            let s = lay.p(pl.speaker.x, pl.speaker.y);
            let m = lay.p(mic.x, mic.y);
            if i == geo::TAP_DIRECT {
                p.line_segment([s, m], Stroke::new(if large { 1.4 } else { 1.0 }, col));
            } else if tap.has_point {
                let r = lay.p(tap.reflection_point.x, tap.reflection_point.y);
                p.extend(Shape::dashed_line(&[s, r, m], Stroke::new(0.8, col), 4.0, 4.0));
            }
        }
    }

    // Objects: round ones as discs, panels as a bar along their face.
    let obj = on_palette(OBJ);
    for o in c.objects.iter().filter(|o| o.on) {
        let centre = lay.p(o.pos.x, o.pos.y);
        if o.flat {
            let half = lay.d(o.ux, o.uy) * (o.radius * lay.sc);
            p.line_segment([centre - half, centre + half], Stroke::new((0.12 * lay.sc).clamp(2.0, 5.0), obj));
        } else {
            let r = (o.radius * lay.sc).max(1.5);
            p.circle(centre, r, obj.gamma_multiply(0.25), Stroke::new(1.0, obj));
        }
    }

    // The speaker: a box with its aim.
    let s = lay.p(pl.speaker.x, pl.speaker.y);
    let aim = lay.d(pl.speaker_aim.x, pl.speaker_aim.y);
    let size = if large { 9.0 } else { 6.0 };
    p.line_segment([s, s + aim * size * 1.6], Stroke::new(if large { 2.0 } else { 1.5 }, on_palette(CONE)));
    p.rect_filled(Rect::from_center_size(s, vec2(size, size)), 1.0, t.text);

    // The microphones: filled discs, faint when omnidirectional (as the C++ draws them), each
    // with its aim.
    let omni = ins.params.get("micPat").copied().unwrap_or(0.0).round() == 0.0;
    for (mic, mic_aim, colour, name) in [(pl.mic_a, pl.mic_a_aim, MIC_A, "A"), (pl.mic_b, pl.mic_b_aim, MIC_B, "B")] {
        let m = lay.p(mic.x, mic.y);
        let r = if large { 6.0 } else { 3.5 };
        let col = if omni { colour.gamma_multiply(0.75) } else { colour };
        if !omni {
            p.line_segment([m, m + lay.d(mic_aim.x, mic_aim.y) * r * 2.2], Stroke::new(1.5, col));
        }
        p.circle_filled(m, r, col);
        if large {
            p.text(m, Align2::CENTER_CENTER, name, bold(8.0), Color32::from_rgb(0x15, 0x18, 0x1C));
        }
    }

    // Readouts: RT60 low / mid / high and the volume (large); the mid RT60 alone on a tile.
    let rt = ac.rt60;
    if large {
        let line = format!("RT60 {} / {} / {} s", rt_text(rt.lo), rt_text(rt.mid), rt_text(rt.hi));
        p.text(pos2(rect.left() + 6.0, rect.top() + 4.0), Align2::LEFT_TOP, line, bold(9.5), t.text);
        let size = format!("{} \u{d7} {} \u{b7} {}", feet_string(room.width_m), feet_string(room.length_m), cubic_feet(ac.volume));
        p.text(pos2(rect.left() + 6.0, rect.bottom() - 3.0), Align2::LEFT_BOTTOM, size, regular(8.5), t.text_dim);
    } else if rect.height() >= 28.0 {
        p.text(pos2(rect.right() - 3.0, rect.bottom() - 2.0), Align2::RIGHT_BOTTOM, format!("{} s", rt_text(rt.mid)), regular(8.0), t.text_dim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_is_the_engines_room_and_is_cached() {
        let ctx = egui::Context::default();
        let module = Id::new("m");
        let mut ins = Insert::new("fs_chamber");
        let a = chamber_for(&ctx, module, &ins);
        // The engine's own geometry, from the module's values.
        assert_eq!(*a, build_chamber_for(&knob_values(&ins)));
        assert!(Arc::ptr_eq(&a, &chamber_for(&ctx, module, &ins)), "no rebuild while nothing changes");
        ins.params.insert("shape".into(), 3.0);
        ins.params.insert("roomW".into(), 0.9);
        let b = chamber_for(&ctx, module, &ins);
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(b.room.num_walls, 6, "a hexagon");
        assert_eq!(*b, build_chamber_for(&knob_values(&ins)));
        // Hostile stored values are clamped as the engine clamps them.
        ins.params.insert("roomL".into(), f32::NAN);
        ins.params.insert("wallMat".into(), 1e9);
        let c = chamber_for(&ctx, module, &ins);
        assert!(c.room.length_m.is_finite() && c.acoustics.rt60.mid.is_finite());
    }

    #[test]
    fn value_texts_come_from_the_built_room() {
        use soundcraft_dsp::plugins::fs::chamber::chamber_made_dsp as d;
        let ctx = egui::Context::default();
        let m = Id::new("m");
        let mut ins = Insert::new("fs_chamber");
        let p = |id: &str| CHAMBER_PARAMS.iter().find(|p| p.id == id).unwrap();
        let ch = build_chamber_for(&knob_values(&ins));
        assert_eq!(value_text(&ctx, m, &ins, p("decay"), 0.5), format!("\u{d7}1.00 \u{b7} {:.1} s", ch.acoustics.rt60.mid));
        assert_eq!(value_text(&ctx, m, &ins, p("spkX"), 0.25), feet_string(ch.placement.speaker.x));
        assert_eq!(value_text(&ctx, m, &ins, p("micZ"), 0.45), format!("{} up", feet_string(ch.placement.mic_a.z)));
        assert!(value_text(&ctx, m, &ins, p("ceilSlope"), 0.4).contains('\u{2192}'));
        assert_eq!(value_text(&ctx, m, &ins, p("ceilSlope"), 0.0), "flat");
        // The value under the knob, not the stored one: a dragged width is that room's width.
        assert_eq!(value_text(&ctx, m, &ins, p("roomW"), 1.0), feet_string(d::knob_to_room_w(1.0)));
        // An empty slot prints its percentage; a placed one its position and diameter.
        assert_eq!(value_text(&ctx, m, &ins, p("o2s"), 0.5), "50 %");
        ins.params.insert("o2t".into(), 1.0);
        let ch = build_chamber_for(&knob_values(&ins));
        assert_eq!(value_text(&ctx, m, &ins, p("o2s"), 0.5), format!("\u{d8} {}", feet_string(ch.objects[1].radius * 2.0)));
        assert_eq!(value_text(&ctx, m, &ins, p("o2x"), 0.5), feet_string(ch.objects[1].pos.x));
        // Parameters that need no room keep their own text.
        assert_eq!(value_text(&ctx, m, &ins, p("mix"), 0.35), "35 %");
        assert_eq!(value_text(&ctx, m, &ins, p("o2r"), 0.5), "90\u{b0}");
    }

    #[test]
    fn layout_turns_the_room_on_its_side_when_bigger() {
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 50.0));
        // A long room in a wide tile lies on its side.
        let l = Layout::new(wide, 4.0, 12.0);
        assert!(l.rot);
        let far = l.p(4.0, 12.0);
        assert!(wide.expand(0.5).contains(far) && wide.expand(0.5).contains(l.p(0.0, 0.0)));
        // A wide room stays upright, y up.
        let l = Layout::new(wide, 12.0, 4.0);
        assert!(!l.rot);
        assert!(l.p(0.0, 4.0).y < l.p(0.0, 0.0).y);
        assert_eq!(cubic_feet(179.25), "6,330 ft\u{b3}");
        assert_eq!(material_colour(f32::NAN), material_colour(0.0));
    }
}

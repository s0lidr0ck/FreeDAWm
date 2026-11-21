//! The instrument-panel kit the three built-in synths are drawn with: a printed panel between
//! wood end cheeks, a name badge with a lamp, a preset "LCD" with previous and next, framed
//! sections with the label set into the frame, knobs with a dark cap, pointer and printed ticks,
//! envelope faders, inset dark displays, source chips and panel buttons. One family; a
//! [`Face`] gives each synth its panel colour, display colour and accent.
//!
//! The controls take the editor kit's gestures ([`crate::editor_kit::controls::dial_input`]):
//! vertical drag (Shift for fine), double-click to the default, the wheel, Ctrl+click (Cmd+click)
//! to type a value. A knob or fader changes its parameter through `mix.instrument_param`, merged
//! into one undo step per gesture, so a move is written as automation in Touch, Latch and Write
//! like any instrument parameter.

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, dial_input};
use crate::editor_kit::look::{tracked_text, tracked_width};
use crate::theme::{bold, mono};
use egui::{Align2, Color32, CornerRadius, CursorIcon, Id, Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::plugins::synth_mod::LiveRead;
use soundcraft_dsp::{ParamInfo, PluginInfo, Unit};
use soundcraft_model::{Insert, TrackId};
use std::cell::RefCell;
use std::f32::consts::PI;

/// One synth's look: the panel, its print, the displays and the accent.
#[derive(Clone, Copy, Debug)]
pub struct Face {
    /// The panel's colour, as stops across it (left to right when `brushed`, else top to bottom).
    pub panel: &'static [(f32, Color32)],
    pub brushed: bool,
    /// The print: labels and frames, the quieter print (values, notes), the frames' line.
    pub ink: Color32,
    pub ink2: Color32,
    pub line: Color32,
    /// The panel buttons' face.
    pub key: Color32,
    /// The inset displays: background, lettering, the bright trace, the grid.
    pub well: Color32,
    pub well_ink: Color32,
    pub glow: Color32,
    pub grid: Color32,
    /// The knob cap, light and dark side, and its pointer.
    pub cap_hi: Color32,
    pub cap_lo: Color32,
    pub pointer: Color32,
    /// The accent (lamp, pressed buttons, macro caps) and the ink on it.
    pub accent: Color32,
    pub on_accent: Color32,
    /// The end cheeks' wood, dark and light.
    pub wood: (Color32, Color32),
    /// The rear plate's colour (the Back), as stops top to bottom.
    pub rear: &'static [(f32, Color32)],
    /// Print on a small plate of the panel's colour (the Back, where cables pass under it).
    pub plates: bool,
}

impl Face {
    /// The panel colour at `t` (0..1) across it.
    pub fn panel_at(&self, t: f32) -> Color32 {
        let stops = self.panel;
        let Some(first) = stops.first() else { return Color32::GRAY };
        let mut a = *first;
        for b in stops.iter().copied() {
            if t <= b.0 {
                let span = b.0 - a.0;
                let u = if span > 0.0 { (t - a.0) / span } else { 1.0 };
                return a.1.lerp_to_gamma(b.1, u.clamp(0.0, 1.0));
            }
            a = b;
        }
        a.1
    }
}

/// The sources' colours, in [`soundcraft_dsp::plugins::patch::SOURCES`] order (amp envelope,
/// envelope 2, LFO, velocity, mod wheel, key, random, then the four macros).
pub const SOURCE_COLOURS: [Color32; 7] = [
    Color32::from_rgb(0xff, 0x6f, 0x91),
    Color32::from_rgb(0xa9, 0x8b, 0xff),
    Color32::from_rgb(0x38, 0xc6, 0xdc),
    Color32::from_rgb(0xe9, 0xb9, 0x2c),
    Color32::from_rgb(0x5f, 0xd6, 0xa6),
    Color32::from_rgb(0xff, 0x8b, 0x3d),
    Color32::from_rgb(0x74, 0xcf, 0x52),
];

/// Corner radius as egui takes it.
fn cr(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

/// `c` at `a` (0..1) opacity.
pub fn alpha(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

// ------------------------------------------------------------------------------- the unit

/// The width of an end cheek.
pub const CHEEK: f32 = 18.0;

/// A rectangle filled with `stops` across it: vertical bands left to right when `across`, else
/// horizontal bands top to bottom.
fn gradient(p: &Painter, r: Rect, stops: &[(f32, Color32)], across: bool) {
    if stops.len() < 2 {
        p.rect_filled(r, 0.0, stops.first().map_or(Color32::GRAY, |s| s.1));
        return;
    }
    let mut mesh = Mesh::default();
    for (t, c) in stops {
        let (a, b) = if across {
            let x = r.left() + r.width() * t;
            (pos2(x, r.top()), pos2(x, r.bottom()))
        } else {
            let y = r.top() + r.height() * t;
            (pos2(r.left(), y), pos2(r.right(), y))
        };
        mesh.colored_vertex(a, *c);
        mesh.colored_vertex(b, *c);
    }
    for i in 0..(stops.len() as u32 - 1) {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 3);
        mesh.add_triangle(k, k + 3, k + 2);
    }
    p.add(Shape::mesh(mesh));
}

/// One wood end cheek: the board's colour across it and a grain of fine darker and lighter lines.
fn cheek(p: &Painter, r: Rect, face: &Face, left: bool) {
    let (d, l) = face.wood;
    let rounding = if left { CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 } } else { CornerRadius { nw: 0, sw: 0, ne: 8, se: 8 } };
    p.rect_filled(r, rounding, d);
    let inner = r.shrink2(vec2(0.0, 3.0));
    gradient(p, inner, &[(0.0, d), (0.55, l), (1.0, d)], true);
    // The grain: lines a little off vertical, irregularly spaced (the same every frame).
    let mut x = r.left() + 3.0;
    let mut i = 0u32;
    while x < r.right() - 2.0 {
        let dark = i % 3 != 1;
        let c = if dark { alpha(Color32::BLACK, 0.16) } else { alpha(Color32::WHITE, 0.07) };
        let lean = if i.is_multiple_of(2) { 1.2 } else { -0.8 };
        p.line_segment([pos2(x, r.top() + 2.0), pos2(x + lean, r.bottom() - 2.0)], Stroke::new(1.0, c));
        x += 3.0 + ((i * 7) % 5) as f32;
        i += 1;
    }
    // Light on the top edge, shade at the bottom.
    p.line_segment([pos2(r.left() + 2.0, r.top() + 0.5), pos2(r.right() - 2.0, r.top() + 0.5)], Stroke::new(1.0, alpha(Color32::WHITE, 0.2)));
    p.line_segment([pos2(r.left() + 2.0, r.bottom() - 1.0), pos2(r.right() - 2.0, r.bottom() - 1.0)], Stroke::new(2.0, alpha(Color32::BLACK, 0.4)));
}

/// Paints the instrument: a drop shadow, the two cheeks and the printed panel between them.
/// Returns the panel's inside (inside its margins).
pub fn unit(p: &Painter, r: Rect, face: &Face) -> Rect {
    for i in 1..=6 {
        let i = i as f32;
        p.rect_filled(r.translate(vec2(0.0, 2.0 + i)).expand(i * 1.5), cr(8.0 + i), alpha(Color32::BLACK, 0.06));
    }
    let left = Rect::from_min_size(r.min, vec2(CHEEK, r.height()));
    let right = Rect::from_min_max(pos2(r.right() - CHEEK, r.top()), r.max);
    let panel = Rect::from_min_max(pos2(left.right(), r.top()), pos2(right.left(), r.bottom()));
    gradient(p, panel, face.panel, face.brushed);
    if face.brushed {
        // Galvanised steel: faint horizontal brushing.
        let mut y = panel.top() + 2.0;
        let mut i = 0u32;
        while y < panel.bottom() {
            let c = if i.is_multiple_of(2) { alpha(Color32::WHITE, 0.10) } else { alpha(Color32::BLACK, 0.035) };
            p.hline(panel.x_range(), y, Stroke::new(1.0, c));
            y += 2.0 + (i % 3) as f32;
            i += 1;
        }
    }
    p.hline(panel.x_range(), panel.top() + 0.5, Stroke::new(1.0, alpha(Color32::WHITE, 0.19)));
    p.hline(panel.x_range(), panel.bottom() - 0.5, Stroke::new(1.0, alpha(Color32::BLACK, 0.2)));
    cheek(p, left, face, true);
    cheek(p, right, face, false);
    Rect::from_min_max(panel.min + vec2(10.0, 8.0), panel.max - vec2(10.0, 8.0))
}

/// The lamp beside the name: the accent, glowing.
pub fn lamp(p: &Painter, c: Pos2, face: &Face) {
    for i in (1..=4).rev() {
        p.circle_filled(c, 4.5 + i as f32 * 1.6, alpha(face.accent, 0.07));
    }
    p.circle_filled(c, 6.5, alpha(Color32::BLACK, 0.2));
    p.circle_filled(c, 4.5, face.accent);
    p.circle_filled(c - vec2(1.3, 1.3), 1.6, alpha(Color32::WHITE, 0.55));
}

/// The name badge: the name large and letter-spaced, the kind under it. Returns its width.
pub fn badge(p: &Painter, left_centre: Pos2, name: &str, kind: &str, face: &Face) -> f32 {
    let row = Rect::from_min_size(pos2(left_centre.x, left_centre.y - 12.0), vec2(400.0, 16.0));
    let w1 = tracked_text(p, row, Align2::LEFT_CENTER, name, 16.0, 0.14, true, true, face.ink).width();
    let row2 = Rect::from_min_size(pos2(left_centre.x + 1.0, left_centre.y + 4.0), vec2(400.0, 10.0));
    let w2 = tracked_text(p, row2, Align2::LEFT_CENTER, kind, 7.5, 0.22, true, true, face.ink2).width();
    w1.max(w2)
}

/// What the preset display did.
#[derive(Default)]
pub struct LcdOut {
    pub prev: bool,
    pub next: bool,
    /// The name was clicked (the preset menu).
    pub name: Option<Response>,
}

/// An inset dark well (displays, the LCD): background, inner shadow, a lit lower edge.
pub fn well(p: &Painter, r: Rect, face: &Face, round: f32) {
    p.rect_filled(r, cr(round), face.well);
    for i in 0..4 {
        let i = i as f32;
        let band = Rect::from_min_max(r.min + vec2(1.0, 1.0 + i), pos2(r.right() - 1.0, r.top() + 2.0 + i));
        p.rect_filled(band, 0.0, alpha(Color32::BLACK, 0.28 - i * 0.06));
    }
    p.rect_stroke(r, cr(round), Stroke::new(1.0, alpha(Color32::BLACK, 0.5)), StrokeKind::Inside);
    p.hline(r.left() + round..=r.right() - round, r.bottom() + 0.5, Stroke::new(1.0, alpha(Color32::WHITE, 0.16)));
}

/// The preset display: ◀, the preset's name, ▶.
pub fn lcd(ui: &mut Ui, r: Rect, salt: impl std::hash::Hash + std::fmt::Debug, text: &str, face: &Face) -> LcdOut {
    let p = ui.painter().clone();
    well(&p, r, face, 4.0);
    let id = Id::new(("synth-lcd", salt));
    let arrow = |ui: &mut Ui, rr: Rect, k: &str, glyph: &str, label: &str| {
        let resp = ui.interact(rr, id.with(k), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        let a = if resp.hovered() { 1.0 } else { 0.55 };
        p.text(rr.center(), Align2::CENTER_CENTER, glyph, mono(11.0), alpha(face.well_ink, a));
        resp.on_hover_text(label).clicked()
    };
    let left = Rect::from_min_size(r.min, vec2(20.0, r.height()));
    let right = Rect::from_min_max(pos2(r.right() - 20.0, r.top()), r.max);
    let mut out =
        LcdOut { prev: arrow(ui, left, "prev", "◀", "Previous preset"), next: arrow(ui, right, "next", "▶", "Next preset"), name: None };
    let mid = Rect::from_min_max(pos2(left.right(), r.top()), pos2(right.left(), r.bottom()));
    let resp = ui.interact(mid, id.with("name"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Presets"));
    let glow = if resp.hovered() { face.glow } else { face.well_ink };
    // The display's own glow behind the lettering.
    p.text(mid.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, text, mono(11.0), alpha(glow, 0.25));
    p.text(mid.center(), Align2::CENTER_CENTER, text, mono(11.0), glow);
    out.name = Some(resp.on_hover_text("Presets: choose one"));
    out
}

/// A panel button (Front, Back, Surprise): letter-spaced on the button face, the accent when on.
pub fn button(ui: &mut Ui, r: Rect, salt: impl std::hash::Hash + std::fmt::Debug, text: &str, on: bool, face: &Face) -> Response {
    let id = Id::new(("synth-btn", salt));
    let resp = ui.interact(r, id, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, text));
    let p = ui.painter();
    let down = on || resp.is_pointer_button_down_on();
    let body = if down { r.translate(vec2(0.0, 1.0)) } else { r };
    if !down {
        p.rect_filled(r.translate(vec2(0.0, 2.0)), cr(4.0), face.line);
    } else {
        p.rect_filled(r.translate(vec2(0.0, 1.0)), cr(4.0), face.line);
    }
    let fill = if on {
        face.accent
    } else if resp.hovered() {
        face.key.lerp_to_gamma(face.accent, 0.12)
    } else {
        face.key
    };
    p.rect_filled(body, cr(4.0), fill);
    p.rect_stroke(body, cr(4.0), Stroke::new(1.0, face.line), StrokeKind::Inside);
    p.hline(body.left() + 3.0..=body.right() - 3.0, body.top() + 1.0, Stroke::new(1.0, alpha(Color32::WHITE, 0.25)));
    let ink = if on { face.on_accent } else { face.ink };
    tracked_text(p, body, Align2::CENTER_CENTER, text, 9.5, 0.1, true, true, ink);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp
}

/// A small key for the window's own switches (Detach, Pin, Follow): quieter than a [`button`],
/// a lit dot when on.
pub fn small_key(ui: &mut Ui, r: Rect, salt: impl std::hash::Hash + std::fmt::Debug, text: &str, on: bool, face: &Face) -> Response {
    let resp = ui.interact(r, Id::new(("synth-small-key", salt)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, text));
    let p = ui.painter();
    let fill = if resp.hovered() { face.key.lerp_to_gamma(face.accent, 0.1) } else { face.key };
    p.rect_filled(r.translate(vec2(0.0, 1.0)), cr(3.0), face.line);
    p.rect_filled(r, cr(3.0), fill);
    p.rect_stroke(r, cr(3.0), Stroke::new(1.0, face.line), StrokeKind::Inside);
    let dot = pos2(r.left() + 7.0, r.center().y);
    p.circle_filled(dot, 2.5, if on { face.accent } else { alpha(face.ink, 0.25) });
    tracked_text(p, Rect::from_min_max(pos2(dot.x + 6.0, r.top()), r.max), Align2::LEFT_CENTER, text, 8.5, 0.1, true, true, face.ink2);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp
}

/// The width a [`small_key`] needs for `text`.
pub fn small_key_width(p: &Painter, text: &str) -> f32 {
    tracked_width(p, text, 8.5, 0.1, true) + 20.0
}

/// The width a [`button`] needs for `text`.
pub fn button_width(p: &Painter, text: &str) -> f32 {
    tracked_width(p, text, 9.5, 0.1, true) + 16.0
}

// ------------------------------------------------------------------------------- sections

/// The points of a rounded rectangle's outline, clockwise from the top left corner's end.
fn rounded_outline(r: Rect, rad: f32) -> Vec<Pos2> {
    let mut pts = Vec::with_capacity(48);
    let corners = [
        (pos2(r.right() - rad, r.top() + rad), -0.5 * PI),
        (pos2(r.right() - rad, r.bottom() - rad), 0.0),
        (pos2(r.left() + rad, r.bottom() - rad), 0.5 * PI),
        (pos2(r.left() + rad, r.top() + rad), PI),
    ];
    for (c, a0) in corners {
        for i in 0..=8 {
            let a = a0 + 0.5 * PI * i as f32 / 8.0;
            pts.push(c + rad * vec2(a.cos(), a.sin()));
        }
    }
    pts
}

/// A framed section: a rounded line around `r` with `title` (and a quieter `note` after it) set
/// into the top of the frame, the line broken where the label sits.
pub fn section(p: &Painter, r: Rect, title: &str, note: &str, face: &Face) {
    let label_y = r.top();
    let x0 = r.left() + 10.0;
    let lr = Rect::from_min_size(pos2(x0 + 6.0, label_y - 8.0), vec2(600.0, 16.0));
    let w1 = tracked_width(p, title, 10.0, 0.14, true);
    let w2 = if note.is_empty() { 0.0 } else { 6.0 + p.layout_no_wrap(note.to_string(), bold(9.5), face.ink2).size().x };
    let gap_end = x0 + 6.0 + w1 + w2 + 6.0;
    // The outline, from just after the label clockwise back to just before it.
    let rad = 6.0;
    let mut pts = vec![pos2(gap_end.min(r.right() - rad), r.top())];
    pts.extend(rounded_outline(r, rad));
    pts.push(pos2(x0, r.top()));
    p.add(Shape::line(pts, Stroke::new(1.5, face.line)));
    if face.plates {
        p.rect_filled(
            Rect::from_min_max(pos2(x0 + 2.0, label_y - 7.0), pos2(gap_end - 2.0, label_y + 7.0)),
            cr(2.0),
            alpha(face.panel_at(0.5), 0.95),
        );
    }
    tracked_text(p, lr, Align2::LEFT_CENTER, title, 10.0, 0.14, true, true, face.ink);
    if !note.is_empty() {
        p.text(pos2(x0 + 6.0 + w1 + 6.0, lr.center().y), Align2::LEFT_CENTER, note, bold(9.5), face.ink2);
    }
}

// ------------------------------------------------------------------------------- displays

/// An inset display: the well, its grid every `step` points, and its name in the corner.
pub fn display(p: &Painter, r: Rect, tag: &str, face: &Face) {
    well(p, r, face, 5.0);
    let inner = r.shrink(1.0);
    let mut x = inner.left() + 32.0;
    while x < inner.right() {
        p.vline(x, inner.y_range(), Stroke::new(1.0, face.grid));
        x += 32.0;
    }
    let mut y = inner.top() + 24.0;
    while y < inner.bottom() {
        p.hline(inner.x_range(), y, Stroke::new(1.0, face.grid));
        y += 24.0;
    }
    if !tag.is_empty() {
        tracked_text(
            p,
            Rect::from_min_size(r.min + vec2(10.0, 4.0), vec2(300.0, 14.0)),
            Align2::LEFT_CENTER,
            tag,
            9.5,
            0.16,
            true,
            true,
            alpha(face.well_ink, 0.5),
        );
    }
}

/// A trace on a display, with a soft glow under it.
pub fn trace(p: &Painter, pts: Vec<Pos2>, c: Color32, w: f32) {
    p.add(Shape::line(pts.clone(), Stroke::new(w + 3.0, alpha(c, 0.12))));
    p.add(Shape::line(pts, Stroke::new(w, c)));
}

/// A small note in a display's lower left corner.
pub fn display_note(p: &Painter, r: Rect, text: &str, face: &Face) {
    p.text(pos2(r.left() + 10.0, r.bottom() - 8.0), Align2::LEFT_BOTTOM, text, mono(10.0), alpha(face.well_ink, 0.6));
}

// ------------------------------------------------------------------------------- knobs

/// Where a synth's controls are and what they set.
pub struct Ctx<'a> {
    pub track: TrackId,
    pub ins: &'a Insert,
    pub info: &'static PluginInfo,
    pub face: &'a Face,
    /// The modulation drawn on the knobs (an arc for each source or macro target).
    pub arcs: &'a [ModArc],
    /// Destinations to light up (a source hovered, or one being dragged).
    pub lit: &'a [String],
    /// Whether `param` takes modulation (a source dropped on it patches it).
    pub patchable: &'a dyn Fn(&str) -> bool,
    /// What a gesture on an arc does (`None`: the knobs carry no patching).
    pub patching: Option<&'a dyn Patching>,
    /// Every knob's centre as drawn this frame, by parameter (where a dragged source lands; on
    /// the Back, every input jack's).
    pub centres: &'a RefCell<Vec<(String, Pos2)>>,
    /// The Back: the same layout with a jack where each control that takes modulation is.
    pub back: bool,
    /// The Back's output jacks as drawn this frame: source index ([`SOURCE_COLOURS`] order, then
    /// the macros from 7) and centre.
    pub outputs: &'a RefCell<Vec<(usize, Pos2)>>,
    /// Modulation is shown moving (off with Reduce motion: the arcs stay static).
    pub motion: bool,
    /// What the synth is doing now, read from the audio thread this frame (`None`: no reading,
    /// as when the audio is off).
    pub live: Option<LiveRead>,
    /// Destinations whose arcs light up brightly a moment (a macro just turned).
    pub flash: &'a [String],
}

/// The dot that shows where a modulated knob's value is now: its colour and radius (at the
/// knob's full size).
pub const LIVE_DOT: Color32 = Color32::from_rgb(0xff, 0xfb, 0xf0);
/// See [`LIVE_DOT`].
pub const LIVE_DOT_R: f32 = 2.7;

/// One arc on a knob: the range a source (or a macro) moves it over, relative to where the knob
/// stands, in the knob's travel; and the amount a drag on the arc changes.
#[derive(Clone, Debug)]
pub struct ModArc {
    pub dest: String,
    pub colour: Color32,
    pub lo: f32,
    pub hi: f32,
    pub amount: f32,
    /// What drives it, for its menu ("LFO", a macro's name).
    pub name: String,
    /// What drives it, by index: a source ([`SOURCE_COLOURS`] order), or a macro from 7.
    pub src: usize,
}

/// What the panel does with a gesture on an arc (index into [`Ctx::arcs`]).
pub trait Patching {
    /// The arc's amount dragged to `amount`, merged into the gesture until [`Patching::end`].
    fn set_amount(&self, app: &mut SoundApp, arc: usize, amount: f32);
    /// The drag on the arc ended.
    fn end(&self, app: &mut SoundApp);
    /// The arc's cable removed (one undo step).
    fn remove(&self, app: &mut SoundApp, arc: usize);
}

#[derive(Clone, Copy)]
struct ArcDrag {
    arc: usize,
    start: f32,
    start_y: f32,
}

impl Ctx<'_> {
    /// `id`'s description.
    pub fn param_info(&self, id: &str) -> Option<&'static ParamInfo> {
        self.info.params.iter().find(|p| p.id == id)
    }

    /// `id`'s value as the synth clamps it (its default when missing).
    pub fn value(&self, id: &str) -> f32 {
        self.param_info(id).map_or(0.0, |p| p.clamp(self.ins.params.get(id).copied().filter(|v| v.is_finite()).unwrap_or(p.default)))
    }

    /// The macros' offset on destination `id` now, in its travel (each target's `from` to `to`
    /// at its macro's position).
    pub fn macro_offset(&self, id: &str) -> f32 {
        let m = &self.ins.modulation;
        m.macros
            .iter()
            .enumerate()
            .flat_map(|(i, mac)| {
                let v = (self.value(soundcraft_dsp::plugins::synth_mod::MACRO_PARAMS.get(i).copied().unwrap_or("")) / 100.0).clamp(0.0, 1.0);
                mac.targets.iter().filter(move |t| t.dest == id).map(move |t| t.from + (t.to - t.from) * v)
            })
            .sum()
    }

    /// Where patched destination `id` is now, in its knob's travel (0..1): what the synth reads
    /// back for the voice it shows, else the knob plus the macros' offsets. `None` when nothing
    /// drives it, or with Reduce motion.
    pub fn live_pos(&self, id: &str) -> Option<f32> {
        if !self.motion || !self.arcs.iter().any(|a| a.dest == id) {
            return None;
        }
        if let Some(v) = self.live.as_ref().and_then(|l| l.dest(&self.ins.plugin, id)) {
            return Some(v);
        }
        let info = self.param_info(id)?;
        Some((info.to_normalized(self.value(id)) + self.macro_offset(id)).clamp(0.0, 1.0))
    }

    /// [`Ctx::live_pos`] in `id`'s own units.
    pub fn live_value(&self, id: &str) -> Option<f32> {
        let info = self.param_info(id)?;
        self.live_pos(id).map(|v| info.clamp(info.from_normalized(v)))
    }

    /// Source `i`'s level now for its light (0..1; bipolar sources centred), `None` with
    /// Reduce motion or no reading. Macros (from 7) read their knob.
    pub fn source_level(&self, i: usize) -> Option<f32> {
        if !self.motion {
            return None;
        }
        let n = soundcraft_dsp::plugins::synth_mod::N_SOURCES;
        if i >= n {
            let id = soundcraft_dsp::plugins::synth_mod::MACRO_PARAMS.get(i - n)?;
            return Some((self.value(id) / 100.0).clamp(0.0, 1.0));
        }
        let live = self.live.as_ref()?;
        let src = soundcraft_dsp::plugins::synth_mod::SOURCES.get(i)?;
        if src.per_voice && !live.sounding {
            return Some(0.0);
        }
        let v = *live.sources.get(i)?;
        Some(if src.lo < 0.0 { (v + 1.0) * 0.5 } else { v }.clamp(0.0, 1.0))
    }
}

/// A short reading of `v` for the print under a knob: "900 Hz", "2.4 kHz", "5 ms", "1.2 s".
pub fn short_value(info: &ParamInfo, v: f32) -> String {
    let v = info.clamp(v);
    match info.unit {
        Unit::Hz if v >= 1000.0 => format!("{:.1} kHz", v / 1000.0),
        Unit::Hz if v < 10.0 => format!("{v:.2} Hz"),
        Unit::Hz => format!("{v:.0} Hz"),
        Unit::Ms if v >= 1000.0 => format!("{:.2} s", v / 1000.0),
        Unit::Ms if v < 10.0 => format!("{v:.1} ms"),
        Unit::Ms => format!("{v:.0} ms"),
        Unit::Semitones => format!("{v:+.0} st"),
        Unit::Cents => format!("{v:+.0} ct"),
        Unit::Db => format!("{v:+.1} dB"),
        Unit::Choice => info.format(v).replace("Low-pass ", "LP ").replace("High-pass", "HP").replace("Band-pass", "BP").replace(" dB", ""),
        _ => info.format(v),
    }
}

/// [`short_value`] of parameter `id` as the synth has it now.
pub fn short_value_of(cx: &Ctx, id: &str) -> String {
    cx.param_info(id).map(|i| short_value(i, cx.value(id))).unwrap_or_default()
}

/// Sends `value` for `param` through `mix.instrument_param`, merged into the gesture `gesture`.
pub fn set_param(app: &mut SoundApp, track: TrackId, param: &str, value: f32, gesture: &str) {
    if let Err(e) = app.engine.execute_merged(
        "mix.instrument_param",
        &json!({"track": track.0, "param": param, "value": value}),
        &format!("synth_face:{}:{gesture}", track.0),
    ) {
        app.ui.status = e.to_string();
    }
}

/// The knob's size (its cap, ticks and arc), points.
pub const KNOB: f32 = 34.0;
/// A knob's cell: the knob, its label and value.
pub const KNOB_CELL: Pos2 = pos2(46.0, 60.0);

const A0: f32 = PI * 0.75;
const SWEEP: f32 = PI * 1.5;

/// Where the live dot of a knob at `c`, `size` across, is drawn for a value at `v01` of its
/// travel (on the outermost arc's ring).
pub fn live_dot_at(c: Pos2, size: f32, v01: f32) -> Pos2 {
    let a = A0 + SWEEP * v01.clamp(0.0, 1.0);
    c + vec2(a.cos(), a.sin()) * 20.0 * size / 58.0
}

/// The knob itself at `c`, `size` across, at `v01` of its travel: printed ticks (`steps` of them
/// for a stepped knob), modulation arcs (bright and haloed when `lit`: a macro just turned), the
/// cap with its shadow and gradient, the pointer, and where the modulated value is now (`live`,
/// a bright dot riding the arcs).
#[allow(clippy::too_many_arguments)]
pub fn paint_knob(
    p: &Painter,
    c: Pos2,
    size: f32,
    v01: f32,
    face: &Face,
    cap: (Color32, Color32, Color32),
    arcs: &[(Color32, f32, f32)],
    steps: usize,
    lit: bool,
    live: Option<f32>,
) {
    let s = size / 58.0;
    let pt = |a: f32, r: f32| c + vec2(a.cos(), a.sin()) * r * s;
    let stepped = (2..=12).contains(&steps);
    let n = if stepped { steps - 1 } else { 10 };
    for i in 0..=n {
        let t = A0 + SWEEP * i as f32 / n as f32;
        let major = stepped || i % 5 == 0;
        let (r1, w, a) = if major { (26.0, 1.6, 0.9) } else { (24.5, 1.0, 0.5) };
        p.line_segment([pt(t, 22.5), pt(t, r1)], Stroke::new(w * s.max(0.8), alpha(face.ink, a)));
    }
    let a = A0 + SWEEP * v01.clamp(0.0, 1.0);
    for (k, (col, lo, hi)) in arcs.iter().enumerate() {
        let from = A0 + SWEEP * (v01 + lo).clamp(0.0, 1.0);
        let to = A0 + SWEEP * (v01 + hi).clamp(0.0, 1.0);
        let r = 20.0 - k.min(3) as f32 * 3.4;
        if to - from < 0.02 {
            p.circle_filled(pt(a, r), 1.8 * s, *col);
            continue;
        }
        let segs = (((to - from) / 0.06).ceil() as usize).max(1);
        let arc: Vec<Pos2> = (0..=segs).map(|i| pt(from + (to - from) * i as f32 / segs as f32, r)).collect();
        if lit {
            p.add(Shape::line(arc.clone(), Stroke::new(6.5 * s, alpha(*col, 0.35))));
            p.add(Shape::line(arc, Stroke::new(3.6 * s, col.lerp_to_gamma(Color32::WHITE, 0.35))));
        } else {
            p.add(Shape::line(arc, Stroke::new(3.2 * s, *col)));
        }
    }
    if let Some(v) = live.filter(|_| !arcs.is_empty()) {
        let at = live_dot_at(c, size, v);
        let rad = LIVE_DOT_R * size / KNOB;
        p.circle_filled(at, rad + 1.1, alpha(Color32::BLACK, 0.55));
        p.circle_filled(at, rad, LIVE_DOT);
    }
    let (hi_c, lo_c, ptr) = cap;
    let r = 17.0 * s;
    if lit {
        p.circle_filled(c, r + 6.0 * s, alpha(face.accent, 0.18));
    }
    p.circle_filled(c + vec2(0.0, 1.8 * s), r, alpha(Color32::BLACK, 0.33));
    p.circle_filled(c, r, lo_c);
    // The cap's gradient: lighter towards the upper left.
    let ri = 15.5 * s;
    for i in 0..8 {
        let t = i as f32 / 8.0;
        let rr = ri * (1.0 - t * 0.75);
        let off = vec2(-0.32, -0.42) * ri * t;
        p.circle_filled(c + off, rr, lo_c.lerp_to_gamma(hi_c, 0.25 + t * 0.75));
    }
    p.circle_stroke(c, 10.5 * s, Stroke::new(1.0, alpha(Color32::WHITE, 0.09)));
    p.line_segment([pt(a, 6.0), pt(a, 14.5)], Stroke::new(2.6 * s, ptr));
}

/// What a knob did this frame.
pub struct KnobOut {
    pub response: Response,
    /// The knob's centre (where a dragged source lands).
    pub centre: Pos2,
}

/// A parameter knob in the cell `cell`: the knob, its label printed under it and its value small
/// beneath. `id` is the parameter.
pub fn knob(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, cell: Rect, id: &str, label: &str) -> KnobOut {
    knob_ex(app, ui, cx, cell, id, label, None)
}

/// [`knob`] with its own cap colours (the macros' accent caps).
pub fn knob_ex(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, cell: Rect, id: &str, label: &str, cap: Option<(Color32, Color32, Color32)>) -> KnobOut {
    let face = cx.face;
    let centre = pos2(cell.center().x, cell.top() + KNOB * 0.5 + 1.0);
    let krect = Rect::from_center_size(centre, vec2(KNOB, KNOB));
    if cx.back {
        return back_control(ui, cx, cell, centre, id, label);
    }
    let Some(info) = cx.param_info(id) else {
        let resp = ui.interact(krect, Id::new(("synth-knob-missing", id)), Sense::hover());
        return KnobOut { response: resp, centre };
    };
    let v = cx.value(id);
    let v01 = info.to_normalized(v);
    let value_text = short_value(info, v);
    let opts =
        DialOpts { label: info.name, value_text: &value_text, default: info.to_normalized(info.default), info: Some(info), ..Default::default() };
    let wid = Id::new(("synth-knob", cx.track.0, id));
    cx.centres.borrow_mut().push((id.to_string(), centre));
    let mine: Vec<usize> = cx.arcs.iter().enumerate().filter(|(_, a)| a.dest == id).map(|(i, _)| i).collect();
    // A press on the ring (outside the cap) of a modulated knob drags its arc's amount instead.
    let arc_key = wid.with("arc");
    let out = dial_input(ui, krect, wid, v01, &opts);
    let mut arc_drag = ui.data(|d| d.get_temp::<ArcDrag>(arc_key));
    if out.response.drag_started()
        && cx.patching.is_some()
        && let Some(&last) = mine.last()
        && let Some(at) = ui.input(|i| i.pointer.press_origin())
        && at.distance(centre) > KNOB * 17.0 / 58.0
    {
        let a = ArcDrag { arc: last, start: cx.arcs[last].amount, start_y: at.y };
        ui.data_mut(|d| d.insert_temp(arc_key, a));
        arc_drag = Some(a);
    }
    let mut shown = v01;
    if let (Some(a), Some(h)) = (arc_drag, cx.patching) {
        if out.response.dragged()
            && let Some(p) = out.response.interact_pointer_pos()
        {
            let sens = if ui.input(|i| i.modifiers.shift) { 0.002 } else { 0.008 };
            let amount = (a.start + (a.start_y - p.y) * sens).clamp(-1.0, 1.0);
            if cx.arcs.get(a.arc).is_some_and(|x| (x.amount - amount).abs() > 1e-4) {
                h.set_amount(app, a.arc, amount);
            }
        }
        if !out.response.dragged() || out.ended {
            ui.data_mut(|d| d.remove::<ArcDrag>(arc_key));
            h.end(app);
        }
    } else {
        if let Some(n) = out.value {
            let new = info.clamp(info.from_normalized(n));
            shown = info.to_normalized(new);
            if (new - v).abs() > f32::EPSILON {
                set_param(app, cx.track, id, new, id);
            }
        }
        if out.ended {
            app.engine.end_merge();
        }
    }
    if let Some(h) = cx.patching
        && !mine.is_empty()
    {
        out.response.context_menu(|ui| {
            for &i in &mine {
                let name = cx.arcs.get(i).map_or(String::new(), |a| a.name.clone());
                if ui.button(format!("Remove {name} from {}", info.name)).clicked() {
                    h.remove(app, i);
                    ui.close();
                }
            }
        });
    }
    let steps = if info.unit == Unit::Choice { info.choices.len() } else { 0 };
    let arcs: Vec<(Color32, f32, f32)> = mine.iter().filter_map(|i| cx.arcs.get(*i)).map(|a| (a.colour, a.lo, a.hi)).collect();
    let lit = cx.lit.iter().any(|d| d == id);
    let caps = cap.unwrap_or((face.cap_hi, face.cap_lo, face.pointer));
    let p = ui.painter();
    let grow = if lit { 1.08 } else { 1.0 };
    let flash = cx.flash.iter().any(|d| d == id);
    paint_knob(p, centre, KNOB * grow, shown, face, caps, &arcs, steps, flash, cx.live_pos(id));
    let lr = label_row(cell, krect.bottom());
    tracked_text(p, lr, Align2::CENTER_CENTER, label, 10.0, 0.04, true, true, face.ink);
    let shown_text = if out.value.is_some() { short_value(info, info.from_normalized(shown)) } else { value_text.clone() };
    p.text(pos2(cell.center().x, lr.bottom() - 1.0), Align2::CENTER_TOP, shown_text, mono(9.5), face.ink2);
    let full = if arcs.is_empty() {
        format!("{}: {}", info.name, info.format(v))
    } else {
        format!("{}: {} (modulated: drag the ring to set how far, right-click to remove)", info.name, info.format(v))
    };
    let response = out.response.on_hover_text(full);
    KnobOut { response, centre }
}

/// A row of knob cells across `r`, evenly spaced, `n` of them: their rectangles.
pub fn cells(r: Rect, n: usize) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let w = r.width() / n as f32;
    (0..n).map(|i| Rect::from_min_size(pos2(r.left() + w * i as f32 + (w - KNOB_CELL.x) * 0.5, r.top()), vec2(KNOB_CELL.x, KNOB_CELL.y))).collect()
}

/// A section's share of a row: `Knobs(n)` takes room in proportion to its knobs, `Fixed(w)` is
/// `w` points wide (the faders).
#[derive(Clone, Copy, Debug)]
pub enum Share {
    Knobs(usize),
    Fixed(f32),
}

/// The gap between sections in a row.
pub const GAP: f32 = 8.0;
/// A section's height for one row of knobs: the frame's top margin, the cells, the bottom margin.
pub const SECTION_H: f32 = 12.0 + 60.0 + 4.0;

/// Splits row `r` into its sections' rectangles, `GAP` apart.
pub fn row(r: Rect, shares: &[Share]) -> Vec<Rect> {
    let fixed: f32 = shares.iter().map(|s| if let Share::Fixed(w) = s { *w } else { 0.0 }).sum();
    let knobs: usize = shares.iter().map(|s| if let Share::Knobs(n) = s { *n } else { 0 }).sum();
    let free = (r.width() - fixed - GAP * shares.len().saturating_sub(1) as f32).max(0.0);
    let per = if knobs > 0 { free / knobs as f32 } else { 0.0 };
    let mut x = r.left();
    shares
        .iter()
        .map(|s| {
            let w = match s {
                Share::Knobs(n) => per * *n as f32,
                Share::Fixed(w) => *w,
            };
            let out = Rect::from_min_size(pos2(x, r.top()), vec2(w, r.height()));
            x += w + GAP;
            out
        })
        .collect()
}

/// The knob cells inside section `s` (its frame margins left out).
pub fn section_cells(s: Rect, n: usize) -> Vec<Rect> {
    cells(Rect::from_min_max(s.min + vec2(3.0, 12.0), s.max - vec2(3.0, 4.0)), n)
}

/// Four ADSR faders inside section `s` for the parameters `ids` (attack, decay, sustain,
/// release), their caps lined in `colour`.
pub fn adsr(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, s: Rect, ids: [&str; 4], colour: Color32) {
    let inner = Rect::from_min_max(s.min + vec2(6.0, 12.0), s.max - vec2(6.0, 3.0));
    let w = inner.width() / 4.0;
    for (i, (id, label)) in ids.iter().zip(["A", "D", "S", "R"]).enumerate() {
        let r = Rect::from_min_size(pos2(inner.left() + w * i as f32 + (w - FADER.x) * 0.5, inner.top()), vec2(FADER.x, inner.height()));
        fader(app, ui, cx, r, id, label, colour);
    }
}

/// A two-way switch (a choice of two, Off and On): a small lit key; a click flips it (one undo
/// step).
pub fn switch(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, cell: Rect, id: &str, label: &str) -> Response {
    let face = cx.face;
    let on = cx.value(id) >= 0.5;
    let key = Rect::from_center_size(pos2(cell.center().x, cell.top() + KNOB * 0.5 + 1.0), vec2(22.0, 14.0));
    if cx.back {
        return back_control(ui, cx, cell, key.center(), id, label).response;
    }
    let resp = ui.interact(key, Id::new(("synth-switch", cx.track.0, id)), Sense::click());
    let name = cx.param_info(id).map_or(label, |i| i.name);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, name));
    if resp.clicked() {
        app.engine.end_merge();
        set_param(app, cx.track, id, if on { 0.0 } else { 1.0 }, id);
        app.engine.end_merge();
    }
    let p = ui.painter();
    p.rect_filled(key.translate(vec2(0.0, 2.0)), cr(3.0), face.line);
    p.rect_filled(key, cr(3.0), face.key);
    p.rect_stroke(key, cr(3.0), Stroke::new(1.0, face.line), StrokeKind::Inside);
    let led = Rect::from_center_size(key.center(), vec2(12.0, 4.0));
    p.rect_filled(led, cr(2.0), if on { face.accent } else { alpha(face.ink, 0.25) });
    if on {
        p.rect_filled(led.expand(2.0), cr(3.0), alpha(face.accent, 0.2));
    }
    let lr = label_row(cell, cell.top() + KNOB + 1.0);
    tracked_text(p, lr, Align2::CENTER_CENTER, label, 10.0, 0.04, true, true, face.ink);
    p.text(pos2(cell.center().x, lr.bottom() - 1.0), Align2::CENTER_TOP, if on { "On" } else { "Off" }, mono(9.5), face.ink2);
    resp.on_hover_text(format!("{}: {}", cx.param_info(id).map_or(label, |i| i.name), if on { "On" } else { "Off" }))
}

// ------------------------------------------------------------------------------- faders

#[derive(Clone, Copy)]
struct FaderDrag {
    start_y: f32,
    start: f32,
}

/// The height of a fader's slot and label.
pub const FADER: Pos2 = pos2(22.0, 60.0);

/// An envelope fader in `r` for `id`: a slot with printed ticks and a cap with a line in
/// `colour`; dragging moves the cap with the pointer (Shift for fine), double-click returns to
/// the default, Ctrl+click (Cmd+click) types a value, one undo step per gesture.
pub fn fader(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, r: Rect, id: &str, label: &str, colour: Color32) -> Response {
    let face = cx.face;
    let slot = Rect::from_min_max(pos2(r.center().x - 2.0, r.top() + 4.0), pos2(r.center().x + 2.0, r.bottom() - 15.0));
    let travel = (slot.height() - 10.0).max(10.0);
    if cx.back {
        return back_control(ui, cx, r, pos2(r.center().x, r.top() + 22.0), id, label).response;
    }
    let Some(info) = cx.param_info(id) else { return ui.interact(r, Id::new(("synth-fader-missing", id)), Sense::hover()) };
    let v = cx.value(id);
    let v01 = info.to_normalized(v);
    let wid = Id::new(("synth-fader", cx.track.0, id));
    let hit = Rect::from_min_max(pos2(r.left(), slot.top() - 4.0), pos2(r.right(), slot.bottom() + 4.0));
    let resp = ui.interact(hit, wid, Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, info.name));
    let mut new01 = None;
    let mut ended = false;
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    if resp.drag_started() {
        let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).map_or(0.0, |p| p.y);
        ui.data_mut(|d| d.insert_temp(wid.with("drag"), FaderDrag { start_y: press, start: v01 }));
    }
    if resp.dragged()
        && let (Some(p), Some(d)) = (resp.interact_pointer_pos(), ui.data(|d| d.get_temp::<FaderDrag>(wid.with("drag"))))
    {
        let fine = if ui.input(|i| i.modifiers.shift) { 0.25 } else { 1.0 };
        new01 = Some((d.start + (d.start_y - p.y) / travel * fine).clamp(0.0, 1.0));
    }
    if resp.drag_stopped() {
        ended = true;
    }
    if resp.double_clicked() {
        new01 = Some(info.to_normalized(info.default));
        ended = true;
    }
    if crate::value_entry::wants_entry(ui, &resp) {
        crate::value_entry::open(ui, wid, &short_value(info, v));
    }
    if let Some(x) = crate::value_entry::show(ui, wid, hit).and_then(|t| crate::value_entry::parse(info, &t)) {
        new01 = Some(info.to_normalized(x));
        ended = true;
    }
    let mut shown = v01;
    if let Some(n) = new01 {
        let val = info.clamp(info.from_normalized(n));
        shown = info.to_normalized(val);
        if (val - v).abs() > f32::EPSILON {
            set_param(app, cx.track, id, val, id);
        }
    }
    if ended {
        app.engine.end_merge();
    }
    let p = ui.painter();
    p.rect_filled(slot, cr(2.0), alpha(Color32::BLACK, 0.45));
    for i in 0..5 {
        let y = slot.top() + 5.0 + travel * i as f32 / 4.0;
        p.hline(r.left()..=r.left() + 4.0, y, Stroke::new(1.0, alpha(face.ink, 0.6)));
    }
    let y = slot.bottom() - 5.0 - travel * shown;
    let cap = Rect::from_center_size(pos2(r.center().x, y), vec2(16.0, 10.0));
    p.rect_filled(cap.translate(vec2(0.0, 1.5)), cr(2.0), face.cap_lo);
    p.rect_filled(cap, cr(2.0), face.cap_hi);
    p.hline(cap.left() + 1.0..=cap.right() - 1.0, cap.center().y, Stroke::new(2.0, colour));
    let lr = Rect::from_min_size(pos2(r.left() - 4.0, slot.bottom() + 2.0), vec2(r.width() + 8.0, 12.0));
    tracked_text(p, lr, Align2::CENTER_CENTER, label, 10.0, 0.04, true, true, face.ink);
    resp.on_hover_text(format!("{}: {}", info.name, info.format(v)))
}

// ------------------------------------------------------------------------------- source chips

/// A source chip: a jack-like ring in the source's colour and its name on a key, its middle lit
/// by the source's level now (`level`, 0..1; `None`: dark). Returns the response (drag it onto a
/// knob to patch).
pub fn chip(
    ui: &mut Ui,
    r: Rect,
    salt: impl std::hash::Hash + std::fmt::Debug,
    text: &str,
    colour: Color32,
    face: &Face,
    level: Option<f32>,
) -> Response {
    let resp = ui.interact(r, Id::new(("synth-chip", salt)), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
    let p = ui.painter();
    p.rect_filled(r.translate(vec2(0.0, 2.0)), cr(4.0), face.line);
    p.rect_filled(r, cr(4.0), face.key);
    p.rect_stroke(r, cr(4.0), Stroke::new(1.0, face.line), StrokeKind::Inside);
    if resp.hovered() || resp.dragged() {
        p.rect_stroke(r.expand(2.0), cr(5.0), Stroke::new(2.0, colour), StrokeKind::Inside);
        ui.ctx().set_cursor_icon(if resp.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab });
    }
    let jack = pos2(r.left() + 11.0, r.center().y);
    p.circle_filled(jack, 5.5, Color32::from_rgb(0x16, 0x13, 0x0f));
    p.circle_stroke(jack, 4.2, Stroke::new(2.6, colour));
    p.circle_stroke(jack, 5.5, Stroke::new(1.0, alpha(Color32::BLACK, 0.3)));
    level_light(p, jack, 3.4, colour, level);
    tracked_text(p, Rect::from_min_max(pos2(jack.x + 9.0, r.top()), r.max), Align2::LEFT_CENTER, text, 9.5, 0.05, true, true, face.ink);
    resp
}

/// A small light in a jack's middle at `c` glowing with a source's level (0..1): the source's
/// colour, brighter and with a faint halo as it rises. Nothing when `level` is `None` or 0.
pub fn level_light(p: &Painter, c: Pos2, rad: f32, colour: Color32, level: Option<f32>) {
    let Some(l) = level.filter(|l| l.is_finite() && *l > 0.01).map(|l| l.clamp(0.0, 1.0)) else { return };
    p.circle_filled(c, rad + 3.0 * l, alpha(colour, 0.22 * l));
    p.circle_filled(c, rad, colour.lerp_to_gamma(Color32::WHITE, 0.5 * l).gamma_multiply(0.35 + 0.65 * l));
}

/// The width a [`chip`] needs for `text`.
pub fn chip_width(p: &Painter, text: &str) -> f32 {
    tracked_width(p, text, 9.5, 0.05, true) + 26.0
}

/// The label row under a control whose body ends at `bottom`.
fn label_row(cell: Rect, bottom: f32) -> Rect {
    Rect::from_min_size(pos2(cell.left() - 8.0, bottom + 1.0), vec2(cell.width() + 16.0, 12.0))
}

/// The Back's face: the same instrument's rear plate, darker, with light print.
pub fn rear(face: &Face) -> Face {
    Face {
        panel: face.rear,
        brushed: false,
        ink: Color32::from_rgb(0xe8, 0xe2, 0xd6),
        ink2: Color32::from_rgb(0x9c, 0x95, 0x8a),
        line: Color32::from_rgb(0x55, 0x51, 0x4b),
        key: Color32::from_rgb(0x3a, 0x37, 0x33),
        on_accent: face.on_accent,
        plates: true,
        ..*face
    }
}

/// An input jack on the Back (a socket a cable plugs into).
pub fn input_jack(p: &Painter, c: Pos2, lit: bool) {
    p.circle_filled(c + vec2(0.0, 1.0), 9.5, alpha(Color32::BLACK, 0.4));
    p.circle_filled(c, 9.5, Color32::from_rgb(0x9a, 0x96, 0x8e));
    p.circle_stroke(c, 9.0, Stroke::new(1.0, alpha(Color32::WHITE, 0.25)));
    p.circle_filled(c, 6.5, Color32::from_rgb(0x1e, 0x1d, 0x1b));
    p.circle_filled(c, 2.6, Color32::from_rgb(4, 4, 4));
    if lit {
        p.circle_stroke(c, 12.0, Stroke::new(2.0, alpha(Color32::WHITE, 0.5)));
    }
}

/// An output jack on the Back, ringed in its source's colour.
pub fn output_jack(p: &Painter, c: Pos2, colour: Color32) {
    p.circle_filled(c + vec2(0.0, 1.0), 9.5, alpha(Color32::BLACK, 0.4));
    // A coloured bezel round the nut marks an output.
    p.circle_filled(c, 9.5, colour.lerp_to_gamma(Color32::BLACK, 0.25));
    p.circle_filled(c, 8.0, Color32::from_rgb(0x9a, 0x96, 0x8e));
    p.circle_filled(c, 6.8, Color32::from_rgb(0x1e, 0x1d, 0x1b));
    p.circle_stroke(c, 6.0, Stroke::new(2.0, colour));
    p.circle_filled(c, 2.6, Color32::from_rgb(4, 4, 4));
}

/// A blank on the Back where a control that takes no modulation is: a small capped plate.
fn blank(p: &Painter, c: Pos2, face: &Face) {
    p.circle_filled(c, 6.0, alpha(face.line, 0.7));
    p.circle_filled(c, 3.0, alpha(Color32::BLACK, 0.35));
}

/// A control as the Back shows it: an input jack (it takes modulation) or a blank, with its label.
fn back_control(ui: &mut Ui, cx: &Ctx, cell: Rect, centre: Pos2, id: &str, label: &str) -> KnobOut {
    let p = ui.painter();
    let takes = (cx.patchable)(id);
    if takes {
        input_jack(p, centre, cx.lit.iter().any(|d| d == id));
        cx.centres.borrow_mut().push((id.to_string(), centre));
    } else {
        blank(p, centre, cx.face);
    }
    let lr = label_row(cell, cell.top() + KNOB + 1.0);
    let ink = if takes { cx.face.ink } else { cx.face.ink2 };
    // A plate under the name, so a cable passing never hides it.
    let w = tracked_width(p, label, 10.0, 0.04, true);
    p.rect_filled(Rect::from_center_size(lr.center(), vec2(w + 6.0, 12.0)), cr(2.0), alpha(cx.face.panel_at(0.5), 0.92));
    tracked_text(p, lr, Align2::CENTER_CENTER, label, 10.0, 0.04, true, true, ink);
    let response = ui.interact(Rect::from_center_size(centre, vec2(2.0, 2.0)), Id::new(("synth-back-jack", cx.track.0, id)), Sense::hover());
    if takes {
        let name = cx.param_info(id).map_or(label, |i| i.name);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, format!("{name} socket")));
    }
    KnobOut { response, centre }
}

/// A display as the Back shows it: a plain rear plate with the display's name and four screws.
pub fn rear_plate(p: &Painter, r: Rect, tag: &str, face: &Face) {
    p.rect_filled(r, cr(5.0), alpha(Color32::BLACK, 0.25));
    p.rect_stroke(r, cr(5.0), Stroke::new(1.0, alpha(face.line, 0.9)), StrokeKind::Inside);
    for c in [r.left_top() + vec2(8.0, 8.0), r.right_top() + vec2(-8.0, 8.0), r.left_bottom() + vec2(8.0, -8.0), r.right_bottom() + vec2(-8.0, -8.0)]
    {
        p.circle_filled(c, 3.0, alpha(face.ink2, 0.6));
        p.line_segment([c - vec2(2.0, 0.0), c + vec2(2.0, 0.0)], Stroke::new(1.0, alpha(Color32::BLACK, 0.6)));
    }
    tracked_text(
        p,
        Rect::from_center_size(r.center(), vec2(r.width(), 14.0)),
        Align2::CENTER_CENTER,
        tag,
        9.5,
        0.2,
        true,
        true,
        alpha(face.ink2, 0.8),
    );
}

/// The width of an envelope's section (four faders, and room on its frame for its output).
pub const ENV_W: f32 = 136.0;

/// The short tags printed beside the sources' output jacks on the Back, in source order.
pub const OUT_TAGS: [&str; 7] = ["AMP", "ENV 2", "LFO", "VEL", "WHL", "KEY", "RND"];

/// The Back's output jacks of the sources `which` ([`SOURCE_COLOURS`] order) on section `s`:
/// set into the top of its frame at the right end, right to left, each a socket ringed in its
/// source's colour with its tag beside it, so outputs always sit where the eye learns to look.
/// Nothing on the Front. Each is registered in [`Ctx::outputs`].
pub fn outputs_on(ui: &mut Ui, cx: &Ctx, s: Rect, which: &[usize]) {
    if !cx.back {
        return;
    }
    let p = ui.painter().clone();
    let mut x = s.right() - 10.0;
    for &i in which.iter().rev() {
        let c = pos2(x - 8.0, s.top());
        let tag = OUT_TAGS.get(i).copied().unwrap_or("");
        let w = tracked_width(&p, tag, 8.5, 0.06, true);
        let plate = Rect::from_min_max(pos2(c.x - 11.0 - w - 2.0, c.y - 6.5), pos2(c.x + 10.0, c.y + 6.5));
        p.rect_filled(plate, cr(3.0), alpha(cx.face.panel_at(0.5), 0.95));
        let colour = SOURCE_COLOURS.get(i).copied().unwrap_or(Color32::WHITE);
        output_jack(&p, c, colour);
        level_light(&p, c, 2.6, colour, cx.source_level(i));
        tracked_text(
            &p,
            Rect::from_min_max(pos2(plate.left() + 2.0, c.y - 6.0), pos2(c.x - 10.0, c.y + 6.0)),
            Align2::RIGHT_CENTER,
            tag,
            8.5,
            0.06,
            true,
            true,
            cx.face.ink,
        );
        cx.outputs.borrow_mut().push((i, c));
        let name = soundcraft_dsp::plugins::synth_mod::SOURCES.get(i).map_or(tag, |s| s.name);
        let r = ui.interact(Rect::from_center_size(c, vec2(2.0, 2.0)), Id::new(("synth-back-out", cx.track.0, i)), Sense::hover());
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, format!("{name} out")));
        x = plate.left() - 2.0;
    }
}

//! Patching on the synth panels, inside one synth: the modulation sources as chips to drag onto
//! a knob (a coloured arc on the knob then shows the range it moves over; drag the arc to set the
//! amount, right-click the knob to remove), the four macro knobs (drag a macro's name onto a knob
//! to give it a target, double-click the name to rename it), Surprise, and the Back of the panel:
//! the same patch as cables between source jacks and destination jacks, dragged to re-patch and
//! pulled out to remove. Every change is one undo step (`mix.mod_*`, `mix.macro_*`).

use super::kit::{self, Ctx, Face, ModArc};
use crate::SoundApp;
use egui::{Align2, Color32, CursorIcon, Id, LayerId, Order, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::plugins::synth_mod::{self as sm, SOURCES};
use soundcraft_model::{Insert, TrackId};

/// What drew an arc on a knob: a slot of the matrix, or a macro's target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ArcKind {
    Slot(usize),
    /// Macro (0-based) and its target's index.
    Macro(usize, usize),
}

/// The colour of source `i` ([`SOURCES`] order).
pub fn source_colour(i: usize) -> Color32 {
    kit::SOURCE_COLOURS.get(i).copied().unwrap_or(Color32::WHITE)
}

/// The arcs of `ins`'s patch, and what drew each: every slot over its source's reach, every macro
/// target over its range (the macros in the accent).
pub fn arcs(ins: &Insert, face: &Face) -> (Vec<ModArc>, Vec<ArcKind>) {
    let (mut out, mut kinds) = (Vec::new(), Vec::new());
    for (i, s) in ins.modulation.slots.iter().enumerate() {
        let Some(si) = sm::source_index(&s.source) else { continue };
        let src = &SOURCES[si];
        let (a, b) = (s.amount * src.lo, s.amount * src.hi);
        out.push(ModArc {
            dest: s.dest.clone(),
            colour: source_colour(si),
            lo: a.min(b),
            hi: a.max(b),
            amount: s.amount,
            name: src.name.to_string(),
            src: si,
        });
        kinds.push(ArcKind::Slot(i));
    }
    for (m, mac) in ins.modulation.macros.iter().enumerate() {
        for (t, tg) in mac.targets.iter().enumerate() {
            let name = ins.modulation.macro_name(m);
            out.push(ModArc {
                dest: tg.dest.clone(),
                colour: face.accent,
                lo: tg.from.min(tg.to),
                hi: tg.from.max(tg.to),
                amount: tg.to,
                name,
                src: SOURCES.len() + m,
            });
            kinds.push(ArcKind::Macro(m, t));
        }
    }
    (out, kinds)
}

/// The panel's answer to gestures on the arcs ([`kit::Patching`]).
pub struct Hooks<'a> {
    pub track: TrackId,
    pub ins: &'a Insert,
    pub kinds: &'a [ArcKind],
}

impl kit::Patching for Hooks<'_> {
    fn set_amount(&self, app: &mut SoundApp, arc: usize, amount: f32) {
        if let Some(k) = self.kinds.get(arc) {
            set_arc_amount(app, self.track, self.ins, *k, amount, "arc");
        }
    }

    fn end(&self, app: &mut SoundApp) {
        app.engine.end_merge();
    }

    fn remove(&self, app: &mut SoundApp, arc: usize) {
        if let Some(k) = self.kinds.get(arc) {
            remove_arc(app, self.track, self.ins, *k);
        }
    }
}

/// What is being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    /// A source ([`SOURCES`] index) from its chip or jack.
    Source(usize),
    /// A macro (0-based) from its name or jack.
    Macro(usize),
    /// A slot's cable by its destination end (the Back).
    Cable(usize),
    /// A macro target's cable by its destination end (the Back).
    MacroCable(usize, usize),
}

/// The panel's own state (not saved): which side shows, what is dragged and from where, when the
/// cables were last moved (they sway a moment), a macro being renamed.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub back: bool,
    pub drag: Option<(Drag, Pos2)>,
    pub moved_at: f64,
    pub rename: Option<(usize, String)>,
    /// The source chip hovered last frame (its targets light up).
    pub hover_source: Option<usize>,
    /// The Back's input jack whose amount knob shows.
    pub amount_at: Option<String>,
    /// The macro (0-based) whose knob or name was hovered last frame (its targets light up).
    pub hover_macro: Option<usize>,
    /// The macro last turned, and when (its targets' arcs light up a moment).
    pub macro_touch: Option<(usize, f64)>,
}

/// How long a turned macro's targets stay lit, seconds.
pub const MACRO_FLASH_S: f64 = 0.8;

/// The destinations macro `m` (0-based) of `ins` drives.
pub fn macro_targets(ins: &Insert, m: usize) -> Vec<String> {
    ins.modulation.macros.get(m).map(|x| x.targets.iter().map(|t| t.dest.clone()).collect()).unwrap_or_default()
}

fn state_id(track: TrackId) -> Id {
    Id::new(("synth-face-state", track.0))
}

pub fn state(ctx: &egui::Context, track: TrackId) -> State {
    ctx.data(|d| d.get_temp::<State>(state_id(track))).unwrap_or_default()
}

pub fn set_state(ctx: &egui::Context, track: TrackId, st: State) {
    ctx.data_mut(|d| d.insert_temp(state_id(track), st));
}

/// Runs `cmd` with `p` (one undo step); a refusal shows in the status line.
fn run(app: &mut SoundApp, cmd: &str, p: Value) -> bool {
    match app.run(cmd, p) {
        Ok(_) => true,
        Err(e) => {
            app.ui.status = e;
            false
        }
    }
}

/// The destinations `ins`'s synth takes.
pub fn dests(plugin: &str) -> &'static [&'static str] {
    sm::dests(plugin)
}

/// The name a destination is shown by.
pub fn dest_name(plugin: &str, id: &str) -> String {
    sm::dest_info(plugin, id).map_or_else(|| id.to_string(), |i| i.name.to_string())
}

/// What a drop of `drag` on destination `dest` does: patch it (a source), give the macro a target,
/// or move a cable's end there. Returns whether something changed.
fn drop_on(app: &mut SoundApp, track: TrackId, ins: &Insert, drag: Drag, dest: &str) -> bool {
    let t = track.0;
    match drag {
        Drag::Source(i) => run(app, "mix.mod_add", json!({"track": t, "source": SOURCES[i].id, "dest": dest, "amount": 0.25})),
        Drag::Macro(m) => run(app, "mix.macro_assign", json!({"track": t, "macro": m + 1, "dest": dest, "amount": 0.5})),
        Drag::Cable(slot) => {
            let same = ins.modulation.slots.get(slot).is_some_and(|s| s.dest == dest);
            !same && run(app, "mix.mod_set", json!({"track": t, "slot": slot, "dest": dest}))
        }
        Drag::MacroCable(m, ti) => {
            let Some(tg) = ins.modulation.macros.get(m).and_then(|x| x.targets.get(ti)).cloned() else { return false };
            if tg.dest == dest {
                return false;
            }
            // Moving a macro's target is two changes, one undo step.
            app.engine.end_merge();
            let key = format!("synth_face:{t}:macro-move");
            let a = app.engine.execute_merged("mix.macro_assign", &json!({"track": t, "macro": m + 1, "dest": tg.dest, "remove": true}), &key);
            let b =
                app.engine.execute_merged("mix.macro_assign", &json!({"track": t, "macro": m + 1, "dest": dest, "from": tg.from, "to": tg.to}), &key);
            app.engine.end_merge();
            if let Err(e) = a.and(b) {
                app.ui.status = e.to_string();
            }
            true
        }
    }
}

/// A cable's end pulled out on the Back (dropped on no jack): the slot or macro target removed.
fn pull_out(app: &mut SoundApp, track: TrackId, ins: &Insert, drag: Drag) -> bool {
    match drag {
        Drag::Cable(slot) => run(app, "mix.mod_remove", json!({"track": track.0, "slot": slot})),
        Drag::MacroCable(m, ti) => {
            let Some(d) = ins.modulation.macros.get(m).and_then(|x| x.targets.get(ti)).map(|tg| tg.dest.clone()) else { return false };
            run(app, "mix.macro_assign", json!({"track": track.0, "macro": m + 1, "dest": d, "remove": true}))
        }
        _ => false,
    }
}

/// Removes what drew `arc` (one undo step).
pub fn remove_arc(app: &mut SoundApp, track: TrackId, ins: &Insert, kind: ArcKind) {
    match kind {
        ArcKind::Slot(i) => {
            pull_out(app, track, ins, Drag::Cable(i));
        }
        ArcKind::Macro(m, t) => {
            pull_out(app, track, ins, Drag::MacroCable(m, t));
        }
    }
}

/// Sets the amount behind `arc` to `amount`, merged into the open gesture `gesture`.
pub fn set_arc_amount(app: &mut SoundApp, track: TrackId, ins: &Insert, kind: ArcKind, amount: f32, gesture: &str) {
    let key = format!("synth_face:{}:{gesture}", track.0);
    let amount = amount.clamp(-1.0, 1.0);
    let r = match kind {
        ArcKind::Slot(i) => app.engine.execute_merged("mix.mod_set", &json!({"track": track.0, "slot": i, "amount": amount}), &key),
        ArcKind::Macro(m, t) => {
            let Some(tg) = ins.modulation.macros.get(m).and_then(|x| x.targets.get(t)) else { return };
            app.engine.execute_merged(
                "mix.macro_assign",
                &json!({"track": track.0, "macro": m + 1, "dest": tg.dest, "from": tg.from, "to": amount}),
                &key,
            )
        }
    };
    if let Err(e) = r {
        app.ui.status = e.to_string();
    }
}

/// A cable from `a` to `b` hanging with a sag (and a sway `sway` points, swinging), with a shadow
/// and plugs at both ends.
pub fn cable(p: &Painter, a: Pos2, b: Pos2, colour: Color32, sway: f32) {
    for sh in cable_shapes(a, b, colour, sway) {
        p.add(sh);
    }
    for e in [a, b] {
        p.circle_filled(e, 4.5, colour.lerp_to_gamma(Color32::BLACK, 0.3));
        p.circle_filled(e, 2.8, colour);
    }
}

/// [`cable`]'s rope without its plugs, as shapes: shadow, body, highlight.
pub fn cable_shapes(a: Pos2, b: Pos2, colour: Color32, sway: f32) -> Vec<Shape> {
    let d = a.distance(b);
    let sag = 18.0 + d * 0.18 + sway;
    let dx = (b.x - a.x) * 0.3;
    // The rope hangs below the higher end and leaves the lower end gently, so a cable running up
    // the panel does not dip under its own jack first.
    let (sa, sb) = if a.y > b.y + 20.0 {
        (sag * 0.25, sag)
    } else if b.y > a.y + 20.0 {
        (sag, sag * 0.25)
    } else {
        (sag, sag)
    };
    let c1 = a + vec2(dx, sa);
    let c2 = b + vec2(-dx, sb);
    let pts: Vec<Pos2> = (0..=40)
        .map(|i| {
            let t = i as f32 / 40.0;
            let u = 1.0 - t;
            let v = a.to_vec2() * (u * u * u) + c1.to_vec2() * (3.0 * u * u * t) + c2.to_vec2() * (3.0 * u * t * t) + b.to_vec2() * (t * t * t);
            v.to_pos2()
        })
        .collect();
    vec![
        Shape::line(pts.iter().map(|q| *q + vec2(0.0, 2.5)).collect(), Stroke::new(5.5, kit::alpha(Color32::BLACK, 0.45))),
        Shape::line(pts.clone(), Stroke::new(3.8, colour)),
        Shape::line(pts.iter().map(|q| *q - vec2(0.0, 1.0)).collect(), Stroke::new(1.0, kit::alpha(Color32::WHITE, 0.25))),
    ]
}

/// How far the cables swing now, `t` seconds after they were moved (none with Reduce motion).
pub fn sway(now: f64, moved_at: f64, reduce_motion: bool) -> f32 {
    let t = (now - moved_at) as f32;
    if reduce_motion || !(0.0..1.6).contains(&t) {
        return 0.0;
    }
    16.0 * (std::f32::consts::TAU * 1.4 * t).sin() * (-t * 2.6).exp()
}

/// The foreground painter for what is dragged (over every window).
fn top_painter(ui: &Ui, track: TrackId) -> Painter {
    ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::new(("synth-face-drag", track.0))))
}

/// Where a dragged source or macro is dropped on the Front: the knob under the pointer takes it.
/// Draws the cable being dragged. Call after every knob was drawn (their centres recorded).
pub fn front_drop(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, st: &mut State) {
    let Some((drag, from)) = st.drag else { return };
    if !matches!(drag, Drag::Source(_) | Drag::Macro(_)) {
        return;
    }
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let colour = match drag {
        Drag::Source(i) => source_colour(i),
        _ => cx.face.accent,
    };
    let near = |at: Pos2| -> Option<String> {
        let centres = cx.centres.borrow();
        centres
            .iter()
            .filter(|(_, c)| c.distance(at) < kit::KNOB * 0.6)
            .min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at)))
            .map(|(d, _)| d.clone())
    };
    if let Some(at) = pointer {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        cable(&top_painter(ui, cx.track), from, at, colour, 0.0);
    }
    if ui.input(|i| i.pointer.any_released()) || !ui.input(|i| i.pointer.any_down()) {
        st.drag = None;
        if let Some(at) = pointer
            && let Some(dest) = near(at)
        {
            if (cx.patchable)(&dest) {
                drop_on(app, cx.track, cx.ins, drag, &dest);
                st.moved_at = ui.input(|i| i.time);
            } else {
                let what = cx.param_info(&dest).map_or(dest.clone(), |i| i.name.to_string());
                app.ui.status = format!("{what} cannot be modulated: it steps or sets how the synth plays");
            }
        }
    }
}

/// The sources (and their hint) as chips in `r`, left-aligned: dragging one starts a patch;
/// hovering one lights what it drives (the Front; on the Back each source's output sits on its
/// own module, [`kit::outputs_on`]). Returns the hovered source.
pub fn sources(ui: &mut Ui, cx: &Ctx, r: Rect, st: &mut State) -> Option<usize> {
    let face = cx.face;
    let p = ui.painter().clone();
    kit::section(&p, r, "Sources", "drag one onto a knob", face);
    let inner = Rect::from_min_max(r.min + vec2(8.0, 13.0), r.max - vec2(8.0, 4.0));
    let mut hovered = None;
    let widths: Vec<f32> = SOURCES.iter().map(|s| kit::chip_width(&p, s.name)).collect();
    let (line_h, gap) = (20.0, 5.0);
    let lines = if widths.iter().sum::<f32>() + gap * (widths.len() as f32 - 1.0) <= inner.width() { 1.0 } else { 2.0 };
    let top = inner.center().y - (lines * line_h + (lines - 1.0) * 6.0) * 0.5;
    let (mut x, mut y) = (inner.left(), top);
    for (i, s) in SOURCES.iter().enumerate() {
        let w = widths[i];
        if (lines > 1.0 && i == SOURCES.len().div_ceil(2)) || x + w > inner.right() + 0.5 {
            x = inner.left();
            y += line_h + 6.0;
        }
        let cr = Rect::from_min_size(pos2(x, y), vec2(w, line_h));
        let resp = kit::chip(ui, cr, (cx.track.0, "src", i), s.name, source_colour(i), face, cx.source_level(i));
        if resp.hovered() {
            hovered = Some(i);
        }
        if resp.drag_started() {
            st.drag = Some((Drag::Source(i), pos2(cr.left() + 11.0, cr.center().y)));
        }
        let _ = resp.on_hover_text(format!("{}: drag onto a knob to have it move that knob", s.name));
        x += w + gap;
    }
    hovered
}

/// The four macro knobs in section `r`: the knob sets the macro (an ordinary, automatable
/// parameter); its name under it is a handle: drag it onto a knob to give the macro that target,
/// double-click it to rename the macro. On the Back each macro is an output jack.
pub fn macros(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, r: Rect, st: &mut State) {
    let face = cx.face;
    let p = ui.painter().clone();
    kit::section(&p, r, "Macros", if cx.back { "outputs" } else { "" }, face);
    let cells = kit::section_cells(r, 4);
    let caps = (face.accent.lerp_to_gamma(Color32::WHITE, 0.15), face.accent.lerp_to_gamma(Color32::BLACK, 0.45), face.on_accent);
    let now = ui.input(|i| i.time);
    st.hover_macro = None;
    for (m, cell) in cells.iter().enumerate() {
        let name = cx.ins.modulation.macro_name(m);
        if cx.back {
            let c = pos2(cell.center().x, cell.top() + kit::KNOB * 0.5 + 1.0);
            kit::output_jack(&p, c, face.accent);
            kit::level_light(&p, c, 2.6, face.accent, cx.source_level(SOURCES.len() + m));
            let lr = Rect::from_min_size(pos2(cell.left() - 8.0, cell.top() + kit::KNOB + 2.0), vec2(cell.width() + 16.0, 12.0));
            let w = crate::editor_kit::look::tracked_width(&p, &name, 9.5, 0.02, true);
            p.rect_filled(Rect::from_center_size(lr.center(), vec2(w + 6.0, 12.0)), 2.0, kit::alpha(face.panel_at(0.5), 0.92));
            crate::editor_kit::look::tracked_text(&p, lr, Align2::CENTER_CENTER, &name, 9.5, 0.02, true, true, face.ink);
            cx.outputs.borrow_mut().push((SOURCES.len() + m, c));
            continue;
        }
        let param = sm::MACRO_PARAMS[m];
        let knob = kit::knob_ex(app, ui, cx, *cell, param, &name, Some(caps));
        // Turning a macro lights what it drives a moment; hovering it lights them as a source
        // chip does.
        if knob.response.dragged() || knob.response.changed() {
            st.macro_touch = Some((m, now));
        }
        if knob.response.hovered() {
            st.hover_macro = Some(m);
        }
        let label = Rect::from_min_size(pos2(cell.left() - 4.0, cell.top() + kit::KNOB + 2.0), vec2(cell.width() + 8.0, 12.0));
        let hid = Id::new(("synth-macro-name", cx.track.0, m));
        if let Some((rm, text)) = st.rename.as_mut().filter(|(rm, _)| *rm == m) {
            let mut t = text.clone();
            let edit = ui.put(label.expand2(vec2(10.0, 2.0)), egui::TextEdit::singleline(&mut t).id(hid.with("edit")).char_limit(24));
            edit.request_focus();
            let done = ui.input(|i| i.key_pressed(egui::Key::Enter)) || edit.lost_focus();
            let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
            *text = t;
            if cancel {
                st.rename = None;
            } else if done {
                let name = text.trim().to_string();
                let idx = *rm;
                st.rename = None;
                run(app, "mix.macro_name", json!({"track": cx.track.0, "macro": idx + 1, "name": name}));
            }
            continue;
        }
        let resp = ui.interact(label, hid, Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Macro {} handle", m + 1)));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
            p.rect_stroke(label, 3.0, Stroke::new(1.0, kit::alpha(face.accent, 0.6)), egui::StrokeKind::Inside);
            st.hover_macro = Some(m);
        }
        if resp.drag_started() {
            st.drag = Some((Drag::Macro(m), label.center()));
        }
        if resp.double_clicked() {
            st.rename = Some((m, name.clone()));
        }
        let n = cx.ins.modulation.macros.get(m).map_or(0, |x| x.targets.len());
        let _ = resp.on_hover_text(format!("{name}: {n} target(s). Drag onto a knob to add one; double-click to rename"));
    }
}

/// What patches a destination on the Back: its cables, each `(arc index, source position)`.
fn cables_into<'a>(cx: &'a Ctx, kinds: &'a [ArcKind], dest: &'a str) -> impl Iterator<Item = (usize, Option<Pos2>)> + 'a {
    cx.arcs.iter().enumerate().filter(move |(_, a)| a.dest == dest).map(move |(i, _)| (i, output_of(cx, kinds.get(i).copied())))
}

/// Where the output jack of what drew an arc is.
fn output_of(cx: &Ctx, kind: Option<ArcKind>) -> Option<Pos2> {
    let idx = match kind? {
        ArcKind::Slot(i) => sm::source_index(&cx.ins.modulation.slots.get(i)?.source)?,
        ArcKind::Macro(m, _) => SOURCES.len() + m,
    };
    cx.outputs.borrow().iter().find(|(k, _)| *k == idx).map(|(_, c)| *c)
}

/// The Back's cables and gestures, over the layout the panel drew with its jacks (the inputs in
/// `cx.centres`, the outputs in `cx.outputs`). The cables go into `slot` (drawn under every jack
/// and label), their plugs over the jacks. Drag from an output jack to an input to patch; drag a
/// cable's end off its jack to another to re-patch, or away from every jack to pull it out; hover
/// a jack for what is patched there, and turn the small amount knob that appears beside it.
pub fn back(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, area: Rect, st: &mut State, reduce_motion: bool, slot: egui::layers::ShapeIdx) {
    let p = ui.painter().clone();
    let (_, kinds) = arcs(cx.ins, cx.face);
    // The sockets on the panel; `@opN` marks where operator N's controls are when they are not
    // shown (Steeple shows one operator at a time): its cables end there.
    let all: Vec<(String, Pos2)> = cx.centres.borrow().clone();
    let inputs: Vec<(String, Pos2)> = all.iter().filter(|(d, _)| !d.starts_with('@')).cloned().collect();
    let mut ends: Vec<(String, Pos2)> = inputs.clone();
    for a in cx.arcs {
        if ends.iter().any(|(d, _)| *d == a.dest) {
            continue;
        }
        let op = a.dest.strip_prefix("op").and_then(|r| r.chars().next()).filter(char::is_ascii_digit);
        if let Some(n) = op
            && let Some((_, c)) = all.iter().find(|(d, _)| *d == format!("@op{n}"))
        {
            ends.push((a.dest.clone(), *c));
        }
    }
    let outputs: Vec<(usize, Pos2)> = cx.outputs.borrow().clone();
    let now = ui.input(|i| i.time);
    let sw = sway(now, st.moved_at, reduce_motion);
    if sw != 0.0 {
        ui.ctx().request_repaint();
    }
    let dragged = st.drag.map(|(d, _)| d);
    let drag_of = |k: ArcKind| match k {
        ArcKind::Slot(i) => Drag::Cable(i),
        ArcKind::Macro(m, t) => Drag::MacroCable(m, t),
    };
    // The cables, fanned a little where several go into one jack.
    let mut shapes = Vec::new();
    let mut plugs = Vec::new();
    for (dest, b) in &ends {
        let into: Vec<(usize, Option<Pos2>)> = cables_into(cx, &kinds, dest).collect();
        let n = into.len();
        for (k, (arc, from)) in into.into_iter().enumerate() {
            let (Some(a), Some(kind)) = (from, kinds.get(arc).copied()) else { continue };
            if Some(drag_of(kind)) == dragged {
                continue;
            }
            let fan = (k as f32 - (n as f32 - 1.0) * 0.5) * 5.0;
            let end = *b + vec2(fan, 0.0);
            let colour = cx.arcs.get(arc).map_or(Color32::WHITE, |x| x.colour);
            // A cable carries its source's level: a little brighter as it rises.
            let level = cx.arcs.get(arc).and_then(|x| cx.source_level(x.src)).unwrap_or(0.0);
            let colour = colour.lerp_to_gamma(Color32::WHITE, 0.28 * level);
            let phase = if arc % 2 == 0 { 1.0 } else { -0.7 };
            shapes.extend(cable_shapes(a, end, colour, sw * phase));
            plugs.push((a, colour));
            plugs.push((end, colour));
        }
    }
    p.set(slot, Shape::Vec(shapes));
    for (c, colour) in plugs {
        p.circle_filled(c, 4.5, colour.lerp_to_gamma(Color32::BLACK, 0.35));
        p.circle_filled(c, 2.8, colour);
    }
    // The sockets with cables glow with the level of what drives them: a ring round the nut.
    for (dest, c) in &ends {
        let level = cx
            .arcs
            .iter()
            .filter(|a| &a.dest == dest)
            .filter_map(|a| cx.source_level(a.src).map(|l| (l, a.colour)))
            .max_by(|x, y| x.0.total_cmp(&y.0));
        if let Some((l, colour)) = level.filter(|(l, _)| *l > 0.01) {
            p.circle_stroke(*c, 11.5, Stroke::new(1.0 + 1.5 * l, kit::alpha(colour, 0.15 + 0.55 * l)));
        }
    }
    // Gestures over the whole back.
    let resp = ui.interact(area, Id::new(("synth-back", cx.track.0)), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Patch cables"));
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let near_input =
        |at: Pos2| inputs.iter().filter(|(_, c)| c.distance(at) < 12.0).min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at))).cloned();
    let near_output = |at: Pos2| outputs.iter().find(|(_, c)| c.distance(at) < 12.0).copied();
    if resp.drag_started()
        && let Some(at) = ui.input(|i| i.pointer.press_origin())
    {
        if let Some((k, c)) = near_output(at) {
            let d = if k < SOURCES.len() { Drag::Source(k) } else { Drag::Macro(k - SOURCES.len()) };
            st.drag = Some((d, c));
        } else if let Some((dest, _)) = near_input(at)
            && let Some((arc, Some(from))) = cables_into(cx, &kinds, &dest).last()
            && let Some(kind) = kinds.get(arc).copied()
        {
            st.drag = Some((drag_of(kind), from));
        }
    }
    if let (Some((d, from)), Some(at)) = (st.drag, pointer) {
        let colour = match d {
            Drag::Source(i) => source_colour(i),
            Drag::Cable(i) => cx.ins.modulation.slots.get(i).and_then(|s| sm::source_index(&s.source)).map_or(Color32::WHITE, source_colour),
            _ => cx.face.accent,
        };
        cable(&top_painter(ui, cx.track), from, at, colour, 0.0);
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    if resp.drag_stopped()
        && let Some((d, _)) = st.drag.take()
    {
        let target = pointer.and_then(near_input).map(|(dest, _)| dest);
        let changed = match target {
            Some(dest) => {
                let check = match d {
                    Drag::Source(i) => sm::check_slot(&cx.ins.plugin, SOURCES[i].id, &dest).err(),
                    Drag::Cable(slot) => cx.ins.modulation.slots.get(slot).and_then(|s| sm::check_slot(&cx.ins.plugin, &s.source, &dest).err()),
                    _ => None,
                };
                match check {
                    Some(e) => {
                        app.ui.status = e;
                        false
                    }
                    None => drop_on(app, cx.track, cx.ins, d, &dest),
                }
            }
            None => pull_out(app, cx.track, cx.ins, d),
        };
        if changed {
            st.moved_at = now;
        }
    }
    // Hover: what is patched at a jack, and the amount knob beside a patched input.
    let hovered_input = pointer.and_then(near_input);
    if st.drag.is_none() {
        if let Some((dest, _)) = &hovered_input {
            let name = dest_name(&cx.ins.plugin, dest);
            let list: Vec<String> = cx.arcs.iter().filter(|a| &a.dest == dest).map(|a| format!("{} {:+.0}%", a.name, a.amount * 100.0)).collect();
            let text = if list.is_empty() {
                format!("{name}: drag a source's cable here")
            } else {
                format!("{name}: {} · drag a cable's end away to pull it out", list.join(", "))
            };
            let _ = resp.clone().on_hover_text_at_pointer(text);
            if !list.is_empty() {
                st.amount_at = Some(dest.clone());
            }
        } else if let Some((k, _)) = pointer.and_then(near_output) {
            let name = if k < SOURCES.len() { SOURCES[k].name.to_string() } else { cx.ins.modulation.macro_name(k - SOURCES.len()) };
            let _ = resp.clone().on_hover_text_at_pointer(format!("{name}: drag from here to a socket"));
        }
    }
    amount_knob(app, ui, cx, &kinds, &inputs, st, pointer);
}

/// The small knob beside a patched input on the Back that sets its last cable's amount (one undo
/// step per drag). It stays while the pointer is on the jack or the knob.
fn amount_knob(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, kinds: &[ArcKind], inputs: &[(String, Pos2)], st: &mut State, pointer: Option<Pos2>) {
    let Some(dest) = st.amount_at.clone() else { return };
    let Some(&(_, c)) = inputs.iter().find(|(d, _)| *d == dest).map(|x| x as &(String, Pos2)) else {
        st.amount_at = None;
        return;
    };
    let Some(arc) = cx.arcs.iter().rposition(|a| a.dest == dest) else {
        st.amount_at = None;
        return;
    };
    let kr = Rect::from_center_size(c + vec2(19.0, -12.0), vec2(16.0, 16.0));
    let id = Id::new(("synth-back-amount", cx.track.0));
    let busy = ui.ctx().is_being_dragged(id.with("dial"));
    let stays = busy || pointer.is_some_and(|q| q.distance(c) < 13.0 || kr.expand(4.0).contains(q));
    if !stays {
        st.amount_at = None;
        return;
    }
    let amount = cx.arcs[arc].amount;
    let opts = crate::editor_kit::controls::DialOpts { label: "Amount", default: 0.5, ..Default::default() };
    let out = crate::editor_kit::controls::dial_input(ui, kr, id.with("dial"), (amount + 1.0) * 0.5, &opts);
    if let Some(v) = out.value
        && let Some(k) = kinds.get(arc).copied()
    {
        set_arc_amount(app, cx.track, cx.ins, k, (v * 2.0 - 1.0).clamp(-1.0, 1.0), "back-amount");
    }
    if out.ended {
        app.engine.end_merge();
    }
    let p = ui.painter();
    let colour = cx.arcs[arc].colour;
    p.circle_filled(kr.center(), 8.5, Color32::from_rgb(0x1e, 0x1d, 0x1b));
    p.circle_stroke(kr.center(), 8.5, Stroke::new(1.0, kit::alpha(Color32::WHITE, 0.4)));
    let a0 = -std::f32::consts::FRAC_PI_2;
    let a1 = a0 + std::f32::consts::PI * 0.9 * amount;
    let n = 16;
    let arc_pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            kr.center() + 6.0 * vec2(a.cos(), a.sin())
        })
        .collect();
    p.add(Shape::line(arc_pts, Stroke::new(2.5, colour)));
    p.text(kr.center() + vec2(0.0, 13.0), Align2::CENTER_TOP, format!("{:+.0}%", amount * 100.0), crate::theme::mono(9.0), cx.face.ink);
    let _ = out.response.on_hover_text("Amount: drag up or down");
}

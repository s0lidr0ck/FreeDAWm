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
        });
        kinds.push(ArcKind::Slot(i));
    }
    for (m, mac) in ins.modulation.macros.iter().enumerate() {
        for (t, tg) in mac.targets.iter().enumerate() {
            let name = ins.modulation.macro_name(m);
            out.push(ModArc { dest: tg.dest.clone(), colour: face.accent, lo: tg.from.min(tg.to), hi: tg.from.max(tg.to), amount: tg.to, name });
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
    let sag = 50.0 + (b.y - a.y).abs() * 0.25 + (b.x - a.x).abs() * 0.08 + sway;
    let dx = (b.x - a.x) * 0.3;
    let c1 = a + vec2(dx, sag);
    let c2 = b + vec2(-dx, sag);
    let pts: Vec<Pos2> = (0..=40)
        .map(|i| {
            let t = i as f32 / 40.0;
            let u = 1.0 - t;
            let v = a.to_vec2() * (u * u * u) + c1.to_vec2() * (3.0 * u * u * t) + c2.to_vec2() * (3.0 * u * t * t) + b.to_vec2() * (t * t * t);
            v.to_pos2()
        })
        .collect();
    p.add(Shape::line(pts.iter().map(|q| *q + vec2(0.0, 3.0)).collect(), Stroke::new(7.0, kit::alpha(Color32::BLACK, 0.45))));
    p.add(Shape::line(pts.clone(), Stroke::new(5.0, colour)));
    p.add(Shape::line(pts.iter().map(|q| *q - vec2(0.0, 1.2)).collect(), Stroke::new(1.2, kit::alpha(Color32::WHITE, 0.25))));
    for e in [a, b] {
        p.circle_filled(e, 5.5, colour.lerp_to_gamma(Color32::BLACK, 0.3));
        p.circle_filled(e, 3.0, colour);
    }
}

/// How far the cables swing now, `t` seconds after they were moved (none with Reduce motion).
pub fn sway(now: f64, moved_at: f64, reduce_motion: bool) -> f32 {
    let t = (now - moved_at) as f32;
    if reduce_motion || !(0.0..1.6).contains(&t) {
        return 0.0;
    }
    16.0 * (std::f32::consts::TAU * 1.4 * t).sin() * (-t * 2.6).exp()
}

/// A jack on the Back: the nut, the socket ringed in `ring`, the hole.
fn jack(p: &Painter, c: Pos2, ring: Color32) {
    p.circle_filled(c, 12.0, Color32::from_rgb(0x8d, 0x8a, 0x84));
    p.circle_filled(c, 9.0, Color32::from_rgb(0x2a, 0x29, 0x26));
    p.circle_stroke(c, 9.0, Stroke::new(2.0, ring));
    p.circle_filled(c, 4.0, Color32::from_rgb(5, 5, 5));
}

/// The foreground painter for what is dragged (over every window).
fn top_painter(ui: &Ui, track: TrackId) -> Painter {
    ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::new(("synth-face-drag", track.0))))
}

/// The sources (and their hint) as chips in `r`: dragging one starts a patch; hovering one lights
/// what it drives. Returns the hovered source.
pub fn sources(ui: &mut Ui, cx: &Ctx, r: Rect, st: &mut State) -> Option<usize> {
    let face = cx.face;
    let p = ui.painter().clone();
    kit::section(&p, r, "Sources", "drag one onto a knob", face);
    let inner = Rect::from_min_max(r.min + vec2(12.0, 16.0), r.max - vec2(12.0, 8.0));
    let mut hovered = None;
    let widths: Vec<f32> = SOURCES.iter().map(|s| kit::chip_width(&p, s.name)).collect();
    let line_h = 28.0;
    let gap = 6.0;
    let total: f32 = widths.iter().sum::<f32>() + gap * (widths.len().saturating_sub(1)) as f32;
    // One line when they fit, centred in the frame.
    let (mut x, mut y) = if total <= inner.width() {
        (inner.left() + (inner.width() - total) * 0.5, inner.center().y - line_h * 0.5 - 4.0)
    } else {
        (inner.left(), inner.top() + 4.0)
    };
    for (i, s) in SOURCES.iter().enumerate() {
        let w = widths[i];
        if x + w > inner.right() {
            x = inner.left();
            y += line_h + 8.0;
        }
        let cr = Rect::from_min_size(pos2(x, y), vec2(w, line_h - 4.0));
        let resp = kit::chip(ui, cr, (cx.track.0, "src", i), s.name, source_colour(i), face);
        if resp.hovered() {
            hovered = Some(i);
        }
        if resp.drag_started() {
            st.drag = Some((Drag::Source(i), pos2(cr.left() + 13.0, cr.center().y)));
        }
        let _ = resp.on_hover_text(format!("{}: drag onto a knob to have it move that knob", s.name));
        x += w + gap;
    }
    hovered
}

/// The four macro knobs in section `r`: the knob sets the macro (an ordinary, automatable
/// parameter); its name under it is a handle: drag it onto a knob to give the macro that target,
/// double-click it to rename the macro.
pub fn macros(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, r: Rect, st: &mut State) {
    let face = cx.face;
    let p = ui.painter().clone();
    kit::section(&p, r, "Macros", "one knob, many targets", face);
    let cells = kit::section_cells(r, 4);
    let caps = (face.accent.lerp_to_gamma(Color32::WHITE, 0.15), face.accent.lerp_to_gamma(Color32::BLACK, 0.45), face.on_accent);
    for (m, cell) in cells.iter().enumerate() {
        let name = cx.ins.modulation.macro_name(m);
        let param = sm::MACRO_PARAMS[m];
        kit::knob_ex(app, ui, cx, *cell, param, &name, Some(caps));
        let label = Rect::from_min_size(pos2(cell.left() - 4.0, cell.top() + kit::KNOB + 1.0), vec2(cell.width() + 8.0, 14.0));
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

/// The Back of the panel in `r`: source jacks on the left, a jack for every destination on the
/// right, the patch as hanging cables between them. Drag from a source jack to a destination to
/// patch; drag a cable's end to another destination to re-patch, or away from every jack to pull
/// it out.
pub fn back(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, r: Rect, st: &mut State, reduce_motion: bool) {
    let p = ui.painter().clone();
    p.rect_filled(r, 5.0, Color32::from_rgb(0x1b, 0x1a, 0x18));
    let mut y = r.top() + 1.0;
    while y < r.bottom() {
        p.hline(r.left() + 1.0..=r.right() - 1.0, y, Stroke::new(2.0, Color32::from_rgb(0x1f, 0x1e, 0x1b)));
        y += 4.0;
    }
    kit::well(&p, r, &kit::Face { well: Color32::TRANSPARENT, ..*cx.face }, 5.0);
    let head = |x: f32, text: &str, align: Align2| {
        crate::editor_kit::look::tracked_text(
            &p,
            Rect::from_min_size(pos2(x, r.top() + 10.0), vec2(0.0, 14.0)),
            align,
            text,
            10.0,
            0.2,
            true,
            true,
            Color32::from_rgb(0x7d, 0x77, 0x6e),
        );
    };
    let label = Color32::from_rgb(0xb8, 0xb1, 0xa6);
    head(r.left() + 26.0, "Sources out", Align2::LEFT_CENTER);
    head(r.right() - 26.0, "Mod in", Align2::RIGHT_CENTER);
    // Source jacks: the seven sources, then the four macros.
    let n_src = SOURCES.len() + 4;
    let top = r.top() + 44.0;
    let step = ((r.height() - 64.0) / (n_src - 1) as f32).min(52.0);
    let src_at = |i: usize| pos2(r.left() + 38.0, top + step * i as f32);
    let mut jacks_src: Vec<(Drag, Pos2)> = Vec::new();
    // The jacks' names, printed over the cables on a dark plate so a cable never hides one.
    let mut labels: Vec<(Pos2, Align2, String)> = Vec::new();
    for (i, s) in SOURCES.iter().enumerate() {
        let c = src_at(i);
        jack(&p, c, source_colour(i));
        labels.push((c + vec2(20.0, 0.0), Align2::LEFT_CENTER, s.name.to_uppercase()));
        jacks_src.push((Drag::Source(i), c));
    }
    for m in 0..4 {
        let c = src_at(SOURCES.len() + m);
        jack(&p, c, cx.face.accent);
        labels.push((c + vec2(20.0, 0.0), Align2::LEFT_CENTER, cx.ins.modulation.macro_name(m).to_uppercase()));
        jacks_src.push((Drag::Macro(m), c));
    }
    // Destination jacks: two columns from the right.
    let ds = dests(&cx.ins.plugin);
    let per_col = ds.len().div_ceil(2).max(1);
    let dstep = ((r.height() - 64.0) / (per_col.max(2) - 1) as f32).min(52.0);
    let dest_at = |i: usize| {
        let col = i / per_col;
        pos2(r.right() - 38.0 - col as f32 * 230.0, top + dstep * (i % per_col) as f32)
    };
    for (i, d) in ds.iter().enumerate() {
        let c = dest_at(i);
        jack(&p, c, Color32::from_rgb(0x5c, 0x59, 0x52));
        labels.push((c - vec2(20.0, 0.0), Align2::RIGHT_CENTER, dest_name(&cx.ins.plugin, d).to_uppercase()));
    }
    let dest_pos = |d: &str| ds.iter().position(|x| *x == d).map(dest_at);
    let now = ui.input(|i| i.time);
    let sw = sway(now, st.moved_at, reduce_motion);
    if sw != 0.0 {
        ui.ctx().request_repaint();
    }
    // The cables, the one being dragged left out (it follows the pointer).
    let dragged = st.drag.map(|(d, _)| d);
    let mut cable_ends: Vec<(Drag, Pos2, Pos2, Color32)> = Vec::new();
    for (i, s) in cx.ins.modulation.slots.iter().enumerate() {
        let (Some(si), Some(b)) = (sm::source_index(&s.source), dest_pos(&s.dest)) else { continue };
        cable_ends.push((Drag::Cable(i), src_at(si), b, source_colour(si)));
    }
    for (m, mac) in cx.ins.modulation.macros.iter().enumerate().take(4) {
        for (t, tg) in mac.targets.iter().enumerate() {
            if let Some(b) = dest_pos(&tg.dest) {
                cable_ends.push((Drag::MacroCable(m, t), src_at(SOURCES.len() + m), b, cx.face.accent));
            }
        }
    }
    for (k, (d, a, b, c)) in cable_ends.iter().enumerate() {
        if Some(*d) != dragged {
            // Each cable swings a little out of step with the next.
            let phase = if k % 2 == 0 { 1.0 } else { -0.7 };
            cable(&p, *a, *b, *c, sw * phase);
        }
    }
    for (at, align, text) in labels {
        let g = p.layout_no_wrap(text, kit_font(), label);
        let r = align.anchor_size(at, g.size());
        p.rect_filled(r.expand2(vec2(4.0, 1.0)), 3.0, kit::alpha(Color32::from_rgb(0x1b, 0x1a, 0x18), 0.85));
        p.galley(r.min, g, label);
    }
    // Gestures: press on a source jack (a new cable) or on a destination jack holding a cable.
    let resp = ui.interact(r, Id::new(("synth-back", cx.track.0)), Sense::drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Patch cables"));
    let pointer = ui.input(|i| i.pointer.interact_pos());
    if resp.drag_started()
        && let Some(at) = ui.input(|i| i.pointer.press_origin())
    {
        if let Some((d, c)) = jacks_src.iter().find(|(_, c)| c.distance(at) < 14.0) {
            st.drag = Some((*d, *c));
        } else if let Some((d, a, _, _)) = cable_ends.iter().rev().find(|(_, _, b, _)| b.distance(at) < 14.0) {
            st.drag = Some((*d, *a));
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
        let target = pointer.and_then(|at| ds.iter().enumerate().find(|(i, _)| dest_at(*i).distance(at) < 16.0).map(|(_, x)| *x));
        let changed = match target {
            Some(dest) => {
                let check = match d {
                    Drag::Source(i) => sm::check_slot(&cx.ins.plugin, SOURCES[i].id, dest).err(),
                    Drag::Cable(slot) => cx.ins.modulation.slots.get(slot).and_then(|s| sm::check_slot(&cx.ins.plugin, &s.source, dest).err()),
                    _ => None,
                };
                match check {
                    Some(e) => {
                        app.ui.status = e;
                        false
                    }
                    None => drop_on(app, cx.track, cx.ins, d, dest),
                }
            }
            None => pull_out(app, cx.track, cx.ins, d),
        };
        if changed {
            st.moved_at = now;
        }
    }
}

fn kit_font() -> egui::FontId {
    crate::theme::bold(10.5)
}

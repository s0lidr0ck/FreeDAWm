//! FireStation CleanQ's ceiling in the module drawer: the shape over the graph, its nodes to drag
//! (Draw starts them on what is being learned), Learn from what plays, and Clear. After the C++ editor's Ceiling panel
//! (`CleanQGraph.h` overlay and node drag, `PluginEditor.cpp` capture and Auto Generate).
//!
//! Everything the user does goes through `mix.module_ceiling` (undoable; a drag is one step). The
//! capture for Learn is fed here, at most ~30 times a second, from the module's input stream (the
//! plugin's `pre` analysis ring): the editor's `feedCapture` into the translated averager. Done
//! sends the averaged, smoothed capture to the command, which lays the nodes on it.
//!
//! The overlay is on the analyser's scale, as the C++ draws it: -96..0 dBFS up the whole plot.

use crate::SoundApp;
use crate::module_graphs::{SPEC_INTERVAL, freq_to_x, x_to_freq};
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::analysis::Analysis;
use soundcraft_dsp::plugins::fs::cleanq::clean_q_capture::CeilingCapture;
use soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling::{self as cl, Ceiling};
use soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling_state::CeilingState;
use soundcraft_dsp::plugins::fs::cleanq::clean_q_dsp::FFT_SIZE;
use soundcraft_model::{Insert, ModuleId, TrackId};
use std::sync::Arc;

/// The ceiling's colour (`CleanQConfig.h` `warn`), deepened on the light theme to read on white.
pub fn ceiling_color() -> Color32 {
    if crate::theme::is_light() { Color32::from_rgb(0xB0, 0x78, 0x00) } else { Color32::from_rgb(0xFF, 0xD1, 0x66) }
}

/// `specDbToY`: -96..0 dBFS up the whole plot.
pub fn spec_y(rect: Rect, db: f64) -> f32 {
    let t = if db.is_finite() { ((db + 96.0) / 96.0).clamp(0.0, 1.0) } else { 0.0 };
    rect.bottom() - t as f32 * rect.height()
}

/// `yToSpecDb`.
pub fn y_spec(rect: Rect, y: f32) -> f64 {
    if !y.is_finite() {
        return -96.0;
    }
    f64::from(rect.bottom() - y) / f64::from(rect.height()).max(1.0) * 96.0 - 96.0
}

/// A Learn in progress: the capture and what it shows so far.
#[derive(Debug, Clone, Default)]
pub struct Learn {
    capture: CeilingCapture,
    /// Samples written to the input stream when last read.
    seen: usize,
    /// UI time the capture last took a frame (seconds).
    at: Option<f64>,
    buf: Vec<f32>,
    /// The capture on the ceiling grid (dB), as of the last frame.
    trace: Vec<f64>,
}

impl Learn {
    /// Takes a frame of `pre` (the module's input) when it has new samples, at most once per
    /// [`SPEC_INTERVAL`]; `tau` and `smoothing` are the state's `avgSeconds` and `smooth`.
    pub fn feed(&mut self, pre: &soundcraft_dsp::analysis::AnalysisRing, now: f64, sr: f64, tau: f64, smoothing: i32) {
        let due = self.at.is_none_or(|t| !now.is_finite() || now < t || now - t >= SPEC_INTERVAL - 1e-3);
        let w = pre.written();
        if !due || w == self.seen {
            return;
        }
        self.seen = w;
        self.at = Some(now);
        self.buf.resize(FFT_SIZE, 0.0);
        pre.latest(&mut self.buf);
        if self.capture.feed(&self.buf, now, tau) {
            self.trace = self.capture.grid(sr, smoothing);
        }
    }

    /// The capture itself (the wide editor's always-on capture reads and resets it).
    pub fn capture(&self) -> &CeilingCapture {
        &self.capture
    }

    /// Reset Capture: forget everything averaged.
    pub fn reset_capture(&mut self) {
        self.capture.reset();
        self.trace.clear();
    }

    /// Seconds averaged so far.
    pub fn seconds(&self) -> f64 {
        self.capture.averager().seconds_averaged()
    }

    /// The capture on the ceiling grid (empty: nothing yet).
    pub fn trace(&self) -> &[f64] {
        &self.trace
    }
}

/// The module's ceiling state, parsed once per change of its stored state (kept per module).
pub fn state_of(ctx: &egui::Context, track: TrackId, ins: &Insert) -> Arc<CeilingState> {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    ins.state.hash(&mut h);
    let hash = h.finish();
    let key = Id::new(("cq-ceiling-state", track.0, ins.id.0));
    if let Some((_, st)) = ctx.data(|d| d.get_temp::<(u64, Arc<CeilingState>)>(key)).filter(|(k, _)| *k == hash) {
        return st;
    }
    let st = Arc::new(ins.state_bytes().and_then(|b| CeilingState::read(&b)).unwrap_or_default());
    ctx.data_mut(|d| d.insert_temp(key, (hash, Arc::clone(&st))));
    st
}

/// The nodes as the command takes them.
fn points(c: &Ceiling) -> serde_json::Value {
    json!(c.pts.iter().map(|p| [p.hz, p.db]).collect::<Vec<_>>())
}

/// A node dragged from `from` (the shape when the drag began) by `delta` (pixels), as
/// `ceilingMouseDrag`: up and down move it and (unless `alone`) every node to its right by the
/// same amount; left and right move only its frequency, between its neighbours with a sixth of an
/// octave to spare (the end nodes stay at 20 Hz and 20 kHz); with `snap` the run into it lands on
/// a multiple of 1.5 dB per octave.
pub fn dragged(rect: Rect, from: &Ceiling, i: usize, delta: egui::Vec2, snap: bool, alone: bool) -> Ceiling {
    let mut c = from.clone();
    let n = from.pts.len();
    let Some(start) = from.pts.get(i).copied() else { return c };
    let mut hz = start.hz;
    if i > 0 && i + 1 < n {
        let gap = 2f64.powf(1.0 / 6.0);
        let lo = from.pts[i - 1].hz * gap;
        let hi = from.pts[i + 1].hz / gap;
        if lo < hi {
            hz = f64::from(x_to_freq(rect, freq_to_x(rect, hz as f32) + delta.x)).clamp(lo, hi);
        }
    }
    c.pts[i].hz = hz;
    let mut db = y_spec(rect, spec_y(rect, start.db) + delta.y).clamp(-96.0, 0.0);
    if snap && i > 0 {
        let reference = c.pts[i - 1];
        let oct = (hz / reference.hz).log2();
        if oct.abs() > 1.0e-3 {
            let slope = (((db - reference.db) / oct) / 1.5).round() * 1.5;
            db = (reference.db + slope * oct).clamp(-96.0, 0.0);
        }
    }
    cl::move_node(&mut c, i, db, !alone);
    c
}

/// Draws the ceiling (and a Learn's capture so far) over a CleanQ graph in `rect`; with `editing`
/// (the Ceiling tab), its nodes are handles to drag.
pub fn graph(app: &mut SoundApp, ui: &Ui, track: TrackId, ins: &Insert, rect: Rect, editing: bool) {
    let t = Tokens::current();
    let st = state_of(ui.ctx(), track, ins);
    let warn = ceiling_color();
    let p = ui.painter_at(rect);
    let c = &st.shape;

    // The capture so far, white (`trace`), while learning.
    if let Some(trace) = app.ui.cq_learn.get(&(track, ins.id)).map(Learn::trace).filter(|t| t.len() == cl::GRID_N) {
        let pts: Vec<Pos2> =
            (0..cl::GRID_N).filter(|&i| trace[i] > -110.0).map(|i| pos2(freq_to_x(rect, cl::grid_hz(i) as f32), spec_y(rect, trace[i]))).collect();
        p.add(Shape::line(pts, Stroke::new(1.6, t.text.gamma_multiply(0.8))));
    }

    if c.is_valid() {
        let (lo, hi) = (c.lo_hz(), c.hi_hz());
        let steps = (((freq_to_x(rect, hi as f32) - freq_to_x(rect, lo as f32)) / 3.0).round() as usize).max(2);
        let line: Vec<Pos2> = (0..=steps)
            .map(|k| {
                let hz = lo * (hi / lo).powf(k as f64 / steps as f64);
                pos2(freq_to_x(rect, hz as f32), spec_y(rect, c.at(hz)))
            })
            .collect();
        // The space above it faintly shaded: that is the "ceiling".
        for w in line.windows(2) {
            if let [a, b] = w {
                let shade = vec![*a, *b, pos2(b.x, rect.top()), pos2(a.x, rect.top())];
                p.add(Shape::convex_polygon(shade, warn.gamma_multiply(if editing { 0.07 } else { 0.04 }), Stroke::NONE));
            }
        }
        p.add(Shape::line(line, Stroke::new(if editing { 2.0 } else { 1.4 }, warn.gamma_multiply(if editing { 0.9 } else { 0.6 }))));
        if editing {
            // The slope of every run long enough to label, over its middle.
            for i in 0..c.pts.len().saturating_sub(1) {
                let (a, b) = (c.pts[i], c.pts[i + 1]);
                if freq_to_x(rect, b.hz as f32) - freq_to_x(rect, a.hz as f32) < 60.0 {
                    continue;
                }
                let mid = (a.hz * b.hz).sqrt();
                let s = c.slope(i);
                let at = pos2(freq_to_x(rect, mid as f32), spec_y(rect, c.at(mid)) - 4.0);
                // Not on a run lying along the top or the floor, where it would sit on the scale.
                if at.y < rect.top() + 14.0 || at.y > rect.bottom() - 16.0 {
                    continue;
                }
                p.text(at, Align2::CENTER_BOTTOM, format!("{}{s:.1} dB/oct", if s > 0.05 { "+" } else { "" }), regular(8.5), warn);
            }
        }
    }

    if editing {
        nodes(app, ui, track, ins, rect, &st, &p);
    } else if ui.data(|d| d.get_temp::<(usize, Ceiling, Pos2)>(drag_key(track, ins))).is_some() {
        // The tab changed mid-drag: that drag is over, and so is its undo step.
        ui.data_mut(|d| d.remove::<(usize, Ceiling, Pos2)>(drag_key(track, ins)));
        app.engine.end_merge();
    }
}

/// Where a node drag in progress is kept (per module, whichever drawer pass draws it).
fn drag_key(track: TrackId, ins: &Insert) -> Id {
    Id::new(("cq-ceiling-drag", track.0, ins.id.0))
}

/// The nodes as handles: dragged, each frame is one `mix.module_ceiling` merged into one undo step.
fn nodes(app: &mut SoundApp, ui: &Ui, track: TrackId, ins: &Insert, rect: Rect, st: &CeilingState, p: &Painter) {
    let t = Tokens::current();
    let warn = ceiling_color();
    let from_key = drag_key(track, ins);
    // The node dragged, the shape and the pointer when the drag began: the drag is worked out from
    // there, not from the last frame, so nodes that hit the top or bottom come back.
    let mut dragging: Option<(usize, Ceiling, Pos2)> = ui.data(|d| d.get_temp(from_key));
    let mut released = false;
    for (i, node) in st.shape.pts.iter().enumerate() {
        let at = pos2(freq_to_x(rect, node.hz as f32), spec_y(rect, node.db));
        let resp = ui.interact(Rect::from_center_size(at, vec2(16.0, 16.0)), ui.id().with(("cq-node", i)), Sense::drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Ceiling node {}", i + 1)));
        if resp.drag_started()
            && let Some(start) = resp.interact_pointer_pos()
        {
            // A new gesture, whatever an earlier one left open (a drag cut short by the drawer
            // closing): its own undo step.
            app.engine.end_merge();
            dragging = Some((i, st.shape.clone(), start));
        }
        if resp.dragged()
            && let Some((di, from, start)) = dragging.as_ref().filter(|(di, ..)| *di == i)
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let m = ui.input(|x| x.modifiers);
            let c = dragged(rect, from, *di, pos - *start, st.snap && !m.shift, m.alt);
            let key = format!("cq-ceiling:{}:{}", track.0, ins.id.0);
            if let Err(e) =
                app.engine.execute_merged("mix.module_ceiling", &json!({"track": track.0, "module": ins.id.0, "points": points(&c)}), &key)
            {
                app.ui.status = e.to_string();
            }
        }
        if resp.drag_stopped() {
            released = true;
        }
        let on = dragging.as_ref().is_some_and(|(di, ..)| *di == i) || resp.hovered();
        p.circle_filled(at, 4.5, t.graph_bg);
        p.circle_stroke(at, 4.0, Stroke::new(if on { 2.2 } else { 1.5 }, warn));
        if dragging.as_ref().is_some_and(|(di, ..)| *di == i) {
            let text = format!("{}  {:.1} dB", freq_text(node.hz), node.db);
            p.text(at + vec2(0.0, 9.0), Align2::CENTER_TOP, text, bold(8.5), warn);
        } else if resp.hovered() {
            let _ = resp.on_hover_text(format!(
                "{}  {:.1} dB — drag: up/down moves this node and the ones after it (Alt: this one only), left/right its frequency; Shift: no snap",
                freq_text(node.hz),
                node.db
            ));
        }
    }
    // Released, or stale: kept from a drag no node is in any more (the pointer is up).
    if released || (dragging.is_some() && !ui.input(|x| x.pointer.any_down())) {
        dragging = None;
        app.engine.end_merge();
    }
    ui.data_mut(|d| {
        if let Some(v) = dragging {
            d.insert_temp(from_key, v);
        } else {
            d.remove::<(usize, Ceiling, Pos2)>(from_key);
        }
    });
}

fn freq_text(hz: f64) -> String {
    if hz >= 1000.0 { format!("{:.2} kHz", hz / 1000.0) } else { format!("{hz:.0} Hz") }
}

/// The Ceiling tab's row: Learn (Done while learning), Draw, Clear, and what is going on (a Learn
/// is fed by [`feed`], on any tab). Returns the y under the row.
#[allow(clippy::too_many_arguments)]
pub fn controls(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, row: Rect, accent: Color32) -> f32 {
    let t = Tokens::current();
    let key = (track, ins.id);
    let st = state_of(ui.ctx(), track, ins);
    let learning = app.ui.cq_learn.get(&key).map(Learn::seconds);

    let p = ui.painter().clone();
    let (h, gap, w) = (20.0, 4.0, 48.0);
    let mut x = row.min.x;
    let mut button = |ui: &mut Ui, id: &str, label: &str, on: bool, tip: &str| {
        let r = Rect::from_min_size(pos2(x, row.min.y), vec2(w, h));
        x += w + gap;
        let resp = ui.interact(r, ui.id().with(("cq-ceiling-btn", id)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        crate::module_drawer::pill(&p, r, label, on, resp.hovered(), accent);
        resp.on_hover_text(tip).clicked()
    };

    let learn_label = if learning.is_some() { "Done" } else { "Learn" };
    let target = json!({"track": track.0, "module": ins.id.0});
    let learn_tip = if learning.is_some() {
        "Lay the ceiling on what was captured (16 nodes, where it turns)"
    } else {
        "Capture the input's average spectrum while it plays, then press Done to lay the ceiling on it"
    };
    if button(ui, "learn", learn_label, learning.is_some(), learn_tip) {
        let mut p = target.clone();
        p["action"] = json!(if learning.is_some() { "done" } else { "start" });
        if let Err(e) = app.run("ui.mix.ceiling_learn", p) {
            app.ui.status = e;
        }
    }
    let draw_tip = "Start the ceiling on what is being learned (16 nodes on the capture, as Done and Auto Generate lay them), \
                    then drag its nodes toward the shape you want; a noise guide is a slope to drag toward, not a line to put the ceiling on. \
                    Learn first: with nothing captured there is nothing to start from";
    if button(ui, "draw", "Draw", false, draw_tip) {
        // Draw starts from the capture (Auto Generate), never from a flat line or a guide: a
        // ceiling far from the source is what made Conform gut a vocal.
        let trace = app.ui.cq_learn.get(&key).map(|l| l.trace().to_vec()).unwrap_or_default();
        let c = if trace.len() == cl::GRID_N { cl::auto_ceiling(&trace) } else { Ceiling::default() };
        if !c.is_valid() {
            app.ui.status = "Nothing captured yet: press Learn and play something, then Draw starts the ceiling on it.".into();
        } else if let Err(e) = app.run("mix.module_ceiling", json!({"track": track.0, "module": ins.id.0, "points": points(&c)})) {
            app.ui.status = e;
        }
    }
    if button(ui, "clear", "Clear", false, "Remove the ceiling (and stop learning)") {
        let mut p = target.clone();
        p["action"] = json!("cancel");
        let _ = app.run("ui.mix.ceiling_learn", p);
        if st.shape.is_valid()
            && let Err(e) = app.run("mix.module_ceiling", json!({"track": track.0, "module": ins.id.0, "points": null}))
        {
            app.ui.status = e;
        }
    }

    let status = match learning {
        Some(s) if s > 0.0 => format!("Learning… {s:.1} s"),
        Some(_) => "Learning — play something".to_string(),
        None if st.shape.is_valid() => "Drag the nodes to draw".to_string(),
        None => "No ceiling yet".to_string(),
    };
    let r = Rect::from_x_y_ranges(x + 2.0..=row.max.x, row.min.y..=row.min.y + h);
    p.with_clip_rect(r).text(pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, status, regular(9.0), t.text_dim);
    row.min.y + h
}

/// `ui.mix.ceiling_learn {track, module, action: "start" | "done" | "cancel"}`: Learn on a CleanQ
/// (its drawer feeds the capture while it plays). `start` begins a fresh capture; `done` lays the
/// ceiling on it through `mix.module_ceiling {learn}` (undoable); `cancel` drops it. `track` is an
/// id or a name, `module` an id or a plugin name.
pub fn learn_command(app: &mut SoundApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let (track, module) = crate::menus::named_module(app, p)?;
    let ins = app.engine.session().track(track).and_then(|t| t.mixer.module(module)).cloned().ok_or("no such module")?;
    if ins.plugin != "fs_cleanq" {
        return Err(format!("`{}` has no ceiling to learn (a FireStation CleanQ does)", ins.plugin));
    }
    let key = (track, module);
    match p.get("action").and_then(serde_json::Value::as_str) {
        Some("start") => {
            app.ui.cq_learn.insert(key, Learn::default());
            Ok(json!({"learning": true}))
        }
        Some("cancel") => Ok(json!({"learning": false, "cancelled": app.ui.cq_learn.remove(&key).is_some()})),
        Some("done") => {
            let l = app.ui.cq_learn.remove(&key).ok_or("not learning")?;
            let st = ins.state_bytes().and_then(|b| CeilingState::read(&b)).unwrap_or_default();
            let grid = l.capture.grid(app.engine.session().sample_rate.as_f64(), st.smoothing);
            if grid.is_empty() {
                let m = "Nothing captured yet: play something first.";
                app.ui.status = m.into();
                return Err(m.into());
            }
            let seconds = l.seconds();
            let r = app.run("mix.module_ceiling", json!({"track": track.0, "module": module.0, "learn": grid}))?;
            Ok(json!({"learning": false, "seconds": seconds, "points": r["points"]}))
        }
        _ => Err("`action` must be \"start\", \"done\" or \"cancel\"".into()),
    }
}

/// Feeds a Learn in progress on this CleanQ from `analysis` (the module's streams), whatever tab
/// its drawer shows: the C++ capture runs whenever the editor is open (`refreshAnalyser` ->
/// `feedCapture`). Called by every open CleanQ drawer each frame.
pub fn feed(app: &mut SoundApp, ui: &Ui, track: TrackId, ins: &Insert, analysis: Option<&Arc<Analysis>>, sr: f32) {
    let Some(l) = app.ui.cq_learn.get_mut(&(track, ins.id)) else { return };
    let Some(a) = analysis else { return };
    let st = state_of(ui.ctx(), track, ins);
    let sr = if sr.is_finite() && sr > 0.0 { f64::from(sr) } else { 48_000.0 };
    l.feed(&a.pre, ui.input(|i| i.time), sr, st.avg_seconds, st.smoothing);
    ui.ctx().request_repaint();
}

/// Forgets Learns of modules no longer shown in a drawer.
pub fn prune(learn: &mut std::collections::HashMap<(TrackId, ModuleId), Learn>, open: &std::collections::HashMap<TrackId, ModuleId>) {
    learn.retain(|(t, m), _| open.get(t) == Some(m));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_scale_round_trips() {
        let r = Rect::from_min_size(pos2(10.0, 20.0), vec2(300.0, 96.0));
        assert_eq!(spec_y(r, 0.0), r.top());
        assert_eq!(spec_y(r, -96.0), r.bottom());
        assert_eq!(spec_y(r, f64::NAN), r.bottom());
        assert!((y_spec(r, spec_y(r, -40.0)) + 40.0).abs() < 1e-4);
    }

    #[test]
    fn dragging_a_node_moves_it_and_the_rest_and_snaps() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 192.0));
        let flat = cl::flat_ceiling(-48.0);
        // 2 px per dB: 10 px down is -5 dB, carried to every node after it.
        let c = dragged(r, &flat, 5, vec2(0.0, 10.0), false, false);
        assert!((c.pts[5].db + 53.0).abs() < 1e-3);
        assert!((c.pts[15].db + 53.0).abs() < 1e-3);
        assert_eq!(c.pts[4].db, -48.0);
        // Alt: only this node.
        let c = dragged(r, &flat, 5, vec2(0.0, 10.0), false, true);
        assert_eq!(c.pts[6].db, -48.0);
        // Snap: the run into node 5 lands on a multiple of 1.5 dB/oct.
        let c = dragged(r, &flat, 5, vec2(0.0, 7.0), true, false);
        let s = c.slope(4);
        assert!((s / 1.5 - (s / 1.5).round()).abs() < 1e-9, "{s}");
        // Sideways: between the neighbours with a sixth of an octave to spare; the ends stay.
        let c = dragged(r, &flat, 5, vec2(-1000.0, 0.0), false, false);
        assert!((c.pts[5].hz - flat.pts[4].hz * 2f64.powf(1.0 / 6.0)).abs() < 1e-6);
        let c = dragged(r, &flat, 0, vec2(300.0, -1000.0), false, false);
        assert_eq!(c.pts[0].hz, 20.0);
        assert!(c.pts.iter().all(|p| p.db == 0.0), "held inside the plot");
        // A node that is not there changes nothing.
        assert_eq!(dragged(r, &flat, 99, vec2(5.0, 5.0), true, false), flat);
    }
}

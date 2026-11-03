//! Swale in the strip: its graph (the C++ `SwaleDisplay`: one cycle of the envelope as the
//! settings describe it, beside about five seconds of what the engine did) and the drawer's
//! per-mode rules (`SwaleConfig.h`'s `whatApplies`).
//!
//! The history is the engine's own `envOut` and `detOut` readouts, sampled UI-side at the C++
//! editor's 30 Hz into a fixed ring (like the compressor's GR history, it is never invented), and
//! a trigger mark wherever the engine's `trigSeq` readout moved between two samples.

use crate::module_drawer::Switch;
use crate::module_graphs::LiveValues;
use crate::theme::{Tokens, bold, module_accent, regular};
use egui::{Align2, Color32, Id, Pos2, Rect, Shape, Stroke, pos2};
use soundcraft_dsp::plugins::fs::swale::{SWALE_PARAMS, swale_config as sc, swale_dsp as sd};
use soundcraft_model::Insert;
use std::sync::Arc;

/// Points kept: about five seconds at the editor's 30 Hz poll (`historySpan`).
pub const HISTORY_LEN: usize = 150;
/// Seconds between two points.
pub const HISTORY_TICK: f64 = 1.0 / 30.0;
/// The detector scale's floor, dB (`DY`: -80..0).
const DET_FLOOR_DB: f32 = -80.0;

/// The span the history shows, seconds (`historySpan` points at the poll rate: about five).
pub const HISTORY_SPAN: f64 = HISTORY_LEN as f64 * HISTORY_TICK;

/// The envelope history of one Swale module: a fixed ring of timed readings, filled in place (no
/// allocation once made). One point per reading taken, at most one per [`HISTORY_TICK`]: a late
/// frame leaves a gap the drawing bridges, never copies of one reading.
#[derive(Debug, Clone)]
pub struct SwaleHistory {
    time: [f64; HISTORY_LEN],
    env: [f32; HISTORY_LEN],
    det_db: [f32; HISTORY_LEN],
    trig: [bool; HISTORY_LEN],
    /// The slot the next point goes into.
    head: usize,
    len: usize,
    /// When the next point may be taken (UI clock, seconds).
    next: f64,
    /// The trigger count last seen, and a swell seen since the last point.
    last_trig: Option<f32>,
    pending: bool,
    /// The UI pass this was last brought up to date in, plus one (0: never).
    pass: u64,
}

impl Default for SwaleHistory {
    fn default() -> Self {
        SwaleHistory {
            time: [0.0; HISTORY_LEN],
            env: [0.0; HISTORY_LEN],
            det_db: [DET_FLOOR_DB; HISTORY_LEN],
            trig: [false; HISTORY_LEN],
            head: 0,
            len: 0,
            next: 0.0,
            last_trig: None,
            pending: false,
            pass: 0,
        }
    }
}

impl SwaleHistory {
    /// Takes the engine's readouts at UI time `now` (seconds) as one point, when one is due (the
    /// editor's 30 Hz poll). No reading (the module did not run, or a nonsense value) adds
    /// nothing; after a pause longer than the span, or a clock that went back, the ring starts
    /// again.
    pub fn feed(&mut self, now: f64, live: Option<&LiveValues>) {
        let Some(r) = live.map(|l| l.readouts) else { return };
        let (env, det, trig) = (r[2], r[3], r[4]);
        if !env.is_finite() || !now.is_finite() {
            return;
        }
        if trig.is_finite() {
            if self.last_trig.is_some_and(|t| t != trig) {
                self.pending = true;
            }
            self.last_trig = Some(trig);
        }
        if let Some(last) = self.newest_time()
            && (now - last > HISTORY_SPAN || now < last)
        {
            // A long pause (the strip was hidden) or a new clock: start again.
            self.len = 0;
        }
        if self.len > 0 && now < self.next {
            return;
        }
        // The editor's conversion (`timerCallback`): the detector in dB, -100 below 1e-5.
        let det_db = if det.is_finite() && det > 1.0e-5 { 20.0 * det.log10() } else { -100.0 };
        let first = self.len == 0;
        self.time[self.head] = now;
        self.env[self.head] = env.clamp(0.0, 1.0);
        self.det_db[self.head] = det_db;
        self.trig[self.head] = std::mem::take(&mut self.pending);
        self.head = (self.head + 1) % HISTORY_LEN;
        self.len = (self.len + 1).min(HISTORY_LEN);
        // The next poll on the 30 Hz grid; a late frame does not catch up with copies.
        self.next = if first { now } else { self.next } + HISTORY_TICK;
        if self.next <= now {
            self.next = now + HISTORY_TICK;
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn slot(&self, i: usize) -> Option<usize> {
        (i < self.len).then(|| (self.head + HISTORY_LEN - self.len + i) % HISTORY_LEN)
    }

    /// Point `i`, oldest first: (envelope 0..1, detector dB, a swell started here).
    pub fn point(&self, i: usize) -> Option<(f32, f32, bool)> {
        self.slot(i).map(|at| (self.env[at], self.det_db[at], self.trig[at]))
    }

    /// When point `i` was taken (UI clock, seconds).
    pub fn time(&self, i: usize) -> Option<f64> {
        self.slot(i).map(|at| self.time[at])
    }

    /// When the newest point was taken.
    pub fn newest_time(&self) -> Option<f64> {
        self.len.checked_sub(1).and_then(|i| self.time(i))
    }

    /// Whether a swell started within `secs` before the newest point (the lamp).
    pub fn triggered_within(&self, secs: f64) -> bool {
        let Some(last) = self.newest_time() else { return false };
        (0..self.len).rev().take_while(|i| self.time(*i).is_some_and(|t| last - t <= secs)).any(|i| self.point(i).is_some_and(|p| p.2))
    }
}

/// The area under a polyline down to `floor`, as one mesh (a strip of quads).
fn fill_under(p: &egui::Painter, pts: &[Pos2], floor: f32, color: Color32) {
    if pts.len() < 2 {
        return;
    }
    let mut mesh = egui::Mesh::default();
    mesh.reserve_vertices(pts.len() * 2);
    mesh.reserve_triangles((pts.len() - 1) * 2);
    for (i, q) in pts.iter().enumerate() {
        mesh.colored_vertex(*q, color);
        mesh.colored_vertex(pos2(q.x, floor), color);
        if i > 0 {
            let k = (i * 2) as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// The history of the module `key` names, brought up to date at most once per UI pass, so its
/// tile and its drawer share it. Kept in the context as one `Arc`, updated in place while nothing
/// else holds it (last frame's copies are gone), so a frame allocates nothing for it.
pub fn history(ctx: &egui::Context, key: Id, live: Option<&LiveValues>) -> Arc<SwaleHistory> {
    let key = key.with("swale-history");
    let pass = ctx.cumulative_pass_nr() + 1;
    let cur: Option<Arc<SwaleHistory>> = ctx.data(|d| d.get_temp(key));
    if let Some(h) = cur.as_ref().filter(|h| h.pass == pass) {
        return Arc::clone(h);
    }
    ctx.data_mut(|d| d.remove_temp::<Arc<SwaleHistory>>(key));
    let mut h = cur.unwrap_or_default();
    let now = ctx.input(|i| i.time);
    let m = Arc::make_mut(&mut h);
    m.feed(now, live);
    m.pass = pass;
    // (The app repaints at the meter rate while audio runs: nothing to ask for here.)
    ctx.data_mut(|d| d.insert_temp(key, Arc::clone(&h)));
    h
}

/// A parameter of `ins` (its default when unset), clamped as the engine receives it.
fn value(ins: &Insert, index: usize) -> f32 {
    SWALE_PARAMS.get(index).map_or(0.0, |p| p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default)))
}

fn index(ins: &Insert, i: usize) -> i32 {
    value(ins, i).round() as i32
}

fn on(ins: &Insert, i: usize) -> bool {
    value(ins, i) > 0.5
}

/// Whether the engine runs on its own BPM knob (Tempo Source = Manual).
pub fn manual_tempo(ins: &Insert) -> bool {
    index(ins, sd::P_TEMPO_SRC) == 1
}

/// `whatApplies` for the module's current settings. The session always gives the host a tempo.
pub fn applies(ins: &Insert) -> sc::Applicable {
    sc::what_applies(index(ins, sd::P_MODE), on(ins, sd::P_ATK_SYNC), on(ins, sd::P_HOLD_SYNC), on(ins, sd::P_REL_SYNC), manual_tempo(ins), true)
}

/// The drawer's rules for a Swale control (`refreshLayout`): Sync swaps a stage's time knob for its
/// note, each mode shows only what it uses, Pick calls Hold "Hang", the editor's own short names
/// (Retrig, Low Cut). The BPM knob and the Tempo Source are hidden while Tempo Source is Host:
/// the session tempo drives sync in the DAW (controller ruling, as for Mod and Filter). Set to
/// Manual by any path (a preset, a session, automation), both show, so it can be put back.
pub fn switch(ins: &Insert, id: &str) -> Option<Switch> {
    let a = applies(ins);
    let used = |on: bool, label: Option<&'static str>, opts: &'static [&'static str]| match (on, label) {
        (false, _) => Some(Switch::Hide),
        (true, Some(label)) => Some(Switch::Show { label, opts }),
        (true, None) => None,
    };
    let hang = (a.hold_label != "Hold").then_some(a.hold_label);
    match id {
        "attack" => used(a.attack_time, None, &[]),
        "atkNote" => used(a.attack_note, Some("Attack"), &sc::NOTES),
        "hold" => used(a.hold_time, hang, &[]),
        "holdNote" => used(a.hold_note, Some(a.hold_label), &sc::HOLD_NOTES),
        "release" => used(a.release_time, None, &[]),
        "relNote" => used(a.release_note, Some("Release"), &sc::NOTES),
        "sens" => used(a.sens, None, &[]),
        "rate" => used(a.rate, None, &[]),
        "peakBeat" => used(a.peak_beat, None, &[]),
        "phrase" => used(a.phrase, None, &[]),
        "retrig" => Some(Switch::Show { label: "Retrig", opts: &[] }),
        "detHp" => Some(Switch::Show { label: "Low Cut", opts: &[] }),
        // Hidden while it is Host (the session tempo drives sync); a Manual tempo that arrives
        // from a preset, a session or automation stays visible, BPM with it, so it can be undone.
        "tempoSrc" | "bpm" if manual_tempo(ins) => None,
        "tempoSrc" | "bpm" => Some(Switch::Hide),
        _ => None,
    }
}

/// Rhythmic has no trigger, so the two detector knobs are dead: the editor dims them in place
/// (`setLocked`).
pub fn locked(ins: &Insert, id: &str) -> bool {
    matches!(id, "retrig" | "detHp") && applies(ins).dim_detector
}

/// The tempo the engine is using (readout `[5]`, `bpmOut`), else the BPM knob's (what the engine
/// runs on with no host tempo).
pub fn bpm_in_use(ins: &Insert, live: Option<&LiveValues>) -> f32 {
    match live.map(|l| l.readouts[5]) {
        Some(b) if b.is_finite() && b >= 20.0 => b,
        _ => sd::knob_to_bpm(value(ins, sd::P_BPM)),
    }
}

/// `stageTimes` for the module's current settings at `bpm`.
pub fn stage_times(ins: &Insert, bpm: f32) -> sc::StageTimes {
    sc::stage_times(
        index(ins, sd::P_MODE),
        bpm,
        on(ins, sd::P_ATK_SYNC),
        value(ins, sd::P_ATTACK),
        index(ins, sd::P_ATK_NOTE),
        on(ins, sd::P_HOLD_SYNC),
        value(ins, sd::P_HOLD),
        index(ins, sd::P_HOLD_NOTE),
        on(ins, sd::P_REL_SYNC),
        value(ins, sd::P_RELEASE),
        index(ins, sd::P_REL_NOTE),
        index(ins, sd::P_RATE),
    )
}

/// One traversal of the envelope at `t` seconds from the start of the shape (`gainAt`).
fn gain_at(curve: i32, tm: &sc::StageTimes, t: f64, sustain: f64) -> f64 {
    let (a, h, r) = (tm.attack, tm.hold, tm.release);
    if t < a {
        return sd::shape(curve, t / a.max(1.0e-6));
    }
    if t < a + sustain + h {
        return 1.0;
    }
    if t < a + sustain + h + r {
        return sd::shape(curve, 1.0 - (t - a - sustain - h) / r.max(1.0e-6));
    }
    0.0
}

/// The resting gain: Floor if it is dialled in, and the Slow Gear FET never quite shuts.
fn resting_gain(ins: &Insert) -> f32 {
    let k = value(ins, sd::P_FLOOR);
    let f = if k < 0.005 { 0.0 } else { 10f32.powf(sd::knob_to_floor_db(k) / 20.0) };
    if index(ins, sd::P_VCA) == 1 { f.max(0.006) } else { f }
}

fn dashed(p: &egui::Painter, a: Pos2, b: Pos2, dash: f32, gap: f32, stroke: Stroke) {
    p.extend(Shape::dashed_line(&[a, b], stroke, dash, gap));
}

/// The left panel (`paintShape`): one cycle of the envelope as the settings describe it, the beat
/// mid-panel in Rhythmic, the pick at the start otherwise.
fn draw_shape(p: &egui::Painter, area: Rect, ins: &Insert, tm: &sc::StageTimes, accent: Color32, large: bool) {
    let t = Tokens::current();
    let mode = index(ins, sd::P_MODE);
    let curve = index(ins, sd::P_CURVE);
    let floor = f64::from(resting_gain(ins));
    let (top, bot) = (area.top(), area.bottom());
    let y_of = |g: f64| bot - ((floor + (1.0 - floor) * g) as f32) * (bot - top);
    p.line_segment([pos2(area.left(), bot), pos2(area.right(), bot)], Stroke::new(1.0, t.graph_grid));
    p.line_segment([pos2(area.left(), top), pos2(area.right(), top)], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.5)));

    // Pick holds open while the note rings, so the picture shows some of that.
    let sustain = if mode == 0 { (0.25 * (tm.attack + tm.hold + tm.release)).max(0.05) } else { 0.0 };
    let total = if mode == 2 { tm.cycle } else { tm.attack + sustain + tm.hold + tm.release };
    if !(total > 0.0 && total.is_finite()) || area.width() < 4.0 {
        return;
    }
    let x_of = |s: f64| area.left() + (s / total) as f32 * area.width();
    let steps = (area.width() as usize).clamp(24, 200);
    let pts: Vec<Pos2> = if mode == 2 {
        // One cycle starting half a cycle before the beat, so the beat sits mid-panel.
        let offset = if on(ins, sd::P_PEAK_BEAT) { tm.attack } else { 0.0 };
        let shift = ((offset - 0.5 * tm.cycle) % tm.cycle + tm.cycle) % tm.cycle;
        (0..=steps)
            .map(|i| {
                let s = i as f64 / steps as f64 * tm.cycle;
                pos2(x_of(s), y_of(gain_at(curve, tm, (s + shift) % tm.cycle, 0.0)))
            })
            .collect()
    } else {
        (0..=steps)
            .map(|i| {
                let s = i as f64 / steps as f64 * total;
                pos2(x_of(s), y_of(gain_at(curve, tm, s, sustain)))
            })
            .collect()
    };
    fill_under(p, &pts, bot, accent.gamma_multiply(0.11));
    p.add(Shape::line(pts, Stroke::new(if large { 1.8 } else { 1.3 }, accent)));
    let f = regular(8.0);
    if mode == 2 {
        let xb = x_of(0.5 * tm.cycle);
        dashed(p, pos2(xb, top - if large { 4.0 } else { 0.0 }), pos2(xb, bot), 3.0, 3.0, Stroke::new(1.0, t.text));
        if large {
            p.text(pos2(xb + 3.0, top - 2.0), Align2::LEFT_BOTTOM, "beat", f, t.text_dim);
        }
    } else if large {
        p.text(pos2(area.left(), top - 2.0), Align2::LEFT_BOTTOM, "pick", f.clone(), t.text_dim);
        if mode == 0 {
            p.text(pos2(x_of(tm.attack) + 3.0, top - 2.0), Align2::LEFT_BOTTOM, "note rings", f, t.text_dim.gamma_multiply(0.75));
        }
    }
}

/// The right panel (`paintHistory`): the detector behind (not in Rhythmic) against its trigger
/// and re-arm lines, the VCA opening in front, and a mark at every swell the engine started.
fn draw_history(p: &egui::Painter, area: Rect, ins: &Insert, h: &SwaleHistory, accent: Color32, large: bool) {
    let t = Tokens::current();
    let mode = index(ins, sd::P_MODE);
    let (top, bot) = (area.top(), area.bottom());
    p.line_segment([pos2(area.left(), bot), pos2(area.right(), bot)], Stroke::new(1.0, t.graph_grid));
    p.line_segment([pos2(area.left(), top), pos2(area.right(), top)], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.5)));
    let det_y = |db: f32| bot - ((db - DET_FLOOR_DB) / -DET_FLOOR_DB).clamp(0.0, 1.0) * (bot - top);
    if mode != 2 {
        let th = sd::knob_to_sens_db(value(ins, sd::P_SENS));
        let re = th + sd::knob_to_rearm_db(value(ins, sd::P_RETRIG));
        let line = t.text_dim.gamma_multiply(0.7);
        dashed(p, pos2(area.left(), det_y(th)), pos2(area.right(), det_y(th)), 4.0, 4.0, Stroke::new(1.0, line));
        dashed(p, pos2(area.left(), det_y(re)), pos2(area.right(), det_y(re)), 1.0, 4.0, Stroke::new(1.0, line));
        if large {
            let f = regular(8.0);
            p.text(pos2(area.left() + 2.0, det_y(th) - 2.0), Align2::LEFT_BOTTOM, "trigger", f.clone(), t.text_dim);
            let lets_go = mode == 0 && on(ins, sd::P_PHRASE);
            p.text(pos2(area.left() + 2.0, det_y(re) - 2.0), Align2::LEFT_BOTTOM, if lets_go { "lets go" } else { "re-arm" }, f, t.text_dim);
        }
    }
    let n = h.len();
    if n < 2 {
        return;
    }
    let Some(last) = h.newest_time() else { return };
    // Placed by when each reading was taken: the newest at the right, the span across.
    let x_of = |i: usize| area.right() - ((last - h.time(i).unwrap_or(last)) / HISTORY_SPAN) as f32 * area.width();
    let shown = (0..n).filter(|i| h.time(*i).is_some_and(|t| last - t <= HISTORY_SPAN));
    // Swells the engine started: a mark from the floor to the top.
    for i in shown.clone() {
        if h.point(i).is_some_and(|q| q.2) {
            let x = x_of(i);
            p.line_segment([pos2(x, top), pos2(x, bot)], Stroke::new(1.0, accent.gamma_multiply(0.45)));
            p.circle_filled(pos2(x, top + 1.5), if large { 2.0 } else { 1.5 }, accent);
        }
    }
    if mode != 2 {
        let det: Vec<Pos2> = shown.clone().filter_map(|i| h.point(i).map(|q| pos2(x_of(i), det_y(q.1)))).collect();
        p.add(Shape::line(det, Stroke::new(1.0, t.text_dim.gamma_multiply(0.6))));
    }
    let env: Vec<Pos2> = shown.filter_map(|i| h.point(i).map(|q| pos2(x_of(i), bot - q.0 * (bot - top)))).collect();
    fill_under(p, &env, bot, accent.gamma_multiply(0.11));
    p.add(Shape::line(env, Stroke::new(if large { 1.8 } else { 1.3 }, accent)));
}

/// Swale's graph: the shape beside the history (the history alone on a slim tile); the drawer
/// adds the mode, the stage lengths, the scales' words and the swell lamp.
pub fn draw(ui: &egui::Ui, p: &egui::Painter, rect: Rect, ins: &Insert, module: Id, live: Option<&LiveValues>, large: bool) {
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let h = history(ui.ctx(), module, live);
    let bpm = bpm_in_use(ins, live);
    let tm = stage_times(ins, bpm);
    let pad = if large { 6.0 } else { 3.0 };
    let head = if large { 24.0 } else { 0.0 };
    let foot = if large { 4.0 } else { 0.0 };
    let body = Rect::from_min_max(pos2(rect.left() + pad, rect.top() + pad + head), pos2(rect.right() - pad, rect.bottom() - pad - foot));
    if !(body.width() >= 4.0 && body.height() >= 4.0) {
        return;
    }
    let with_shape = large || rect.height() >= 40.0;
    let hist = if with_shape {
        let split = body.left() + body.width() * 0.4;
        let shape = Rect::from_min_max(body.min, pos2(split - pad, body.bottom()));
        draw_shape(p, shape, ins, &tm, accent, large);
        p.line_segment([pos2(split, body.top()), pos2(split, body.bottom())], Stroke::new(1.0, t.graph_grid));
        Rect::from_min_max(pos2(split + pad, body.top()), body.max)
    } else {
        body
    };
    draw_history(p, hist, ins, &h, accent, large);

    if large {
        let mode = index(ins, sd::P_MODE);
        let y = rect.top() + pad;
        p.text(pos2(rect.left() + pad, y), Align2::LEFT_TOP, sc::MODES.get(mode as usize).copied().unwrap_or(""), bold(9.0), t.text);
        let dot = "  \u{b7}  ";
        let mut text = format!(
            "A {}{dot}{}{}{dot}R {}",
            sc::format_seconds(tm.attack),
            if mode == 0 { "hang " } else { "H " },
            sc::format_seconds(tm.hold),
            sc::format_seconds(tm.release)
        );
        if mode == 2 {
            text += &format!("{dot}every {}{}", sc::format_seconds(tm.cycle), if tm.fitted { " (fitted)" } else { "" });
        }
        p.text(pos2(rect.right() - pad - 14.0, y), Align2::RIGHT_TOP, text, regular(8.5), accent);
        // The lamp: one flash per swell (about 100 ms, as the editor's).
        let lamp = pos2(rect.right() - pad - 4.0, y + 5.0);
        if h.triggered_within(0.1) {
            p.circle_filled(lamp, 6.0, accent.gamma_multiply(0.25));
            p.circle_filled(lamp, 3.5, accent);
        } else {
            p.circle_filled(lamp, 3.5, t.graph_grid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(env: f32, det: f32, trig: f32) -> LiveValues {
        let mut r = [f32::NAN; 8];
        r[2] = env;
        r[3] = det;
        r[4] = trig;
        r[5] = 120.0;
        LiveValues { readouts: r, ..LiveValues::default() }
    }

    #[test]
    fn history_keeps_five_seconds_of_readouts_and_marks_swells() {
        let mut h = SwaleHistory::default();
        h.feed(10.0, None);
        assert!(h.is_empty(), "no meter: nothing invented");
        h.feed(10.0, Some(&live(f32::NAN, 0.1, 0.0)));
        assert!(h.is_empty(), "no reading: nothing invented");
        h.feed(10.0, Some(&live(0.25, 0.1, 4.0)));
        assert_eq!(h.len(), 1);
        assert_eq!(h.point(0), Some((0.25, -20.0, false)), "the first count is not a swell");
        // A frame inside the poll interval takes nothing.
        h.feed(10.01, Some(&live(0.3, 0.1, 4.0)));
        assert_eq!(h.len(), 1);
        // A late frame (a tenth of a second on) takes one point, not copies; the count moved: it
        // is marked, and the point sits at its own time.
        h.feed(10.11, Some(&live(0.5, 1e-7, 5.0)));
        assert_eq!(h.len(), 2);
        assert_eq!(h.point(1), Some((0.5, -100.0, true)));
        assert_eq!(h.time(1), Some(10.11));
        assert!(h.triggered_within(0.1));
        // The same count again: no new mark.
        h.feed(10.21, Some(&live(0.75, 0.01, 5.0)));
        assert_eq!(h.point(2).map(|p| p.2), Some(false));
        // Bounded: ten seconds of 60 Hz frames keep the last 150 points, one per poll, newest last.
        for f in 0..600 {
            h.feed(10.25 + f as f64 / 60.0, Some(&live(f as f32 / 600.0, 0.01, 5.0)));
        }
        assert_eq!(h.len(), HISTORY_LEN);
        let span = h.newest_time().unwrap() - h.time(0).unwrap();
        assert!((span - (HISTORY_LEN - 1) as f64 / 30.0).abs() < 0.02, "one point per 1/30 s: {span}");
        let last = h.point(HISTORY_LEN - 1).unwrap().0;
        assert!(last >= 597.0 / 600.0, "the newest reading is last: {last}");
        for i in 1..HISTORY_LEN {
            assert_ne!(h.point(i - 1).unwrap().0, h.point(i).unwrap().0, "no duplicate fills");
        }
        assert!(h.point(HISTORY_LEN).is_none());
        // A long pause starts again; hostile readings are clamped or skipped.
        h.feed(100.0, Some(&live(9.0, f32::INFINITY, f32::NAN)));
        assert_eq!(h.len(), 1);
        assert_eq!(h.point(0), Some((1.0, -100.0, false)));
        h.feed(f64::NAN, Some(&live(0.5, 0.5, 1.0)));
        assert_eq!(h.len(), 1);
        // A clock that went back starts again too.
        h.feed(50.0, Some(&live(0.5, 0.5, 1.0)));
        assert_eq!((h.len(), h.time(0)), (1, Some(50.0)));
    }

    #[test]
    fn history_is_filled_in_place() {
        // The ring is fixed arrays: feeding never moves or grows it.
        let mut h = Arc::new(SwaleHistory::default());
        let at = Arc::as_ptr(&h);
        for f in 0..1000 {
            let m = Arc::make_mut(&mut h);
            m.feed(f as f64 / 60.0, Some(&live(0.5, 0.1, (f / 30) as f32)));
        }
        assert_eq!(at, Arc::as_ptr(&h));
        assert_eq!(h.len(), HISTORY_LEN);
    }

    #[test]
    fn switches_follow_the_mode_and_the_syncs() {
        let mut ins = Insert::new("fs_swale");
        // Pick (the default): Hold is Hang, no Rate, the phrase switch, the trigger knobs.
        assert_eq!(switch(&ins, "hold"), Some(Switch::Show { label: "Hang", opts: &[] }));
        assert_eq!(switch(&ins, "attack"), None);
        assert_eq!(switch(&ins, "atkNote"), Some(Switch::Hide));
        assert_eq!(switch(&ins, "rate"), Some(Switch::Hide));
        assert_eq!(switch(&ins, "peakBeat"), Some(Switch::Hide));
        assert_eq!(switch(&ins, "phrase"), None);
        assert_eq!(switch(&ins, "sens"), None);
        assert_eq!(switch(&ins, "bpm"), Some(Switch::Hide));
        assert_eq!(switch(&ins, "tempoSrc"), Some(Switch::Hide));
        // A Manual tempo source that arrived from elsewhere is shown, with its BPM, so it can be
        // put back to Host.
        ins.params.insert("tempoSrc".into(), 1.0);
        assert_eq!(switch(&ins, "tempoSrc"), None);
        assert_eq!(switch(&ins, "bpm"), None);
        assert!(
            crate::module_drawer::grid_params(soundcraft_dsp::plugin_info("fs_swale").unwrap(), &ins, Some("attack"), Some("Swell"))
                .0
                .iter()
                .any(|p| p.id == "tempoSrc")
        );
        ins.params.insert("tempoSrc".into(), 0.0);
        assert_eq!(switch(&ins, "tempoSrc"), Some(Switch::Hide));
        assert_eq!(switch(&ins, "detHp"), Some(Switch::Show { label: "Low Cut", opts: &[] }));
        assert!(!locked(&ins, "retrig"));
        // Synced Attack: the note in its place, named Attack.
        ins.params.insert("atkSync".into(), 1.0);
        assert_eq!(switch(&ins, "attack"), Some(Switch::Hide));
        assert_eq!(switch(&ins, "atkNote"), Some(Switch::Show { label: "Attack", opts: &sc::NOTES }));
        ins.params.insert("holdSync".into(), 1.0);
        assert_eq!(switch(&ins, "holdNote"), Some(Switch::Show { label: "Hang", opts: &sc::HOLD_NOTES }));
        // One-Shot: Hold, no phrase.
        ins.params.insert("mode".into(), 1.0);
        assert_eq!(switch(&ins, "holdNote"), Some(Switch::Show { label: "Hold", opts: &sc::HOLD_NOTES }));
        ins.params.insert("holdSync".into(), 0.0);
        assert_eq!(switch(&ins, "hold"), None);
        assert_eq!(switch(&ins, "phrase"), Some(Switch::Hide));
        // Rhythmic: Rate and Peak on Beat, no Sens, the detector knobs dimmed.
        ins.params.insert("mode".into(), 2.0);
        assert_eq!(switch(&ins, "rate"), None);
        assert_eq!(switch(&ins, "peakBeat"), None);
        assert_eq!(switch(&ins, "sens"), Some(Switch::Hide));
        assert!(locked(&ins, "retrig") && locked(&ins, "detHp") && !locked(&ins, "mix"));
        // Hostile stored values never panic.
        for bad in [f32::NAN, f32::INFINITY, -9.0, 1e9] {
            for k in ["mode", "atkSync", "holdSync", "relSync", "rate", "curve"] {
                ins.params.insert(k.into(), bad);
            }
            for p in SWALE_PARAMS.iter() {
                let _ = switch(&ins, p.id);
            }
            let tm = stage_times(&ins, bpm_in_use(&ins, Some(&live(0.0, 0.0, 0.0))));
            assert!(tm.attack.is_finite() && tm.cycle.is_finite());
        }
    }

    #[test]
    fn stage_times_use_the_engines_tempo() {
        let mut ins = Insert::new("fs_swale");
        ins.params.insert("mode".into(), 2.0);
        ins.params.insert("rate".into(), 9.0);
        // The engine's tempo when it reports one, else the BPM knob's 120.
        assert_eq!(bpm_in_use(&ins, None), 120.0);
        let mut l = live(0.0, 0.0, 0.0);
        l.readouts[5] = 90.0;
        assert_eq!(bpm_in_use(&ins, Some(&l)), 90.0);
        let tm = stage_times(&ins, 90.0);
        assert!((tm.cycle - 60.0 / 90.0).abs() < 1e-12);
    }

    #[test]
    fn strip_hooks_station_knobs_labels_accent_and_drawer() {
        use crate::key_knobs::{default_key_knobs, tile_knob_applies, tile_knob_label};
        use crate::module_drawer::{drawer_groups, hero_applies, model_switch, param_locked};
        use crate::module_graphs::{GraphKind, graph_kind};
        let info = soundcraft_dsp::plugin_info("fs_swale").unwrap();
        assert_eq!(graph_kind("fs_swale"), GraphKind::Swale);
        assert_eq!(default_key_knobs(info), ["attack", "hold", "release", "mix"]);
        assert_eq!(crate::theme::station_accent("fs_swale"), Some(Color32::from_rgb(0xF2, 0xB8, 0x4B)));
        assert_eq!(drawer_groups(info), ["Swell", "More"]);
        let mut ins = Insert::new("fs_swale");
        assert_eq!(crate::chain_view::model_name(&ins, info).as_deref(), Some("Pick"));
        let labels = |ins: &Insert| -> Vec<String> { default_key_knobs(info).iter().map(|k| tile_knob_label(ins, info.param(k).unwrap())).collect() };
        assert_eq!(labels(&ins), ["Attack", "Hang", "Release", "Mix"]);
        for mode in 0..3 {
            for sync in 0..2 {
                ins.params.insert("mode".into(), mode as f32);
                for k in ["atkSync", "holdSync", "relSync"] {
                    ins.params.insert(k.into(), sync as f32);
                }
                let l = labels(&ins);
                for (i, a) in l.iter().enumerate() {
                    assert!(!l[i + 1..].contains(a), "mode {mode}: {l:?}");
                }
                // Synced stages hand over to their notes: the tile's time knobs dim, Mix never.
                for k in ["attack", "hold", "release"] {
                    assert_eq!(tile_knob_applies(&ins, k), sync == 0, "mode {mode}, {k}");
                }
                assert!(tile_knob_applies(&ins, "mix"));
                assert_eq!(hero_applies(&ins), sync == 0);
                assert_eq!(model_switch(&ins, "bpm"), Some(Switch::Hide));
                assert_eq!(param_locked(&ins, "detHp"), mode == 2);
            }
        }
        assert_eq!(labels(&ins)[1], "Hold", "Rhythmic calls it Hold");
    }

    #[test]
    fn history_follows_a_real_engine() {
        // The engine's own readouts, read once per 1/30 s of audio: the trace is its envOut and
        // there is one mark per swell it counted, no more.
        let mut p = soundcraft_dsp::create("fs_swale").unwrap();
        p.prepare(48_000.0, 1600, 2);
        p.set_param("mode", 2.0);
        p.set_param("rate", 9.0); // 1/4 at the BPM knob's 120: a swell every 0.5 s
        let mut h = SwaleHistory::default();
        let mut io = vec![vec![0.5f32; 1600], vec![0.5f32; 1600]];
        let mut first = None;
        let mut r = [f32::NAN; 8];
        for blk in 0..150 {
            p.process(&mut io, 1600);
            r = [f32::NAN; 8];
            p.readouts(&mut r);
            first.get_or_insert(r[4]);
            h.feed(blk as f64 / 30.0 + if blk > 0 { 1e-6 } else { 0.0 }, Some(&LiveValues { readouts: r, ..LiveValues::default() }));
            assert_eq!(h.point(h.len() - 1).map(|q| q.0), Some(r[2].clamp(0.0, 1.0)), "block {blk} len {} next {}", h.len(), h.next);
        }
        assert_eq!(h.len(), HISTORY_LEN);
        let marks = (0..h.len()).filter(|i| h.point(*i).unwrap().2).count();
        let counted = (r[4] - first.unwrap()) as usize;
        assert!(counted >= 8, "{counted}");
        assert_eq!(marks, counted, "one mark per swell the engine started");
    }
}

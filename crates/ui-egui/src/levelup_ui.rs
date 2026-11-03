//! LevelUp in the strip: its graph (the C++ editor's Level scope, 30 s of short-term loudness
//! against the target and the gate, with the big short-term readout and the gain correction bar,
//! plus what the Reference and Deliver tabs show: the reference's gain, integrated loudness, true
//! peak and the selected platform's gain), the pairing status line, and Learn's write-back.
//!
//! Everything shown is the engine's own readouts (`fs_levelup`'s `readouts`), sampled UI-side at
//! the editor's 10 Hz scope rate into a fixed ring (never invented); nothing is computed per
//! frame beyond drawing.

use crate::SoundApp;
use crate::module_graphs::LiveValues;
use crate::module_graphs::module_key;
use crate::theme::{Tokens, bold, module_accent, regular};
use egui::{Align2, Color32, Id, Pos2, Rect, Shape, Stroke, pos2};
use serde_json::json;
use soundcraft_dsp::plugins::fs::levelup::{LEVELUP_PARAMS, level_up_dsp as d};
use soundcraft_model::{Insert, ModuleId, Session, Track, TrackId};
use std::sync::Arc;

/// Points kept: thirty seconds at the editor's one point per 100 ms (`history.assign (300)`).
pub const HISTORY_LEN: usize = 300;
/// Seconds between two points.
pub const HISTORY_TICK: f64 = 0.1;
/// The span the scope shows, seconds.
pub const HISTORY_SPAN: f64 = HISTORY_LEN as f64 * HISTORY_TICK;
/// The scope's scale, LUFS (`lo`, `hi`).
const LO: f32 = -70.0;
const HI: f32 = 0.0;

/// The loudness history of one LevelUp: short-term in (the editor's scope) and out, a fixed ring
/// filled in place (no allocation once made).
#[derive(Debug, Clone)]
pub struct LevelUpHistory {
    time: [f64; HISTORY_LEN],
    lufs_in: [f32; HISTORY_LEN],
    lufs_out: [f32; HISTORY_LEN],
    head: usize,
    len: usize,
    next: f64,
    /// The UI pass this was last brought up to date in, plus one (0: never).
    pass: u64,
}

impl Default for LevelUpHistory {
    fn default() -> Self {
        LevelUpHistory {
            time: [0.0; HISTORY_LEN],
            lufs_in: [-100.0; HISTORY_LEN],
            lufs_out: [-100.0; HISTORY_LEN],
            head: 0,
            len: 0,
            next: 0.0,
            pass: 0,
        }
    }
}

impl LevelUpHistory {
    /// Takes the readouts at UI time `now` as one point when one is due. No reading adds nothing;
    /// after a pause longer than the span, or a clock that went back, the ring starts again.
    pub fn feed(&mut self, now: f64, live: Option<&LiveValues>) {
        let Some(r) = live.map(|l| l.readouts) else { return };
        let (i, o) = (r[0], r[2]);
        if !(i.is_finite() && o.is_finite() && now.is_finite()) {
            return;
        }
        if let Some(last) = self.newest_time()
            && (now - last > HISTORY_SPAN || now < last)
        {
            self.len = 0;
        }
        if self.len > 0 && now < self.next {
            return;
        }
        let first = self.len == 0;
        self.time[self.head] = now;
        self.lufs_in[self.head] = i;
        self.lufs_out[self.head] = o;
        self.head = (self.head + 1) % HISTORY_LEN;
        self.len = (self.len + 1).min(HISTORY_LEN);
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

    /// Point `i`, oldest first: (time, short-term in, short-term out).
    pub fn point(&self, i: usize) -> Option<(f64, f32, f32)> {
        self.slot(i).map(|at| (self.time[at], self.lufs_in[at], self.lufs_out[at]))
    }

    pub fn newest_time(&self) -> Option<f64> {
        self.len.checked_sub(1).and_then(|i| self.point(i)).map(|p| p.0)
    }
}

/// The history of the module `key` names, brought up to date once per UI pass (shared by its
/// tile and its drawer), updated in place inside one `Arc`.
pub fn history(ctx: &egui::Context, key: Id, live: Option<&LiveValues>) -> Arc<LevelUpHistory> {
    let key = key.with("levelup-history");
    let pass = ctx.cumulative_pass_nr() + 1;
    let cur: Option<Arc<LevelUpHistory>> = ctx.data(|d| d.get_temp(key));
    if let Some(h) = cur.as_ref().filter(|h| h.pass == pass) {
        return Arc::clone(h);
    }
    ctx.data_mut(|d| d.remove_temp::<Arc<LevelUpHistory>>(key));
    let mut h = cur.unwrap_or_default();
    let now = ctx.input(|i| i.time);
    let m = Arc::make_mut(&mut h);
    m.feed(now, live);
    m.pass = pass;
    ctx.data_mut(|d| d.insert_temp(key, Arc::clone(&h)));
    h
}

/// A parameter of `ins` (its default when unset), clamped as the engine receives it.
fn value(ins: &Insert, index: usize) -> f32 {
    LEVELUP_PARAMS.get(index).map_or(0.0, |p| p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default)))
}

fn index(ins: &Insert, i: usize) -> i32 {
    value(ins, i).round() as i32
}

/// `lufsText`: "--" before a reading.
pub fn lufs_text(v: f32) -> String {
    if !v.is_finite() || v <= -69.0 { "--".into() } else { format!("{v:.1}") }
}

/// `signedDb`.
pub fn signed_db(v: f32) -> String {
    format!("{}{v:.1}", if v >= 0.0 { "+" } else { "" })
}

/// A readout, `None` when it has no reading.
fn reading(live: Option<&LiveValues>, i: usize) -> Option<f32> {
    live.map(|l| l.readouts[i]).filter(|v| v.is_finite())
}

/// The target the leveler aims at: the followed IN's loudness on a linked OUT, else the knob's.
pub fn target_lufs(ins: &Insert, live: Option<&LiveValues>) -> f32 {
    reading(live, 7).unwrap_or_else(|| d::knob_to_target(value(ins, d::P_TARGET)))
}

/// What the host's link group says about a module's pairing (worked out from the session once
/// per document revision by [`learn_watch`], read by the graph).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PairInfo {
    /// The module is in a link group.
    pub linked: bool,
    /// Why the pairing cannot work in its group (the C++'s "Pairing unavailable"), if so.
    pub unavailable: Option<&'static str>,
    /// The pairs (0..3) that have an IN in the group, other than this module.
    pub in_pairs: Vec<i32>,
}

/// [`PairInfo`] for module `ins` on track `track` of `s`.
pub fn pair_info(s: &Session, track: TrackId, ins: &Insert) -> PairInfo {
    let Some(g) = ins.link_group else { return PairInfo::default() };
    let members: Vec<(TrackId, &Insert)> =
        s.tracks.iter().flat_map(|t| t.mixer.inserts.iter().map(move |i| (t.id, i))).filter(|(_, i)| i.link_group == Some(g)).collect();
    let unavailable = if members.iter().any(|(_, i)| i.plugin != "fs_levelup") {
        Some("the link group holds other plugins")
    } else if members.len() > soundcraft_dsp::plugins::fs::levelup::level_up_link::MAX_INSTANCES {
        Some("more than 32 modules in the link group")
    } else {
        None
    };
    let mut in_pairs: Vec<i32> = members
        .iter()
        .filter(|(t, i)| !(*t == track && i.id == ins.id) && i.plugin == "fs_levelup" && index(i, d::P_LINK) == d::LINK_IN)
        .map(|(_, i)| index(i, d::P_LINK_GROUP).clamp(0, 3))
        .collect();
    in_pairs.sort_unstable();
    in_pairs.dedup();
    PairInfo { linked: true, unavailable, in_pairs }
}

fn pair_name(i: i32) -> &'static str {
    soundcraft_dsp::plugins::fs::levelup::PAIRS.get(i.clamp(0, 3) as usize).copied().unwrap_or("A")
}

/// The status line (the editor's "what this instance is doing in its pair"), and whether it is a
/// warning. `pair`: the host group's view ([`pair_info`]); `None` before it is known.
pub fn status(ins: &Insert, live: Option<&LiveValues>, pair: Option<&PairInfo>) -> (String, bool) {
    let role = index(ins, d::P_LINK);
    let mine = index(ins, d::P_LINK_GROUP).clamp(0, 3);
    let name = pair_name(mine);
    if role == d::LINK_OFF {
        return ("Leveling on its own".into(), false);
    }
    let what = if role == d::LINK_IN { "IN" } else { "OUT" };
    if let Some(p) = pair {
        if !p.linked {
            return (format!("{what} of pair {name} · not in a link group: pick one under LINK"), true);
        }
        if let Some(why) = p.unavailable {
            return (format!("Pairing unavailable: {why}"), true);
        }
    }
    if role == d::LINK_IN {
        return (format!("IN of pair {name}"), false);
    }
    match reading(live, 7) {
        Some(l) => (format!("OUT of pair {name} · matching its IN at {} LUFS", lufs_text(l)), false),
        None => match pair.map(|p| p.in_pairs.as_slice()).unwrap_or(&[]) {
            others if !others.is_empty() && !others.contains(&mine) => {
                let list: Vec<&str> = others.iter().map(|i| pair_name(*i)).collect();
                (format!("No IN in pair {name} (the group's IN: pair {})", list.join(", ")), true)
            }
            _ => (format!("OUT of pair {name} · no IN found yet; using the Target knob"), true),
        },
    }
}

fn dashed(p: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke) {
    p.extend(Shape::dashed_line(&[a, b], stroke, 4.0, 4.0));
}

/// The area under a polyline down to `floor`, as one mesh.
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

/// LevelUp's graph. See the module docs.
pub fn draw(ui: &egui::Ui, p: &egui::Painter, rect: Rect, ins: &Insert, module: Id, live: Option<&LiveValues>, large: bool) {
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let h = history(ui.ctx(), module, live);
    let pad = if large { 8.0 } else { 3.0 };
    let head = if large { 18.0 } else { 0.0 };
    let foot = if large { 22.0 } else { 5.0 };
    let scope = Rect::from_min_max(pos2(rect.left() + pad, rect.top() + pad + head), pos2(rect.right() - pad, rect.bottom() - pad - foot));
    if !(scope.width() >= 4.0 && scope.height() >= 4.0) {
        return;
    }
    let y_of = |v: f32| scope.bottom() - (v.clamp(LO, HI) - LO) / (HI - LO) * scope.height();

    // grid, every 10 LU; a label the target line would run through is left out
    let target = target_lufs(ins, live);
    for k in 0..=7 {
        let v = LO + 10.0 * k as f32;
        let y = y_of(v);
        p.line_segment([pos2(scope.left(), y), pos2(scope.right(), y)], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.6)));
        if large && k % 2 == 1 && k < 7 && (y - y_of(target)).abs() > 11.0 {
            p.text(pos2(scope.left() + 2.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{v:.0}"), regular(8.0), t.text_dim);
        }
    }
    // the gate (shaded below target - gate) and the target (dashed)
    let gate = d::knob_to_gate(value(ins, d::P_GATE));
    let gy = y_of(target - gate);
    p.rect_filled(Rect::from_min_max(pos2(scope.left(), gy), scope.max), 0.0, t.text_dim.gamma_multiply(0.08));
    dashed(p, pos2(scope.left(), y_of(target)), pos2(scope.right(), y_of(target)), Stroke::new(1.0, accent.gamma_multiply(0.7)));

    // the history: out thin and dim, in (the editor's scope) bright and filled
    if let Some(last) = h.newest_time() {
        let x_of = |tm: f64| scope.right() - ((last - tm) / HISTORY_SPAN) as f32 * scope.width();
        let shown = (0..h.len()).filter_map(|i| h.point(i)).filter(|q| last - q.0 <= HISTORY_SPAN);
        let out: Vec<Pos2> = shown.clone().map(|q| pos2(x_of(q.0), y_of(q.2))).collect();
        let inp: Vec<Pos2> = shown.map(|q| pos2(x_of(q.0), y_of(q.1))).collect();
        fill_under(p, &inp, scope.bottom(), accent.gamma_multiply(0.10));
        p.add(Shape::line(out, Stroke::new(1.0, t.text_dim.gamma_multiply(0.8))));
        p.add(Shape::line(inp, Stroke::new(if large { 1.8 } else { 1.3 }, accent)));
    }

    // the big readout, top right: short-term in, on a plate the scope's lines stop at
    let st_in = reading(live, 0).unwrap_or(-100.0);
    let big = if large { 20.0 } else { 11.0 };
    let tr = pos2(scope.right() - 3.0, scope.top() + 1.0);
    let num = p.layout_no_wrap(lufs_text(st_in), bold(big), t.text);
    let cap = large.then(|| p.layout_no_wrap("LUFS SHORT-TERM".into(), regular(7.5), t.text_dim));
    let w = num.size().x.max(cap.as_ref().map_or(0.0, |c| c.size().x));
    let h = num.size().y + cap.as_ref().map_or(0.0, |c| c.size().y + 1.0);
    let plate = Rect::from_min_max(pos2(tr.x - w - 4.0, tr.y - 1.0), pos2(tr.x + 2.0, tr.y + h + 1.0));
    p.rect_filled(plate, 3.0, t.graph_bg.gamma_multiply(0.85));
    let num_w = num.size().x;
    p.galley(pos2(tr.x - num_w, tr.y), num, t.text);
    if let Some(c) = cap {
        let cw = c.size().x;
        p.galley(pos2(tr.x - cw, tr.y + big + 2.0), c, t.text_dim);
    }

    // the gain correction bar along the foot, centred, scaled by Max Gain
    let delta = reading(live, 1).unwrap_or(0.0);
    let mg = d::knob_to_max_gain(value(ins, d::P_MAX_GAIN)).max(1.0);
    let bar_h = if large { 5.0 } else { 3.0 };
    let bar_y = rect.bottom() - pad - if large { 7.0 } else { 3.0 };
    let bar_l = if large { scope.left() + 66.0 } else { scope.left() };
    let bar = Rect::from_min_max(pos2(bar_l, bar_y - bar_h / 2.0), pos2(scope.right(), bar_y + bar_h / 2.0));
    p.rect_filled(bar, 2.0, t.graph_grid);
    let cx = bar.center().x;
    p.line_segment([pos2(cx, bar.top() - 1.0), pos2(cx, bar.bottom() + 1.0)], Stroke::new(1.0, t.text_dim));
    let pct = (delta / mg).clamp(-1.0, 1.0) * 0.5 * bar.width();
    let fill = if pct < 0.0 {
        Rect::from_min_max(pos2(cx + pct, bar.top()), pos2(cx, bar.bottom()))
    } else {
        Rect::from_min_max(pos2(cx, bar.top()), pos2(cx + pct, bar.bottom()))
    };
    p.rect_filled(fill, 2.0, accent);
    if large {
        p.text(pos2(scope.left(), bar_y), Align2::LEFT_CENTER, format!("{} dB", signed_db(delta)), bold(11.0), accent);
    }

    if large {
        // The delivery and reference readings, right-aligned on the header line; the status line
        // on the left, clipped short of them.
        let y = rect.top() + pad;
        let integ = reading(live, 3).unwrap_or(-100.0);
        let tp = reading(live, 4).unwrap_or(-120.0);
        let tp_text = if tp <= -119.0 { "TP --".to_string() } else { format!("TP {tp:.1}") };
        let mut parts: Vec<(String, egui::FontId, Color32)> = vec![(format!("INT {}", lufs_text(integ)), regular(8.5), t.text)];
        // a true peak over -1 dBTP is where MP3/AAC encoding can clip
        parts.push((tp_text, regular(8.5), if tp > -1.0 { t.meter_red } else { t.text }));
        if let Some(g) = reading(live, 5) {
            let slot = index(ins, d::P_REF_SLOT).clamp(0, 3) + 1;
            parts.push((format!("REF {slot} {} dB", signed_db(g)), bold(8.5), accent));
        } else {
            let plat = index(ins, d::P_PLATFORM);
            if plat > 0 {
                let g = d::platform_gain_db(plat, integ, tp);
                let on = value(ins, d::P_PREVIEW) > 0.5;
                parts.push((
                    format!("{} {} dB{}", d::platform(plat).name, signed_db(g), if on { " (preview)" } else { "" }),
                    regular(8.5),
                    t.text_dim,
                ));
            }
        }
        let mut x = rect.right() - pad;
        for (text, font, color) in parts.into_iter().rev() {
            let r = p.text(pos2(x, y), Align2::RIGHT_TOP, text, font, color);
            x = r.left() - 8.0;
        }
        let pair: Option<PairInfo> = ui.ctx().data(|d| d.get_temp(module.with("levelup-pair")));
        let (s, warn) = status(ins, live, pair.as_ref());
        let clip = Rect::from_min_max(pos2(rect.left() + pad, rect.top()), pos2(x - 2.0, scope.top()));
        p.with_clip_rect(clip).text(pos2(rect.left() + pad, y), Align2::LEFT_TOP, s, regular(8.5), if warn { t.meter_yellow } else { accent });
    }
}

/// The Learn readout last seen per LevelUp module, `(track, module)` to its bits.
pub type LearnSeen = std::collections::HashMap<(u64, u32), u32>;

/// Every LevelUp whose engine has just learned a Target the session does not hold (readout `[6]`,
/// held until the host sets Target): `(track, module, knob value)`. Edge-triggered: only a readout
/// that CHANGED since `seen` (brought up to date here) is acted on, so a Learn value still held
/// after the user turned Target since (the meters a frame behind) never reverts that edit. Reads
/// the strip meters, so it works whether or not the module is shown.
pub fn learn_followups(tracks: &[Track], meters: &soundcraft_playback::MeterSnapshot, seen: &mut LearnSeen) -> Vec<(TrackId, ModuleId, f32)> {
    let mut out = Vec::new();
    for t in tracks {
        let Some(strip) = meters.tracks.get(&t.id) else { continue };
        for ins in t.mixer.inserts.iter().filter(|i| i.plugin == "fs_levelup") {
            let Some(m) = strip.modules.iter().find(|m| m.module == ins.id) else { continue };
            let k = m.readouts[6];
            let changed = seen.insert((t.id.0, ins.id.0), k.to_bits()) != Some(k.to_bits());
            if changed && k.is_finite() && (0.0..=1.0).contains(&k) && (value(ins, d::P_TARGET) - k).abs() > 1e-6 {
                out.push((t.id, ins.id, k));
            }
        }
    }
    out
}

/// From the app's frame logic: Learn's result goes onto the session's Target knob (the C++
/// processor's timer: `setParamValue (pTarget, learned)`), one undo step per learn, for every
/// LevelUp whether its strip is shown or not, about ten times a second; and each LevelUp's
/// pairing view ([`pair_info`]) is worked out again when the document changed.
pub(crate) fn learn_watch(app: &mut SoundApp, ctx: &egui::Context) {
    let s = app.engine.session();
    if !s.tracks.iter().any(|t| t.mixer.inserts.iter().any(|i| i.plugin == "fs_levelup")) {
        return;
    }
    let rev = app.engine.revision;
    let rev_key = Id::new("levelup-pair-rev");
    if ctx.data(|d| d.get_temp::<u64>(rev_key)) != Some(rev) {
        for t in &s.tracks {
            for ins in t.mixer.inserts.iter().filter(|i| i.plugin == "fs_levelup") {
                let info = pair_info(s, t.id, ins);
                ctx.data_mut(|d| d.insert_temp(module_key(t.id.0, ins.id.0).with("levelup-pair"), info));
            }
        }
        ctx.data_mut(|d| d.insert_temp(rev_key, rev));
    }
    let now = ctx.input(|i| i.time);
    let due_key = Id::new("levelup-learn-due");
    if now < ctx.data(|d| d.get_temp::<f64>(due_key)).unwrap_or(0.0) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(due_key, now + 0.1));
    let Some(meters) = app.player.as_ref().map(|p| p.meters()) else { return };
    let seen_key = Id::new("levelup-learn-seen");
    let mut seen: LearnSeen = ctx.data(|d| d.get_temp(seen_key)).unwrap_or_default();
    let todo = learn_followups(&app.engine.session().tracks, &meters, &mut seen);
    ctx.data_mut(|d| d.insert_temp(seen_key, seen));
    for (track, module, k) in todo {
        let _ = app.run("mix.module_param", json!({"track": track.0, "module": module.0, "param": "target", "value": k}));
    }
}

/// The Deliver tab's own row: Reset (`resetDelivery`: integrated loudness and the true-peak hold
/// start again), through `mix.module_action`. Returns the y under it.
pub fn deliver_controls(app: &mut SoundApp, ui: &mut egui::Ui, track: TrackId, ins: &Insert, row: Rect, accent: Color32) -> f32 {
    let t = Tokens::current();
    let r = Rect::from_min_size(row.min, egui::vec2(56.0, row.height()));
    let resp = ui.interact(r, ui.id().with(("levelup-reset", track.0, ins.id.0)), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Reset"));
    crate::module_drawer::pill(ui.painter(), r, "Reset", false, resp.hovered(), accent);
    if resp.on_hover_text("Start measuring again: do this at the top of a full play-through").clicked() {
        let _ = app.run("mix.module_action", json!({"track": track.0, "module": ins.id.0, "action": "reset_delivery"}));
    }
    ui.painter().text(pos2(r.right() + 8.0, r.center().y), Align2::LEFT_CENTER, "Integrated and true peak start again", regular(9.0), t.text_dim);
    row.max.y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(st_in: f32, st_out: f32) -> LiveValues {
        let mut r = [f32::NAN; 8];
        r[0] = st_in;
        r[2] = st_out;
        LiveValues { readouts: r, ..LiveValues::default() }
    }

    #[test]
    fn history_keeps_thirty_seconds_at_ten_points_a_second() {
        let mut h = LevelUpHistory::default();
        h.feed(1.0, None);
        assert!(h.is_empty());
        h.feed(1.0, Some(&live(f32::NAN, -20.0)));
        assert!(h.is_empty(), "no reading, no point");
        for k in 0..400 {
            h.feed(2.0 + k as f64 * 0.05, Some(&live(-20.0 - k as f32 * 0.01, -18.0)));
        }
        // 20 s at 20 frames a second: one point per 100 ms
        assert!((199..=201).contains(&h.len()), "{}", h.len());
        for k in 0..600 {
            h.feed(30.0 + k as f64 * 0.1, Some(&live(-23.0, -18.0)));
        }
        assert_eq!(h.len(), HISTORY_LEN);
        let (t0, i, o) = h.point(0).unwrap();
        assert!((h.newest_time().unwrap() - t0 - (HISTORY_SPAN - HISTORY_TICK)).abs() < 1e-6);
        assert_eq!((i, o), (-23.0, -18.0));
        // a long pause starts again
        h.feed(500.0, Some(&live(-10.0, -10.0)));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn role_is_in_the_drawer_but_never_automated_and_the_presses_are_momentary() {
        use crate::module_drawer::{drawer_shows, is_momentary, link_role_param, param_offered};
        let info = soundcraft_dsp::plugin_info("fs_levelup").unwrap();
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, Some("A"));
        let m = s.track_mut(t).unwrap().mixer.add_module(Insert::new("fs_levelup"), None).unwrap();
        let track = s.track(t).unwrap();
        let ins = track.mixer.module(m).unwrap();
        let role = info.param("link").unwrap();
        assert!(link_role_param(info, "link") && !link_role_param(info, "linkGroup"));
        assert!(drawer_shows(ins, role) && param_offered(ins, role));
        let lanes: Vec<&str> = crate::edit_window::lane_params(track, ins, info).iter().map(|p| p.id).collect();
        assert!(!lanes.contains(&"link") && !lanes.contains(&"bypass"), "{lanes:?}");
        assert!(lanes.contains(&"linkGroup") && lanes.contains(&"target"));
        assert!(is_momentary(ins, "normalize") && is_momentary(ins, "learn") && !is_momentary(ins, "freeze"));
        // the drawer's tabs are the editor's
        assert_eq!(crate::module_drawer::drawer_groups(info), ["Level", "Reference", "Deliver"]);
    }

    #[test]
    fn texts_status_and_target() {
        assert_eq!(lufs_text(-69.5), "--");
        assert_eq!(lufs_text(f32::NAN), "--");
        assert_eq!(lufs_text(-23.04), "-23.0");
        assert_eq!(signed_db(3.0), "+3.0");
        assert_eq!(signed_db(-1.25), "-1.2");
        let mut ins = Insert::new("fs_levelup");
        assert_eq!(status(&ins, None, None).0, "Leveling on its own");
        assert_eq!(target_lufs(&ins, None), -18.0);
        ins.params.insert("link".into(), 2.0);
        ins.params.insert("linkGroup".into(), 1.0);
        assert_eq!(status(&ins, None, None), ("OUT of pair B · no IN found yet; using the Target knob".into(), true));
        let mut l = live(-20.0, -20.0);
        l.readouts[7] = -21.0;
        assert_eq!(status(&ins, Some(&l), None).0, "OUT of pair B · matching its IN at -21.0 LUFS");
        assert_eq!(target_lufs(&ins, Some(&l)), -21.0);
        ins.params.insert("link".into(), 1.0);
        assert_eq!(status(&ins, None, None).0, "IN of pair B");
    }

    fn session_with(modules: &[(&str, f32, f32, Option<u32>)]) -> (Session, Vec<(TrackId, ModuleId)>) {
        let mut s = Session::default();
        let mut ids = Vec::new();
        for (plugin, role, pair, group) in modules {
            let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, None);
            let mut ins = Insert::new(*plugin);
            if *plugin == "fs_levelup" {
                ins.params.insert("link".into(), *role);
                ins.params.insert("linkGroup".into(), *pair);
            }
            ins.link_group = *group;
            ids.push((t, s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap()));
        }
        (s, ids)
    }

    fn ins_of(s: &Session, at: (TrackId, ModuleId)) -> Insert {
        s.track(at.0).unwrap().mixer.module(at.1).unwrap().clone()
    }

    #[test]
    fn the_status_line_names_the_group_the_pair_and_why_pairing_fails() {
        // an OUT in pair A with only an IN in pair B in its group
        let (s, ids) = session_with(&[("fs_levelup", 2.0, 0.0, Some(1)), ("fs_levelup", 1.0, 1.0, Some(1))]);
        let out = ins_of(&s, ids[0]);
        let p = pair_info(&s, ids[0].0, &out);
        assert_eq!(p, PairInfo { linked: true, unavailable: None, in_pairs: vec![1] });
        assert_eq!(status(&out, None, Some(&p)), ("No IN in pair A (the group's IN: pair B)".into(), true));
        // not in a link group
        let (s, ids) = session_with(&[("fs_levelup", 2.0, 0.0, None)]);
        let out = ins_of(&s, ids[0]);
        let p = pair_info(&s, ids[0].0, &out);
        assert!(!p.linked);
        assert_eq!(status(&out, None, Some(&p)).0, "OUT of pair A · not in a link group: pick one under LINK");
        // another plugin in the group: unavailable
        let (s, ids) = session_with(&[("fs_levelup", 1.0, 0.0, Some(2)), ("test_link", 0.0, 0.0, Some(2))]);
        let inp = ins_of(&s, ids[0]);
        let p = pair_info(&s, ids[0].0, &inp);
        assert_eq!(status(&inp, None, Some(&p)), ("Pairing unavailable: the link group holds other plugins".into(), true));
        // the IN of the OUT's own pair, not yet measured
        let (s, ids) = session_with(&[("fs_levelup", 2.0, 2.0, Some(3)), ("fs_levelup", 1.0, 2.0, Some(3))]);
        let out = ins_of(&s, ids[0]);
        let p = pair_info(&s, ids[0].0, &out);
        assert_eq!(status(&out, None, Some(&p)).0, "OUT of pair C · no IN found yet; using the Target knob");
    }

    #[test]
    fn learn_goes_back_on_the_target_from_the_meters_whether_shown_or_not() {
        // Pure: what the frame logic writes back reads only the session and the strip meters, no
        // tile or drawer.
        let (s, ids) = session_with(&[("fs_levelup", 0.0, 0.0, None), ("fs_levelup", 0.0, 0.0, None)]);
        let mut snap = soundcraft_playback::MeterSnapshot::default();
        for (i, (t, m)) in ids.iter().enumerate() {
            let mut r = [f32::NAN; 8];
            r[6] = if i == 0 { 0.5 } else { f32::NAN };
            let mut strip = soundcraft_mix::StripMeter::default();
            strip.modules.push(soundcraft_mix::ModuleMeter { module: *m, readouts: r, ..Default::default() });
            snap.tracks.insert(*t, strip);
        }
        let mut seen = LearnSeen::new();
        assert_eq!(learn_followups(&s.tracks, &snap, &mut seen), vec![(ids[0].0, ids[0].1, 0.5)]);
        // acted on once: the same reading is not written again
        assert!(learn_followups(&s.tracks, &snap, &mut seen).is_empty());
        // once the session holds it, nothing to write even on a first sight
        let mut s2 = s.clone();
        s2.track_mut(ids[0].0).unwrap().mixer.module_mut(ids[0].1).unwrap().params.insert("target".into(), 0.5);
        assert!(learn_followups(&s2.tracks, &snap, &mut LearnSeen::new()).is_empty());
    }

    #[test]
    fn a_learn_equal_to_the_target_never_reverts_a_later_target_edit() {
        // Learn takes exactly the stored Target (0.5): the plugin keeps its readout (the host never
        // set Target since). The user then turns Target to 0.8 while the meters still show 0.5:
        // nothing is written back over the edit.
        let (mut s, ids) = session_with(&[("fs_levelup", 0.0, 0.0, None)]);
        let (t, m) = ids[0];
        s.track_mut(t).unwrap().mixer.module_mut(m).unwrap().params.insert("target".into(), 0.5);
        let mut snap = soundcraft_playback::MeterSnapshot::default();
        let mut r = [f32::NAN; 8];
        r[6] = 0.5;
        let mut strip = soundcraft_mix::StripMeter::default();
        strip.modules.push(soundcraft_mix::ModuleMeter { module: m, readouts: r, ..Default::default() });
        snap.tracks.insert(t, strip);
        let mut seen = LearnSeen::new();
        assert!(learn_followups(&s.tracks, &snap, &mut seen).is_empty(), "already the Target");
        s.track_mut(t).unwrap().mixer.module_mut(m).unwrap().params.insert("target".into(), 0.8);
        for _ in 0..3 {
            assert!(learn_followups(&s.tracks, &snap, &mut seen).is_empty(), "the edit is not reverted");
        }
        // a new Learn (the readout changes) is written
        snap.tracks.get_mut(&t).unwrap().modules[0].readouts[6] = 0.3;
        assert_eq!(learn_followups(&s.tracks, &snap, &mut seen), vec![(t, m, 0.3)]);
    }
}

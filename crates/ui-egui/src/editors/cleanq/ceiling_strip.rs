//! `CleanQCeilingCard.h`: the Ceiling strip docked along the bottom of the graph (noise guides,
//! the capture, the ceiling, Conform, and the dynamic ceiling).

use super::*;
use crate::editor_kit::cards::docked_strip;
use crate::editor_kit::icons::tri_side;
use soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling as cl;

/// JUCE's skew from a midpoint (`setSkewFactorFromMidPoint`).
fn skew_of(lo: f64, hi: f64, mid: f64) -> f64 {
    0.5f64.ln() / ((mid - lo) / (hi - lo)).ln()
}
fn to_prop(v: f64, lo: f64, hi: f64, skew: f64) -> f32 {
    (((v - lo) / (hi - lo)).clamp(0.0, 1.0).powf(skew)) as f32
}
fn from_prop(pr: f32, lo: f64, hi: f64, skew: f64) -> f64 {
    lo + (hi - lo) * f64::from(pr).clamp(0.0, 1.0).powf(1.0 / skew)
}

// ============================================================================ the Ceiling strip

/// Where the strip's controls go: left to right along four rows. The full editor's rows never
/// run out of room; the slim layout's wrap (`wrap`), a group of controls moving to the next line
/// together, and its rows start at the strip's left edge rather than after the title column.
struct Flow {
    left: f32,
    right: f32,
    indent: f32,
    x: f32,
    y: f32,
    row_h: f32,
    gap_y: f32,
    wrap: bool,
}

impl Flow {
    fn line_start(&self) -> f32 {
        self.left + self.indent
    }
    fn at_start(&self) -> bool {
        self.x <= self.line_start() + 0.5
    }
    fn newline(&mut self) {
        self.y += self.row_h + self.gap_y;
        self.x = self.line_start();
    }
    /// The next row of the strip (both layouts).
    fn next_row(&mut self) {
        self.newline();
    }
    /// The slim layout ends the line here.
    fn soft_break(&mut self) {
        if self.wrap && !self.at_start() {
            self.newline();
        }
    }
    /// Keeps the next `w` together on one line.
    fn group(&mut self, w: f32) {
        if self.wrap && !self.at_start() && self.x + w > self.right + 0.5 {
            self.newline();
        }
    }
    /// The next `w` wide cell of the row.
    fn take(&mut self, w: f32) -> Rect {
        self.group(w);
        let r = Rect::from_min_size(pos2(self.x, self.y), vec2(w, self.row_h));
        self.x += w;
        r
    }
    /// Space between cells (none at the start of a wrapped line).
    fn gap(&mut self, w: f32) {
        if !(self.wrap && self.at_start()) {
            self.x += w;
        }
    }
    /// The rest of the line up to `right`, on a line of its own when less than `min_w` is left.
    fn rest(&mut self, min_w: f32, right: f32) -> Rect {
        if self.wrap && !self.at_start() && right - self.x < min_w {
            self.newline();
        }
        let right = if self.wrap { right.max(self.right) } else { right };
        Rect::from_min_max(pos2(self.x, self.y), pos2(right.max(self.x), self.y + self.row_h))
    }
    fn bottom(&self) -> f32 {
        self.y + self.row_h
    }
}

/// A caption in `r`: one clipped line, or in the slim layout up to two wrapped lines.
fn strip_text(p: &egui::Painter, r: Rect, text: &str, colour: Color32, wrap: bool) {
    if wrap {
        let mut job = egui::text::LayoutJob::simple(text.to_string(), regular(10.0), colour, r.width());
        job.wrap.max_rows = 2;
        let g = p.layout_job(job);
        let y = r.center().y - g.size().y * 0.5;
        p.with_clip_rect(r.expand2(vec2(0.0, 3.0))).galley(pos2(r.left(), y), g, colour);
    } else {
        p.with_clip_rect(r).text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, text, regular(10.5), colour);
    }
}

/// The strip in `strip`; `slim`: the slim layout's wrapped rows. Returns the bottom of what it
/// laid out (the slim layout sizes the strip by it).
pub(super) fn ceiling_strip(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, strip: Rect, slim: bool) -> f32 {
    let _ = ui.interact(strip, ui.id().with("cqw-strip-swallow"), Sense::click());
    let p = ui.painter().clone();
    docked_strip(&p, strip, CARD_BG, &PALETTE);
    let s = &*cx.ceil;
    let key = (cx.host.track, cx.ins().id);
    let (has_data, seconds) = app.ui.cq_learn.get(&key).map_or((false, 0.0), |l| (l.capture().averager().has_data(), l.seconds()));
    let trace = st.trace.clone();
    let r = strip.shrink(8.0);
    let (row_h, gap_y, title_w) = (24.0, 6.0, 70.0);
    let mut fl = Flow { left: r.left(), right: r.right(), indent: 0.0, x: r.left(), y: r.top(), row_h, gap_y, wrap: slim };
    let label = |p: &egui::Painter, fl: &mut Flow, text: &str, w: f32| {
        let c = fl.take(w);
        p.text(pos2(c.left(), c.center().y), Align2::LEFT_CENTER, text, regular(10.5), TEXT_FAINT);
    };
    // The slim layout's narrower cells.
    let noise_w = if slim { 44.0 } else { 52.0 };
    let (dyn_label_w, dyn_slider_w, dyn_value_w) = if slim { (40.0, 44.0, 40.0) } else { (44.0, 56.0, 44.0) };
    let mut changed: Option<CeilingState> = None;
    let mut edit = |f: &dyn Fn(&mut CeilingState)| {
        let mut n = changed.clone().unwrap_or_else(|| s.clone());
        f(&mut n);
        changed = Some(n);
    };

    // ---- row 1: title, noise, capture ... pin, close
    let y1 = r.top();
    let title = fl.take(title_w);
    p.text(pos2(title.left(), title.center().y), Align2::LEFT_CENTER, "CEILING", bold(12.0), WARN);
    fl.indent = if slim { 0.0 } else { title_w };
    let close_r = Rect::from_min_size(pos2(r.right() - 24.0, y1), vec2(24.0, row_h));
    let pin_r = Rect::from_min_size(pos2(close_r.left() - 4.0 - 36.0, y1), vec2(36.0, row_h));
    fl.soft_break();
    fl.group(44.0 + noise_w * cl::NUM_NOISES as f32);
    label(&p, &mut fl, "Noise", 44.0);
    for i in 0..cl::NUM_NOISES {
        let edge = if i == 0 {
            Edge::Left
        } else if i == cl::NUM_NOISES - 1 {
            Edge::Right
        } else {
            Edge::Middle
        };
        if card_button(ui, fl.take(noise_w), ("noise", i), &PALETTE, Btn::new(cl::NOISES[i].name).on(s.noise_on[i]).edge(edge), Sense::click())
            .clicked()
        {
            edit(&|n: &mut CeilingState| {
                n.noise_on[i] = !n.noise_on[i];
                n.noise_manual[i] = false;
            });
        }
    }
    fl.gap(18.0);
    fl.group(54.0 + 58.0 + 6.0 + 74.0 + 6.0 + 48.0);
    label(&p, &mut fl, "Capture", 54.0);
    let avg = format!(
        "{}    ",
        if s.avg_seconds <= 0.0 { "All".to_string() } else { format!("{:.*} s", if s.avg_seconds < 1.0 { 1 } else { 0 }, s.avg_seconds) }
    );
    let ar = fl.take(58.0);
    let aresp = card_button(ui, ar, "avg", &PALETTE, Btn::new(&avg), Sense::click());
    tri_down(&p, pos2(ar.right() - 9.0, ar.center().y), ink_of(&PALETTE, &aresp, false));
    egui::Popup::menu(&aresp).show(|ui| {
        if ui.add(egui::Button::selectable(s.avg_seconds <= 0.0, "Everything played")).clicked() {
            edit(&|n: &mut CeilingState| n.avg_seconds = 0.0);
            ui.close();
        }
        for v in [1.0, 3.0, 10.0, 30.0] {
            if ui.add(egui::Button::selectable((s.avg_seconds - v).abs() < 0.01, format!("Remember the last {v:.0} s"))).clicked() {
                edit(&|n: &mut CeilingState| n.avg_seconds = v);
                ui.close();
            }
        }
    });
    fl.gap(6.0);
    let sm = format!("1/{} oct    ", s.smoothing);
    let smr = fl.take(74.0);
    let sresp = card_button(ui, smr, "smooth", &PALETTE, Btn::new(&sm), Sense::click());
    tri_down(&p, pos2(smr.right() - 9.0, smr.center().y), ink_of(&PALETTE, &sresp, false));
    egui::Popup::menu(&sresp).show(|ui| {
        for n in [3, 6, 12] {
            if ui.add(egui::Button::selectable(s.smoothing == n, format!("Smooth to 1/{n} octave"))).clicked() {
                edit(&|c: &mut CeilingState| c.smoothing = n);
                ui.close();
            }
        }
    });
    fl.gap(6.0);
    if card_button(ui, fl.take(48.0), "resetcap", &PALETTE, Btn::new("Reset"), Sense::click()).clicked() {
        if let Some(l) = app.ui.cq_learn.get_mut(&key) {
            l.reset_capture();
        }
        st.conform_result.clear();
    }
    fl.gap(8.0);
    let status = if !has_data {
        "no signal".to_string()
    } else if st.hold {
        "held".to_string()
    } else {
        format!("live, {:.1} s", if s.avg_seconds > 0.0 { seconds.min(s.avg_seconds) } else { seconds })
    };
    let sr = fl.rest(90.0, pin_r.left() - 8.0);
    strip_text(&p, sr, &status, TEXT_DIM, false);
    if card_button(ui, pin_r, "pin", &PALETTE, Btn::new("Pin").on(s.pinned), Sense::click()).clicked() {
        edit(&|n: &mut CeilingState| n.pinned = !n.pinned);
    }
    if card_button(ui, close_r, "close", &PALETTE, Btn::new("\u{00D7}"), Sense::click()).clicked() {
        st.ceiling_open = false;
    }

    // ---- row 2: the ceiling
    fl.next_row();
    fl.group(44.0 + 104.0 + 6.0 + 48.0 + 6.0 + 44.0);
    label(&p, &mut fl, "Ceiling", 44.0);
    if card_button(ui, fl.take(104.0), "autogen", &PALETTE, Btn::new("Auto Generate").bold(), Sense::click()).clicked() {
        if trace.len() != cl::GRID_N {
            st.conform_result = "Nothing captured yet: play something first.".into();
        } else {
            let c = cl::auto_ceiling(&trace);
            if c.is_valid() {
                set_ceiling(app, cx, &c, false);
                st.ceiling_node = None;
                st.conform_result.clear();
            } else {
                st.conform_result = "Not enough signal to generate from.".into();
            }
        }
    }
    fl.gap(6.0);
    if card_button(ui, fl.take(48.0), "resetceil", &PALETTE, Btn::new("Reset"), Sense::click()).clicked() {
        let mut m: Vec<f64> = trace.iter().copied().filter(|v| *v > -100.0).collect();
        let level = if m.is_empty() {
            -30.0
        } else {
            let mid = m.len() / 2;
            m.select_nth_unstable_by(mid, f64::total_cmp);
            cl::node_level(m[mid])
        };
        set_ceiling(app, cx, &cl::flat_ceiling(level), false);
        st.ceiling_node = None;
        st.conform_result.clear();
    }
    fl.gap(6.0);
    if card_button(ui, fl.take(44.0), "snap", &PALETTE, Btn::new("Snap").on(s.snap), Sense::click()).clicked() {
        edit(&|n: &mut CeilingState| n.snap = !n.snap);
    }
    fl.gap(18.0);
    let has = s.shape.is_valid();
    let n = s.shape.pts.len();
    let node_w = if slim { (r.width() - 44.0).min(200.0) } else { 200.0 };
    fl.group(22.0 + node_w + 22.0);
    let prev = fl.take(22.0);
    let pr = card_button(ui, prev, "prevnode", &PALETTE, Btn::new("").edge(Edge::Left).enabled(has), Sense::click());
    tri_side(&p, prev.center(), true, ink_of(&PALETTE, &pr, false));
    let node_area = fl.take(node_w);
    let next = fl.take(22.0);
    let nx = card_button(ui, next, "nextnode", &PALETTE, Btn::new("").edge(Edge::Right).enabled(has), Sense::click());
    tri_side(&p, next.center(), false, ink_of(&PALETTE, &nx, false));
    fl.gap(12.0);
    for (resp, d) in [(pr, -1i32), (nx, 1)] {
        if resp.clicked() && n > 0 {
            st.ceiling_node = Some(match st.ceiling_node {
                None => {
                    if d > 0 {
                        0
                    } else {
                        n - 1
                    }
                }
                Some(c) => (c as i32 + d).clamp(0, n as i32 - 1) as usize,
            });
        }
    }
    p.rect_filled(node_area, CornerRadius::ZERO, PALETTE.well);
    let node_text = match st.ceiling_node.filter(|i| has && *i < n) {
        Some(i) => {
            let pt = s.shape.pts[i];
            let f = if pt.hz >= 1000.0 {
                format!("{:.*} kHz", if pt.hz >= 10_000.0 { 1 } else { 2 }, pt.hz / 1000.0)
            } else {
                format!("{} Hz", pt.hz.round() as i64)
            };
            format!("Node {} of {}   {}   {:.1} dB", i + 1, n, f, pt.db)
        }
        None if has => "Click a node".into(),
        None => "no ceiling yet".into(),
    };
    p.with_clip_rect(node_area).text(node_area.center(), Align2::CENTER_CENTER, node_text, regular(10.5), if has { TEXT_BRIGHT } else { TEXT_FAINT });
    let hint = if has {
        "Drag a node: it and every node after it move.  Alt: just that node."
    } else {
        "Play something, then Auto Generate: 16 nodes on the capture."
    };
    let hr = fl.rest(160.0, r.right());
    strip_text(&p, hr, hint, TEXT_FAINT, slim);

    // ---- row 3: how much, how far, how many -- then do it
    fl.next_row();
    fl.group(50.0 + 150.0 + 44.0);
    label(&p, &mut fl, "Amount", 50.0);
    let amc = fl.take(150.0);
    let am = Rect::from_min_size(pos2(amc.left(), amc.center().y - 8.0), vec2(150.0, 16.0));
    let amount_out = h_slider(ui, am, "amount", &PALETTE, s.amount as f32, 1.0, false);
    let nv = amount_out.prop;
    let amount = nv.map_or(s.amount, f64::from);
    if let Some(v) = nv {
        let v = (f64::from(v) * 100.0).round() / 100.0;
        edit(&|n: &mut CeilingState| n.amount = v);
    }
    let amv = fl.take(44.0);
    p.text(pos2(amv.right(), amv.center().y), Align2::RIGHT_CENTER, format!("{} %", (amount * 100.0).round() as i64), regular(10.5), TEXT_BRIGHT);
    fl.gap(8.0);
    fl.group(66.0 + 10.0 + 60.0 + 20.0 + 20.0);
    let mx = format!("\u{00B1}{} dB    ", s.max_db.round() as i64);
    let mr = fl.take(66.0);
    let mresp = card_button(ui, mr, "maxdb", &PALETTE, Btn::new(&mx), Sense::click());
    tri_down(&p, pos2(mr.right() - 9.0, mr.center().y), ink_of(&PALETTE, &mresp, false));
    egui::Popup::menu(&mresp).show(|ui| {
        for v in [3, 6, 9, 12, 18, 24] {
            if ui.add(egui::Button::selectable(s.max_db.round() as i32 == v, format!("\u{00B1}{v} dB at most"))).clicked() {
                edit(&|n: &mut CeilingState| n.max_db = f64::from(v));
                ui.close();
            }
        }
    });
    fl.gap(10.0);
    let bt = fl.take(60.0);
    // Conform's fixed bells: at most 12 (a state from before them may say 16).
    let most = cl::CONFORM_MAX_BELLS as i32;
    p.text(pos2(bt.left(), bt.center().y), Align2::LEFT_CENTER, format!("Bands {}", s.max_bands.min(most)), regular(10.5), TEXT_DIM);
    if card_button(ui, fl.take(20.0), "bandsdown", &PALETTE, Btn::new("-").edge(Edge::Left), Sense::click()).clicked() {
        edit(&|n: &mut CeilingState| n.max_bands = (n.max_bands.min(most) - 1).clamp(1, most));
    }
    if card_button(ui, fl.take(20.0), "bandsup", &PALETTE, Btn::new("+").edge(Edge::Right), Sense::click()).clicked() {
        edit(&|n: &mut CeilingState| n.max_bands = (n.max_bands.min(most) + 1).clamp(1, most));
    }
    fl.gap(8.0);
    if card_button(ui, fl.take(68.0), "cuts", &PALETTE, Btn::new("Cuts only").on(s.cuts_only), Sense::click()).clicked() {
        edit(&|n: &mut CeilingState| n.cuts_only = !n.cuts_only);
    }
    fl.gap(18.0);
    fl.group(92.0 + 6.0 + 62.0);
    if card_button(ui, fl.take(92.0), "conform", &PALETTE, Btn::new("Conform").on(true).bold(), Sense::click()).clicked() {
        conform(app, cx, st, &trace);
    }
    fl.gap(6.0);
    let any_gen = !s.generated.is_empty();
    let rr = fl.take(62.0);
    let mut rui = ui.new_child(egui::UiBuilder::new().max_rect(rr));
    if !any_gen {
        rui.multiply_opacity(0.4);
    }
    if card_button(&mut rui, rr, "remove", &PALETTE, Btn::new("Remove").enabled(any_gen), Sense::click()).clicked() {
        let mut list: Vec<(&str, f32)> = s
            .generated
            .iter()
            .filter(|g| cx.bands.get(**g as usize).is_some_and(|b| b.on != q::ON_FREE))
            .map(|g| (bp(*g as usize, cq::F_ON), q::ON_FREE as f32))
            .collect();
        if s.makeup_db != 0.0 {
            let (out, _) = cl::apply_makeup(q::knob_to_out_db(pv(cx.ins(), "outGain")), s.makeup_db, 0.0);
            list.push(("outGain", q::out_db_to_knob(out).clamp(0.0, 1.0)));
        }
        // The bands freed, their make-up taken off and the list cleared: one undo step.
        let mut ns = s.clone();
        ns.generated.clear();
        ns.makeup_db = 0.0;
        crate::editor_kit::one_step(app, |app| {
            set_params_merged(app, cx.host, &list, "remove-generated");
            crate::editor_kit::set_state_merged(app, cx.host, &ns.write(), "remove-generated");
        });
        if st.selected.is_some_and(|sl| s.generated.contains(&(sl as i32))) {
            select(st, None);
        }
        st.conform_result.clear();
    }
    fl.gap(10.0);
    let result = if st.conform_result.is_empty() && any_gen {
        format!("{} {} from Conform", s.generated.len(), if s.generated.len() == 1 { "band" } else { "bands" })
    } else {
        st.conform_result.clone()
    };
    if !(slim && result.is_empty()) {
        let rs = fl.rest(160.0, r.right());
        strip_text(&p, rs, &result, TEXT_DIM, slim);
    }

    // ---- row 4: the dynamic ceiling
    fl.next_row();
    let dyn_on = pv(cx.ins(), "ceilOn") > 0.5;
    fl.group(54.0 + 36.0);
    // "Dynamic" lights while the dynamic ceiling is cutting (`dynCutting`).
    let dl = fl.take(54.0);
    p.text(pos2(dl.left(), dl.center().y), Align2::LEFT_CENTER, "Dynamic", regular(10.5), if cx.cutting() { WARN } else { TEXT_FAINT });
    if card_button(ui, fl.take(36.0), "dynon", &PALETTE, Btn::new("On").on(dyn_on), Sense::click()).clicked() {
        params(app, cx, &[("ceilOn", if dyn_on { 0.0 } else { 1.0 })]);
    }
    fl.gap(12.0);
    let knobs: [(&str, &str, f64, f64, f64, &str, usize); 5] = [
        ("ceilThr", "Thresh", 0.0, 12.0, 6.0, " dB", 1),
        ("ceilAmount", "Amount", 0.0, 100.0, 50.0, " %", 0),
        ("ceilMaxCut", "Max", 0.0, 24.0, 12.0, " dB", 1),
        ("ceilAttack", "Attack", 1.0, 100.0, 15.0, " ms", 0),
        ("ceilRelease", "Release", 20.0, 1000.0, 200.0, " ms", 0),
    ];
    for (id, name, lo, hi, mid, unit, dp) in knobs {
        fl.group(dyn_label_w + dyn_slider_w + 4.0 + dyn_value_w);
        label(&p, &mut fl, name, dyn_label_w);
        let skew = if id == "ceilAttack" || id == "ceilRelease" { skew_of(lo, hi, mid) } else { 1.0 };
        let v = f64::from(pv(cx.ins(), id));
        let def = f64::from(cq::CLEANQ_PARAMS.iter().find(|p| p.id == id).map_or(0.0, |p| p.default));
        let sc = fl.take(dyn_slider_w);
        let sr = Rect::from_min_size(pos2(sc.left(), sc.center().y - 8.0), vec2(dyn_slider_w, 16.0));
        let so = h_slider(ui, sr, id, &PALETTE, to_prop(v, lo, hi, skew), to_prop(def, lo, hi, skew), !dyn_on);
        let (nv, done) = (so.prop, so.ended);
        let shown = nv.map_or(v, |pr| from_prop(pr, lo, hi, skew));
        if nv.is_some() {
            let step = if id == "ceilAmount" || id == "ceilAttack" || id == "ceilRelease" { 1.0 } else { 0.1 };
            params_merged(app, cx, &[(id, ((shown / step).round() * step) as f32)], id);
        }
        if done {
            end_gesture(app);
        }
        fl.x += 4.0;
        let vc = fl.take(dyn_value_w);
        p.text(
            pos2(vc.left(), vc.center().y),
            Align2::LEFT_CENTER,
            format!("{shown:.dp$}{unit}"),
            regular(10.5),
            al(TEXT_BRIGHT, if dyn_on { 1.0 } else { 0.5 }),
        );
        fl.gap(6.0);
    }
    let delta = pv(cx.ins(), "ceilDelta") > 0.5;
    if card_button(ui, fl.take(48.0), "delta", &PALETTE, Btn::new("Delta").on(delta), Sense::click()).clicked() {
        params(app, cx, &[("ceilDelta", if delta { 0.0 } else { 1.0 })]);
    }

    if let Some(n) = changed {
        if nv.is_some() {
            // The Amount slider: a drag is one undo step.
            crate::editor_kit::set_state_merged(app, cx.host, &n.write(), "ceil-amount");
        } else {
            write_state(app, cx, &n);
        }
    }
    if amount_out.ended {
        end_gesture(app);
    }
    fl.bottom() + 8.0
}

/// `conformNow`: replace the whole EQ with bands that move the capture onto the ceiling, and put
/// the make-up that keeps the capture's level on the output (the last Conform's taken back first,
/// so the user's own trim stays).
fn conform(app: &mut SoundApp, cx: &Cx, st: &mut Editor, trace: &[f64]) {
    let s = &*cx.ceil;
    if trace.len() != cl::GRID_N {
        st.conform_result = "Nothing captured yet: play something first.".into();
        return;
    }
    if !s.shape.is_valid() {
        st.conform_result = "No ceiling yet \u{2014} press Auto Generate.".into();
        return;
    }
    let replaced = cx.bands.iter().filter(|b| b.on != q::ON_FREE && !s.generated.contains(&(b.slot as i32))).count();
    let cs = cl::ConformSettings {
        amount: s.amount,
        max_db: s.max_db,
        cuts_only: s.cuts_only,
        max_bands: s.max_bands.min(cl::CONFORM_MAX_BELLS as i32),
        sample_rate: cx.sr(),
        // the learned material's sample peak: the make-up keeps it 1 dB under 0 dBFS
        input_peak_db: app.ui.cq_learn.get(&(cx.host.track, cx.ins().id)).map_or(f64::NEG_INFINITY, |l| l.capture().averager().peak_db()),
    };
    let result = cl::conform(trace, &s.shape, &cs);
    let mut list: Vec<(&str, f32)> = (0..q::NUM_BANDS).map(|b| (bp(b, cq::F_ON), q::ON_FREE as f32)).collect();
    let mut generated = Vec::new();
    for (slot, b) in result.bands.iter().enumerate().take(q::NUM_BANDS) {
        list.retain(|(id, _)| *id != bp(slot, cq::F_ON));
        list.extend([
            (bp(slot, cq::F_FREQ), q::freq_to_knob(b.hz).clamp(0.0, 1.0)),
            (bp(slot, cq::F_GAIN), q::gain_to_knob(b.db).clamp(0.0, 1.0)),
            (bp(slot, cq::F_Q), q::q_to_knob(b.q).clamp(0.0, 1.0)),
            (bp(slot, cq::F_TYPE), b.type_ as f32),
            (bp(slot, cq::F_PLACE), q::PLACE_STEREO as f32),
            (bp(slot, cq::F_SLOPE), 1.0),
            (bp(slot, cq::F_DYN), 0.0),
            (bp(slot, cq::F_ON), q::ON_ACTIVE as f32),
        ]);
        generated.push(slot as i32);
    }
    // The make-up: the output trim with the last Conform's taken back (if its bands are still
    // there to own it) and this one's put on.
    let old_makeup = if s.generated.is_empty() { 0.0 } else { s.makeup_db };
    let (out, applied) = cl::apply_makeup(q::knob_to_out_db(pv(cx.ins(), "outGain")), old_makeup, result.makeup_db);
    list.push(("outGain", q::out_db_to_knob(out).clamp(0.0, 1.0)));
    // The new bands, the output and the list of what Conform wrote: one undo step.
    let mut ns = s.clone();
    ns.generated = generated;
    ns.makeup_db = if ns.generated.is_empty() { 0.0 } else { applied };
    crate::editor_kit::one_step(app, |app| {
        set_params_merged(app, cx.host, &list, "conform");
        crate::editor_kit::set_state_merged(app, cx.host, &ns.write(), "conform");
    });
    st.sweeps = Default::default();
    select(st, None);
    st.conform_result = conform_status(&result, &cs, ns.makeup_db);
    if replaced > 0 {
        st.conform_result += &format!(", replacing {replaced} {} of yours (Undo brings them back)", if replaced == 1 { "band" } else { "bands" });
    }
}

/// What Conform did, honestly: the correction asked for BEFORE the range clamp, the EQ it wrote,
/// the output make-up, and a warning when the ceiling asked for more than the range allows.
pub(super) fn conform_status(r: &cl::ConformResult, cs: &cl::ConformSettings, makeup_db: f64) -> String {
    if r.bands.is_empty() {
        return "Already within a quarter of a dB of the ceiling.".into();
    }
    let mut t = format!(
        "{} {}: asked {:+.1}..{:+.1} dB, EQ {:+.1}..{:+.1} dB, output {:+.1} dB",
        r.bands.len(),
        if r.bands.len() == 1 { "band" } else { "bands" },
        r.asked_lo,
        r.asked_hi,
        r.curve_lo,
        r.curve_hi,
        makeup_db
    );
    if r.makeup_capped {
        t += &format!(
            " (held from {:+.1} dB: the learned peak, {:.1} dBFS, stays {:.0} dB under 0)",
            r.makeup_wanted_db,
            cs.input_peak_db,
            cl::CONFORM_PEAK_HEADROOM_DB
        );
    }
    if r.clamped(cs.max_db) {
        t += &format!(" \u{2014} past \u{00B1}{} dB: the ceiling is far from the source", cs.max_db.round() as i64);
    }
    t
}

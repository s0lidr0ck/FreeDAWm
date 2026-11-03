//! Shale in the strip: its graph (the C++ `ShaleDisplay`: the input dim, the gated output bright
//! over it, the detector as a line and the threshold you drag, on a mirrored dB axis) and the
//! drawer's rules (what is hidden, what Auto Band and Kit Link lock, the panel's value text) and
//! the Kit Link tab's status lines and Line Up (the editor's link strip and phase row).
//!
//! The history is the engine's own: the DSP bins every 4 ms (`ShaleDSP::scope`, 2500 bins, ten
//! seconds) and pushes each finished bin to its analysis ring as `[peak, gain, det]`; the strip
//! keeps the last [`SCOPE_BINS`] of them ([`crate::module_graphs::Streams`]). The tile shows the
//! last 2.5 s, the drawer 2.5, 5 (the editor's default) or 10, picked by the span button as the
//! editor's; columns merge their bins as the C++ does (the loudest peak, the most-closed gate,
//! the loudest detector), so a transient is never averaged away.

use crate::SoundApp;
use crate::module_drawer::Switch;
use crate::module_graphs::LiveValues;
use crate::theme::{Tokens, bold, module_accent, regular};
use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::ParamInfo;
use soundcraft_dsp::analysis::REPORT_LEN;
use soundcraft_dsp::plugins::fs::shale::{self as sh, INERT_PARAMS, SHALE_PARAMS, shale_align, shale_config as sc, shale_dsp as sd, shale_link};
use soundcraft_model::{Insert, Session, TrackId};
use std::collections::VecDeque;

/// Bins the strip keeps: the DSP's ten seconds.
pub const SCOPE_BINS: usize = sd::SCOPE_BINS;
/// Values per bin on the ring.
pub const SCOPE_VALUES: usize = sd::SCOPE_BIN_VALUES;
/// One bin per 4 ms (`scopeHop`): 250 a second at any rate.
pub const BINS_PER_SECOND: f32 = 250.0;
/// The spans the drawer offers (`setDisplaySeconds`), and the tile's.
pub const SPANS: [f32; 3] = [2.5, 5.0, 10.0];
pub const TILE_SPAN: f32 = 2.5;
/// The most columns a graph draws (wider rects merge more bins per column).
const MAX_COLS: usize = 2048;

/// The value of a parameter of `ins`, clamped (its default when unset).
fn value(ins: &Insert, index: usize) -> f32 {
    SHALE_PARAMS.get(index).map_or(0.0, |p| p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default)))
}

/// Every value of `ins` in parameter order, for `formatParam`'s context.
fn context(ins: &Insert) -> sc::Context {
    let mut v = [0.0f32; sd::NUM_PARAMS];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = value(ins, i);
    }
    sc::Context::from_values(&v)
}

/// The MIDI Out pill's tooltip.
pub const MIDI_OUT_TIP: &str = "Sends a note per hit to any track whose input is MIDI from this module";

/// The drawer's rule for a Shale parameter: SC Listen (the engine never reads it) is hidden; the
/// rest show under their own names (MIDI Out and MIDI Note feed a track's "MIDI from" input).
pub fn switch(_ins: &Insert, id: &str) -> Option<Switch> {
    if id == "phaseRef" {
        // the editor's pill: "Phase Ref" (the full name does not fit a drawer cell)
        return Some(Switch::Show { label: "Phase Ref", opts: &[] });
    }
    INERT_PARAMS.contains(&id).then_some(Switch::Hide)
}

/// The detector band and the DeBleed crossover are the Mic's while Auto Band is on: the editor
/// locks the three dials (`refreshForMic`). With Kit Link off, its group, role, Reject, Phase
/// Reference and Align To are inactive (`refreshLinkStrip`, `refreshAlign`: `setInactive`).
pub fn locked(ins: &Insert, id: &str) -> bool {
    (matches!(id, "detLow" | "detHigh" | "xover") && value(ins, sd::P_AUTO_BAND) > 0.5)
        || (matches!(id, "linkGroup" | "linkRole" | "reject" | "phaseRef" | "alignTo") && value(ins, sd::P_LINK) < 0.5)
}

fn choice(ins: &Insert, index: usize) -> i32 {
    value(ins, index).round() as i32
}

fn mic_name(i: i32) -> &'static str {
    usize::try_from(i).ok().and_then(|i| sc::INSTRUMENTS.get(i)).copied().unwrap_or("?")
}

fn group_name(i: i32) -> &'static str {
    sc::LINK_GROUPS.get(i.clamp(0, 3) as usize).copied().unwrap_or("A")
}

/// The link strip's status (`refreshLinkStrip`), and whether it is a warning: which other mics are
/// live in this mic's group (A–D) of its link group, or why it hears nobody. The C++ reads the
/// live instances from its shared file; here the session says who is in the link group with Kit
/// Link on. `no_slot`: the module's report says it got no slot (a member past the 16).
pub fn link_status(s: &Session, track: TrackId, ins: &Insert, no_slot: bool) -> (String, bool) {
    if value(ins, sd::P_LINK) < 0.5 {
        return ("Kit Link off · this gate decides on its own".into(), false);
    }
    let Some(g) = ins.link_group else {
        return ("Kit Link on, in no link group · pick one under LINK to hear the other mics".into(), true);
    };
    let members: Vec<(TrackId, &Insert)> =
        s.tracks.iter().flat_map(|t| t.mixer.inserts.iter().map(move |i| (t.id, i))).filter(|(_, i)| i.link_group == Some(g)).collect();
    if members.iter().any(|(_, i)| i.plugin != "fs_shale") {
        return ("Kit Link unavailable · the link group holds other plugins".into(), true);
    }
    if no_slot {
        return (format!("Kit Link unavailable · all {} link slots are in use", shale_link::MAX_INSTANCES), true);
    }
    let mine = choice(ins, sd::P_LINK_GROUP).clamp(0, 3);
    let others = members.iter().filter(|(t, i)| !(*t == track && i.id == ins.id) && value(i, sd::P_LINK) > 0.5);
    let names: Vec<&str> =
        others.clone().filter(|(_, i)| choice(i, sd::P_LINK_GROUP).clamp(0, 3) == mine).map(|(_, i)| mic_name(choice(i, sd::P_INSTRUMENT))).collect();
    let elsewhere: Vec<String> = others
        .filter(|(_, i)| choice(i, sd::P_LINK_GROUP).clamp(0, 3) != mine)
        .map(|(_, i)| format!("{} is in {}", mic_name(choice(i, sd::P_INSTRUMENT)), group_name(choice(i, sd::P_LINK_GROUP))))
        .collect();
    if names.is_empty() {
        let tail = if elsewhere.is_empty() { String::new() } else { format!(" ({})", elsewhere.join(", ")) };
        (format!("Group {} · no other mic yet{tail}", group_name(mine)), true)
    } else {
        (format!("Group {} · with {}", group_name(mine), names.join(", ")), false)
    }
}

/// The phase row (`refreshAlign`): what was measured, in words, and whether Line Up has anything
/// to use. `report`: the module's `Analysis::report` (`None` before it has played). Offsets in
/// samples at `sr`, the unit a person nudges a track by.
pub fn phase_status(ins: &Insert, report: Option<[f32; REPORT_LEN]>, sr: f64) -> (String, bool) {
    let sr = if sr > 0.0 { sr } else { 48_000.0 };
    let rep = report.filter(|r| r[sh::REPORT_MARK] == sh::REPORT_VALID);
    let align_to = choice(ins, sd::P_ALIGN_TO);
    let align_on = value(ins, sd::P_ALIGN) > 0.5;
    let applied = shale_align::knob_to_offset_ms(f64::from(value(ins, sd::P_ALIGN_OFFSET))) * 0.001 * sr;
    let flip_applied = value(ins, sd::P_ALIGN_FLIP) > 0.5;
    let state = rep.map_or(sh::AL_LINK_OFF, |r| r[sh::REPORT_STATE] as i32);
    let ref_name = rep.map_or("?", |r| mic_name(r[sh::REPORT_REF] as i32));
    let hits = rep.map_or(0, |r| r[sh::REPORT_HITS].max(0.0) as usize);
    let text = match state {
        sh::AL_UNAVAILABLE => "Phase: the shared audio is unavailable".to_string(),
        sh::AL_IS_REFERENCE => "The phase reference: the other mics in this group line up to this one".to_string(),
        sh::AL_NO_REFERENCE if align_to > 0 => {
            format!("Phase: no {} with Kit Link on", sc::ALIGN_TOS.get(align_to as usize).copied().unwrap_or("?"))
        }
        sh::AL_NO_REFERENCE => "Phase: no reference · pick Align To, or Phase Reference on a mic in this group".to_string(),
        sh::AL_NO_TRANSPORT => "Phase: start the transport · it needs the host's sample positions".to_string(),
        sh::AL_LISTENING => format!("Phase: listening against {ref_name} · play some hits"),
        sh::AL_MEASURED => {
            let r = rep.unwrap_or_default();
            let measured = f64::from(r[sh::REPORT_OFFSET_MS]) * 0.001 * sr;
            let m = measured.round() as i64;
            let mut s = format!("vs {ref_name}: {}{m} smp", if m > 0 { "+" } else { "" });
            if r[sh::REPORT_POLARITY] < 0.0 {
                s.push_str(", flip");
            }
            let spread = f64::from(r[sh::REPORT_SPREAD_MS]) * 0.001 * sr;
            s.push_str(&format!(" ({hits} {}, ±{spread:.1})", if hits == 1 { "hit" } else { "hits" }));
            if align_on {
                let lined = (applied - measured).abs() < 1.5 && flip_applied == (r[sh::REPORT_POLARITY] < 0.0);
                s.push_str(if lined { " · lined up" } else { " · set by hand" });
            }
            s
        }
        _ => "Phase: switch Kit Link on to line this mic up with the others".to_string(),
    };
    (text, state == sh::AL_MEASURED && hits >= sh::LINE_UP_HITS)
}

/// Whether the phase row offers Line Up: a Phase Reference only measures when pointed at a mic.
pub fn measures(ins: &Insert) -> bool {
    value(ins, sd::P_PHASE_REF) < 0.5 || choice(ins, sd::P_ALIGN_TO) > 0
}

/// `applyAlignment`: Align Offset, Align Polarity and Align from a measured report, as
/// `mix.module_param` values; `None` when there is not enough to go on yet.
pub fn line_up_values(report: [f32; REPORT_LEN]) -> Option<[(&'static str, f32); 3]> {
    let ok = report[sh::REPORT_MARK] == sh::REPORT_VALID
        && report[sh::REPORT_STATE] as i32 == sh::AL_MEASURED
        && report[sh::REPORT_HITS] >= sh::LINE_UP_HITS as f32
        && report[sh::REPORT_OFFSET_MS].is_finite();
    ok.then(|| {
        [
            ("alignOffset", shale_align::offset_ms_to_knob(f64::from(report[sh::REPORT_OFFSET_MS])) as f32),
            ("alignFlip", if report[sh::REPORT_POLARITY] < 0.0 { 1.0 } else { 0.0 }),
            ("align", 1.0),
        ]
    })
}

/// Line Up's click (`applyAlignment`): Align Offset, Align Polarity and Align from the report, as
/// one undo step ("nothing to use yet" when the report has too few hits).
pub fn line_up(
    engine: &mut soundcraft_engine::Engine,
    track: TrackId,
    module: soundcraft_model::ModuleId,
    report: [f32; REPORT_LEN],
) -> Result<(), String> {
    let values = line_up_values(report).ok_or_else(|| "Line Up: nothing measured to use yet".to_string())?;
    let key = format!("shale-lineup:{}:{}", track.0, module.0);
    engine.end_merge();
    let mut result = Ok(());
    for (param, v) in values {
        let cmd = json!({"track": track.0, "module": module.0, "param": param, "value": v});
        if let Err(e) = engine.execute_merged("mix.module_param", &cmd, &key) {
            result = Err(e.to_string());
        }
    }
    engine.end_merge();
    result
}

/// The Kit Link tab's own rows, above its controls: the link strip's status (with the hits this
/// mic handed to another, readout `[7]`) and the phase row (its status and Line Up, as one undo
/// step). Returns the y under them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn link_controls(
    app: &mut SoundApp,
    ui: &mut Ui,
    track: TrackId,
    ins: &Insert,
    rect: Rect,
    accent: Color32,
    report: Option<[f32; REPORT_LEN]>,
    live: Option<&LiveValues>,
) -> f32 {
    let t = Tokens::current();
    let p = ui.painter().clone();
    let galley = link_status_galley(app, ui, &p, track, ins, rect.width().max(40.0), report, live);
    let row1 = Rect::from_min_size(rect.min, vec2(rect.width(), galley.size().y.max(16.0)));
    p.galley(pos2(row1.min.x, row1.center().y - galley.size().y * 0.5), galley, t.text_dim);
    let row2 = Rect::from_min_size(pos2(rect.min.x, row1.max.y + 4.0), vec2(rect.width(), 20.0));
    let sr = app.engine.session().sample_rate.as_f64();
    let (phase, can) = phase_status(ins, report, sr);
    let mut x = row2.min.x;
    if measures(ins) {
        let r = Rect::from_min_size(row2.min, vec2(64.0, row2.height()));
        let id = ui.id().with(("shale-lineup", track.0, ins.id.0));
        let resp = ui.interact(r, id, if can { Sense::click() } else { Sense::hover() });
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, can, "Line Up"));
        crate::module_drawer::pill(&p, r, "Line Up", false, can && resp.hovered(), accent);
        if !can {
            // the editor's 0.45 alpha for a Line Up with nothing to use yet
            p.rect_filled(r, CornerRadius::same(3), t.panel_bg.gamma_multiply(0.55));
        }
        let resp = resp.on_hover_text("Take the measured offset and polarity, and switch Align on. Play a few hits first: it needs at least four.");
        if resp.clicked()
            && let Some(r) = report
            && let Err(e) = line_up(&mut app.engine, track, ins.id, r)
        {
            app.ui.status = e;
        }
        x = r.max.x + 8.0;
    }
    // wrapped onto a second line beside Line Up when it is long
    let galley = p.layout(phase, regular(9.5), t.text_dim, (rect.max.x - x).max(40.0));
    let h = galley.size().y;
    p.galley(pos2(x, row2.center().y - h.min(row2.height() + 12.0) * 0.5), galley, t.text_dim);
    row2.max.y.max(row2.center().y + h * 0.5)
}

/// What the link strip's status galley was laid out from: it is rebuilt only when one changes.
#[derive(Clone, Copy, PartialEq)]
struct LinkStatusKey {
    revision: u64,
    no_slot: bool,
    handed: i64,
    width: u32,
    colors: (Color32, Color32),
    /// The DPI it was laid out at (a galley holds glyphs for one scale).
    pixels_per_point: u32,
}

/// The link strip's status ([`link_status`], with the hits this mic handed to another), laid out:
/// cached per module and rebuilt only when the session's revision, the slot, the handed-over
/// count, the width, the theme or the DPI changes, so a frame that changes nothing allocates nothing.
#[allow(clippy::too_many_arguments)]
fn link_status_galley(
    app: &SoundApp,
    ui: &Ui,
    p: &egui::Painter,
    track: TrackId,
    ins: &Insert,
    width: f32,
    report: Option<[f32; REPORT_LEN]>,
    live: Option<&LiveValues>,
) -> std::sync::Arc<egui::Galley> {
    let t = Tokens::current();
    let no_slot = report.is_some_and(|r| r[sh::REPORT_MARK] == sh::REPORT_VALID && r[sh::REPORT_NO_SLOT] > 0.5);
    let handed = live.map(|l| l.readouts[7]).filter(|n| n.is_finite() && *n >= 1.0).map_or(0, |n| n as i64);
    let key = LinkStatusKey {
        revision: app.engine.revision,
        no_slot,
        handed,
        width: width.to_bits(),
        colors: (t.meter_yellow, t.text_dim),
        pixels_per_point: p.pixels_per_point().to_bits(),
    };
    let id = Id::new(("shale-link-status", track.0, ins.id.0));
    if let Some((k, g)) = ui.data(|d| d.get_temp::<(LinkStatusKey, std::sync::Arc<egui::Galley>)>(id))
        && k == key
    {
        return g;
    }
    let (mut status, warn) = link_status(app.engine.session(), track, ins, no_slot);
    if handed > 0 {
        status.push_str(&format!(" · {} handed to another mic", if handed == 1 { "1 hit".to_string() } else { format!("{handed} hits") }));
    }
    let color = if warn { t.meter_yellow } else { t.text_dim };
    let galley = p.layout(status, regular(9.5), color, width);
    ui.data_mut(|d| d.insert_temp(id, (key, std::sync::Arc::clone(&galley))));
    galley
}

/// The panel's text for a knob (`formatParam`): Length and Hold through the selected Mic's
/// table, Crossover and the detector band through the Mic while Auto Band is on.
pub fn value_text(ins: &Insert, info: &ParamInfo, v: f32) -> String {
    match SHALE_PARAMS.iter().position(|p| p.id == info.id) {
        Some(index) => sc::format_param(index, info.clamp(v), &context(ins)).unwrap_or_else(|| info.format(v)),
        None => info.format(v),
    }
}

/// `norm`: -66..0 dB across the half height (the threshold knob's own range).
pub fn norm(db: f32) -> f32 {
    if db.is_finite() { ((db + 66.0) / 66.0).clamp(0.0, 1.0) } else { 0.0 }
}

/// `toDb`.
pub fn to_db(lin: f32) -> f32 {
    20.0 * (lin.max(0.0) + 1.0e-7).log10()
}

/// The display's columns from the newest `shown` bins (fewer kept: the older ones are the C++
/// ring's empty bins), merged as `paint` does: per column the loudest peak, the most-closed gate
/// and the loudest detector, a column no bin lands in carrying its left neighbour. Written into
/// `out` (its length is the column count); no allocation.
pub fn columns(bins: &VecDeque<[f32; SCOPE_VALUES]>, shown: usize, out: &mut [[f32; SCOPE_VALUES]]) {
    let cols = out.len();
    if cols == 0 {
        return;
    }
    let empty = [0.0, 1.0, 0.0];
    out.fill(empty);
    let mut filled = [false; MAX_COLS];
    let len = bins.len();
    for i in 0..shown {
        let b = (len + i).checked_sub(shown).and_then(|k| bins.get(k)).copied().unwrap_or(empty);
        let c = ((i as f32 / shown as f32 * cols as f32) as usize).min(cols - 1);
        let col = &mut out[c];
        col[0] = col[0].max(sane(b[0]));
        col[1] = col[1].min(sane_gain(b[1]));
        col[2] = col[2].max(sane(b[2]));
        if let Some(f) = filled.get_mut(c) {
            *f = true;
        }
    }
    for c in 1..cols.min(MAX_COLS) {
        if !filled[c] {
            out[c] = out[c - 1];
        }
    }
}

fn sane(x: f32) -> f32 {
    if x.is_finite() { x.max(0.0) } else { 0.0 }
}

fn sane_gain(x: f32) -> f32 {
    if x.is_finite() { x.clamp(0.0, 1.0) } else { 1.0 }
}

/// Where the display draws: its body, the centre line and the half height (`paint`'s `mid` and
/// `half`).
pub fn geometry(rect: Rect, large: bool) -> (Rect, f32, f32) {
    let pad = if large { 4.0 } else { 2.0 };
    let body = rect.shrink(pad);
    let half = (body.height() * 0.5 - if large { 3.0 } else { 1.0 }).max(0.0);
    (body, body.center().y, half)
}

/// The drawer's span for module `module` (seconds), as the editor keeps it.
pub fn span(ctx: &egui::Context, module: Id) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(module.with("shale-span"))).filter(|s| SPANS.contains(s)).unwrap_or(5.0)
}

/// The span button's rect in a drawer graph.
fn span_button(rect: Rect) -> Rect {
    Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(46.0, 15.0))
}

/// The Shale graph.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &egui::Ui,
    p: &egui::Painter,
    rect: Rect,
    ins: &Insert,
    module: Id,
    live: Option<&LiveValues>,
    bins: Option<&VecDeque<[f32; SCOPE_VALUES]>>,
    large: bool,
) {
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let (body, mid, half) = geometry(rect, large);
    if !(body.width() >= 4.0 && body.height() >= 4.0 && half > 0.5) {
        return;
    }
    let seconds = if large { span(ui.ctx(), module) } else { TILE_SPAN };
    let shown = ((seconds * BINS_PER_SECOND).round() as usize).clamp(10, SCOPE_BINS);

    // ---- grid, every 12 dB, mirrored
    for db in [-12.0f32, -24.0, -36.0, -48.0, -60.0] {
        let h = half * norm(db);
        for y in [mid - h, mid + h] {
            p.line_segment([pos2(body.left(), y), pos2(body.right(), y)], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.6)));
        }
    }
    // ---- a faint line every second, counted back from now at the right edge
    let mut s = 1.0f32;
    while s < seconds {
        let x = body.left() + body.width() * (1.0 - s / seconds);
        p.line_segment([pos2(x, body.top() + 2.0), pos2(x, body.bottom() - 2.0)], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.45)));
        s += 1.0;
    }

    // ---- the columns
    let cols = (body.width().round() as usize).clamp(1, MAX_COLS);
    let mut buf = [[0.0f32; SCOPE_VALUES]; MAX_COLS];
    let out = &mut buf[..cols];
    let empty = VecDeque::new();
    columns(bins.unwrap_or(&empty), shown, out);
    let dx = body.width() / cols as f32;
    let dim = t.text_dim.gamma_multiply(0.45);
    for (c, b) in out.iter().enumerate() {
        let x = body.left() + c as f32 * dx;
        let in_h = half * norm(to_db(b[0]));
        if in_h <= 0.0 {
            continue;
        }
        p.rect_filled(Rect::from_min_size(pos2(x, mid - in_h), vec2(dx.max(1.0), in_h * 2.0)), CornerRadius::ZERO, dim);
        let out_h = half * norm(to_db(b[0] * b[1]));
        if out_h > 0.0 {
            p.rect_filled(Rect::from_min_size(pos2(x, mid - out_h), vec2(dx.max(1.0), out_h * 2.0)), CornerRadius::ZERO, accent.gamma_multiply(0.92));
        }
    }
    // ---- the detector, as a line: what the threshold is actually compared against
    let dark = ui.visuals().dark_mode;
    // The editor's light blue; a deeper blue on the light palette, where the light one vanishes.
    let det_color = if dark { Color32::from_rgb(0x7F, 0xD4, 0xFF).gamma_multiply(0.55) } else { Color32::from_rgb(0x1F, 0x78, 0xB4) };
    // Segment by segment: no point list is made per frame.
    let det_at = |c: usize, b: &[f32; SCOPE_VALUES]| pos2(body.left() + c as f32 * dx, mid - half * norm(to_db(b[2])));
    for (c, w) in out.windows(2).enumerate() {
        p.line_segment([det_at(c, &w[0]), det_at(c + 1, &w[1])], Stroke::new(1.0, det_color));
    }

    // ---- the threshold line (orange), mirrored below
    let t_db = sd::knob_to_thresh_db(value(ins, sd::P_THRESHOLD));
    let ty = mid - half * norm(t_db);
    // The editor's orange; deeper on the light palette so the line and its tag read.
    let orange = if dark { Color32::from_rgb(0xFF, 0xB4, 0x54) } else { Color32::from_rgb(0xC8, 0x6A, 0x10) };
    p.line_segment([pos2(body.left(), ty), pos2(body.right(), ty)], Stroke::new(if large { 1.4 } else { 1.0 }, orange.gamma_multiply(0.85)));
    p.line_segment([pos2(body.left(), mid + (mid - ty)), pos2(body.right(), mid + (mid - ty))], Stroke::new(0.7, orange.gamma_multiply(0.6)));

    // ---- a strip along the bottom showing where the gate was open
    let sy = body.bottom() - if large { 4.0 } else { 2.0 };
    for (c, b) in out.iter().enumerate() {
        if b[1] >= 0.5 {
            p.rect_filled(
                Rect::from_min_size(pos2(body.left() + c as f32 * dx, sy), vec2(dx.max(1.0), if large { 3.0 } else { 2.0 })),
                0.0,
                accent.gamma_multiply(0.35),
            );
        }
    }

    // The trigger lamp: a hit within the last few frames.
    let lamp = lamp_on(ui.ctx(), module, live);
    if large {
        // The threshold's tag at the right, the span button and the Mic at the left.
        let tag = Rect::from_min_size(pos2(body.right() - 62.0, ty - 8.0), vec2(56.0, 16.0));
        p.rect_filled(tag, CornerRadius::same(3), t.graph_bg);
        p.rect_stroke(tag, CornerRadius::same(3), Stroke::new(1.0, orange.gamma_multiply(0.5)), egui::StrokeKind::Inside);
        p.text(tag.center(), Align2::CENTER_CENTER, format!("{t_db:.1} dB"), bold(9.0), orange);
        let b = span_button(rect);
        p.rect_filled(b, CornerRadius::same(3), t.graph_bg);
        p.rect_stroke(b, CornerRadius::same(3), Stroke::new(1.0, t.graph_grid), egui::StrokeKind::Inside);
        let text = if seconds < 3.0 { "2.5 sec".to_string() } else { format!("{} sec", seconds.round() as i32) };
        p.text(b.center(), Align2::CENTER_CENTER, text, bold(8.5), t.text_dim);
        let mic = sc::INSTRUMENTS.get(value(ins, sd::P_INSTRUMENT).round().max(0.0) as usize).copied().unwrap_or("");
        let x = b.right() + 14.0;
        p.text(pos2(x, b.center().y), Align2::LEFT_CENTER, mic.to_uppercase(), bold(8.5), t.text);
        // The live numbers worth seeing (the editor's footer): dB out of the highs.
        let cut = live.map_or(f32::NAN, |l| l.readouts[3]);
        if value(ins, sd::P_DEBLEED) > 0.0005 && cut.is_finite() && cut > 0.2 {
            p.text(pos2(body.right() - 4.0, b.center().y), Align2::RIGHT_CENTER, format!("{cut:.1} dB out of the highs"), regular(8.5), t.text_dim);
        }
        if lamp {
            p.circle_filled(pos2(x + mic_width(p, mic) + 8.0, b.center().y), 3.0, accent);
        }
    } else if lamp && rect.height() >= 20.0 {
        p.circle_filled(pos2(body.right() - 4.0, body.top() + 4.0), 2.5, accent);
    }
}

fn mic_width(p: &egui::Painter, mic: &str) -> f32 {
    p.layout_no_wrap(mic.to_uppercase(), bold(8.5), Color32::WHITE).size().x
}

/// The editor's trigger lamp (`flashTicks`, 6 frames at 30 Hz): lit for 0.2 s after the trigger
/// count readout moves. Kept per module in the context; a missing reading leaves it alone.
fn lamp_on(ctx: &egui::Context, module: Id, live: Option<&LiveValues>) -> bool {
    let key = module.with("shale-lamp");
    let now = ctx.input(|i| i.time);
    let (mut last_count, mut lit_until): (f32, f64) = ctx.data(|d| d.get_temp(key)).unwrap_or((f32::NAN, f64::NEG_INFINITY));
    if let Some(n) = live.map(|l| l.readouts[4]).filter(|n| n.is_finite()) {
        if last_count.is_finite() && n != last_count {
            lit_until = now + 0.2;
        }
        last_count = n;
    }
    ctx.data_mut(|d| d.insert_temp(key, (last_count, lit_until)));
    let on = now < lit_until;
    if on {
        ctx.request_repaint();
    }
    on
}

/// The drawer graph's own controls: the span button (2.5 → 5 → 10 seconds) and the threshold
/// line, dragged above or below the centre (the display is mirrored), as one undo step.
pub(crate) fn drawer_controls(app: &mut SoundApp, ui: &Ui, track: TrackId, ins: &Insert, rect: Rect, module: Id) {
    let b = span_button(rect);
    let span_resp = ui.interact(b, ui.id().with(("shale-span", ins.id.0)), Sense::click());
    if span_resp.clicked() {
        let s = span(ui.ctx(), module);
        let next = if s < 3.0 {
            5.0
        } else if s < 7.0 {
            10.0
        } else {
            2.5
        };
        ui.ctx().data_mut(|d| d.insert_temp(module.with("shale-span"), next));
    }
    let (body, mid, half) = geometry(rect, true);
    let area = ui.interact(body, ui.id().with(("shale-threshold", ins.id.0)), Sense::drag());
    let busy = ui.id().with(("shale-threshold-busy", ins.id.0));
    if area.dragged() && !span_resp.hovered() {
        if let Some(pos) = area.interact_pointer_pos()
            && half > 1.0
        {
            let v = threshold_at(pos.y, mid, half);
            let key = format!("modparam:{}:{}:threshold", track.0, ins.id.0);
            let cmd = json!({"track": track.0, "module": ins.id.0, "param": "threshold", "value": v});
            if let Err(e) = app.engine.execute_merged("mix.module_param", &cmd, &key) {
                app.ui.status = e.to_string();
            }
            ui.data_mut(|d| d.insert_temp(busy, true));
        }
    } else if ui.data(|d| d.get_temp::<bool>(busy)).unwrap_or(false) {
        ui.data_mut(|d| d.remove::<bool>(busy));
        app.engine.end_merge();
    }
    if area.hovered() && !span_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
}

/// `dragTo`: the threshold knob value at a pointer height (either side of the centre line).
pub fn threshold_at(y: f32, mid: f32, half: f32) -> f32 {
    if half.is_nan() || half <= 1.0 || !y.is_finite() {
        return 0.0;
    }
    ((y - mid).abs() / half).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_merge_as_the_cpp_display() {
        let bins: VecDeque<[f32; 3]> = [[0.1, 1.0, 0.2], [0.5, 0.2, 0.1], [0.3, 0.9, 0.4], [0.0, 1.0, 0.0]].into_iter().collect();
        let mut out = [[0.0f32; 3]; 2];
        columns(&bins, 4, &mut out);
        assert_eq!(out, [[0.5, 0.2, 0.2], [0.3, 0.9, 0.4]]);
        // More columns than bins: each bin carried across.
        let mut wide = [[0.0f32; 3]; 8];
        columns(&bins, 4, &mut wide);
        assert_eq!(wide[1], wide[0]);
        assert_eq!(wide[7], [0.0, 1.0, 0.0]);
        // Fewer bins kept than shown: the older ones are empty bins (open gate, no level).
        let mut out = [[0.0f32; 3]; 4];
        columns(&bins, 8, &mut out);
        assert_eq!(out[0], [0.0, 1.0, 0.0]);
        assert_eq!(out[3], [0.3, 0.9, 0.4]);
        // Hostile bins never escape their ranges.
        let bad: VecDeque<[f32; 3]> = [[f32::NAN, f32::INFINITY, -1.0]].into_iter().collect();
        columns(&bad, 1, &mut out[..1]);
        assert_eq!(out[0], [0.0, 1.0, 0.0]);
        columns(&bad, 1, &mut []);
    }

    #[test]
    fn the_scale_is_the_threshold_knobs() {
        assert_eq!(norm(0.0), 1.0);
        assert_eq!(norm(-66.0), 0.0);
        assert_eq!(norm(-99.0), 0.0);
        assert_eq!(norm(f32::NAN), 0.0);
        assert!((norm(-33.0) - 0.5).abs() < 1e-6);
        // Dragging to a height gives the knob whose line sits there, either side of the centre.
        for v in [0.0f32, 0.3, 0.55, 1.0] {
            let h = 100.0 * norm(sd::knob_to_thresh_db(v));
            assert!((threshold_at(50.0 - h, 50.0, 100.0) - v).abs() < 1e-5, "{v}");
            assert!((threshold_at(50.0 + h, 50.0, 100.0) - v).abs() < 1e-5, "{v}");
        }
        assert_eq!(threshold_at(f32::NAN, 50.0, 100.0), 0.0);
        assert_eq!(threshold_at(10.0, 50.0, 0.0), 0.0);
    }

    #[test]
    fn drawer_rules_hide_listen_show_midi_and_lock_auto_band() {
        let mut ins = Insert::new("fs_shale");
        for id in INERT_PARAMS {
            assert_eq!(switch(&ins, id), Some(Switch::Hide), "{id}");
        }
        assert_eq!(INERT_PARAMS, ["scListen"]);
        // MIDI Out and MIDI Note drive a module MIDI input now: shown.
        assert_eq!(switch(&ins, "midiOut"), None);
        assert_eq!(switch(&ins, "midiNote"), None);
        assert_eq!(crate::module_drawer::param_tip(&ins, "midiOut").as_deref(), Some(MIDI_OUT_TIP));
        assert!(crate::module_drawer::param_tip(&ins, "threshold").is_none());
        assert_eq!(switch(&ins, "phaseRef"), Some(Switch::Show { label: "Phase Ref", opts: &[] }));
        for id in ["link", "linkGroup", "linkRole", "reject", "alignTo", "align", "alignOffset", "alignFlip", "scSource", "threshold", "instrument"] {
            assert_eq!(switch(&ins, id), None, "{id}");
        }
        // Kit Link's own controls are inactive while it is off.
        for id in ["linkGroup", "linkRole", "reject", "phaseRef", "alignTo"] {
            assert!(locked(&ins, id), "{id}");
        }
        assert!(!locked(&ins, "link") && !locked(&ins, "align"));
        ins.params.insert("link".into(), 1.0);
        assert!(!locked(&ins, "linkRole") && !locked(&ins, "alignTo"));
        ins.params.remove("link");
        assert!(locked(&ins, "xover") && locked(&ins, "detLow") && locked(&ins, "detHigh"));
        assert!(!locked(&ins, "threshold"));
        ins.params.insert("autoBand".into(), 0.0);
        assert!(!locked(&ins, "xover"));
        // Value texts follow the Mic and Auto Band, as the panel's.
        let info = |id: &str| SHALE_PARAMS.iter().find(|p| p.id == id).unwrap();
        assert_eq!(value_text(&ins, info("xover"), 1.0), "15.0 k");
        ins.params.insert("autoBand".into(), 1.0);
        ins.params.insert("instrument".into(), 0.0);
        assert_eq!(value_text(&ins, info("xover"), 1.0), "300 Hz");
        assert_eq!(value_text(&ins, info("length"), 0.0), "-8.0 dB");
        assert_eq!(value_text(&ins, info("hold"), 0.0), "14 ms");
        assert_eq!(value_text(&ins, info("instrument"), 3.0), "Snare Top");
    }

    fn shale(mic: f32, link: f32, group: f32, host: Option<u32>) -> Insert {
        let mut ins = Insert::new("fs_shale");
        ins.params.insert("instrument".into(), mic);
        ins.params.insert("link".into(), link);
        ins.params.insert("linkGroup".into(), group);
        ins.link_group = host;
        ins
    }

    #[test]
    fn line_up_is_one_undo_step() {
        use soundcraft_dsp::plugins::fs::shale::AlignReport;
        let mut s = Session::default();
        let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, Some("Snare"));
        let m = s.track_mut(t).unwrap().mixer.add_module(shale(3.0, 1.0, 0.0, Some(1)), None).unwrap();
        let mut engine = soundcraft_engine::Engine::new(s);
        let param = |e: &soundcraft_engine::Engine, id: &str| e.session().track(t).unwrap().mixer.module(m).unwrap().params.get(id).copied();
        let before = (param(&engine, "alignOffset"), param(&engine, "alignFlip"), param(&engine, "align"));
        let undo_before = engine.undo_history().len();
        let measured = AlignReport { state: sh::AL_MEASURED, ref_instrument: 6, hits: 6, offset_ms: 2.0, spread_ms: 0.01, corr: 0.9, polarity: -1 };
        // a gesture left open elsewhere must not swallow it
        engine.execute_merged("mix.module_param", &json!({"track": t.0, "module": m.0, "param": "reject", "value": 0.0}), "other").unwrap();
        let undo_before = undo_before + 1;
        line_up(&mut engine, t, m, measured.to_values()).unwrap();
        assert_eq!(engine.undo_history().len(), undo_before + 1, "{:?}", engine.undo_history());
        assert_eq!(param(&engine, "alignFlip"), Some(1.0));
        assert_eq!(param(&engine, "align"), Some(1.0));
        let knob = param(&engine, "alignOffset").unwrap();
        assert!((f64::from(knob) - shale_align::offset_ms_to_knob(2.0)).abs() < 1e-6);
        // a second Line Up is its own step too
        line_up(&mut engine, t, m, measured.to_values()).unwrap();
        assert_eq!(engine.undo_history().len(), undo_before + 2);
        assert!(engine.undo());
        assert!(engine.undo());
        assert_eq!((param(&engine, "alignOffset"), param(&engine, "alignFlip"), param(&engine, "align")), before, "one undo takes all three back");
        assert!(line_up(&mut engine, t, m, AlignReport { hits: 2, ..measured }.to_values()).is_err());
    }

    #[test]
    fn the_link_status_galley_is_laid_out_again_when_the_dpi_changes() {
        let app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let ins = shale(3.0, 1.0, 0.0, Some(1));
        let ctx = egui::Context::default();
        let mut seen = Vec::new();
        for ppp in [1.0f32, 2.0] {
            ctx.set_pixels_per_point(ppp);
            for _ in 0..2 {
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    let p = ui.painter().clone();
                    let g = link_status_galley(&app, ui, &p, TrackId(1), &ins, 200.0, None, None);
                    seen.push((ui.ctx().pixels_per_point(), g.pixels_per_point));
                });
                out.textures_delta.clear();
            }
        }
        assert!(seen.iter().all(|(want, got)| want == got), "{seen:?}");
        assert!(seen.iter().any(|(want, _)| *want == 2.0), "{seen:?}");
    }

    #[test]
    fn the_link_strip_names_the_other_mics_or_why_there_are_none() {
        let mut s = Session::default();
        let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, Some("Snare"));
        let u = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, Some("Kick"));
        let me = s.track_mut(t).unwrap().mixer.add_module(shale(3.0, 1.0, 0.0, Some(1)), None).unwrap();
        let mine = |s: &Session| s.track(t).unwrap().mixer.module(me).unwrap().clone();
        assert_eq!(link_status(&s, t, &mine(&s), false), ("Group A · no other mic yet".into(), true));
        let kick = s.track_mut(u).unwrap().mixer.add_module(shale(0.0, 1.0, 1.0, Some(1)), None).unwrap();
        assert_eq!(link_status(&s, t, &mine(&s), false).0, "Group A · no other mic yet (Kick In is in B)");
        s.track_mut(u).unwrap().mixer.module_mut(kick).unwrap().params.insert("linkGroup".into(), 0.0);
        assert_eq!(link_status(&s, t, &mine(&s), false), ("Group A · with Kick In".into(), false));
        // only a member whose report says it got no slot hears that the slots are full
        assert_eq!(link_status(&s, t, &mine(&s), true), ("Kit Link unavailable · all 16 link slots are in use".into(), true));
        // a member with Kit Link off is not live
        s.track_mut(u).unwrap().mixer.module_mut(kick).unwrap().params.insert("link".into(), 0.0);
        assert!(link_status(&s, t, &mine(&s), false).0.contains("no other mic yet"));
        // another plugin in the link group; no link group; Kit Link off
        s.track_mut(u)
            .unwrap()
            .mixer
            .add_module(
                {
                    let mut i = Insert::new("fs_levelup");
                    i.link_group = Some(1);
                    i
                },
                None,
            )
            .unwrap();
        assert!(link_status(&s, t, &mine(&s), false).0.contains("holds other plugins"));
        assert!(link_status(&s, t, &shale(3.0, 1.0, 0.0, None), false).0.contains("in no link group"));
        assert_eq!(link_status(&s, t, &shale(3.0, 0.0, 0.0, Some(1)), false), ("Kit Link off · this gate decides on its own".into(), false));
    }

    #[test]
    fn the_phase_row_reads_the_report_and_line_up_takes_it() {
        use soundcraft_dsp::plugins::fs::shale::{AlignReport, LINE_UP_HITS};
        let ins = shale(3.0, 1.0, 0.0, Some(1));
        assert_eq!(phase_status(&ins, None, 48_000.0).0, "Phase: switch Kit Link on to line this mic up with the others");
        let measured = AlignReport {
            state: sh::AL_MEASURED,
            ref_instrument: 9,
            hits: 6,
            offset_ms: 100.0 / 48.0,
            spread_ms: 0.4 / 48.0,
            corr: 0.9,
            polarity: -1,
        };
        let (text, can) = phase_status(&ins, Some(measured.to_values()), 48_000.0);
        assert_eq!(text, "vs Overheads: +100 smp, flip (6 hits, ±0.4)");
        assert!(can);
        let values = line_up_values(measured.to_values()).unwrap();
        assert_eq!(values[1], ("alignFlip", 1.0));
        assert_eq!(values[2], ("align", 1.0));
        assert!((values[0].1 - (0.5 + 100.0 / 48.0 / 20.0) as f32).abs() < 1e-6);
        // Lined up once those values are on the module; "set by hand" otherwise.
        let mut lined = ins.clone();
        for (id, v) in values {
            lined.params.insert(id.into(), v);
        }
        assert!(phase_status(&lined, Some(measured.to_values()), 48_000.0).0.ends_with("· lined up"));
        lined.params.insert("alignFlip".into(), 0.0);
        assert!(phase_status(&lined, Some(measured.to_values()), 48_000.0).0.ends_with("· set by hand"));
        // Not enough hits, other states, garbage: no Line Up.
        let few = AlignReport { hits: LINE_UP_HITS - 1, ..measured };
        assert!(!phase_status(&ins, Some(few.to_values()), 48_000.0).1);
        assert!(line_up_values(few.to_values()).is_none());
        let listening = AlignReport { state: sh::AL_LISTENING, ..measured };
        assert_eq!(phase_status(&ins, Some(listening.to_values()), 48_000.0), ("Phase: listening against Overheads · play some hits".into(), false));
        assert!(line_up_values([f32::NAN; REPORT_LEN]).is_none() && line_up_values([0.0; REPORT_LEN]).is_none());
        let none = AlignReport { state: sh::AL_NO_REFERENCE, ..measured };
        assert!(phase_status(&ins, Some(none.to_values()), 48_000.0).0.starts_with("Phase: no reference"));
        // A Phase Reference offers no Line Up unless it is pointed at a mic of its own.
        let mut r = ins.clone();
        r.params.insert("phaseRef".into(), 1.0);
        assert!(!measures(&r));
        r.params.insert("alignTo".into(), 4.0);
        assert!(measures(&r));
        assert_eq!(
            phase_status(&r, Some(AlignReport { state: sh::AL_NO_REFERENCE, ..measured }.to_values()), 48_000.0).0,
            "Phase: no Snare Top with Kit Link on"
        );
    }
}

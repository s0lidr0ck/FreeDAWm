//! Mini graphs for channel-chain module tiles: EQ curve, compressor transfer curve, level bars,
//! and each FireStation engine's own graph (CleanQ's curve over its spectra, Comp's transfer
//! curve and lane meters, Sat/Tape's levels and drive, Vint's frequency response, Twine's scope
//! against its ceiling).

use crate::curve_worker::CurveWorker;
use crate::theme::{Tokens, bold, module_accent, regular};
use egui::{Align2, Color32, CornerRadius, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use soundcraft_dsp::analysis::{Analysis, AnalysisRing, RING_LEN};
use soundcraft_dsp::plugins::fs::cleanq::{self as cq, CLEANQ_PARAMS};
use soundcraft_dsp::plugins::fs::vint::{self, VINT_PARAMS};
use soundcraft_model::Insert;
use std::collections::VecDeque;
use std::sync::Arc;

/// Live readings for a module, adapted from the real meter by the caller.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiveValues {
    /// Linear peaks.
    pub in_peak: f32,
    pub out_peak: f32,
    /// Gain reduction in dB (positive = reducing).
    pub gr_db: f32,
    /// Module-specific readouts; for the compressor `[0]` is the detector input level in dBFS.
    /// NaN: no reading (the module did not run, or its plugin writes nothing there).
    pub readouts: [f32; 8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphKind {
    EqCurve,
    Compressor,
    Level,
    /// FireStation CleanQ: its curve with band dots over the pre/post spectra.
    CleanQ,
    /// FireStation Comp: lane A's transfer curve, the live input dot, the three lanes' GR.
    Comp,
    /// FireStation Sat and Tape: in/out levels and the drive.
    Console,
    /// FireStation Vint: the module's measured frequency response (cached, recomputed on a budget).
    Vint,
    /// FireStation Twine: a scrolling scope of its input against the ceiling, clip and limit GR.
    Twine,
    /// FireStation Pitch: the voice mix (each voice the style uses, and the dry) as level bars,
    /// with the input level, the OC-2 lock lamp and the Whammy's interval.
    Pitch,
    /// FireStation Mod: one cycle of the LFO at its depth, with the live phase of both sides.
    Mod,
    /// FireStation Dist: the circuit's name, in/out levels, and the drive/level arcs.
    Dist,
    /// FireStation Transient: the C++ editor's scope, the last 2.5 s of input and output peaks
    /// over a lane of the attack and sustain gain applied, with the live gains beside it.
    Transient,
    /// FireStation Filter: the circuit's response shape at the live (modulated) cutoff, the knob's
    /// own position, and the step row when Steps is the source.
    Filter,
    /// FireStation Delay: an impulse's repeats on a time axis (heads, pattern taps, ping-pong
    /// sides) under the feedback decay envelope, from the engine's own tables and echo times.
    Delay,
    /// FireStation Reverb: the decay per band (RT60 over time from the engine's maps), the
    /// pre-delay in force, Freeze, and the in/out levels.
    Reverb,
    /// FireStation Swale: one cycle of the envelope as the settings describe it, beside the last
    /// five seconds of the engine's VCA opening and detector with a mark at every swell.
    Swale,
    /// FireStation Shale: the C++ display, the last seconds of input (dim) and gated output
    /// (bright) on a mirrored dB axis, the detector as a line and the threshold.
    Shale,
    /// FireStation ChamberMade: a top-down plan of the room from the engine's own geometry
    /// (outline, objects, speaker, microphones and their direct paths), with its RT60.
    Chamber,
    /// FireStation Amp: the amp's tone stack as its engine solves it (on the curve worker), with
    /// the in/out levels, the watts into the speaker, the sag and the cabinet in use.
    Amp,
    /// FireStation LevelUp: the editor's scope, 30 s of short-term loudness in (and out) against
    /// the target and the gate, the gain correction, the delivery meter (integrated, true peak) and
    /// the reference and pairing state, all from the engine's readouts.
    LevelUp,
    /// Manners: the gain curve it is applying right now over frequency (cuts below the line, lifts
    /// above it), with the edges of its working range.
    Manners,
    /// Sift: the last seconds of the voice going in, with what was taken off its sibilance
    /// hanging from the top and what was taken off its plosives standing on the bottom.
    Sift,
}

pub fn graph_kind(plugin_id: &str) -> GraphKind {
    match plugin_id {
        "eq_7band" | "eq_1band" => GraphKind::EqCurve,
        "compressor" | "channel_strip" => GraphKind::Compressor,
        // A FireStation engine's own graph (`EngineUi::graph`).
        _ => crate::engine_ui::engine_ui(plugin_id).map_or(GraphKind::Level, |u| u.graph),
    }
}

/// Whether a graph reads its module's sample streams (see [`Streams`]).
pub fn uses_streams(kind: GraphKind) -> bool {
    matches!(kind, GraphKind::CleanQ | GraphKind::Twine | GraphKind::Transient | GraphKind::Shale | GraphKind::Manners | GraphKind::Sift)
}

const F_MIN: f32 = 20.0;
const F_MAX: f32 = 20_000.0;
const DB_RANGE: f32 = 18.0;
const COMP_MIN_DB: f32 = -60.0;
const DOT_R: f32 = 4.5;

/// Band colours, low to high.
const BAND_COLORS: [Color32; 5] = [
    Color32::from_rgb(236, 96, 96),
    Color32::from_rgb(240, 170, 70),
    Color32::from_rgb(110, 210, 110),
    Color32::from_rgb(90, 190, 240),
    Color32::from_rgb(180, 130, 240),
];

/// (freq id, gain id) of each draggable EQ band, by plugin.
fn eq_bands(plugin: &str) -> &'static [(&'static str, &'static str)] {
    match plugin {
        "eq_7band" => &[
            ("low_shelf_freq", "low_shelf_gain"),
            ("low_mid_freq", "low_mid_gain"),
            ("mid_freq", "mid_gain"),
            ("high_mid_freq", "high_mid_gain"),
            ("high_shelf_freq", "high_shelf_gain"),
        ],
        "eq_1band" => &[("freq", "gain")],
        _ => &[],
    }
}

/// The colour of the EQ band a parameter belongs to (its dot on the curve), if any.
pub fn band_color(plugin: &str, param: &str) -> Option<Color32> {
    let group = param.rsplit_once('_').map_or(param, |(head, _)| head);
    eq_bands(plugin).iter().position(|(fid, _)| fid.rsplit_once('_').is_some_and(|(head, _)| head == group)).and_then(|i| BAND_COLORS.get(i).copied())
}

pub fn freq_to_x(rect: Rect, f: f32) -> f32 {
    let f = if f.is_finite() { f.clamp(F_MIN, F_MAX) } else { 1000.0 };
    rect.left() + rect.width() * (f / F_MIN).ln() / (F_MAX / F_MIN).ln()
}

pub fn x_to_freq(rect: Rect, x: f32) -> f32 {
    let t = if rect.width() > 0.0 && x.is_finite() { ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) } else { 0.5 };
    F_MIN * (F_MAX / F_MIN).powf(t)
}

pub fn db_to_y(rect: Rect, db: f32) -> f32 {
    let db = if db.is_finite() { db.clamp(-DB_RANGE, DB_RANGE) } else { 0.0 };
    rect.center().y - db / DB_RANGE * rect.height() * 0.5
}

pub fn y_to_db(rect: Rect, y: f32) -> f32 {
    if rect.height() <= 0.0 || !y.is_finite() {
        return 0.0;
    }
    ((rect.center().y - y) / (rect.height() * 0.5) * DB_RANGE).clamp(-DB_RANGE, DB_RANGE)
}

fn param(ins: &Insert, id: &str) -> f32 {
    let info = soundcraft_dsp::plugin_info(&ins.plugin).and_then(|i| i.param(id));
    let dflt = info.map_or(0.0, |p| p.default);
    let v = ins.params.get(id).copied().unwrap_or(dflt);
    info.map_or(v, |p| p.clamp(v))
}

/// A usable sample rate: the session's, or 48 kHz when it is missing or nonsense.
pub(crate) fn sane_sr(sample_rate: f32) -> f32 {
    if sample_rate.is_finite() && sample_rate >= 8000.0 { sample_rate } else { 48_000.0 }
}

/// The EQ curve's level (dB) at `freq`, with `extra` overriding the insert's parameters.
fn curve_db_at(ins: &Insert, freq: f32, sr: f32, extra: &[(&str, f32)]) -> f32 {
    let mut overrides: Vec<(&str, f32)> = ins.params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    overrides.extend_from_slice(extra);
    let resp = if ins.plugin == "eq_1band" {
        soundcraft_dsp::eq1_response(&overrides, &[freq], sr)
    } else {
        soundcraft_dsp::eq7_response(&overrides, &[freq], sr)
    };
    resp.first().copied().filter(|v| v.is_finite()).unwrap_or(0.0)
}

/// Where a band's dot sits: at the band frequency, on the curve. A shelf only reaches about half
/// its gain at its corner and neighbouring bands and filters add to it, so a dot drawn at the raw
/// gain would float off the curve.
fn band_dot(rect: Rect, ins: &Insert, fid: &str, sr: f32) -> Pos2 {
    let f = param(ins, fid);
    pos2(freq_to_x(rect, f), db_to_y(rect, curve_db_at(ins, f, sr, &[])))
}

/// The band gain that puts the curve at `target_db` at `freq` (the band moved there): one Newton
/// step on the curve's response to the gain, so repeated drag frames settle under the pointer.
fn gain_for_curve(ins: &Insert, (fid, gid): (&str, &str), gi: &soundcraft_dsp::ParamInfo, freq: f32, target_db: f32, sr: f32) -> f32 {
    let g0 = param(ins, gid);
    let at = |g: f32| curve_db_at(ins, freq, sr, &[(fid, freq), (gid, g)]);
    let c0 = at(g0);
    // Probe towards the middle of the range so the probe itself is never clamped.
    let step = if g0 + 1.0 <= gi.max { 1.0 } else { -1.0 };
    let slope = (at(g0 + step) - c0) / step;
    if !slope.is_finite() || slope.abs() < 0.05 {
        return g0;
    }
    gi.clamp(g0 + (target_db - c0) / slope)
}

/// Static compressor output level (dB) for an input level, with a soft knee of width `knee` dB.
pub fn comp_curve(input: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
    let r = ratio.max(1.0);
    let w = knee.max(0.0);
    let over = input - threshold;
    if w > 0.0 && over.abs() <= w * 0.5 {
        let k = over + w * 0.5;
        input + (1.0 / r - 1.0) * k * k / (2.0 * w)
    } else if over <= 0.0 {
        input
    } else {
        threshold + over / r
    }
}

/// (threshold, ratio, knee) of a compressor or channel-strip insert.
fn comp_params(ins: &Insert) -> (f32, f32, f32) {
    if ins.plugin == "channel_strip" {
        (param(ins, "comp_threshold"), param(ins, "comp_ratio"), 0.0)
    } else {
        (param(ins, "threshold"), param(ins, "ratio"), param(ins, "knee"))
    }
}

fn level_pos(peak: f32) -> f32 {
    if !peak.is_finite() || peak <= 0.0 {
        return 0.0;
    }
    ((20.0 * peak.log10() - COMP_MIN_DB) / -COMP_MIN_DB).clamp(0.0, 1.0)
}

/// One module's identity across all its graphs (its tile and its drawer): its track and its id.
pub fn module_key(track: u64, module: u32) -> Id {
    Id::new(("module-graph", track, module))
}

/// Draws the module's graph into `rect`. `large` adds grid labels. `streams` is what the
/// module's sample streams show (CleanQ's spectra, Twine's scope), when it has any. `module` is
/// the module's [`module_key`], which its graphs share a compute budget under (Vint's curve).
#[allow(clippy::too_many_arguments)]
pub fn draw_graph(
    ui: &mut Ui,
    rect: Rect,
    kind: GraphKind,
    ins: &Insert,
    module: Id,
    live: Option<&LiveValues>,
    streams: Option<&Streams>,
    sample_rate: f32,
    large: bool,
) {
    if !rect.is_finite() || rect.width() < 2.0 || rect.height() < 2.0 {
        // Nothing to draw in (a collapsed layout); infinite extents would make endless dashes.
        return;
    }
    let t = Tokens::current();
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(3), t.graph_bg);
    let label = |pos: Pos2, a: Align2, s: &str| {
        if large {
            p.text(pos, a, s, regular(9.0), t.text_dim);
        }
    };
    match kind {
        GraphKind::EqCurve => {
            let sr = sane_sr(sample_rate);
            for (f, name) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
                let x = freq_to_x(rect, f);
                p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, t.graph_grid));
                label(pos2(x + 2.0, rect.bottom() - 1.0), Align2::LEFT_BOTTOM, name);
            }
            for db in [-12.0, 0.0, 12.0] {
                let y = db_to_y(rect, db);
                p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, t.graph_grid));
                label(pos2(rect.left() + 2.0, y), Align2::LEFT_BOTTOM, &format!("{db:+.0}"));
            }
            let n = (rect.width().max(2.0) as usize).clamp(2, 256);
            let freqs: Vec<f32> = (0..n).map(|i| x_to_freq(rect, rect.left() + rect.width() * i as f32 / (n - 1) as f32)).collect();
            let overrides: Vec<(&str, f32)> = ins.params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            let resp = if ins.plugin == "eq_1band" {
                soundcraft_dsp::eq1_response(&overrides, &freqs, sr)
            } else {
                soundcraft_dsp::eq7_response(&overrides, &freqs, sr)
            };
            let zero = db_to_y(rect, 0.0);
            let pts: Vec<Pos2> = freqs.iter().zip(&resp).map(|(f, db)| pos2(freq_to_x(rect, *f), db_to_y(rect, *db))).collect();
            for w in pts.windows(2) {
                if let [a, b] = w {
                    p.add(Shape::convex_polygon(vec![*a, *b, pos2(b.x, zero), pos2(a.x, zero)], t.graph_fill, Stroke::NONE));
                }
            }
            p.add(Shape::line(pts, Stroke::new(1.5, t.graph_line)));
            for (i, (fid, _)) in eq_bands(&ins.plugin).iter().enumerate() {
                let c = BAND_COLORS.get(i).copied().unwrap_or(t.graph_line);
                let at = band_dot(rect, ins, fid, sr);
                p.circle_filled(at, DOT_R - 1.0, c);
                p.circle_stroke(at, DOT_R - 1.0, Stroke::new(1.0, t.graph_bg));
            }
        }
        GraphKind::Compressor => {
            let (thr, ratio, knee) = comp_params(ins);
            let pos = |inp: f32, out: f32| {
                let fx = ((inp - COMP_MIN_DB) / -COMP_MIN_DB).clamp(0.0, 1.0);
                let fy = ((out - COMP_MIN_DB) / -COMP_MIN_DB).clamp(0.0, 1.0);
                pos2(rect.left() + 2.0 + (rect.width() - 8.0) * fx, rect.bottom() - 2.0 - (rect.height() - 4.0) * fy)
            };
            for db in [-48.0, -24.0, 0.0] {
                let a = pos(db, COMP_MIN_DB);
                let b = pos(COMP_MIN_DB, db);
                p.line_segment([pos2(a.x, rect.top()), pos2(a.x, rect.bottom())], Stroke::new(1.0, t.graph_grid));
                p.line_segment([pos2(rect.left(), b.y), pos2(rect.right(), b.y)], Stroke::new(1.0, t.graph_grid));
                label(pos2(a.x - 1.0, rect.bottom() - 1.0), Align2::RIGHT_BOTTOM, &format!("{db:.0}"));
            }
            p.line_segment([pos(COMP_MIN_DB, COMP_MIN_DB), pos(0.0, 0.0)], Stroke::new(1.0, t.graph_grid));
            let pts: Vec<Pos2> = (0..=48)
                .map(|i| {
                    let x = COMP_MIN_DB + -COMP_MIN_DB * i as f32 / 48.0;
                    pos(x, comp_curve(x, thr, ratio, knee))
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(1.5, t.graph_line)));
            if let Some(l) = live {
                let inp = l.readouts[0];
                if inp.is_finite() {
                    let inp = inp.clamp(COMP_MIN_DB, 0.0);
                    p.circle_filled(pos(inp, comp_curve(inp, thr, ratio, knee)), 3.0, t.text);
                }
                let gr = if l.gr_db.is_finite() { l.gr_db.abs().min(24.0) } else { 0.0 };
                let bar = Rect::from_min_max(pos2(rect.right() - 5.0, rect.top() + 2.0), pos2(rect.right() - 2.0, rect.bottom() - 2.0));
                let fill = Rect::from_min_size(bar.min, vec2(bar.width(), bar.height() * gr / 24.0));
                p.rect_filled(fill, 0.0, t.gr_color);
            }
        }
        GraphKind::Level => {
            let (i, o) = live.map_or((0.0, 0.0), |l| (level_pos(l.in_peak), level_pos(l.out_peak)));
            let w = rect.width() / 5.0;
            for (n, v) in [i, o].into_iter().enumerate() {
                let x0 = rect.left() + w * (1.0 + 2.0 * n as f32);
                let foot = if large { 13.0 } else { 3.0 };
                let col = Rect::from_min_max(pos2(x0, rect.top() + 3.0), pos2(x0 + w, (rect.bottom() - foot).max(rect.top() + 4.0)));
                p.rect_filled(col, 1.0, t.graph_grid);
                let h = col.height() * v;
                let color = if v > 0.97 {
                    t.meter_red
                } else if v > 0.85 {
                    t.meter_yellow
                } else {
                    t.meter_green
                };
                p.rect_filled(Rect::from_min_max(pos2(col.left(), col.bottom() - h), col.right_bottom()), 1.0, color);
                label(pos2(col.center().x, rect.bottom() - 1.0), Align2::CENTER_BOTTOM, if n == 0 { "in" } else { "out" });
            }
        }
        GraphKind::CleanQ => draw_cleanq(ui, &p, rect, ins, streams, sample_rate, large),
        GraphKind::Comp => draw_comp(&p, rect, ins, live, large),
        GraphKind::Console => draw_console(&p, rect, ins, live, large),
        GraphKind::Twine => draw_twine(&p, rect, ins, live, streams, large),
        GraphKind::Vint => draw_vint(ui, &p, rect, ins, module, sample_rate, large),
        GraphKind::Pitch => draw_pitch(&p, rect, ins, live, large),
        GraphKind::Mod => draw_mod(&p, rect, ins, live, large),
        GraphKind::Dist => draw_dist(&p, rect, ins, live, large),
        GraphKind::Transient => draw_transient(&p, rect, ins, live, streams, large),
        GraphKind::Filter => draw_filter(&p, rect, ins, live, large),
        GraphKind::Delay => draw_delay(&p, rect, ins, live, large),
        GraphKind::Reverb => draw_reverb(&p, rect, ins, live, large),
        GraphKind::Swale => crate::swale_ui::draw(ui, &p, rect, ins, module, live, large),
        GraphKind::Chamber => crate::chamber_plan::draw(ui.ctx(), &p, rect, ins, module, large),
        GraphKind::Amp => draw_amp(ui, &p, rect, ins, module, live, sample_rate, large),
        GraphKind::Shale => crate::shale_ui::draw(ui, &p, rect, ins, module, live, streams.map(|s| &s.shale), large),
        GraphKind::LevelUp => crate::levelup_ui::draw(ui, &p, rect, ins, module, live, large),
        GraphKind::Manners => draw_manners(&p, rect, ins, streams, large),
        GraphKind::Sift => {
            let accent = module_accent(&ins.plugin);
            sift_timeline(&p, rect, streams, accent, large);
        }
    }
    p.rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0, t.module_tile_border), StrokeKind::Inside);
}

/// Dragging a band dot of an EQ curve drawn in `rect` (with the same `sample_rate` as
/// [`draw_graph`]): the new `(param id, value)` pairs, frequency and gain, clamped to the
/// parameter ranges, chosen so the curve passes under the pointer. `None` when nothing is dragged.
pub fn eq_band_drag(ui: &Ui, rect: Rect, ins: &Insert, sample_rate: f32) -> Option<Vec<(String, f32)>> {
    let info = soundcraft_dsp::plugin_info(&ins.plugin)?;
    let sr = sane_sr(sample_rate);
    let mut out = None;
    for (i, (fid, gid)) in eq_bands(&ins.plugin).iter().enumerate() {
        let at = band_dot(rect, ins, fid, sr);
        let hit = Rect::from_center_size(at, vec2(DOT_R * 3.0, DOT_R * 3.0));
        let resp = ui.interact(hit, ui.id().with(("eq-band", ins.plugin.as_str(), i)), Sense::drag());
        if !resp.dragged() {
            continue;
        }
        let Some(pos) = resp.interact_pointer_pos() else { continue };
        let (Some(fi), Some(gi)) = (info.param(fid), info.param(gid)) else { continue };
        let freq = fi.clamp(x_to_freq(rect, pos.x));
        let gain = gain_for_curve(ins, (fid, gid), gi, freq, y_to_db(rect, pos.y), sr);
        out = Some(vec![((*fid).to_string(), freq), ((*gid).to_string(), gain)]);
    }
    out
}

// ------------------------------------------------------------------ FireStation engines

/// CleanQ's analyzer: FFT size, and how much of the last frame each new one keeps.
pub const SPEC_FFT: usize = 4096;
pub const SPEC_SMOOTH: f32 = 0.7;
/// CleanQ's analyzer takes a new frame at most this often (about 30 Hz, as the C++ editor's
/// timer), however fast the UI repaints.
pub const SPEC_INTERVAL: f64 = 1.0 / 30.0;
/// Transient's scope: the values per bin on its ring (`SCOPE_BIN_VALUES`), and the bins shown:
/// the C++ editor's last 2.5 s of 4 ms bins.
pub const TRANSIENT_VALUES: usize = soundcraft_dsp::plugins::fs::transient::transient_engine_dsp::SCOPE_BIN_VALUES;
pub const TRANSIENT_BINS: usize = 625;
/// Twine's scope: input samples per column, columns kept (about 2 s at 48 kHz).
pub const SCOPE_BUCKET: usize = 256;
pub const SCOPE_COLS: usize = 375;

/// What a module's sample streams show, carried from frame to frame (smoothing, scrolling).
#[derive(Debug, Clone, Default)]
pub struct Streams {
    /// Smoothed magnitude spectra (dBFS per bin of a [`SPEC_FFT`]-point transform) of the
    /// module's input and output; empty while that stream is not fed.
    pub pre: Vec<f32>,
    pub post: Vec<f32>,
    /// The post stream as a scope: (min, max) of every [`SCOPE_BUCKET`] samples, newest last.
    pub scope: VecDeque<(f32, f32)>,
    /// Transient's scope bins (input peak, output peak, attack dB, sustain dB), newest last, the
    /// last [`TRANSIENT_BINS`].
    pub bins: VecDeque<[f32; TRANSIENT_VALUES]>,
    /// Shale's scope bins (input peak, gate gain, detector peak), newest last, the last
    /// [`crate::shale_ui::SCOPE_BINS`] (the DSP's ten seconds).
    pub shale: VecDeque<[f32; crate::shale_ui::SCOPE_VALUES]>,
    /// Sift's cuts per scope column (sibilance dB, plosive dB), newest last, as long as `scope`.
    pub sift: VecDeque<(f32, f32)>,
    /// Manners' gain curve as of its last block: (band centre Hz, gain dB), low to high.
    pub manners: Vec<(f32, f32)>,
    /// Columns ever pushed onto `scope`.
    scope_pushed: u64,
    /// The UI pass these were last brought up to date in, plus one (0: never).
    pass: u64,
    /// Samples written to (pre, post) when last read.
    seen: (usize, usize),
    /// The scope column being filled: (samples, min, max).
    acc: (usize, f32, f32),
    /// Room for the samples a spectrum is taken of, kept between frames.
    scratch: Vec<f32>,
    /// The newest frame's spectrum, before smoothing (kept between frames).
    fresh: Vec<f32>,
    /// The FFT plan, window and work buffers, made once.
    analyzer: soundcraft_dsp::spectrum::Analyzer,
    /// When the spectra last took a frame (UI seconds; `None`: never).
    spec_at: Option<f64>,
}

impl Streams {
    /// Brings these up to date with `a` for a `kind` graph at UI time `now` (seconds): CleanQ's
    /// spectra while samples arrive, at most once per [`SPEC_INTERVAL`]; Twine's scope columns
    /// for the samples written since the last call.
    pub fn feed(&mut self, a: &Analysis, kind: GraphKind, now: f64) {
        match kind {
            GraphKind::CleanQ => {
                // Time that went backwards (a new context) or is not finite takes a frame now.
                let due = match self.spec_at {
                    // (A millisecond's slack: two 60 Hz frames are one interval, rounding aside.)
                    Some(t) => !now.is_finite() || now < t || now - t >= SPEC_INTERVAL - 1e-3,
                    None => true,
                };
                if !due {
                    return;
                }
                self.spec_at = Some(now);
                let (scratch, fresh, an) = (&mut self.scratch, &mut self.fresh, &mut self.analyzer);
                feed_spectrum(&mut self.pre, &mut self.seen.0, &a.pre, scratch, fresh, an);
                feed_spectrum(&mut self.post, &mut self.seen.1, &a.post, scratch, fresh, an);
            }
            GraphKind::Twine => self.feed_scope(&a.post),
            GraphKind::Transient => self.feed_bins(&a.post),
            GraphKind::Shale => self.feed_shale(&a.post),
            GraphKind::Sift => {
                use soundcraft_dsp::plugins::fs::sift::{LIVE_PLO_CUT, LIVE_SIB_CUT};
                // The input as a scope, and beside each new column the largest cuts since the last.
                let before = self.scope_pushed;
                self.feed_scope(&a.pre);
                let new = (self.scope_pushed - before).min(SCOPE_COLS as u64);
                if new > 0 {
                    let cut = |i: usize| {
                        let v = a.live.take(i);
                        if v.is_finite() { v.max(0.0) } else { 0.0 }
                    };
                    let cuts = (cut(LIVE_SIB_CUT), cut(LIVE_PLO_CUT));
                    for _ in 0..new {
                        while self.sift.len() >= SCOPE_COLS {
                            self.sift.pop_front();
                        }
                        self.sift.push_back(cuts);
                    }
                }
            }
            GraphKind::Manners => {
                use soundcraft_dsp::plugins::fs::manners::{LIVE_BANDS, LIVE_GAIN, LIVE_HZ, MAX_BANDS};
                let bands = a.live.get(LIVE_BANDS);
                let bands = if bands.is_finite() && bands > 0.0 { (bands as usize).min(MAX_BANDS) } else { 0 };
                self.manners.clear();
                self.manners.extend(
                    (0..bands).map(|i| (a.live.get(LIVE_HZ + i), a.live.get(LIVE_GAIN + i))).filter(|(hz, g)| hz.is_finite() && g.is_finite()),
                );
                // The spectra behind the curve in its editor, at the analyzer's pace.
                let due = match self.spec_at {
                    Some(t) => !now.is_finite() || now < t || now - t >= SPEC_INTERVAL - 1e-3,
                    None => true,
                };
                if due {
                    self.spec_at = Some(now);
                    let (scratch, fresh, an) = (&mut self.scratch, &mut self.fresh, &mut self.analyzer);
                    feed_spectrum(&mut self.pre, &mut self.seen.0, &a.pre, scratch, fresh, an);
                    feed_spectrum(&mut self.post, &mut self.seen.1, &a.post, scratch, fresh, an);
                }
            }
            _ => {}
        }
    }

    /// Transient's scope bins written since the last call (whole bins, at most what is shown).
    fn feed_bins(&mut self, ring: &AnalysisRing) {
        let w = ring.written();
        // A ring that went backwards is a new instance: take what it has.
        let new = if w >= self.seen.1 { w - self.seen.1 } else { w };
        self.seen.1 = w;
        let take = (new / TRANSIENT_VALUES).min(TRANSIENT_BINS) * TRANSIENT_VALUES;
        if take == 0 {
            return;
        }
        // Read through the kept scratch buffer (no allocation once it has grown).
        self.scratch.resize(take, 0.0);
        ring.latest(&mut self.scratch);
        for b in self.scratch.chunks_exact(TRANSIENT_VALUES) {
            while self.bins.len() >= TRANSIENT_BINS {
                self.bins.pop_front();
            }
            self.bins.push_back([b[0], b[1], b[2], b[3]]);
        }
    }

    /// Shale's scope bins written since the last call (whole bins, at most what is kept).
    fn feed_shale(&mut self, ring: &AnalysisRing) {
        use crate::shale_ui::{SCOPE_BINS, SCOPE_VALUES};
        let w = ring.written();
        // A ring that went backwards is a new instance: take what it has.
        let new = if w >= self.seen.1 { w - self.seen.1 } else { w };
        self.seen.1 = w;
        let take = (new / SCOPE_VALUES).min(SCOPE_BINS).min(RING_LEN / SCOPE_VALUES) * SCOPE_VALUES;
        if take == 0 {
            return;
        }
        // Read through the kept scratch buffer (no allocation once it has grown).
        self.scratch.resize(take, 0.0);
        ring.latest(&mut self.scratch);
        for b in self.scratch.chunks_exact(SCOPE_VALUES) {
            while self.shale.len() >= SCOPE_BINS {
                self.shale.pop_front();
            }
            self.shale.push_back([b[0], b[1], b[2]]);
        }
    }

    fn feed_scope(&mut self, ring: &AnalysisRing) {
        let w = ring.written();
        // A ring that went backwards is a new instance: take what it has.
        let new = if w >= self.seen.1 { w - self.seen.1 } else { w }.min(RING_LEN);
        self.seen.1 = w;
        if new == 0 {
            return;
        }
        let mut buf = vec![0.0f32; new];
        ring.latest(&mut buf);
        for x in buf {
            let (n, lo, hi) = &mut self.acc;
            if *n == 0 {
                (*lo, *hi) = (x, x);
            } else {
                *lo = lo.min(x);
                *hi = hi.max(x);
            }
            *n += 1;
            if *n >= SCOPE_BUCKET {
                while self.scope.len() >= SCOPE_COLS {
                    self.scope.pop_front();
                }
                self.scope.push_back((*lo, *hi));
                self.scope_pushed += 1;
                *n = 0;
            }
        }
    }
}

/// A new smoothed spectrum of `ring` when it has new samples; cleared while it was never fed.
fn feed_spectrum(
    spec: &mut Vec<f32>,
    seen: &mut usize,
    ring: &AnalysisRing,
    buf: &mut Vec<f32>,
    fresh: &mut Vec<f32>,
    analyzer: &mut soundcraft_dsp::spectrum::Analyzer,
) {
    let w = ring.written();
    if w == 0 {
        spec.clear();
        *seen = 0;
        return;
    }
    if w == *seen {
        return;
    }
    *seen = w;
    buf.resize(SPEC_FFT, 0.0);
    ring.latest(buf);
    analyzer.magnitude_db_into(buf, SPEC_FFT, fresh);
    if spec.len() != fresh.len() {
        spec.clone_from(fresh);
        return;
    }
    for (s, f) in spec.iter_mut().zip(fresh.iter().copied()) {
        *s = SPEC_SMOOTH * *s + (1.0 - SPEC_SMOOTH) * f;
    }
}

/// The streams of the module `key` names, brought up to date at most once per UI pass, so its
/// tile and its drawer share them. Shared, not copied: the context keeps one `Arc`, updated in
/// place while nothing else holds it (the tile's and drawer's copies of last frame are gone).
pub fn module_streams(ctx: &egui::Context, key: Id, a: &Analysis, kind: GraphKind) -> Arc<Streams> {
    let pass = ctx.cumulative_pass_nr() + 1;
    let cur: Option<Arc<Streams>> = ctx.data(|d| d.get_temp(key));
    if let Some(s) = cur.as_ref().filter(|s| s.pass == pass) {
        return Arc::clone(s);
    }
    // Out of the context first, so the update below is in place, not a copy.
    ctx.data_mut(|d| d.remove_temp::<Arc<Streams>>(key));
    let mut s = cur.unwrap_or_default();
    let now = ctx.input(|i| i.time);
    let m = Arc::make_mut(&mut s);
    m.feed(a, kind, now);
    m.pass = pass;
    ctx.data_mut(|d| d.insert_temp(key, Arc::clone(&s)));
    s
}

/// A dB value's height in `rect` for a ±`range` dB scale (non-finite: 0 dB).
pub(crate) fn db_y(rect: Rect, db: f32, range: f32) -> f32 {
    let db = if db.is_finite() { db.clamp(-range, range) } else { 0.0 };
    rect.center().y - db / range * rect.height() * 0.5
}

pub(crate) fn y_db(rect: Rect, y: f32, range: f32) -> f32 {
    if rect.height() <= 0.0 || !y.is_finite() {
        return 0.0;
    }
    ((rect.center().y - y) / (rect.height() * 0.5) * range).clamp(-range, range)
}

/// CleanQ's curve scale, ±dB.
pub const CQ_RANGE: f32 = 24.0;
/// The analyzer's floor (dBFS) and its tilt, dB per octave about 1 kHz, so that music reads level
/// across the band, as analyzers commonly show it.
const SPEC_FLOOR: f32 = -96.0;
const SPEC_TILT: f32 = 3.0;

/// CleanQ's values in parameter order, each clamped, a missing one at its default.
pub fn cleanq_values(ins: &Insert) -> Vec<f32> {
    CLEANQ_PARAMS.iter().map(|p| p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default))).collect()
}

/// A CleanQ band in use: its mode is not Off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CqBand {
    /// 0..24.
    pub slot: usize,
    /// On (not bypassed).
    pub active: bool,
    /// The type index: Bell, Low Shelf, High Shelf, Low Cut, High Cut, Notch, Band Pass.
    pub kind: i32,
    pub freq: f32,
    pub gain_db: f32,
}

impl CqBand {
    /// Bells and shelves have a gain to drag; cuts, notches and band passes only a frequency.
    pub fn has_gain(&self) -> bool {
        (0..=2).contains(&self.kind)
    }
}

/// The bands of a CleanQ in use (mode not Off), in slot order.
pub fn cleanq_bands(values: &[f32]) -> Vec<CqBand> {
    use cq::clean_q_dsp as q;
    (0..q::NUM_BANDS)
        .filter_map(|slot| {
            let get = |f: usize| values.get(cq::band_param(slot, f)).copied().unwrap_or(0.0);
            let on = get(cq::F_ON).round() as i32;
            (on != q::ON_FREE).then(|| CqBand {
                slot,
                active: on == q::ON_ACTIVE,
                kind: get(cq::F_TYPE).round() as i32,
                freq: q::knob_to_freq(get(cq::F_FREQ)) as f32,
                gain_db: q::knob_to_gain_db(get(cq::F_GAIN)) as f32,
            })
        })
        .collect()
}

/// A band's colour: the slots go round the hue wheel, as EQ displays colour their bands.
pub fn cq_band_color(slot: usize) -> Color32 {
    let h = (slot % 24) as f32 / 24.0;
    let v = if crate::theme::is_light() { 0.72 } else { 0.98 };
    egui::ecolor::Hsva::new(h, 0.55, v, 1.0).into()
}

/// The parameter id of one field of a CleanQ band.
pub fn cq_param_id(slot: usize, field: usize) -> &'static str {
    CLEANQ_PARAMS.get(cq::band_param(slot, field)).map_or("", |p| p.id)
}

/// A cached CleanQ curve: the hash of what it was computed from, and the curve.
type CurveCache = (u64, std::sync::Arc<Vec<f32>>);

/// The whole CleanQ curve (dB) at `freqs`, recomputed only when the values, points or rate change.
/// One entry per `key` (a module's graph at one width), replaced when its inputs change.
fn cleanq_curve(ctx: &egui::Context, key: Id, values: &[f32], freqs: &[f32], sr: f32) -> std::sync::Arc<Vec<f32>> {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    values.iter().chain(freqs).chain([&sr]).for_each(|v| v.to_bits().hash(&mut h));
    let hash = h.finish();
    if let Some((_, c)) = ctx.data(|d| d.get_temp::<CurveCache>(key)).filter(|(k, _)| *k == hash) {
        return c;
    }
    let c: std::sync::Arc<Vec<f32>> =
        std::sync::Arc::new(freqs.iter().map(|f| cq::response_db(values, sr, *f)).map(|v| if v.is_finite() { v } else { 0.0 }).collect());
    ctx.data_mut(|d| d.insert_temp::<CurveCache>(key, (hash, std::sync::Arc::clone(&c))));
    c
}

/// Where a band's dot sits: a bell or shelf at its frequency and gain, any other on the curve.
pub(crate) fn cq_dot(rect: Rect, values: &[f32], b: &CqBand, sr: f32) -> Pos2 {
    let db = if b.has_gain() { b.gain_db } else { cq::response_db(values, sr, b.freq) };
    pos2(freq_to_x(rect, b.freq), db_y(rect, db, CQ_RANGE))
}

/// A spectrum as a filled outline over `rect`: the loudest bin under each point, tilted.
pub(crate) fn spectrum(p: &egui::Painter, rect: Rect, spec: &[f32], sr: f32, fill: Color32, line: Option<Stroke>) {
    if spec.len() < 4 {
        return;
    }
    let n = (rect.width().max(2.0) as usize / 2).clamp(2, 200);
    let bin_of = |f: f32| f * SPEC_FFT as f32 / sr;
    let last = spec.len() - 1;
    let half = rect.width() / (n - 1) as f32 * 0.5;
    let pts: Vec<Pos2> = (0..n)
        .map(|i| {
            let x = rect.left() + rect.width() * i as f32 / (n - 1) as f32;
            let (f0, f1, f) = (x_to_freq(rect, x - half), x_to_freq(rect, x + half), x_to_freq(rect, x));
            let (k0, k1) = ((bin_of(f0).floor().max(1.0) as usize).min(last), (bin_of(f1).ceil() as usize).min(last));
            let db = if k1 > k0 + 1 {
                spec.get(k0..=k1).map_or(SPEC_FLOOR, |s| s.iter().copied().fold(f32::MIN, f32::max))
            } else {
                // Fewer bins than points: interpolate.
                let k = bin_of(f).clamp(1.0, last as f32);
                let (a, t) = (k.floor() as usize, k.fract());
                let b = (a + 1).min(last);
                spec.get(a).copied().unwrap_or(SPEC_FLOOR) * (1.0 - t) + spec.get(b).copied().unwrap_or(SPEC_FLOOR) * t
            };
            let db = db + SPEC_TILT * (f / 1000.0).log2();
            let v = if db.is_finite() { ((db - SPEC_FLOOR) / -SPEC_FLOOR).clamp(0.0, 1.0) } else { 0.0 };
            pos2(x, rect.bottom() - rect.height() * v)
        })
        .collect();
    for w in pts.windows(2) {
        if let [a, b] = w {
            p.add(Shape::convex_polygon(vec![*a, *b, pos2(b.x, rect.bottom()), pos2(a.x, rect.bottom())], fill, Stroke::NONE));
        }
    }
    if let Some(s) = line {
        p.add(Shape::line(pts, s));
    }
}

/// A curve with a soft glow under it.
fn glow_line(p: &egui::Painter, pts: Vec<Pos2>, color: Color32, width: f32) {
    p.add(Shape::line(pts.clone(), Stroke::new(width * 3.5, color.gamma_multiply(0.14))));
    p.add(Shape::line(pts, Stroke::new(width, color)));
}

fn draw_cleanq(ui: &Ui, p: &egui::Painter, rect: Rect, ins: &Insert, streams: Option<&Streams>, sample_rate: f32, large: bool) {
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let sr = sane_sr(sample_rate);
    let tiny = rect.height() < 30.0;
    // A wide graph (the VST-sized editor) takes the editor's full scale: 20 Hz..20 kHz in 1-2-5
    // steps and every 6 dB.
    let wide = rect.width() > 500.0;
    let fticks: &[(f32, &str)] = if wide {
        &[
            (20.0, "20"),
            (50.0, "50"),
            (100.0, "100"),
            (200.0, "200"),
            (500.0, "500"),
            (1000.0, "1k"),
            (2000.0, "2k"),
            (5000.0, "5k"),
            (10_000.0, "10k"),
            (20_000.0, "20k"),
        ]
    } else {
        &[(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")]
    };
    for &(f, name) in fticks {
        let x = freq_to_x(rect, f);
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, t.graph_grid));
        if large {
            p.text(pos2(x + 2.0, rect.bottom() - 1.0), Align2::LEFT_BOTTOM, name, regular(if wide { 10.0 } else { 9.0 }), t.text_dim);
        }
    }
    if !tiny {
        let dbs: &[f32] = if wide { &[-18.0, -12.0, -6.0, 6.0, 12.0, 18.0] } else { &[-12.0, 12.0] };
        for &db in dbs {
            let y = db_y(rect, db, CQ_RANGE);
            p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.7)));
            if large {
                p.text(pos2(rect.left() + 3.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:+.0}"), regular(8.5), t.text_dim);
            }
        }
    }
    let zero = db_y(rect, 0.0, CQ_RANGE);
    p.line_segment([pos2(rect.left(), zero), pos2(rect.right(), zero)], Stroke::new(1.0, t.graph_grid));

    // The spectra: the input faint, the output in the engine's colour.
    if let Some(s) = streams {
        spectrum(p, rect, &s.pre, sr, t.text_dim.gamma_multiply(0.16), None);
        spectrum(p, rect, &s.post, sr, accent.gamma_multiply(0.13), Some(Stroke::new(1.0, accent.gamma_multiply(0.45))));
    }

    let values = cleanq_values(ins);
    let n = ((rect.width() / 1.5).max(2.0) as usize).clamp(2, 240);
    let freqs: Vec<f32> = (0..n).map(|i| x_to_freq(rect, rect.left() + rect.width() * i as f32 / (n - 1) as f32)).collect();
    let curve = cleanq_curve(ui.ctx(), Id::new(("cleanq-curve", ui.id(), ins.id.0, n)), &values, &freqs, sr);
    let pts: Vec<Pos2> = freqs.iter().zip(curve.iter()).map(|(f, db)| pos2(freq_to_x(rect, *f), db_y(rect, *db, CQ_RANGE))).collect();
    for w in pts.windows(2) {
        if let [a, b] = w {
            p.add(Shape::convex_polygon(vec![*a, *b, pos2(b.x, zero), pos2(a.x, zero)], accent.gamma_multiply(0.16), Stroke::NONE));
        }
    }
    glow_line(p, pts, accent, if large { 2.0 } else { 1.5 });

    let r = if large {
        5.5
    } else if tiny {
        2.5
    } else {
        3.5
    };
    for b in cleanq_bands(&values) {
        let at = cq_dot(rect, &values, &b, sr);
        let c = cq_band_color(b.slot);
        if b.active {
            p.circle_filled(at, r + 2.0, c.gamma_multiply(0.22));
            p.circle_filled(at, r, c);
            p.circle_stroke(at, r, Stroke::new(1.0, t.graph_bg));
        } else {
            p.circle_filled(at, r, t.graph_bg);
            p.circle_stroke(at, r, Stroke::new(1.2, c.gamma_multiply(0.6)));
        }
        if large {
            let above = at.y > rect.top() + 18.0;
            let pos = if above { at - vec2(0.0, r + 2.0) } else { at + vec2(0.0, r + 2.0) };
            let align = if above { Align2::CENTER_BOTTOM } else { Align2::CENTER_TOP };
            p.text(pos, align, format!("{}", b.slot + 1), bold(8.5), if b.active { c } else { t.text_dim });
        }
    }
}

/// Touching a CleanQ band dot: its slot, and while it is dragged the new values (knob 0..1) of its
/// frequency and, for a bell or shelf, its gain.
#[derive(Debug, Clone, PartialEq)]
pub struct BandTouch {
    pub slot: usize,
    pub params: Vec<(String, f32)>,
}

/// The new knob values for band `b` dragged to `pos` in `rect`: its frequency and, for a bell or
/// shelf, its gain, each clamped to the knob's 0..1.
pub fn cleanq_drag_values(rect: Rect, b: &CqBand, pos: Pos2) -> Vec<(String, f32)> {
    use cq::clean_q_dsp as q;
    let f = x_to_freq(rect, pos.x).clamp(10.0, 20_000.0);
    let mut params = vec![(cq_param_id(b.slot, cq::F_FREQ).to_string(), q::freq_to_knob(f64::from(f)).clamp(0.0, 1.0))];
    if b.has_gain() {
        let g = y_db(rect, pos.y, CQ_RANGE);
        params.push((cq_param_id(b.slot, cq::F_GAIN).to_string(), q::gain_to_knob(f64::from(g)).clamp(0.0, 1.0)));
    }
    params
}

/// The band dot touched in a CleanQ graph drawn in `rect` (with the same `sample_rate` as
/// [`draw_graph`]), if any. Only bands in use have dots.
pub fn cleanq_band_drag(ui: &Ui, rect: Rect, ins: &Insert, sample_rate: f32) -> Option<BandTouch> {
    let sr = sane_sr(sample_rate);
    let values = cleanq_values(ins);
    let mut out = None;
    for b in cleanq_bands(&values) {
        let at = cq_dot(rect, &values, &b, sr);
        let hit = Rect::from_center_size(at, vec2(DOT_R * 3.0, DOT_R * 3.0));
        let resp = ui.interact(hit, ui.id().with(("cq-band", b.slot)), Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Band {}", b.slot + 1)));
        if resp.dragged()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            out = Some(BandTouch { slot: b.slot, params: cleanq_drag_values(rect, &b, pos) });
        } else if (resp.clicked() || resp.drag_started()) && out.is_none() {
            out = Some(BandTouch { slot: b.slot, params: Vec::new() });
        }
    }
    out
}

/// The engine's ratio knob for its Clean (and 160) style (`ratioFromKnob`).
fn comp_ratio_from_knob(t: f32) -> f32 {
    if t > 0.965 {
        return 1000.0;
    }
    let u = t / 0.965;
    1.0 + 19.0 * u * u
}

/// A switch's position for a 0..1 knob over `list` (the engine's `(int) (knob * (n - 1) + 0.5)`).
fn switch<T: Copy>(knob: f32, list: &[T]) -> Option<T> {
    let n = list.len().checked_sub(1)?;
    let i = (knob.clamp(0.0, 1.0) * n as f32 + 0.5) as usize;
    list.get(i.min(n)).copied()
}

/// Lane A's static curve as (threshold dB, ratio, knee dB): the engine's `updateParams` per
/// style (threshold `-50 dB x amount`, the Vari-Mu's `-45`), its circuit styles drawn at their
/// switch's nominal ratio. For the eye only: the circuit models' real curves bend.
pub fn comp_lane_a(ins: &Insert) -> (f32, f32, f32) {
    let v = |id: &str| param(ins, id);
    let style = (v("style") + 0.5) as i32;
    let (amount, rk, kk) = (v("amount"), v("ratio"), v("knee"));
    let thr = if style == 5 { -45.0 * amount } else { -50.0 * amount };
    let (ratio, knee) = match style {
        // 1176: 4, 8, 12, 20 and all buttons in (drawn as 30:1).
        1 => (switch(rk, &[4.0, 8.0, 12.0, 20.0, 30.0]).unwrap_or(4.0), 6.0),
        // LA-2A: Compress or Limit.
        2 => (if rk >= 0.5 { 20.0 } else { 4.0 }, 10.0),
        3 => (comp_ratio_from_knob(rk), if kk >= 0.5 { 14.0 } else { 0.0 }),
        // G Bus: 2, 4, 10.
        4 => (switch(rk, &[2.0, 4.0, 10.0]).unwrap_or(4.0), 6.0),
        5 => (6.0, 10.0),
        7 => switch(rk, &[(1.0, 0.0), (2.0, 30.0), (3.0, 20.0), (4.0, 15.0), (6.0, 10.0), (10.0, 20.0), (20.0, 6.0), (100.0, 2.0)])
            .unwrap_or((4.0, 15.0)),
        8 => (switch(rk, &[1.5, 2.0, 3.0, 4.0, 6.0, 10.0, 1000.0]).unwrap_or(4.0), switch(kk, &[0.0, 6.0, 14.0]).unwrap_or(6.0)),
        9 => {
            let r = switch(rk, &[1.5, 2.0, 3.0, 4.0, 6.0, 121.0]).unwrap_or(4.0);
            (r, if r > 100.0 { 2.0 } else { 6.0 })
        }
        // Clean, and Ross with the same dB-domain mapping.
        _ => (comp_ratio_from_knob(rk), 24.0 * kk),
    };
    (thr, ratio, knee)
}

/// Meter-style colour for a 0..1 level.
fn level_color(t: &Tokens, v: f32) -> Color32 {
    if v > 0.97 {
        t.meter_red
    } else if v > 0.85 {
        t.meter_yellow
    } else {
        t.meter_green
    }
}

/// One gain-reduction bar: its label, its reading as a fraction of the scale, whether its stage
/// is on, its colour.
type GrBar<'a> = (&'a str, f32, bool, Color32);

/// Bars hanging from the top of `r`, labelled when `large`.
fn hanging_bars(p: &egui::Painter, r: Rect, bars: &[GrBar<'_>], large: bool) {
    let t = Tokens::current();
    let n = bars.len().max(1) as f32;
    let foot = if large { 12.0 } else { 0.0 };
    let w = r.width() / n;
    for (i, (label, v, on, color)) in bars.iter().enumerate() {
        let x0 = r.left() + w * i as f32;
        let track = Rect::from_min_max(pos2(x0 + w * 0.22, r.top() + 2.0), pos2(x0 + w * 0.78, r.bottom() - 2.0 - foot));
        p.rect_filled(track, CornerRadius::same(1), if *on { t.graph_grid } else { t.graph_grid.gamma_multiply(0.45) });
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        if *on && v > 0.0 {
            p.rect_filled(Rect::from_min_size(track.min, vec2(track.width(), track.height() * v)), CornerRadius::same(1), *color);
        }
        if large {
            let c = if *on { t.text_dim } else { t.text_dim.gamma_multiply(0.5) };
            p.text(pos2(track.center().x, r.bottom() - 1.0), Align2::CENTER_BOTTOM, *label, bold(8.0), c);
        }
    }
}

/// Live values with no reading at all (the module is not running).
fn no_reading() -> LiveValues {
    LiveValues { readouts: [f32::NAN; 8], ..LiveValues::default() }
}

fn draw_comp(p: &egui::Painter, rect: Rect, ins: &Insert, live: Option<&LiveValues>, large: bool) {
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let (thr, ratio, knee) = comp_lane_a(ins);
    let bars_w = if large { 34.0 } else { 14.0 };
    let plot = Rect::from_min_max(rect.min, pos2(rect.max.x - bars_w, rect.max.y));
    let pos = |inp: f32, out: f32| {
        let fx = ((inp - COMP_MIN_DB) / -COMP_MIN_DB).clamp(0.0, 1.0);
        let fy = ((out - COMP_MIN_DB) / -COMP_MIN_DB).clamp(0.0, 1.0);
        pos2(plot.left() + 2.0 + (plot.width() - 6.0) * fx, plot.bottom() - 2.0 - (plot.height() - 4.0) * fy)
    };
    for db in [-48.0, -24.0, 0.0] {
        let a = pos(db, COMP_MIN_DB);
        let b = pos(COMP_MIN_DB, db);
        p.line_segment([pos2(a.x, plot.top()), pos2(a.x, plot.bottom())], Stroke::new(1.0, t.graph_grid));
        p.line_segment([pos2(plot.left(), b.y), pos2(plot.right(), b.y)], Stroke::new(1.0, t.graph_grid));
        if large {
            p.text(pos2(a.x - 2.0, plot.bottom() - 1.0), Align2::RIGHT_BOTTOM, format!("{db:.0}"), regular(8.5), t.text_dim);
        }
    }
    // The 1:1 line, and the reduction wedge between it and the curve.
    p.line_segment([pos(COMP_MIN_DB, COMP_MIN_DB), pos(0.0, 0.0)], Stroke::new(1.0, t.graph_grid));
    let steps = 64;
    let xs: Vec<f32> = (0..=steps).map(|i| COMP_MIN_DB + -COMP_MIN_DB * i as f32 / steps as f32).collect();
    let pts: Vec<Pos2> = xs.iter().map(|x| pos(*x, comp_curve(*x, thr, ratio, knee))).collect();
    for (w, xw) in pts.windows(2).zip(xs.windows(2)) {
        if let ([a, b], [xa, xb]) = (w, xw) {
            let (ua, ub) = (pos(*xa, *xa), pos(*xb, *xb));
            if (ua.y - a.y).abs() > 0.2 || (ub.y - b.y).abs() > 0.2 {
                p.add(Shape::convex_polygon(vec![ua, ub, *b, *a], accent.gamma_multiply(0.14), Stroke::NONE));
            }
        }
    }
    glow_line(p, pts, accent, if large { 2.0 } else { 1.5 });
    let lv = live.copied().unwrap_or_else(no_reading);
    let inp = lv.readouts[0];
    if inp.is_finite() {
        let inp = inp.clamp(COMP_MIN_DB, 0.0);
        let at = pos(inp, comp_curve(inp, thr, ratio, knee));
        p.circle_filled(at, 6.0, t.text.gamma_multiply(0.18));
        p.circle_filled(at, 3.0, t.text);
    }
    if large {
        let style =
            soundcraft_dsp::plugin_info(&ins.plugin).and_then(|i| i.param("style")).map(|s| s.format(param(ins, "style"))).unwrap_or_default();
        let r = if ratio >= 100.0 { "∞:1".to_string() } else { format!("{ratio:.1}:1") };
        p.text(plot.min + vec2(5.0, 4.0), Align2::LEFT_TOP, format!("{style} · {r}"), bold(9.0), accent);
    }
    let lanes = [("A", 1, "aOn"), ("B", 2, "bOn"), ("C", 3, "cOn")];
    let bars: Vec<GrBar<'_>> = lanes.iter().map(|(name, i, on)| (*name, lv.readouts[*i] / 24.0, param(ins, on) >= 0.5, t.gr_color)).collect();
    hanging_bars(p, Rect::from_min_max(pos2(plot.right() + 1.0, rect.top()), rect.max), &bars, large);
}

/// An arc from `a0` to `a1` (radians, 0 = up, clockwise) about `c`.
fn arc(c: Pos2, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let n = ((a1 - a0).abs() * r / 3.0).clamp(4.0, 64.0) as usize;
    (0..=n).map(|i| a0 + (a1 - a0) * i as f32 / n as f32).map(|a| c + vec2(a.sin(), -a.cos()) * r).collect()
}

/// The drive scale of Sat and Tape, dB (their `knob_to_drive_db`: -6 + 30 x knob).
const DRIVE_MIN_DB: f32 = -6.0;
const DRIVE_MAX_DB: f32 = 24.0;

fn draw_console(p: &egui::Painter, rect: Rect, ins: &Insert, live: Option<&LiveValues>, large: bool) {
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let lv = live.copied().unwrap_or_else(no_reading);
    let lin_db = |pk: f32| if pk > 0.0 && pk.is_finite() { 20.0 * pk.log10() } else { f32::NEG_INFINITY };
    let in_db = if lv.readouts[0].is_finite() { lv.readouts[0] } else { lin_db(lv.in_peak) };
    let out_db = if lv.readouts[1].is_finite() { lv.readouts[1] } else { lin_db(lv.out_peak) };
    let drive = if lv.readouts[2].is_finite() { lv.readouts[2] } else { DRIVE_MIN_DB + (DRIVE_MAX_DB - DRIVE_MIN_DB) * param(ins, "drive") };

    // The drive gauge at the right, the levels at the left.
    let d = (rect.height() - if large { 8.0 } else { 4.0 }).min(rect.width() * 0.42).max(8.0);
    let gauge = Rect::from_center_size(pos2(rect.right() - d * 0.5 - 4.0, rect.center().y + d * 0.06), vec2(d, d));
    let c = gauge.center();
    let r = d * 0.42;
    let (a0, a1) = (-2.1f32, 2.1f32);
    let frac = ((drive - DRIVE_MIN_DB) / (DRIVE_MAX_DB - DRIVE_MIN_DB)).clamp(0.0, 1.0);
    let w = (r * 0.2).clamp(2.0, 6.0);
    p.add(Shape::line(arc(c, r, a0, a1), Stroke::new(w, t.graph_grid)));
    if frac > 0.0 {
        let av = a0 + (a1 - a0) * frac;
        p.add(Shape::line(arc(c, r, a0, av), Stroke::new(w * 2.6, accent.gamma_multiply(0.16))));
        p.add(Shape::line(arc(c, r, a0, av), Stroke::new(w, accent)));
        p.circle_filled(c + vec2(av.sin(), -av.cos()) * r, w * 0.75, t.text);
    }
    if large {
        p.text(c - vec2(0.0, 2.0), Align2::CENTER_BOTTOM, format!("{drive:+.1}"), bold(14.0), t.text);
        p.text(c + vec2(0.0, 1.0), Align2::CENTER_TOP, "dB DRIVE", bold(8.0), t.text_dim);
    } else if r >= 14.0 {
        p.text(c, Align2::CENTER_CENTER, format!("{drive:+.0}"), bold(9.0), t.text);
    }

    let lane = Rect::from_min_max(pos2(rect.left() + 6.0, rect.top() + 4.0), pos2(gauge.left() - 8.0, rect.bottom() - 4.0));
    let label_w = if large { 26.0 } else { 0.0 };
    let bar_h = if large { 10.0 } else { ((lane.height() - 6.0) * 0.5).clamp(2.0, 7.0) };
    let gap = if large { 16.0 } else { 4.0 };
    let top = lane.center().y - bar_h - gap * 0.5;
    for (i, (name, db)) in [("IN", in_db), ("OUT", out_db)].into_iter().enumerate() {
        let y = top + i as f32 * (bar_h + gap);
        let bar = Rect::from_min_max(pos2(lane.left() + label_w, y), pos2(lane.right(), y + bar_h));
        p.rect_filled(bar, CornerRadius::same(2), t.graph_grid);
        let v = ((db - COMP_MIN_DB) / -COMP_MIN_DB).clamp(0.0, 1.0);
        if v > 0.0 {
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * v, bar.height())), CornerRadius::same(2), level_color(&t, v));
        }
        if large {
            p.text(pos2(lane.left(), bar.center().y), Align2::LEFT_CENTER, name, bold(8.5), t.text_dim);
            let txt = if db.is_finite() && db > -120.0 { format!("{db:.1}") } else { "−∞".to_string() };
            p.text(pos2(bar.right(), bar.bottom() + 1.0), Align2::RIGHT_TOP, txt, regular(8.5), t.text_dim);
        }
    }
    // Tape's wow and flutter, as its engine reports it.
    if large && ins.plugin == "fs_tape" && lv.readouts[3].is_finite() {
        p.text(pos2(lane.left(), rect.bottom() - 3.0), Align2::LEFT_BOTTOM, format!("W&F {:.2}%", lv.readouts[3]), regular(8.5), t.text_dim);
    }
}

/// Pitch's voice bars: the range they cover, dB (unity sits near the top, so a boosted voice
/// shows).
const PITCH_DB_FLOOR: f32 = -36.0;
const PITCH_DB_TOP: f32 = 6.0;

/// The level knobs Pitch's main row can hold (`PitchConfig.h` `mainRowFor`); its other knobs
/// (Detune, Pedal, Glide) are not levels.
fn pitch_voice_param(index: usize) -> bool {
    use soundcraft_dsp::plugins::fs::pitch::pitch_engine_dsp as pd;
    matches!(index, pd::P_SUB | pd::P_OCT_DOWN | pd::P_DRY | pd::P_OCT_UP | pd::P_OCT2_UP | pd::P_FIFTH | pd::P_VOICE)
}

/// A Pitch voice bar's caption: the style's name for the knob ("-2 Oct" on the OC-2, "Doubles" on
/// the H910), else the parameter's, with "Oct" dropped ("-1 Oct" reads "-1").
pub fn pitch_bar_label(style: i32, index: usize) -> String {
    use soundcraft_dsp::plugins::fs::pitch::{PITCH_PARAMS, pitch_config as pc};
    let name = pc::knob_label(style, index).or_else(|| PITCH_PARAMS.get(index).map(|p| p.name)).unwrap_or("");
    name.strip_suffix(" Oct").unwrap_or(name).to_uppercase()
}

/// One of Pitch's voice bars: the parameter, its caption, its level in dB (`-inf`: off), and
/// whether it is the dry.
pub type PitchBar = (usize, String, f32, bool);

/// Pitch's voice bars for a module, in the order of the style's main row. A shifted voice's level
/// is its knob times Level (the wet path's master); the dry is its own knob.
pub fn pitch_bars(ins: &Insert) -> Vec<PitchBar> {
    use soundcraft_dsp::plugins::fs::pitch::{PITCH_PARAMS, pitch_config as pc, pitch_engine_dsp as pd};
    let knob = |i: usize| PITCH_PARAMS.get(i).map_or(0.0, |p| param(ins, p.id));
    let style = knob(pd::P_STYLE).round() as i32;
    let level = pd::knob_to_gain(knob(pd::P_LEVEL));
    pc::main_row_for(style)
        .iter()
        .copied()
        .filter(|&i| pitch_voice_param(i))
        .map(|i| {
            let dry = i == pd::P_DRY;
            let g = pd::knob_to_gain(knob(i)) * if dry { 1.0 } else { level };
            let db = if g > 0.0 { 20.0 * g.log10() } else { f32::NEG_INFINITY };
            (i, pitch_bar_label(style, i), db, dry)
        })
        .collect()
}

/// A voice bar's level, whole dB, signed; unity reads "0", never "-0".
pub fn pitch_db_text(db: f32) -> String {
    let r = db.round();
    if r == 0.0 { "0".to_string() } else { format!("{r:+.0}") }
}

fn draw_pitch(p: &egui::Painter, rect: Rect, ins: &Insert, live: Option<&LiveValues>, large: bool) {
    use soundcraft_dsp::plugins::fs::pitch::{pitch_config as pc, pitch_engine_dsp as pd};
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let lv = live.copied().unwrap_or_else(no_reading);
    let style = param(ins, "style").round() as i32;

    // The input level at the right.
    let meter_w = if large { 30.0 } else { 6.0 };
    let foot = if large { 13.0 } else { 0.0 };
    let head = if large { 16.0 } else { 0.0 };
    let pad = if large { 6.0 } else { 2.0 };
    let meter =
        Rect::from_min_max(pos2(rect.right() - meter_w - pad * 0.5, rect.top() + pad + head), pos2(rect.right() - pad, rect.bottom() - pad - foot));
    let track = if large { Rect::from_center_size(meter.center(), vec2(8.0, meter.height())) } else { meter };
    p.rect_filled(track, CornerRadius::same(2), t.graph_grid);
    let in_db = if lv.readouts[0].is_finite() {
        lv.readouts[0]
    } else if lv.in_peak > 0.0 && lv.in_peak.is_finite() {
        20.0 * lv.in_peak.log10()
    } else {
        f32::NEG_INFINITY
    };
    let v = ((in_db - COMP_MIN_DB) / -COMP_MIN_DB).clamp(0.0, 1.0);
    if v > 0.0 {
        p.rect_filled(
            Rect::from_min_max(pos2(track.left(), track.bottom() - track.height() * v), track.max),
            CornerRadius::same(2),
            level_color(&t, v),
        );
    }
    if large {
        p.text(pos2(track.center().x, rect.bottom() - 1.0), Align2::CENTER_BOTTOM, "IN", bold(8.0), t.text_dim);
    }

    // The voice bars, unity marked.
    let area = Rect::from_min_max(pos2(rect.left() + pad, rect.top() + pad + head), pos2(meter.left() - pad * 1.5, rect.bottom() - pad - foot));
    let bars = pitch_bars(ins);
    let n = bars.len().max(1) as f32;
    let w = area.width() / n;
    let y_of = |db: f32| area.bottom() - area.height() * ((db - PITCH_DB_FLOOR) / (PITCH_DB_TOP - PITCH_DB_FLOOR)).clamp(0.0, 1.0);
    let unity = y_of(0.0);
    p.line_segment([pos2(area.left(), unity), pos2(area.right(), unity)], Stroke::new(1.0, t.graph_grid));
    for (k, (_, caption, db, dry)) in bars.iter().enumerate() {
        let x0 = area.left() + w * k as f32;
        let col = Rect::from_min_max(pos2(x0 + w * 0.18, area.top()), pos2(x0 + w * 0.82, area.bottom()));
        p.rect_filled(col, CornerRadius::same(2), t.graph_grid.gamma_multiply(0.5));
        // The dry in a quiet grey beside the voices' accent.
        let color = if *dry { t.text_dim.gamma_multiply(0.6) } else { accent };
        if db.is_finite() && *db > PITCH_DB_FLOOR {
            let top = y_of(*db);
            p.rect_filled(Rect::from_min_max(pos2(col.left(), top), col.max), CornerRadius::same(2), color);
            if large {
                p.text(pos2(col.center().x, top - 1.0), Align2::CENTER_BOTTOM, pitch_db_text(*db), regular(8.0), t.text);
            }
        }
        if large {
            p.text(pos2(col.center().x, rect.bottom() - 1.0), Align2::CENTER_BOTTOM, caption, bold(8.0), if *dry { t.text_dim } else { t.text });
        }
    }

    // What the style is doing, along the top: the OC-2's lock lamp, the Whammy's interval, else
    // how late the shifted voices are.
    if large {
        let at = pos2(rect.left() + pad, rect.top() + 3.0);
        if style == pd::STYLE_OC2 {
            let lit = lv.readouts[2] >= 0.5;
            p.circle_filled(at + vec2(4.0, 5.0), 3.5, if lit { accent } else { t.graph_grid });
            p.text(at + vec2(12.0, 0.0), Align2::LEFT_TOP, if lit { "TRACKING" } else { "TRACK" }, bold(8.0), if lit { t.text } else { t.text_dim });
        } else if style == pd::STYLE_WHAMMY {
            // The editor's footer readout (`whammyReadout`), heel to toe from the knobs.
            let interval = param(ins, "interval").round() as i32;
            p.text(at, Align2::LEFT_TOP, pc::whammy_readout(interval, param(ins, "pedal")), bold(8.0), t.text);
        } else {
            let ms = pd::latency_ms(style, param(ins, "grain"));
            p.text(at, Align2::LEFT_TOP, format!("VOICES +{ms:.0} ms"), bold(8.0), t.text_dim);
        }
    }
}

/// What Mod's graph shows, worked out from the module's parameters and its readouts: the LFO's
/// shape factor, its depth (0..1), its rate in Hz, the two sides' phases (`None`: no reading
/// yet) and the caption ("LFO 0.780 Hz", plus the division when synced).
#[derive(Debug, Clone, PartialEq)]
pub struct ModLfo {
    pub shape: f32,
    pub depth: f32,
    pub hz: f32,
    pub phase: Option<(f32, f32)>,
    pub caption: String,
}

/// Mod's LFO as the engine runs it: `readouts[0]` the left phase, `[1]` the rate, `[2]` the
/// shape factor; the parameters stand in for any reading missing, as the C++ editor's own rate
/// readout does (`lfoRateHz` from the parameters).
pub fn mod_lfo(ins: &Insert, live: Option<&LiveValues>) -> ModLfo {
    use soundcraft_dsp::plugins::fs::mod_engine::mod_engine_dsp as md;
    let lv = live.copied().unwrap_or_else(no_reading);
    let idx = |id: &str| param(ins, id).round() as i32;
    let (circuit, variant, sync, div) = (idx("circuit"), idx("variant"), idx("sync"), idx("division"));
    let shape = if lv.readouts[2].is_finite() { lv.readouts[2] } else { md::lfo_shape_for(circuit, variant, idx("shape")) };
    let hz = if lv.readouts[1].is_finite() && lv.readouts[1] > 0.0 {
        lv.readouts[1]
    } else {
        md::lfo_rate_hz(circuit, variant, param(ins, "rate"), sync, div, param(ins, "tempo"))
    };
    let phase = lv.readouts[0].is_finite().then(|| {
        let l = lv.readouts[0].rem_euclid(1.0);
        (l, (l + param(ins, "width") * 0.5).rem_euclid(1.0))
    });
    // The editor's readout: 3 decimals under 1 Hz, 2 under 10, else 1.
    let value = if hz >= 10.0 {
        format!("{hz:.1}")
    } else if hz >= 1.0 {
        format!("{hz:.2}")
    } else {
        format!("{hz:.3}")
    };
    let mut caption = format!("LFO {value} Hz");
    if sync == 1 {
        let divs = soundcraft_dsp::plugins::fs::mod_engine::mod_config::DIVISION_SWITCH.1;
        caption.push_str(&format!("  \u{b7}  {}", divs.get(div.clamp(0, 7) as usize).copied().unwrap_or("")));
    }
    ModLfo { shape, depth: param(ins, "depth"), hz, phase, caption }
}

fn draw_mod(p: &egui::Painter, rect: Rect, ins: &Insert, live: Option<&LiveValues>, large: bool) {
    use soundcraft_dsp::plugins::fs::mod_engine::mod_engine_dsp::lfo_value;
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let lfo = mod_lfo(ins, live);
    let head = if large { 16.0 } else { 0.0 };
    let pad = if large { 8.0 } else { 3.0 };
    let area = Rect::from_min_max(pos2(rect.left() + pad, rect.top() + pad + head), pos2(rect.right() - pad, rect.bottom() - pad));
    let mid = area.center().y;
    let half = area.height() * 0.5;
    // Depth scales the swing; a little always shows, so the shape still reads at Depth 0.
    let amp = 0.12 + 0.88 * lfo.depth.clamp(0.0, 1.0);
    let at = |ph: f32| pos2(area.left() + area.width() * ph, mid - lfo_value(ph, lfo.shape) * amp * half);
    p.line_segment([pos2(area.left(), mid), pos2(area.right(), mid)], Stroke::new(1.0, t.graph_grid));
    for y in [mid - half, mid + half] {
        p.line_segment([pos2(area.left(), y), pos2(area.right(), y)], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.5)));
    }
    let n = (area.width() as usize).clamp(16, 240);
    let pts: Vec<Pos2> = (0..=n).map(|i| at(i as f32 / n as f32)).collect();
    p.add(Shape::line(pts.clone(), Stroke::new(if large { 5.0 } else { 3.0 }, accent.gamma_multiply(0.14))));
    p.add(Shape::line(pts, Stroke::new(if large { 1.6 } else { 1.2 }, accent)));
    if let Some((l, r)) = lfo.phase {
        let rad = if large { 5.0 } else { 3.0 };
        // The right side first, so the left (the readout) is on top when Width puts them together.
        p.circle_stroke(at(r), rad, Stroke::new(1.5, t.text_dim));
        p.circle_filled(at(l), rad, t.text);
        p.circle_stroke(at(l), rad, Stroke::new(1.0, accent));
        if large {
            p.text(at(l) + vec2(0.0, -rad - 1.0), Align2::CENTER_BOTTOM, "L", bold(8.0), t.text);
            p.text(at(r) + vec2(0.0, rad + 1.0), Align2::CENTER_TOP, "R", bold(8.0), t.text_dim);
        }
    }
    if large {
        p.text(pos2(rect.left() + pad, rect.top() + 3.0), Align2::LEFT_TOP, &lfo.caption, bold(8.5), t.text);
        // Level follows Depth under Link: said beside Depth, as Dist's graph says it beside Level.
        let link = if param(ins, "link") >= 0.5 { " \u{b7} LINK" } else { "" };
        p.text(pos2(rect.right() - pad, rect.top() + 3.0), Align2::RIGHT_TOP, format!("DEPTH {:.0}{link}", lfo.depth * 10.0), bold(8.5), t.text_dim);
    }
}

/// What Delay's graph shows, worked out from the module's parameters and the engine's readouts:
/// the repeats of one impulse up to `window_ms` (the engine's head and pattern tables, its
/// routing and feedback law: `delay_config::echo_taps`), their decay envelope, and the captions.
#[derive(Debug, Clone, PartialEq)]
pub struct DelayView {
    pub taps: Vec<soundcraft_dsp::plugins::fs::delay::delay_config::Echo>,
    pub envelope: Vec<(f32, f32)>,
    pub window_ms: f32,
    /// Left caption: the style and the time ("Tape  350 ms", "Digital  1/4  500 ms").
    pub caption: String,
    /// Right caption: the feedback, or FREEZE.
    pub status: String,
}

/// The most repeats the graph draws.
const DELAY_TAPS_MAX: usize = 96;

/// Delay's taps as the engine runs them: the echo times are its readouts (`[0]`/`[1]`, which
/// include the tempo in force and the loop's own delay), else the same maps from the parameters
/// at the tempo it reported (`[2]`) or its Manual BPM.
pub fn delay_view(ins: &Insert, live: Option<&LiveValues>) -> DelayView {
    use soundcraft_dsp::plugins::fs::delay::{delay_config as dc, delay_engine_dsp as dd};
    let lv = live.copied().unwrap_or_else(no_reading);
    let idx = |id: &str| param(ins, id).round() as i32;
    let (style, routing, synced) = (idx("style"), idx("routing"), param(ins, "sync") >= 0.5);
    let ok = |v: f32| v.is_finite() && v > 0.0;
    let bpm = if ok(lv.readouts[2]) { lv.readouts[2] } else { dd::knob_to_bpm(param(ins, "tempo")) };
    let (pl, pr) =
        dc::delay_ms(param(ins, "time"), synced, idx("division"), routing, param(ins, "swing"), param(ins, "offset"), idx("divisionR"), bpm);
    let ms_l = if ok(lv.readouts[0]) { lv.readouts[0] } else { pl };
    let ms_r = if ok(lv.readouts[1]) { lv.readouts[1] } else { pr };
    // The loop's own delay: the engine's (`[7]`), else its geometry at 48 kHz.
    let loop_ms = if lv.readouts[7].is_finite() && lv.readouts[7] >= 0.0 && ok(lv.readouts[0]) {
        lv.readouts[7]
    } else {
        dc::loop_ms(48_000.0, style, param(ins, "character"), param(ins, "diffusion"), idx("pitch"))
    };
    let setup = dc::TapSetup {
        style,
        routing,
        heads: idx("heads"),
        pattern: idx("pattern"),
        reverse: param(ins, "reverse") >= 0.5,
        freeze: param(ins, "freeze") >= 0.5,
        feedback: param(ins, "feedback") * 1.15,
        ms_l,
        ms_r,
        loop_ms,
    };
    // A cycle: the longest head that is on, or a ping-pong round trip; five of them fit.
    let heads = if setup.pattern > 0 || setup.reverse { 1.0 } else { dd::head_table(dd::style_is_multi_head(style), style, setup.heads).max_ratio };
    let cycle = if routing == 2 { ms_l + ms_r } else { ms_l.max(ms_r) * heads };
    let window_ms = (cycle * 5.0).clamp(100.0, 10_000.0);
    let taps = dc::echo_taps(&setup, window_ms, DELAY_TAPS_MAX);
    let envelope = dc::decay_envelope(&taps);
    let name = dc::STYLES.get(style.clamp(0, 5) as usize).copied().unwrap_or("");
    let ms_text = |ms: f32| if ms >= 1000.0 { format!("{:.2} s", ms / 1000.0) } else { format!("{} ms", ms.round() as i32) };
    let time = if synced { format!("{}  {}", dc::DIVISIONS[idx("division").clamp(0, 15) as usize], ms_text(ms_l)) } else { ms_text(ms_l) };
    let mut caption = format!("{name}  {time}");
    if setup.reverse {
        caption.push_str("  REV");
    }
    let status = if setup.freeze { "FREEZE".to_string() } else { format!("FB {}%", (param(ins, "feedback") * 115.0).round() as i32) };
    DelayView { taps, envelope, window_ms, caption, status }
}

fn draw_delay(p: &egui::Painter, rect: Rect, ins: &Insert, live: Option<&LiveValues>, large: bool) {
    use soundcraft_dsp::plugins::fs::delay::delay_config::Side;
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let v = delay_view(ins, live);
    let head = if large { 16.0 } else { 0.0 };
    let pad = if large { 8.0 } else { 3.0 };
    let area = Rect::from_min_max(pos2(rect.left() + pad, rect.top() + pad + head), pos2(rect.right() - pad, rect.bottom() - pad));
    if area.width() < 4.0 || area.height() < 4.0 {
        return;
    }
    // Repeats on a dB scale (-48..0), so the feedback decay reads as a straight slope.
    const FLOOR_DB: f32 = -48.0;
    let x = |ms: f32| area.left() + area.width() * (ms / v.window_ms).clamp(0.0, 1.0);
    let h = |g: f32| {
        let db = if g > 0.0 { 20.0 * g.log10() } else { FLOOR_DB };
        area.height() * ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
    };
    for db in [-12.0f32, -24.0, -36.0] {
        let y = area.bottom() - area.height() * (db - FLOOR_DB) / -FLOOR_DB;
        p.line_segment([pos2(area.left(), y), pos2(area.right(), y)], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.5)));
    }
    p.line_segment([pos2(area.left(), area.bottom()), pos2(area.right(), area.bottom())], Stroke::new(1.0, t.graph_grid));
    // The dry impulse at 0.
    p.line_segment([pos2(area.left() + 1.0, area.bottom()), pos2(area.left() + 1.0, area.top())], Stroke::new(1.0, t.text_dim.gamma_multiply(0.5)));
    // The decay envelope over the repeats.
    if v.envelope.len() >= 2 {
        let pts: Vec<Pos2> = v.envelope.iter().map(|(ms, g)| pos2(x(*ms), area.bottom() - h(*g))).collect();
        p.add(Shape::line(pts.clone(), Stroke::new(if large { 5.0 } else { 3.0 }, accent.gamma_multiply(0.12))));
        p.add(Shape::line(pts, Stroke::new(1.0, accent.gamma_multiply(0.5))));
    }
    let w = if large { 3.0 } else { 2.0 };
    for e in &v.taps {
        let (dx, color) = match e.side {
            Side::Right => (w * 0.7, t.text_dim),
            _ => (0.0, accent),
        };
        let color = if e.pattern { color.gamma_multiply(0.7) } else { color };
        let xx = x(e.ms) + dx;
        let top = area.bottom() - h(e.gain);
        p.line_segment([pos2(xx, area.bottom()), pos2(xx, top)], Stroke::new(if e.pattern { w * 0.6 } else { w }, color));
        if large && !e.pattern {
            p.circle_filled(pos2(xx, top), w * 0.8, color);
        }
    }
    if large {
        p.text(pos2(rect.left() + pad, rect.top() + 3.0), Align2::LEFT_TOP, &v.caption, bold(8.5), t.text);
        // The status, and the span the time axis covers.
        let ms = v.window_ms;
        let span = if ms >= 1000.0 { format!("{:.1} s", ms / 1000.0) } else { format!("{} ms", ms.round() as i32) };
        p.text(pos2(rect.right() - pad, rect.top() + 3.0), Align2::RIGHT_TOP, format!("{}  \u{b7}  {span}", v.status), bold(8.5), t.text_dim);
    }
}

/// What Reverb's graph shows, worked out from the module's parameters through the engine's own maps
/// (`reverb_config::decay_shape`) and its readouts: the decay per band, the machine running, and the
/// captions.
#[derive(Debug, Clone, PartialEq)]
pub struct ReverbDecay {
    pub shape: soundcraft_dsp::plugins::fs::reverb::reverb_config::DecayShape,
    pub machine: i32,
    /// "Hall · RT60 2.5 s" (or "· FREEZE").
    pub caption: String,
    /// "PRE 20 ms" plus the band ratios: "LOW ×1.00 · HI ×0.35 > 6.00 k".
    pub detail: String,
}

/// Reverb's decay as the engine is set: `readouts[2]` the pre-delay in force (ms; a synced one
/// follows the host tempo), `[4]` the machine running, `[5]` the freeze amount; the parameters
/// stand in for any reading missing (a synced pre-delay then at the Manual BPM, the engine's own
/// fallback without a host tempo).
pub fn reverb_decay(ins: &Insert, live: Option<&LiveValues>) -> ReverbDecay {
    use soundcraft_dsp::plugins::fs::reverb::{reverb_config as rc, reverb_engine_dsp as rd};
    let lv = live.copied().unwrap_or_else(no_reading);
    let idx = |id: &str| param(ins, id).round() as i32;
    let ok = |v: f32, lo: f32, hi: f32| (v.is_finite() && v >= lo && v <= hi).then_some(v);
    let machine = ok(lv.readouts[4], 0.0, 5.0).map_or(idx("machine"), |m| m.round() as i32).clamp(0, 5);
    let frozen = ok(lv.readouts[5], 0.0, 1.0).map_or(param(ins, "freeze") >= 0.5, |f| f >= 0.5);
    let pre_ms = ok(lv.readouts[2], 0.0, 10_000.0).unwrap_or_else(|| {
        if idx("sync") == 1 {
            rd::division_beats(idx("division")) * 60_000.0 / rd::knob_to_bpm(param(ins, "tempo"))
        } else {
            rd::knob_to_pre_ms(param(ins, "preDelay"))
        }
    });
    let shape = rc::decay_shape(machine, param(ins, "decay"), param(ins, "bass"), param(ins, "damping"), idx("color"), frozen, pre_ms * 0.001);
    let name = rc::MACHINES.get(machine as usize).copied().unwrap_or("");
    let caption = if frozen {
        format!("{name}  \u{b7}  FREEZE")
    } else if machine == rc::SHIMMER {
        // The shifter re-enters the loop, so the shifted voices ring on past the set RT60.
        format!("{name}  \u{b7}  RT60 {} + shift tail", rc::format_seconds(shape.t60_mid))
    } else {
        format!("{name}  \u{b7}  RT60 {}", rc::format_seconds(shape.t60_mid))
    };
    let pre = format!("PRE {pre_ms:.0} ms");
    let detail = if machine == rc::SPRING {
        pre
    } else {
        format!(
            "{pre}  \u{b7}  LOW \u{d7}{:.2}  \u{b7}  HI \u{d7}{:.2} > {}",
            shape.t60_low / shape.t60_mid.max(1e-6),
            shape.t60_high / shape.t60_mid.max(1e-6),
            rc::format_kilohertz(f64::from(shape.hi_hz))
        )
    };
    ReverbDecay { shape, machine, caption, detail }
}

/// The decay graph's floor, dB.
const REVERB_FLOOR_DB: f32 = -60.0;

/// Reverb: each band's level after the note (0 dB falling 60 dB per its RT60) over time, starting
/// after the pre-delay (shaded); the mids in the accent, the lows and highs thinner. Freeze holds
/// them flat. Thin in/out level bars at the right.
fn draw_reverb(p: &egui::Painter, rect: Rect, ins: &Insert, live: Option<&LiveValues>, large: bool) {
    use soundcraft_dsp::plugins::fs::reverb::reverb_config as rc;
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let rv = reverb_decay(ins, live);
    let s = rv.shape;
    let lv = live.copied().unwrap_or_else(no_reading);
    let lin_db = |pk: f32| if pk > 0.0 && pk.is_finite() { 20.0 * pk.log10() } else { f32::NEG_INFINITY };
    let in_db = if lv.readouts[0].is_finite() { lv.readouts[0] } else { lin_db(lv.in_peak) };
    let out_db = if lv.readouts[1].is_finite() { lv.readouts[1] } else { lin_db(lv.out_peak) };

    let head = if large { 16.0 } else { 0.0 };
    let foot = if large { 13.0 } else { 0.0 };
    let pad = if large { 8.0 } else { 3.0 };
    let bars_w = if large { 22.0 } else { 9.0 };
    let area = Rect::from_min_max(pos2(rect.left() + pad, rect.top() + pad + head), pos2(rect.right() - pad - bars_w, rect.bottom() - pad - foot));
    if area.width() < 4.0 || area.height() < 4.0 {
        return;
    }
    // The time window: the slowest band's RT60 after the pre-delay, with a little room.
    let ring = s.t60_low.max(s.t60_mid).max(s.t60_high);
    let span = ((s.pre_s + ring) * 1.08).clamp(0.2, 90.0);
    let x = |sec: f32| area.left() + area.width() * (sec / span).clamp(0.0, 1.0);
    // 0 dB sits a little under the top, so a frozen (flat) tail stays visible against the frame.
    let top = area.top() + 3.0;
    let y = |db: f32| top + (area.bottom() - top) * (db / REVERB_FLOOR_DB).clamp(0.0, 1.0);
    for db in [-20.0, -40.0] {
        p.line_segment([pos2(area.left(), y(db)), pos2(area.right(), y(db))], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.6)));
    }
    p.line_segment([pos2(area.left(), area.bottom()), pos2(area.right(), area.bottom())], Stroke::new(1.0, t.graph_grid));
    // The pre-delay: no tail yet.
    let x0 = x(s.pre_s);
    if x0 > area.left() + 0.5 {
        p.rect_filled(Rect::from_min_max(area.min, pos2(x0, area.bottom())), CornerRadius::ZERO, t.graph_grid.gamma_multiply(0.35));
    }
    // One band: from 0 dB at the pre-delay to the floor at pre + RT60 (flat when frozen).
    let band_end = |t60: f32| if s.frozen { pos2(area.right(), y(0.0)) } else { pos2(x(s.pre_s + t60), y(REVERB_FLOOR_DB)) };
    let start = pos2(x0, y(0.0));
    let mid_end = band_end(s.t60_mid);
    // The mids' envelope, filled under the line.
    let fill = if s.frozen {
        vec![start, mid_end, pos2(area.right(), area.bottom()), pos2(x0, area.bottom())]
    } else {
        vec![start, mid_end, pos2(x0, area.bottom())]
    };
    p.add(Shape::convex_polygon(fill, accent.gamma_multiply(0.13), Stroke::NONE));
    if rv.machine != rc::SPRING && !s.frozen {
        let (lo_end, hi_end) = (band_end(s.t60_low), band_end(s.t60_high));
        for end in [lo_end, hi_end] {
            p.line_segment([start, end], Stroke::new(if large { 1.2 } else { 1.0 }, t.text_dim.gamma_multiply(0.8)));
        }
        if large {
            // Each label just left of where its band reaches the floor; the earlier one steps up a
            // line when the two would collide.
            let close = (lo_end.x - hi_end.x).abs() < 26.0;
            for (end, label) in [(lo_end, "LOW"), (hi_end, "HI")] {
                let lift = if close && end.x <= lo_end.x.min(hi_end.x) { 11.0 } else { 0.0 };
                p.text(pos2(end.x - 3.0, end.y - 2.0 - lift), Align2::RIGHT_BOTTOM, label, bold(7.5), t.text_dim);
            }
        }
    }
    glow_line(p, vec![start, mid_end], accent, if large { 2.0 } else { 1.5 });
    if s.frozen && large {
        p.text(pos2(area.right() - 2.0, y(0.0) + 3.0), Align2::RIGHT_TOP, "FREEZE", bold(8.0), accent);
    }

    // In and out levels.
    let col_w = if large { 7.0 } else { 3.0 };
    let gap = if large { 4.0 } else { 2.0 };
    for (i, db) in [in_db, out_db].into_iter().enumerate() {
        let x1 = rect.right() - pad - (1 - i) as f32 * (col_w + gap);
        let col = Rect::from_min_max(pos2(x1 - col_w, area.top()), pos2(x1, area.bottom()));
        p.rect_filled(col, CornerRadius::same(1), t.graph_grid);
        let v = ((db - COMP_MIN_DB) / -COMP_MIN_DB).clamp(0.0, 1.0);
        if v > 0.0 {
            p.rect_filled(Rect::from_min_max(pos2(col.left(), col.bottom() - col.height() * v), col.max), CornerRadius::same(1), level_color(&t, v));
        }
        if large {
            p.text(pos2(col.center().x, rect.bottom() - 2.0), Align2::CENTER_BOTTOM, if i == 0 { "I" } else { "O" }, bold(7.5), t.text_dim);
        }
    }
    if large {
        p.text(pos2(rect.left() + pad, rect.top() + 3.0), Align2::LEFT_TOP, &rv.caption, bold(8.5), accent);
        let foot_clip = Rect::from_min_max(pos2(rect.left(), rect.bottom() - foot - 2.0), pos2(rect.right() - pad - bars_w, rect.bottom()));
        p.with_clip_rect(foot_clip).text(pos2(rect.left() + pad, rect.bottom() - 2.0), Align2::LEFT_BOTTOM, &rv.detail, regular(8.0), t.text_dim);
        p.text(pos2(area.right(), rect.top() + 3.0), Align2::RIGHT_TOP, rc::format_seconds(span), regular(8.0), t.text_dim);
    }
}

/// Dist: the pedal's name over its in/out levels, and a gauge with Drive (outer arc, in the
/// accent) and Level (inner arc) on the dials' 0..10 scale, from the engine's smoothed values
/// (`readouts` 2..4: drive knob, level gain, circuit), else the stored params.
fn draw_dist(p: &egui::Painter, rect: Rect, ins: &Insert, live: Option<&LiveValues>, large: bool) {
    use soundcraft_dsp::plugins::fs::dist::wrapper;
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let lv = live.copied().unwrap_or_else(no_reading);
    let lin_db = |pk: f32| if pk > 0.0 && pk.is_finite() { 20.0 * pk.log10() } else { f32::NEG_INFINITY };
    let in_db = if lv.readouts[0].is_finite() { lv.readouts[0] } else { lin_db(lv.in_peak) };
    let out_db = if lv.readouts[1].is_finite() { lv.readouts[1] } else { lin_db(lv.out_peak) };
    let unit = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let drive = unit(if lv.readouts[2].is_finite() { lv.readouts[2] } else { param(ins, "drive") });
    let level = unit(if lv.readouts[3].is_finite() && lv.readouts[3] >= 0.0 { lv.readouts[3].sqrt() } else { param(ins, "level") });
    let circuit = if lv.readouts[4].is_finite() { lv.readouts[4] } else { param(ins, "circuit") };
    let def = wrapper::circuit(if circuit.is_finite() { circuit.round() as i32 } else { 0 });

    let d = (rect.height() - if large { 8.0 } else { 4.0 }).min(rect.width() * 0.42).max(8.0);
    let gauge = Rect::from_center_size(pos2(rect.right() - d * 0.5 - 4.0, rect.center().y + d * 0.06), vec2(d, d));
    let c = gauge.center();
    let r = d * 0.42;
    let (a0, a1) = (-2.1f32, 2.1f32);
    let w = (r * 0.2).clamp(2.0, 6.0);
    let ri = (r - w * 1.9).max(2.0);
    let wi = (w * 0.55).max(1.5);
    p.add(Shape::line(arc(c, r, a0, a1), Stroke::new(w, t.graph_grid)));
    p.add(Shape::line(arc(c, ri, a0, a1), Stroke::new(wi, t.graph_grid)));
    if drive > 0.0 {
        let av = a0 + (a1 - a0) * drive;
        p.add(Shape::line(arc(c, r, a0, av), Stroke::new(w * 2.6, accent.gamma_multiply(0.16))));
        p.add(Shape::line(arc(c, r, a0, av), Stroke::new(w, accent)));
        p.circle_filled(c + vec2(av.sin(), -av.cos()) * r, w * 0.75, t.text);
    }
    if level > 0.0 {
        let av = a0 + (a1 - a0) * level;
        p.add(Shape::line(arc(c, ri, a0, av), Stroke::new(wi, t.text_dim)));
    }
    if large {
        p.text(c - vec2(0.0, 2.0), Align2::CENTER_BOTTOM, format!("{:.1}", drive * 10.0), bold(14.0), t.text);
        p.text(c + vec2(0.0, 1.0), Align2::CENTER_TOP, def.dial_drive.to_uppercase(), bold(8.0), t.text_dim);
    } else if r >= 14.0 {
        p.text(c, Align2::CENTER_CENTER, format!("{:.0}", drive * 10.0), bold(9.0), t.text);
    }

    let lane = Rect::from_min_max(pos2(rect.left() + 6.0, rect.top() + 4.0), pos2(gauge.left() - 8.0, rect.bottom() - 4.0));
    // The pedal's name, over the levels when there is room for both.
    let name_h = if large {
        16.0
    } else if lane.height() >= 26.0 {
        11.0
    } else {
        0.0
    };
    if name_h > 0.0 {
        let font = if large { bold(11.0) } else { bold(8.5) };
        p.with_clip_rect(lane).text(lane.left_top(), Align2::LEFT_TOP, def.name, font, accent);
    }
    let lane = Rect::from_min_max(pos2(lane.left(), lane.top() + name_h), lane.max);
    let label_w = if large { 26.0 } else { 0.0 };
    let bar_h = if large { 10.0 } else { ((lane.height() - 6.0) * 0.5).clamp(2.0, 7.0) };
    let gap = if large { 14.0 } else { 4.0 };
    let top = lane.center().y - bar_h - gap * 0.5;
    for (i, (name, db)) in [("IN", in_db), ("OUT", out_db)].into_iter().enumerate() {
        let y = top + i as f32 * (bar_h + gap);
        let bar = Rect::from_min_max(pos2(lane.left() + label_w, y), pos2(lane.right(), y + bar_h));
        p.rect_filled(bar, CornerRadius::same(2), t.graph_grid);
        let v = ((db - COMP_MIN_DB) / -COMP_MIN_DB).clamp(0.0, 1.0);
        if v > 0.0 {
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * v, bar.height())), CornerRadius::same(2), level_color(&t, v));
        }
        if large {
            p.text(pos2(lane.left(), bar.center().y), Align2::LEFT_CENTER, name, bold(8.5), t.text_dim);
            let txt = if db.is_finite() && db > -120.0 { format!("{db:.1}") } else { "\u{2212}\u{221e}".to_string() };
            p.text(pos2(bar.right(), bar.bottom() + 1.0), Align2::RIGHT_TOP, txt, regular(8.5), t.text_dim);
        }
    }
    if large {
        let link = if param(ins, "link") >= 0.5 { " \u{b7} LINK" } else { "" };
        let txt = format!("{} {:.1}{link}", def.dial_level.to_uppercase(), level * 10.0);
        p.text(pos2(lane.left(), rect.bottom() - 3.0), Align2::LEFT_BOTTOM, txt, regular(8.5), t.text_dim);
    }
}

/// Transient's gain lane, top to bottom (`TransientScope`: +18 at the top, -24 at the bottom), as
/// 0..1 down the lane; a nonsense gain reads 0 dB.
pub fn transient_lane_t(db: f32) -> f32 {
    let db = if db.is_finite() { db } else { 0.0 };
    ((18.0 - db) / 42.0).clamp(0.0, 1.0)
}

/// Transient's signal trace: a linear peak as 0..1 up the wave area, -60 dBFS at the floor.
pub fn transient_wave_t(peak: f32) -> f32 {
    let db = if peak > 1.0e-6 { 20.0 * peak.log10() } else { -120.0 };
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// The C++ scope's per-pixel reduction of its last `shown` bins into `w` columns: the loudest
/// peaks and the most extreme gains of the bins under each column. Bins older than the history
/// are empty (the C++ ring starts zeroed).
pub fn transient_columns(bins: &VecDeque<[f32; TRANSIENT_VALUES]>, shown: usize, w: usize) -> Vec<[f32; TRANSIENT_VALUES]> {
    let len = bins.len();
    (0..w)
        .map(|px| {
            let from = (px as f32 / w as f32 * shown as f32) as usize;
            let to = ((px + 1) as f32 / w as f32 * shown as f32).max((from + 1) as f32) as usize;
            let mut c = [0.0f32; TRANSIENT_VALUES];
            for k in from..to {
                let Some(b) = (len + k).checked_sub(shown).and_then(|i| bins.get(i)) else { continue };
                c[0] = c[0].max(b[0]);
                c[1] = c[1].max(b[1]);
                if b[2].abs() > c[2].abs() {
                    c[2] = b[2];
                }
                if b[3].abs() > c[3].abs() {
                    c[3] = b[3];
                }
            }
            c
        })
        .collect()
}

/// The Filter graph's level scale: this far above and below 0 dB.
const FILTER_DB_TOP: f32 = 24.0;
const FILTER_DB_FLOOR: f32 = -36.0;

/// The parameters the Filter graph's knob-side values depend on (the circuit, its tuning
/// switches, and the knobs `updateShared` reads), in the cache key's order.
const FILTER_DISPLAY_KEYS: [&str; 11] =
    ["circuit", "cutoff", "reso", "drive", "qtRange", "muRange", "cbInductor", "cbVoice", "voxVoice", "tbMod", "moogComp"];

/// What the Filter graph needs from the engine, from the knobs alone: where they put the cutoff
/// (Hz) and the small-signal constants the engine derives from Reso and Drive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterDisplay {
    pub knob_hz: f32,
    pub coefs: soundcraft_dsp::plugins::fs::filter::filter_engine_dsp::LinearCoefs,
}

/// [`FilterDisplay`] for a module: the engine's own `updateChannel` and `updateShared`, run on a
/// scratch engine with this module's circuit, switches and knobs.
pub fn filter_display(ins: &Insert) -> FilterDisplay {
    use soundcraft_dsp::plugins::fs::filter::{FILTER_PARAMS, FilterEngineDsp, filter_engine_dsp as fd};
    let mut e = FilterEngineDsp::new();
    for i in [fd::P_CIRCUIT, fd::P_QT_RANGE, fd::P_MU_RANGE, fd::P_CB_INDUCTOR, fd::P_CB_VOICE, fd::P_VOX_VOICE, fd::P_TB_MOD, fd::P_MOOG_COMP] {
        if let Some(info) = FILTER_PARAMS.get(i) {
            e.set_parameter(i, param(ins, info.id));
        }
    }
    let coefs = e.linear_coefs(param(ins, "reso"), param(ins, "drive"));
    FilterDisplay { knob_hz: e.position_to_hz(f64::from(param(ins, "cutoff"))) as f32, coefs }
}

/// [`filter_display`], cached per module and recomputed only when one of
/// [`FILTER_DISPLAY_KEYS`] changes (no scratch engine per frame).
pub fn filter_display_cached(ctx: &egui::Context, ins: &Insert) -> FilterDisplay {
    let key: [u32; 11] = FILTER_DISPLAY_KEYS.map(|k| param(ins, k).to_bits());
    let id = Id::new(("fs-filter-display", ins.id.0));
    if let Some((k, d)) = ctx.data(|m| m.get_temp::<([u32; 11], FilterDisplay)>(id))
        && k == key
    {
        return d;
    }
    let d = filter_display(ins);
    ctx.data_mut(|m| m.insert_temp(id, (key, d)));
    d
}

/// Where the Filter's knobs put the cutoff with no modulation, in Hz (the engine's own map).
pub fn filter_knob_hz(ins: &Insert) -> f32 {
    filter_display(ins).knob_hz
}

/// A small complex number for the Filter graph's response.
#[derive(Clone, Copy)]
struct Cx(f64, f64);

impl Cx {
    const ONE: Cx = Cx(1.0, 0.0);
    fn add(self, o: Cx) -> Cx {
        Cx(self.0 + o.0, self.1 + o.1)
    }
    fn mul(self, o: Cx) -> Cx {
        Cx(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn scale(self, k: f64) -> Cx {
        Cx(self.0 * k, self.1 * k)
    }
    fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
    fn pow(self, n: u32) -> Cx {
        (0..n).fold(Cx::ONE, |a, _| a.mul(self))
    }
}

/// Whether the Filter graph draws a circuit's response to scale. The wahs (a transistor stage
/// around an inductor) and the TB-303's coupled diode ladder have no simple linear form here, so
/// they show a neutral shape (bandpass, 24 dB lowpass) at the live cutoff with a separate
/// resonance gauge, and no level scale.
pub fn filter_curve_is_to_scale(ins: &Insert) -> bool {
    !matches!(param(ins, "circuit").round() as i32, 2 | 3 | 7)
}

/// The Filter graph's curve, in dB at `freqs`, with the corner at `fc`. For the state-variable
/// circuits, the Moog ladder and the MS-20 it is the small-signal response of the engine's own
/// solve (the analog prototype its trapezoidal integrators discretise, corner per stage at `fc`):
/// - SVF: `c.lp = 1/D`, `c.bp = s/D`, `c.hp = s^2/D` with `D = s^2 + d s + 1`, `d = svfDamp`,
///   the input scaled by `svfInComp`; Q-Tron Mix is `(lp + bp) / 2`, SEM/Wasp Notch `lp + hp`;
/// - Moog: tap `n` is `(1+s)^(4-n) / ((1+s)^4 + k)` with `k = ladK`, input `ladInComp`; the loop
///   self-oscillates at k = 4, so the drawing holds k below 3.98;
/// - MS-20 (Korg35, diode off): LP `1/(s^2 + (2-K) s + 1)`, HP `s(1+s)/(same)`, HP+LP the HP
///   (corner fc/4, K*0.8) into the LP; K = msK, held below 1.98 (it oscillates at 2);
///
/// times the linear gain the core sees (`inV driveG outV / driveComp`) and blended with the dry
/// by Mix exactly as the engine mixes. The wahs and the TB-303 draw a neutral shape (see
/// [`filter_curve_is_to_scale`]). Saturation, rails and the diode limiter are not drawn.
pub fn filter_response_db(ins: &Insert, disp: &FilterDisplay, fc: f32, freqs: &[f32]) -> Vec<f32> {
    let circuit = param(ins, "circuit").round() as i32;
    let mix = f64::from(param(ins, "mix"));
    let mode = |id: &str| param(ins, id).round() as i32;
    let c = disp.coefs;
    let fc = if fc.is_finite() && fc > 0.0 { f64::from(fc) } else { 1000.0 };
    let svf = |s: Cx, m: i32| {
        let den = s.mul(s).add(s.scale(c.svf_damp)).add(Cx::ONE);
        let num = match m {
            0 => Cx::ONE,
            1 => s,
            2 => s.mul(s),
            3 => s.mul(s).add(Cx::ONE),     // Notch: lp + hp
            _ => Cx::ONE.add(s).scale(0.5), // Q-Tron Mix: (lp + bp) / 2
        };
        c.svf_in_comp * num.abs() / den.abs().max(1e-12)
    };
    let ladder = |s: Cx, taps: u32| {
        let one_s = Cx::ONE.add(s);
        let den = one_s.pow(4).add(Cx(c.lad_k.min(3.98), 0.0));
        c.lad_in_comp * one_s.pow(4 - taps.min(4)).abs() / den.abs().max(1e-12)
    };
    let ms_den = |s: Cx, k: f64| s.mul(s).add(s.scale(2.0 - k.min(1.98))).add(Cx::ONE);
    freqs
        .iter()
        .map(|&f| {
            let s = Cx(0.0, f64::from(f) / fc);
            let wet = match circuit {
                0 => svf(s, if mode("qtMode") == 3 { 4 } else { mode("qtMode") }),
                1 => svf(s, mode("muMode")),
                // Neutral shapes: a unit bandpass, a plain four-pole lowpass.
                2 | 3 => s.abs() / s.mul(s).add(s).add(Cx::ONE).abs().max(1e-12),
                7 => 1.0 / Cx::ONE.add(s).pow(4).abs().max(1e-12),
                4 => ladder(s, 4 - mode("moogSlope").clamp(0, 3) as u32),
                5 => {
                    let k = c.ms_k;
                    let lp = |s: Cx| 1.0 / ms_den(s, k).abs().max(1e-12);
                    let hp = |s: Cx, k: f64| s.mul(Cx::ONE.add(s)).abs() / ms_den(s, k).abs().max(1e-12);
                    match mode("msMode") {
                        0 => lp(s),
                        1 => hp(s, k),
                        _ => hp(s.scale(4.0), k * 0.8) * lp(s),
                    }
                }
                6 => svf(s, mode("semMode")),
                _ => svf(s, mode("waspMode")),
            };
            // The engine's mix: dry at unity, wet through the core's linear gain.
            let gain = if filter_curve_is_to_scale(ins) { c.gain } else { 1.0 };
            let blend = (mix * wet * gain + (1.0 - mix)) as f32;
            if blend > 0.0 && blend.is_finite() { 20.0 * blend.log10() } else { FILTER_DB_FLOOR }
        })
        .collect()
}

/// Transient's live gain bars cover ±this many dB.
const TRANSIENT_BAR_DB: f32 = 24.0;

fn draw_transient(p: &egui::Painter, rect: Rect, ins: &Insert, live: Option<&LiveValues>, streams: Option<&Streams>, large: bool) {
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    // The sustain in the body text colour (softened, so a long cut does not drown the attack
    // spikes on the light palette), the attack in the accent (`TransientScope`).
    let sus_color = t.text.gamma_multiply(0.6);
    let lv = live.copied().unwrap_or_else(no_reading);
    let pad = if large { 6.0 } else { 2.0 };
    let tall = rect.height() >= 40.0;
    let bars_w = if large {
        58.0
    } else if tall {
        12.0
    } else {
        0.0
    };
    let head = if large { 14.0 } else { 0.0 };
    let body = Rect::from_min_max(pos2(rect.left() + pad, rect.top() + pad + head), pos2(rect.right() - bars_w - pad, rect.bottom() - pad));
    if !(body.width() >= 2.0 && body.height() >= 2.0) {
        return;
    }
    // The wave over the lane; a slim tile shows the lane alone.
    let (wave, lane) = if tall {
        let lane_h = (body.height() * if large { 0.42 } else { 0.45 }).max(10.0);
        let gap = if large { 6.0 } else { 2.0 };
        (
            Some(Rect::from_min_max(body.min, pos2(body.right(), body.bottom() - lane_h - gap))),
            Rect::from_min_max(pos2(body.left(), body.bottom() - lane_h), body.max),
        )
    } else {
        (None, body)
    };
    let panel = t.graph_grid.gamma_multiply(0.35);
    if let Some(wv) = wave {
        p.rect_filled(wv, CornerRadius::same(2), panel);
    }
    p.rect_filled(lane, CornerRadius::same(2), panel);
    let y_lane = |db: f32| lane.top() + lane.height() * transient_lane_t(db);
    let zero = y_lane(0.0);
    p.line_segment([pos2(lane.left(), zero), pos2(lane.right(), zero)], Stroke::new(1.0, t.graph_grid));

    let empty = VecDeque::new();
    let bins = streams.map_or(&empty, |s| &s.bins);
    let w = (body.width() as usize).clamp(1, 2000);
    let cols = transient_columns(bins, TRANSIENT_BINS, w);
    let dx = body.width() / w as f32;
    let mut out_line = Vec::with_capacity(w);
    for (px, c) in cols.iter().enumerate() {
        let x = body.left() + (px as f32 + 0.5) * dx;
        if let Some(wv) = wave {
            let y_wave = |pk: f32| wv.bottom() - wv.height() * transient_wave_t(pk);
            let yi = y_wave(c[0]);
            if yi < wv.bottom() {
                p.line_segment([pos2(x, wv.bottom()), pos2(x, yi)], Stroke::new(dx.max(1.0), t.text_dim.gamma_multiply(0.35)));
            }
            out_line.push(pos2(x, y_wave(c[1])));
        }
        if c[3].abs() > 0.05 {
            p.line_segment([pos2(x, zero), pos2(x, y_lane(c[3]))], Stroke::new(dx.max(1.0), sus_color));
        }
        if c[2].abs() > 0.05 {
            p.line_segment([pos2(x, zero), pos2(x, y_lane(c[2]))], Stroke::new(dx.max(1.0), accent.gamma_multiply(0.9)));
        }
    }
    if out_line.len() > 1 {
        p.add(Shape::line(out_line, Stroke::new(if large { 1.3 } else { 1.0 }, accent)));
    }

    if large {
        let f = regular(8.0);
        if let Some(wv) = wave {
            p.text(pos2(wv.right() - 3.0, wv.top() + 1.0), Align2::RIGHT_TOP, "0 dB", f.clone(), t.text_dim);
            p.text(pos2(wv.right() - 3.0, wv.bottom() - 1.0), Align2::RIGHT_BOTTOM, "-60", f.clone(), t.text_dim);
        }
        p.text(pos2(lane.right() - 3.0, lane.top() + 1.0), Align2::RIGHT_TOP, "+18", f.clone(), t.text_dim);
        p.text(pos2(lane.right() - 3.0, lane.bottom() - 1.0), Align2::RIGHT_BOTTOM, "-24", f, t.text_dim);
        // The legend along the top: the signal at the left, the two gains at the right.
        let y = rect.top() + pad;
        p.text(pos2(body.left(), y), Align2::LEFT_TOP, "SIGNAL  in / out", bold(8.0), t.text_dim);
        let mut x = body.right();
        for (name, color) in [("SUSTAIN", sus_color), ("ATTACK", accent)] {
            let g = p.layout_no_wrap(name.to_string(), bold(8.0), t.text_dim);
            x -= g.size().x;
            p.galley(pos2(x, y), g, t.text_dim);
            x -= 12.0;
            p.rect_filled(Rect::from_min_size(pos2(x, y + 1.5), vec2(8.0, 8.0)), CornerRadius::same(2), color);
            x -= 10.0;
        }
    }

    // The live gains: the last block's most extreme attack and sustain, about a zero line.
    if bars_w > 0.0 {
        let area = Rect::from_min_max(pos2(rect.right() - bars_w, rect.top() + pad + head), pos2(rect.right() - pad, rect.bottom() - pad));
        let foot = if large { 24.0 } else { 0.0 };
        let span = Rect::from_min_max(area.min, pos2(area.right(), area.bottom() - foot));
        let mid = span.center().y;
        let cw = area.width() / 2.0;
        for (i, (name, db, color)) in [("ATT", lv.readouts[4], accent), ("SUS", lv.readouts[5], sus_color)].into_iter().enumerate() {
            let cx = area.left() + cw * (i as f32 + 0.5);
            let half = if large { 4.0 } else { 2.0 };
            let track = Rect::from_min_max(pos2(cx - half, span.top()), pos2(cx + half, span.bottom()));
            p.rect_filled(track, CornerRadius::same(2), t.graph_grid);
            p.line_segment([pos2(track.left() - 2.0, mid), pos2(track.right() + 2.0, mid)], Stroke::new(1.0, t.text_dim));
            let v = if db.is_finite() { (db / TRANSIENT_BAR_DB).clamp(-1.0, 1.0) } else { 0.0 };
            if v != 0.0 {
                let y = mid - v * span.height() * 0.5;
                p.rect_filled(Rect::from_min_max(pos2(track.left(), y.min(mid)), pos2(track.right(), y.max(mid))), CornerRadius::same(1), color);
            }
            if large {
                let text = if db.is_finite() && db.abs() >= 0.05 { format!("{db:+.1}") } else { "0".to_string() };
                p.text(pos2(cx, area.bottom() - 11.0), Align2::CENTER_BOTTOM, text, regular(8.0), t.text);
                p.text(pos2(cx, area.bottom()), Align2::CENTER_BOTTOM, name, bold(8.0), t.text_dim);
            }
        }
    }
}

fn draw_filter(p: &egui::Painter, rect: Rect, ins: &Insert, live: Option<&LiveValues>, large: bool) {
    use soundcraft_dsp::plugins::fs::filter::{FILTER_PARAMS, filter_config as fcfg, filter_engine_dsp as fd};
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let lv = live.copied().unwrap_or_else(no_reading);
    let circuit = param(ins, "circuit").round() as i32;
    let src = param(ins, "modSrc").round() as i32;
    let steps_on = src == 3;
    let to_scale = filter_curve_is_to_scale(ins);
    let pad = if large { 4.0 } else { 1.0 };
    let head = if large { 15.0 } else { 0.0 };
    let row_h = match (steps_on, large) {
        (false, _) => 0.0,
        (true, true) => 20.0,
        (true, false) => 6.0,
    };
    let foot = if large { 11.0 } else { 0.0 };
    // The resonance gauge of the neutral-shape circuits, at the right.
    let gauge_w = if to_scale {
        0.0
    } else if large {
        14.0
    } else {
        4.0
    };
    let plot = Rect::from_min_max(
        pos2(rect.left() + pad, rect.top() + pad + head),
        pos2(rect.right() - pad - gauge_w - if gauge_w > 0.0 { pad } else { 0.0 }, rect.bottom() - pad - foot - row_h),
    );
    if plot.height() < 4.0 || plot.width() < 8.0 {
        return;
    }
    let y_of = |db: f32| {
        let db = if db.is_finite() { db.clamp(FILTER_DB_FLOOR, FILTER_DB_TOP) } else { FILTER_DB_FLOOR };
        plot.bottom() - plot.height() * (db - FILTER_DB_FLOOR) / (FILTER_DB_TOP - FILTER_DB_FLOOR)
    };
    // Grid: the decades, and 0 dB where the curve is to scale.
    for (f, name) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        let x = freq_to_x(plot, f);
        p.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, t.graph_grid));
        if large {
            p.text(pos2(x + 2.0, rect.bottom() - 1.0), Align2::LEFT_BOTTOM, name, regular(8.0), t.text_dim);
        }
    }
    if to_scale {
        let zero = y_of(0.0);
        p.line_segment([pos2(plot.left(), zero), pos2(plot.right(), zero)], Stroke::new(1.0, t.graph_grid));
    }

    // The live cutoff (the engine's own report), else where the knob puts it.
    let disp = filter_display_cached(p.ctx(), ins);
    let live_hz = lv.readouts[2];
    let fc = if live_hz.is_finite() && live_hz > 0.0 { live_hz } else { disp.knob_hz };
    let n = (plot.width().max(2.0) as usize).clamp(2, 160);
    let freqs: Vec<f32> = (0..n).map(|i| x_to_freq(plot, plot.left() + plot.width() * i as f32 / (n - 1) as f32)).collect();
    let resp = filter_response_db(ins, &disp, fc, &freqs);
    let pts: Vec<Pos2> = freqs.iter().zip(&resp).map(|(f, db)| pos2(freq_to_x(plot, *f), y_of(*db))).collect();
    // The fill under the curve, as one mesh.
    let fill = accent.gamma_multiply(0.18);
    let mut mesh = egui::Mesh::default();
    for (i, q) in pts.iter().enumerate() {
        mesh.colored_vertex(*q, fill);
        mesh.colored_vertex(pos2(q.x, plot.bottom()), fill);
        if i > 0 {
            let b = (2 * i) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(pts, Stroke::new(1.5, accent)));
    // The knob's own position (a dim tick) and the live cutoff (the accent line).
    let kx = freq_to_x(plot, disp.knob_hz);
    p.line_segment([pos2(kx, plot.bottom() - 5.0), pos2(kx, plot.bottom())], Stroke::new(2.0, t.text_dim));
    let lx = freq_to_x(plot, fc);
    p.extend(Shape::dashed_line(&[pos2(lx, plot.top()), pos2(lx, plot.bottom())], Stroke::new(1.0, accent), 3.0, 2.0));

    // The resonance gauge (neutral-shape circuits): the Reso knob as a bar.
    if !to_scale {
        let g = Rect::from_min_max(pos2(plot.right() + pad, plot.top()), pos2(plot.right() + pad + gauge_w, plot.bottom()));
        let bar = if large { Rect::from_center_size(g.center(), vec2(5.0, g.height())) } else { g };
        p.rect_filled(bar, CornerRadius::same(2), t.graph_grid);
        let r = param(ins, "reso").clamp(0.0, 1.0);
        p.rect_filled(Rect::from_min_max(pos2(bar.left(), bar.bottom() - bar.height() * r), bar.max), CornerRadius::same(2), accent);
        if large {
            p.text(pos2(g.center().x, rect.bottom() - 1.0), Align2::CENTER_BOTTOM, "RES", bold(7.0), t.text_dim);
            p.text(pos2(plot.left() + 2.0, plot.top() + 1.0), Align2::LEFT_TOP, "shape", regular(8.0), t.text_dim);
        }
    }

    // The step row: one bar per step, the current one lit.
    if steps_on {
        let count = fd::knob_to_step_count(param(ins, "seqSteps"));
        let cur = if lv.readouts[5].is_finite() { lv.readouts[5].round() as i32 } else { -1 };
        let row = Rect::from_min_max(pos2(plot.left(), plot.bottom() + 2.0), pos2(plot.right(), plot.bottom() + row_h));
        let w = row.width() / count.max(1) as f32;
        for k in 0..count {
            let v = FILTER_PARAMS.get(fd::P_STEP1 + k as usize).map_or(0.0, |info| param(ins, info.id));
            let cell =
                Rect::from_min_max(pos2(row.left() + w * k as f32 + 0.5, row.top()), pos2(row.left() + w * (k + 1) as f32 - 0.5, row.bottom()));
            p.rect_filled(cell, CornerRadius::same(1), t.graph_grid.gamma_multiply(0.6));
            let h = cell.height() * v.clamp(0.0, 1.0);
            let color = if k == cur { accent } else { accent.gamma_multiply(0.45) };
            p.rect_filled(Rect::from_min_max(pos2(cell.left(), cell.bottom() - h), cell.max), CornerRadius::same(1), color);
        }
    }

    if large {
        let c = fcfg::circuit(circuit);
        // The circuit and its mode (or slope), as the editor's switch rows name them.
        let mode = c.switches.iter().find(|s| s.label == "Mode" || s.label == "Slope").and_then(|s| {
            let info = FILTER_PARAMS.get(s.param)?;
            info.choices.get(param(ins, info.id).round().max(0.0) as usize).copied()
        });
        let title = match mode {
            Some(m) => format!("{}  \u{b7}  {m}", c.name),
            None => c.name.to_string(),
        };
        p.text(pos2(rect.left() + pad + 1.0, rect.top() + 2.0), Align2::LEFT_TOP, title, bold(8.5), t.text);
        let note = FILTER_PARAMS.get(fd::P_NOTE).and_then(|i| i.choices.get(param(ins, "note").round().max(0.0) as usize)).copied().unwrap_or("");
        let source = match src {
            1 => "ENV".to_string(),
            2 if param(ins, "lfoSync") >= 0.5 => format!("LFO {note}"),
            2 => format!("LFO {:.2} Hz", fd::knob_to_lfo_hz(param(ins, "lfoRate"))),
            3 => format!("STEPS {note}"),
            _ => "MANUAL".to_string(),
        };
        p.text(
            pos2(rect.right() - pad - 1.0, rect.top() + 2.0),
            Align2::RIGHT_TOP,
            format!("{source}   {}", fcfg::format_cutoff_hz(fc)),
            bold(8.5),
            accent,
        );
    }
}

/// Twine's GR bars cover this many dB.
const TWINE_GR_RANGE: f32 = 12.0;

fn draw_twine(p: &egui::Painter, rect: Rect, ins: &Insert, live: Option<&LiveValues>, streams: Option<&Streams>, large: bool) {
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let bars_w = if large { 48.0 } else { 12.0 };
    let scope = Rect::from_min_max(rect.min, pos2(rect.max.x - bars_w, rect.max.y));
    let mid = scope.center().y;
    // Full scale sits a little inside the frame, so overs show.
    let half = (scope.height() * 0.5 - 2.0) / 1.12;
    let y = |a: f32| mid - a.clamp(-1.12, 1.12) * half;
    p.line_segment([pos2(scope.left(), mid), pos2(scope.right(), mid)], Stroke::new(1.0, t.graph_grid));
    let ceil_db = soundcraft_dsp::plugins::fs::twine::knob_to_ceiling_db(param(ins, "ceiling"));
    let ceil = 10f32.powf(ceil_db / 20.0);

    if let Some(cols) = streams.map(|s| &s.scope).filter(|c| !c.is_empty()) {
        let w = scope.width() / SCOPE_COLS as f32;
        let n = cols.len();
        let body = Stroke::new(w.max(1.0), accent.gamma_multiply(0.75));
        let hot = Stroke::new(w.max(1.0), t.meter_red);
        for (i, (lo, hi)) in cols.iter().enumerate() {
            let x = scope.right() - (n - 1 - i) as f32 * w - w * 0.5;
            let (lo, hi) = (lo.clamp(-1.12, 1.12), hi.clamp(-1.12, 1.12));
            p.line_segment([pos2(x, y(hi.min(ceil))), pos2(x, y(lo.max(-ceil)))], body);
            if hi > ceil {
                p.line_segment([pos2(x, y(hi)), pos2(x, y(ceil))], hot);
            }
            if lo < -ceil {
                p.line_segment([pos2(x, y(-ceil)), pos2(x, y(lo))], hot);
            }
        }
    }
    for a in [ceil, -ceil] {
        let yy = y(a);
        p.extend(Shape::dashed_line(&[pos2(scope.left(), yy), pos2(scope.right(), yy)], Stroke::new(1.0, accent), 4.0, 3.0));
    }
    if large {
        p.text(pos2(scope.right() - 4.0, y(ceil) - 2.0), Align2::RIGHT_BOTTOM, format!("{ceil_db:.1} dBTP"), bold(8.5), accent);
        // The scope shows what goes in (the C++ editor's scope source).
        p.text(scope.min + vec2(5.0, 4.0), Align2::LEFT_TOP, "input", regular(8.5), t.text_dim);
    }
    let lv = live.copied().unwrap_or_else(no_reading);
    let bars = [("CLIP", lv.readouts[2] / TWINE_GR_RANGE, true, accent), ("LIM", lv.readouts[3] / TWINE_GR_RANGE, true, t.gr_color)];
    hanging_bars(p, Rect::from_min_max(pos2(scope.right() + 1.0, rect.top()), rect.max), &bars, large);
}

/// Sift's cut lanes: this many dB fills a lane.
pub const SIFT_RANGE: f32 = 18.0;
/// The colour of Sift's plosive cuts (its sibilance cuts wear the module's accent).
pub const SIFT_PLOSIVE: Color32 = Color32::from_rgb(0x8F, 0xB7, 0xE5);

/// Sift's timeline in `rect`: the input scrolling by (dim), the sibilance cuts hanging from the
/// top in `accent` and the plosive cuts standing on the bottom in [`SIFT_PLOSIVE`].
pub(crate) fn sift_timeline(p: &egui::Painter, rect: Rect, streams: Option<&Streams>, accent: Color32, large: bool) {
    let t = Tokens::current();
    let mid = rect.center().y;
    let half = rect.height() * 0.5 - 2.0;
    p.line_segment([pos2(rect.left(), mid), pos2(rect.right(), mid)], Stroke::new(1.0, t.graph_grid));
    let Some(s) = streams else { return };
    let w = rect.width() / SCOPE_COLS as f32;
    let x_of = |i: usize, n: usize| rect.right() - (n - 1 - i) as f32 * w - w * 0.5;
    let wave = Stroke::new(w.max(1.0), t.text_dim.gamma_multiply(0.7));
    let n = s.scope.len();
    for (i, (lo, hi)) in s.scope.iter().enumerate() {
        let x = x_of(i, n);
        let (lo, hi) = (lo.clamp(-1.0, 1.0), hi.clamp(-1.0, 1.0));
        p.line_segment([pos2(x, mid - hi * half), pos2(x, mid - lo * half)], wave);
    }
    // A lane is 45 % of the height: the two never meet.
    let lane = rect.height() * 0.45;
    let n = s.sift.len();
    for (i, (sib, plo)) in s.sift.iter().enumerate() {
        let x = x_of(i, n);
        let h = |db: f32| (db / SIFT_RANGE).clamp(0.0, 1.0) * lane;
        if *sib > 0.05 {
            p.line_segment([pos2(x, rect.top() + 1.0), pos2(x, rect.top() + 1.0 + h(*sib))], Stroke::new(w.max(1.0), accent));
        }
        if *plo > 0.05 {
            p.line_segment([pos2(x, rect.bottom() - 1.0), pos2(x, rect.bottom() - 1.0 - h(*plo))], Stroke::new(w.max(1.0), SIFT_PLOSIVE));
        }
    }
    if large {
        p.text(rect.min + vec2(5.0, 4.0), Align2::LEFT_TOP, "sibilance", regular(8.5), accent);
        p.text(pos2(rect.left() + 5.0, rect.bottom() - 4.0), Align2::LEFT_BOTTOM, "plosives", regular(8.5), SIFT_PLOSIVE);
    }
}

/// Manners' curve scale, ±dB.
pub const MANNERS_RANGE: f32 = 12.0;

/// Manners: the gain it applies over frequency, as its engine last reported it. Cuts hang under the
/// 0 dB line and lifts stand on it; outside the working range the graph is dimmed.
fn draw_manners(p: &egui::Painter, rect: Rect, ins: &Insert, streams: Option<&Streams>, large: bool) {
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let zero = db_y(rect, 0.0, MANNERS_RANGE);
    for (f, name) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        let x = freq_to_x(rect, f);
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, t.graph_grid));
        if large {
            p.text(pos2(x + 2.0, rect.bottom() - 1.0), Align2::LEFT_BOTTOM, name, regular(9.0), t.text_dim);
        }
    }
    for db in [-6.0, 6.0] {
        let y = db_y(rect, db, MANNERS_RANGE);
        p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, t.graph_grid));
        if large {
            p.text(pos2(rect.left() + 2.0, y), Align2::LEFT_BOTTOM, format!("{db:+.0}"), regular(9.0), t.text_dim);
        }
    }
    p.line_segment([pos2(rect.left(), zero), pos2(rect.right(), zero)], Stroke::new(1.0, t.text_dim));
    // The working range: what lies outside it is left alone.
    let (lo, hi) = (param(ins, "rangeLo"), param(ins, "rangeHi"));
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let shade = t.graph_bg.gamma_multiply(0.55);
    if lo > F_MIN * 1.02 {
        p.rect_filled(Rect::from_min_max(rect.min, pos2(freq_to_x(rect, lo), rect.bottom())), 0.0, shade);
    }
    if hi < F_MAX * 0.98 {
        p.rect_filled(Rect::from_min_max(pos2(freq_to_x(rect, hi), rect.top()), rect.max), 0.0, shade);
    }
    let Some(curve) = streams.map(|s| &s.manners).filter(|c| c.len() >= 2) else { return };
    let pts: Vec<Pos2> = curve.iter().map(|(hz, db)| pos2(freq_to_x(rect, *hz), db_y(rect, *db, MANNERS_RANGE))).collect();
    for w in pts.windows(2) {
        if let [a, b] = w {
            // Split where the curve crosses 0 dB, so each piece is convex and one colour.
            let fill = |a: Pos2, b: Pos2| {
                let color = if (a.y + b.y) * 0.5 > zero { t.gr_color.gamma_multiply(0.35) } else { accent.gamma_multiply(0.35) };
                p.add(Shape::convex_polygon(vec![a, b, pos2(b.x, zero), pos2(a.x, zero)], color, Stroke::NONE));
            };
            if (a.y - zero) * (b.y - zero) < 0.0 {
                let k = (zero - a.y) / (b.y - a.y);
                let mid = pos2(a.x + (b.x - a.x) * k, zero);
                fill(*a, mid);
                fill(mid, *b);
            } else {
                fill(*a, *b);
            }
        }
    }
    glow_line(p, pts, accent, if large { 1.5 } else { 1.0 });
}

/// Vint's curve scale, ±dB.
pub const VINT_RANGE: f32 = 24.0;
/// The least time between two computes of one module's Vint curve, seconds.
pub const VINT_BUDGET_S: f64 = 0.1;

/// Vint's values in parameter order, each clamped, a missing one at its default.
pub fn vint_values(ins: &Insert) -> Vec<f32> {
    VINT_PARAMS.iter().map(|p| p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default))).collect()
}

/// A graph's Vint curve: the curve shown and the hash of what it was computed from, and the hash
/// of the inputs last asked of the worker (while that is not in yet).
#[derive(Clone, Default)]
struct VintCurve {
    hash: u64,
    curve: Option<std::sync::Arc<Vec<f32>>>,
    asked: Option<u64>,
}

/// A module's Vint compute budget: when its curve was last asked for and by which graph, and a
/// graph of it held back since (with when it last asked), which goes next.
#[derive(Clone, Copy)]
struct VintBudget {
    last: f64,
    by: Id,
    waiting: Option<(Id, f64)>,
}

/// A held-back graph that has not asked for this long (closed, or resized away) gives up its turn.
const VINT_WAIT_FORGET_S: f64 = 0.5;

/// The worker that computes Vint curves off the UI thread, one per context.
fn vint_worker(ctx: &egui::Context) -> std::sync::Arc<CurveWorker> {
    let id = Id::new("vint-curve-worker");
    if let Some(w) = ctx.data(|d| d.get_temp::<std::sync::Arc<CurveWorker>>(id)) {
        return w;
    }
    let repaint = ctx.clone();
    let w = std::sync::Arc::new(CurveWorker::new(
        Box::new(|v, sr, f| {
            let mut out = vec![0.0f32; f.len()];
            vint::frequency_response(v, sr, f, &mut out);
            out
        }),
        Box::new(move || repaint.request_repaint()),
    ));
    ctx.data_mut(|d| d.insert_temp(id, std::sync::Arc::clone(&w)));
    w
}

/// Vint's curve (dB) at `freqs` for the graph `key` (a module's graph at one width; one entry,
/// replaced when its inputs change). Computing it runs whole Vints, so it is computed off the UI
/// thread ([`CurveWorker`]) and a module (`module`, shared by its tile and its drawer) asks for at
/// most one per [`VINT_BUDGET_S`]. Until the new curve is in, a graph shows its last one (`None`
/// before its first). Graphs take turns, so a drawer is not starved by its tile while a knob is
/// dragged. Never blocks.
fn vint_curve(ctx: &egui::Context, key: Id, module: Id, values: &[f32], freqs: &[f32], sr: f32) -> Option<std::sync::Arc<Vec<f32>>> {
    let now = ctx.input(|i| i.time);
    vint_curve_with(ctx, &vint_worker(ctx), key, module, now, values, freqs, sr)
}

/// [`vint_curve`] at time `now`, with `worker`.
#[allow(clippy::too_many_arguments)]
fn vint_curve_with(
    ctx: &egui::Context,
    worker: &CurveWorker,
    key: Id,
    module: Id,
    now: f64,
    values: &[f32],
    freqs: &[f32],
    sr: f32,
) -> Option<std::sync::Arc<Vec<f32>>> {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    values.iter().chain(freqs).chain([&sr]).for_each(|v| v.to_bits().hash(&mut h));
    let hash = h.finish();
    let budget_id = Id::new(("vint-budget", module));
    let mut budget = ctx.data(|d| d.get_temp::<VintBudget>(budget_id));
    let mut entry = ctx.data(|d| d.get_temp::<VintCurve>(key)).unwrap_or_default();
    let pick_up = |entry: &mut VintCurve| {
        if let Some((h, c)) = worker.take(key) {
            entry.hash = h;
            entry.curve = Some(c);
            if entry.asked == Some(h) {
                entry.asked = None;
            }
        }
    };
    pick_up(&mut entry);
    let store = |entry: &VintCurve| ctx.data_mut(|d| d.insert_temp(key, entry.clone()));
    if entry.curve.is_some() && entry.hash == hash {
        // Up to date: no longer waiting for a turn.
        if let Some(b) = budget.as_mut().filter(|b| b.waiting.is_some_and(|(w, _)| w == key)) {
            b.waiting = None;
            let b = *b;
            ctx.data_mut(|d| d.insert_temp(budget_id, b));
        }
        store(&entry);
        return entry.curve;
    }
    if entry.asked == Some(hash) {
        // On its way.
        store(&entry);
        return entry.curve;
    }
    if let Some(mut b) = budget {
        let since = now - b.last;
        // A clock that went backwards (a new context) does not hold the curve back.
        let spent = (0.0..VINT_BUDGET_S).contains(&since);
        let other_waits = b.waiting.is_some_and(|(w, seen)| w != key && (0.0..=VINT_WAIT_FORGET_S).contains(&(now - seen)));
        if spent || (b.by == key && other_waits) {
            if !other_waits {
                b.waiting = Some((key, now));
            }
            ctx.data_mut(|d| d.insert_temp(budget_id, b));
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(if spent { VINT_BUDGET_S - since } else { 0.0 }));
            store(&entry);
            return entry.curve;
        }
    }
    worker.request(key, hash, values, sr, freqs);
    entry.asked = Some(hash);
    let waiting = budget.and_then(|b| b.waiting).filter(|(w, _)| *w != key);
    ctx.data_mut(|d| d.insert_temp(budget_id, VintBudget { last: now, by: key, waiting }));
    // An inline worker (wasm32) has it already.
    pick_up(&mut entry);
    store(&entry);
    entry.curve
}

fn draw_vint(ui: &Ui, p: &egui::Painter, rect: Rect, ins: &Insert, module: Id, sample_rate: f32, large: bool) {
    let t = Tokens::current();
    let accent = module_accent(&ins.plugin);
    let sr = sane_sr(sample_rate);
    let tiny = rect.height() < 30.0;
    // A wide graph (the VST-sized editor) takes the editor's full scale: 20 Hz..20 kHz in 1-2-5
    // steps and every 6 dB.
    let wide = rect.width() > 500.0;
    let fticks: &[(f32, &str)] = if wide {
        &[
            (20.0, "20"),
            (50.0, "50"),
            (100.0, "100"),
            (200.0, "200"),
            (500.0, "500"),
            (1000.0, "1k"),
            (2000.0, "2k"),
            (5000.0, "5k"),
            (10_000.0, "10k"),
            (20_000.0, "20k"),
        ]
    } else {
        &[(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")]
    };
    for &(f, name) in fticks {
        let x = freq_to_x(rect, f);
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, t.graph_grid));
        if large {
            p.text(pos2(x + 2.0, rect.bottom() - 1.0), Align2::LEFT_BOTTOM, name, regular(if wide { 10.0 } else { 9.0 }), t.text_dim);
        }
    }
    if !tiny {
        let dbs: &[f32] = if wide { &[-18.0, -12.0, -6.0, 6.0, 12.0, 18.0] } else { &[-12.0, 12.0] };
        for &db in dbs {
            let y = db_y(rect, db, VINT_RANGE);
            p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.7)));
            if large {
                p.text(pos2(rect.left() + 3.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:+.0}"), regular(8.5), t.text_dim);
            }
        }
    }
    let zero = db_y(rect, 0.0, VINT_RANGE);
    p.line_segment([pos2(rect.left(), zero), pos2(rect.right(), zero)], Stroke::new(1.0, t.graph_grid));

    let values = vint_values(ins);
    let n = ((rect.width() / 1.5).max(2.0) as usize).clamp(2, 240);
    let freqs: Vec<f32> = (0..n).map(|i| x_to_freq(rect, rect.left() + rect.width() * i as f32 / (n - 1) as f32)).collect();
    if let Some(curve) = vint_curve(ui.ctx(), Id::new(("vint-curve", ui.id(), ins.id.0, n)), module, &values, &freqs, sr) {
        let pts: Vec<Pos2> = freqs.iter().zip(curve.iter()).map(|(f, db)| pos2(freq_to_x(rect, *f), db_y(rect, *db, VINT_RANGE))).collect();
        for w in pts.windows(2) {
            if let [a, b] = w {
                p.add(Shape::convex_polygon(vec![*a, *b, pos2(b.x, zero), pos2(a.x, zero)], accent.gamma_multiply(0.16), Stroke::NONE));
            }
        }
        glow_line(p, pts, accent, if large { 2.0 } else { 1.5 });
    }
    if large {
        let circuit =
            soundcraft_dsp::plugin_info(&ins.plugin).and_then(|i| i.param("circuit")).map(|c| c.format(param(ins, "circuit"))).unwrap_or_default();
        p.text(rect.min + vec2(5.0, 4.0), Align2::LEFT_TOP, circuit, bold(9.0), accent);
    }
}

/// The amp's knobs in parameter order, as its tone curve takes them.
pub fn amp_values(ins: &Insert) -> Vec<f32> {
    soundcraft_dsp::plugins::fs::amp::AMP_PARAMS.iter().map(|p| p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default))).collect()
}

/// The worker that computes Amp tone curves off the UI thread, one per context.
fn amp_worker(ctx: &egui::Context) -> std::sync::Arc<CurveWorker> {
    let id = Id::new("amp-curve-worker");
    if let Some(w) = ctx.data(|d| d.get_temp::<std::sync::Arc<CurveWorker>>(id)) {
        return w;
    }
    let repaint = ctx.clone();
    let w = std::sync::Arc::new(CurveWorker::new(
        Box::new(|v, sr, f| {
            let mut out = vec![0.0f32; f.len()];
            soundcraft_dsp::plugins::fs::amp::tone_curve(v, sr, f, &mut out);
            out
        }),
        Box::new(move || repaint.request_repaint()),
    ));
    ctx.data_mut(|d| d.insert_temp(id, std::sync::Arc::clone(&w)));
    w
}

/// The span of the tone curve drawn, dB under its peak.
pub const AMP_RANGE: f32 = 30.0;

/// Amp: the tone stack of the amp in use (its engine's own network, solved in the frequency domain
/// on the curve worker; drawn relative to its peak), the in/out levels at the right, and in the
/// drawer the watts, the sag and the cabinet in use.
#[allow(clippy::too_many_arguments)]
fn draw_amp(ui: &Ui, p: &egui::Painter, rect: Rect, ins: &Insert, module: Id, live: Option<&LiveValues>, sample_rate: f32, large: bool) {
    use soundcraft_dsp::plugins::fs::amp::AMP_NAMES;
    let t = Tokens::current();
    let accent = crate::theme::module_accent_for(ins);
    let lv = live.copied().unwrap_or_else(no_reading);
    let sr = sane_sr(sample_rate);
    let meters_w = if large { 30.0 } else { 9.0 };
    let plot = rect.with_max_x(rect.max.x - meters_w);
    for (f, name) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        let x = freq_to_x(plot, f);
        p.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, t.graph_grid));
        if large {
            p.text(pos2(x + 2.0, plot.bottom() - 1.0), Align2::LEFT_BOTTOM, name, regular(9.0), t.text_dim);
        }
    }
    let y_of = |db: f32| {
        let top = plot.top() + if large { 18.0 } else { 3.0 };
        let bottom = plot.bottom() - if large { 12.0 } else { 2.0 };
        top + (bottom - top) * (-db / AMP_RANGE).clamp(0.0, 1.0)
    };
    if large {
        for db in [-10.0f32, -20.0] {
            let y = y_of(db);
            p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, t.graph_grid.gamma_multiply(0.7)));
            p.text(pos2(plot.left() + 3.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:.0}"), regular(8.5), t.text_dim);
        }
    }
    let values = amp_values(ins);
    let n = ((plot.width() / 1.5).max(2.0) as usize).clamp(2, 240);
    let freqs: Vec<f32> = (0..n).map(|i| x_to_freq(plot, plot.left() + plot.width() * i as f32 / (n - 1) as f32)).collect();
    let now = ui.input(|i| i.time);
    let key = Id::new(("amp-curve", ui.id(), ins.id.0, n));
    if let Some(curve) = vint_curve_with(ui.ctx(), &amp_worker(ui.ctx()), key, module, now, &values, &freqs, sr) {
        let peak = curve.iter().copied().filter(|v| v.is_finite()).fold(f32::MIN, f32::max);
        let base = plot.bottom();
        let pts: Vec<Pos2> = freqs.iter().zip(curve.iter()).map(|(f, db)| pos2(freq_to_x(plot, *f), y_of(db - peak))).collect();
        for w in pts.windows(2) {
            if let [a, b] = w {
                p.add(Shape::convex_polygon(vec![*a, *b, pos2(b.x, base), pos2(a.x, base)], accent.gamma_multiply(0.14), Stroke::NONE));
            }
        }
        glow_line(p, pts, accent, if large { 2.0 } else { 1.5 });
    }

    // the levels, at the right
    let lin_db = |pk: f32| if pk > 0.0 && pk.is_finite() { 20.0 * pk.log10() } else { f32::NEG_INFINITY };
    let in_db = if lv.readouts[0].is_finite() { lv.readouts[0] } else { lin_db(lv.in_peak) };
    let out_db = if lv.readouts[1].is_finite() { lv.readouts[1] } else { lin_db(lv.out_peak) };
    let lane =
        Rect::from_min_max(pos2(plot.right() + 3.0, rect.top() + 3.0), pos2(rect.right() - 2.0, rect.bottom() - if large { 12.0 } else { 3.0 }));
    let bw = ((lane.width() - 2.0) * 0.5).max(1.0);
    for (i, (name, db)) in [("IN", in_db), ("OUT", out_db)].into_iter().enumerate() {
        let bar = Rect::from_min_size(pos2(lane.left() + i as f32 * (bw + 2.0), lane.top()), vec2(bw, lane.height()));
        p.rect_filled(bar, CornerRadius::same(2), t.graph_grid);
        let v = ((db - COMP_MIN_DB) / -COMP_MIN_DB).clamp(0.0, 1.0);
        if v > 0.0 {
            p.rect_filled(Rect::from_min_max(pos2(bar.left(), bar.bottom() - bar.height() * v), bar.max), CornerRadius::same(2), level_color(&t, v));
        }
        if large {
            p.text(pos2(bar.center().x, rect.bottom() - 1.0), Align2::CENTER_BOTTOM, name, regular(7.5), t.text_dim);
        }
    }

    if large {
        let which = crate::module_drawer::amp_of(ins);
        let amp = AMP_NAMES.get(which).copied().unwrap_or("");
        // The AC30's Normal input skips the Top Boost stack (see `tone_curve`).
        let curve = match (which, param(ins, "acChannel").round() as i32) {
            (soundcraft_dsp::plugins::fs::amp::amp_engine_dsp::AMP_AC30, 1) => "NORMAL · CUT",
            (soundcraft_dsp::plugins::fs::amp::amp_engine_dsp::AMP_AC30, 2) => "BOTH · MIXER",
            _ => "TONE STACK",
        };
        p.text(plot.min + vec2(5.0, 3.0), Align2::LEFT_TOP, format!("{} · {curve}", amp.to_uppercase()), bold(9.0), accent);
        let cab = ["built-in cabinet", "IR cabinet", "no cabinet"]
            .into_iter()
            .enumerate()
            .find(|(i, _)| lv.readouts[5] == *i as f32)
            .map_or("", |(_, c)| c);
        let mut info = String::new();
        if lv.readouts[2].is_finite() && lv.readouts[3].is_finite() {
            info = format!("{:.1} W · sag {:.0} V", lv.readouts[2].max(0.0), lv.readouts[3].max(0.0));
        }
        if !cab.is_empty() {
            info = if info.is_empty() { cab.to_string() } else { format!("{info} · {cab}") };
        }
        p.text(pos2(plot.right() - 4.0, plot.top() + 3.0), Align2::RIGHT_TOP, info, regular(8.5), t.text_dim);
    }
}

#[cfg(test)]
mod amp_graph_tests {
    use super::*;

    #[test]
    fn amp_values_follow_the_module_and_the_curve_is_finite() {
        let mut ins = Insert::new("fs_amp");
        ins.params.insert("amp".into(), 5.0);
        ins.params.insert("acTreble".into(), f32::NAN);
        let v = amp_values(&ins);
        assert_eq!(v.len(), soundcraft_dsp::plugins::fs::amp::NUM_PARAMS);
        assert_eq!(v[0], 5.0);
        assert!(v.iter().all(|x| x.is_finite()), "NaN becomes the default");
        let freqs: Vec<f32> = (0..32).map(|i| 30.0 * 600f32.powf(i as f32 / 31.0)).collect();
        let mut out = vec![0.0; freqs.len()];
        soundcraft_dsp::plugins::fs::amp::tone_curve(&v, 48_000.0, &freqs, &mut out);
        assert!(out.iter().all(|x| x.is_finite()));
    }
}

#[cfg(test)]
mod filter_graph_tests {
    use super::*;

    #[test]
    fn filter_curve_follows_circuit_and_mode() {
        let mut ins = Insert::new("fs_filter");
        let freqs = [50.0, 1000.0, 15_000.0];
        // Q-Tron lowpass at 1 kHz: the lows pass, the top is gone, the peak sits at the corner.
        let lp = filter_response_db(&ins, &filter_display(&ins), 1000.0, &freqs);
        // (The passband sits at svfInComp * inV * outV = 1.4 / sqrt(2.9): -1.7 dB at mid Peak.)
        assert!(lp[0].abs() < 3.0 && lp[2] < -30.0 && lp[1] > lp[0] + 6.0, "{lp:?}");
        ins.params.insert("qtMode".into(), 2.0);
        let hp = filter_response_db(&ins, &filter_display(&ins), 1000.0, &freqs);
        assert!(hp[0] < -30.0 && hp[2].abs() < 3.0, "{hp:?}");
        // Mix at 0: dry, flat.
        ins.params.insert("mix".into(), 0.0);
        assert!(filter_response_db(&ins, &filter_display(&ins), 1000.0, &freqs).iter().all(|d| d.abs() < 1e-3));
        // Every circuit and mode stays finite, whatever the stored values.
        for circuit in 0..9 {
            for m in 0..4 {
                let mut ins = Insert::new("fs_filter");
                ins.params.insert("circuit".into(), circuit as f32);
                for id in ["qtMode", "muMode", "msMode", "semMode", "waspMode", "moogSlope", "tbMod"] {
                    ins.params.insert(id.into(), m as f32);
                }
                ins.params.insert("reso".into(), 1.0);
                for fc in [10.0, 1000.0, 22_000.0, f32::NAN] {
                    assert!(filter_response_db(&ins, &filter_display(&ins), fc, &freqs).iter().all(|d| d.is_finite()), "circuit {circuit} mode {m}");
                }
                assert!(filter_knob_hz(&ins).is_finite());
            }
        }
    }

    /// The drawn peak in dB (the curve's maximum over a fine sweep around the corner).
    fn peak_db(ins: &Insert) -> f32 {
        let freqs: Vec<f32> = (0..2000).map(|i| 1000.0 * 2f32.powf((i as f32 - 1000.0) / 500.0)).collect();
        filter_response_db(ins, &filter_display(ins), 1000.0, &freqs).into_iter().fold(f32::MIN, f32::max)
    }

    #[test]
    fn filter_curve_peaks_follow_the_engines_linear_response() {
        // Q-Tron at full Peak: c.lp and c.bp have the same peak, Q * svfInComp = sqrt(14), times
        // the core's linear gain inV * outV = 1.4 -- whichever mode is picked.
        let mut ins = Insert::new("fs_filter");
        ins.params.insert("reso".into(), 1.0);
        let want = 20.0 * (14.0f32.sqrt() * 1.4).log10();
        let lp = peak_db(&ins);
        ins.params.insert("qtMode".into(), 1.0);
        let bp = peak_db(&ins);
        assert!((lp - bp).abs() < 1.0, "LP {lp} vs BP {bp}");
        assert!((bp - want).abs() < 0.5, "BP {bp} vs {want}");
        ins.params.insert("qtMode".into(), 3.0);
        // Mix (lp + bp) / 2 at the corner: |1 + j| / 2 * Q, times the same comp and gain.
        let mix_want = 20.0 * (2.0f32.sqrt() / 2.0 * 14.0 / 14.0f32.sqrt() * 1.4).log10();
        assert!((peak_db(&ins) - mix_want).abs() < 0.5, "{} vs {mix_want}", peak_db(&ins));
        // The wahs and the TB-303 draw a neutral shape: Reso does not move it.
        for circuit in [2.0, 3.0, 7.0] {
            let mut a = Insert::new("fs_filter");
            a.params.insert("circuit".into(), circuit);
            assert!(!filter_curve_is_to_scale(&a));
            a.params.insert("reso".into(), 0.0);
            let lo = peak_db(&a);
            a.params.insert("reso".into(), 1.0);
            assert_eq!(lo, peak_db(&a), "circuit {circuit}");
        }
    }

    #[test]
    fn filter_display_is_cached_until_a_key_param_moves() {
        let ctx = egui::Context::default();
        let mut ins = Insert::new("fs_filter");
        let a = filter_display_cached(&ctx, &ins);
        assert_eq!(a, filter_display(&ins));
        // A param outside the key (Mix) leaves the cached entry; Cutoff recomputes it.
        ins.params.insert("mix".into(), 0.3);
        assert_eq!(filter_display_cached(&ctx, &ins), a);
        ins.params.insert("cutoff".into(), 0.9);
        let b = filter_display_cached(&ctx, &ins);
        assert!(b.knob_hz > a.knob_hz);
        assert_eq!(b, filter_display(&ins));
    }

    #[test]
    fn filter_knob_hz_is_the_engines_map() {
        let mut ins = Insert::new("fs_filter");
        ins.params.insert("circuit".into(), 4.0);
        ins.params.insert("cutoff".into(), 0.5);
        assert!((filter_knob_hz(&ins) - 30.0 * 600.0f32.sqrt()).abs() < 0.01);
        ins.params.insert("circuit".into(), 7.0);
        ins.params.insert("tbMod".into(), 1.0);
        ins.params.insert("cutoff".into(), 1.0);
        assert!((filter_knob_hz(&ins) - 12_000.0).abs() < 0.5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comp_curve_shape() {
        assert_eq!(comp_curve(-40.0, -20.0, 4.0, 0.0), -40.0);
        assert!((comp_curve(0.0, -20.0, 4.0, 0.0) - -15.0).abs() < 1e-4);
        // The knee meets the straight line at its edges.
        assert!((comp_curve(-23.0, -20.0, 4.0, 6.0) - -23.0).abs() < 1e-4);
        assert!((comp_curve(-17.0, -20.0, 4.0, 6.0) - -19.25).abs() < 1e-4);
    }

    #[test]
    fn band_dots_sit_on_the_curve() {
        let mut ins = Insert::new("eq_7band");
        ins.params.insert("low_shelf_gain".into(), 8.0);
        ins.params.insert("high_shelf_gain".into(), 6.0);
        ins.params.insert("hpf_on".into(), 1.0);
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0));
        let sr = 44_100.0;
        for (fid, _) in eq_bands("eq_7band") {
            let f = param(&ins, fid);
            let curve = soundcraft_dsp::eq7_response(&ins.params.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(), &[f], sr);
            let dot = band_dot(r, &ins, fid, sr);
            assert!((dot.y - db_to_y(r, curve[0])).abs() < 1e-3, "{fid}");
            assert!((dot.x - freq_to_x(r, f)).abs() < 1e-3, "{fid}");
        }
        // The shelf dot is not at its raw gain (the old, off-curve placement).
        assert!((band_dot(r, &ins, "low_shelf_freq", sr).y - db_to_y(r, 8.0)).abs() > 2.0);
    }

    #[test]
    fn dragging_a_shelf_puts_the_curve_under_the_pointer() {
        let ins = Insert::new("eq_7band");
        let gi = soundcraft_dsp::plugin_info("eq_7band").and_then(|i| i.param("low_shelf_gain")).unwrap();
        let sr = 48_000.0;
        let mut cur = ins.clone();
        // Repeated drag frames converge on the target level at the corner.
        for _ in 0..4 {
            let g = gain_for_curve(&cur, ("low_shelf_freq", "low_shelf_gain"), gi, 100.0, 5.0, sr);
            cur.params.insert("low_shelf_gain".into(), g);
        }
        assert!((curve_db_at(&cur, 100.0, sr, &[]) - 5.0).abs() < 0.1);
        // Out of reach: clamps to the range, never NaN.
        let g = gain_for_curve(&ins, ("low_shelf_freq", "low_shelf_gain"), gi, 100.0, 1e9, sr);
        assert!(g.is_finite() && g <= gi.max);
        let g = gain_for_curve(&ins, ("low_shelf_freq", "low_shelf_gain"), gi, 100.0, f32::NAN, sr);
        assert!(g.is_finite());
    }

    #[test]
    fn band_colours_follow_the_dots() {
        assert_eq!(band_color("eq_7band", "low_shelf_q"), Some(BAND_COLORS[0]));
        assert_eq!(band_color("eq_7band", "high_shelf_gain"), Some(BAND_COLORS[4]));
        assert_eq!(band_color("eq_7band", "hpf_freq"), None);
        assert_eq!(band_color("compressor", "threshold"), None);
    }

    #[test]
    fn axis_maps_round_trip() {
        let r = Rect::from_min_size(pos2(10.0, 10.0), vec2(200.0, 100.0));
        for f in [20.0, 440.0, 20_000.0] {
            assert!((x_to_freq(r, freq_to_x(r, f)) / f - 1.0).abs() < 1e-3);
        }
        assert!((y_to_db(r, db_to_y(r, 6.0)) - 6.0).abs() < 1e-3);
    }

    #[test]
    fn graphs_draw_without_panicking() {
        let mut base = Insert::new("eq_7band");
        base.params.insert("mid_gain".into(), 9.0);
        let live = LiveValues { in_peak: 0.5, out_peak: f32::NAN, gr_db: 30.0, readouts: [f32::NAN; 8] };
        let mut h = egui_kittest::Harness::new_ui(|ui| {
            let r = Rect::from_min_size(pos2(5.0, 5.0), vec2(160.0, 80.0));
            for (plugin, kind) in [
                ("eq_7band", GraphKind::EqCurve),
                ("eq_1band", GraphKind::EqCurve),
                ("compressor", GraphKind::Compressor),
                ("channel_strip", GraphKind::Compressor),
                ("x", GraphKind::Level),
            ] {
                let i = Insert { plugin: plugin.into(), ..base.clone() };
                draw_graph(ui, r, kind, &i, Id::NULL, Some(&live), None, 44_100.0, true);
                draw_graph(ui, r, kind, &i, Id::NULL, None, None, f32::NAN, false);
                assert!(eq_band_drag(ui, r, &i, f32::NAN).is_none());
            }
        });
        h.run();
    }

    /// An analysis whose pre and post streams carry a sine (post louder than full scale).
    fn fed_analysis(n: usize) -> std::sync::Arc<Analysis> {
        let a = Analysis::new();
        for i in 0..n {
            let x = (i as f32 * 0.05).sin();
            a.pre.push(0.5 * x);
            a.post.push(1.3 * x);
        }
        a
    }

    #[test]
    fn firestation_graphs_draw_hostile_values_without_panicking() {
        let a = fed_analysis(10_000);
        let mut streams = Streams::default();
        streams.feed(&a, GraphKind::CleanQ, 1.0);
        streams.feed(&a, GraphKind::Twine, 2.0);
        streams.feed(&a, GraphKind::Transient, 2.5);
        streams.feed(&a, GraphKind::Shale, 2.7);
        let hostile = LiveValues {
            in_peak: f32::INFINITY,
            out_peak: -1.0,
            gr_db: f32::NAN,
            readouts: [f32::INFINITY, f32::NAN, -5.0, 1e9, -1e9, 0.0, 0.0, 0.0],
        };
        let engines = soundcraft_dsp::plugins().iter().filter(|p| p.category == soundcraft_dsp::Category::FireStation);
        assert!(engines.clone().count() >= 15 && engines.clone().any(|p| p.id == "fs_reverb") && engines.clone().any(|p| p.id == "fs_chamber"));
        let mut ready = false;
        let mut h = egui_kittest::Harness::new_ui(move |ui| {
            if !ready {
                crate::fonts::install(ui.ctx());
                ready = true;
                return;
            }
            // Every FireStation engine in the registry, so a new one can't be left out.
            for plugin in soundcraft_dsp::plugins().iter().filter(|p| p.category == soundcraft_dsp::Category::FireStation).map(|p| p.id) {
                let mut ins = Insert::new(plugin);
                for (k, v) in [
                    ("b01_on", 2.0),
                    ("b01_gain", f32::NAN),
                    ("b02_on", 1.0),
                    ("b02_type", 3.0),
                    ("style", 99.0),
                    ("ceiling", f32::INFINITY),
                    ("drive", -3.0),
                    ("circuit", f32::NAN),
                    ("level", f32::INFINITY),
                ] {
                    ins.params.insert(k.into(), v);
                }
                let kind = graph_kind(plugin);
                for r in
                    [Rect::from_min_size(pos2(5.0, 5.0), vec2(160.0, 80.0)), Rect::from_min_size(pos2(5.0, 5.0), vec2(40.0, 12.0)), Rect::NOTHING]
                {
                    draw_graph(ui, r, kind, &ins, Id::NULL, Some(&hostile), Some(&streams), f32::NAN, true);
                    draw_graph(ui, r, kind, &ins, Id::NULL, None, None, 0.0, false);
                }
                let r = Rect::from_min_size(pos2(5.0, 5.0), vec2(160.0, 80.0));
                assert!(cleanq_band_drag(ui, r, &ins, f32::NAN).is_none());
            }
        });
        h.run();
    }

    #[test]
    fn streams_feed_spectra_and_a_scope() {
        let a = fed_analysis(SCOPE_BUCKET * 10 + 7);
        let mut s = Streams::default();
        s.feed(&a, GraphKind::CleanQ, 3.0);
        assert_eq!(s.pre.len(), SPEC_FFT / 2 + 1);
        assert_eq!(s.post.len(), SPEC_FFT / 2 + 1);
        let first = s.post.clone();
        // No new samples: unchanged. New samples: smoothed toward the new frame.
        s.feed(&a, GraphKind::CleanQ, 4.0);
        assert_eq!(s.post, first);
        for _ in 0..512 {
            a.post.push(0.0);
        }
        s.feed(&a, GraphKind::CleanQ, 5.0);
        assert_ne!(s.post, first);
        // An unfed stream (Twine's pre) shows nothing.
        let twine = Analysis::new();
        for i in 0..(SCOPE_BUCKET * 3) {
            twine.post.push(if i == 5 { 0.9 } else { 0.1 });
        }
        let mut t = Streams::default();
        t.feed(&twine, GraphKind::CleanQ, 6.0);
        assert!(t.pre.is_empty());
        let mut t = Streams::default();
        t.feed(&twine, GraphKind::Twine, 7.0);
        assert_eq!(t.scope.len(), 3);
        assert_eq!(t.scope[0], (0.1, 0.9));
        // Columns are capped, and partial columns carry over between feeds.
        for i in 0..(SCOPE_BUCKET * SCOPE_COLS) {
            twine.post.push(0.2);
            if i % 1000 == 0 {
                t.feed(&twine, GraphKind::Twine, 8.0);
            }
        }
        t.feed(&twine, GraphKind::Twine, 9.0);
        assert_eq!(t.scope.len(), SCOPE_COLS);
        assert_eq!(t.scope.back(), Some(&(0.2, 0.2)));
    }

    #[test]
    fn cleanq_spectra_take_a_frame_at_most_30_times_a_second() {
        let a = fed_analysis(SPEC_FFT * 2);
        let mut s = Streams::default();
        let t0 = 100.0;
        s.feed(&a, GraphKind::CleanQ, t0);
        let first = s.post.clone();
        assert_eq!(first.len(), SPEC_FFT / 2 + 1);
        let more = |a: &Analysis| (0..512).for_each(|_| a.post.push(0.0));
        // New samples, but too soon: the frame waits.
        more(&a);
        s.feed(&a, GraphKind::CleanQ, t0 + SPEC_INTERVAL * 0.5);
        assert_eq!(s.post, first, "throttled");
        s.feed(&a, GraphKind::CleanQ, t0 + SPEC_INTERVAL * 0.9);
        assert_eq!(s.post, first, "still throttled");
        // Due: the samples that arrived meanwhile are taken.
        s.feed(&a, GraphKind::CleanQ, t0 + SPEC_INTERVAL);
        let second = s.post.clone();
        assert_ne!(second, first);
        // Time that goes backwards (a new context) or is not finite takes a frame at once.
        more(&a);
        s.feed(&a, GraphKind::CleanQ, 1.0);
        assert_ne!(s.post, second);
        let third = s.post.clone();
        more(&a);
        s.feed(&a, GraphKind::CleanQ, f64::NAN);
        assert_ne!(s.post, third);
        // Twine's scope is never throttled.
        let mut t = Streams::default();
        t.feed(&a, GraphKind::Twine, 5.0);
        let cols = t.scope.len();
        (0..SCOPE_BUCKET).for_each(|_| a.post.push(0.1));
        t.feed(&a, GraphKind::Twine, 5.0);
        assert_eq!(t.scope.len(), cols + 1);
    }

    #[test]
    fn module_streams_are_shared_not_copied() {
        let a = fed_analysis(SPEC_FFT);
        let ctx = egui::Context::default();
        let key = Id::new("cq");
        let mut ptrs = Vec::new();
        for pass in 0..3 {
            for _ in 0..512 {
                a.post.push(0.25);
            }
            let input = egui::RawInput { time: Some(f64::from(pass)), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let tile = module_streams(ui.ctx(), key, &a, GraphKind::CleanQ);
                let drawer = module_streams(ui.ctx(), key, &a, GraphKind::CleanQ);
                assert!(Arc::ptr_eq(&tile, &drawer), "one Streams per pass");
                ptrs.push((Arc::as_ptr(&tile), tile.post.as_ptr()));
            });
            out.textures_delta.clear();
        }
        // Updated in place from pass to pass: the same allocation, never a copy.
        assert!(ptrs.windows(2).all(|w| w[0] == w[1]), "{ptrs:?}");
    }

    #[test]
    fn module_streams_update_once_per_pass() {
        let a = fed_analysis(SCOPE_BUCKET * 2);
        let ctx = egui::Context::default();
        let key = Id::new("m");
        let mut cols = (0, 0);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let first = module_streams(ui.ctx(), key, &a, GraphKind::Twine);
            for _ in 0..SCOPE_BUCKET {
                a.post.push(0.0);
            }
            // The drawer asks again in the same pass: the same columns, not one more.
            let again = module_streams(ui.ctx(), key, &a, GraphKind::Twine);
            cols = (first.scope.len(), again.scope.len());
        });
        out.textures_delta.clear();
        assert_eq!(cols, (2, 2));
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| cols.0 = module_streams(ui.ctx(), key, &a, GraphKind::Twine).scope.len());
        out.textures_delta.clear();
        assert_eq!(cols.0, 3);
    }

    #[test]
    fn cleanq_shows_and_drags_only_bands_in_use() {
        use soundcraft_dsp::plugins::fs::cleanq::{F_FREQ, clean_q_dsp::gain_to_knob};
        let mut ins = Insert::new("fs_cleanq");
        assert!(cleanq_bands(&cleanq_values(&ins)).is_empty(), "every band starts Off");
        ins.params.insert("b03_on".into(), 2.0);
        ins.params.insert("b03_gain".into(), gain_to_knob(6.0));
        ins.params.insert("b07_on".into(), 1.0);
        ins.params.insert("b07_type".into(), 3.0);
        let bands = cleanq_bands(&cleanq_values(&ins));
        assert_eq!(bands.iter().map(|b| (b.slot, b.active)).collect::<Vec<_>>(), [(2, true), (6, false)]);
        assert!((bands[0].gain_db - 6.0).abs() < 1e-3);
        assert!(bands[0].has_gain() && !bands[1].has_gain(), "a low cut has no gain to drag");
        assert_eq!(cq_param_id(2, F_FREQ), "b03_freq");

        // A drag to (1 kHz, -12 dB) sets the bell's knobs there; a cut only moves in frequency.
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0));
        let to = pos2(freq_to_x(r, 1000.0), db_y(r, -12.0, CQ_RANGE));
        let v = cleanq_drag_values(r, &bands[0], to);
        assert_eq!(v.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["b03_freq", "b03_gain"]);
        let mut moved = ins.clone();
        moved.params.extend(v);
        let b = cleanq_bands(&cleanq_values(&moved))[0];
        assert!((b.freq / 1000.0 - 1.0).abs() < 1e-3 && (b.gain_db + 12.0).abs() < 1e-2, "{b:?}");
        assert_eq!(cleanq_drag_values(r, &bands[1], to).len(), 1);
        // Far outside: clamped to the knob range.
        let v = cleanq_drag_values(r, &bands[0], pos2(-1e9, 1e9));
        assert!(v.iter().all(|(_, x)| (0.0..=1.0).contains(x)));
    }

    #[test]
    fn the_cleanq_curve_cache_keeps_one_entry_per_graph() {
        let ctx = egui::Context::default();
        let freqs: Vec<f32> = (0..64).map(|i| 20.0 * 1000f32.powf(i as f32 / 63.0)).collect();
        let (a, b) = (Id::new("tile"), Id::new("drawer"));
        let mut values = cleanq_values(&Insert::new("fs_cleanq"));
        values[cq::band_param(0, cq::F_ON)] = 2.0;
        let mut last = None;
        for i in 0..100 {
            values[cq::band_param(0, cq::F_GAIN)] = i as f32 / 100.0;
            last = Some(cleanq_curve(&ctx, a, &values, &freqs, 48_000.0));
            cleanq_curve(&ctx, b, &values, &freqs, 48_000.0);
        }
        assert_eq!(ctx.data(|d| d.count::<CurveCache>()), 2, "one entry per graph, not one per state");
        // Unchanged inputs: the cached curve itself.
        let again = cleanq_curve(&ctx, a, &values, &freqs, 48_000.0);
        assert!(last.is_some_and(|l| std::sync::Arc::ptr_eq(&l, &again)));
    }

    #[test]
    fn comp_curve_follows_lane_a() {
        let mut ins = Insert::new("fs_comp");
        let (thr, ratio, knee) = comp_lane_a(&ins);
        // Defaults (Clean): amount 0.4 is -20 dB, the ratio knob's square law, knee 24 x 0.25.
        assert!((thr + 20.0).abs() < 1e-4);
        assert!((ratio - (1.0 + 19.0 * (0.4f32 / 0.965).powi(2))).abs() < 1e-4);
        assert!((knee - 6.0).abs() < 1e-4);
        ins.params.insert("style".into(), 1.0);
        ins.params.insert("ratio".into(), 1.0);
        assert_eq!(comp_lane_a(&ins).1, 30.0, "1176 all buttons");
        ins.params.insert("style".into(), 5.0);
        ins.params.insert("amount".into(), 1.0);
        assert_eq!(comp_lane_a(&ins).0, -45.0, "Vari-Mu threshold");
        assert_eq!(switch::<f32>(0.5, &[]), None);
    }

    #[test]
    fn the_compressor_dot_shows_only_a_real_reading() {
        let dots = |readout: f32| {
            let live = LiveValues { in_peak: 0.5, out_peak: 0.5, gr_db: 0.0, readouts: [readout; 8] };
            let mut h = egui_kittest::Harness::new_ui(move |ui| {
                let r = Rect::from_min_size(pos2(5.0, 5.0), vec2(160.0, 80.0));
                draw_graph(ui, r, GraphKind::Compressor, &Insert::new("compressor"), Id::NULL, Some(&live), None, 48_000.0, false);
            });
            h.run();
            h.output().shapes.iter().filter(|s| matches!(&s.shape, Shape::Circle(c) if c.radius == 3.0)).count()
        };
        assert_eq!(dots(-12.0), 1);
        assert_eq!(dots(f32::NAN), 0, "no reading (bypassed, or a plugin that writes none): no dot at 0 dBFS");
        let unread = crate::chain_view::live_values(&soundcraft_mix::ModuleMeter::default());
        assert!(unread.readouts[0].is_nan(), "the tile passes \"no reading\" on");
    }

    /// An inline worker standing in for Vint's compute, counting its calls.
    fn counting() -> (CurveWorker, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let m = std::sync::Arc::clone(&n);
        let w = CurveWorker::inline(Box::new(move |v, _, f| {
            m.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            vec![v.first().copied().unwrap_or(0.0); f.len()]
        }));
        (w, n)
    }

    #[test]
    fn the_vint_curve_cache_stays_bounded() {
        let ctx = egui::Context::default();
        let freqs: Vec<f32> = (0..64).map(|i| 20.0 * 1000f32.powf(i as f32 / 63.0)).collect();
        let (m1, m2) = (module_key(1, 1), module_key(2, 1));
        let graphs = [(Id::new("tile1"), m1), (Id::new("drawer1"), m1), (Id::new("tile2"), m2)];
        let (w, n) = counting();
        let n = || n.load(std::sync::atomic::Ordering::Relaxed);
        let mut values = vint_values(&Insert::new("fs_vint"));
        let mut last = None;
        for i in 0..100 {
            values[0] = i as f32;
            // Far enough apart that the budget never holds a graph back.
            let now = f64::from(i) * 1.0;
            for (j, (key, module)) in graphs.iter().enumerate() {
                let c = vint_curve_with(&ctx, &w, *key, *module, now + 0.3 * j as f64, &values, &freqs, 48_000.0);
                if j == 0 {
                    last = c;
                }
            }
        }
        assert_eq!(n(), 300, "every changed state was computed");
        assert_eq!(ctx.data(|d| d.count::<VintCurve>()), 3, "one curve per graph, not one per state");
        assert_eq!(ctx.data(|d| d.count::<VintBudget>()), 2, "one budget per module");
        // Unchanged inputs: the cached curve itself, without a compute.
        let again = vint_curve_with(&ctx, &w, graphs[0].0, m1, 1000.0, &values, &freqs, 48_000.0);
        assert!(last.is_some_and(|l| again.is_some_and(|a| std::sync::Arc::ptr_eq(&l, &a))));
        assert_eq!(n(), 300);
    }

    #[test]
    fn the_vint_curve_is_computed_at_most_once_per_100ms() {
        let ctx = egui::Context::default();
        let freqs = [100.0, 1000.0, 10_000.0];
        let (tile, drawer, m) = (Id::new("tile"), Id::new("drawer"), module_key(7, 3));
        let (w, n) = counting();
        let n = || n.load(std::sync::atomic::Ordering::Relaxed);
        let mut values = vint_values(&Insert::new("fs_vint"));
        let mut at = |now: f64, key: Id, v0: f32| {
            values[0] = v0;
            vint_curve_with(&ctx, &w, key, m, now, &values, &freqs, 48_000.0).map(|c| c[0])
        };
        assert_eq!(at(10.0, tile, 1.0), Some(1.0));
        // Two changes within 100 ms: no compute, the last curve stays.
        assert_eq!(at(10.03, tile, 2.0), Some(1.0));
        assert_eq!(at(10.08, tile, 3.0), Some(1.0));
        assert_eq!(n(), 1, "two param changes within 100 ms trigger one compute");
        // The module's other graph shares the budget: nothing to show yet.
        assert_eq!(at(10.09, drawer, 3.0), None);
        assert_eq!(n(), 1);
        // Once the budget frees, the graph that waited first goes; then the two take turns.
        assert_eq!(at(10.11, tile, 3.0), Some(3.0));
        assert_eq!(at(10.11, drawer, 3.0), None);
        assert_eq!(at(10.22, tile, 4.0), Some(3.0), "the drawer waited: its turn");
        assert_eq!(at(10.22, drawer, 4.0), Some(4.0));
        assert_eq!(n(), 3);
        // A drag that changes every frame still updates both, taking turns.
        let (mut tile_new, mut drawer_new) = (0, 0);
        for f in 0..60 {
            let now = 11.0 + f64::from(f) / 30.0;
            let v = 100.0 + f as f32;
            if at(now, tile, v) == Some(v) {
                tile_new += 1;
            }
            if at(now, drawer, v) == Some(v) {
                drawer_new += 1;
            }
        }
        assert!(n() - 3 <= 21, "at most one compute per 100 ms over 2 s: {}", n() - 3);
        assert!(tile_new >= 5 && drawer_new >= 5, "both graphs follow the drag: {tile_new}, {drawer_new}");
    }

    #[test]
    fn the_vint_curve_shows_a_console_boost() {
        let mut ins = Insert::new("fs_vint");
        ins.params.insert("circuit".into(), 1.0);
        ins.params.insert("conMidFreq".into(), 2.0);
        ins.params.insert("conMidGain".into(), 1.0);
        let ctx = egui::Context::default();
        // The real worker, off this thread: polled until the curve is in. (This context's clock
        // stands still, so each curve asked for here is a module of its own.)
        let curve = |ins: &Insert, freqs: &[f32], key: &str| {
            let t0 = std::time::Instant::now();
            loop {
                let c = vint_curve(&ctx, Id::new(key), Id::new(("m", key)), &vint_values(ins), freqs, 48_000.0);
                if c.is_some() || t0.elapsed() > std::time::Duration::from_secs(20) {
                    break c;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        // The first ask hands the compute to the worker and returns before it is done: a curve is
        // picked up before a request is made, so the first ask has none whatever the timing.
        // (That a request does not block is `curve_worker`'s test, gated, not timed.)
        assert!(vint_curve(&ctx, Id::new("g"), Id::new(("m", "g")), &vint_values(&ins), &[100.0, 1600.0], 48_000.0).is_none());
        let c = curve(&ins, &[100.0, 1600.0], "g").unwrap();
        assert!(c.iter().all(|v| v.is_finite()));
        assert!(c[1] - c[0] > 10.0, "the 1.6 kHz bell stands above 100 Hz: {c:?}");
        // The render's settings: the 80 Hz high-pass rolls the low end off, with no notch.
        for (k, v) in [("conHpf", 2.0), ("conLowFreq", 2.0), ("conLowGain", 0.72), ("conMidFreq", 3.0), ("conMidGain", 0.8), ("conHighGain", 0.68)] {
            ins.params.insert(k.into(), v);
        }
        let freqs = [30.0, 50.0, 80.0, 200.0];
        let c = curve(&ins, &freqs, "hpf").unwrap();
        assert!(c[0] < c[3] - 12.0, "30 Hz well below 200 Hz: {c:?}");
        assert!(c.windows(2).all(|w| w[0] <= w[1] + 0.3), "rising to 200 Hz, no notch: {c:?}");
        assert_eq!(graph_kind("fs_vint"), GraphKind::Vint);
    }

    /// Writes every graph kind, small and large, to `$GRAPH_GALLERY` (a PNG path); does nothing
    /// without it. For looking at the graphs, not a regression check.
    #[test]
    fn graph_gallery() {
        let _palette = crate::theme::lock_test_palette();
        let Some(path) = std::env::var_os("GRAPH_GALLERY").map(std::path::PathBuf::from) else { return };
        let mut eq7 = Insert::new("eq_7band");
        for (k, v) in [
            ("low_shelf_gain", 4.0),
            ("low_mid_gain", -6.0),
            ("low_mid_freq", 300.0),
            ("mid_gain", 3.0),
            ("high_mid_gain", -3.0),
            ("high_shelf_gain", 6.0),
            ("hpf_on", 1.0),
            ("hpf_freq", 60.0),
        ] {
            eq7.params.insert(k.into(), v);
        }
        let mut eq1 = Insert::new("eq_1band");
        eq1.params.insert("gain".into(), 9.0);
        eq1.params.insert("freq".into(), 2500.0);
        let mut comp = Insert::new("compressor");
        comp.params.insert("threshold".into(), -24.0);
        comp.params.insert("ratio".into(), 4.0);
        let strip = Insert::new("channel_strip");
        let level = Insert::new("gain");
        let live = LiveValues { in_peak: 0.7, out_peak: 0.35, gr_db: 7.5, readouts: [-10.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] };
        for light in [false, true] {
            let items = [eq7.clone(), eq1.clone(), comp.clone(), strip.clone(), level.clone()];
            let mut ready = false;
            let mut harness = egui_kittest::Harness::builder().with_size(vec2(1000.0, 560.0)).with_pixels_per_point(2.0).wgpu().build_ui(move |ui| {
                if !ready {
                    crate::fonts::install(ui.ctx());
                    crate::theme::apply(ui.ctx(), light);
                    ready = true;
                    return;
                }
                ui.painter().rect_filled(ui.max_rect(), 0.0, Tokens::current().module_tile_bg);
                for (i, ins) in items.iter().enumerate() {
                    let kind = graph_kind(&ins.plugin);
                    let small = Rect::from_min_size(pos2(10.0 + 190.0 * i as f32, 10.0), vec2(180.0, 90.0));
                    draw_graph(ui, small, kind, ins, Id::NULL, Some(&live), None, 44_100.0, false);
                    let col = (i % 2) as f32;
                    let row = (i / 2) as f32;
                    let large = Rect::from_min_size(pos2(10.0 + 495.0 * col, 115.0 + 150.0 * row), vec2(480.0, 140.0));
                    draw_graph(ui, large, kind, ins, Id::NULL, Some(&live), None, 44_100.0, true);
                }
            });
            harness.input_mut().max_texture_side = Some(8192);
            harness.run_steps(3);
            let out = if light { path.with_extension("light.png") } else { path.clone() };
            harness.render().unwrap().save(out).unwrap();
        }
    }

    #[test]
    fn pitch_bars_follow_the_style() {
        let mut ins = Insert::new("fs_pitch");
        let caps = |ins: &Insert| pitch_bars(ins).into_iter().map(|b| b.1).collect::<Vec<_>>();
        // POG: the main row's levels, in its order.
        assert_eq!(caps(&ins), ["SUB", "-1", "DRY", "+1", "+2", "5TH"]);
        let bars = pitch_bars(&ins);
        assert!(bars[0].2.is_infinite() && bars[0].2 < 0.0, "sub off");
        assert!(bars[2].3 && bars[2].2.abs() < 0.01, "the dry at unity");
        // A voice is its knob times Level: -1 Oct at 0.6 with Level at unity.
        assert!((bars[1].2 - 20.0 * (2.0f32 * 0.36).log10()).abs() < 0.01);
        ins.params.insert("level".into(), 0.5);
        assert!((pitch_bars(&ins)[1].2 - 20.0 * (0.72f32 * 0.5).log10()).abs() < 0.01);
        assert!(pitch_bars(&ins)[2].2.abs() < 0.01, "Level leaves the dry alone");
        ins.params.insert("style".into(), 2.0);
        assert_eq!(caps(&ins), ["-2", "-1", "DIRECT"]);
        ins.params.insert("style".into(), 3.0);
        assert_eq!(caps(&ins), ["DRY", "DOUBLES"]);
        ins.params.insert("style".into(), 4.0);
        assert_eq!(caps(&ins), ["WHAMMY", "DRY"]);
        ins.params.insert("style".into(), f32::NAN);
        assert_eq!(caps(&ins).len(), 6, "a nonsense style reads as the default");
        assert_eq!(pitch_db_text(-0.001), "0");
        assert_eq!(pitch_db_text(-2.6), "-3");
        assert_eq!(pitch_db_text(4.4), "+4");
    }

    #[test]
    fn reverb_decay_reads_the_engine_else_the_parameters() {
        let mut ins = Insert::new("fs_reverb");
        // No reading: the parameters (Hall, 2.5 s, 20 ms pre-delay, Damping 6 kHz).
        let r = reverb_decay(&ins, None);
        assert_eq!(r.machine, 1);
        assert!((r.shape.t60_mid - 2.5).abs() < 0.01);
        assert!((r.shape.pre_s - 0.02).abs() < 1e-4);
        assert!(!r.shape.frozen);
        assert_eq!(r.caption, "Hall  \u{b7}  RT60 2.5 s");
        assert_eq!(r.detail, "PRE 20 ms  \u{b7}  LOW \u{d7}1.00  \u{b7}  HI \u{d7}0.35 > 6.00 k");
        // Synced with no reading: the note at the Manual BPM (1/16 at 120 = 125 ms).
        ins.params.insert("sync".into(), 1.0);
        assert!((reverb_decay(&ins, None).shape.pre_s - 0.125).abs() < 1e-4);
        // A reading: the engine's pre-delay (host tempo), its machine (mid-flush) and its freeze.
        let mut lv = no_reading();
        lv.readouts[2] = 400.0;
        lv.readouts[4] = 3.0;
        lv.readouts[5] = 1.0;
        let r = reverb_decay(&ins, Some(&lv));
        assert!((r.shape.pre_s - 0.4).abs() < 1e-6);
        assert_eq!(r.machine, 3);
        assert!(r.shape.frozen);
        assert_eq!(r.caption, "Spring  \u{b7}  FREEZE");
        assert_eq!(r.detail, "PRE 400 ms");
        // The plate's decay through the engine's table: 1.25x the knob, lows x0.5, highs x0.33.
        ins.params.insert("machine".into(), 2.0);
        ins.params.insert("sync".into(), 0.0);
        let r = reverb_decay(&ins, None);
        assert!((r.shape.t60_mid - 3.125).abs() < 0.01 && (r.shape.t60_low - 1.5625).abs() < 0.01);
        assert_eq!(r.shape.hi_hz, 2500.0);
        // The Shimmer says its shifted path rings past the RT60.
        let mut sh = Insert::new("fs_reverb");
        sh.params.insert("machine".into(), 5.0);
        assert_eq!(reverb_decay(&sh, None).caption, "Shimmer  \u{b7}  RT60 2.5 s + shift tail");
        // Nonsense reads as no reading.
        lv.readouts = [f32::NAN, f32::INFINITY, -5.0, 1e9, -1e9, 7.0, 0.0, 0.0];
        let r = reverb_decay(&ins, Some(&lv));
        assert_eq!(r.machine, 2);
        assert!(r.shape.pre_s.is_finite() && r.shape.t60_mid.is_finite() && !r.shape.frozen);
    }

    #[test]
    fn mod_lfo_reads_the_engine_else_the_parameters() {
        let mut ins = Insert::new("fs_mod");
        // No reading: the parameters (Dim D button 2: 0.78 Hz, its trapezoid), no dot.
        let l = mod_lfo(&ins, None);
        assert!((l.hz - 0.78).abs() < 1e-6);
        assert_eq!(l.shape, 1.4);
        assert_eq!(l.phase, None);
        assert_eq!(l.caption, "LFO 0.780 Hz");
        // A reading: the engine's phase, the right side Width x 0.5 cycles on.
        let mut lv = no_reading();
        lv.readouts[0] = 0.8;
        lv.readouts[1] = 4.0;
        lv.readouts[2] = 0.0;
        ins.params.insert("width".into(), 1.0);
        let l = mod_lfo(&ins, Some(&lv));
        let (pl, pr) = l.phase.unwrap();
        assert!((pl - 0.8).abs() < 1e-6 && (pr - 0.3).abs() < 1e-6, "{pl} {pr}");
        assert_eq!((l.hz, l.shape), (4.0, 0.0));
        assert_eq!(l.caption, "LFO 4.00 Hz");
        // Synced: the division joins the caption.
        ins.params.insert("sync".into(), 1.0);
        ins.params.insert("division".into(), 6.0);
        assert_eq!(mod_lfo(&ins, Some(&lv)).caption, "LFO 4.00 Hz  \u{b7}  1/8");
        // Nonsense reads as no reading.
        lv.readouts = [f32::NAN, -3.0, f32::INFINITY, 0.0, 0.0, 0.0, 0.0, 0.0];
        let l = mod_lfo(&ins, Some(&lv));
        assert!(l.phase.is_none() && l.hz > 0.0 && l.shape.is_finite());
    }

    #[test]
    fn transient_streams_keep_the_scope_bins() {
        // Transient's post ring carries the C++ scope: four values per 4 ms bin.
        let a = Analysis::new();
        for k in 0..10 {
            a.post.push_slice(&[0.5, 0.25, k as f32, -(k as f32)]);
        }
        let mut s = Streams::default();
        s.feed(&a, GraphKind::Transient, 1.0);
        assert_eq!(s.bins.len(), 10);
        assert_eq!(s.bins[9], [0.5, 0.25, 9.0, -9.0]);
        // Only what is new is taken; the history is capped at the last 2.5 s, newest last.
        s.feed(&a, GraphKind::Transient, 1.1);
        assert_eq!(s.bins.len(), 10);
        for k in 0..(TRANSIENT_BINS + 5) {
            a.post.push_slice(&[0.1, 0.2, 1.0, (k % 7) as f32]);
            if k % 100 == 0 {
                s.feed(&a, GraphKind::Transient, 2.0);
            }
        }
        s.feed(&a, GraphKind::Transient, 3.0);
        assert_eq!(s.bins.len(), TRANSIENT_BINS);
        assert_eq!(s.bins.back().copied(), Some([0.1, 0.2, 1.0, ((TRANSIENT_BINS + 4) % 7) as f32]));
        // A new instance (its ring starts again): what it has.
        let b = Analysis::new();
        b.post.push_slice(&[0.9, 0.9, 3.0, 0.0]);
        s.feed(&b, GraphKind::Transient, 4.0);
        assert_eq!(s.bins.back().copied(), Some([0.9, 0.9, 3.0, 0.0]));
    }

    #[test]
    fn transient_columns_keep_peaks_and_extremes() {
        // The C++ scope's per-pixel reduction: the loudest peaks and the most extreme gains.
        let bins: VecDeque<[f32; 4]> =
            [[0.1, 0.2, 3.0, -1.0], [0.4, 0.1, -5.0, 2.0], [0.2, 0.3, 1.0, -6.0], [0.0, 0.0, 0.0, 0.0]].into_iter().collect();
        let cols = transient_columns(&bins, 4, 2);
        assert_eq!(cols, [[0.4, 0.2, -5.0, 2.0], [0.2, 0.3, 1.0, -6.0]]);
        // Fewer bins than shown: the missing (older) ones are empty, as the C++ ring starts zeroed.
        let cols = transient_columns(&bins, 8, 4);
        assert_eq!(cols[0], [0.0; 4]);
        assert_eq!(cols[1], [0.0; 4]);
        assert_eq!(cols[3], [0.2, 0.3, 1.0, -6.0]);
        assert!(transient_columns(&bins, 4, 0).is_empty());
        // The lane: +18 at the top, -24 at the bottom; the wave: -60 dBFS at the floor.
        assert_eq!(transient_lane_t(18.0), 0.0);
        assert_eq!(transient_lane_t(-24.0), 1.0);
        assert!((transient_lane_t(0.0) - 18.0 / 42.0).abs() < 1e-6);
        assert_eq!(transient_lane_t(f32::NAN), transient_lane_t(0.0));
        assert_eq!(transient_wave_t(1.0), 1.0);
        assert_eq!(transient_wave_t(0.0), 0.0);
        assert!((transient_wave_t(10f32.powf(-1.5)) - 0.5).abs() < 1e-5);
        assert_eq!(transient_wave_t(0.001), 0.0, "-60 dBFS is the floor");
        assert_eq!(transient_wave_t(f32::NAN), 0.0);
    }

    #[test]
    fn delay_view_reads_the_engine_else_the_parameters() {
        use soundcraft_dsp::plugins::fs::delay::delay_config::Side;
        let mut ins = Insert::new("fs_delay");
        // No reading: the parameters (350 ms, Feedback 0.35 x 1.15), one head.
        let v = delay_view(&ins, None);
        assert_eq!(v.caption, "Digital  350 ms");
        assert_eq!(v.status, "FB 40%");
        assert!((v.taps[0].ms - 350.0).abs() < 0.05, "{}", v.taps[0].ms);
        // Stereo: both lines repeat together.
        assert_eq!((v.taps[0].side, v.taps[1].side), (Side::Left, Side::Right));
        let left: Vec<_> = v.taps.iter().filter(|e| e.side == Side::Left).collect();
        assert!((left[1].gain - 0.35 * 1.15).abs() < 1e-5);
        assert!((v.window_ms - 1750.0).abs() < 0.5);
        assert_eq!(v.envelope.len(), v.taps.len());
        // A reading: the engine's echo times and tempo; synced, the note value joins the caption.
        let mut lv = no_reading();
        lv.readouts[0] = 500.0;
        lv.readouts[1] = 500.0;
        lv.readouts[2] = 120.0;
        ins.params.insert("sync".into(), 1.0);
        ins.params.insert("division".into(), 7.0);
        let v = delay_view(&ins, Some(&lv));
        assert_eq!(v.caption, "Digital  1/4  500 ms");
        assert!((v.taps[0].ms - 500.0).abs() < 1e-3);
        // Synced with no reading: the Manual BPM (the engine's own fallback without a host tempo).
        let v = delay_view(&ins, None);
        assert!((v.taps[0].ms - 500.0).abs() < 0.05, "120 bpm quarter");
        // Ping-pong: the sides alternate. Freeze: no decay.
        ins.params.insert("routing".into(), 2.0);
        ins.params.insert("freeze".into(), 1.0);
        let v = delay_view(&ins, Some(&lv));
        assert_eq!(v.status, "FREEZE");
        assert_eq!(v.taps[0].side, Side::Left);
        assert_eq!(v.taps[1].side, Side::Right);
        assert!(v.taps.iter().all(|e| e.gain == 1.0));
        // Nonsense reads as no reading.
        lv.readouts = [f32::NAN, -3.0, f32::INFINITY, 0.0, 0.0, 0.0, 0.0, 0.0];
        let v = delay_view(&ins, Some(&lv));
        assert!(v.taps.iter().all(|e| e.ms.is_finite() && e.gain.is_finite()));
        assert!(v.window_ms.is_finite());
    }
}

//! Keel's editor: FreeDAWm's own design (no VST behind it), built on [`crate::editor_kit`] in the
//! Engine family's look.
//!
//! - The header: the title, the preset strip (step through the presets, or open the browser from
//!   the name), the editor's Undo and A/B.
//! - The tone: the amp path's response (the voicing's tone stack at its knobs, Bright and the
//!   cabinet), redrawn as they move.
//! - The controls in four groups: Preamp (the voicing, Input, Drive), Tone (Bass, Mid, Treble,
//!   Bright), Cabinet (the cabinet, Blend) and Output (Squeeze with its gain reduction, Output).
//!
//! Every change goes through `mix.module_params`; a drag is one undo step.

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, SelectionStyle, pill_ex, pill_height};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, TINY_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::presets;
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, readouts, run_with};
use crate::module_graphs::freq_to_x;
use crate::theme::regular;
use egui::{Align2, Color32, Rect, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::keel as dsp;
use soundcraft_mix::StripMeter;
use soundcraft_model::{ModuleId, Track};

// ============================================================================ layout

const PAD_X: f32 = 24.0;
const PAD_Y: f32 = 15.0;
const GAP: f32 = 14.0;
const HEADER_H: f32 = 26.0;
const GRAPH_H: f32 = 170.0;
const SMALL_KNOB: f32 = 62.0;
const GROUP_LABEL_H: f32 = 18.0;
const SEG_H: f32 = 24.0;
const CONTROLS_H: f32 = GROUP_LABEL_H + SEG_H + 8.0 + SMALL_KNOB + 30.0 + 24.0;
const HEIGHT: f32 = PAD_Y * 2.0 + HEADER_H + GAP + GRAPH_H + GAP + CONTROLS_H;

/// The editor's size.
pub const SIZE: Vec2 = vec2(860.0, HEIGHT);

/// Keel's accent: brass.
pub const ACCENT: u32 = 0xFFE0_A85A;
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(ACCENT));

/// A/B and Undo walk every parameter.
pub(crate) const FAMILY: Family = Family::new("Keel", &[]);

/// The widths of the four control groups (they fill the editor with [`GAP`] between them).
const GROUP_W: [f32; 4] = [230.0, 230.0, 180.0, 130.0];

/// The curve's range, dB either side of 0.
const CURVE_DB: f32 = 24.0;
const CURVE_POINTS: usize = 96;

fn value_text(host: &Host, id: &str) -> String {
    let v = param(host, id);
    match id {
        "input" | "output" => format!("{}{v:.1} dB", if v > 0.05 { "+" } else { "" }),
        "blend" if v < 0.5 => "DI".into(),
        "blend" if v > 99.5 => "Amp".into(),
        "blend" => format!("{v:.0} % amp"),
        "squeeze" if v < 0.05 => "Off".into(),
        _ => format!("{v:.1}"),
    }
}

// ============================================================================ the editor

#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    /// The curve as last computed, and the settings it was computed for.
    curve: Vec<f32>,
    curve_for: Vec<f32>,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<Editor>(app, ui, track, module, meters, |_, _| {});
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "keel";

    fn size(&self, _ins: &soundcraft_model::Insert) -> Vec2 {
        SIZE
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        editor_ui(self, app, ui, host);
    }
}

fn keep_centre(r: Rect, w: f32, h: f32) -> Rect {
    Rect::from_center_size(r.center(), vec2(w.min(r.width()), h.min(r.height())))
}

/// A row of small dials centred in `r`.
fn dial_row(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, dials: &[(&str, &str)]) {
    let cell_w = (r.width() / dials.len().max(1) as f32).trunc();
    for (i, (id, label)) in dials.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(r.left() + cell_w * i as f32, r.top()), vec2(cell_w, r.height()));
        let text = value_text(host, id);
        let o = DialOpts { label, value_text: &text, default: family::default01(host, id), small: true, ..DialOpts::default() };
        family::dial_param(app, ui, host, fam, &FAMILY, keep_centre(cell, (cell_w - 4.0).min(96.0), SMALL_KNOB + 28.0), &PALETTE, id, &o);
    }
}

fn group_label(p: &egui::Painter, panel: Rect, text: &str) {
    p.text(pos2(panel.left() + 12.0, panel.top() + 13.0), Align2::LEFT_CENTER, text.to_uppercase(), regular(TINY_SIZE), PALETTE.text_faint);
}

fn header_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, header: Rect) {
    let pal = &PALETTE;
    let title_w = paint_title(ui.painter(), header, "Keel", "Bass Amp", pal);
    let h = pill_height(PillStyle::Small);
    let at = |right: f32, w: f32| Rect::from_min_size(pos2(right - w, header.center().y - h * 0.5), vec2(w, h));
    let b = at(header.right(), 28.0);
    let a = at(b.left() - 3.0, 28.0);
    let undo = at(a.left() - 8.0, 48.0);
    let plain = PillLook { tracked: false, ..PillLook::default() };
    if pill_ex(ui, undo, "keel-undo", pal, "Undo", false, PillStyle::Small, PillLook { dimmed: !st.fam.can_undo(), ..plain }).clicked() {
        family::do_undo(app, host, &mut st.fam);
    }
    if pill_ex(ui, a, "keel-a", pal, "A", !st.fam.on_b(), PillStyle::Small, plain).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, false);
    }
    if pill_ex(ui, b, "keel-b", pal, "B", st.fam.on_b(), PillStyle::Small, plain).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, true);
    }
    // the preset strip, centred in what the title and the pills leave
    let free = Rect::from_min_max(pos2(header.left() + title_w + 16.0, header.top()), pos2(undo.left() - 16.0, header.bottom()));
    let name_w = (free.width() - 60.0).clamp(120.0, 240.0);
    let strip = Rect::from_center_size(free.center(), vec2(name_w + 60.0, h + 2.0));
    let prev = Rect::from_min_size(pos2(strip.left(), strip.center().y - h * 0.5), vec2(24.0, h));
    let next = Rect::from_min_size(pos2(strip.right() - 24.0, strip.center().y - h * 0.5), vec2(24.0, h));
    let name = Rect::from_min_max(pos2(prev.right() + 6.0, strip.top()), pos2(next.left() - 6.0, strip.bottom()));
    for (rect, left) in [(prev, true), (next, false)] {
        if family::arrow_pill(ui, rect, ("keel-preset-step", left), pal, left).clicked() {
            let out = presets::step(app, host, &mut st.fam.presets, if left { -1 } else { 1 });
            if out.recalled.is_some() {
                end_gesture(app);
            }
        }
    }
    let shown = presets::shown_name(host).to_string();
    let dirty = presets::is_dirty(app, host, &mut st.fam.presets);
    if family::preset_name(ui, name, pal, &shown, dirty).clicked() {
        presets::open(app, host, &mut st.fam.presets);
    }
}

/// The frequencies the curve is drawn at: log-spaced over the graph's range.
fn curve_freqs() -> Vec<f32> {
    (0..CURVE_POINTS).map(|i| 20.0 * 1000f32.powf(i as f32 / (CURVE_POINTS - 1) as f32)).collect()
}

fn curve_ui(st: &mut Editor, ui: &mut Ui, host: &Host, panel: Rect) {
    let pal = &PALETTE;
    paint_panel(ui.painter(), panel, 10.0);
    group_label(ui.painter(), panel, "Tone");
    let plot = Rect::from_min_max(pos2(panel.left() + 12.0, panel.top() + GROUP_LABEL_H + 4.0), pos2(panel.right() - 12.0, panel.bottom() - 20.0));
    let p = ui.painter().with_clip_rect(panel.intersect(ui.clip_rect()));
    for (hz, label) in
        [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10_000.0, "10k")]
    {
        let x = freq_to_x(plot, hz);
        p.vline(x, plot.y_range(), Stroke::new(1.0, al(Color32::WHITE, 0.04)));
        p.text(pos2(x, plot.bottom() + 9.0), Align2::CENTER_CENTER, label, regular(TINY_SIZE), pal.text_faint);
    }
    let y_of = |db: f32| plot.center().y - (db / CURVE_DB).clamp(-1.0, 1.0) * plot.height() * 0.5;
    p.hline(plot.x_range(), y_of(0.0), Stroke::new(1.0, al(Color32::WHITE, 0.14)));
    p.rect_stroke(plot, 0.0, Stroke::new(1.0, al(Color32::WHITE, 0.06)), StrokeKind::Inside);
    let ids = ["voicing", "bass", "mid", "treble", "bright", "cab"];
    let now: Vec<f32> = ids.iter().map(|id| param(host, id)).chain([host.sr as f32]).collect();
    if now != st.curve_for {
        let v = |i: usize| now.get(i).copied().unwrap_or(0.0);
        st.curve = dsp::tone_curve(v(0).round() as usize, v(1), v(2), v(3), v(4) > 0.5, v(5).round() as usize, v(6), &curve_freqs());
        st.curve_for = now;
    }
    // drawn around its own average, so the shape is what shows
    let mean = st.curve.iter().sum::<f32>() / st.curve.len().max(1) as f32;
    let pts: Vec<egui::Pos2> = curve_freqs().iter().zip(&st.curve).map(|(hz, db)| pos2(freq_to_x(plot, *hz), y_of(db - mean))).collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.0, pal.accent)));
}

fn controls_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, row: Rect) {
    let pal = &PALETTE;
    let mut x = row.left();
    let panels: Vec<Rect> = GROUP_W
        .iter()
        .map(|w| {
            let r = Rect::from_min_size(pos2(x, row.top()), vec2(*w, row.height()));
            x += w + GAP;
            r
        })
        .collect();
    let [pre, tone, cab, out] = panels.as_slice() else { return };
    for (r, name) in [(pre, "Preamp"), (tone, "Tone"), (cab, "Cabinet"), (out, "Output")] {
        paint_panel(ui.painter(), *r, 10.0);
        group_label(ui.painter(), *r, name);
    }
    let body = |r: &Rect| Rect::from_min_max(pos2(r.left() + 8.0, r.top() + GROUP_LABEL_H + 4.0), pos2(r.right() - 8.0, r.bottom() - 8.0));
    let seg = |b: Rect| Rect::from_min_size(b.min, vec2(b.width(), SEG_H));
    let dials = |b: Rect| Rect::from_min_max(pos2(b.left(), b.top() + SEG_H + 8.0), b.max);

    let b = body(pre);
    family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, seg(b), pal, "voicing", None, 1, SelectionStyle::Filled, false);
    dial_row(app, ui, host, &mut st.fam, dials(b), &[("input", "Input"), ("drive", "Drive")]);

    let b = body(tone);
    let bright = Rect::from_min_size(pos2(b.right() - 64.0, b.top()), vec2(64.0, pill_height(PillStyle::Small)));
    family::pill_param(app, ui, host, &mut st.fam, &FAMILY, bright, pal, "bright", "Bright", PillLook { tracked: false, ..PillLook::default() })
        .on_hover_text("A lift above 2.5 kHz, for pick and finger noise");
    dial_row(app, ui, host, &mut st.fam, dials(b), &[("bass", "Bass"), ("mid", "Mid"), ("treble", "Treble")]);

    let b = body(cab);
    family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, seg(b), pal, "cab", None, 1, SelectionStyle::Filled, false);
    dial_row(app, ui, host, &mut st.fam, dials(b), &[("blend", "Blend")]);

    let b = body(out);
    let gr = readouts(host).map_or(0.0, |r| r[2]).max(0.0);
    let text = if param(host, "squeeze") < 0.05 { String::new() } else { format!("-{gr:.1} dB") };
    ui.painter().text(seg(b).center(), Align2::CENTER_CENTER, text, regular(CAPTION_SIZE), pal.text_dim);
    dial_row(app, ui, host, &mut st.fam, dials(b), &[("squeeze", "Squeeze"), ("output", "Output")]);
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    // Scripts: `ui.mix.drawer {tab: "Presets"}` opens the browser (the editor has no other tab).
    let _ = family::take_drawer_tab(app, host, &mut st.fam);
    if !st.init {
        st.init = true;
        st.fam.presets.category = "All".into();
    }
    if family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, "") {
        return;
    }
    let full = host.rect;
    paint_background(ui.painter(), full);
    let b = full.shrink2(vec2(PAD_X, PAD_Y));
    let header = Rect::from_min_size(b.min, vec2(b.width(), HEADER_H));
    let graph = Rect::from_min_size(pos2(b.left(), header.bottom() + GAP), vec2(b.width(), GRAPH_H));
    let controls = Rect::from_min_size(pos2(b.left(), graph.bottom() + GAP), vec2(b.width(), CONTROLS_H));
    header_ui(st, app, ui, host, header);
    curve_ui(st, ui, host, graph);
    controls_ui(st, app, ui, host, controls);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_groups_fill_the_width() {
        let used: f32 = GROUP_W.iter().sum::<f32>() + GAP * (GROUP_W.len() - 1) as f32;
        assert_eq!(used, SIZE.x - 2.0 * PAD_X);
    }

    #[test]
    fn the_curve_follows_the_knobs() {
        let f = curve_freqs();
        let at = |bass: f32, treble: f32| dsp::tone_curve(0, bass, 5.0, treble, false, 3, 48_000.0, &f);
        let (lo, hi) = (at(10.0, 5.0), at(0.0, 5.0));
        assert!(lo[5] > hi[5] + 6.0, "bass lifts the lows: {} vs {}", lo[5], hi[5]);
        let (bright, dull) = (at(5.0, 10.0), at(5.0, 0.0));
        assert!(bright[85] > dull[85] + 6.0);
    }
}

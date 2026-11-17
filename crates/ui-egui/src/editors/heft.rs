//! Heft's editor: FreeDAWm's own design (no VST behind it), built on [`crate::editor_kit`] in the
//! Engine family's look.
//!
//! - The header: the title, the preset strip, the editor's Undo and A/B.
//! - The picture: a frequency axis with the band below Frequency shaded (where the harmonics are
//!   made from, drawn as tall as Original Bass leaves it) and the harmonics drawn above it (as tall
//!   as Harmonics makes them, odd and even as Character blends them); input, output and added
//!   harmonics meters on the right.
//! - The dials: Frequency, Harmonics, Character, Original Bass, Output.
//!
//! Every change goes through `mix.module_params`; a drag is one undo step.

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, pill_ex, pill_height};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, TINY_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::presets;
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, readouts, run_with};
use crate::module_graphs::freq_to_x;
use crate::theme::regular;
use egui::{Align2, Color32, Rect, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use soundcraft_mix::StripMeter;
use soundcraft_model::{ModuleId, Track};

const PAD_X: f32 = 24.0;
const PAD_Y: f32 = 15.0;
const GAP: f32 = 14.0;
const HEADER_H: f32 = 26.0;
const GRAPH_H: f32 = 180.0;
const SMALL_KNOB: f32 = 62.0;
const GROUP_LABEL_H: f32 = 18.0;
const CONTROLS_H: f32 = GROUP_LABEL_H + SMALL_KNOB + 30.0 + 16.0;
const HEIGHT: f32 = PAD_Y * 2.0 + HEADER_H + GAP + GRAPH_H + GAP + CONTROLS_H;
const METERS_W: f32 = 110.0;

/// The editor's size.
pub const SIZE: Vec2 = vec2(720.0, HEIGHT);

/// Heft's accent: deep amber.
pub const ACCENT: u32 = 0xFFF0_9A3E;
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(ACCENT));

/// A/B and Undo walk every parameter.
pub(crate) const FAMILY: Family = Family::new("Heft", &[]);

/// The dial groups' widths (they fill the editor with [`GAP`] between them).
const GROUP_W: [f32; 3] = [290.0, 150.0, 204.0];

fn value_text(host: &Host, id: &str) -> String {
    let v = param(host, id);
    match id {
        "freq" => format!("{v:.0} Hz"),
        "output" => format!("{}{v:.1} dB", if v > 0.05 { "+" } else { "" }),
        "character" if v < 1.0 => "Even".into(),
        "character" if v > 99.0 => "Odd".into(),
        "original" if v < 0.5 => "Removed".into(),
        _ => format!("{v:.0} %"),
    }
}

#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<Editor>(app, ui, track, module, meters, |_, _| {});
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "heft";

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
    let title_w = paint_title(ui.painter(), header, "Heft", "Bass Enhancer", pal);
    let h = pill_height(PillStyle::Small);
    let at = |right: f32, w: f32| Rect::from_min_size(pos2(right - w, header.center().y - h * 0.5), vec2(w, h));
    let b = at(header.right(), 28.0);
    let a = at(b.left() - 3.0, 28.0);
    let undo = at(a.left() - 8.0, 48.0);
    let plain = PillLook { tracked: false, ..PillLook::default() };
    if pill_ex(ui, undo, "heft-undo", pal, "Undo", false, PillStyle::Small, PillLook { dimmed: !st.fam.can_undo(), ..plain }).clicked() {
        family::do_undo(app, host, &mut st.fam);
    }
    if pill_ex(ui, a, "heft-a", pal, "A", !st.fam.on_b(), PillStyle::Small, plain).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, false);
    }
    if pill_ex(ui, b, "heft-b", pal, "B", st.fam.on_b(), PillStyle::Small, plain).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, true);
    }
    let free = Rect::from_min_max(pos2(header.left() + title_w + 16.0, header.top()), pos2(undo.left() - 16.0, header.bottom()));
    let name_w = (free.width() - 60.0).clamp(120.0, 240.0);
    let strip = Rect::from_center_size(free.center(), vec2(name_w + 60.0, h + 2.0));
    let prev = Rect::from_min_size(pos2(strip.left(), strip.center().y - h * 0.5), vec2(24.0, h));
    let next = Rect::from_min_size(pos2(strip.right() - 24.0, strip.center().y - h * 0.5), vec2(24.0, h));
    let name = Rect::from_min_max(pos2(prev.right() + 6.0, strip.top()), pos2(next.left() - 6.0, strip.bottom()));
    for (rect, left) in [(prev, true), (next, false)] {
        if family::arrow_pill(ui, rect, ("heft-preset-step", left), pal, left).clicked() {
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

/// The band below Frequency and the harmonics above it, on a log axis (20 Hz..20 kHz).
fn picture_ui(ui: &mut Ui, host: &Host, panel: Rect) {
    let pal = &PALETTE;
    paint_panel(ui.painter(), panel, 10.0);
    let plot = Rect::from_min_max(pos2(panel.left() + 12.0, panel.top() + 12.0), pos2(panel.right() - 12.0, panel.bottom() - 20.0));
    let p = ui.painter().with_clip_rect(panel.intersect(ui.clip_rect()));
    for (hz, label) in [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k")] {
        let x = freq_to_x(plot, hz);
        p.vline(x, plot.y_range(), Stroke::new(1.0, al(Color32::WHITE, 0.04)));
        p.text(pos2(x, plot.bottom() + 9.0), Align2::CENTER_CENTER, label, regular(TINY_SIZE), pal.text_faint);
    }
    p.rect_stroke(plot, 0.0, Stroke::new(1.0, al(Color32::WHITE, 0.06)), StrokeKind::Inside);
    let freq = param(host, "freq");
    let amount = param(host, "harmonics") / 100.0;
    let odd = param(host, "character") / 100.0;
    let original = param(host, "original") / 100.0;
    let fx = freq_to_x(plot, freq);
    // the band harmonics are made from
    let band_top = plot.bottom() - plot.height() * 0.8 * original.max(0.04);
    p.rect_filled(Rect::from_min_max(pos2(plot.left(), band_top), pos2(fx, plot.bottom())), 0.0, al(pal.accent, 0.18));
    p.vline(fx, plot.y_range(), Stroke::new(1.5, pal.accent));
    // a fundamental at 0.6 of Frequency and the harmonics it gets (2nd to 6th), those above Frequency
    let f0 = freq * 0.6;
    for k in 2..=6 {
        let hz = f0 * k as f32;
        if hz < freq {
            continue;
        }
        let weight = if k % 2 == 0 { 1.0 - odd } else { odd } * (1.0 / (k as f32 - 1.0)).sqrt();
        let h = plot.height() * 0.7 * amount * weight.max(0.05);
        let x = freq_to_x(plot, hz);
        p.line_segment([pos2(x, plot.bottom()), pos2(x, plot.bottom() - h)], Stroke::new(4.0, al(pal.accent, 0.85)));
    }
    p.text(pos2(fx + 6.0, plot.top() + 8.0), Align2::LEFT_CENTER, format!("harmonics above {freq:.0} Hz"), regular(CAPTION_SIZE), pal.text_dim);
}

/// Input, output and added-harmonics peaks (dBFS) as bars.
fn meters_ui(ui: &mut Ui, host: &Host, panel: Rect) {
    let pal = &PALETTE;
    paint_panel(ui.painter(), panel, 10.0);
    let r = readouts(host);
    let bars = [("In", r.map_or(f32::NAN, |r| r[0])), ("Out", r.map_or(f32::NAN, |r| r[1])), ("Harm", r.map_or(f32::NAN, |r| r[2]))];
    let inner = panel.shrink2(vec2(12.0, 12.0));
    let w = inner.width() / bars.len() as f32;
    for (i, (label, db)) in bars.iter().enumerate() {
        let col = Rect::from_min_size(pos2(inner.left() + w * i as f32 + 6.0, inner.top()), vec2(w - 12.0, inner.height() - 16.0));
        ui.painter().rect_filled(col, 2.0, al(Color32::WHITE, 0.05));
        let fill = if db.is_finite() { ((db + 60.0) / 60.0).clamp(0.0, 1.0) } else { 0.0 };
        let top = col.bottom() - col.height() * fill;
        ui.painter().rect_filled(Rect::from_min_max(pos2(col.left(), top), col.max), 2.0, pal.accent);
        ui.painter().text(pos2(col.center().x, col.bottom() + 9.0), Align2::CENTER_CENTER, *label, regular(TINY_SIZE), pal.text_faint);
    }
}

fn controls_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, row: Rect) {
    let mut x = row.left();
    let panels: Vec<Rect> = GROUP_W
        .iter()
        .map(|w| {
            let r = Rect::from_min_size(pos2(x, row.top()), vec2(*w, row.height()));
            x += w + GAP;
            r
        })
        .collect();
    let [made, original, out] = panels.as_slice() else { return };
    for (r, name) in [(made, "Harmonics"), (original, "Original"), (out, "Output")] {
        paint_panel(ui.painter(), *r, 10.0);
        group_label(ui.painter(), *r, name);
    }
    let body = |r: &Rect| Rect::from_min_max(pos2(r.left() + 8.0, r.top() + GROUP_LABEL_H), pos2(r.right() - 8.0, r.bottom() - 8.0));
    dial_row(app, ui, host, &mut st.fam, body(made), &[("freq", "Frequency"), ("harmonics", "Amount"), ("character", "Character")]);
    dial_row(app, ui, host, &mut st.fam, body(original), &[("original", "Original Bass")]);
    dial_row(app, ui, host, &mut st.fam, body(out), &[("output", "Output")]);
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
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
    let graph = Rect::from_min_size(pos2(b.left(), header.bottom() + GAP), vec2(b.width() - METERS_W - GAP, GRAPH_H));
    let meters = Rect::from_min_size(pos2(graph.right() + GAP, graph.top()), vec2(METERS_W, GRAPH_H));
    let controls = Rect::from_min_size(pos2(b.left(), graph.bottom() + GAP), vec2(b.width(), CONTROLS_H));
    header_ui(st, app, ui, host, header);
    picture_ui(ui, host, graph);
    meters_ui(ui, host, meters);
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
}

//! Transient's VST editor (`Engine 10 - Transient/Source/PluginEditor.cpp`), translated:
//! `resized()` and `paint()` constant for constant, the family header, the source row with its
//! blurb (`TransientConfig.h`), the two big dials with their Focus selectors, the four small
//! dials, the scrolling scope ([`scope`], `TransientScope.h`) and the focus note. Palette:
//! `look::apply (kAccent)` (Chartreuse), the same in both DAW themes. Its interaction inventory is
//! `docs/superpowers/editors/transient.md`.

mod scope;
mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, SelectionStyle, segmented};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, param, run_with};
use crate::theme::regular;
use egui::{Align2, Rect, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::transient::FOCUS_NAMES;
use soundcraft_dsp::plugins::fs::transient::transient_engine_dsp::{knob_to_attack_db, knob_to_out_db, knob_to_sustain_db, knob_to_time_scale};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

// ============================================================================ constants

const K_PAD_X: f32 = 24.0;
const K_PAD_Y: f32 = 15.0;
const K_GAP: f32 = 14.0;
const K_HEADER_H: f32 = 26.0;
const K_ROW_H: f32 = 28.0;
const K_BLURB_H: f32 = 16.0;
const K_LABEL_W: f32 = 62.0;
const K_LABEL_GAP: f32 = 12.0;
const K_BIG_KNOB: f32 = 104.0;
const K_SMALL_KNOB: f32 = 62.0;
const K_FOCUS_H: f32 = 24.0;
const K_MIDDLE_H: f32 = K_BIG_KNOB + 30.0 + 6.0 + K_FOCUS_H + 16.0 + K_SMALL_KNOB + 28.0 + 10.0;
const K_FOOT_H: f32 = 16.0;
/// `kDefaultH`.
const K_DEFAULT_H: f32 = K_PAD_Y * 2.0 + K_HEADER_H + K_GAP + K_ROW_H + 3.0 + K_BLURB_H + K_GAP + K_MIDDLE_H + K_GAP + K_FOOT_H;

/// `setSize (860, kDefaultH)`.
pub const SIZE: Vec2 = vec2(860.0, K_DEFAULT_H);

/// `kAccent`: Chartreuse.
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFFC4E86A));

/// `PresetCatalogue`: the storage key, and what `getPresetParams` leaves out besides `bypass`.
const FAMILY: Family = Family::new("TransientEngine", &["output"]);

// ============================================================================ TransientConfig.h

const SOURCES: [(&str, &str); 7] = [
    ("Drums", "A kit or a single drum. Short, hard attacks; the sustain is the ring and the room."),
    ("Percussion", "Hand drums, claps, shakers: faster than a kit, and the detector ignores the lows."),
    ("Guitar", "Listens to the pick, 1.2-8 kHz. Put Attack on High for more or less pick, same body."),
    ("Piano", "The hammer, then a long ring. Cut Sustain to accent the attack and pull the ring down."),
    ("Bass", "Pluck and pick on the low end: slower attacks, a shorter sustain."),
    ("Vocal", "Consonants against vowels. Gentle: small ratios count for a lot here."),
    ("Full Mix", "Everything at once. Keep the amounts small; it is punch, not surgery."),
];

/// The note under the middle (`paint`).
const FOCUS_NOTE: &str =
    "Focus: where each gain lands \u{00B7} Low below 220 Hz \u{00B7} Mid around 1 kHz \u{00B7} High above 2.5 kHz \u{00B7} 3 ms lookahead";

// ============================================================================ formatting

fn signed_db(db: f32) -> String {
    let db = if db.abs() < 0.05 { 0.0 } else { db };
    format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" })
}
fn format_percent(v: f32) -> String {
    format!("{} %", (v * 100.0).round() as i32)
}
fn format_gain(db: f32) -> String {
    if db.abs() < 0.05 { "0 dB".into() } else { signed_db(db) }
}
fn format_scale(v: f32) -> String {
    let k = knob_to_time_scale(v);
    if (k - 1.0).abs() < 0.02 {
        return "Source".into();
    }
    if k > 1.0 { format!("x{k:.1}") } else { format!("x{k:.2}") }
}

// ============================================================================ the editor

#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's More section is open.
    slim_more: bool,
    /// Scripts: open the slim layout's Source menu on the next frame.
    source_open: bool,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_transient";

    fn size(&self, _ins: &Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { SIZE }
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        if self.slim {
            slim::editor_ui(self, app, ui, host);
        } else {
            editor_ui(self, app, ui, host);
        }
    }
}

/// JUCE `withSizeKeepingCentre` on integer rectangles.
fn keep_centre(r: Rect, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(r.left() + ((r.width() - w) / 2.0).trunc(), r.top() + ((r.height() - h) / 2.0).trunc()), vec2(w, h))
}

fn text_left(p: &egui::Painter, r: Rect, text: &str, size: f32, c: egui::Color32) {
    p.with_clip_rect(r.intersect(p.clip_rect())).text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, text, regular(size), c);
}

fn index(host: &Host, id: &str) -> i32 {
    param(host, id).round() as i32
}

/// A Segmented bound to choice parameter `id`: a click is one undo step.
#[allow(clippy::too_many_arguments)]
fn seg_param(
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    st: &mut FamilyState,
    r: Rect,
    pal: &VstPalette,
    id: &str,
    options: &[&str],
    selected: usize,
) {
    if let Some(i) = segmented(ui, r, ("transient-seg", id), pal, options, 1, Some(selected), SelectionStyle::Filled)
        && i != selected
    {
        family::set(app, host, st, &FAMILY, id, i as f32);
    }
}

/// What both layouts do first: the scripted drawer tabs and the preset browser, which covers the
/// whole editor while it is open (then `true`: paint nothing else).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> bool {
    if !st.init {
        st.init = true;
        st.fam.presets.category = "All".into();
    }
    // "Source" (the menu) and "More" are the slim layout's; the full editor has neither.
    match family::take_drawer_tab(app, host, &mut st.fam).as_deref() {
        Some("Source") if st.slim => st.source_open = true,
        Some("More") if st.slim => st.slim_more = true,
        _ => {}
    }
    family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, SOURCES[index(host, "source").clamp(0, 6) as usize].0)
}

/// The source's index (0..6).
fn source_of(host: &Host) -> usize {
    index(host, "source").clamp(0, 6) as usize
}

/// A dial's readout (`formatValue`).
fn value_text(host: &Host, id: &str) -> String {
    let v = param(host, id);
    match id {
        "attack" => format_gain(knob_to_attack_db(v)),
        "sustain" => format_gain(knob_to_sustain_db(v)),
        "mix" => format_percent(v),
        "output" => signed_db(knob_to_out_db(v)),
        _ => format_scale(v),
    }
}

/// The four small dials under the big two, in `resized()`'s order.
const SMALL_DIALS: [(&str, &str); 4] = [("attackTime", "Attack Time"), ("sustainTime", "Sustain Time"), ("mix", "Mix"), ("output", "Output")];

/// One dial (`attachDial`) in `r`: its readout, its default, one undo step per gesture.
#[allow(clippy::too_many_arguments)]
fn dial_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, id: &str, label: &str, small: bool) {
    let value_text = value_text(host, id);
    let default = host.info.param(id).map_or(0.5, |p| p.to_normalized(p.default));
    let o = DialOpts { label, value_text: &value_text, default, small, ..DialOpts::default() };
    family::dial_param(app, ui, host, fam, &FAMILY, r, &PALETTE, id, &o);
}

/// A Focus selector (`attackFocus`, `sustainFocus`) in `r`.
fn focus_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, id: &str) {
    seg_param(app, ui, host, fam, r, &PALETTE, id, &FOCUS_NAMES, index(host, id).clamp(0, 3) as usize);
}

/// The scope in `r` (the C++ timer repaints it at 30 Hz).
fn scope_ui(ui: &Ui, host: &Host, r: Rect) {
    let streams = crate::chain_view::streams_for(ui, host.track, host.ins, host.meters);
    scope::draw(ui.painter(), r, &PALETTE, streams.as_deref().map(|s| &s.bins), host.sr);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let pal = &PALETTE;
    let full = host.rect;
    let p = ui.painter().clone();
    paint_background(&p, full);

    // ---- resized()
    let mut b = full.shrink2(vec2(K_PAD_X, K_PAD_Y));
    let header = Rect::from_min_size(b.min, vec2(b.width(), K_HEADER_H));
    b.min.y += K_HEADER_H + K_GAP;
    let row = Rect::from_min_size(b.min, vec2(b.width(), K_ROW_H));
    let source_label = Rect::from_min_size(row.min, vec2(K_LABEL_W, K_ROW_H));
    let source_seg = Rect::from_min_max(pos2(row.left() + K_LABEL_W + K_LABEL_GAP, row.top()), row.max);
    b.min.y += K_ROW_H + 3.0;
    let source_blurb = Rect::from_min_max(pos2(b.left() + K_LABEL_W + K_LABEL_GAP, b.top()), pos2(b.right(), b.top() + K_BLURB_H));
    b.min.y += K_BLURB_H + K_GAP;

    let mid_h = K_MIDDLE_H.max(b.height() - K_GAP - K_FOOT_H);
    let mid = Rect::from_min_size(b.min, vec2(b.width(), mid_h));
    b.min.y += mid_h;
    let knob_panel = Rect::from_min_size(mid.min, vec2(360f32.max((mid.width() * 47.0 / 100.0).trunc()), mid.height()));
    let scope_r = Rect::from_min_max(pos2(knob_panel.right() + K_GAP, mid.top()), mid.max);

    let k = knob_panel.shrink(10.0);
    let top = Rect::from_min_size(k.min, vec2(k.width(), K_BIG_KNOB + 30.0));
    let half_w = (top.width() / 2.0).trunc();
    let left = Rect::from_min_size(top.min, vec2(half_w, top.height()));
    let right = Rect::from_min_max(pos2(top.left() + half_w, top.top()), top.max);
    let attack_r = keep_centre(left, K_BIG_KNOB, K_BIG_KNOB + 30.0);
    let sustain_r = keep_centre(right, K_BIG_KNOB, K_BIG_KNOB + 30.0);
    let focus = Rect::from_min_size(pos2(k.left(), top.bottom() + 6.0), vec2(k.width(), K_FOCUS_H));
    let seg_w = (half_w - 16.0).min(200.0);
    let attack_focus = keep_centre(Rect::from_min_size(focus.min, vec2(half_w, K_FOCUS_H)), seg_w, K_FOCUS_H);
    let sustain_focus = keep_centre(Rect::from_min_max(pos2(focus.left() + half_w, focus.top()), focus.max), seg_w, K_FOCUS_H);
    // `removeFromTop` never takes more than is left: the row is shorter than the dials, which
    // centre on it and overhang (as in the VST).
    let bottom_top = focus.bottom() + 16.0;
    let bottom = Rect::from_min_size(pos2(k.left(), bottom_top), vec2(k.width(), (K_SMALL_KNOB + 28.0).min(k.bottom() - bottom_top).max(0.0)));
    let cell_w = (bottom.width() / 4.0).trunc();

    b.min.y += K_GAP;
    let focus_blurb = Rect::from_min_size(b.min, vec2(b.width(), K_FOOT_H));

    // ---- paint()
    let cached_source = index(host, "source").clamp(0, 6);
    text_left(&p, source_label, "Source", LABEL_SIZE, pal.text_body);
    text_left(&p, source_blurb, SOURCES[cached_source as usize].1, CAPTION_SIZE, pal.text_faint);
    text_left(&p, focus_blurb, FOCUS_NOTE, CAPTION_SIZE, pal.text_faint);
    paint_panel(&p, knob_panel, 10.0);

    // ---- the header
    paint_title(&p, header, "Transient", "Engine", pal);
    let rects = family::right_aligned_header(ui, header);
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &rects);

    // ---- source and the two focuses
    let names: Vec<&str> = SOURCES.iter().map(|s| s.0).collect();
    seg_param(app, ui, host, &mut st.fam, source_seg, pal, "source", &names, cached_source as usize);
    focus_ui(app, ui, host, &mut st.fam, attack_focus, "attackFocus");
    focus_ui(app, ui, host, &mut st.fam, sustain_focus, "sustainFocus");

    // ---- dials
    let mut cells: Vec<(Rect, &str, &str, bool)> = vec![(attack_r, "attack", "Attack", false), (sustain_r, "sustain", "Sustain", false)];
    for (i, (id, label)) in SMALL_DIALS.into_iter().enumerate() {
        let cell = Rect::from_min_size(pos2(bottom.left() + cell_w * i as f32, bottom.top()), vec2(cell_w, bottom.height()));
        cells.push((keep_centre(cell, (cell_w - 4.0).min(96.0), K_SMALL_KNOB + 28.0), id, label, true));
    }
    for (r, id, label, small) in cells {
        dial_ui(app, ui, host, &mut st.fam, r, id, label, small);
    }

    // ---- the scope
    scope_ui(ui, host, scope_r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_height_is_the_vsts() {
        assert_eq!(SIZE, vec2(860.0, 441.0));
    }

    #[test]
    fn readouts_match_the_cpp() {
        assert_eq!(format_gain(knob_to_attack_db(0.5)), "0 dB");
        assert_eq!(format_scale(0.5), "Source");
        assert_eq!(format_scale(1.0), "x4.0");
        assert_eq!(format_scale(0.0), "x0.25");
        assert_eq!(signed_db(knob_to_out_db(0.5)), "0.0 dB");
    }
}

//! Sift's wide editor. Sift is Rust-native, so like Manners' this is not a translation of a JUCE
//! editor: it is laid out for this app with the family look ([`crate::editor_kit`]): the header
//! (presets, Undo, A/B, Bypass), the timeline (the voice going in, with what was taken off its
//! sibilance and its plosives), a Sibilance section and a Plosives section, and Listen, which
//! plays one part alone. [`slim`] is the same editor in the classic drawer's width.

mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, SelectionStyle};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, param, run_with};
use crate::module_graphs::{SIFT_PLOSIVE, sift_timeline};
use crate::theme::regular;
use egui::{Align2, Color32, CornerRadius, Rect, Ui, Vec2, pos2, vec2};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

// ============================================================================ constants

const K_PAD_X: f32 = 24.0;
const K_PAD_Y: f32 = 15.0;
const K_GAP: f32 = 14.0;
const K_HEADER_H: f32 = 26.0;
const K_DISPLAY_H: f32 = 210.0;
const K_BIG_KNOB: f32 = 104.0;
const K_SMALL_KNOB: f32 = 62.0;
/// A dial's label and readout under its knob.
const K_DIAL_TEXT: f32 = 30.0;
const K_PANEL_PAD: f32 = 10.0;
/// A section's heading over its dials.
const K_HEAD_H: f32 = 18.0;
const K_PANEL_H: f32 = K_HEAD_H + K_BIG_KNOB + K_DIAL_TEXT + K_PANEL_PAD * 2.0;
const K_ROW_H: f32 = 26.0;
const K_FOOT_H: f32 = 16.0;
const K_HEIGHT: f32 = K_PAD_Y * 2.0 + K_HEADER_H + K_GAP + K_DISPLAY_H + K_GAP + K_PANEL_H + K_GAP + K_ROW_H + K_GAP + K_FOOT_H;

/// The editor's size.
pub const SIZE: Vec2 = vec2(860.0, K_HEIGHT);

/// The module's accent (its tile, its dials, its sibilance cuts).
pub const ACCENT: u32 = 0xFFE5_8FA6;
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(ACCENT));

/// The preset catalogue's key. Listen is how you audition, not part of a sound.
const FAMILY: Family = Family::new("Sift", &["listen"]);

const FOOT_NOTE: &str = "Sift judges by shape, not level: a quiet take is treated like a loud one \u{00B7} Listen plays one part alone";

/// The sibilance dials, left to right: (parameter, label, big).
const SIB_DIALS: [(&str, &str, bool); 4] =
    [("sibAmount", "Amount", true), ("sibRange", "Range", false), ("sibLow", "Low", false), ("sibHigh", "High", false)];
/// The plosive dials.
const PLO_DIALS: [(&str, &str, bool); 3] = [("ploAmount", "Amount", true), ("ploRange", "Range", false), ("ploHigh", "Ceiling", false)];

// ============================================================================ formatting

fn hz_text(hz: f32) -> String {
    if hz >= 1000.0 { format!("{:.1} kHz", hz / 1000.0) } else { format!("{} Hz", hz.round() as i32) }
}

/// A dial's readout.
fn value_text(host: &Host, id: &str) -> String {
    let v = param(host, id);
    match id {
        "sibAmount" | "ploAmount" => format!("{} %", v.round() as i32),
        "sibRange" | "ploRange" => format!("{v:.1} dB"),
        _ => hz_text(v),
    }
}

/// What Listen is playing, in words.
fn listen_note(host: &Host) -> &'static str {
    match param(host, "listen").round() as i32 {
        1 => "only what it takes for sibilance",
        2 => "only what it takes for a plosive",
        3 => "only what it removes",
        _ => "the treated voice",
    }
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
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_sift";

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

fn keep_centre(r: Rect, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(r.left() + ((r.width() - w) / 2.0).trunc(), r.top() + ((r.height() - h) / 2.0).trunc()), vec2(w, h))
}

/// What both layouts do first: the scripted drawer tab ("More" opens the slim layout's section)
/// and the preset browser, which covers the whole editor while it is open (then `true`: paint
/// nothing else).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> bool {
    if !st.init {
        st.init = true;
        st.fam.presets.category = "All".into();
    }
    if family::take_drawer_tab(app, host, &mut st.fam).as_deref() == Some("More") && st.slim {
        st.slim_more = true;
    }
    family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, "Vocals")
}

/// The timeline in `r`: its panel, a legend with the cuts of the moment, and the scope.
fn display_ui(ui: &Ui, host: &Host<'_>, r: Rect, compact: bool) {
    let pal = &PALETTE;
    let p = ui.painter().clone();
    paint_panel(&p, r, 10.0);
    let b = r.shrink2(if compact { vec2(8.0, 8.0) } else { vec2(12.0, 10.0) });
    let head_y = b.top() + 9.0;
    let mut x = b.left();
    let legend: &[(Color32, &str)] = if compact {
        &[(pal.accent, "Sibilance"), (SIFT_PLOSIVE, "Plosives")]
    } else {
        &[(al(pal.text_body, 0.45), "Voice"), (pal.accent, "Sibilance"), (SIFT_PLOSIVE, "Plosives")]
    };
    for (c, t) in legend.iter().copied() {
        p.rect_filled(Rect::from_center_size(pos2(x + 5.0, head_y), vec2(10.0, 10.0)), CornerRadius::same(2), c);
        let g = p.text(pos2(x + 15.0, head_y), Align2::LEFT_CENTER, t, regular(CAPTION_SIZE), pal.text_body);
        x = g.right() + 14.0;
    }
    let live = host.meters.and_then(|m| m.modules.iter().find(|x| x.module == host.ins.id)).map(crate::chain_view::live_values);
    let reading = |i: usize| live.as_ref().and_then(|l| l.readouts.get(i).copied()).filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0);
    let text = if compact {
        format!("\u{2212}{:.1} / \u{2212}{:.1} dB", reading(0), reading(1))
    } else {
        format!("Sibilance \u{2212}{:.1} dB   Plosives \u{2212}{:.1} dB", reading(0), reading(1))
    };
    p.text(pos2(b.right(), head_y), Align2::RIGHT_CENTER, text, regular(CAPTION_SIZE), pal.text_body);
    let plot = Rect::from_min_max(pos2(b.left(), b.top() + if compact { 18.0 } else { 22.0 }), b.max);
    p.rect_filled(plot, CornerRadius::same(3), argb(0xFF0C_0F12));
    let streams = crate::chain_view::streams_for(ui, host.track, host.ins, host.meters);
    sift_timeline(&p.with_clip_rect(plot.intersect(p.clip_rect())), plot, streams.as_deref(), pal.accent, false);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

/// One dial in `r`.
#[allow(clippy::too_many_arguments)]
fn dial_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, id: &str, label: &str, small: bool) {
    let value_text = value_text(host, id);
    let o = DialOpts { label, value_text: &value_text, default: family::default01(host, id), small, ..DialOpts::default() };
    family::dial_param(app, ui, host, fam, &FAMILY, r, &PALETTE, id, &o);
}

/// A choice parameter as a row of segments in `r` (a pick is one undo step).
fn seg_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, id: &str) {
    family::segmented_param(app, ui, host, fam, &FAMILY, r, &PALETTE, id, None, 1, SelectionStyle::Filled, false);
}

/// A section: its panel, its heading in `colour`, and its dials (the big one first, the small
/// ones sharing what is left).
#[allow(clippy::too_many_arguments)]
fn section(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, title: &str, colour: Color32, dials: &[(&str, &str, bool)]) {
    let p = ui.painter().clone();
    paint_panel(&p, r, 10.0);
    let k = r.shrink(K_PANEL_PAD);
    p.rect_filled(Rect::from_center_size(pos2(k.left() + 5.0, k.top() + K_HEAD_H * 0.5 - 2.0), vec2(10.0, 10.0)), CornerRadius::same(2), colour);
    p.text(pos2(k.left() + 16.0, k.top() + K_HEAD_H * 0.5 - 2.0), Align2::LEFT_CENTER, title, regular(LABEL_SIZE), PALETTE.text_bright);
    let body = Rect::from_min_max(pos2(k.left(), k.top() + K_HEAD_H), k.max);
    let big_w = K_BIG_KNOB + 28.0;
    let small = dials.len().saturating_sub(1).max(1) as f32;
    let small_w = ((body.width() - big_w) / small).trunc();
    let mut x = body.left();
    for (id, label, big) in dials.iter().copied() {
        let w = if big { big_w } else { small_w };
        let cell = Rect::from_min_size(pos2(x, body.top()), vec2(w, body.height()));
        let knob = if big { K_BIG_KNOB } else { K_SMALL_KNOB };
        let rect = keep_centre(cell, if big { knob } else { (w - 4.0).min(110.0) }, knob + K_DIAL_TEXT);
        dial_ui(app, ui, host, fam, rect, id, label, !big);
        x += w;
    }
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let pal = &PALETTE;
    let p = ui.painter().clone();
    paint_background(&p, host.rect);

    // ---- layout
    let mut b = host.rect.shrink2(vec2(K_PAD_X, K_PAD_Y));
    let header = Rect::from_min_size(b.min, vec2(b.width(), K_HEADER_H));
    b.min.y += K_HEADER_H + K_GAP;
    let display_r = Rect::from_min_size(b.min, vec2(b.width(), K_DISPLAY_H));
    b.min.y += K_DISPLAY_H + K_GAP;
    // Sibilance has four dials and Plosives three: 56 % and 44 %, a gap between.
    let sib_w = ((b.width() - K_GAP) * 0.56).trunc();
    let sib_r = Rect::from_min_size(b.min, vec2(sib_w, K_PANEL_H));
    let plo_r = Rect::from_min_max(pos2(sib_r.right() + K_GAP, b.top()), pos2(b.right(), b.top() + K_PANEL_H));
    b.min.y += K_PANEL_H + K_GAP;
    let row = Rect::from_min_size(b.min, vec2(b.width(), K_ROW_H));
    b.min.y += K_ROW_H + K_GAP;
    let foot = Rect::from_min_size(b.min, vec2(b.width(), K_FOOT_H));

    // ---- the header
    paint_title(&p, header, "Sift", "", pal);
    let rects = family::right_aligned_header(ui, header);
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &rects);

    // ---- the timeline
    display_ui(ui, host, display_r, false);

    // ---- the two sections
    section(app, ui, host, &mut st.fam, sib_r, "Sibilance", pal.accent, &SIB_DIALS);
    section(app, ui, host, &mut st.fam, plo_r, "Plosives", SIFT_PLOSIVE, &PLO_DIALS);

    // ---- Mode under Sibilance, Listen under Plosives and on to the right edge
    let label = |x: f32, text: &str| p.text(pos2(x, row.center().y), Align2::LEFT_CENTER, text, regular(LABEL_SIZE), pal.text_body).right() + 10.0;
    let x = label(row.left(), "Mode");
    seg_ui(app, ui, host, &mut st.fam, Rect::from_min_max(pos2(x, row.top()), pos2(x + 150.0, row.bottom())), "mode");
    let x = label(x + 150.0 + 28.0, "Listen");
    let listen_r = Rect::from_min_max(pos2(x, row.top()), pos2(x + 340.0, row.bottom()));
    seg_ui(app, ui, host, &mut st.fam, listen_r, "listen");
    p.text(pos2(listen_r.right() + 12.0, row.center().y), Align2::LEFT_CENTER, listen_note(host), regular(CAPTION_SIZE), pal.text_faint);

    p.with_clip_rect(foot.intersect(p.clip_rect())).text(
        pos2(foot.left(), foot.center().y),
        Align2::LEFT_CENTER,
        FOOT_NOTE,
        regular(CAPTION_SIZE),
        pal.text_faint,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_and_readouts() {
        assert_eq!(SIZE, vec2(860.0, 536.0));
        assert_eq!(hz_text(180.0), "180 Hz");
        assert_eq!(hz_text(4_500.0), "4.5 kHz");
    }
}

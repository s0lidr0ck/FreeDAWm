//! Manners' wide editor. Manners is Rust-native, so unlike the other editors here this is not a
//! translation of a JUCE editor: it is laid out for this app with the same family look
//! ([`crate::editor_kit`]): the header (presets, Undo, A/B, Bypass), the display ([`display`]:
//! the input and output spectra, the gain curve being applied and the working range, whose two
//! edges drag), and the dials. [`slim`] is the same editor in the classic drawer's width.

mod display;
mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, run_with, set_params_merged};
use crate::theme::regular;
use egui::{Align2, Rect, Ui, Vec2, pos2, vec2};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

// ============================================================================ constants

const K_PAD_X: f32 = 24.0;
const K_PAD_Y: f32 = 15.0;
const K_GAP: f32 = 14.0;
const K_HEADER_H: f32 = 26.0;
const K_DISPLAY_H: f32 = 250.0;
const K_BIG_KNOB: f32 = 104.0;
const K_SMALL_KNOB: f32 = 62.0;
/// A dial's label and readout under its knob.
const K_DIAL_TEXT: f32 = 30.0;
const K_PANEL_PAD: f32 = 10.0;
const K_PANEL_H: f32 = K_BIG_KNOB + K_DIAL_TEXT + K_PANEL_PAD * 2.0;
const K_FOOT_H: f32 = 16.0;
const K_HEIGHT: f32 = K_PAD_Y * 2.0 + K_HEADER_H + K_GAP + K_DISPLAY_H + K_GAP + K_PANEL_H + K_GAP + K_FOOT_H;

/// The editor's size.
pub const SIZE: Vec2 = vec2(860.0, K_HEIGHT);

/// The accent: the module's own (`engines/manners.rs`).
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFF6F_C7B8));

/// The preset catalogue's key; every parameter but `bypass` is in a preset.
const FAMILY: Family = Family::new("Manners", &[]);

const FOOT_NOTE: &str =
    "Tame turns down what covers its neighbours \u{00B7} Recover lifts what is covered \u{00B7} drag the edges of the display to set where it works";

/// The dials, left to right: (parameter, label, big).
const DIALS: [(&str, &str, bool); 5] =
    [("tame", "Tame", true), ("recover", "Recover", true), ("bias", "Bias", false), ("brighten", "Brighten", false), ("boost", "Boost", false)];

// ============================================================================ formatting

fn percent(v: f32) -> String {
    format!("{} %", v.round() as i32)
}

/// A two-sided knob's readout: what it leans towards, or "Even" in the middle.
fn leaning(v: f32, below: &str, above: &str) -> String {
    let n = v.round() as i32;
    match n {
        0 => "Even".into(),
        n if n < 0 => format!("{} % {below}", -n),
        n => format!("{n} % {above}"),
    }
}

pub(crate) fn hz_text(hz: f32) -> String {
    if hz >= 1000.0 { format!("{:.1} kHz", hz / 1000.0) } else { format!("{} Hz", hz.round() as i32) }
}

/// A two-sided knob's readout where there is no room for words: signed percent.
fn signed(v: f32) -> String {
    match v.round() as i32 {
        0 => "Even".into(),
        n if n < 0 => format!("\u{2212}{} %", -n),
        n => format!("+{n} %"),
    }
}

/// A dial's readout (`short`: the slim layout's).
fn value_text(host: &Host, id: &str, short: bool) -> String {
    let v = param(host, id);
    match id {
        "bias" | "brighten" | "boost" if short => signed(v),
        "bias" => leaning(v, "Tame", "Recover"),
        "brighten" => leaning(v, "darker", "brighter"),
        "boost" => leaning(v, "less bass", "more bass"),
        _ => percent(v),
    }
}

// ============================================================================ the editor

#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    /// The range edge being dragged (0: low, 1: high).
    dragging: Option<usize>,
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
    const PLUGIN: &'static str = "fs_manners";

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
    family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, "Balance")
}

/// The display in `r` and its range edges' gestures (a drag is one undo step).
fn display_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>, r: Rect, compact: bool) {
    let streams = crate::chain_view::streams_for(ui, host.track, host.ins, host.meters);
    let live = host.meters.and_then(|m| m.modules.iter().find(|x| x.module == host.ins.id)).map(crate::chain_view::live_values);
    let range = (param(host, "rangeLo"), param(host, "rangeHi"));
    let out = display::draw(ui, r, &PALETTE, streams.as_deref(), live.as_ref(), range, host.sr as f32, &mut st.dragging, compact);
    if out.started {
        end_gesture(app);
        family::push_undo(&mut st.fam, host, &FAMILY);
    }
    if let Some((edge, hz)) = out.moved {
        let id = if edge == 0 { "rangeLo" } else { "rangeHi" };
        if let Some(info) = host.info.param(id) {
            set_params_merged(app, host, &[(id, info.clamp(hz))], &format!("range-{edge}"));
        }
    }
    if out.ended {
        end_gesture(app);
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

/// One dial in `r`.
#[allow(clippy::too_many_arguments)]
fn dial_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, id: &str, label: &str, small: bool, short: bool) {
    let value_text = value_text(host, id, short);
    let o = DialOpts { label, value_text: &value_text, default: family::default01(host, id), small, ..DialOpts::default() };
    family::dial_param(app, ui, host, fam, &FAMILY, r, &PALETTE, id, &o);
}

/// The Delta switch centred on `c`, and what it is letting through.
fn delta_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, c: egui::Pos2) -> &'static str {
    let pill_w = family::small_pill_w(ui, "Delta").max(64.0);
    let pill_r = Rect::from_center_size(c, vec2(pill_w, crate::editor_kit::controls::pill_height(crate::editor_kit::controls::PillStyle::Small)));
    let look = PillLook { tracked: false, ..PillLook::default() };
    family::pill_param(app, ui, host, fam, &FAMILY, pill_r, &PALETTE, "delta", "Delta", look);
    if param(host, "delta") > 0.5 { "only what it changes" } else { "the whole signal" }
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
    let panel = Rect::from_min_size(b.min, vec2(b.width(), K_PANEL_H));
    b.min.y += K_PANEL_H + K_GAP;
    let foot = Rect::from_min_size(b.min, vec2(b.width(), K_FOOT_H));

    // ---- the header
    paint_title(&p, header, "Manners", "", pal);
    let rects = family::right_aligned_header(ui, header);
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &rects);

    // ---- the display, with its range edges
    display_ui(st, app, ui, host, display_r, false);

    // ---- the dials and Delta
    paint_panel(&p, panel, 10.0);
    let k = panel.shrink(K_PANEL_PAD);
    let cell_w = (k.width() / 6.0).trunc();
    for (i, (id, label, big)) in DIALS.into_iter().enumerate() {
        let cell = Rect::from_min_size(pos2(k.left() + cell_w * i as f32, k.top()), vec2(cell_w, k.height()));
        let knob = if big { K_BIG_KNOB } else { K_SMALL_KNOB };
        let r = keep_centre(cell, if big { knob } else { (cell_w - 4.0).min(110.0) }, knob + K_DIAL_TEXT);
        dial_ui(app, ui, host, &mut st.fam, r, id, label, !big, false);
    }
    let listen = Rect::from_min_max(pos2(k.left() + cell_w * 5.0, k.top()), k.max);
    p.text(pos2(listen.center().x, listen.center().y - 30.0), Align2::CENTER_CENTER, "Listen", regular(LABEL_SIZE), pal.text_body);
    let delta_note = delta_ui(app, ui, host, &mut st.fam, listen.center());
    p.text(pos2(listen.center().x, listen.center().y + 28.0), Align2::CENTER_CENTER, delta_note, regular(CAPTION_SIZE), pal.text_faint);

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
        assert_eq!(SIZE, vec2(860.0, 518.0));
        assert_eq!(percent(25.0), "25 %");
        assert_eq!(leaning(0.2, "Tame", "Recover"), "Even");
        assert_eq!(leaning(-40.0, "Tame", "Recover"), "40 % Tame");
        assert_eq!(leaning(100.0, "darker", "brighter"), "100 % brighter");
        assert_eq!(signed(-30.0), "\u{2212}30 %");
        assert_eq!(signed(12.0), "+12 %");
        assert_eq!(signed(0.0), "Even");
        assert_eq!(hz_text(20.0), "20 Hz");
        assert_eq!(hz_text(2_500.0), "2.5 kHz");
    }
}

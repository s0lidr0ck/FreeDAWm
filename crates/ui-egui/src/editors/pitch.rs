//! PitchEngine's VST editor (`fs_pitch`), a translation of `Engine 3 - Pitch/Source/
//! PluginEditor.cpp` on the editor kit: the family look tinted #7BD4FF, `setSize (900,
//! requiredHeight())` (412, or 451 with the 5th Mode / Interval switch row), the header, the
//! style row and its blurb, the main knob row (per style: which knobs and what they are called),
//! the switch row, the shaping row, and the foot line (the OC-2 lamp and what the style is doing).
//!
//! Controls that do not apply to the style are hidden, not greyed (the family's rule), so the
//! height follows the style and the 5th knob. The tables are the dsp port's (`pitch_config`).

mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillStyle, SelectionStyle, pill_height, segmented_ex};
use crate::editor_kit::family::{self, Family, FamilyState, HeaderRects};
use crate::editor_kit::look::{self, BLURB_SIZE, CAPTION_SIZE, font, tracked_width};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, controls};
use egui::{Align2, Rect, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::pitch::pitch_config as cfg;
use soundcraft_dsp::plugins::fs::pitch::pitch_engine_dsp as d;
use soundcraft_dsp::plugins::fs::pitch::{INTERVALS, PITCH_PARAMS};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

// ---- resized()'s constants, by name
const PAD_X: f32 = 22.0;
const PAD_Y: f32 = 14.0;
const GAP: f32 = 13.0;
const HEADER_H: f32 = 26.0;
const STYLE_ROW_H: f32 = 30.0;
const BLURB_H: f32 = 16.0;
const MAIN_KNOB: f32 = 104.0;
/// The knob, then value and name under it.
const MAIN_ROW_H: f32 = MAIN_KNOB + 32.0;
const SWITCH_ROW_H: f32 = 26.0;
const SHAPE_KNOB: f32 = 72.0;
const SHAPE_ROW_H: f32 = SHAPE_KNOB + 30.0;
const FOOT_H: f32 = 22.0;
const DEFAULT_WIDTH: f32 = 900.0;

/// `kAccent`: the original UI's PEDAL.accent.
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFF7BD4FF));

const FAMILY: Family = Family::new("PitchEngine", &["trim", "level"]);

/// The main row's dials in `resized()`'s order (Detune joins at the end on H910).
const MAIN_ORDER: [usize; 9] = [d::P_SUB, d::P_OCT_DOWN, d::P_DRY, d::P_OCT_UP, d::P_OCT2_UP, d::P_FIFTH, d::P_VOICE, d::P_PEDAL, d::P_GLIDE];

#[derive(Clone, Default)]
pub(crate) struct Editor {
    fam: FamilyState,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's More section is open.
    slim_more: bool,
    /// Scripts: open the slim layout's Style or Interval menu on the next frame.
    style_open: bool,
    interval_open: bool,
}

/// Draws PitchEngine's editor for `module` on `track`.
pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

fn id(i: usize) -> &'static str {
    PITCH_PARAMS.get(i).map_or("", |p| p.id)
}

/// A parameter of `ins` as the plugin clamps it.
fn pv(ins: &Insert, i: usize) -> f32 {
    PITCH_PARAMS.get(i).map_or(0.0, |p| p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default)))
}

fn style_of(ins: &Insert) -> i32 {
    (pv(ins, d::P_STYLE).round() as i32).clamp(0, d::NUM_STYLES - 1)
}

/// `requiredHeight()`.
pub fn required_height(style: i32, fifth: f32) -> f32 {
    let a = cfg::what_applies(style, fifth);
    let mut h = PAD_Y * 2.0 + HEADER_H + GAP + STYLE_ROW_H + BLURB_H + GAP + MAIN_ROW_H + GAP + SHAPE_ROW_H + GAP + FOOT_H;
    if a.fifth_mode || a.interval {
        h += SWITCH_ROW_H + GAP;
    }
    h
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_pitch";

    fn size(&self, ins: &Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { vec2(DEFAULT_WIDTH, required_height(style_of(ins), pv(ins, d::P_FIFTH))) }
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        if self.slim {
            slim::editor_ui(self, app, ui, host);
        } else {
            editor_ui(self, app, ui, host);
        }
    }
}

/// `n` equal columns of `row` (`row.getWidth() / n`, integer pixels, from the left).
fn columns(row: Rect, n: usize) -> impl Iterator<Item = Rect> {
    let w = if n == 0 { 0.0 } else { (row.width() as i32 / n as i32) as f32 };
    (0..n).map(move |k| Rect::from_min_size(pos2(row.left() + w * k as f32, row.top()), vec2(w, row.height())))
}

fn take_top(b: &mut Rect, h: f32) -> Rect {
    let r = Rect::from_min_size(b.min, vec2(b.width(), h));
    b.min.y += h;
    r
}

/// What both layouts do first: the scripted drawer tabs and the preset browser, which covers the
/// whole editor while it is open (then `true`: paint nothing else).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> bool {
    // "Style", "Interval" (the menus) and "More" are the slim layout's.
    match family::take_drawer_tab(app, host, &mut st.fam).as_deref() {
        Some("Style") if st.slim => st.style_open = true,
        Some("Interval") if st.slim => st.interval_open = true,
        Some("More") if st.slim => st.slim_more = true,
        _ => {}
    }
    let style = style_of(host.ins);
    family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, cfg::STYLES[style as usize])
}

/// The main row's dials for `style`, in `resized()`'s order.
fn main_ids(style: i32) -> Vec<usize> {
    let row_ids = cfg::main_row_for(style);
    let mut main: Vec<usize> = MAIN_ORDER.iter().copied().filter(|i| row_ids.contains(i)).collect();
    if style == d::STYLE_H910 && row_ids.contains(&d::P_DETUNE) {
        main.push(d::P_DETUNE);
    }
    main
}

/// The shaping row's dials for `style` (Detune first on POG).
fn shape_ids(style: i32, a: &cfg::Applicable) -> Vec<usize> {
    let mut shape: Vec<usize> = Vec::with_capacity(9);
    let detune_in_main = cfg::main_row_for(style).contains(&d::P_DETUNE);
    if !detune_in_main && a.detune && style != d::STYLE_H910 {
        shape.push(d::P_DETUNE);
    }
    for (i, on) in [
        (d::P_ATTACK, a.attack),
        (d::P_LPF, true),
        (d::P_RESO, true),
        (d::P_WIDTH, a.width),
        (d::P_GRAIN, a.grain),
        (d::P_TRACK, a.track),
        (d::P_TRIM, true),
        (d::P_LEVEL, true),
    ] {
        if on {
            shape.push(i);
        }
    }
    shape
}

/// The OC-2 lamp (the engine's readout 2).
fn lamp_of(host: &Host) -> f32 {
    host.meters.and_then(|m| m.modules.iter().find(|x| x.module == host.ins.id)).map_or(0.0, |m| m.readouts[2])
}

/// The foot line: what the style is doing.
fn foot_text(ins: &Insert, style: i32, lamp: f32) -> String {
    if style == d::STYLE_OC2 {
        format!(
            "{}   \u{00B7}   monophonic: one note at a time, and it glitches on chords by design",
            if lamp > 0.5 { "Locked on a note" } else { "Waiting for a note" }
        )
    } else if style == d::STYLE_WHAMMY {
        format!("{}   \u{00B7}   map an expression pedal to Pedal", cfg::whammy_readout(pv(ins, d::P_INTERVAL).round() as i32, pv(ins, d::P_PEDAL)))
    } else {
        format!(
            "Voices are about {:.1} ms behind the dry, which is half the window   \u{00B7}   Grain sets the window: smaller is tighter, and warbles more",
            d::latency_ms(style, pv(ins, d::P_GRAIN))
        )
    }
}

/// The 5th's Mode switch in `r`.
fn fifth_mode_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut Editor, r: Rect) {
    let sel = pv(host.ins, d::P_FIFTH_MODE).round() as usize;
    if let Some(i) = segmented_ex(ui, r, "fifthMode", &PALETTE, PITCH_PARAMS[d::P_FIFTH_MODE].choices, 1, Some(sel), SelectionStyle::Filled, false) {
        family::set(app, host, &mut st.fam, &FAMILY, "fifthMode", i as f32);
    }
}

/// The OC-2 lamp and the Whammy readout follow the audio (`startTimerHz (20)`).
fn timer(ui: &Ui, host: &Host, style: i32) {
    if host.meters.is_some() && (style == d::STYLE_OC2 || style == d::STYLE_WHAMMY) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let pal = &PALETTE;
    let ins = host.ins;
    let style = style_of(ins);
    let fifth = pv(ins, d::P_FIFTH);
    let a = cfg::what_applies(style, fifth);
    let p = ui.painter().clone();
    look::paint_background(&p, host.rect);

    // ---- resized()
    let mut b = host.rect.shrink2(vec2(PAD_X, PAD_Y));
    let pill_h = pill_height(PillStyle::Small);
    let header_area = take_top(&mut b, HEADER_H);
    let header = {
        let mut row = header_area;
        let right = Rect::from_min_max(pos2(row.right() - 72.0, row.top()), row.max);
        row.max.x = right.left();
        let bypass = Rect::from_center_size(right.center(), vec2(66.0, pill_h));
        row.min.x += (110f32).max((tracked_width(&p, "PitchEngine", 16.0, 0.0, true) + 16.0).trunc());
        let undo_w = family::small_pill_w(ui, "Undo").trunc();
        let (ab_w, arrow_w) = (28.0, 26.0);
        let name_w = (row.width() - (arrow_w * 2.0 + undo_w + ab_w * 2.0 + 36.0)).trunc().clamp(120.0, 260.0);
        let strip_w = arrow_w * 2.0 + name_w + undo_w + ab_w * 2.0 + 30.0;
        let strip = Rect::from_center_size(row.center(), vec2(strip_w.min(row.width()), pill_h));
        let mut x = strip.left();
        let mut take = |w: f32, gap: f32| {
            let r = Rect::from_min_size(pos2(x, strip.top()), vec2(w, pill_h));
            x += w + gap;
            r
        };
        HeaderRects {
            prev: take(arrow_w, 6.0),
            name: take(name_w, 6.0),
            next: take(arrow_w, 6.0),
            undo: take(undo_w, 6.0),
            a: take(ab_w, 4.0),
            b: take(ab_w, 0.0),
            bypass,
        }
    };
    b.min.y += GAP;
    let style_r = take_top(&mut b, STYLE_ROW_H);
    let blurb_area = take_top(&mut b, BLURB_H);
    b.min.y += GAP;
    let main_row = take_top(&mut b, MAIN_ROW_H);
    b.min.y += GAP;
    let switch_row = (a.fifth_mode || a.interval).then(|| {
        let r = take_top(&mut b, SWITCH_ROW_H);
        b.min.y += GAP;
        r
    });
    let shape_row = take_top(&mut b, SHAPE_ROW_H);
    b.min.y += GAP;
    let mut foot_area = take_top(&mut b, FOOT_H);
    let lamp_area = a.track.then(|| {
        let l = Rect::from_min_size(foot_area.min, vec2(16.0, foot_area.height()));
        foot_area.min.x += 16.0 + 6.0;
        l
    });

    // ---- paint()
    look::paint_title(&p, header_area, "Pitch", "Engine", pal);
    p.text(pos2(blurb_area.left(), blurb_area.center().y), Align2::LEFT_CENTER, cfg::style_blurb(style), font(BLURB_SIZE, false), pal.text_body);
    let lamp = lamp_of(host);
    let foot = foot_text(ins, style, lamp);
    p.with_clip_rect(foot_area.intersect(p.clip_rect())).text(
        pos2(foot_area.left(), foot_area.center().y),
        Align2::LEFT_CENTER,
        foot,
        font(CAPTION_SIZE, false),
        pal.text_faint,
    );
    if let Some(l) = lamp_area {
        controls::led(&p, l, pal, lamp > 0.5, pal.accent);
    }

    // ---- the header
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &header);

    // ---- style
    if let Some(i) = segmented_ex(ui, style_r, "style", pal, &cfg::STYLES, 1, Some(style as usize), SelectionStyle::Filled, false) {
        family::set(app, host, &mut st.fam, &FAMILY, "style", i as f32);
    }

    // ---- the main row: whichever knobs this style shows, evenly spread
    let main = main_ids(style);
    for (r, &i) in columns(main_row, main.len()).zip(&main) {
        dial(app, ui, host, st, r, i, style, false);
    }

    // ---- the switch row
    if let Some(row) = switch_row {
        if a.interval {
            let sel = pv(ins, d::P_INTERVAL).round() as usize;
            if let Some(i) = segmented_ex(ui, row, "interval", pal, &INTERVALS, 1, Some(sel), SelectionStyle::Filled, false) {
                family::set(app, host, &mut st.fam, &FAMILY, "interval", i as f32);
            }
        }
        if a.fifth_mode {
            fifth_mode_ui(app, ui, host, st, Rect::from_center_size(row.center(), vec2(row.width().min(300.0), SWITCH_ROW_H)));
        }
    }

    // ---- the shaping row (Detune first on POG)
    let shape = shape_ids(style, &a);
    for (r, &i) in columns(shape_row, shape.len()).zip(&shape) {
        dial(app, ui, host, st, r, i, style, false);
    }

    timer(ui, host, style);
}

/// One `fs::Dial` (`look::styleDial`: the family's knob, mixed-case labels), named per style
/// (`knobLabel`) and printed by `formatValue`.
#[allow(clippy::too_many_arguments)]
fn dial(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut Editor, r: Rect, i: usize, style: i32, small: bool) {
    let pid = id(i);
    let label = cfg::knob_label(style, i).unwrap_or_else(|| PITCH_PARAMS.get(i).map_or("", |p| p.name));
    let text = cfg::format_value(i, family::value01(host, pid));
    let o = DialOpts { label, value_text: &text, default: family::default01(host, pid), small, ..DialOpts::default() };
    family::dial_param(app, ui, host, &mut st.fam, &FAMILY, r, &PALETTE, pid, &o);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_heights_follow_the_style_and_the_fifth() {
        assert_eq!(required_height(d::STYLE_POG, 0.0), 412.0);
        assert_eq!(required_height(d::STYLE_POG, 0.5), 451.0);
        assert_eq!(required_height(d::STYLE_WHAMMY, 0.0), 451.0);
        assert_eq!(required_height(d::STYLE_OC2, 1.0), 412.0);
    }
}

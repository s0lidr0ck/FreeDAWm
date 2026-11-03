//! DistEngine's VST editor (`fs_dist`), a translation of `Engine 2 - Dist/Source/PluginEditor.cpp`
//! (+ `PluginEditor.h`'s `SwitchRow`, `PedalConfig.h`) on the editor kit: the family look tinted
//! #FF7A2F at `setSize (860, requiredHeight())` = 306 + the circuit's switch rows (n x 52 +
//! (n - 1) x 6), the header, the two-row Circuit selector, Input / Drive / Tone / Level named per
//! pedal, Link / the set points / Calibrate beside Level, the circuit's switch rows with each
//! option's description, and the calibration wizard.
//!
//! A/B and undo take the whole rig with the per-circuit knob bank (`getAbParams` + the bank,
//! restored through `mix.module_params {memory}` so no knob swap fires); the Bypass and Link pills
//! do not push undo, and the wizard's own moves never do (`pushUndo`'s guards).

mod descriptions;
mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, SelectionStyle, pill_ex, pill_height, segmented_ex};
use crate::editor_kit::family::{self, Family, FamilyState, HeaderRects};
use crate::editor_kit::link_cal::{self, CalSpec};
use crate::editor_kit::look::{self, CAPTION_SIZE, LABEL_SIZE, font};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb};
use egui::{Align2, Rect, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::dist::DIST_PARAMS;
use soundcraft_dsp::plugins::fs::dist::wrapper::{self as cfg, CircuitDef};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

// ---- resized()'s constants, by name
const PAD_X: f32 = 24.0;
const PAD_Y: f32 = 15.0;
const GAP: f32 = 16.0;
const HEADER_H: f32 = 26.0;
/// Two rows of six, so the circuit names stay readable.
const CIRCUIT_ROW_H: f32 = 62.0;
/// Link / readout / Calibrate, beside the Level knob.
const LINK_COL_W: f32 = 132.0;
const KNOB_MIN: f32 = 84.0;
const KNOB_MAX: f32 = 150.0;
const KNOB_DEFAULT: f32 = 110.0;
const DEFAULT_WIDTH: f32 = 860.0;

/// `SwitchRow`.
const ROW_H: f32 = 28.0;
const DESC_H: f32 = 20.0;
const LABEL_W: f32 = 48.0;
const LABEL_GAP: f32 = 14.0;
const SWITCH_ROW_PREF_H: f32 = ROW_H + 4.0 + DESC_H;

/// `kAccent`.
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFFFF7A2F));

/// `getPresetParams` (circuit, Drive, Tone and the switches); A/B and undo take the whole rig.
const FAMILY: Family = Family {
    whole_rig: true,
    pills_push_undo: false,
    ..Family::new(
        "DistEngine",
        &[
            "level", "trim", "link", "tsLo", "tsHi", "ratLo", "ratHi", "muffLo", "muffHi", "ffLo", "ffHi", "ds1Lo", "ds1Hi", "klonLo", "klonHi",
            "bbLo", "bbHi", "govLo", "govHi", "tdLo", "tdHi", "bdLo", "bdHi", "dpLo", "dpHi", "ocdLo", "ocdHi",
        ],
    )
};

#[derive(Clone, Default)]
pub(crate) struct Editor {
    fam: FamilyState,
    cal: link_cal::Cal,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's More section is open.
    slim_more: bool,
    /// Scripts: open the slim layout's Circuit menu on the next frame.
    circuit_open: bool,
}

/// Draws DistEngine's editor for `module` on `track`.
pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

fn pv(ins: &Insert, id: &str) -> f32 {
    DIST_PARAMS.iter().find(|p| p.id == id).map_or(0.0, |p| p.clamp(ins.params.get(id).copied().unwrap_or(p.default)))
}

fn choices(id: &str) -> &'static [&'static str] {
    DIST_PARAMS.iter().find(|p| p.id == id).map_or(&[], |p| p.choices)
}

fn circuit_of(ins: &Insert) -> (i32, &'static CircuitDef) {
    let i = (pv(ins, "circuit").round() as i32).clamp(0, cfg::CIRCUITS.len() as i32 - 1);
    (i, cfg::circuit(i))
}

/// `requiredHeight()` for a circuit with `rows` switch rows.
pub fn required_height(rows: usize) -> f32 {
    let n = rows as f32;
    let rows_h = if rows == 0 { 0.0 } else { n * SWITCH_ROW_PREF_H + (n - 1.0) * 6.0 };
    HEADER_H + GAP + CIRCUIT_ROW_H + GAP + GAP + rows_h + KNOB_DEFAULT + 30.0 + 2.0 * PAD_Y
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_dist";

    fn size(&self, ins: &Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { vec2(DEFAULT_WIDTH, required_height(circuit_of(ins).1.switches.len())) }
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        if self.slim {
            slim::editor_ui(self, app, ui, host);
        } else {
            editor_ui(self, app, ui, host);
        }
    }
}

fn take_top(b: &mut Rect, h: f32) -> Rect {
    let r = Rect::from_min_size(b.min, vec2(b.width(), h));
    b.min.y += h;
    r
}

/// `formatTenths`.
fn tenths(v: f32) -> String {
    format!("{}", (v * 10.0).round() as i32)
}

/// `formatInputDb`.
fn input_db(v: f32) -> String {
    let mut db = -12.0 + 36.0 * v;
    if db.abs() < 0.05 {
        db = 0.0;
    }
    format!("{}{db:.1} dB", if db >= 0.0 { "+" } else { "" })
}

/// The wizard for circuit `c` (`calRender`'s texts, `currentCircuit()`'s anchors and set points).
fn cal_spec(c: &CircuitDef) -> CalSpec<'static> {
    let n = c.dial_drive;
    CalSpec {
        drive: "drive",
        drive_name: n,
        anchor_lo: c.anchor_lo,
        anchor_hi: c.anchor_hi,
        link_lo: c.link_lo,
        link_hi: c.link_hi,
        input_text: format!(
            "{n} is at 5 and the pedal is on. Turn Input until the pedal reacts the way you expect: a little hair when you play soft, real grit when you dig in. Humbuckers usually want less, single-coils more."
        ),
    }
}

fn plain() -> PillLook {
    PillLook { tracked: false, ..PillLook::default() }
}

/// What both layouts do first: the scripted drawer tabs ("Calibrate" starts the wizard; the slim
/// layout's "Circuit" opens its menu and "More" its More section) and the preset browser, which
/// covers the whole editor while it is open (then `true`: paint nothing else).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> bool {
    let (_, c) = circuit_of(host.ins);
    match family::take_drawer_tab(app, host, &mut st.fam).as_deref() {
        Some("Calibrate") if !st.cal.active => link_cal::start(app, host, &mut st.cal, &cal_spec(c)),
        Some("Circuit") if st.slim => st.circuit_open = true,
        Some("More") if st.slim => st.slim_more = true,
        _ => {}
    }
    st.fam.hold_undo = st.cal.active;
    family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, c.name)
}

/// The four dials as `resized()` names them for circuit `c`: (param, label, small).
fn dial_defs(c: &CircuitDef) -> [(&'static str, &'static str, bool); 4] {
    [("trim", "Input", true), ("drive", c.dial_drive, false), ("tone", c.dial_tone, false), ("level", c.dial_level, false)]
}

/// One dial in `r` (Level locked while Linked, outside the wizard; the wizard's focus ring).
fn dial_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut Editor, r: Rect, pid: &str, label: &str, small: bool) {
    let linked = pv(host.ins, "link") >= 0.5 && !st.cal.active;
    let v = family::value01(host, pid);
    let text = if pid == "trim" { input_db(v) } else { tenths(v) };
    let cal_focus = (pid == "trim" && st.cal.input_focus()) || (pid == "level" && st.cal.level_focus());
    let o = DialOpts {
        label,
        value_text: &text,
        default: family::default01(host, pid),
        small,
        locked: pid == "level" && linked,
        cal_focus,
        ..DialOpts::default()
    };
    family::dial_param(app, ui, host, &mut st.fam, &FAMILY, r, &PALETTE, pid, &o);
}

/// Link's readout: the set points ("0 → 7   ·   10 → 5").
fn link_text(ins: &Insert, c: &CircuitDef) -> String {
    format!(
        "{} \u{2192} {}   \u{00B7}   {} \u{2192} {}",
        (c.anchor_lo * 10.0).round() as i32,
        (pv(ins, c.link_lo) * 10.0).round() as i32,
        (c.anchor_hi * 10.0).round() as i32,
        (pv(ins, c.link_hi) * 10.0).round() as i32
    )
}

/// The Link and Calibrate pills.
fn link_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut Editor, c: &CircuitDef, link_r: Rect, cal_r: Rect) {
    let pal = &PALETTE;
    let link_on = if st.cal.active { st.cal.saved_link >= 0.5 } else { pv(host.ins, "link") >= 0.5 };
    if pill_ex(ui, link_r, "link", pal, "Link", link_on, PillStyle::Small, plain()).clicked() {
        if st.cal.active {
            st.cal.saved_link = if link_on { 0.0 } else { 1.0 };
        } else {
            family::pill_set(app, host, &mut st.fam, &FAMILY, "link", if link_on { 0.0 } else { 1.0 });
        }
    }
    if pill_ex(ui, cal_r, "calibrate", pal, "Calibrate", false, PillStyle::Small, plain()).clicked() && !st.cal.active {
        link_cal::start(app, host, &mut st.cal, &cal_spec(c));
    }
}

/// A switch's current option and its description.
fn switch_state(ins: &Insert, pid: &str) -> (usize, &'static str) {
    let opts = choices(pid);
    let sel = (pv(ins, pid).round().max(0.0) as usize).min(opts.len().saturating_sub(1));
    let desc = descriptions::DESCRIPTIONS.iter().find(|(id, _)| *id == pid).and_then(|(_, d)| d.get(sel)).copied().unwrap_or("");
    (sel, desc)
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let pal = &PALETTE;
    let ins = host.ins;
    let (circuit, c) = circuit_of(ins);
    let p = ui.painter().clone();
    look::paint_background(&p, host.rect);

    // ---- resized()
    let full = host.rect.shrink2(vec2(PAD_X, PAD_Y));
    let rows = c.switches.len() as f32;
    let rows_h = if c.switches.is_empty() { 0.0 } else { rows * SWITCH_ROW_PREF_H + (rows - 1.0) * 6.0 };
    let fixed_h = HEADER_H + GAP + CIRCUIT_ROW_H + GAP + GAP + rows_h;
    let knob = (full.height() - fixed_h - 30.0).clamp(KNOB_MIN, KNOB_MAX);
    let mut b = full;
    let header_area = take_top(&mut b, HEADER_H);
    let btn_h = pill_height(PillStyle::Small);
    let header = {
        let mut right = header_area;
        let mut place_right = |w: f32| {
            let r = Rect::from_min_size(pos2(right.right() - w, right.center().y - btn_h * 0.5), vec2(w, btn_h));
            right.max.x -= w + 6.0;
            r
        };
        let bypass = place_right(family::small_pill_w(ui, "Bypass").trunc());
        let b_r = place_right(family::small_pill_w(ui, "B").trunc().max(28.0));
        let a_r = place_right(family::small_pill_w(ui, "A").trunc().max(28.0));
        let undo_w = family::small_pill_w(ui, "Undo").trunc();
        right.max.x -= 2.0;
        let undo = Rect::from_min_size(pos2(right.right() - undo_w, right.center().y - btn_h * 0.5), vec2(undo_w, btn_h));
        right.max.x -= undo_w + 6.0;
        let arrow_w = 24.0;
        let name_w = (right.width() - 2.0 * arrow_w - 12.0).trunc().clamp(140.0, 230.0);
        let strip = Rect::from_center_size(right.center(), vec2(name_w + 2.0 * arrow_w + 12.0, HEADER_H));
        let prev = Rect::from_center_size(pos2(strip.left() + arrow_w * 0.5, strip.center().y), vec2(arrow_w, 22.0));
        let next = Rect::from_center_size(pos2(strip.right() - arrow_w * 0.5, strip.center().y), vec2(arrow_w, 22.0));
        let name = Rect::from_min_max(pos2(prev.right() + 6.0, strip.center().y - 11.0), pos2(next.left() - 6.0, strip.center().y + 11.0));
        HeaderRects { prev, name, next, undo, a: a_r, b: b_r, bypass }
    };
    b.min.y += GAP;
    let mut circuit_row = take_top(&mut b, CIRCUIT_ROW_H);
    let circuit_label = Rect::from_min_size(circuit_row.min, vec2(LABEL_W, circuit_row.height()));
    circuit_row.min.x += LABEL_W + LABEL_GAP;
    b.min.y += GAP;

    // dials, with Link and Calibrate in a column beside Level
    let mut row = take_top(&mut b, knob + 30.0);
    let link_col = Rect::from_min_max(pos2(row.right() - LINK_COL_W, row.top()), row.max);
    row.max.x = link_col.left();
    let cell_w = (row.width() as i32 / 4) as f32;
    // The Input dial's 84:110 ratio.
    let small_knob = (knob * 0.764).round();
    let cell = |i: usize, k: f32, dy: f32| {
        let cx = row.left() + cell_w * i as f32 + cell_w * 0.5;
        Rect::from_min_size(pos2((cx - k * 0.5).trunc(), row.top() + dy), vec2(k, k + 30.0))
    };
    let defs = dial_defs(c);
    let dials = [cell(0, small_knob, ((knob - small_knob) / 2.0).trunc()), cell(1, knob, 0.0), cell(2, knob, 0.0), cell(3, knob, 0.0)];
    let stack_h = btn_h + 6.0 + 14.0 + 8.0 + btn_h;
    let stack = Rect::from_min_size(pos2(link_col.left() + 4.0, row.top() + ((knob - stack_h) / 2.0).trunc()), vec2(link_col.width() - 8.0, stack_h));
    let link_r = Rect::from_min_size(stack.min, vec2(family::small_pill_w(ui, "Link").trunc().max(64.0), btn_h));
    let link_readout = Rect::from_min_size(pos2(stack.left(), stack.top() + btn_h + 4.0), vec2(stack.width(), 14.0));
    let cal_r =
        Rect::from_min_size(pos2(stack.left(), link_readout.bottom() + 6.0), vec2(family::small_pill_w(ui, "Calibrate").trunc().max(78.0), btn_h));
    b.min.y += GAP;
    let switch_rects: Vec<Rect> = c
        .switches
        .iter()
        .map(|_| {
            let r = take_top(&mut b, SWITCH_ROW_PREF_H);
            b.min.y += 6.0;
            r
        })
        .collect();

    // ---- paint()
    look::paint_title(&p, header_area, "Dist", "Engine", pal);
    p.text(pos2(circuit_label.left(), circuit_label.center().y), Align2::LEFT_CENTER, "Circuit", font(LABEL_SIZE, false), pal.text_body);
    let link_text = link_text(ins, c);
    p.text(pos2(link_readout.left(), link_readout.center().y), Align2::LEFT_CENTER, link_text, font(CAPTION_SIZE, false), pal.text_body);

    // ---- the header
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &header);

    // ---- circuit selector, two rows of six
    if let Some(i) = segmented_ex(ui, circuit_row, "circuit", pal, choices("circuit"), 2, Some(circuit as usize), SelectionStyle::Filled, false) {
        family::set(app, host, &mut st.fam, &FAMILY, "circuit", i as f32);
    }

    // ---- dials
    for (r, (pid, label, small)) in dials.into_iter().zip(defs) {
        dial_ui(app, ui, host, st, r, pid, label, small);
    }

    // ---- Link and Calibrate
    link_ui(app, ui, host, st, c, link_r, cal_r);

    // ---- the circuit's switch rows
    for (r, (pid, label)) in switch_rects.iter().zip(c.switches) {
        switch_row(app, ui, host, st, *r, label, pid);
    }

    // ---- calibration panel, floating over the switch rows
    if st.cal.active {
        let panel = host.rect.shrink2(vec2(PAD_X, PAD_Y));
        let top = panel.top().max(cal_r.bottom() + GAP);
        let panel = Rect::from_min_size(pos2(panel.left(), top), vec2(panel.width(), (panel.bottom() - top).clamp(72.0, 96.0)));
        link_cal::panel(app, ui, host, pal, &mut st.cal, &cal_spec(c), panel);
    }
}

/// `SwitchRow`: the name, the Segmented after it, the current option's description under it
/// (textDim, fitted to two lines).
fn switch_row(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut Editor, r: Rect, label: &str, pid: &str) {
    let pal = &PALETTE;
    let p = ui.painter().clone();
    let row = Rect::from_min_size(r.min, vec2(r.width(), ROW_H));
    p.text(pos2(row.left(), row.center().y), Align2::LEFT_CENTER, label, font(11.0, false), pal.text_body);
    let opts = choices(pid);
    let (sel, desc) = switch_state(host.ins, pid);
    let desc_r = Rect::from_min_max(pos2(r.left() + LABEL_W + LABEL_GAP, row.bottom() + 4.0), r.max);
    let mut size = 10.0;
    let mut galley = p.layout(desc.to_string(), font(size, false), pal.text_dim, desc_r.width().max(1.0));
    while galley.rows.len() > 2 && size > 7.5 {
        size -= 0.5;
        galley = p.layout(desc.to_string(), font(size, false), pal.text_dim, desc_r.width().max(1.0));
    }
    p.with_clip_rect(desc_r.intersect(p.clip_rect())).galley(desc_r.left_top(), galley, pal.text_dim);
    let seg = Rect::from_min_max(pos2(row.left() + LABEL_W + LABEL_GAP, row.top()), row.max);
    if let Some(n) = segmented_ex(ui, seg, ("row", pid), pal, opts, 1, Some(sel), SelectionStyle::Filled, false) {
        family::set(app, host, &mut st.fam, &FAMILY, pid, n as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_heights_follow_the_circuit() {
        assert_eq!(required_height(0), 306.0);
        assert_eq!(required_height(2), 306.0 + 2.0 * 52.0 + 6.0);
        assert_eq!(required_height(4), 306.0 + 4.0 * 52.0 + 3.0 * 6.0);
        // Every switch the table names has a description per option.
        for c in &cfg::CIRCUITS {
            for (pid, _) in c.switches {
                let d = descriptions::DESCRIPTIONS.iter().find(|(id, _)| id == pid).map(|(_, d)| d.len());
                assert_eq!(d, Some(choices(pid).len()), "{pid}");
            }
        }
    }
}

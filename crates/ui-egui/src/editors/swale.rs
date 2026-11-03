//! Swale's VST editor (`Swale/Source/PluginEditor.cpp`, `SwaleDisplay.h`, `SwaleConfig.h`),
//! translated. Swale is not one of the Engine family: it keeps its Amorph build's look (the
//! theme's cool blue-grey defaults, letter-spaced caps, outlined segments) with its amber accent,
//! the same in both DAW themes.
//!
//! Layout and paint follow `resized()` / `paint()`: the header strip (◀ name ▶ Undo A/B More,
//! Bypass at the right), the mode row, the screen ([`display`]), the stage row (Attack, Hold or
//! Hang, Release with their Sync pills; Sens or Rate; Bloom; Mix), the curve row, the More panel
//! and the tempo foot. The size is `max (640, requiredHeight())` when the editor opens; More gives
//! the window its row back rather than squeezing the screen (`setSize (h ± (kMoreH + kGap))`).
//!
//! The 30 Hz timer's readings come from the engine's readouts (`[2]` envelope, `[3]` detector,
//! `[4]` trigger count, `[5]` the tempo in force). Inventory: `docs/superpowers/editors/swale.md`.

mod display;
mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, LabelLayout, PillLook, PillStyle, SelectionStyle, pill_ex, pill_height, pill_width};
use crate::editor_kit::end_gesture;
use crate::editor_kit::family::{self, Family, FamilyState, arrow_pill, dial_attached, pill_param, preset_name, segmented_param};
use crate::editor_kit::look::{font, tracked_text, tracked_width};
use crate::editor_kit::palette::rgb;
use crate::editor_kit::presets;
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, param, readouts, set_params};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::swale::{SWALE_PARAMS, swale_config as sc, swale_dsp as sd};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

const K_ROW_LABEL: Color32 = rgb(0x5D6874);
const K_PANEL_FILL: Color32 = rgb(0x12161A);
const K_PANEL_LINE: Color32 = rgb(0x1F252B);
const K_ROOT_BACK: Color32 = rgb(0x101316);

const K_PAD_X: f32 = 17.0;
const K_PAD_Y: f32 = 11.0;
const K_GAP: f32 = 9.0;
const K_HEADER_H: f32 = 25.0;
const K_MODE_ROW_H: f32 = 28.0;
/// Knob, then value and name under it.
const K_STAGE_DIAL_H: f32 = 130.0;
const K_SYNC_H: f32 = 21.0;
const K_STAGE_H: f32 = K_STAGE_DIAL_H + 4.0 + K_SYNC_H;
const K_CURVE_ROW_H: f32 = 24.0;
const K_MORE_H: f32 = 90.0;
const K_FOOT_H: f32 = 40.0;
const K_SCREEN_MIN: f32 = 140.0;
const K_ROW_LABEL_W: f32 = 58.0;
const K_DEFAULT_WIDTH: f32 = 860.0;
const K_DEFAULT_HEIGHT: f32 = 640.0;

/// The theme's defaults (Swale's Amorph palette) with Swale's accent, its label grey and its
/// Bypass red.
pub const PALETTE: VstPalette =
    VstPalette { accent: argb(0xFFF2_B84B), text_label: argb(0xFF8A_96A3), bypass_red: argb(0xFFFF_6B5E), ..VstPalette::THEME_DEFAULTS };

/// `getPresetParams()`: every parameter but Input, Level, Bypass, Tempo Source and BPM.
pub const PRESET_PARAMS: [&str; 22] = [
    "attack",
    "hold",
    "release",
    "sens",
    "mix",
    "mode",
    "curve",
    "retrig",
    "detHp",
    "floorAmt",
    "bloom",
    "vca",
    "alternate",
    "atkSync",
    "atkNote",
    "holdSync",
    "holdNote",
    "relSync",
    "relNote",
    "rate",
    "peakBeat",
    "phrase",
];

/// The preset facts: `storageKey`, and what `getPresetParams` leaves out.
const FAM: Family = Family::new("Swale", &["trim", "level", "tempoSrc", "bpm"]);

/// The editor's own state.
#[derive(Clone, Default, Debug)]
pub(crate) struct Editor {
    fam: FamilyState,
    /// `showMore`.
    pub more: bool,
    /// The window's height once More has been toggled (`None`: `max (640, requiredHeight())`).
    height: Option<f32>,
    /// The trigger count last seen, and when the lamp goes out (`flash`: 90 ms).
    last_trig: Option<f32>,
    led_until: f64,
    scratch: Vec<Pos2>,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out. The
    /// slim layout's More section is Swale's own More (`showMore`).
    slim: bool,
    slim_h: f32,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

/// `requiredHeight()`: 479, or 578 with More.
pub fn required_height(more: bool) -> f32 {
    let h = K_PAD_Y * 2.0 + K_HEADER_H + K_GAP + K_MODE_ROW_H + K_GAP + K_SCREEN_MIN + K_GAP + K_STAGE_H + K_GAP + K_CURVE_ROW_H + K_GAP + K_FOOT_H;
    if more { h + K_MORE_H + K_GAP } else { h }
}

impl Editor {
    fn height(&self) -> f32 {
        self.height.unwrap_or_else(|| K_DEFAULT_HEIGHT.max(required_height(self.more)))
    }

    /// `moreButton.onClick`: the panel opens and the window grows by its row (the screen keeps its
    /// size), or closes and gives it back.
    pub fn toggle_more(&mut self) {
        let h = self.height();
        self.more = !self.more;
        let d = K_MORE_H + K_GAP;
        self.height = Some(required_height(self.more).max(if self.more { h + d } else { h - d }));
    }
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_swale";

    fn size(&self, _ins: &Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { vec2(K_DEFAULT_WIDTH, self.height()) }
    }

    /// `showMore = processor.isMorePanelOpen()`.
    fn opened(&mut self, ins: &Insert) {
        self.more = crate::editor_kit::ui_state(ins).get("more").and_then(serde_json::Value::as_bool).unwrap_or(false);
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        if self.slim {
            slim::editor_ui(self, app, ui, host);
        } else {
            editor_ui(self, app, ui, host);
        }
    }
}

fn idx(id: &str) -> usize {
    SWALE_PARAMS.iter().position(|p| p.id == id).unwrap_or(usize::MAX)
}

/// `formatValue (index, v)`.
fn value_text(id: &str, v: f32) -> String {
    sc::format_value(idx(id), v)
}

/// `getEffectiveBpm()`: the engine's (readout `[5]`) while it plays, else the knob on Manual, else
/// the session's.
fn effective_bpm(host: &Host, ro: Option<[f32; 8]>) -> f32 {
    if let Some(b) = ro.map(|r| r[5]).filter(|b| b.is_finite() && *b >= 20.0) {
        return b;
    }
    let knob = sd::knob_to_bpm(param(host, "bpm"));
    if param(host, "tempoSrc") > 0.5 {
        return knob;
    }
    let t = host.session.tempo.tempo_at_tick(0) as f32;
    if t.is_finite() && t >= 20.0 { t } else { knob }
}

/// `resized()`'s header areas (they need the font to measure the pills).
struct Header {
    title: Rect,
    bypass: Rect,
    prev: Rect,
    name: Rect,
    next: Rect,
    undo: Rect,
    ab: Rect,
    /// The More pill (the slim layout has its More row instead).
    more: Option<Rect>,
}

/// `resized()`'s areas below the header.
struct Body {
    mode: Rect,
    screen: Rect,
    /// Attack, Hold, Release slots.
    stages: [Rect; 3],
    /// Sens / Rate, Bloom, Mix.
    dials: [Rect; 3],
    curve_row: Rect,
    more_panel: Option<Rect>,
    foot: Rect,
}

fn header_layout(ui: &Ui, r: Rect) -> Header {
    let b = r.shrink2(vec2(K_PAD_X, K_PAD_Y));
    let pill_h = pill_height(PillStyle::Small);
    // title left, preset strip centred, Bypass right
    let mut row = Rect::from_min_size(b.min, vec2(b.width(), K_HEADER_H));
    let right = Rect::from_min_max(pos2(row.right() - 70.0, row.top()), row.max);
    row.max.x -= 70.0;
    let bypass = Rect::from_center_size(right.center(), vec2(66.0, pill_h));
    let tw = 90f32.max((tracked_width(ui.painter(), "SWALE", 17.0, 0.22, true) + 10.0).floor());
    let title = Rect::from_min_size(row.min, vec2(tw, row.height()));
    row.min.x += tw;
    let undo_w = pill_width(ui, "Undo", PillStyle::Small).floor();
    let ab_w = 30.0;
    let more_w = pill_width(ui, "More", PillStyle::Small).floor();
    let arrow_w = 26.0;
    let name_w = (row.width() - (arrow_w * 2.0 + undo_w + ab_w + more_w + 36.0)).clamp(120.0, 260.0);
    let strip_w = arrow_w * 2.0 + name_w + undo_w + ab_w + more_w + 30.0;
    let strip = Rect::from_center_size(row.center(), vec2(strip_w.min(row.width()), pill_h));
    let mut x = strip.left();
    let mut next_r = |w: f32| {
        let t = Rect::from_min_size(pos2(x, strip.top()), vec2(w, pill_h));
        x += w + 6.0;
        t
    };
    let prev = next_r(arrow_w);
    let name = next_r(name_w);
    let next = next_r(arrow_w);
    let undo = next_r(undo_w);
    let ab = next_r(ab_w);
    let more = next_r(more_w);
    Header { title, bypass, prev, name, next, undo, ab, more: Some(more) }
}

fn body_layout(r: Rect, more: bool) -> Body {
    let mut b = r.shrink2(vec2(K_PAD_X, K_PAD_Y));
    let mut take = |h: f32| {
        let t = Rect::from_min_size(b.min, vec2(b.width(), h));
        b.min.y += h;
        t
    };
    take(K_HEADER_H);
    take(K_GAP);
    let mode = take(K_MODE_ROW_H);
    take(K_GAP);
    // Everything below the screen is fixed, so the screen takes whatever is left.
    let mut below = K_STAGE_H + K_GAP + K_CURVE_ROW_H + K_GAP + K_FOOT_H + K_GAP;
    if more {
        below += K_MORE_H + K_GAP;
    }
    let left = r.bottom() - K_PAD_Y - (r.top() + K_PAD_Y + K_HEADER_H + K_GAP + K_MODE_ROW_H + K_GAP);
    let screen = take(K_SCREEN_MIN.max(left - below));
    take(K_GAP);
    // The stages row: weights 1.28 for the stages, 1 for the dials, 8 px between.
    let srow = take(K_STAGE_H);
    let usable = srow.width() - 5.0 * 8.0;
    let width_for = |w: f32| (usable * w / (1.28 * 3.0 + 3.0)).round();
    let mut sx = srow.left();
    let mut slot = |w: f32, h: f32| {
        let t = Rect::from_min_size(pos2(sx, srow.top()), vec2(w, h));
        sx += w + 8.0;
        t
    };
    let stages = [slot(width_for(1.28), K_STAGE_H), slot(width_for(1.28), K_STAGE_H), slot(width_for(1.28), K_STAGE_H)];
    let dials = [slot(width_for(1.0), K_STAGE_DIAL_H), slot(width_for(1.0), K_STAGE_DIAL_H), slot(width_for(1.0), K_STAGE_DIAL_H)];
    take(K_GAP);
    let curve_row = take(K_CURVE_ROW_H);
    take(K_GAP);
    let more_panel = more.then(|| {
        let m = take(K_MORE_H);
        take(K_GAP);
        m
    });
    let foot = take(K_FOOT_H);
    Body { mode, screen, stages, dials, curve_row, more_panel, foot }
}

/// A Swale pill: `Pill::Small`, letter-spaced caps (the theme's default `setTracked (true)`).
fn tracked_look(active: Option<Color32>) -> PillLook {
    PillLook { active_colour: active, tracked: true, dimmed: false }
}

/// `moreButton.onClick`: [`Editor::toggle_more`], and `processor.setMorePanelOpen (showMore)`.
fn toggle_more(st: &mut Editor, app: &mut SoundApp, host: &Host) {
    st.toggle_more();
    crate::editor_kit::set_ui_state(app, host, &[("more", serde_json::json!(st.more))]);
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host) {
    let pal = &PALETTE;
    let r = host.rect;
    let p = ui.painter().clone();
    p.rect_filled(r, CornerRadius::ZERO, K_ROOT_BACK);
    if let Some((_, _, tab)) = app.ui.drawer_tab.take_if(|(t, m, _)| *t == host.track && *m == host.ins.id) {
        match tab.as_str() {
            "Presets" => presets::open(app, host, &mut st.fam.presets),
            "More" => toggle_more(st, app, host),
            _ => {}
        }
    }
    if family::browser(app, ui, host, pal, &mut st.fam, &FAM, "") {
        ui.ctx().request_repaint();
        return;
    }

    let hd = header_layout(ui, r);
    let l = body_layout(r, st.more);
    let ro = readouts(host);
    let bpm = effective_bpm(host, ro);
    let manual = param(host, "tempoSrc") > 0.5;
    let a = sc::what_applies(
        param(host, "mode").round() as i32,
        param(host, "atkSync") > 0.5,
        param(host, "holdSync") > 0.5,
        param(host, "relSync") > 0.5,
        manual,
        true,
    );

    // ---- paint(): the title, row labels, the curve's line, the More panel, the tempo readout.
    tracked_text(&p, hd.title, Align2::LEFT_CENTER, "Swale", 17.0, 0.22, true, true, pal.accent);
    tracked_text(
        &p,
        Rect::from_min_size(l.curve_row.min, vec2(K_ROW_LABEL_W, l.curve_row.height())),
        Align2::LEFT_CENTER,
        "Curve",
        9.0,
        0.18,
        true,
        false,
        K_ROW_LABEL,
    );
    tracked_text(
        &p,
        Rect::from_min_size(l.foot.min, vec2(K_ROW_LABEL_W, l.foot.height())),
        Align2::LEFT_CENTER,
        "Tempo",
        9.0,
        0.18,
        true,
        false,
        K_ROW_LABEL,
    );

    // ---- header
    header_controls(st, app, ui, host, &hd);

    // ---- mode, and the screen it describes
    segmented_param(app, ui, host, &mut st.fam, &FAM, l.mode, pal, "mode", Some(&sc::MODES), 1, SelectionStyle::Outlined, true);
    let live = ro.map(|r| crate::module_graphs::LiveValues { readouts: r, ..Default::default() });
    let hist = crate::swale_ui::history(ui.ctx(), crate::module_graphs::module_key(host.track.0, host.ins.id.0), live.as_ref());
    let now = ui.input(|i| i.time);
    let settings = screen_settings(st, host, ro, bpm, now);
    display::paint(&p, l.screen, pal, &settings, &hist, now < st.led_until, &mut st.scratch);

    // ---- the stages
    let dial = |app: &mut SoundApp, ui: &mut Ui, fam: &mut FamilyState, rect: Rect, id: &str, label: &str, o: DialOpts| {
        dial_ctl(app, ui, host, fam, rect, id, label, o);
    };
    let stage_rows = [
        ("attack", "atkNote", "atkSync", a.attack_note, "Attack"),
        ("hold", "holdNote", "holdSync", a.hold_note, a.hold_label),
        ("release", "relNote", "relSync", a.release_note, "Release"),
    ];
    for (slot, (time_id, note_id, sync_id, synced, label)) in l.stages.iter().zip(stage_rows) {
        let dial_r = Rect::from_min_size(slot.min, vec2(slot.width(), K_STAGE_DIAL_H));
        dial(app, ui, &mut st.fam, dial_r, if synced { note_id } else { time_id }, label, DialOpts::default());
        let sync_r = Rect::from_center_size(pos2(slot.center().x, dial_r.bottom() + 4.0 + K_SYNC_H * 0.5), vec2(slot.width().min(58.0), K_SYNC_H));
        pill_param(app, ui, host, &mut st.fam, &FAM, sync_r, pal, sync_id, "Sync", tracked_look(None));
    }
    if a.sens {
        dial(app, ui, &mut st.fam, l.dials[0], "sens", "Sens", DialOpts::default());
    }
    if a.rate {
        dial(app, ui, &mut st.fam, l.dials[0], "rate", "Rate", DialOpts::default());
    }
    dial(app, ui, &mut st.fam, l.dials[1], "bloom", "Bloom", DialOpts::default());
    dial(app, ui, &mut st.fam, l.dials[2], "mix", "Mix", DialOpts::default());

    // ---- the curve row: the four curves, Peak on Beat (Rhythmic), Per Note / Phrase (Pick), the line
    let pill_h = pill_height(PillStyle::Small);
    let cr = l.curve_row;
    let mut x = cr.left() + K_ROW_LABEL_W + 8.0;
    segmented_param(
        app,
        ui,
        host,
        &mut st.fam,
        &FAM,
        Rect::from_min_size(pos2(x, cr.top()), vec2(196.0, cr.height())),
        pal,
        "curve",
        None,
        1,
        SelectionStyle::Outlined,
        true,
    );
    x += 196.0 + 10.0;
    if a.peak_beat {
        let pr = Rect::from_min_size(pos2(x, cr.center().y - pill_h * 0.5), vec2(98.0, pill_h));
        pill_param(app, ui, host, &mut st.fam, &FAM, pr, pal, "peakBeat", "Peak on Beat", tracked_look(None));
        x += 98.0 + 10.0;
    }
    if a.phrase {
        segmented_param(
            app,
            ui,
            host,
            &mut st.fam,
            &FAM,
            Rect::from_min_size(pos2(x, cr.top()), vec2(150.0, cr.height())),
            pal,
            "phrase",
            None,
            1,
            SelectionStyle::Outlined,
            true,
        );
        x += 150.0 + 10.0;
    }
    let blurb_r = Rect::from_min_max(pos2(x, cr.top()), cr.max);
    let g = p.layout(sc::curve_blurb(param(host, "curve").round() as i32).to_string(), font(10.0, false), pal.text_dim, blurb_r.width().max(1.0));
    p.with_clip_rect(blurb_r).galley(pos2(blurb_r.left(), blurb_r.center().y - g.size().y * 0.5), g, pal.text_dim);

    // ---- the More panel
    if let Some(mp) = l.more_panel {
        p.rect_filled(mp, CornerRadius::same(4), K_PANEL_FILL);
        p.rect_stroke(mp.shrink(0.5), CornerRadius::same(4), Stroke::new(1.0, K_PANEL_LINE), StrokeKind::Middle);
        let inner = mp.shrink2(vec2(10.0, 6.0));
        let mut ix = inner.left();
        let mut col = |w: f32, gap: f32| {
            let t = Rect::from_min_size(pos2(ix, inner.top()), vec2(w, inner.height()));
            ix += w + gap;
            t
        };
        let dim = a.dim_detector;
        let more_dials = [
            (col(66.0, 0.0), "retrig", "Retrig", dim),
            (col(66.0, 14.0), "detHp", "Low Cut", dim),
            (col(66.0, 0.0), "floorAmt", "Floor", false),
            (col(66.0, 14.0), "alternate", "Alternate", false),
            (col(66.0, 0.0), "trim", "Input", false),
            (col(66.0, 16.0), "level", "Level", false),
        ];
        for (rect, id, label, locked) in more_dials {
            dial(app, ui, &mut st.fam, rect, id, label, DialOpts { locked, ..DialOpts::default() });
        }
        let vca_col = Rect::from_min_max(pos2(ix, inner.top()), inner.max);
        tracked_text(
            &p,
            Rect::from_min_size(pos2(vca_col.left(), mp.top() + 6.0), vec2(vca_col.width(), 14.0)),
            Align2::LEFT_CENTER,
            "VCA",
            9.0,
            0.18,
            true,
            false,
            K_ROW_LABEL,
        );
        let seg = Rect::from_min_size(pos2(vca_col.left(), vca_col.top() + 14.0), vec2(vca_col.width().min(180.0), 24.0));
        segmented_param(app, ui, host, &mut st.fam, &FAM, seg, pal, "vca", None, 1, SelectionStyle::Outlined, true);
        let vb = Rect::from_min_max(pos2(vca_col.left(), seg.bottom() + 4.0), vca_col.max);
        let g = p.layout(sc::vca_blurb(param(host, "vca").round() as i32).to_string(), font(10.0, false), pal.text_dim, vb.width().max(1.0));
        p.with_clip_rect(vb).galley(vb.min, g, pal.text_dim);
    }

    // ---- foot: tempo source, the BPM knob when it applies, the readout, the standing advice
    let f = l.foot;
    let mut fx = f.left() + K_ROW_LABEL_W + 8.0;
    let ts = Rect::from_center_size(pos2(fx + 64.0, f.center().y), vec2(128.0, 26.0));
    segmented_param(app, ui, host, &mut st.fam, &FAM, ts, pal, "tempoSrc", None, 1, SelectionStyle::Outlined, true);
    fx += 128.0 + 10.0;
    if a.bpm_knob {
        let br = Rect::from_min_size(pos2(fx, f.top()), vec2(108.0, f.height()));
        dial(app, ui, &mut st.fam, br, "bpm", "BPM", DialOpts { layout: LabelLayout::Right, ..DialOpts::default() });
        fx += 108.0 + 10.0;
    }
    let rest = Rect::from_min_max(pos2(fx, f.top()), f.max);
    let about_w = 230f32.min(rest.width() / 2.0);
    let about = Rect::from_min_max(pos2(rest.right() - about_w, rest.top()), rest.max);
    tracked_text(&p, about, Align2::RIGHT_CENTER, "Put me before delay and reverb", 10.0, 0.12, true, false, pal.text_faint);
    let readout = Rect::from_center_size(pos2(rest.center().x, rest.center().y), vec2(rest.width(), 14.0));
    let runs = tempo_runs(host, bpm);
    let mut rx = readout.left();
    for (text, c) in runs {
        let w = tracked_width(&p, &text, 10.0, 0.12, true);
        tracked_text(
            &p,
            Rect::from_min_size(pos2(rx, readout.top()), vec2(w, readout.height())),
            Align2::LEFT_CENTER,
            &text,
            10.0,
            0.12,
            true,
            false,
            c,
        );
        rx += w;
    }

    // The timer: the screen, the lamp and the tempo readout (30 Hz).
    if ro.is_some() || now < st.led_until || st.fam.presets.message.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// The header's gestures: the arrows step the presets, the name opens the browser, Undo, A/B,
/// More (where shown) and Bypass.
fn header_controls(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, hd: &Header) {
    let pal = &PALETTE;
    let mut out = presets::BrowserOut::default();
    if arrow_pill(ui, hd.prev, "prev", pal, true).clicked() {
        out = presets::step(app, host, &mut st.fam.presets, -1);
    }
    if arrow_pill(ui, hd.next, "next", pal, false).clicked() {
        out = presets::step(app, host, &mut st.fam.presets, 1);
    }
    st.fam.presets.not_in_presets = FAM.not_in_presets;
    let dirty = presets::is_dirty(app, host, &mut st.fam.presets);
    let shown = presets::shown_name(host);
    if preset_name(ui, hd.name, pal, shown, dirty).clicked() {
        presets::open(app, host, &mut st.fam.presets);
    }
    let undo_look = PillLook { dimmed: !st.fam.can_undo(), ..tracked_look(None) };
    if pill_ex(ui, hd.undo, "undo", pal, "Undo", false, PillStyle::Small, undo_look).clicked() {
        family::do_undo(app, host, &mut st.fam);
    }
    let on_b = st.fam.on_b();
    if pill_ex(ui, hd.ab, "ab", pal, if on_b { "B" } else { "A" }, on_b, PillStyle::Small, tracked_look(None)).clicked() {
        family::toggle_ab(app, host, &mut st.fam, &FAM);
    }
    if let Some(more_r) = hd.more
        && pill_ex(ui, more_r, "more", pal, "More", st.more, PillStyle::Small, tracked_look(None)).clicked()
    {
        toggle_more(st, app, host);
    }
    let bypassed = param(host, "bypass") > 0.5;
    if pill_ex(ui, hd.bypass, "bypass", pal, "Bypass", bypassed, PillStyle::Small, tracked_look(Some(pal.bypass_red))).clicked() {
        family::push_undo(&mut st.fam, host, &FAM);
        set_params(app, host, &[("bypass", if bypassed { 0.0 } else { 1.0 })]);
    }
    if out.recalled.is_some() {
        end_gesture(app);
    }
}

/// The swell lamp (`flash`: 90 ms after the trigger count moves) and the screen's settings.
fn screen_settings(st: &mut Editor, host: &Host, ro: Option<[f32; 8]>, bpm: f32, now: f64) -> display::Settings {
    if let Some(t) = ro.map(|r| r[4]).filter(|t| t.is_finite()) {
        if st.last_trig.is_some_and(|l| l != t) {
            st.led_until = now + 0.09;
        }
        st.last_trig = Some(t);
    }
    display::Settings {
        mode: param(host, "mode").round() as i32,
        curve: param(host, "curve").round() as i32,
        peak_beat: param(host, "peakBeat") > 0.5,
        phrase: param(host, "phrase") > 0.5,
        bypassed: param(host, "bypass") > 0.5,
        slow_gear: param(host, "vca") > 0.5,
        sens: param(host, "sens"),
        retrig: param(host, "retrig"),
        floor_amt: param(host, "floorAmt"),
        times: crate::swale_ui::stage_times(host.ins, bpm),
    }
}

/// A dial bound to `id` with Swale's readout and letter-spaced name.
#[allow(clippy::too_many_arguments)]
fn dial_ctl(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, rect: Rect, id: &str, label: &str, o: DialOpts) {
    let text = value_text(id, param(host, id));
    let o = DialOpts { label, value_text: &text, tracked: true, ..o };
    dial_attached(app, ui, host, fam, &FAM, rect, &PALETTE, id, &o);
}

/// The tempo readout's runs: "120 BPM" in the accent, then "  ·  manual" (or "host").
fn tempo_runs(host: &Host, bpm: f32) -> [(String, Color32); 2] {
    let pal = &PALETTE;
    let knob = format!("{}", sd::knob_to_bpm(param(host, "bpm")).round_ties_even() as i64);
    let dot = "  \u{b7}  ";
    if param(host, "tempoSrc") > 0.5 {
        [(format!("{knob} BPM"), pal.accent), (format!("{dot}manual"), pal.text_dim)]
    } else {
        [(format!("{bpm:.1} BPM"), pal.accent), (format!("{dot}host"), pal.text_dim)]
    }
}

/// A knob's centre or a row's centre in an editor at `frame`, for tests and scripts (`which`:
/// "attack", "hold", "release", "sens", "bloom", "mix", "mode", "screen").
pub fn spot(frame: Rect, more: bool, which: &str) -> Option<Pos2> {
    let l = body_layout(frame, more);
    let knob = |r: Rect| {
        crate::editor_kit::controls::dial_knob_bounds(Rect::from_min_size(r.min, vec2(r.width(), K_STAGE_DIAL_H)), LabelLayout::Below).center()
    };
    match which {
        "attack" => Some(knob(l.stages[0])),
        "hold" => Some(knob(l.stages[1])),
        "release" => Some(knob(l.stages[2])),
        "sens" => Some(knob(l.dials[0])),
        "bloom" => Some(knob(l.dials[1])),
        "mix" => Some(knob(l.dials[2])),
        "mode" => Some(l.mode.center()),
        "screen" => Some(l.screen.center()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_follows_more_as_the_vst() {
        assert_eq!(required_height(false), 479.0);
        assert_eq!(required_height(true), 578.0);
        let mut e = Editor::default();
        assert_eq!(e.height(), 640.0, "max (640, requiredHeight())");
        e.toggle_more();
        assert_eq!(e.height(), 739.0, "More gives the window its row back");
        e.toggle_more();
        assert_eq!(e.height(), 640.0);
    }
}

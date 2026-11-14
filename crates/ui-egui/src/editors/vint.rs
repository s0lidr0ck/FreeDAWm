//! Vint's VST editor (`Engine 11 - Vint/Source/PluginEditor.cpp`), translated: the family header,
//! the circuit row, the circuit's panel — the five EQs on their hardware faces (`VintConfig.h`:
//! every control where it sits on the panel, the knobs drawn as the box's, `VintKnob.h`), the
//! three flavour circuits in columns — the curve ([`scope`], `VintScope.h`) and the output
//! section. Palette: `look::apply (kAccent)` (Brass), the same in both DAW themes. Vint keeps its
//! own model-specific knob drawings (the owner's ruling). Its interaction inventory is
//! `docs/superpowers/editors/vint.md`.

mod config;
mod draw;
mod knob;
mod maximizer;
mod scope;
mod slim;
mod vitalizer;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, SelectionStyle, dial, pill_ex, pill_height, segmented};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, set_params_merged};
use crate::theme::{bold, regular};
use config::{Face, FaceStyle, Glyph, Kind, Placed, ToggleStyle, circuit, circuits, columns_for, format_knob, format_knob_for, step_text};
use draw::{Fill, Jc, fill_ellipse, fill_round_rect, line, quad, round_rect, stroke_closed, text_up};
use egui::{Align2, Id, Pos2, Rect, Sense, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::vint::vint_engine_dsp::{self as d, NUM_PARAMS};
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
const K_KNOB: f32 = 62.0;
const K_CELL_H: f32 = K_KNOB + 28.0;
const K_TITLE_H: f32 = 18.0;
const K_TOGGLE_H: f32 = 24.0;
const K_PANEL_H: f32 = 10.0 + K_TITLE_H + 3.0 * K_CELL_H + 10.0;
const K_SCOPE_H: f32 = 190.0;
const K_FOOT_KNOB: f32 = 56.0;
const K_DEFAULT_H: f32 = K_PAD_Y * 2.0 + K_HEADER_H + K_GAP + K_ROW_H + 3.0 + K_BLURB_H + K_GAP + K_PANEL_H + K_GAP + K_SCOPE_H;
const K_DEFAULT_W: f32 = 1000.0;

/// `setSize (kDefaultW, kDefaultH)`.
pub const SIZE: Vec2 = vec2(K_DEFAULT_W, K_DEFAULT_H);

/// `kAccent`: Brass.
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFFD9B26A));

/// The parameter ids by index (`Parameters.h`).
/// The hardware credit line of the circuit named `name` (`crate::hardware_credit`).
pub(crate) fn circuit_credit(name: &str) -> &'static str {
    circuits().iter().find(|c| c.name == name).map_or("", |c| c.credit)
}

fn id_of(i: usize) -> &'static str {
    soundcraft_dsp::plugins::fs::vint::VINT_PARAMS.get(i).map_or("", |p| p.id)
}

/// `PresetCatalogue`: the storage key, and what `getPresetParams` leaves out besides `bypass`.
const FAMILY: Family = Family::new("VintEngine", &["output", "quality"]);

// ============================================================================ the editor

#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    scope: scope::View,
    /// The VU's needle (VU), and when it last moved.
    needle: (f32, f64),
    /// The Maximizer's level lamps: the level they show, in dBu.
    lamps: f32,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// Scripts: open the slim layout's circuit menu on the next frame.
    circuits_open: bool,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_vint";

    fn size(&self, _ins: &Insert) -> Vec2 {
        if self.slim { slim::size(self.slim_h) } else { SIZE }
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

fn text_left(p: &egui::Painter, r: Rect, text: &str, font: egui::FontId, c: egui::Color32) {
    p.with_clip_rect(r.intersect(p.clip_rect())).text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, text, font, c);
}

/// A parameter by index, as the plugin clamps it.
fn pv(host: &Host, i: usize) -> f32 {
    param(host, id_of(i))
}

/// A choice parameter's labels (`choicesFor`).
fn choices(host: &Host, i: usize) -> &'static [&'static str] {
    host.info.param(id_of(i)).map_or(&[], |p| p.choices)
}

/// The grain tile (`plateGrain`): fine speckle and faint brushing, seeded so it never changes.
fn grain_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = Id::new("vint-plate-grain");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let img = egui::ColorImage::from_rgba_unmultiplied([256, 256], &plate_grain());
    let t = ctx.load_texture("vint-plate-grain", img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// `juce::Random`.
struct JuceRandom(i64);
impl JuceRandom {
    fn next_int(&mut self) -> i32 {
        self.0 = ((self.0 as u64).wrapping_mul(0x5_DEEC_E66D).wrapping_add(11) & 0xFFFF_FFFF_FFFF) as i64;
        (self.0 >> 16) as i32
    }
    fn next_int_max(&mut self, max: i32) -> i32 {
        ((u64::from(self.next_int() as u32) * max as u64) >> 32) as i32
    }
    fn next_float(&mut self) -> f32 {
        ((self.next_int() as u32) as f32 / (u32::MAX as f32 + 1.0)).min(1.0 - f32::EPSILON)
    }
}

/// The 256 x 256 grain tile's straight RGBA bytes.
fn plate_grain() -> Vec<u8> {
    let mut rng = JuceRandom(0x5EED);
    let mut px = vec![0u8; 256 * 256 * 4];
    for y in 0..256 {
        for x in 0..256 {
            let n = rng.next_float();
            let dark = n < 0.5;
            let a = if dark { 10 + rng.next_int_max(16) } else { 4 + rng.next_int_max(10) } as u8;
            let v = if dark { 0 } else { 255 };
            let i = (y * 256 + x) * 4;
            px[i..i + 4].copy_from_slice(&[v, v, v, a]);
        }
    }
    // the brush marks: white lines, 0.8 px, 3..6 % (blended over the speckle)
    for _ in 0..40 {
        let y = rng.next_float() * 256.0;
        let x = rng.next_float() * 256.0;
        let len = 30.0 + rng.next_float() * 120.0;
        let alpha = 0.03 + 0.03 * rng.next_float();
        let dy = (rng.next_float() - 0.5) * 2.0;
        let steps = len.ceil() as i32;
        for s in 0..=steps {
            let t = s as f32 / steps.max(1) as f32;
            let (px_x, px_y) = ((x + len * t).floor() as i32, (y + dy * t).floor() as i32);
            if !(0..256).contains(&px_x) || !(0..256).contains(&px_y) {
                continue;
            }
            let i = ((px_y * 256 + px_x) * 4) as usize;
            let a0 = f32::from(px[i + 3]) / 255.0;
            let src = alpha * 0.8;
            let out_a = src + a0 * (1.0 - src);
            let c0 = f32::from(px[i]) / 255.0;
            let c = if out_a > 0.0 { (src + c0 * a0 * (1.0 - src)) / out_a } else { 0.0 };
            let cb = (c * 255.0).round() as u8;
            px[i..i + 4].copy_from_slice(&[cb, cb, cb, (out_a * 255.0).round() as u8]);
        }
    }
    px
}

/// A face control's bounds (`resized`).
fn face_bounds(pc: &Placed, scale: f32, origin: Pos2) -> Rect {
    let w = (pc.w * scale).round();
    let c = pos2(origin.x + pc.x * scale, origin.y + pc.y * scale);
    let centred = |h: f32| Rect::from_center_size(pos2(c.x.round(), c.y.round()), vec2(w, h));
    match (pc.kind, pc.toggle) {
        (Kind::Knob | Kind::Step, _) => {
            let top = (c.y - knob::centre_offset(w, pc.style, pc.dots, pc.name_below)).round();
            Rect::from_min_size(pos2((c.x - w * 0.5).round(), top), vec2(w, knob::height_for(w, pc.style, pc.dots, pc.name_below)))
        }
        (_, ToggleStyle::Bat) => centred((w * 1.35).round()),
        (_, ToggleStyle::Plate) => centred((w * 0.36).round()),
        (_, ToggleStyle::SslTall | ToggleStyle::SslTallRed) => centred((w * 2.6).round()),
        (_, ToggleStyle::Ssl | ToggleStyle::SslRed) => centred((w * 0.42).round()),
        (_, ToggleStyle::Dome) => centred((w * 0.86).round() + 22.0),
        _ => centred(K_TOGGLE_H),
    }
}

/// A knob / switch change through the commands: a drag one undo step, a click or wheel step one.
fn apply_knob(app: &mut SoundApp, host: &Host, st: &mut FamilyState, param_i: usize, n: usize, out: &knob::KnobOut) {
    let id = id_of(param_i);
    if out.started {
        // `onDragStart`: `pushUndo`, then the gesture.
        end_gesture(app);
        family::push_undo(st, host, &FAMILY);
    }
    if let Some(v) = out.value {
        // `attachStep`: the knob moves 0..1, the parameter is the position it snaps to.
        let real = if n > 0 { (v.clamp(0.0, 1.0) * (n - 1) as f32).round() } else { v };
        if (real - pv(host, param_i)).abs() > 1e-7 {
            set_params_merged(app, host, &[(id, real)], &format!("vint-{id}"));
        }
    }
    if out.ended {
        end_gesture(app);
    }
}

/// The Maximizer's level lamps on the 30 Hz timer, as the VU: the loudest block's output RMS
/// since the last tick. Returns the level they show, in dBu.
fn maximizer_level(st: &mut Editor, ui: &Ui, host: &Host) -> f32 {
    let now = ui.input(|i| i.time);
    let dt = now - st.needle.1;
    if !(0.0..1.0 / 30.0).contains(&dt) {
        let rms = crate::editor_kit::live(host).map_or(0.0, |l| l.take(soundcraft_dsp::plugins::fs::vint::LIVE_OUT_LEVEL));
        maximizer::level_step(&mut st.lamps, rms, dt.clamp(0.0, 1.0) as f32);
        st.needle.1 = now;
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    st.lamps
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if !st.init {
        st.init = true;
        st.fam.presets.category = "All".into();
        st.needle = (-22.0, ui.input(|i| i.time));
        st.lamps = -90.0;
    }
    let _ = family::take_drawer_tab(app, host, &mut st.fam);
    let pal = &PALETTE;
    let full = host.rect;
    if family::browser(app, ui, host, pal, &mut st.fam, &FAMILY, circuit((pv(host, d::P_CIRCUIT).round() as i32).clamp(0, 7)).name) {
        return;
    }
    let p = ui.painter().clone();
    paint_background(&p, full);
    let cached_circuit = (pv(host, d::P_CIRCUIT).round() as i32).clamp(0, d::NUM_CIRCUITS - 1);
    let def = circuit(cached_circuit);

    // ---- resized()
    let mut b = full.shrink2(vec2(K_PAD_X, K_PAD_Y));
    let header = Rect::from_min_size(b.min, vec2(b.width(), K_HEADER_H));
    b.min.y += K_HEADER_H + K_GAP;
    let row = Rect::from_min_size(b.min, vec2(b.width(), K_ROW_H));
    let circuit_label = Rect::from_min_size(row.min, vec2(K_LABEL_W, K_ROW_H));
    let circuit_seg = Rect::from_min_max(pos2(row.left() + K_LABEL_W + K_LABEL_GAP, row.top()), row.max);
    b.min.y += K_ROW_H + 3.0;
    let circuit_blurb = Rect::from_min_max(pos2(b.left() + K_LABEL_W + K_LABEL_GAP, b.top()), pos2(b.right(), b.top() + K_BLURB_H));
    b.min.y += K_BLURB_H + K_GAP;
    let panel = Rect::from_min_size(b.min, vec2(b.width(), K_PANEL_H));
    b.min.y += K_PANEL_H + K_GAP;
    let bottom = Rect::from_min_size(b.min, vec2(b.width(), K_SCOPE_H.max(b.height())));
    let foot = Rect::from_min_max(pos2(bottom.right() - 330.0, bottom.top()), bottom.max);
    let scope_r = Rect::from_min_max(bottom.min, pos2(foot.left() - K_GAP, bottom.bottom()));
    let f = foot.shrink2(vec2(12.0, 10.0));
    let ftop = Rect::from_min_size(f.min, vec2(f.width(), 26.0));
    let stage_r = keep_centre(Rect::from_min_size(ftop.min, vec2(120.0, 26.0)), 120.0, pill_height(PillStyle::Small));
    let stage_note = Rect::from_min_max(pos2(ftop.left() + 130.0, ftop.top()), ftop.max);
    let knobs = Rect::from_min_size(pos2(f.left(), ftop.bottom() + 8.0), vec2(f.width(), K_FOOT_KNOB + 28.0));
    let kw = (knobs.width() / 3.0).trunc();
    let foot_dial = |i: usize| {
        let cell = if i < 2 {
            Rect::from_min_size(pos2(knobs.left() + kw * i as f32, knobs.top()), vec2(kw, knobs.height()))
        } else {
            Rect::from_min_max(pos2(knobs.left() + 2.0 * kw, knobs.top()), knobs.max)
        };
        keep_centre(cell, kw.min(96.0), K_FOOT_KNOB + 28.0)
    };
    let q = Rect::from_min_size(pos2(f.left(), knobs.bottom() + 10.0), vec2(f.width(), 24.0));
    let quality_label = Rect::from_min_size(q.min, vec2(56.0, 24.0));
    let quality_seg = Rect::from_min_size(pos2(q.left() + 56.0, q.top()), vec2(200.0, 24.0));

    // ---- paint()
    text_left(&p, circuit_label, "Circuit", regular(LABEL_SIZE), pal.text_body);
    text_left(&p, quality_label, "Quality", regular(LABEL_SIZE), pal.text_body);
    // The Enhancer's Maximizer has a unit of its own (the 882's front), not the columns.
    let maxi = maximizer::shown(host);
    let enh_type = (pv(host, d::P_ENH_TYPE).round() as i32).clamp(0, 2);
    let (blurb, credit) = if maxi {
        (maximizer::BLURB, maximizer::CREDIT)
    } else if cached_circuit == d::C_ENHANCER && enh_type == 2 {
        (config::VITALIZER_BLURB, config::VITALIZER_CREDIT)
    } else {
        (def.blurb, def.credit)
    };
    text_left(&p, circuit_blurb, blurb, regular(CAPTION_SIZE), pal.text_faint);
    crate::hardware_credit::hover(ui, circuit_blurb, "vint_circuit", blurb, credit);
    paint_panel(&p, panel, 10.0);
    paint_panel(&p, foot, 10.0);
    let scale = (panel.width() / 1000.0).min(panel.height() / 300.0);
    let origin = pos2(panel.left() + (panel.width() - 1000.0 * scale) * 0.5, panel.top() + (panel.height() - 300.0 * scale) * 0.5);
    if !def.face.empty() {
        paint_face(ui, &p, host, panel, &def.face, scale, origin);
    }
    if maxi {
        let level = maximizer_level(st, ui, host);
        maximizer::paint(&p, host, &maximizer::Lay::wide(panel, scale, origin), level);
    }
    let vita = vitalizer::shown(host);
    if vita {
        vitalizer::paint(&p, host, &vitalizer::Lay::wide(panel, scale, origin), vitalizer::lamps(crate::editor_kit::readouts(host)));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    text_left(&p, stage_note, def.stage, regular(CAPTION_SIZE), pal.text_faint);

    // ---- the header
    paint_title(&p, header, "Vint", "Engine", pal);
    let rects = family::right_aligned_header(ui, header);
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &rects);

    // ---- the circuit (in `circuitOrder`)
    let names: Vec<&str> = config::CIRCUIT_ORDER.iter().map(|c| circuits()[*c as usize].name).collect();
    let slot = config::CIRCUIT_ORDER.iter().position(|c| *c == cached_circuit).unwrap_or(0);
    if let Some(k) = segmented(ui, circuit_seg, "vint-circuit", pal, &names, 1, Some(slot), SelectionStyle::Filled)
        && k != slot
    {
        family::set(app, host, &mut st.fam, &FAMILY, "circuit", config::CIRCUIT_ORDER[k] as f32);
    }

    // ---- the panel's controls
    if maxi {
        let (label, seg) = maximizer::wide_type_rects(panel);
        maximizer::type_selector(app, ui, host, &mut st.fam, label, seg, 1);
        maximizer::controls(app, ui, host, &mut st.fam, &maximizer::Lay::wide(panel, scale, origin));
    } else if vita {
        let (label, seg) = maximizer::wide_type_rects(panel);
        maximizer::type_selector(app, ui, host, &mut st.fam, label, seg, 2);
        vitalizer::controls(app, ui, host, &mut st.fam, &vitalizer::Lay::wide(panel, scale, origin));
    } else if def.face.empty() {
        columns(app, ui, host, &mut st.fam, panel, cached_circuit);
    } else {
        face_controls(app, ui, host, &mut st.fam, &def.face, scale, origin);
        if def.face.meter_w > 0.0 {
            let vr = Rect::from_center_size(
                pos2(origin.x + def.face.meter_x * scale, origin.y + def.face.meter_y * scale),
                vec2(def.face.meter_w * scale, def.face.meter_h * scale),
            );
            let vr = Rect::from_min_size(vr.min.round(), vr.size().round());
            // The 30 Hz timer: `setLevel (gainToDecibels (takeOutputLevel(), -60), 1 / 30)`, the
            // loudest block's output RMS since the last tick.
            let now = ui.input(|i| i.time);
            if now - st.needle.1 >= 1.0 / 30.0 || now < st.needle.1 {
                let rms = crate::editor_kit::live(host).map_or(0.0, |l| l.take(soundcraft_dsp::plugins::fs::vint::LIVE_OUT_LEVEL));
                let db = if rms.is_finite() && rms > 0.0 { (20.0 * rms.log10()).max(-60.0) } else { -60.0 };
                knob::vu_set_level(&mut st.needle.0, db, 1.0 / 30.0);
                st.needle.1 = now;
            }
            knob::paint_vu(ui.painter(), vr, st.needle.0);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
    }

    // ---- the foot
    let has_stage = d::stage_for(cached_circuit) != soundcraft_dsp::plugins::fs::vint::vint_stages::ST_NONE;
    let stage_on = pv(host, d::P_STAGE) > 0.5;
    let plain = PillLook { tracked: false, ..PillLook::default() };
    if has_stage {
        if pill_ex(ui, stage_r, "vint-stage", pal, "Output Stage", stage_on, PillStyle::Small, plain).clicked() {
            family::pill_set(app, host, &mut st.fam, &FAMILY, "stage", if stage_on { 0.0 } else { 1.0 });
        }
    } else {
        ui.scope(|ui| {
            ui.multiply_opacity(0.35);
            let _ = ui.add_enabled_ui(false, |ui| pill_ex(ui, stage_r, "vint-stage", pal, "Output Stage", stage_on, PillStyle::Small, plain));
        });
    }
    let drives = has_stage && cached_circuit != d::C_CONSOLE;
    let small = |label: &'static str, text: &'static str| DialOpts { label, value_text: text, small: true, ..DialOpts::default() };
    let drive_text = format_knob(d::P_DRIVE, pv(host, d::P_DRIVE));
    let mix_text = format!("{} %", (pv(host, d::P_MIX) * 100.0).round() as i32);
    let out_text = format_knob(d::P_OUTPUT, pv(host, d::P_OUTPUT));
    let default_of = |i: usize| host.info.param(id_of(i)).map_or(0.5, |p| p.to_normalized(p.default));
    if drives {
        let o = DialOpts { label: "Drive", value_text: &drive_text, default: default_of(d::P_DRIVE), small: true, ..DialOpts::default() };
        family::dial_param(app, ui, host, &mut st.fam, &FAMILY, foot_dial(0), pal, "drive", &o);
    } else {
        let v = pv(host, d::P_DRIVE);
        ui.scope(|ui| {
            ui.multiply_opacity(0.35);
            let o = DialOpts { value_text: &drive_text, ..small("Drive", "") };
            let _ = dial(ui, foot_dial(0), "vint-drive-off", pal, v, &DialOpts { locked: false, inactive: false, ..o });
        });
    }
    let o = DialOpts { label: "Mix", value_text: &mix_text, default: default_of(d::P_MIX), small: true, ..DialOpts::default() };
    family::dial_param(app, ui, host, &mut st.fam, &FAMILY, foot_dial(1), pal, "mix", &o);
    let o = DialOpts { label: "Output", value_text: &out_text, default: default_of(d::P_OUTPUT), small: true, ..DialOpts::default() };
    family::dial_param(app, ui, host, &mut st.fam, &FAMILY, foot_dial(2), pal, "output", &o);
    let qsel = (pv(host, d::P_QUALITY).round() as i32).clamp(0, 2) as usize;
    if let Some(i) = segmented(ui, quality_seg, "vint-quality", pal, &["Eco", "Normal", "Insane"], 1, Some(qsel), SelectionStyle::Filled)
        && i != qsel
    {
        family::set(app, host, &mut st.fam, &FAMILY, "quality", i as f32);
    }

    // ---- the curve
    let mut values = [0.0f32; NUM_PARAMS];
    for (i, v) in values.iter_mut().enumerate() {
        *v = pv(host, i);
    }
    scope::draw(ui, scope_r, pal, &mut st.scope, &values, Id::new(("vint-scope", host.track.0, host.ins.id.0)));
}

/// JUCE `removeFromTop`: never more than what is left (an empty strip at the bottom then).
fn take_top(c: &mut Rect, h: f32) -> Rect {
    let h = h.min(c.height()).max(0.0);
    let r = Rect::from_min_size(c.min, vec2(c.width(), h));
    c.min.y += h;
    r
}

/// The flavour circuits' columns (`place`), and their titles.
fn columns(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut FamilyState, panel: Rect, circuit_i: i32) {
    let pal = &PALETTE;
    let enh_type = (pv(host, d::P_ENH_TYPE).round() as i32).clamp(0, 2);
    let cols = columns_for(circuit_i, enh_type);
    let inner = panel.shrink(10.0);
    let n = cols.len().max(1) as f32;
    let col_w = 150f32.min((inner.width() / n).trunc());
    let row = keep_centre(inner, col_w * n, inner.height());
    let p = ui.painter().clone();
    let variable = (pv(host, d::P_SMP_MACHINE).round() as i32) == 2;
    for (ci, col) in cols.iter().enumerate() {
        let mut c = Rect::from_min_size(pos2(row.left() + col_w * ci as f32, row.top()), vec2(col_w, row.height()));
        let title = Rect::from_min_size(c.min, vec2(col_w, K_TITLE_H));
        c.min.y += K_TITLE_H;
        p.text(title.center(), Align2::CENTER_CENTER, col.title.to_uppercase(), bold(CAPTION_SIZE), pal.text_body);
        for ctl in &col.controls {
            let id = id_of(ctl.param);
            match ctl.kind {
                Kind::Knob | Kind::Step => {
                    let cell = take_top(&mut c, K_CELL_H);
                    let r = keep_centre(cell, (col_w - 6.0).min(100.0), K_CELL_H);
                    let text = format_knob_for(ctl.param, pv(host, ctl.param), enh_type);
                    let default = host.info.param(id).map_or(0.5, |p| p.to_normalized(p.default));
                    let o = DialOpts { label: ctl.label, value_text: &text, default, small: true, ..DialOpts::default() };
                    // The variable-rate knob only means something on the Variable machine.
                    if ctl.param == d::P_SMP_RATE && !variable {
                        let v = pv(host, ctl.param);
                        ui.scope(|ui| {
                            ui.multiply_opacity(0.35);
                            let _ = dial(ui, r, "vint-rate-off", pal, v, &o);
                        });
                    } else {
                        family::dial_param(app, ui, host, st, &FAMILY, r, pal, id, &o);
                    }
                }
                Kind::Toggle => {
                    let cell = take_top(&mut c, K_TOGGLE_H + 12.0);
                    let r = keep_centre(cell, (col_w - 20.0).min(100.0), K_TOGGLE_H);
                    let on = pv(host, ctl.param) > 0.5;
                    if pill_ex(
                        ui,
                        r,
                        ("vint-col-toggle", ctl.param),
                        pal,
                        ctl.label,
                        on,
                        PillStyle::Small,
                        PillLook { tracked: false, ..PillLook::default() },
                    )
                    .clicked()
                    {
                        family::pill_set(app, host, st, &FAMILY, id, if on { 0.0 } else { 1.0 });
                    }
                }
                Kind::Seg => {
                    // a selector: one option per row, a column of switches
                    let opts = choices(host, ctl.param);
                    let h = opts.len() as f32 * 26.0;
                    let r = take_top(&mut c, h).shrink2(vec2(6.0, 0.0));
                    let _ = take_top(&mut c, 10.0);
                    let sel = (pv(host, ctl.param).round() as i32).clamp(0, opts.len().saturating_sub(1) as i32) as usize;
                    if let Some(i) = segmented(ui, r, ("vint-col-seg", ctl.param), pal, opts, opts.len(), Some(sel), SelectionStyle::Filled)
                        && i != sel
                    {
                        family::set(app, host, st, &FAMILY, id, i as f32);
                    }
                }
            }
        }
    }
}

/// The EQs' face controls: the knobs and switches dressed as the hardware's, and the toggles.
fn face_controls(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut FamilyState, face: &Face, scale: f32, origin: Pos2) {
    let ink = Jc::argb(face.ink);
    for pc in &face.controls {
        let r = face_bounds(pc, scale, origin);
        let on = pv(host, pc.param) > 0.5;
        match pc.kind {
            Kind::Knob | Kind::Step => {
                let n = if pc.kind == Kind::Step { choices(host, pc.param).len().max(2) } else { 0 };
                let v = pv(host, pc.param);
                let (value, default, readout) = if n > 0 {
                    let idx = (v.round() as i32).clamp(0, n as i32 - 1) as usize;
                    let def = host.info.param(id_of(pc.param)).map_or(0.0, |p| p.default.round());
                    let label = choices(host, pc.param).get(idx).copied().unwrap_or("");
                    (idx as f32 / (n - 1) as f32, def / (n - 1) as f32, step_text(pc.param, label))
                } else {
                    let def = host.info.param(id_of(pc.param)).map_or(0.5, |p| p.default);
                    (v, def, format_knob(pc.param, v))
                };
                let kf = knob::KnobFace {
                    style: pc.style,
                    cap: Jc::argb(pc.cap),
                    ink: Jc::argb(if pc.legend_ink != 0 { pc.legend_ink } else { face.ink }),
                    marks: &pc.marks,
                    positions: if pc.kind == Kind::Step { pc.marks.len() } else { 0 },
                    minor: pc.minor_ticks,
                    travel: (pc.travel_sweep != 0.0).then_some((pc.travel_start, pc.travel_sweep)),
                    dots: pc.dots,
                    name: pc.name,
                    name_below: pc.name_below,
                };
                let out = knob::vint_knob(ui, r, pc.param, &kf, value, default, &readout, true);
                apply_knob(app, host, st, pc.param, n, &out);
            }
            Kind::Toggle | Kind::Seg => {
                let resp = ui.interact(r, ui.id().with(("vint-face-toggle", pc.param, pc.x as i32)), Sense::click());
                resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, if pc.name.is_empty() { id_of(pc.param) } else { pc.name })
                });
                let g = ui.painter();
                match pc.toggle {
                    ToggleStyle::Bat => knob::paint_bat(g, r, pc.name, on, pc.inverted, ink),
                    ToggleStyle::Plate => knob::paint_plate(g, r, pc.name, on, pc.inverted),
                    ToggleStyle::Dome => knob::paint_dome(g, r, pc.name, on, pc.inverted, ink),
                    ToggleStyle::Ssl => knob::paint_ssl(g, r, pc.name, on, false, false),
                    ToggleStyle::SslRed => knob::paint_ssl(g, r, pc.name, on, true, false),
                    ToggleStyle::SslTall => knob::paint_ssl(g, r, pc.name, on, false, true),
                    ToggleStyle::SslTallRed => knob::paint_ssl(g, r, pc.name, on, true, true),
                    ToggleStyle::Pill => {
                        let _ = pill_ex(
                            ui,
                            r,
                            ("vint-face-pill", pc.param),
                            &PALETTE,
                            pc.name,
                            on,
                            PillStyle::Small,
                            PillLook { tracked: false, ..PillLook::default() },
                        );
                    }
                }
                if resp.clicked() {
                    family::pill_set(app, host, st, &FAMILY, id_of(pc.param), if on { 0.0 } else { 1.0 });
                }
            }
        }
    }
}

/// The unit's faceplate (`paint`): its colour, grain, screws, printing, rules, curve symbols and
/// pilot lamp.
fn paint_face(ui: &Ui, g: &egui::Painter, host: &Host, panel: Rect, face: &Face, scale: f32, origin: Pos2) {
    let plate = panel.shrink(4.0);
    let plate_c = Jc::argb(face.plate);
    fill_round_rect(g, plate, 6.0, Fill::linear(plate_c.brighter(0.08), plate.left_top(), plate_c.darker(0.18), plate.left_bottom()));
    if face.texture {
        // `setTiledImageFill (grain, 0, 0)`: 256-point tiles from the editor's origin.
        let tex = grain_texture(ui.ctx());
        let gc = g.with_clip_rect(plate.intersect(g.clip_rect()));
        let o = host.rect.min;
        let (i0, i1) = (((plate.left() - o.x) / 256.0).floor() as i32, ((plate.right() - o.x) / 256.0).ceil() as i32);
        let (j0, j1) = (((plate.top() - o.y) / 256.0).floor() as i32, ((plate.bottom() - o.y) / 256.0).ceil() as i32);
        for j in j0..j1 {
            for i in i0..i1 {
                let cell = Rect::from_min_size(o + vec2(i as f32 * 256.0, j as f32 * 256.0), vec2(256.0, 256.0));
                gc.image(tex.id(), cell, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
        }
    }
    stroke_closed(g, &round_rect(plate, 6.0), 1.0, Jc::WHITE.alpha(0.06));
    for sx in [plate.left() + 9.0, plate.right() - 9.0] {
        for sy in [plate.top() + 9.0, plate.bottom() - 9.0] {
            fill_ellipse(g, Rect::from_center_size(pos2(sx, sy), vec2(8.0, 8.0)), Fill::Solid(Jc::BLACK.alpha(0.35)));
            line(g, pos2(sx - 2.5, sy), pos2(sx + 2.5, sy), 1.0, Jc::WHITE.alpha(0.25));
        }
    }
    let ink = Jc::argb(face.ink);
    let unit = |x: f32, y: f32| pos2(origin.x + x * scale, origin.y + y * scale);
    for t in &face.texts {
        let at = unit(t.x, t.y);
        let c = if t.colour != 0 { Jc::argb(t.colour) } else { ink };
        if t.vertical {
            text_up(g, at, t.text, t.size, c);
        } else {
            g.text(at, Align2::CENTER_CENTER, t.text, bold(t.size), c.c32());
        }
    }
    for l in &face.lines {
        line(g, unit(l.x0, l.y0), unit(l.x1, l.y1), 1.4, ink.alpha(0.55));
    }
    for gl in &face.glyphs {
        // the printed curve symbols, as the 1073 prints them
        let at = unit(gl.x, gl.y);
        let (w, h) = (34.0 * scale / 0.95, 14.0 * scale / 0.95);
        let (x0, x1, yt, yb) = (at.x - w * 0.5, at.x + w * 0.5, at.y - h * 0.5, at.y + h * 0.5);
        let mut paths: Vec<Vec<Pos2>> = Vec::new();
        match gl.kind {
            Glyph::HighShelf => {
                let mut a = vec![pos2(x0, at.y), at];
                quad(&mut a, at, pos2(at.x + w * 0.3, at.y), pos2(x1, yt));
                let mut b = vec![at];
                quad(&mut b, at, pos2(at.x + w * 0.3, at.y), pos2(x1, yb));
                paths.extend([a, b]);
            }
            Glyph::LowShelf => {
                let mut a = vec![pos2(x0, yt)];
                quad(&mut a, pos2(x0, yt), pos2(at.x - w * 0.3, at.y), at);
                let mut b = vec![pos2(x0, yb)];
                quad(&mut b, pos2(x0, yb), pos2(at.x - w * 0.3, at.y), at);
                b.push(pos2(x1, at.y));
                paths.extend([a, b]);
            }
            Glyph::Peak => {
                let rr = h * 0.36;
                paths.push(vec![pos2(x0, at.y), pos2(at.x - rr, at.y)]);
                let mut ring = draw::ellipse(at, rr, rr, 32);
                if let Some(first) = ring.first().copied() {
                    ring.push(first);
                }
                paths.push(ring);
                paths.push(vec![pos2(at.x + rr, at.y), pos2(x1, at.y)]);
            }
            Glyph::HighPass => paths.push(vec![pos2(x0, yb), pos2(x0 + w * 0.38, yt), pos2(x1, yt)]),
        }
        for path in paths {
            g.add(egui::Shape::line(path, egui::Stroke::new(2.0, ink.c32())));
        }
    }
    let on = pv(host, d::P_BYPASS) < 0.5;
    if face.lamp_x >= 0.0 && face.led_lamp {
        // an LED, lit while the unit is IN
        let at = unit(face.lamp_x, face.lamp_y);
        let r = 5.0 * scale / 0.95;
        if on {
            fill_ellipse(
                g,
                Rect::from_center_size(at, vec2(r * 7.0, r * 7.0)),
                Fill::radial(Jc::argb(0x88FF2A1A), at, Jc::argb(0x00FF2A1A), at + vec2(r * 3.5, 0.0)),
            );
        }
        let (c1, c2) = if on { (Jc::argb(0xFFFF6A5A), Jc::argb(0xFFC0180C)) } else { (Jc::argb(0xFF5A1612), Jc::argb(0xFF2A0907)) };
        fill_ellipse(g, Rect::from_center_size(at, vec2(2.0 * r, 2.0 * r)), Fill::radial(c1, at - vec2(r * 0.4, r * 0.4), c2, at + vec2(r, r)));
    } else if face.lamp_x >= 0.0 {
        // the pilot lamp: a red jewel in a chrome bezel, lit while the unit is IN
        let at = unit(face.lamp_x, face.lamp_y);
        let r = 11.0 * scale / 0.95;
        fill_ellipse(
            g,
            Rect::from_min_size(pos2(at.x - r - 2.0 + 1.5, at.y - r - 2.0 + 2.5), vec2(2.0 * r + 4.0, 2.0 * r + 4.0)),
            Fill::Solid(Jc::BLACK.alpha(0.45)),
        );
        fill_ellipse(
            g,
            Rect::from_center_size(at, vec2(2.0 * r + 6.0, 2.0 * r + 6.0)),
            Fill::linear(Jc::argb(0xFFE3E6E8), at - vec2(r, r), Jc::argb(0xFF5F6468), at + vec2(r, r)),
        );
        let (hot, cool) = if on { (Jc::argb(0xFFFF3B2F), Jc::argb(0xFF9E120C)) } else { (Jc::argb(0xFF5A1612), Jc::argb(0xFF2A0907)) };
        fill_ellipse(
            g,
            Rect::from_center_size(at, vec2(2.0 * r, 2.0 * r)),
            Fill::radial(hot.brighter(if on { 0.4 } else { 0.0 }), at - vec2(r * 0.3, r * 0.35), cool, at + vec2(r, r)),
        );
        if on {
            fill_ellipse(
                g,
                Rect::from_center_size(at, vec2(r * 5.2, r * 5.2)),
                Fill::radial(Jc::argb(0x66FF4A3A), at, Jc::argb(0x00FF4A3A), at + vec2(r * 2.6, 0.0)),
            );
        }
        fill_ellipse(g, Rect::from_min_size(pos2(at.x - r * 0.45, at.y - r * 0.55), vec2(r * 0.45, r * 0.35)), Fill::Solid(Jc::WHITE.alpha(0.55)));
    }
    if let Some(badge) = face.badge {
        let r = Rect::from_min_size(pos2(plate.left() + 22.0, plate.bottom() - 22.0), vec2(plate.width() - 22.0, 18.0));
        g.text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, badge, bold(11.0), ink.alpha(0.85).c32());
    }
    let _ = FaceStyle::Round;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_size_is_the_vsts() {
        assert_eq!(SIZE, vec2(1000.0, 643.0));
    }

    #[test]
    fn the_grain_is_seeded() {
        let a = plate_grain();
        assert_eq!(a.len(), 256 * 256 * 4);
        assert_eq!(a, plate_grain());
        let mut r = JuceRandom(0x5EED);
        let x = r.next_float();
        assert!((0.0..1.0).contains(&x));
    }
}

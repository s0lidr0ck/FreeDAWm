//! Cicada, the wavetable synth, as an instrument: the dark, display-led face of the family.
//!
//! The large wave display shows the table as a stack: the wave at the current position bright,
//! its neighbours faint behind it; dragging across it moves Position (one undo step per drag).
//! Beside it the two envelopes. Then the sections in signal order: the table (which, where, pitch
//! and warp), unison, sub and noise with the amp envelope; the filter and the LFO with envelope
//! 2; what moves the position, how it plays, the output. Every parameter is on the panel.

use super::kit::{self, Ctx, Share, adsr, section, section_cells};
use crate::SoundApp;
use egui::{Pos2, Rect, Sense, Ui, pos2, vec2};
use soundcraft_dsp::plugins::wavetable::tables;

/// The wavetable synth's plugin id.
pub const ID: &str = "wavetable_synth";

const DISPLAY_H: f32 = 116.0;

/// The front's height: the displays and three rows of sections.
pub const BODY_H: f32 = DISPLAY_H + 10.0 + 3.0 * (kit::SECTION_H + kit::GAP);

/// Draws Cicada's front (its displays and sections) into `area`.
pub fn body(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, area: Rect) {
    let face = cx.face;
    let p = ui.painter().clone();
    let disp = Rect::from_min_size(area.min, vec2(area.width(), DISPLAY_H));
    let parts = kit::row(disp, &[Share::Knobs(1), Share::Fixed(190.0)]);
    if cx.back {
        for (r, tag) in parts.iter().zip(["Wavetable", "Envelopes"]) {
            kit::rear_plate(&p, *r, tag, face);
        }
    } else if let [wave, envs] = parts.as_slice() {
        draw_table(app, ui, *wave, cx);
        draw_envelopes(&p, *envs, cx);
    }

    let row_h = kit::SECTION_H;
    let mut y = disp.bottom() + 10.0;
    let mut next_row = |shares: &[Share]| {
        let rr = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), row_h));
        y += row_h + kit::GAP;
        kit::row(rr, shares)
    };
    let fw = kit::ENV_W;
    let (env1, env2) = (kit::SOURCE_COLOURS[0], kit::SOURCE_COLOURS[1]);

    let r1 = next_row(&[Share::Knobs(4), Share::Knobs(2), Share::Knobs(3), Share::Knobs(2), Share::Fixed(fw)]);
    if let [table, warp, uni, subn, amp] = r1.as_slice() {
        section(&p, *table, "Table", &table_name(cx), face);
        let c = section_cells(*table, 4);
        kit::knob(app, ui, cx, c[0], "table", "Table");
        kit::knob(app, ui, cx, c[1], "position", "Pos");
        kit::knob(app, ui, cx, c[2], "osc_pitch", "Pitch");
        kit::knob(app, ui, cx, c[3], "osc_fine", "Fine");
        section(&p, *warp, "Warp", &kit::short_value_of(cx, "warp_mode"), face);
        let c = section_cells(*warp, 2);
        kit::knob(app, ui, cx, c[0], "warp_mode", "Mode");
        kit::knob(app, ui, cx, c[1], "warp_amount", "Amt");
        let voices = cx.param_info("unison").map(|i| i.format(cx.value("unison"))).unwrap_or_default();
        section(&p, *uni, "Unison", if voices == "Off" { "off" } else { "" }, face);
        let c = section_cells(*uni, 3);
        kit::knob(app, ui, cx, c[0], "unison", "Voices");
        kit::knob(app, ui, cx, c[1], "unison_detune", "Detune");
        kit::knob(app, ui, cx, c[2], "unison_spread", "Spread");
        section(&p, *subn, "Sub & Noise", "", face);
        let c = section_cells(*subn, 2);
        kit::knob(app, ui, cx, c[0], "sub_level", "Sub");
        kit::knob(app, ui, cx, c[1], "noise_level", "Noise");
        section(&p, *amp, "Amp Env", "", face);
        adsr(app, ui, cx, *amp, ["amp_attack", "amp_decay", "amp_sustain", "amp_release"], env1);
        kit::outputs_on(ui, cx, *amp, &[0]);
    }

    let r2 = next_row(&[Share::Knobs(5), Share::Knobs(5), Share::Fixed(fw)]);
    if let [filt, lfo, e2] = r2.as_slice() {
        let kind = kit::short_value_of(cx, "filter_type");
        section(&p, *filt, "Filter", &kind, face);
        let c = section_cells(*filt, 5);
        kit::knob(app, ui, cx, c[0], "filter_type", "Type");
        kit::knob(app, ui, cx, c[1], "cutoff", "Cutoff");
        kit::knob(app, ui, cx, c[2], "resonance", "Reso");
        kit::knob(app, ui, cx, c[3], "env2_cutoff", "Env 2");
        kit::knob(app, ui, cx, c[4], "key_track", "Key Trk");
        let shape = kit::short_value_of(cx, "lfo_wave");
        section(&p, *lfo, "LFO", &shape, face);
        kit::outputs_on(ui, cx, *lfo, &[2]);
        let c = section_cells(*lfo, 5);
        kit::knob(app, ui, cx, c[0], "lfo_wave", "Shape");
        kit::knob(app, ui, cx, c[1], "lfo_rate", "Rate");
        kit::knob(app, ui, cx, c[2], "lfo_pitch", "Pitch");
        kit::knob(app, ui, cx, c[3], "lfo_cutoff", "Cutoff");
        kit::knob(app, ui, cx, c[4], "lfo_amp", "Level");
        section(&p, *e2, "Env 2", "", face);
        adsr(app, ui, cx, *e2, ["env2_attack", "env2_decay", "env2_sustain", "env2_release"], env2);
        kit::outputs_on(ui, cx, *e2, &[1]);
    }

    let r3 = next_row(&[Share::Knobs(4), Share::Knobs(4), Share::Knobs(2)]);
    if let [pm, play, out] = r3.as_slice() {
        section(&p, *pm, "Position Mod", "", face);
        let c = section_cells(*pm, 4);
        kit::knob(app, ui, cx, c[0], "env2_position", "Env 2");
        kit::knob(app, ui, cx, c[1], "lfo_position", "LFO");
        kit::knob(app, ui, cx, c[2], "velocity_position", "Vel");
        kit::knob(app, ui, cx, c[3], "wheel_position", "Wheel");
        section(&p, *play, "Play", "", face);
        kit::outputs_on(ui, cx, *play, &[3, 4, 5, 6]);
        let c = section_cells(*play, 4);
        kit::knob(app, ui, cx, c[0], "glide", "Glide");
        kit::knob(app, ui, cx, c[1], "bend_range", "Bend");
        kit::knob(app, ui, cx, c[2], "mod_wheel", "Vibrato");
        kit::knob(app, ui, cx, c[3], "velocity", "Vel Sens");
        section(&p, *out, "Output", "", face);
        let c = section_cells(*out, 2);
        kit::knob(app, ui, cx, c[0], "level", "Level");
    }
}

/// The table playing: the user's own (by its name) or the built-in bank's.
fn table_name(cx: &Ctx) -> String {
    match crate::wavetable_import::user_table(cx.ins) {
        Some(t) => format!("{} (own)", t.name),
        None => bank_name(cx),
    }
}

/// The built-in table's name.
fn bank_name(cx: &Ctx) -> String {
    let bank = cx.value("table").round().clamp(0.0, (tables::BANKS.len() - 1) as f32) as usize;
    tables::BANKS.get(bank).copied().unwrap_or("").to_string()
}

/// The table as a stack: ten waves through it, back to front, the one at the current position
/// bright and the rest faint; dragging moves Position (one undo step a drag).
fn draw_table(app: &mut SoundApp, ui: &mut Ui, r: Rect, cx: &Ctx) {
    let face = cx.face;
    let p = ui.painter().clone();
    kit::display(&p, r, "Wavetable", face);
    let bank = cx.value("table").round().clamp(0.0, (tables::BANKS.len() - 1) as f32) as usize;
    let pos = (cx.value("position") / 100.0).clamp(0.0, 1.0);
    const LAYERS: usize = 10;
    let (dx, dyy) = (10.0, 6.5);
    let w = r.width() - 40.0 - dx * (LAYERS - 1) as f32;
    let amp = 18.0;
    let mut y = vec![0.0f32; 192];
    let current = ((pos * (LAYERS - 1) as f32).round() as usize).min(LAYERS - 1);
    // An imported table plays instead of the bank (see `wavetable_import`).
    let user = crate::wavetable_import::user_table(cx.ins);
    // Back to front: the deepest layer first, so the nearer ones cover it.
    for i in (0..LAYERS).rev() {
        let at = i as f32 / (LAYERS - 1) as f32;
        let here = i == current;
        let wave_pos = if here { pos } else { at };
        match user.as_deref() {
            Some(t) => t.draw(wave_pos, &mut y),
            None => tables::draw(bank, wave_pos, &mut y),
        }
        let ox = r.left() + 20.0 + dx * i as f32;
        let oy = r.bottom() - 28.0 - dyy * i as f32;
        let pts: Vec<Pos2> =
            y.iter().enumerate().map(|(k, v)| pos2(ox + w * k as f32 / (y.len() - 1) as f32, oy - v.clamp(-1.2, 1.2) * amp)).collect();
        if here {
            kit::trace(&p, pts, face.glow, 2.6);
        } else {
            p.add(egui::Shape::line(pts, egui::Stroke::new(1.1, kit::alpha(face.well_ink, 0.22))));
        }
    }
    let resp = ui.interact(r, egui::Id::new(("cicada-table", cx.track.0)), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Wave display"));
    let resp = resp.on_hover_text(format!(
        "Position {:.0}% · drag across to move through the table · drop an audio file here, or right-click, to make a table from your own audio",
        pos * 100.0
    ));
    // A dropped audio file opens Import Wavetable with it; the right-click menu has the rest.
    crate::wavetable_import::drop_target(app, ui.ctx(), r, cx.track);
    let has_user = user.is_some();
    resp.context_menu(|ui| {
        if ui.button("Import Wavetable...").clicked() {
            crate::wavetable_import::open(app, cx.track, None);
            ui.close();
        }
        if ui.add_enabled(has_user, egui::Button::new("Use the Built-in Table")).on_disabled_hover_text("The built-in table is playing").clicked() {
            if let Err(e) = app.run("mix.wavetable_clear", serde_json::json!({"track": cx.track.0})) {
                app.ui.status = e;
            }
            ui.close();
        }
    });
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        let new = (pos * 100.0 + (d.x - d.y) / (r.width() * 0.6) * 100.0).clamp(0.0, 100.0);
        if (new - pos * 100.0).abs() > 1e-3 {
            kit::set_param(app, cx.track, "position", new, "wave-display");
        }
    }
    if resp.drag_stopped() {
        app.engine.end_merge();
    }
    let (name, waves) = match user.as_deref() {
        Some(t) => (t.name.clone(), t.wave_count()),
        None => (bank_name(cx), tables::FRAMES),
    };
    let wave_no = (pos * (waves.max(1) - 1) as f32).round() as usize + 1;
    kit::display_note(&p, r, &format!("{}   WAVE {wave_no}/{waves}", name.to_uppercase()), face);
}

/// The amp envelope over envelope 2, each to its own scale.
fn draw_envelopes(p: &egui::Painter, r: Rect, cx: &Ctx) {
    kit::display(p, r, "Envelopes", cx.face);
    let h = (r.height() - 30.0) / 2.0;
    let a = Rect::from_min_size(pos2(r.left() + 10.0, r.top() + 20.0), vec2(r.width() - 20.0, h - 3.0));
    let b = Rect::from_min_size(pos2(r.left() + 10.0, a.bottom() + 6.0), vec2(r.width() - 20.0, h - 3.0));
    super::burley::draw_adsr(p, a, cx, "amp", kit::SOURCE_COLOURS[0]);
    super::burley::draw_adsr(p, b, cx, "env2", kit::SOURCE_COLOURS[1]);
}

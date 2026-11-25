//! Plumb's editor: FreeDAWm's own design, built on [`crate::editor_kit`] in the Engine
//! family's look.
//!
//! - The header: the title, the preset strip (Gentle, Medium, Hard), the editor's Undo and A/B, and
//!   Bypass (the module's own bypass in the chain).
//! - The character switch: Natural or Hard tune, one click (it sets Retune speed, Humanize, Keep
//!   vibrato and Note hold; moving any of those afterwards shows Custom).
//! - The key: Follow the song (the Key ruler, key changes included) or a key of its own with a
//!   scale; the twelve notes, each on or off (a scale fills them; switching one makes the scale
//!   Custom).
//! - The pitch display: the sung pitch and the corrected pitch scrolling against the key's note
//!   lanes, and the current note with how far off it was sung, from the module's readouts.
//! - The mode switch: Studio (43 ms, compensated) or Live (a few ms, for singing through it), with
//!   the delay it adds; in Live, the Voice range in place of Formant.
//! - The dials: Retune speed, Humanize, Note hold, Keep vibrato; Formant and Mix.
//!
//! Every change goes through `mix.module_params` (one undo step per gesture).

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, SelectionStyle, dial, pill_ex, pill_height, segmented_ex, segmented_id, tip};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, TINY_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::presets;
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, readouts, run_with, set_params, set_params_merged};
use crate::theme::regular;
use egui::{Align2, Color32, Rect, Response, Stroke, Ui, Vec2, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::plugins::autopitch::{NOTE_NAMES, SCALE_CUSTOM, SCALES, STYLE_CUSTOM, STYLE_HARD, STYLE_NATURAL, STYLE_VALUES, rotate};
use soundcraft_mix::StripMeter;
use soundcraft_model::{ModuleId, Track};
use std::collections::VecDeque;

const PAD_X: f32 = 22.0;
const PAD_Y: f32 = 14.0;
const GAP: f32 = 12.0;
const HEADER_H: f32 = 26.0;
const CHAR_H: f32 = 34.0;
const KEY_H: f32 = 56.0;
const DISPLAY_H: f32 = 190.0;
const SMALL_KNOB: f32 = 58.0;
const GROUP_LABEL_H: f32 = 18.0;
const CONTROLS_H: f32 = GROUP_LABEL_H + SMALL_KNOB + 30.0 + 14.0;
const HEIGHT: f32 = PAD_Y * 2.0 + HEADER_H + GAP + CHAR_H + GAP + KEY_H + GAP + DISPLAY_H + GAP + CONTROLS_H;
const NOTE_W: f32 = 168.0;

/// The editor's size.
pub const SIZE: Vec2 = vec2(760.0, HEIGHT);

/// Plumb's accent: a clear teal.
pub const ACCENT: u32 = 0xFF3E_C8B4;
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(ACCENT));

/// A/B and Undo walk every parameter but the key, the scale, the notes, the mode and the voice
/// range, which presets leave alone too: recalling Hard never changes the key or the mode.
pub(crate) const FAMILY: Family = Family::new(
    "AutoPitch",
    &["key", "scale", "note0", "note1", "note2", "note3", "note4", "note5", "note6", "note7", "note8", "note9", "note10", "note11", "mode", "range"],
);

/// The controls a Style sets.
const STYLE_PARAMS: [&str; 4] = ["retune", "humanize", "vibrato", "hold"];
/// How long the pitch display shows (seconds).
const SHOWN_SECS: f64 = 4.0;

/// What each control does, for its tooltip.
const TIPS: [(&str, &str); 11] = [
    (
        "style",
        "Natural: gentle correction that keeps the vibrato (long notes more gently still). Hard tune: every note snaps at once and the vibrato is flattened. Moving Retune speed, Humanize, Keep vibrato or Note hold makes it Custom.",
    ),
    ("key", "Follow the song: the Key ruler's key at each moment, key changes included. Or pick a key of its own and a scale."),
    ("scale", "The scale of its own key: it fills the twelve notes. Switching a note on or off makes it Custom."),
    ("retune", "How fast a note is pulled onto the key: Instant (hard tune) to 200 ms (slow and natural)."),
    ("humanize", "Notes held longer than half a second are corrected more gently, so long notes keep their life."),
    ("vibrato", "On: the note's centre is corrected and the singer's vibrato kept. Off: the vibrato is flattened too."),
    (
        "hold",
        "How much closer the voice must get to another note before the target changes. Higher stops a wide vibrato flipping between neighbouring notes; lower follows quick note changes sooner.",
    ),
    ("formant", "Keep: the voice's character stays as sung. Shift it up or down (semitones) to sound brighter or darker."),
    ("mix", "How much of the corrected voice you hear against the original (both delayed alike)."),
    ("mode", MODE_TIP),
    ("range", RANGE_TIP),
];

const MODE_TIP: &str = "Studio: about 43 ms of delay, lined up by the mixer with every other track; the cleanest sound, for recorded tracks (with Low Latency Monitoring on it is bypassed on an armed or monitored track, like any delaying effect). Live: about 4 to 8 ms, for singing through it; Low Latency Monitoring keeps it in, and you hear yourself that much later plus your interface's own round trip. Live keeps about half the vibrato at Hard tune and moves the vowel colour a little with the correction; Formant does not apply.";
const RANGE_TIP: &str = "Live mode listens down to the lowest note of the voice range: a higher range means less delay. Low: from 70 Hz (bass voices); Middle: from 100 Hz (most voices); High: from 150 Hz.";

fn tip_of(id: &str) -> &'static str {
    TIPS.iter().find(|(k, _)| *k == id).map_or("", |t| t.1)
}

fn value_text(host: &Host, id: &str) -> String {
    let v = param(host, id);
    match id {
        "retune" if v < 0.5 => "Instant".into(),
        "retune" => format!("{v:.0} ms"),
        "hold" => format!("{v:.0} c"),
        "formant" if v.abs() < 0.05 => "Keep".into(),
        "formant" => format!("{v:+.1} st"),
        _ => format!("{v:.0} %"),
    }
}

/// Note names spelled with sharps (the keys of G, D, A, E, B and F# major and their minors).
const SHARP_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Whether the key whose notes are `mask` is written with sharps: its major (or relative major)
/// tonic is one of G, D, A, E, B, F#.
fn sharp_key(mask: u16) -> bool {
    (0..12u8).find(|t| rotate(SCALES[0].1, *t) == mask).is_some_and(|t| [7, 2, 9, 4, 11, 6].contains(&t))
}

/// A pitch class's name as the key `mask` spells it.
fn pc_name(pc: usize, mask: u16) -> &'static str {
    let names = if sharp_key(mask) { &SHARP_NAMES } else { &NOTE_NAMES };
    names.get(pc % 12).copied().unwrap_or("")
}

/// A note's name and octave from MIDI cents, spelled as the key `mask` spells it ("F#4").
fn note_name(cents: f32, mask: u16) -> String {
    let m = (cents / 100.0).round() as i32;
    format!("{}{}", pc_name(m.rem_euclid(12) as usize, mask), m.div_euclid(12) - 1)
}

#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    /// (time, sung, corrected) as the readouts gave them, newest last.
    hist: VecDeque<(f64, f32, f32)>,
    /// The display's centre (MIDI cents), following the voice slowly.
    centre: f32,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<Editor>(app, ui, track, module, meters, |_, _| {});
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "autopitch";

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

fn style(host: &Host) -> usize {
    param(host, "style").round().max(0.0) as usize
}

/// A change to one of the controls a Style sets: under Natural or Hard tune the others take the
/// Style's values and the Style becomes Custom, in the same step.
fn with_custom(host: &Host, id: &str, v: f32) -> Vec<(&'static str, f32)> {
    let mut list: Vec<(&'static str, f32)> = Vec::new();
    let st = style(host);
    if STYLE_PARAMS.contains(&id) && st != STYLE_CUSTOM {
        if let Some(sv) = STYLE_VALUES.get(st) {
            list.extend([("retune", sv.0), ("humanize", sv.1), ("vibrato", if sv.2 { 1.0 } else { 0.0 }), ("hold", sv.3)]);
        }
        list.push(("style", STYLE_CUSTOM as f32));
    }
    let key = STYLE_PARAMS.iter().chain(["formant", "mix"].iter()).find(|k| **k == id).copied();
    if let Some(k) = key {
        list.retain(|(p, _)| *p != k);
        list.push((k, v));
    }
    list
}

fn dial_row(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, dials: &[(&str, &str)]) {
    let cell_w = (r.width() / dials.len().max(1) as f32).trunc();
    for (i, (id, label)) in dials.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(r.left() + cell_w * i as f32, r.top()), vec2(cell_w, r.height()));
        let rect = keep_centre(cell, (cell_w - 4.0).min(96.0), SMALL_KNOB + 28.0);
        let Some(info) = host.info.params.iter().find(|p| p.id == *id) else { continue };
        let text = value_text(host, id);
        // Under Natural or Hard tune the dial shows the Style's own value.
        let shown = match (STYLE_PARAMS.contains(id), STYLE_VALUES.get(style(host))) {
            (true, Some(sv)) => match *id {
                "retune" => sv.0,
                "humanize" => sv.1,
                _ => sv.3,
            },
            _ => param(host, id),
        };
        let text = if STYLE_PARAMS.contains(id) && style(host) != STYLE_CUSTOM { info.format(shown) } else { text };
        let o = DialOpts { label, value_text: &text, default: family::default01(host, id), small: true, info: Some(info), ..DialOpts::default() };
        let out = dial(ui, rect, ("ap-dial", *id), &PALETTE, info.to_normalized(shown), &o);
        if out.started {
            end_gesture(app);
            family::push_undo(fam, host, &FAMILY);
        }
        if let Some(v) = out.value {
            set_params_merged(app, host, &with_custom(host, id, info.clamp(info.from_normalized(v))), &format!("dial-{id}"));
        }
        if out.ended {
            end_gesture(app);
        }
        let _ = out.response.on_hover_text(tip_of(id));
    }
}

fn group_label(p: &egui::Painter, panel: Rect, text: &str) {
    p.text(pos2(panel.left() + 12.0, panel.top() + 13.0), Align2::LEFT_CENTER, text.to_uppercase(), regular(TINY_SIZE), PALETTE.text_faint);
}

fn plain() -> PillLook {
    PillLook { tracked: false, ..PillLook::default() }
}

fn header_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, header: Rect) {
    let pal = &PALETTE;
    let title_w = paint_title(ui.painter(), header, "Plumb", "Pitch Correction", pal);
    let h = pill_height(PillStyle::Small);
    let at = |right: f32, w: f32| Rect::from_min_size(pos2(right - w, header.center().y - h * 0.5), vec2(w, h));
    let bypass = at(header.right(), 58.0);
    let b = at(bypass.left() - 6.0, 26.0);
    let a = at(b.left() - 3.0, 26.0);
    let undo = at(a.left() - 6.0, 46.0);
    if pill_ex(ui, undo, "ap-undo", pal, "Undo", false, PillStyle::Small, PillLook { dimmed: !st.fam.can_undo(), ..plain() }).clicked() {
        family::do_undo(app, host, &mut st.fam);
    }
    if pill_ex(ui, a, "ap-a", pal, "A", !st.fam.on_b(), PillStyle::Small, plain()).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, false);
    }
    if pill_ex(ui, b, "ap-b", pal, "B", st.fam.on_b(), PillStyle::Small, plain()).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, true);
    }
    let on = host.ins.bypass;
    let look = PillLook { active_colour: Some(pal.bypass_red), ..plain() };
    let resp = pill_ex(ui, bypass, "ap-bypass", pal, "Bypass", on, PillStyle::Small, look);
    if resp.clicked() {
        end_gesture(app);
        let r = app.run("mix.module_bypass", json!({"track": host.track.0, "module": host.ins.id.0, "value": !on})).map(|_| ());
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
    let _ = resp.on_hover_text("Take Plumb out of the signal (the voice still comes out delayed alike, so nothing moves in time).");
    let free = Rect::from_min_max(pos2(header.left() + title_w + 16.0, header.top()), pos2(undo.left() - 16.0, header.bottom()));
    let name_w = (free.width() - 60.0).clamp(100.0, 200.0);
    let strip = Rect::from_center_size(free.center(), vec2(name_w + 60.0, h + 2.0));
    let prev = Rect::from_min_size(pos2(strip.left(), strip.center().y - h * 0.5), vec2(24.0, h));
    let next = Rect::from_min_size(pos2(strip.right() - 24.0, strip.center().y - h * 0.5), vec2(24.0, h));
    let name = Rect::from_min_max(pos2(prev.right() + 6.0, strip.top()), pos2(next.left() - 6.0, strip.bottom()));
    st.fam.presets.not_in_presets = FAMILY.not_in_presets;
    for (rect, left) in [(prev, true), (next, false)] {
        if family::arrow_pill(ui, rect, ("ap-preset-step", left), pal, left).clicked() {
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

/// The character switch (Natural / Hard tune; neither lit under Custom) and the mode's line.
fn character_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, row: Rect) {
    let pal = &PALETTE;
    let seg = Rect::from_min_size(pos2(row.left(), row.top() + 2.0), vec2(300.0, row.height() - 4.0));
    let s = style(host);
    let sel = (s != STYLE_CUSTOM).then_some(s);
    if let Some(i) = segmented_ex(ui, seg, "ap-style", pal, &["Natural", "Hard tune"], 1, sel, SelectionStyle::Filled, false) {
        let pick = if i == 0 { STYLE_NATURAL } else { STYLE_HARD };
        if let Some(sv) = STYLE_VALUES.get(pick) {
            family::push_undo(&mut st.fam, host, &FAMILY);
            end_gesture(app);
            set_params(
                app,
                host,
                &[("style", pick as f32), ("retune", sv.0), ("humanize", sv.1), ("vibrato", if sv.2 { 1.0 } else { 0.0 }), ("hold", sv.3)],
            );
        }
    }
    tip(ui, segmented_id(ui, "ap-style"), tip_of("style"));
    let p = ui.painter();
    if s == STYLE_CUSTOM {
        p.text(pos2(seg.right() + 12.0, seg.center().y), Align2::LEFT_CENTER, "Custom", regular(LABEL_SIZE), pal.accent);
    }
    // Studio / Live, and the delay it adds.
    let live = live_mode(host);
    let mode = Rect::from_min_size(pos2(row.right() - 190.0, seg.top()), vec2(190.0, seg.height()));
    if let Some(i) = segmented_ex(ui, mode, "ap-mode", pal, &["Studio", "Live"], 1, Some(usize::from(live)), SelectionStyle::Filled, false) {
        family::push_undo(&mut st.fam, host, &FAMILY);
        end_gesture(app);
        set_params(app, host, &[("mode", i as f32)]);
    }
    tip(ui, segmented_id(ui, "ap-mode"), MODE_TIP);
    let latency_ms = readouts(host).map(|r| r[6]).filter(|v| v.is_finite() && (r_mode(host) == live)).unwrap_or_else(|| delay_ms(host));
    let text = if live { format!("about {latency_ms:.0} ms: sing through it") } else { format!("{latency_ms:.0} ms delay, lined up by the mixer") };
    let r = ui.painter().text(pos2(mode.left() - 12.0, seg.center().y), Align2::RIGHT_CENTER, text, regular(CAPTION_SIZE), pal.text_dim);
    let resp = ui.interact(r, ui.id().with("ap-delay"), egui::Sense::hover());
    let _ = resp.on_hover_text(MODE_TIP);
}

/// Whether the module is set to Live mode.
fn live_mode(host: &Host) -> bool {
    param(host, "mode") > 0.5
}

/// The mode the readouts come from (Live when [7] says so).
fn r_mode(host: &Host) -> bool {
    readouts(host).is_some_and(|r| r[7] > 0.5)
}

/// The delay the settings give (ms), before any readout: Studio's frame, or Live's 1 ms plus half
/// the voice range's longest period.
fn delay_ms(host: &Host) -> f32 {
    if live_mode(host) {
        let floor = soundcraft_dsp::plugins::autopitch::RANGE_FLOORS.get(param(host, "range").round().max(0.0) as usize).copied().unwrap_or(100.0);
        (1.0 + 500.0 / floor) as f32
    } else {
        // The Vocal Editor's frame: 2048 points at 48 kHz, the nearest power of two at other rates.
        let n = 2f64.powf((2048.0 * host.sr / 48_000.0).max(1.0).log2().round().clamp(8.0, 16.0));
        (n / host.sr * 1000.0) as f32
    }
}

/// The key, the scale and the twelve notes.
fn key_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, panel: Rect) {
    let pal = &PALETTE;
    paint_panel(ui.painter(), panel, 10.0);
    group_label(ui.painter(), panel, "Key");
    let h = pill_height(PillStyle::Small);
    let y = panel.top() + GROUP_LABEL_H + 8.0;
    let key = param(host, "key").round().max(0.0) as usize;
    let follow = key == 0;
    let scale = param(host, "scale").round().max(0.0) as usize;
    // Key: ◀ name ▶
    let stepper = |ui: &mut Ui, x: f32, w: f32, salt: &str, text: &str, dimmed: bool| -> (Response, Response, Response) {
        let prev = Rect::from_min_size(pos2(x, y), vec2(22.0, h));
        let name = Rect::from_min_size(pos2(x + 26.0, y), vec2(w, h));
        let next = Rect::from_min_size(pos2(name.right() + 4.0, y), vec2(22.0, h));
        let a = family::arrow_pill(ui, prev, (salt, "prev"), pal, true);
        let look = PillLook { dimmed, ..plain() };
        let n = pill_ex(ui, name, (salt, "name"), pal, text, false, PillStyle::Small, look);
        let b = family::arrow_pill(ui, next, (salt, "next"), pal, false);
        (a, n, b)
    };
    let key_info = host.info.param("key");
    let key_text = key_info.map_or(String::new(), |i| i.format(key as f32));
    let x0 = panel.left() + 12.0;
    let (a, n, b) = stepper(ui, x0, 118.0, "ap-key", &key_text, false);
    let step_key = |d: i32| ((key as i32 + d).rem_euclid(13)) as f32;
    let mut change: Option<Vec<(&'static str, f32)>> = None;
    if a.clicked() {
        change = Some(key_change(host, step_key(-1)));
    }
    if b.clicked() {
        change = Some(key_change(host, step_key(1)));
    }
    let _ = n.on_hover_text(tip_of("key"));
    let scale_text = host.info.param("scale").map_or(String::new(), |i| i.format(scale as f32));
    let scale_text = if follow { "Song's major / minor".to_string() } else { scale_text };
    let (a, n, b) = stepper(ui, x0 + 178.0, 136.0, "ap-scale", &scale_text, follow);
    if !follow {
        let step = |d: i32| ((scale as i32 + d).rem_euclid(SCALES.len() as i32 + 1)) as usize;
        if a.clicked() {
            change = Some(scale_change(host, step(-1)));
        }
        if b.clicked() {
            change = Some(scale_change(host, step(1)));
        }
    }
    let _ = n.on_hover_text(if follow {
        "Following the song: its key's major or minor scale. Pick a key of its own to choose a scale and notes."
    } else {
        tip_of("scale")
    });
    // The twelve notes: the ones in use lit.
    let (tonic, mask) = shown_key(host);
    let notes_x = x0 + 178.0 + 136.0 + 64.0;
    let w = ((panel.right() - 12.0 - notes_x) / 12.0).floor();
    for pc in 0..12usize {
        let r = Rect::from_min_size(pos2(notes_x + w * pc as f32, y), vec2(w - 3.0, h));
        let on = mask & (1 << pc) != 0;
        let look = PillLook { dimmed: follow, active_colour: (pc as u8 == tonic).then_some(pal.accent), ..plain() };
        let resp = pill_ex(ui, r, ("ap-note", pc), pal, pc_name(pc, mask), on, PillStyle::Small, look);
        if resp.clicked() && !follow {
            let mut list: Vec<(&'static str, f32)> = Vec::new();
            for (i, id) in NOTE_IDS.iter().enumerate() {
                let bit = mask & (1 << i) != 0;
                let v = if i == pc { !bit } else { bit };
                list.push((id, if v { 1.0 } else { 0.0 }));
            }
            list.push(("scale", SCALE_CUSTOM as f32));
            change = Some(list);
        }
        let _ = resp.on_hover_text(if follow {
            "The notes of the song's key here. Pick a key of its own to switch notes on and off."
        } else {
            "On: voices near this note are pulled onto it. Off: it is skipped, to the nearest note that is on."
        });
    }
    if let Some(list) = change {
        family::push_undo(&mut st.fam, host, &FAMILY);
        end_gesture(app);
        set_params(app, host, &list);
    }
}

const NOTE_IDS: [&str; 12] = ["note0", "note1", "note2", "note3", "note4", "note5", "note6", "note7", "note8", "note9", "note10", "note11"];

/// The notes as switches: (id, on) for `mask`.
fn notes_of(mask: u16) -> impl Iterator<Item = (&'static str, f32)> {
    NOTE_IDS.iter().enumerate().map(move |(i, id)| (*id, if mask & (1 << i) != 0 { 1.0 } else { 0.0 }))
}

/// A new key: under a named scale the note switches follow it.
fn key_change(host: &Host, key: f32) -> Vec<(&'static str, f32)> {
    let mut list = vec![("key", key)];
    let scale = param(host, "scale").round().max(0.0) as usize;
    if let Some(s) = SCALES.get(scale)
        && key >= 1.0
    {
        list.extend(notes_of(rotate(s.1, key as u8 - 1)));
    }
    list
}

/// A new scale: it fills the note switches (Custom keeps them).
fn scale_change(host: &Host, scale: usize) -> Vec<(&'static str, f32)> {
    let mut list = vec![("scale", scale as f32)];
    let tonic = param(host, "key").round().max(1.0) as u8 - 1;
    if let Some(s) = SCALES.get(scale) {
        list.extend(notes_of(rotate(s.1, tonic)));
    }
    list
}

/// The key the module uses now: its readouts, or worked out from the settings (following the
/// song without readouts: the band's key).
fn shown_key(host: &Host) -> (u8, u16) {
    if let Some(r) = readouts(host)
        && r[3].is_finite()
        && r[4].is_finite()
        && r[4] > 0.0
    {
        return (r[3] as u8 % 12, r[4] as u16);
    }
    let key = param(host, "key").round().max(0.0) as usize;
    if key == 0 {
        let b = &host.session.band;
        let scale = if b.scale == 1 { SCALES[1].1 } else { SCALES[0].1 };
        return (b.key % 12, rotate(scale, b.key));
    }
    let tonic = (key - 1) as u8;
    let scale = param(host, "scale").round().max(0.0) as usize;
    let mask = match SCALES.get(scale) {
        Some(s) => rotate(s.1, tonic),
        None => (0..12).filter(|i| param(host, NOTE_IDS[*i]) > 0.5).fold(0u16, |m, i| m | (1 << i)),
    };
    (tonic, mask)
}

/// The scrolling pitch display and the current note.
fn display_ui(st: &mut Editor, ui: &mut Ui, host: &Host, panel: Rect) {
    let pal = &PALETTE;
    paint_panel(ui.painter(), panel, 10.0);
    let now = ui.input(|i| i.time);
    let r = readouts(host);
    let (sung, moved, target) = r.map_or((f32::NAN, f32::NAN, f32::NAN), |r| (r[0], r[1], r[2]));
    st.hist.push_back((now, sung, moved));
    while st.hist.front().is_some_and(|h| now - h.0 > SHOWN_SECS) || st.hist.len() > 2000 {
        st.hist.pop_front();
    }
    if sung.is_finite() {
        let goal = (sung / 100.0).round() * 100.0;
        st.centre = if st.centre == 0.0 || (st.centre - goal).abs() > 600.0 { goal } else { st.centre + (goal - st.centre) * 0.05 };
    } else if st.centre == 0.0 {
        st.centre = 6000.0;
    }
    let plot = Rect::from_min_max(pos2(panel.left() + 40.0, panel.top() + 10.0), pos2(panel.right() - NOTE_W - 12.0, panel.bottom() - 10.0));
    let p = ui.painter().with_clip_rect(panel.intersect(ui.clip_rect()));
    let span = 900.0f32;
    let lo = st.centre - span / 2.0;
    let y_of = |c: f32| plot.bottom() - (c - lo) / span * plot.height();
    let (tonic, mask) = shown_key(host);
    let first = (lo / 100.0).ceil() as i32;
    let last = ((lo + span) / 100.0).floor() as i32;
    for m in first..=last {
        let pc = m.rem_euclid(12) as usize;
        let y = y_of(m as f32 * 100.0);
        if mask & (1 << pc) != 0 {
            let lane = Rect::from_min_max(pos2(plot.left(), y - 5.0), pos2(plot.right(), y + 5.0));
            let fill = if pc as u8 == tonic { al(pal.accent, 0.16) } else { al(Color32::WHITE, 0.05) };
            p.rect_filled(lane, 2.0, fill);
            p.text(pos2(plot.left() - 6.0, y), Align2::RIGHT_CENTER, note_name(m as f32 * 100.0, mask), regular(TINY_SIZE), pal.text_dim);
        } else {
            p.hline(plot.x_range(), y, Stroke::new(1.0, al(Color32::WHITE, 0.03)));
        }
    }
    let x_of = |t: f64| plot.right() - ((now - t) / SHOWN_SECS) as f32 * plot.width();
    let line = |which: usize, colour: Color32, width: f32| {
        let mut run: Vec<egui::Pos2> = Vec::new();
        for h in &st.hist {
            let v = if which == 0 { h.1 } else { h.2 };
            if v.is_finite() && (lo..lo + span).contains(&v) {
                run.push(pos2(x_of(h.0), y_of(v)));
            } else if !run.is_empty() {
                p.add(egui::Shape::line(std::mem::take(&mut run), Stroke::new(width, colour)));
            }
        }
        if run.len() > 1 {
            p.add(egui::Shape::line(run, Stroke::new(width, colour)));
        }
    };
    line(0, al(Color32::WHITE, 0.45), 1.5);
    line(1, pal.accent, 2.2);
    p.text(pos2(plot.left() + 4.0, plot.top() + 6.0), Align2::LEFT_TOP, "sung", regular(TINY_SIZE), al(Color32::WHITE, 0.6));
    p.text(pos2(plot.left() + 36.0, plot.top() + 6.0), Align2::LEFT_TOP, "corrected", regular(TINY_SIZE), pal.accent);
    // The current note and how far off it was sung.
    let note = Rect::from_min_max(pos2(plot.right() + 12.0, plot.top()), pos2(panel.right() - 12.0, plot.bottom()));
    p.rect_filled(note, 8.0, al(Color32::BLACK, 0.18));
    if target.is_finite() && sung.is_finite() {
        let off = sung - target;
        p.text(note.center() - vec2(0.0, 22.0), Align2::CENTER_CENTER, note_name(target, mask), regular(38.0), pal.text_bright);
        let col = if off.abs() < 10.0 { pal.accent } else { pal.text_dim };
        p.text(note.center() + vec2(0.0, 22.0), Align2::CENTER_CENTER, format!("sung {off:+.0} c"), regular(LABEL_SIZE), col);
        let bar = Rect::from_center_size(note.center() + vec2(0.0, 46.0), vec2(note.width() - 30.0, 4.0));
        p.rect_filled(bar, 2.0, al(Color32::WHITE, 0.08));
        let x = bar.center().x + (off / 50.0).clamp(-1.0, 1.0) * bar.width() / 2.0;
        p.circle_filled(pos2(x, bar.center().y), 4.0, col);
    } else {
        p.text(note.center(), Align2::CENTER_CENTER, "—", regular(30.0), pal.text_faint);
        p.text(note.center() + vec2(0.0, 30.0), Align2::CENTER_CENTER, "play a voice", regular(CAPTION_SIZE), pal.text_faint);
    }
}

fn controls_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, row: Rect) {
    let pal = &PALETTE;
    let widths = [row.width() * 0.62 - GAP / 2.0, row.width() * 0.38 - GAP / 2.0];
    let left = Rect::from_min_size(row.min, vec2(widths[0], row.height()));
    let right = Rect::from_min_size(pos2(left.right() + GAP, row.top()), vec2(widths[1], row.height()));
    for (r, name) in [(left, "Correction"), (right, "Voice")] {
        paint_panel(ui.painter(), r, 10.0);
        group_label(ui.painter(), r, name);
    }
    let body = |r: &Rect| Rect::from_min_max(pos2(r.left() + 8.0, r.top() + GROUP_LABEL_H), pos2(r.right() - 8.0, r.bottom() - 8.0));
    let lb = body(&left);
    let dials = Rect::from_min_max(lb.min, pos2(lb.right() - 104.0, lb.bottom()));
    dial_row(app, ui, host, &mut st.fam, dials, &[("retune", "Retune Speed"), ("humanize", "Humanize"), ("hold", "Note Hold")]);
    // Keep vibrato: a switch.
    let on = match STYLE_VALUES.get(style(host)) {
        Some(sv) => sv.2,
        None => param(host, "vibrato") > 0.5,
    };
    let pill = Rect::from_center_size(pos2(lb.right() - 52.0, lb.center().y - 6.0), vec2(96.0, pill_height(PillStyle::Small)));
    let resp = pill_ex(ui, pill, "ap-vibrato", pal, "Keep Vibrato", on, PillStyle::Small, plain());
    if resp.clicked() {
        family::push_undo(&mut st.fam, host, &FAMILY);
        end_gesture(app);
        set_params(app, host, &with_custom(host, "vibrato", if on { 0.0 } else { 1.0 }));
    }
    let _ = resp.on_hover_text(tip_of("vibrato"));
    let rb = body(&right);
    if live_mode(host) {
        // Live: the voice range in place of Formant (which Live does not apply).
        let half = Rect::from_min_size(rb.min, vec2(rb.width() / 2.0, rb.height()));
        let seg = Rect::from_center_size(half.center() - vec2(0.0, 6.0), vec2(half.width() - 16.0, 66.0));
        let sel = param(host, "range").round().max(0.0) as usize;
        if let Some(i) = segmented_ex(ui, seg, "ap-range", pal, &["Low", "Middle", "High"], 3, Some(sel), SelectionStyle::Filled, false) {
            family::push_undo(&mut st.fam, host, &FAMILY);
            end_gesture(app);
            set_params(app, host, &[("range", i as f32)]);
        }
        tip(ui, segmented_id(ui, "ap-range"), RANGE_TIP);
        ui.painter().text(pos2(half.center().x, seg.bottom() + 10.0), Align2::CENTER_CENTER, "Voice Range", regular(LABEL_SIZE), pal.text_dim);
        let rest = Rect::from_min_max(pos2(half.right(), rb.top()), rb.max);
        dial_row(app, ui, host, &mut st.fam, rest, &[("mix", "Mix")]);
    } else {
        dial_row(app, ui, host, &mut st.fam, rb, &[("formant", "Formant"), ("mix", "Mix")]);
    }
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
    let chars = Rect::from_min_size(pos2(b.left(), header.bottom() + GAP), vec2(b.width(), CHAR_H));
    let key = Rect::from_min_size(pos2(b.left(), chars.bottom() + GAP), vec2(b.width(), KEY_H));
    let display = Rect::from_min_size(pos2(b.left(), key.bottom() + GAP), vec2(b.width(), DISPLAY_H));
    let controls = Rect::from_min_size(pos2(b.left(), display.bottom() + GAP), vec2(b.width(), CONTROLS_H));
    header_ui(st, app, ui, host, header);
    character_ui(st, app, ui, host, chars);
    key_ui(st, app, ui, host, key);
    display_ui(st, ui, host, display);
    controls_ui(st, app, ui, host, controls);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_style_control_makes_it_custom_in_the_same_step() {
        // Under Hard tune, Humanize up: the Style's other values are kept and the Style is Custom.
        let list = {
            let info = soundcraft_dsp::plugin_info("autopitch").unwrap();
            let mut ins = soundcraft_model::Insert::new("autopitch");
            ins.params.insert("style".into(), STYLE_HARD as f32);
            let host = Host {
                track: soundcraft_model::TrackId(1),
                ins: &ins,
                info,
                meters: None,
                sr: 48_000.0,
                rect: Rect::from_min_size(pos2(0.0, 0.0), SIZE),
                session: std::sync::Arc::new(soundcraft_model::Session::default()),
            };
            with_custom(&host, "humanize", 30.0)
        };
        assert!(list.contains(&("style", STYLE_CUSTOM as f32)) && list.contains(&("humanize", 30.0)));
        assert!(list.contains(&("retune", 0.0)) && list.contains(&("vibrato", 0.0)) && list.contains(&("hold", STYLE_VALUES[1].3)));
        assert_eq!(list.iter().filter(|(k, _)| *k == "humanize").count(), 1);
        assert_eq!(note_name(6600.0, rotate(SCALES[0].1, 6)), "F#4");
        assert_eq!(note_name(6800.0, rotate(SCALES[0].1, 6)), "G#4");
        assert_eq!(note_name(6800.0, SCALES[0].1), "Ab4");
    }

    #[test]
    fn every_control_has_a_tooltip() {
        let info = soundcraft_dsp::plugin_info("autopitch").unwrap();
        for p in info.params {
            assert!(!tip_of(p.id).is_empty() || p.id.starts_with("note"), "{}", p.id);
        }
    }
}

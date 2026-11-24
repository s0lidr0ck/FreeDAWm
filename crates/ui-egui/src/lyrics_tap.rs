//! Tap to time (in the Lyrics window): play the song and press J once per word (hold it for the
//! word's length: key down at its start, key up at its end). The word to tap is shown large with
//! the next ones after it. Backspace takes the last tap back, R starts the line again from just
//! before it, H plays at half speed, Esc leaves. Each finished line becomes one undo step
//! (`lyrics.tap_words`: taps less the latency, snapped to the nearest note start unless the snap
//! is off). In Line Starts mode one tap per line places the lines on the Lyrics ruler and spreads
//! their words over the notes (`lyrics.tap_lines`).

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, Key, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_time::Samples;

/// The key tapped per word or line.
pub const TAP_KEY: Key = Key::J;
/// A key held at least this long (seconds) gives the word its end.
const HOLD_SECS: f64 = 0.12;

/// What a tap marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TapMode {
    /// One tap per word.
    #[default]
    Words,
    /// One tap per line start; the words are spread inside each line.
    Lines,
}

/// Tap mode's state.
#[derive(Debug, Clone, PartialEq)]
pub struct TapState {
    pub on: bool,
    pub mode: TapMode,
    pub track: Option<TrackId>,
    /// The line being tapped (index into the sheet's lyric lines, in order).
    pub line: usize,
    /// This line's taps: key down, and key up when held.
    pub taps: Vec<(Samples, Option<Samples>)>,
    /// The key is down (since this transport time, and egui time).
    pub down: Option<(Samples, f64)>,
    /// Line Starts mode: the lines tapped so far.
    pub line_taps: Vec<(u64, Samples)>,
    /// Taken off every tap (ms): how late a tap lands after the sound.
    pub latency_ms: f32,
    /// Snap window (ms); 0 keeps the raw taps.
    pub snap_ms: f32,
    pub half_speed: bool,
    /// What the last line's commit said.
    pub last: String,
}

impl Default for TapState {
    fn default() -> TapState {
        TapState {
            on: false,
            mode: TapMode::Words,
            track: None,
            line: 0,
            taps: Vec::new(),
            down: None,
            line_taps: Vec::new(),
            latency_ms: 30.0,
            snap_ms: 60.0,
            half_speed: false,
            last: String::new(),
        }
    }
}

/// The sheet's lyric lines in order: (id, number, words).
pub fn lines(app: &SoundApp) -> Vec<(u64, u32, Vec<String>)> {
    let s = app.engine.session();
    let sheet = &s.lyric_sheet;
    sheet
        .lines
        .iter()
        .zip(sheet.kinds())
        .filter_map(|(l, k)| match k {
            soundcraft_model::LineKind::Lyric(n) => Some((l.id, n, soundcraft_engine::word_time::sheet_words(s, l.id))),
            _ => None,
        })
        .collect()
}

/// Turns tap mode on (`lyrics.tap` / AI › Tap Words to Time…): on the timing track, from the line
/// at the playhead (else the first), the Lyrics window shown.
pub fn start(app: &mut SoundApp, mode: TapMode) -> Result<(), String> {
    let all = lines(app);
    if all.is_empty() {
        return Err("Type the lyrics first (Window › Transport & Session › Lyrics)".into());
    }
    let track = soundcraft_engine::cmd::lyric_timing::timing_track(&app.engine, "lyrics.tap", &json!({})).map_err(|e| e.to_string())?;
    let s = app.engine.session();
    let at = s.edit.selection.start;
    let here = s.lyric_sheet.placed_at(at).and_then(|p| all.iter().position(|(id, _, _)| *id == p.line)).unwrap_or(0);
    let tap = &mut app.words.tap;
    let keep = (tap.latency_ms, tap.snap_ms);
    *tap = TapState { on: true, mode, track: Some(track), line: here, latency_ms: keep.0, snap_ms: keep.1, ..TapState::default() };
    app.ui.show_lyrics = true;
    Ok(())
}

/// Leaves tap mode, keeping what was tapped (a part-tapped line keeps its first words).
pub fn stop(app: &mut SoundApp) {
    commit_line(app);
    commit_lines(app);
    if app.words.tap.half_speed {
        let _ = app.run("transport.varispeed", json!({}));
    }
    app.words.tap.on = false;
}

/// Sends this line's taps (one undo step) and moves to the next line.
fn commit_line(app: &mut SoundApp) {
    let tap = app.words.tap.clone();
    if tap.mode != TapMode::Words || tap.taps.is_empty() {
        return;
    }
    let Some((id, n, words)) = lines(app).get(tap.line).cloned() else { return };
    let taps: Vec<serde_json::Value> = tap.taps.iter().map(|(d, u)| u.map_or(json!([d]), |u| json!([d, u]))).collect();
    let p = json!({"track": tap.track.map(|t| t.0), "line": id, "taps": taps, "latency_ms": tap.latency_ms, "snap_ms": tap.snap_ms});
    app.words.tap.last = match app.run("lyrics.tap_words", p) {
        Ok(v) => format!("Line {n}: {} of {} words timed", v["words"], words.len()),
        Err(e) => e,
    };
    app.words.tap.taps.clear();
    app.words.tap.line += 1;
}

/// Line Starts mode: places the tapped lines and spreads their words.
fn commit_lines(app: &mut SoundApp) {
    let tap = app.words.tap.clone();
    if tap.mode != TapMode::Lines || tap.line_taps.is_empty() {
        return;
    }
    let taps: Vec<serde_json::Value> = tap.line_taps.iter().map(|(l, at)| json!({"line": l, "at": at})).collect();
    let p = json!({"track": tap.track.map(|t| t.0), "taps": taps, "latency_ms": tap.latency_ms, "snap_ms": tap.snap_ms});
    app.words.tap.last = match app.run("lyrics.tap_lines", p) {
        Ok(v) => format!("{} lines placed; {} words guessed", v["placed"].as_array().map_or(0, Vec::len), v["guessed"]),
        Err(e) => e,
    };
    app.words.tap.line_taps.clear();
}

/// Plays from two seconds before the current line (its marker, when placed).
pub fn play_from_line(app: &mut SoundApp) {
    let Some((id, _, _)) = lines(app).get(app.words.tap.line).cloned() else { return };
    let from = app.engine.session().lyric_sheet.place_of(id).map(|p| p.start);
    let sr = app.engine.session().sample_rate.as_f64();
    let from = from.map_or_else(|| app.position(), |a| (a - soundcraft_time::to_samples(2.0 * sr)).max(0));
    let _ = app.run("transport.play", json!({"from": from}));
}

/// A tap: key down at `at` (transport time).
pub fn tap_down(app: &mut SoundApp, at: Samples, now: f64) {
    let all = lines(app);
    let tap = &mut app.words.tap;
    let Some((id, _, words)) = all.get(tap.line) else { return };
    match tap.mode {
        TapMode::Words => {
            if tap.taps.len() < words.len() {
                tap.taps.push((at, None));
                tap.down = Some((at, now));
            }
        }
        TapMode::Lines => {
            tap.line_taps.push((*id, at));
            tap.line += 1;
        }
    }
}

/// The tap key let go at `at`: a hold gives the word its end; the line's last word sends it.
pub fn tap_up(app: &mut SoundApp, at: Samples, now: f64) {
    let all = lines(app);
    let tap = &mut app.words.tap;
    if let Some((_, since)) = tap.down.take()
        && now - since >= HOLD_SECS
        && let Some(last) = tap.taps.last_mut()
    {
        last.1 = Some(at.max(last.0 + 1));
    }
    let n = all.get(tap.line).map_or(0, |l| l.2.len());
    if tap.mode == TapMode::Words && n > 0 && tap.taps.len() >= n {
        commit_line(app);
    }
}

/// Backspace: the last tap back.
pub fn back(app: &mut SoundApp) {
    let tap = &mut app.words.tap;
    match tap.mode {
        TapMode::Words => {
            tap.taps.pop();
        }
        TapMode::Lines => {
            if tap.line_taps.pop().is_some() {
                tap.line = tap.line.saturating_sub(1);
            }
        }
    }
}

/// R: this line again from just before it.
pub fn redo_line(app: &mut SoundApp) {
    app.words.tap.taps.clear();
    play_from_line(app);
}

/// H: half speed on or off.
pub fn toggle_half(app: &mut SoundApp) {
    let half = !app.words.tap.half_speed;
    app.words.tap.half_speed = half;
    let _ = app.run("transport.varispeed", if half { json!({"speed": 0.5}) } else { json!({}) });
}

/// Takes the tap keys out of this frame's input while tap mode is on (before the shortcuts see
/// them) and acts on them. A text field with the keyboard keeps its keys.
pub fn take_keys(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.words.tap.on || ctx.egui_wants_keyboard_input() {
        return;
    }
    let keys = [TAP_KEY, Key::Backspace, Key::R, Key::H, Key::Escape];
    let events: Vec<(Key, bool, bool)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed, repeat, modifiers, .. } if keys.contains(key) && modifiers.is_none() => {
                    Some((*key, *pressed, *repeat))
                }
                _ => None,
            })
            .collect()
    });
    if events.is_empty() {
        return;
    }
    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { key, modifiers, .. } if keys.contains(key) && modifiers.is_none())));
    let now = ctx.input(|i| i.time);
    for (key, pressed, repeat) in events {
        if repeat {
            continue;
        }
        let at = app.position();
        match (key, pressed) {
            (k, true) if k == TAP_KEY => tap_down(app, at, now),
            (k, false) if k == TAP_KEY => tap_up(app, at, now),
            (Key::Backspace, true) => back(app),
            (Key::R, true) => redo_line(app),
            (Key::H, true) => toggle_half(app),
            (Key::Escape, true) => stop(app),
            _ => {}
        }
    }
}

fn store<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, name: &str, v: T) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(format!("soundcraft-tap-{name}")), v));
}

/// The tap panel (the Lyrics window while tap mode is on).
pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let all = lines(app);
    let tap = app.words.tap.clone();
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(egui::RichText::new("TAP TO TIME").font(bold(13.0)).color(t.header_text));
        ui.add_space(12.0);
        let mut mode = tap.mode;
        ui.selectable_value(&mut mode, TapMode::Words, "Words").on_hover_text("Tap J once per word (hold it for the word's length)");
        ui.selectable_value(&mut mode, TapMode::Lines, "Line starts")
            .on_hover_text("Tap J at the start of each line: its words are spread over the notes inside");
        if mode != tap.mode {
            commit_line(app);
            commit_lines(app);
            app.words.tap.mode = mode;
        }
        ui.add_space(12.0);
        ui.label(egui::RichText::new("snap").color(t.text_dim));
        ui.add(egui::Slider::new(&mut app.words.tap.snap_ms, 0.0..=150.0).suffix(" ms").max_decimals(0))
            .on_hover_text("Each tap moves to the nearest note start within this; 0: the raw taps");
        ui.label(egui::RichText::new("latency").color(t.text_dim));
        ui.add(egui::Slider::new(&mut app.words.tap.latency_ms, 0.0..=250.0).suffix(" ms").max_decimals(0))
            .on_hover_text("Taken off every tap: raise it if the words come out late");
        let half = ui.selectable_label(tap.half_speed, "½ speed").on_hover_text("Play at half speed (H)");
        if half.clicked() {
            toggle_half(app);
        }
        if ui.button("Done").on_hover_text("Leave tap mode (Esc)").clicked() {
            stop(app);
        }
    });
    ui.add_space(6.0);
    let rect = ui.available_rect_before_wrap();
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.panel_bg2);
    store(ui.ctx(), "rect", rect);
    let Some((_, number, words)) = all.get(tap.line).cloned() else {
        p.text(rect.center(), Align2::CENTER_CENTER, "Every line is tapped. Done leaves tap mode.", regular(16.0), t.text_dim);
        store(ui.ctx(), "word", String::new());
        return;
    };
    let cur = tap.taps.len();
    let (big, line_font) = (bold(44.0), regular(18.0));
    let x0 = rect.left() + 24.0;
    let mut y = rect.top() + 18.0;
    let total = all.len();
    let status = match tap.mode {
        TapMode::Words => format!("Line {number} of {total} \u{b7} word {} of {}", (cur + 1).min(words.len()), words.len()),
        TapMode::Lines => format!("Line {number} of {total}: tap J where it starts"),
    };
    p.text(pos2(x0, y), Align2::LEFT_TOP, &status, regular(12.5), t.text_dim);
    y += 26.0;
    // The line, tapped words dim, the current one lit.
    let mut x = x0;
    for (i, w) in words.iter().enumerate() {
        let lit = tap.mode == TapMode::Words && i == cur;
        let colour = if i < cur && tap.mode == TapMode::Words {
            t.text_dim
        } else if lit {
            Color32::from_rgb(242, 196, 64)
        } else {
            t.text
        };
        let r = p.text(pos2(x, y), Align2::LEFT_TOP, w, line_font.clone(), colour);
        x = r.right() + 10.0;
    }
    y += 40.0;
    // The word to tap, large, with the next few.
    let word = match tap.mode {
        TapMode::Words => words.get(cur).cloned().unwrap_or_default(),
        TapMode::Lines => words.join(" "),
    };
    let r = p.text(pos2(x0, y), Align2::LEFT_TOP, &word, big, Color32::from_rgb(242, 196, 64));
    store(ui.ctx(), "word", word.clone());
    if tap.mode == TapMode::Words {
        let next: Vec<String> = words.iter().skip(cur + 1).take(4).cloned().collect();
        let next =
            if next.is_empty() { all.get(tap.line + 1).map(|l| format!("\u{2192} {}", l.2.join(" "))).unwrap_or_default() } else { next.join("  ") };
        p.text(pos2(r.right() + 18.0, r.center().y), Align2::LEFT_CENTER, &next, regular(20.0), t.text_dim);
    }
    y = r.bottom() + 20.0;
    if let Some((_, n, ws)) = all.get(tap.line + 1) {
        p.text(pos2(x0, y), Align2::LEFT_TOP, format!("next: {n}. {}", ws.join(" ")), regular(14.0), t.text_dim);
        y += 24.0;
    }
    let held = tap.down.is_some();
    let help = "J: tap (hold for the word's length)   Backspace: take back   R: this line again   H: half speed   Esc: done";
    p.text(pos2(x0, y + 8.0), Align2::LEFT_TOP, help, regular(12.0), t.text_dim);
    if !tap.last.is_empty() {
        p.text(pos2(x0, y + 30.0), Align2::LEFT_TOP, &tap.last, regular(12.0), t.accent);
    }
    // Buttons for the mouse.
    let br = egui::Rect::from_min_size(pos2(x0, y + 58.0), vec2(360.0, 30.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(br));
    child.horizontal(|ui| {
        let b = ui.add(egui::Button::new(if held { "Tapping\u{2026}" } else { "Tap (J)" }).min_size(vec2(90.0, 26.0)).sense(egui::Sense::drag()));
        let now = ui.input(|i| i.time);
        if b.drag_started() {
            let at = app.position();
            tap_down(app, at, now);
        }
        if b.drag_stopped() {
            let at = app.position();
            tap_up(app, at, now);
        }
        if ui.button("Play from line").clicked() {
            play_from_line(app);
        }
        if ui.button("\u{2190} Back").clicked() {
            back(app);
        }
        if ui.button("Line \u{2191}").on_hover_text("The line before (this line's taps are dropped)").clicked() {
            app.words.tap.taps.clear();
            app.words.tap.line = app.words.tap.line.saturating_sub(1);
        }
        if ui.button("Line \u{2193}").on_hover_text("The next line (this line's taps so far are kept)").clicked() {
            if app.words.tap.mode == TapMode::Words && !app.words.tap.taps.is_empty() {
                commit_line(app);
            } else {
                app.words.tap.line = (app.words.tap.line + 1).min(all.len());
            }
        }
    });
}

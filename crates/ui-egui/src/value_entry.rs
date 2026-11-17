//! Typing a value into a control. Ctrl+click (Cmd+click on a Mac) on a plug-in knob or dial opens
//! a box holding its value as shown; Enter sets what was typed, Escape or a click elsewhere leaves
//! the control as it was.
//!
//! What may be typed ([`parse`]): the number as the control shows it, with or without its unit
//! ("-6", "-6 dB", "2.5k" or "2.5 kHz", "40 %"), a choice's name, On / Off for a switch. A control
//! that shows its value in its own terms (a FireStation knob reading 0 to 10, or "+3.0 dB" for a
//! 0..1 travel) takes that number: the travel is found that shows it.

use egui::{Id, Rect, Ui, vec2};
use soundcraft_dsp::{ParamInfo, Unit};

/// The first number in `text`, with a `k` right after it (or after a space) as thousands, and
/// "-inf" as minus infinity. `None`: no number.
fn number(text: &str) -> Option<f32> {
    let t = text.trim().to_lowercase();
    if t.starts_with("-inf") || t.starts_with("−inf") {
        return Some(f32::NEG_INFINITY);
    }
    let t = t.replace('−', "-").replace(',', ".");
    let start = t.find(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.')?;
    let rest = t.get(start..)?;
    let end =
        rest.char_indices().find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || ((*c == '-' || *c == '+') && *i == 0))).map_or(rest.len(), |(i, _)| i);
    let v: f32 = rest.get(..end)?.parse().ok()?;
    let after = rest.get(end..).unwrap_or("").trim_start();
    let v = if after.starts_with('k') { v * 1000.0 } else { v };
    v.is_finite().then_some(v)
}

/// What a control's shown text reads as a number (see [`number`]); "Off" and other words read as
/// minus infinity, below every number (a knob whose bottom end reads "Off").
fn shown_number(info: &ParamInfo, t01: f32) -> f32 {
    number(&info.format(info.from_normalized(t01))).unwrap_or(f32::NEG_INFINITY)
}

/// The value for `text` typed into a control for `info` (clamped into its range), or `None` when
/// it does not read as one.
pub fn parse(info: &ParamInfo, text: &str) -> Option<f32> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    match info.unit {
        Unit::Choice => {
            let lower = t.to_lowercase();
            let exact = info.choices.iter().position(|c| c.to_lowercase() == lower);
            let prefix = || info.choices.iter().position(|c| c.to_lowercase().starts_with(&lower));
            return exact.or_else(prefix).map(|i| info.clamp(i as f32));
        }
        Unit::Toggle => {
            let on = match t.to_lowercase().as_str() {
                "on" | "1" | "yes" | "true" => true,
                "off" | "0" | "no" | "false" => false,
                _ => return None,
            };
            return Some(if on { info.max } else { info.min });
        }
        _ => {}
    }
    let typed = number(t)?;
    if info.display.is_none() {
        return Some(info.clamp(typed.max(info.min)));
    }
    // A control shown in its own terms: the travel that shows the typed number.
    let (lo_v, hi_v) = (shown_number(info, 0.0), shown_number(info, 1.0));
    if lo_v == hi_v {
        return None;
    }
    let rising = hi_v > lo_v;
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if (shown_number(info, mid) < typed) == rising {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(info.clamp(info.from_normalized(0.5 * (lo + hi))))
}

/// Whether this click on a control asks to type its value (Ctrl+click, Cmd+click on a Mac).
pub fn wants_entry(ui: &Ui, resp: &egui::Response) -> bool {
    resp.clicked() && ui.input(|i| i.modifiers.command)
}

#[derive(Clone, Default)]
struct Entry {
    text: String,
    fresh: bool,
}

fn key(id: Id) -> Id {
    id.with("value-entry")
}

/// Opens the box for the control `id` holding `shown` (its value as it reads).
pub fn open(ui: &Ui, id: Id, shown: &str) {
    ui.data_mut(|d| d.insert_temp(key(id), Entry { text: shown.to_string(), fresh: true }));
}

/// Whether the box for `id` is open.
pub fn is_open(ui: &Ui, id: Id) -> bool {
    ui.data(|d| d.get_temp::<Entry>(key(id)).is_some())
}

/// Draws the box for control `id` over `rect` while it is open. Returns the text when Enter
/// sets it (the box closes); Escape or a click elsewhere closes it with nothing.
pub fn show(ui: &Ui, id: Id, rect: Rect) -> Option<String> {
    let mut entry = ui.data(|d| d.get_temp::<Entry>(key(id)))?;
    // The box is a layer of its own: place it where the control is on screen (an editor drawn
    // scaled is in a transformed layer).
    let rect = ui.ctx().layer_transform_to_global(ui.layer_id()).map_or(rect, |t| t * rect);
    let w = rect.width().max(64.0);
    let at = egui::pos2(rect.center().x - w * 0.5, rect.center().y - 11.0);
    let mut done: Option<Option<String>> = None;
    egui::Area::new(key(id).with("area")).order(egui::Order::Foreground).fixed_pos(at).show(ui.ctx(), |ui| {
        let resp = ui.add_sized(vec2(w, 22.0), egui::TextEdit::singleline(&mut entry.text).horizontal_align(egui::Align::Center));
        if entry.fresh {
            resp.request_focus();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(entry.text.chars().count()));
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), resp.id);
            }
            entry.fresh = false;
        } else if resp.lost_focus() {
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            done = Some(enter.then(|| entry.text.clone()));
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            done = Some(None);
        }
    });
    match done {
        Some(out) => {
            ui.data_mut(|d| d.remove::<Entry>(key(id)));
            out
        }
        None => {
            ui.data_mut(|d| d.insert_temp(key(id), entry));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, param: &str) -> &'static ParamInfo {
        soundcraft_dsp::plugin_info(id).and_then(|p| p.param(param)).unwrap()
    }

    #[test]
    fn numbers_read_with_or_without_their_unit() {
        assert_eq!(number("-6"), Some(-6.0));
        assert_eq!(number("-6.5 dB"), Some(-6.5));
        assert_eq!(number("2.5k"), Some(2500.0));
        assert_eq!(number("2.50 kHz"), Some(2500.0));
        assert_eq!(number("40 %"), Some(40.0));
        assert_eq!(number("+3,5"), Some(3.5));
        assert_eq!(number("-inf dB"), Some(f32::NEG_INFINITY));
        assert_eq!(number("loud"), None);
        assert_eq!(number(""), None);
    }

    #[test]
    fn a_typed_value_lands_on_the_control() {
        // A plain dB control: the number is the value, clamped.
        let gain = info("gain", "gain");
        assert_eq!(parse(gain, "-6 dB"), Some(-6.0));
        assert_eq!(parse(gain, "999"), Some(gain.max));
        assert_eq!(parse(gain, "abc"), None);
        // Hertz, typed in kilohertz; a plain 0..10 knob.
        assert_eq!(parse(info("sprawl", "split2"), "1.5k"), Some(1500.0));
        assert_eq!(parse(info("keel", "drive"), "7.5"), Some(7.5));
        // A choice by its name (or the start of it), a switch by On / Off.
        let voicing = info("keel", "voicing");
        assert_eq!(parse(voicing, "grind"), Some(2.0));
        assert_eq!(parse(voicing, "Solid"), Some(1.0));
        assert_eq!(parse(voicing, "nothing"), None);
        let bright = info("keel", "bright");
        assert_eq!(parse(bright, "on"), Some(1.0));
        assert_eq!(parse(bright, "Off"), Some(0.0));
    }

    #[test]
    fn a_knob_shown_in_its_own_terms_takes_the_shown_number() {
        // FireStation Amp's knobs travel 0..1 and read 0..10, its trims read in dB.
        let volume = info("fs_amp", "volume");
        let v = parse(volume, "7").unwrap();
        assert!((volume.format(v).parse::<f32>().unwrap() - 7.0).abs() < 0.06, "{}", volume.format(v));
        let trim = info("fs_amp", "input");
        let v = parse(trim, "+3 dB").unwrap();
        let shown = number(&trim.format(v)).unwrap();
        assert!((shown - 3.0).abs() < 0.06, "{}", trim.format(v));
        // past the end: the end
        assert_eq!(parse(volume, "99"), Some(volume.max));
    }
}

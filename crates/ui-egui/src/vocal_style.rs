//! How vocal notes look, shared by the Vocal Editor and the track lane (plan 4): the colour of a
//! note's offset from its target (teal on pitch, through amber, to red at 48 c and beyond), its
//! thickness (loudness), and the cents and note-name labels.

use egui::Color32;

/// The note colour on pitch (the mockup's teal; MIDI notes use their track colour, so the app
/// has no teal of its own).
pub const NOTE_TEAL: Color32 = Color32::from_rgb(63, 208, 176);

/// The colour stops: cents off target and the colour there (linear between them).
const STOPS: [(f32, [u8; 3]); 5] =
    [(0.0, [63, 208, 176]), (10.0, [96, 205, 140]), (22.0, [226, 196, 72]), (34.0, [240, 138, 58]), (48.0, [236, 72, 72])];

/// The colour of a note `cents` off its target (sign ignored, clamped at 48 c); non-finite
/// values read as on pitch.
pub fn off_colour(cents: f32) -> Color32 {
    if !cents.is_finite() {
        return NOTE_TEAL;
    }
    let c = cents.abs().min(48.0);
    let mut prev = STOPS[0];
    for stop in STOPS.iter().skip(1) {
        if c <= stop.0 {
            let f = (c - prev.0) / (stop.0 - prev.0);
            let mix = |i: usize| {
                let (a, b) = (f32::from(prev.1.get(i).copied().unwrap_or(0)), f32::from(stop.1.get(i).copied().unwrap_or(0)));
                (a + (b - a) * f).round().clamp(0.0, 255.0) as u8
            };
            return Color32::from_rgb(mix(0), mix(1), mix(2));
        }
        prev = *stop;
    }
    Color32::from_rgb(236, 72, 72)
}

/// A note bar's height: 6 px for a note 10 dB or more under `typical_db`, 28 px at 6 dB or more
/// over it (linear between), at the default 20 px row; scaled by `row_h / 20`.
pub fn note_thickness(level_db: f32, typical_db: f32, row_h: f32) -> f32 {
    let k = ((level_db - typical_db + 10.0) / 16.0).clamp(0.0, 1.0);
    let k = if k.is_finite() { k } else { 0.0 };
    (6.0 + 22.0 * k) * (row_h / 20.0)
}

/// Cents as the roll labels them: "+7c", "−22c" (U+2212), "±0c".
pub fn fmt_cents(c: f32) -> String {
    let r = if c.is_finite() { c.round() } else { 0.0 };
    if r > 0.0 {
        format!("+{r:.0}c")
    } else if r < 0.0 {
        format!("\u{2212}{:.0}c", -r)
    } else {
        "\u{b1}0c".into()
    }
}

const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The name of the semitone nearest `cents` (MIDI × 100; 6900 is A4): "G#4".
pub fn note_name(cents: f32) -> String {
    let m = if cents.is_finite() { (cents / 100.0).round().clamp(-1000.0, 1000.0) as i32 } else { 69 };
    let name = NAMES.get(m.rem_euclid(12) as usize).copied().unwrap_or("C");
    format!("{name}{}", m.div_euclid(12) - 1)
}

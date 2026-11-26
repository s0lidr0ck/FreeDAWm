//! Colour strips: a Mix window strip's background, its section boxes (inserts, sends, I/O) and its
//! fader well take on the track's colour, so groups of instruments read at a glance
//! (`UiState::mix_strip_tint`, the Mix window's Colour Off / Subtle / Medium / Strong).
//!
//! The colour is blended into the theme's own strip colours, then darkened (dark theme) or
//! lightened (light theme) only as far as the strip's text needs: every label colour drawn on a
//! tinted surface keeps at least [`MIN_CONTRAST`] (WCAG AA for small text) on it, whatever the
//! track colour. Yellow and light greens are taken darker in the dark theme and paler in the light
//! one; deep blues and reds keep most of their colour. Meters, fader caps, keys and readouts keep
//! their own fills, so their colours read as before. Grey tracks stay neutral. At Off nothing is
//! drawn differently.

use crate::theme::Tokens;
use crate::widgets::shade;
use egui::Color32;

/// The amounts the Mix window offers: (amount, label).
pub const STEPS: [(f32, &str); 4] = [(0.0, "Off"), (0.35, "Subtle"), (0.65, "Medium"), (1.0, "Strong")];

/// The least contrast between a label and the tinted surface under it.
pub const MIN_CONTRAST: f64 = 4.5;

/// How much of the track's colour a strip at full amount takes (the rest is the theme's).
const MAX_MIX: f32 = 0.85;

/// How much lighter the strip's top is than its bottom (its gradient, as untinted).
const TOP_LIFT: f32 = 0.06;

/// The step `amount` is at, if it is one of [`STEPS`].
pub fn step_of(amount: f32) -> Option<f32> {
    let a = clamp_amount(amount);
    STEPS.iter().map(|(s, _)| *s).find(|s| (s - a).abs() < 0.01)
}

/// `amount` as 0..1 (anything else, NaN included, as Off).
pub fn clamp_amount(amount: f32) -> f32 {
    if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 0.0 }
}

/// A grey track colour: no tint, the strip stays neutral.
pub fn is_neutral(color: [u8; 3]) -> bool {
    let [r, g, b] = color;
    r.max(g).max(b) - r.min(g).min(b) < 16
}

/// A tinted strip's colours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripLook {
    /// The strip's background (bottom; the top is [`StripLook::top`]).
    pub bg: Color32,
    pub top: Color32,
    /// The section boxes (PREAMP, inserts, sends, I/O) and the well behind the fader and meter.
    pub section: Color32,
    /// The selected strip's outline (a tint can take the selected strip's own lighter fill).
    pub outline: Color32,
}

/// The strip colours for a track of `color` at `amount`, with the current theme; `None` when the
/// strip is drawn as untinted (Off, or a grey track).
pub fn look(color: [u8; 3], amount: f32, selected: bool) -> Option<StripLook> {
    look_with(&Tokens::current(), color, amount, selected)
}

/// [`look`] with the palette `t`.
pub fn look_with(t: &Tokens, color: [u8; 3], amount: f32, selected: bool) -> Option<StripLook> {
    let k = clamp_amount(amount);
    if k <= 0.0 || is_neutral(color) {
        return None;
    }
    let c = saturated(color, k);
    let mix = k * MAX_MIX;
    let base = if selected { t.strip_selected } else { t.strip_bg };
    let bg = readable(t, base.lerp_to_gamma(c, mix), &[t.text, t.text_dim], TOP_LIFT);
    // The sections stay a step darker (dark theme) or lighter (light) than the strip, as untinted.
    let section = readable(t, t.strip_section.lerp_to_gamma(c, mix), &[t.section_label, t.text, t.text_dim], 0.0);
    let outline = if contrast(t.accent, bg) >= 3.0 { t.accent } else { t.text };
    Some(StripLook { bg, top: shade(bg, TOP_LIFT), section, outline })
}

/// The track colour with its saturation raised with the amount (a strong tint reads as colour,
/// not as a grey with a cast).
fn saturated([r, g, b]: [u8; 3], k: f32) -> Color32 {
    let (r, g, b) = (f32::from(r), f32::from(g), f32::from(b));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max <= 0.0 {
        return Color32::BLACK;
    }
    let s = (max - min) / max;
    let s2 = (s + (1.0 - s) * 0.35 * k).min(1.0);
    // Same hue and value, more saturation: each channel moves away from the maximum.
    let f = |c: f32| {
        let rel = if max - min > 0.0 { (max - c) / (max - min) } else { 0.0 };
        (max - rel * s2 * max).clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(f(r), f(g), f(b))
}

/// `bg` taken darker (light text) or lighter (dark text) until every one of `texts` has
/// [`MIN_CONTRAST`] on it, and on it lifted by `lift` (the strip's lighter top).
fn readable(t: &Tokens, mut bg: Color32, texts: &[Color32], lift: f32) -> Color32 {
    let light_text = luminance(t.text) > luminance(t.strip_bg);
    let ok = |c: Color32| texts.iter().all(|x| contrast(*x, c) >= MIN_CONTRAST);
    for _ in 0..80 {
        if ok(bg) && ok(shade(bg, lift)) {
            break;
        }
        bg = if light_text { shade(bg, -0.04) } else { shade(bg, 0.04) };
    }
    bg
}

/// WCAG relative luminance (sRGB).
pub fn luminance(c: Color32) -> f64 {
    let lin = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio of two colours (1..21).
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

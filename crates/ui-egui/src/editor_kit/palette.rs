//! `Common/Theme.h`: the palette every FireStation editor paints from, and `EngineLook::apply`.
//!
//! In the C++ the palette is a set of globals each engine overwrites in its editor's
//! constructor. Here each editor declares its own `const PALETTE: VstPalette`, so two editors open
//! at once never share colours, and the DAW theme never reaches inside an editor (no FireStation
//! editor has a light skin; the owner's rule is the VST's palette in both DAW themes).

use egui::Color32;

/// A JUCE colour literal, `0xAARRGGBB`.
pub const fn argb(v: u32) -> Color32 {
    let a = (v >> 24) as u8;
    let (r, g, b) = ((v >> 16) as u8, (v >> 8) as u8, v as u8);
    if a == 0xFF { Color32::from_rgb(r, g, b) } else { Color32::from_rgba_premultiplied(premul(r, a), premul(g, a), premul(b, a), a) }
}

const fn premul(c: u8, a: u8) -> u8 {
    ((c as u32 * a as u32 + 127) / 255) as u8
}

/// An opaque colour from `0xRRGGBB`.
pub const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// `Colour::withAlpha` / `withMultipliedAlpha` on an opaque colour: `c` at `a` (0..1) opacity.
pub fn al(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(if a.is_finite() { a.clamp(0.0, 1.0) } else { 0.0 })
}

/// `fs::theme`'s colours, plus Bypass's red (`fs::look::bypassRed`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VstPalette {
    pub accent: Color32,
    pub background: Color32,
    pub panel: Color32,
    pub panel_alt: Color32,
    pub border: Color32,
    pub border_hover: Color32,
    pub text_bright: Color32,
    pub text_body: Color32,
    pub text_dim: Color32,
    pub text_label: Color32,
    pub text_faint: Color32,
    pub on_accent: Color32,
    pub star: Color32,
    pub led_off: Color32,
    pub led_off_ring: Color32,
    pub knob_cap: Color32,
    /// `fs::look::bypassRed`: Bypass reads the same in every engine.
    pub bypass_red: Color32,
    /// The caution colour (CleanQ's `warn`; the lit state of Bypass-like toggles there).
    pub warn: Color32,
    /// Kit extension: a value box's fill (CleanQ's `NumField`); the family uses `panel_alt`.
    pub field: Color32,
    /// Kit extension: the darkest well (a slider's track, a text editor's ground).
    pub well: Color32,
}

/// `fs::look::bypassRed`.
pub const BYPASS_RED: Color32 = argb(0xFFE0574B);

impl VstPalette {
    /// `fs::theme::defaults` (DistEngine's).
    pub const THEME_DEFAULTS: VstPalette = VstPalette {
        accent: argb(0xFFFF7A2F),
        background: argb(0xFF0E1114),
        panel: argb(0xFF14181C),
        panel_alt: argb(0xFF13171B),
        border: argb(0xFF262C33),
        border_hover: argb(0xFF3A444F),
        text_bright: argb(0xFFDFE6EC),
        text_body: argb(0xFF9AA6B2),
        text_dim: argb(0xFF7D8894),
        text_label: argb(0xFF6F7B86),
        text_faint: argb(0xFF4D5763),
        on_accent: argb(0xFF0B0E11),
        star: argb(0xFFF2C94C),
        led_off: argb(0xFF1E2A22),
        led_off_ring: argb(0xFF2C3A30),
        knob_cap: argb(0xFF2F3136),
        bypass_red: BYPASS_RED,
        warn: argb(0xFFFFD166),
        field: argb(0xFF13171B),
        well: argb(0xFF0B0E12),
    };

    /// `fs::look::apply (accent)`: the family palette tinted by the engine's accent. The LED
    /// colours are not touched by `apply`, so they stay the defaults.
    pub const fn engine_look(accent: Color32) -> VstPalette {
        VstPalette {
            accent,
            background: argb(0xFF1C1D20),
            panel: argb(0xFF232428),
            panel_alt: argb(0xFF1F2023),
            border: argb(0xFF34353A),
            border_hover: argb(0xFF54555C),
            text_bright: argb(0xFFF4F1E8),
            text_body: argb(0xFF9A978E),
            text_dim: argb(0xFFCFCCC2),
            text_label: argb(0xFF9A978E),
            text_faint: argb(0xFF7D7B75),
            on_accent: argb(0xFF16171A),
            knob_cap: argb(0xFF2F3136),
            star: argb(0xFFF2C94C),
            field: argb(0xFF1F2023),
            well: argb(0xFF151618),
            ..Self::THEME_DEFAULTS
        }
    }

    /// `setAccent`.
    pub const fn with_accent(self, accent: Color32) -> VstPalette {
        VstPalette { accent, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn juce_colours_and_the_family_look() {
        assert_eq!(argb(0xFF5ED1FF), Color32::from_rgb(0x5E, 0xD1, 0xFF));
        assert_eq!(argb(0x80FFFFFF).a(), 0x80);
        let p = VstPalette::engine_look(rgb(0x123456));
        assert_eq!(p.accent, rgb(0x123456));
        assert_eq!(p.background, rgb(0x1C1D20));
        assert_eq!(p.led_off, VstPalette::THEME_DEFAULTS.led_off);
        assert_eq!(p.bypass_red, rgb(0xE0574B));
        assert_eq!(VstPalette::THEME_DEFAULTS.with_accent(rgb(1)).accent, rgb(1));
    }
}

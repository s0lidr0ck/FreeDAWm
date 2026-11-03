//! The owner's FreeDAWm bird and wordmark, drawn in the About box and as the menu bar's first item
//! (`assets/brand/`, made from `assets/app-icon/freedawm-*-source.png` by `packaging/icons.py`).
//!
//! The wordmark's "Free" and "m" are white, which vanishes on the light theme's bars: for the
//! light theme those letters (bright, unsaturated pixels) are tinted to the theme's text colour,
//! and "DAW" (the saturated gradient) keeps its colours. Both variants are decoded and made once,
//! the first time either is drawn, then kept in the context's data.

use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions};
use std::sync::Arc;

/// The accessible name of the menu bar's first item and the About box's heading.
pub const APP_NAME: &str = "FreeDAWm";

/// The public repository (a placeholder until the public repo exists; `docs/release-playbook.md`
/// lists every place that names it).
pub const REPOSITORY_URL: &str = "https://github.com/s0lidr0ck/FreeDAWm";

const BIRD_PNG: &[u8] = include_bytes!("../assets/brand/freedawm-bird-192.png");
const WORDMARK_PNG: &[u8] = include_bytes!("../assets/brand/freedawm-wordmark-720.png");

/// A pixel counts as part of the white letters when it is this bright (0..1, the brightest
/// channel)…
const WHITE_MIN_VALUE: f32 = 0.78;
/// …and this little saturated (0..1, (max - min) / max). The gradient's darkest blue is about 0.8.
const WHITE_MAX_SATURATION: f32 = 0.22;

/// The decoded art, uploaded once per context.
#[derive(Clone)]
pub struct BrandArt {
    pub bird: TextureHandle,
    /// The wordmark as supplied (dark theme).
    pub wordmark_dark: TextureHandle,
    /// The wordmark with its white letters in the light theme's text colour.
    pub wordmark_light: TextureHandle,
    /// Width over height of the wordmark.
    pub wordmark_aspect: f32,
}

impl BrandArt {
    /// The art for `ctx`, made the first time it is asked for. `None` when the embedded images
    /// can't be decoded (never expected; the caller falls back to text).
    pub fn get(ctx: &Context) -> Option<Arc<BrandArt>> {
        let id = egui::Id::new("freedawm-brand-art");
        if let Some(art) = ctx.data(|d| d.get_temp::<Arc<BrandArt>>(id)) {
            return Some(art);
        }
        let art = Arc::new(Self::load(ctx)?);
        ctx.data_mut(|d| d.insert_temp(id, Arc::clone(&art)));
        Some(art)
    }

    fn load(ctx: &Context) -> Option<BrandArt> {
        let options = TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
        let bird = decode(BIRD_PNG)?;
        let dark = decode(WORDMARK_PNG)?;
        let [w, h] = dark.size;
        let light = tint_white(&dark, crate::theme::Tokens::LIGHT.text);
        Some(BrandArt {
            bird: ctx.load_texture("freedawm-bird", bird, options),
            wordmark_dark: ctx.load_texture("freedawm-wordmark-dark", dark, options),
            wordmark_light: ctx.load_texture("freedawm-wordmark-light", light, options),
            wordmark_aspect: if h == 0 { 1.0 } else { w as f32 / h as f32 },
        })
    }

    /// The wordmark for the active theme.
    pub fn wordmark(&self) -> &TextureHandle {
        if crate::theme::is_light() { &self.wordmark_light } else { &self.wordmark_dark }
    }
}

fn decode(png: &[u8]) -> Option<ColorImage> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// True for a pixel of the white letters: bright and nearly grey.
pub fn is_white(c: Color32) -> bool {
    let [r, g, b, _] = c.to_srgba_unmultiplied().map(|v| f32::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    max >= WHITE_MIN_VALUE && (max - min) / max.max(f32::EPSILON) <= WHITE_MAX_SATURATION
}

/// `img` with every white-letter pixel ([`is_white`]) recoloured to `text`, its alpha kept (so the
/// letters' anti-aliased edges stay smooth); every other pixel is unchanged.
pub fn tint_white(img: &ColorImage, text: Color32) -> ColorImage {
    let [tr, tg, tb, _] = text.to_srgba_unmultiplied();
    let pixels = img
        .pixels
        .iter()
        .map(|&c| if c.a() > 0 && is_white(c) { Color32::from_rgba_unmultiplied(tr, tg, tb, c.to_srgba_unmultiplied()[3]) } else { c })
        .collect();
    ColorImage { size: img.size, source_size: img.source_size, pixels }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn white_letters_are_tinted_and_the_gradient_is_kept() {
        let white = Color32::from_rgba_unmultiplied(255, 255, 255, 255);
        let edge = Color32::from_rgba_unmultiplied(250, 250, 252, 90);
        let blue = Color32::from_rgb(40, 140, 255);
        let pink = Color32::from_rgb(230, 50, 150);
        let orange = Color32::from_rgb(250, 140, 60);
        let clear = Color32::TRANSPARENT;
        assert!(is_white(white) && is_white(edge));
        assert!(!is_white(blue) && !is_white(pink) && !is_white(orange));
        let img = ColorImage::new([6, 1], vec![white, edge, blue, pink, orange, clear]);
        let text = Color32::from_rgb(20, 22, 26);
        let out = tint_white(&img, text);
        assert_eq!(out.pixels[0].to_srgba_unmultiplied(), [20, 22, 26, 255]);
        // Colours are stored premultiplied: a translucent pixel reads back within one step.
        let near = |a: u8, b: u8| a.abs_diff(b) <= 1;
        let [r, g, b, _] = out.pixels[1].to_srgba_unmultiplied();
        assert!(near(r, 20) && near(g, 22) && near(b, 26), "an anti-aliased edge takes the colour: {r} {g} {b}");
        assert_eq!(out.pixels[1].a(), edge.a(), "and keeps its alpha");
        assert_eq!(&out.pixels[2..], &img.pixels[2..], "DAW's gradient and the background are untouched");
    }

    #[test]
    fn the_embedded_wordmark_has_both_kinds_of_pixel() {
        let img = decode(WORDMARK_PNG).unwrap();
        let opaque: Vec<Color32> = img.pixels.iter().copied().filter(|c| c.a() == 255).collect();
        let white = opaque.iter().filter(|c| is_white(**c)).count();
        assert!(white > opaque.len() / 10, "Free and m: {white} of {}", opaque.len());
        assert!(white < opaque.len() * 9 / 10, "DAW stays coloured");
        assert_eq!(decode(BIRD_PNG).unwrap().size, [192, 192]);
    }
}

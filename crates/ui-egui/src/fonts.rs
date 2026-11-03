//! Fonts: egui's bundled defaults, plus the operating system's own UI fonts loaded at runtime when
//! present (never redistributed). A "bold" family is always defined so the UI can use it, and a
//! "mono-bold" one (the system's bold monospace, else egui's monospace), and a "splash" one: the
//! bundled Inter Regular (`assets/fonts/`, OFL-1.1) for the launch splash's statement, so its look
//! does not depend on the system (the UI font is unchanged).

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

/// (path, ttc index) candidates for regular and bold UI text, by platform.
const REGULAR: &[(&str, u32)] = &[
    ("/System/Library/Fonts/HelveticaNeue.ttc", 0),
    ("/System/Library/Fonts/Helvetica.ttc", 0),
    ("C:\\Windows\\Fonts\\segoeui.ttf", 0),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/TTF/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/dejavu/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf", 0),
    ("/usr/local/share/fonts/dejavu/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf", 0),
    ("/usr/share/fonts/liberation-sans-fonts/LiberationSans-Regular.ttf", 0),
];
const BOLD: &[(&str, u32)] = &[
    ("/System/Library/Fonts/HelveticaNeue.ttc", 1),
    ("/System/Library/Fonts/Helvetica.ttc", 1),
    ("C:\\Windows\\Fonts\\segoeuib.ttf", 0),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/TTF/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf", 0),
    ("/usr/local/share/fonts/dejavu/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf", 0),
    ("/usr/share/fonts/liberation-sans-fonts/LiberationSans-Bold.ttf", 0),
];
/// Bold monospace (the VST editors' `Font::getDefaultMonospacedFontName()` in bold: ChamberMade's
/// readouts). Windows' default fixed face, Lucida Console, has no bold; Consolas Bold stands in.
const MONO_BOLD: &[(&str, u32)] = &[
    ("/System/Library/Fonts/Menlo.ttc", 1),
    ("C:\\Windows\\Fonts\\consolab.ttf", 0),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf", 0),
    ("/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf", 0),
    ("/usr/share/fonts/dejavu/DejaVuSansMono-Bold.ttf", 0),
    ("/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono-Bold.ttf", 0),
    ("/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf", 0),
];
/// Hangul fallback, appended after every family so Korean session, track and clip names draw
/// whatever the interface language. The Noto Sans CJK collection's face 1 is its Korean face.
const HANGUL: &[(&str, u32)] = &[
    ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
    ("/System/Library/Fonts/Supplemental/AppleGothic.ttf", 0),
    ("C:\\Windows\\Fonts\\malgun.ttf", 0),
    ("C:\\Windows\\Fonts\\gulim.ttc", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 1),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 1),
    ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 1),
    ("/usr/share/fonts/truetype/nanum/NanumGothic.ttf", 0),
    ("/usr/share/fonts/naver-nanum/NanumGothic.ttf", 0),
];
/// The launch splash's family name (Inter Regular, then the proportional fallbacks).
pub const SPLASH_FAMILY: &str = "splash";
/// The font data's name in the definitions.
const SPLASH_FONT: &str = "inter-regular";
/// Inter Regular 4.1 (rsms/inter release, `extras/ttf/`), OFL-1.1: `assets/fonts/Inter-LICENSE.txt`.
static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

/// The splash family's font, for a host that draws the splash before [`install`] runs
/// (`Context::add_font`; it takes effect at the next pass).
pub fn splash_font() -> egui::epaint::text::FontInsert {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    FontInsert::new(
        SPLASH_FONT,
        FontData::from_static(INTER_REGULAR),
        vec![InsertFontFamily { family: FontFamily::Name(SPLASH_FAMILY.into()), priority: FontPriority::Highest }],
    )
}

/// The splash family is bound in `ctx` (drawing text in an unbound family panics in egui). Only
/// after the context's first pass (egui has no fonts before it).
pub fn has_splash_family(ctx: &egui::Context) -> bool {
    let family = FontFamily::Name(SPLASH_FAMILY.into());
    ctx.fonts(|f| f.definitions().families.contains_key(&family))
}

/// Font files larger than this are skipped: egui keeps the whole file in memory.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FONT_BYTES: u64 = 64 << 20;

/// Asks fontconfig for the desktop's font matching `pattern`. Only TrueType/OpenType results are
/// used: egui cannot read Type 1 or bitmap fonts, which `fc-match` may return on minimal systems.
/// With `lang`, the match must support that language (`fc-match` always returns its best match,
/// which is a Latin face when no Korean font is installed).
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn load_from_fontconfig(pattern: &str, lang: Option<&str>) -> Option<FontData> {
    let output = std::process::Command::new("fc-match").args(["--format=%{file}\\n%{index}\\n%{lang}\\n", pattern]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut lines = stdout.lines();
    let path = lines.next()?.trim();
    // The upper 16 bits carry a variable font's named instance; the face index is the lower 16.
    let index = lines.next()?.trim().parse::<u32>().ok()? & 0xFFFF;
    if let Some(lang) = lang
        && !lines.next().unwrap_or_default().split('|').any(|l| l.trim() == lang)
    {
        return None;
    }
    if std::fs::metadata(path).ok()?.len() > MAX_FONT_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let sfnt = matches!(bytes.get(..4), Some(b"\0\x01\0\0" | b"OTTO" | b"true" | b"ttcf"));
    if !sfnt {
        return None;
    }
    let mut font = FontData::from_owned(bytes);
    font.index = index;
    Some(font)
}

#[cfg(not(target_arch = "wasm32"))]
fn load(cands: &[(&str, u32)], pattern: &str, lang: Option<&str>) -> Option<FontData> {
    if soundcraft_engine::brand::env_var_os("NO_SYSTEM_FONTS").is_some() {
        return None;
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    if let Some(font) = load_from_fontconfig(pattern, lang) {
        return Some(font);
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let _ = (pattern, lang);
    for (p, idx) in cands {
        if std::fs::metadata(p).is_ok_and(|m| m.len() > MAX_FONT_BYTES) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(p) {
            let mut fd = FontData::from_owned(bytes);
            fd.index = *idx;
            return Some(fd);
        }
    }
    None
}

#[cfg(target_arch = "wasm32")]
fn load(_: &[(&str, u32)], _: &str, _: Option<&str>) -> Option<FontData> {
    None
}

pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let base: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut bold_family = base.clone();
    if let Some(reg) = load(REGULAR, "sans-serif", None) {
        fonts.font_data.insert("system-regular".into(), Arc::new(reg));
        if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
            f.insert(0, "system-regular".into());
        }
    }
    if let Some(b) = load(BOLD, "sans-serif:style=Bold", None) {
        fonts.font_data.insert("system-bold".into(), Arc::new(b));
        bold_family.insert(0, "system-bold".into());
    } else if fonts.font_data.contains_key("system-regular") {
        bold_family.insert(0, "system-regular".into());
    }
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    fonts.font_data.insert(SPLASH_FONT.into(), Arc::new(FontData::from_static(INTER_REGULAR)));
    let mut splash_family = vec![SPLASH_FONT.to_string()];
    splash_family.extend(base.iter().cloned());
    fonts.families.insert(FontFamily::Name(SPLASH_FAMILY.into()), splash_family);
    let mut mono_bold: Vec<String> = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    if let Some(m) = load(MONO_BOLD, "monospace:style=Bold", None) {
        fonts.font_data.insert("system-mono-bold".into(), Arc::new(m));
        mono_bold.insert(0, "system-mono-bold".into());
    }
    fonts.families.insert(FontFamily::Name("mono-bold".into()), mono_bold);
    if let Some(h) = load(HANGUL, "sans-serif:lang=ko", Some("ko")) {
        fonts.font_data.insert("system-hangul".into(), Arc::new(h));
        for family in fonts.families.values_mut() {
            family.push("system-hangul".into());
        }
    }
    fonts
}

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hangul_candidates_cover_every_desktop_platform() {
        for prefix in ["/System/Library/Fonts/", "C:\\Windows\\Fonts\\", "/usr/share/fonts/"] {
            assert!(HANGUL.iter().any(|(p, _)| p.starts_with(prefix)), "no Hangul candidate under {prefix}");
        }
        // Face 0 of the Noto Sans CJK collection is Japanese; the Korean face is 1.
        assert!(HANGUL.iter().filter(|(p, _)| p.ends_with("NotoSansCJK-Regular.ttc")).all(|(_, i)| *i == 1));
    }

    /// The splash family is Inter first, with the UI's fallbacks after it; the UI font is not Inter.
    #[test]
    fn the_splash_family_is_the_bundled_inter() {
        let defs = definitions();
        let family = defs.families.get(&FontFamily::Name(SPLASH_FAMILY.into())).expect("splash family");
        assert_eq!(family.first().map(String::as_str), Some(SPLASH_FONT));
        assert!(family.len() > 1, "fallbacks after Inter");
        assert!(defs.families.get(&FontFamily::Proportional).is_some_and(|f| !f.iter().any(|n| n == SPLASH_FONT)), "UI font unchanged");
        assert!(INTER_REGULAR.len() > 100_000 && INTER_REGULAR.get(..4) == Some(&b"\0\x01\0\0"[..]), "a TrueType file");
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        assert!(!has_splash_family(&ctx));
        ctx.add_font(splash_font());
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        assert!(has_splash_family(&ctx), "add_font binds the family for a splash drawn before install");
        let font = egui::FontId::new(20.0, FontFamily::Name(SPLASH_FAMILY.into()));
        // Inter draws it (not the default font: the widths differ). `has_glyphs` can't tell for a
        // one-font family (its replacement face is that font).
        let width = |font: egui::FontId| ctx.fonts_mut(|f| "Creativity".chars().map(|c| f.glyph_width(&font, c)).sum::<f32>());
        let (inter, ubuntu) = (width(font), width(egui::FontId::proportional(20.0)));
        assert!(inter > 0.0 && (inter - ubuntu).abs() > 0.5, "{inter} vs {ubuntu}");
    }

    /// Korean names draw in every family, not as boxes; skipped when the system has no Korean font.
    #[test]
    fn every_family_draws_hangul_with_a_system_korean_font() {
        let defs = definitions();
        if !defs.font_data.contains_key("system-hangul") {
            eprintln!("skipping: no Korean system font");
            return;
        }
        let families: Vec<FontFamily> = defs.families.keys().cloned().collect();
        let ctx = egui::Context::default();
        ctx.set_fonts(defs);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        for family in families {
            let font = egui::FontId::new(13.0, family.clone());
            assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, "가한국어세션")), "{family:?} has no Hangul");
        }
    }
}

//! Window › Color Palette (Track › Color…): the 64-colour palette as a grid of 12 hues by 4
//! shades plus the 16 original colours, custom colours from a picker with the last 8 kept, and
//! the two tint amounts. A click colours the selected tracks, Shift+click the selected clips.
//! The selected tracks' colours are ringed. The palette itself is `soundcraft_model::palette`.

use crate::SoundApp;
use crate::theme::{Tokens, rgb};
use egui::{Color32, RichText, Sense, Stroke, StrokeKind, vec2};
use serde_json::json;
use soundcraft_model::TRACK_COLORS;
use soundcraft_model::palette::{self, GRID, HUES, SHADE_NAMES, SHADES, grid_index};

/// Custom colours kept as swatches.
pub const MAX_CUSTOM: usize = 8;

/// A grid swatch's side, and the gap between swatches.
const SWATCH: f32 = 22.0;
const GAP: f32 = 3.0;
/// The row labels' column.
const LABEL_W: f32 = 58.0;

/// The accessible name of a palette swatch (what a screen reader and the tests read): its
/// palette name, marked when a selected track has it.
pub fn swatch_label(name: &str, current: bool) -> String {
    if current { format!("{name} (selected track)") } else { name.to_string() }
}

/// One swatch: a click target painted in `c`, named `name`, ringed when `current`.
fn swatch(ui: &mut egui::Ui, c: [u8; 3], size: egui::Vec2, name: &str, current: bool) -> egui::Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let label = swatch_label(name, current);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &label));
    let p = ui.painter();
    p.rect_filled(r, 3.0, rgb(c));
    if current {
        // Two rings, light and dark, so the mark reads on any colour and either theme.
        p.rect_stroke(r.expand(1.0), 4.0, Stroke::new(2.0, t.text), StrokeKind::Outside);
        p.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.5, t.panel_bg), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_stroke(r, 3.0, Stroke::new(1.0, t.text_dim), StrokeKind::Outside);
    }
    resp.on_hover_text(name)
}

/// The palette as swatches: the grid (a row per shade, labelled) and the originals below.
/// `marked`: colours to ring. Returns the palette index clicked.
pub fn palette_grid(ui: &mut egui::Ui, marked: &[[u8; 3]], size: f32, labels: bool) -> Option<usize> {
    let mut picked = None;
    let gap = (size * GAP / SWATCH).max(2.0);
    let row_w = HUES as f32 * size + (HUES - 1) as f32 * gap;
    ui.spacing_mut().item_spacing = vec2(gap, gap);
    let label_col = |ui: &mut egui::Ui, text: &str| {
        if labels {
            let (r, _) = ui.allocate_exact_size(vec2(LABEL_W, size), Sense::hover());
            ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, text, crate::theme::regular(11.0), Tokens::current().text_dim);
        }
    };
    for shade in 0..SHADES {
        ui.horizontal(|ui| {
            label_col(ui, SHADE_NAMES.get(shade).copied().unwrap_or(""));
            for hue in 0..HUES {
                let i = grid_index(shade, hue);
                let Some(c) = GRID.get(shade).and_then(|row| row.get(hue)).copied() else { continue };
                let name = palette::palette_name(i).unwrap_or_default();
                if swatch(ui, c, vec2(size, size), &name, marked.contains(&c)).clicked() {
                    picked = Some(i);
                }
            }
        });
    }
    // The 16 originals, as wide as the grid (indexes 0–15, as in every older version).
    let n = TRACK_COLORS.len() as f32;
    let w = ((row_w - (n - 1.0) * gap) / n).max(6.0);
    ui.horizontal(|ui| {
        label_col(ui, "Original");
        for (i, c) in TRACK_COLORS.iter().enumerate() {
            let name = palette::palette_name(i).unwrap_or_default();
            if swatch(ui, *c, vec2(w, size), &name, marked.contains(c)).clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}

/// The colours of the selected tracks, in track order.
fn selected_colors(app: &SoundApp) -> Vec<[u8; 3]> {
    let s = app.engine.session();
    s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).map(|t| t.color).collect()
}

/// What the window says the selection's colour is.
fn current_text(app: &SoundApp) -> String {
    let s = app.engine.session();
    let tracks: Vec<_> = s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).collect();
    let name = |c: [u8; 3]| palette::palette_index_of(c).and_then(palette::palette_name).unwrap_or_else(|| format!("custom {}", palette::hex(c)));
    match tracks.as_slice() {
        [] => "No track selected".to_string(),
        [t] => format!("{}: {}", t.name, name(t.color)),
        [first, rest @ ..] if rest.iter().all(|t| t.color == first.color) => format!("{} tracks: {}", tracks.len(), name(first.color)),
        _ => format!("{} tracks, several colors", tracks.len()),
    }
}

/// Apply `color` (a palette index, or any colour) to the selected tracks, or with `clips` to the
/// selected clips.
fn apply(app: &mut SoundApp, index: Option<usize>, color: [u8; 3], clips: bool) {
    let r = if clips {
        app.run("clip.color", json!({"color": color}))
    } else {
        match index {
            Some(i) => app.run("track.color", json!({"color": i})),
            None => app.run("track.color", json!({"color": color})),
        }
    };
    if let Err(e) = r {
        app.ui.status = e;
    }
}

/// Keep `c` first in the custom colours (no repeats, at most [`MAX_CUSTOM`]).
pub fn remember_custom(list: &mut Vec<[u8; 3]>, c: [u8; 3]) {
    list.retain(|x| *x != c);
    list.insert(0, c);
    list.truncate(MAX_CUSTOM);
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_color_palette;
    if !open {
        return;
    }
    egui::Window::new("Color Palette").open(&mut open).default_width(390.0).resizable(false).show(ctx, |ui| body(app, ui));
    app.ui.show_color_palette = open;
}

fn body(app: &mut SoundApp, ui: &mut egui::Ui) {
    let t = Tokens::current();
    let shift = ui.input(|x| x.modifiers.shift);
    ui.label("Click: the selected tracks · Shift+click: the selected clips");
    ui.label(RichText::new(current_text(app)).small().color(t.text_dim));
    ui.add_space(4.0);
    let marked = selected_colors(app);
    if let Some(i) = palette_grid(ui, &marked, SWATCH, true) {
        let c = palette::palette_color(i).unwrap_or(TRACK_COLORS[0]);
        apply(app, Some(i), c, shift);
    }
    // Custom colours: the recent ones, then Custom… for the picker.
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(LABEL_W, SWATCH), Sense::hover());
        ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, "Custom", crate::theme::regular(11.0), t.text_dim);
        let recent: Vec<[u8; 3]> = app.ui.custom_colors.iter().take(MAX_CUSTOM).copied().collect();
        for c in recent {
            let name = format!("Custom {}", palette::hex(c));
            if swatch(ui, c, vec2(SWATCH, SWATCH), &name, marked.contains(&c)).clicked() {
                apply(app, None, c, shift);
                remember_custom(&mut app.ui.custom_colors, c);
            }
        }
        let opening = app.ui.custom_pick.is_none();
        if ui.selectable_label(!opening, "Custom...").on_hover_text("Pick any color").clicked() {
            app.ui.custom_pick = if opening { Some(marked.first().copied().unwrap_or(TRACK_COLORS[0])) } else { None };
        }
    });
    if let Some(c) = app.ui.custom_pick {
        let mut col = rgb(c);
        ui.add_space(2.0);
        egui::color_picker::color_picker_color32(ui, &mut col, egui::color_picker::Alpha::Opaque);
        let c = [col.r(), col.g(), col.b()];
        app.ui.custom_pick = Some(c);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(SWATCH * 2.0, SWATCH), Sense::hover());
            ui.painter().rect_filled(r, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
            ui.label(palette::hex(c));
            if ui.button("Apply").on_hover_text("Color the selected tracks (Shift: the selected clips)").clicked() {
                apply(app, None, c, ui.input(|x| x.modifiers.shift));
                remember_custom(&mut app.ui.custom_colors, c);
            }
            if ui.button("Close").clicked() {
                app.ui.custom_pick = None;
            }
        });
    }
    ui.separator();
    ui.add(egui::Slider::new(&mut app.ui.track_tint, 0.0..=1.0).show_value(false).text("Track tint in the Edit window"));
    ui.add(egui::Slider::new(&mut app.ui.mix_strip_tint, 0.0..=1.0).show_value(false).text("Strip colour in the Mix window"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_colours_keep_the_last_eight_newest_first() {
        let mut v = Vec::new();
        for i in 0..10u8 {
            remember_custom(&mut v, [i, 0, 0]);
        }
        assert_eq!(v.len(), MAX_CUSTOM);
        assert_eq!(v[0], [9, 0, 0]);
        assert_eq!(v[7], [2, 0, 0]);
        remember_custom(&mut v, [5, 0, 0]);
        assert_eq!(v[0], [5, 0, 0]);
        assert_eq!(v.iter().filter(|c| **c == [5, 0, 0]).count(), 1);
        assert_eq!(v.len(), MAX_CUSTOM);
    }
}

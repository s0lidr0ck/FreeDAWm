//! One plugin card in the browser grid: its picture (or a tinted placeholder with its name and
//! vendor), the format badge, the favourite star, the name and vendor underneath, and its menu.

use super::catalog::{BrowserCategory, BrowserItem, Format};
use crate::theme::{Tokens, bold, regular};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2, pos2, vec2};
use std::sync::Arc;

/// What "Refresh thumbnail" does, as its tooltip: it opens the plugin and waits for it to load.
pub const REFRESH_HELP: &str = "Opens the plugin on the selected track and captures its editor once it has loaded. \
    For a plugin that loads slowly, open its editor, wait until it is ready, then right-click its slot and \
    choose Capture Thumbnail Now.";

/// The picture area of a small card.
pub const SMALL: Vec2 = vec2(132.0, 96.0);
/// The picture area of a large card.
pub const LARGE: Vec2 = vec2(200.0, 140.0);
/// The name and vendor lines under the picture.
pub const CAPTION: f32 = 32.0;

/// The picture area for the card size.
pub fn thumb_size(large: bool) -> Vec2 {
    if large { LARGE } else { SMALL }
}

/// A whole card, picture and caption.
pub fn card_size(large: bool) -> Vec2 {
    let t = thumb_size(large);
    vec2(t.x, t.y + CAPTION)
}

/// What happened to a card this frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CardResponse {
    pub clicked: bool,
    pub double_clicked: bool,
    pub star_toggled: bool,
    pub drag_started: bool,
    /// The item picked from the card's menu.
    pub menu: Option<CardAction>,
}

/// The card menu's items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    InsertOnSelected,
    /// Only offered when the plugin is on the selected track.
    OpenEditor,
    Favourite,
    SetCategory(BrowserCategory),
    RefreshThumbnail,
    /// Only offered for hosted (VST3, CLAP, AU) plugins.
    ShowInFolder,
}

fn on_selected_id() -> Id {
    Id::new("browser-on-selected-track")
}

/// Tells this frame's cards which plugins are on the selected track (their menus then offer
/// Open editor).
pub fn set_on_selected_track(ctx: &egui::Context, plugins: Vec<String>) {
    ctx.data_mut(|d| d.insert_temp(on_selected_id(), Arc::new(plugins)));
}

fn on_selected_track(ctx: &egui::Context, plugin: &str) -> bool {
    ctx.data(|d| d.get_temp::<Arc<Vec<String>>>(on_selected_id())).is_some_and(|p| p.iter().any(|p| p == plugin))
}

/// Whether the plugin is hosted (has a file of its own).
pub fn hosted(format: Format) -> bool {
    matches!(format, Format::Vst3 | Format::Clap | Format::Au)
}

/// The placeholder's tint: a hue per category, kept quiet against the theme's panel colour.
fn tint(category: BrowserCategory, t: &Tokens) -> Color32 {
    let i = BrowserCategory::ALL.iter().position(|c| *c == category).unwrap_or_default();
    let hue = (i as f32 * 0.137 + 0.58).fract();
    let colour: Color32 = egui::ecolor::Hsva::new(hue, 0.55, 0.62, 1.0).into();
    t.panel_bg2.lerp_to_gamma(colour, 0.28)
}

/// Text cut to one line of `width`, with an ellipsis.
fn one_line(ui: &Ui, text: &str, font: egui::FontId, colour: Color32, width: f32) -> Arc<egui::Galley> {
    let mut job = LayoutJob::simple(text.to_string(), font, colour, width);
    job.wrap = TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// The placeholder: a tinted tile with the name and vendor.
fn placeholder(ui: &Ui, r: Rect, item: &BrowserItem, large: bool, t: &Tokens) {
    let p = ui.painter();
    let fill = tint(item.category, t);
    p.rect_filled(r, CornerRadius::same(4), fill);
    let inner = r.shrink2(vec2(10.0, 8.0));
    let mut job = LayoutJob::simple(item.name.clone(), bold(if large { 15.0 } else { 12.5 }), t.text, inner.width());
    job.halign = egui::Align::Center;
    job.wrap = TextWrapping { max_width: inner.width(), max_rows: 3, break_anywhere: false, overflow_character: Some('…') };
    let name = p.layout_job(job);
    let vendor = one_line(ui, &item.vendor, regular(if large { 11.5 } else { 10.0 }), t.text_dim, inner.width());
    let h = name.size().y + 3.0 + vendor.size().y;
    let top = r.center().y - h / 2.0 - 4.0;
    p.galley(pos2(r.center().x, top), name.clone(), t.text);
    p.galley(pos2(r.center().x - vendor.size().x / 2.0, top + name.size().y + 3.0), vendor, t.text_dim);
}

/// Where a picture of `size` (pixels) goes in `area`: as wide as the area; a picture taller than
/// the area shows its top part (the editor's title and display), a shorter one sits in the
/// middle. Returns the rectangle drawn into and the picture's part (uv) shown there.
pub fn fit(area: Rect, size: Vec2) -> (Rect, Rect) {
    if size.x <= 0.0 || size.y <= 0.0 {
        return (area, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
    }
    let h = area.width() * size.y / size.x;
    if h >= area.height() {
        (area, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, area.height() / h)))
    } else {
        let r = Rect::from_center_size(area.center(), vec2(area.width(), h));
        (r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)))
    }
}

/// One card. Cards are laid out by the caller (`card_size` each); the star and the menu act on
/// the card's own plugin.
pub fn card(ui: &mut Ui, item: &BrowserItem, thumb: Option<&TextureHandle>, starred: bool, large: bool) -> CardResponse {
    let t = Tokens::current();
    let size = card_size(large);
    let (_, rect) = ui.allocate_space(size);
    let resp = ui.interact(rect, Id::new(("browser-card", item.id.as_str())), Sense::click_and_drag());
    if resp.drag_started() {
        super::drop::start(ui.ctx(), item);
    }
    let thumb_rect = Rect::from_min_size(rect.min, thumb_size(large));
    if ui.is_rect_visible(rect) {
        match thumb {
            Some(tex) => {
                ui.painter().rect_filled(thumb_rect, CornerRadius::same(4), t.panel_bg2);
                let (at, uv) = fit(thumb_rect.shrink(1.0), tex.size_vec2());
                ui.painter().image(tex.id(), at, uv, Color32::WHITE);
            }
            None => placeholder(ui, thumb_rect, item, large, &t),
        }
        let border = if resp.hovered() { t.accent } else { t.border };
        ui.painter().rect_stroke(thumb_rect, CornerRadius::same(4), Stroke::new(1.0, border), StrokeKind::Inside);
        // Format badge, bottom left of the picture.
        let badge = ui.painter().layout_no_wrap(item.format.label().to_string(), bold(9.0), t.text);
        let br = Rect::from_min_size(pos2(thumb_rect.min.x + 5.0, thumb_rect.max.y - badge.size().y - 9.0), badge.size() + vec2(8.0, 4.0));
        ui.painter().rect_filled(br, CornerRadius::same(3), t.window_bg.gamma_multiply(0.85));
        ui.painter().galley(br.min + vec2(4.0, 2.0), badge, t.text);
        // Name and vendor.
        let w = rect.width();
        let name = one_line(ui, &item.name, bold(11.5), t.text, w);
        let vendor = one_line(ui, &item.vendor, regular(10.5), t.text_dim, w);
        let y = thumb_rect.max.y + 3.0;
        ui.painter().galley(pos2(rect.min.x, y), name.clone(), t.text);
        ui.painter().galley(pos2(rect.min.x, y + name.size().y + 1.0), vendor, t.text_dim);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &item.name));

    // The star, over the card (registered after it, so it takes its own clicks).
    let star_rect = Rect::from_min_size(pos2(thumb_rect.max.x - 24.0, thumb_rect.min.y + 3.0), vec2(21.0, 21.0));
    let star = ui.interact(star_rect, Id::new(("browser-star", item.id.as_str())), Sense::click());
    if ui.is_rect_visible(star_rect) && (starred || resp.hovered() || star.hovered()) {
        let colour = if starred {
            t.accent
        } else if star.hovered() {
            t.text
        } else {
            t.text_dim
        };
        ui.painter().text(star_rect.center(), Align2::CENTER_CENTER, if starred { "★" } else { "☆" }, regular(15.0), colour);
    }
    let star_label = format!("Favourite {}", item.name);
    star.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, starred, &star_label));
    let star_toggled = star.clicked();
    let star = star.on_hover_text(if starred { "Remove from favourites" } else { "Add to favourites" });
    drop(star);

    let mut menu = None;
    let on_track = on_selected_track(ui.ctx(), &item.id);
    let resp = resp.on_hover_text(format!("{} · {} · {} · {}", item.name, item.vendor, item.format.label(), item.category.label()));
    resp.context_menu(|ui| {
        if ui.button("Insert on selected track").clicked() {
            menu = Some(CardAction::InsertOnSelected);
        }
        if on_track && ui.button("Open editor").clicked() {
            menu = Some(CardAction::OpenEditor);
        }
        if ui.button(if starred { "Remove from favourites" } else { "Add to favourites" }).clicked() {
            menu = Some(CardAction::Favourite);
        }
        ui.menu_button("Category", |ui| {
            for c in BrowserCategory::ALL {
                if ui.radio(item.category == c, c.label()).clicked() {
                    menu = Some(CardAction::SetCategory(c));
                }
            }
        });
        if hosted(item.format) {
            let refresh = ui.button("Refresh thumbnail").on_hover_text(REFRESH_HELP);
            if refresh.clicked() {
                menu = Some(CardAction::RefreshThumbnail);
            }
            if ui.button("Show in folder").clicked() {
                menu = Some(CardAction::ShowInFolder);
            }
        }
        if menu.is_some() {
            ui.close();
        }
    });
    CardResponse { clicked: resp.clicked(), double_clicked: resp.double_clicked(), star_toggled, drag_started: resp.drag_started(), menu }
}

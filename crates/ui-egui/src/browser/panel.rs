//! The docked plugin browser panel: Instruments / Effects / Presets tabs, a search field, filter
//! chips and a virtualised grid of cards, with a note listing the plugins the scans could not use.

use super::card::{self, CardAction, CardResponse};
use super::catalog::{BrowserCategory, BrowserItem, Catalog, Kind, Query, ScanFailure, build_catalog, filter};
use super::{BrowserState, BrowserTab};
use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui, vec2};
use std::collections::BTreeMap;

/// What the panel did on its last frame (read by tests and diagnostics).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrowserDebug {
    /// Cards built (drawn) on the last frame: only the visible rows build theirs.
    pub cards_built: usize,
    /// How long the last frame of the panel took, in milliseconds (0 on the web).
    pub panel_ms: f32,
    /// While a card is dragged: the place under the pointer it would land on.
    pub drop_hover: Option<super::drop::DropTarget>,
}

/// The chip lists for one catalog, tab and set of category overrides: built when one of those
/// changes, not every frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ChipCache {
    /// The catalog's item list (address and length) the lists were built from.
    items: (usize, usize),
    kind: Kind,
    overrides: BTreeMap<String, BrowserCategory>,
    categories: Vec<BrowserCategory>,
    vendors: Vec<String>,
}

impl ChipCache {
    fn key(catalog: &Catalog) -> (usize, usize) {
        (catalog.items.as_ptr() as usize, catalog.items.len())
    }

    fn build(catalog: &Catalog, kind: Kind, overrides: &BTreeMap<String, BrowserCategory>) -> Self {
        let in_tab = || catalog.items.iter().filter(|i| i.kind == kind);
        let categories =
            BrowserCategory::ALL.into_iter().filter(|c| in_tab().any(|i| overrides.get(&i.id).copied().unwrap_or(i.category) == *c)).collect();
        let mut vendors: Vec<String> = in_tab().map(|i| i.vendor.clone()).filter(|v| !v.is_empty()).collect();
        vendors.sort_by_cached_key(|v| v.to_lowercase());
        vendors.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        ChipCache { items: Self::key(catalog), kind, overrides: overrides.clone(), categories, vendors }
    }

    fn fresh(&self, catalog: &Catalog, kind: Kind, overrides: &BTreeMap<String, BrowserCategory>) -> bool {
        self.items == Self::key(catalog) && self.kind == kind && &self.overrides == overrides
    }
}

/// Space between cards, and around the grid.
const GAP: f32 = 8.0;
/// The footer line's height.
const FOOTER: f32 = 22.0;

/// Builds the catalog from the plugins the app knows now: built-ins, the CLAP and VST3 hosts'
/// last scans (none if they have not scanned) and Audio Units, with the scans' failures.
pub(crate) fn rebuild_catalog(app: &mut SoundApp) {
    let clap = soundcraft_clap_host::cached();
    let vst3 = soundcraft_vst3_host::cached();
    let au = soundcraft_au_host::scan();
    let failures: Vec<ScanFailure> =
        soundcraft_vst3_host::scan_failures().into_iter().chain(soundcraft_clap_host::scan_failures()).map(ScanFailure::from).collect();
    app.browser_catalog =
        Some(build_catalog(soundcraft_dsp::plugins(), clap.as_deref().unwrap_or_default(), vst3.as_deref().unwrap_or_default(), &au, failures));
    app.browser_hosts = (clap.is_some(), vst3.is_some());
}

/// The catalog is built on first show, and again once a host that had not scanned has (a scan
/// from the insert menus). Asking an unscanned host is cheap; a scanned one is not asked again.
pub(crate) fn ensure_catalog(app: &mut SoundApp) {
    let (clap, vst3) = app.browser_hosts;
    let newly_scanned = (!clap && soundcraft_clap_host::cached().is_some()) || (!vst3 && soundcraft_vst3_host::cached().is_some());
    if app.browser_catalog.is_none() || newly_scanned {
        rebuild_catalog(app);
    }
}

/// The running scan's line: "Scanning 312 / 767…" and its Cancel.
fn scan_progress(app: &SoundApp, ui: &mut Ui) {
    let Some(job) = &app.plugin_scan else { return };
    let t = Tokens::current();
    let (done, total) = job.progress();
    ui.horizontal(|ui| {
        let what = if job.resolving() { "Resolving plugins" } else { "Scanning" };
        let text = if job.cancelled() {
            "Stopping the scan…".to_string()
        } else if !job.counted() || total == 0 {
            format!("{what}…")
        } else {
            format!("{what} {done} / {total}…")
        };
        ui.label(egui::RichText::new(text).color(t.text_dim).font(regular(11.0)));
        if !job.cancelled() && ui.small_button("Cancel").clicked() {
            job.cancel();
        }
    });
}

/// A tab of the strip at the top (an accessible radio button).
fn tab(ui: &mut Ui, label: &str, on: bool, width: f32) -> bool {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
    let fill = if on {
        t.row_selected
    } else if resp.hovered() {
        t.panel_bg2
    } else {
        t.panel_bg
    };
    ui.painter().rect_filled(r, CornerRadius::same(3), fill);
    if on {
        ui.painter().line_segment([r.left_bottom() + vec2(3.0, -1.0), r.right_bottom() + vec2(-3.0, -1.0)], Stroke::new(2.0, t.accent));
    }
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, bold(11.5), if on { t.text } else { t.text_dim });
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, label));
    resp.clicked()
}

/// A filter chip (an accessible toggle button).
fn chip(ui: &mut Ui, label: &str, on: bool) -> bool {
    let t = Tokens::current();
    let galley = ui.painter().layout_no_wrap(label.to_string(), regular(11.0), t.text);
    let (r, resp) = ui.allocate_exact_size(galley.size() + vec2(14.0, 6.0), Sense::click());
    let fill = if on {
        t.accent
    } else if resp.hovered() {
        t.button_hi
    } else {
        t.panel_bg2
    };
    ui.painter().rect(r, CornerRadius::same(10), fill, Stroke::new(1.0, if on { t.accent } else { t.border }), StrokeKind::Inside);
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, if on { t.text_dark } else { t.text });
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
    resp.clicked()
}

fn toggle_favourite(state: &mut BrowserState, id: &str) {
    if let Some(at) = state.favourites.iter().position(|f| f == id) {
        state.favourites.remove(at);
    } else {
        state.favourites.push(id.to_string());
    }
}

fn kind_of(tab: BrowserTab) -> Kind {
    if tab == BrowserTab::Instruments { Kind::Instrument } else { Kind::Effect }
}

fn failures_open_id() -> Id {
    Id::new("browser-failures-open")
}

/// The plugins on the first selected track (its instrument and inserts).
fn on_selected_track(app: &SoundApp) -> Vec<String> {
    let s = app.engine.session();
    let Some(track) = s.edit.selected_tracks.first().and_then(|t| s.track(*t)) else { return Vec::new() };
    track.instrument.iter().chain(track.mixer.inserts.iter()).map(|i| i.plugin.clone()).collect()
}

pub(crate) fn browser_panel(app: &mut SoundApp, ui: &mut Ui) {
    #[cfg(not(target_arch = "wasm32"))]
    let started = std::time::Instant::now();
    let t = Tokens::current();
    let rect = ui.max_rect();
    app.browser_rect = Some((ui.ctx().viewport_id(), rect));
    ui.painter().rect_filled(rect, 0.0, t.panel_bg);
    ensure_catalog(app);
    super::presets::follow_focus(app);
    let Some(catalog) = app.browser_catalog.take() else { return };
    let mut built = 0;
    let mut rescan = None;
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(GAP, 6.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut ui;
    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);

    // Tabs, card size and the panel menu.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let w = ((ui.available_width() - 64.0) / 3.0).clamp(54.0, 96.0);
        for (tb, label) in [(BrowserTab::Instruments, "Instruments"), (BrowserTab::Effects, "Effects"), (BrowserTab::Presets, "Presets")] {
            if tab(ui, label, app.ui.browser.tab == tb, w) {
                app.ui.browser.tab = tb;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("⏷", |ui| {
                let idle = app.plugin_scan.is_none();
                if ui.add_enabled(idle, egui::Button::new("Rescan plugins")).on_hover_text("Scan new and changed plugins").clicked() {
                    rescan = Some(false);
                    ui.close();
                }
                if ui
                    .add_enabled(idle, egui::Button::new("Rescan, retrying failed plugins"))
                    .on_hover_text("Also try the plugins that failed, crashed or hung the scanner again")
                    .clicked()
                {
                    rescan = Some(true);
                    ui.close();
                }
                ui.checkbox(&mut app.ui.browser.large_cards, "Large cards");
            })
            .response
            .on_hover_text("Plugin browser options");
            let large = app.ui.browser.large_cards;
            let tip = if large { "Small cards" } else { "Large cards" };
            if crate::widgets::icon_button(ui, vec2(24.0, 22.0), "grid", large, tip).clicked() {
                app.ui.browser.large_cards = !large;
            }
        });
    });

    // Search: typing anywhere over the panel lands here.
    let search_id = Id::new("browser-search");
    let typing = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_))));
    let pointer_over = ui.ctx().pointer_latest_pos().is_some_and(|p| rect.contains(p));
    if typing && pointer_over && ui.ctx().memory(|m| m.focused().is_none()) {
        ui.ctx().memory_mut(|m| m.request_focus(search_id));
    }
    // The Presets tab searches its own list.
    let (text, hint) = match app.ui.browser.tab {
        BrowserTab::Presets => (&mut app.ui.browser.preset_text, "Search presets"),
        _ => (&mut app.ui.browser.text, "Search name, vendor or category"),
    };
    ui.add(egui::TextEdit::singleline(text).id(search_id).hint_text(hint).desired_width(f32::INFINITY));

    scan_progress(app, ui);
    let tab_now = app.ui.browser.tab;
    if tab_now == BrowserTab::Presets {
        super::presets::tab(app, ui, &catalog);
    } else {
        let kind = kind_of(tab_now);
        chips(app, ui, &catalog, kind);
        // (The web build hosts no plugins: its hosts report an empty scan, so this never shows.)
        if (!app.browser_hosts.0 || !app.browser_hosts.1) && app.plugin_scan.is_none() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("VST3 and CLAP plugins are not scanned yet.").color(t.text_dim).font(regular(11.0)));
                if ui.small_button("Scan plugins").clicked() {
                    rescan = Some(false);
                }
            });
        }
        let b = &app.ui.browser;
        let query = Query {
            kind,
            text: b.text.clone(),
            favourites: b.favourites_only,
            recent: b.recent_only,
            category: b.category,
            vendor: b.vendor.clone(),
        };
        let shown = filter(&catalog.items, &query, &b.category_overrides, &b.favourites, &b.recent);
        let footer = if catalog.failures.is_empty() { 0.0 } else { FOOTER };
        let grid_h = (ui.available_height() - footer).max(0.0);
        let mut thumbs = std::mem::take(&mut app.browser_thumbs);
        let (responses, offset) = grid(app, &mut thumbs, ui, &catalog, &shown, grid_h, &mut built);
        app.browser_thumbs = thumbs;
        if let (Some(offset), Some(saved)) = (offset, app.ui.browser.scroll.get_mut(tab_now.index())) {
            *saved = offset;
        }
        for (index, r) in responses {
            if let Some(item) = catalog.items.get(index) {
                act(app, item, &r);
            }
        }
    }
    failures_note(ui, &catalog.failures);

    app.browser_catalog = Some(catalog);
    if let Some(retry_failed) = rescan {
        super::scan_job::start(app, retry_failed);
    }
    app.browser_debug.cards_built = built;
    #[cfg(not(target_arch = "wasm32"))]
    {
        app.browser_debug.panel_ms = started.elapsed().as_secs_f32() * 1000.0;
    }
}

/// ★ Favourites, Recent, the categories the tab's plugins have, and the vendor menu.
fn chips(app: &mut SoundApp, ui: &mut Ui, catalog: &Catalog, kind: Kind) {
    if !app.browser_chips.as_ref().is_some_and(|c| c.fresh(catalog, kind, &app.ui.browser.category_overrides)) {
        app.browser_chips = Some(ChipCache::build(catalog, kind, &app.ui.browser.category_overrides));
    }
    let Some(cache) = &app.browser_chips else { return };
    let (present, vendors) = (&cache.categories, &cache.vendors);
    let b = &mut app.ui.browser;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        if chip(ui, "★ Favourites", b.favourites_only) {
            b.favourites_only = !b.favourites_only;
        }
        if chip(ui, "Recent", b.recent_only) {
            b.recent_only = !b.recent_only;
        }
        let active = b.category;
        for c in BrowserCategory::ALL.into_iter().filter(|c| present.contains(c) || active == Some(*c)) {
            if chip(ui, c.label(), b.category == Some(c)) {
                b.category = if b.category == Some(c) { None } else { Some(c) };
            }
        }
        let label = match &b.vendor {
            Some(v) => format!("{v} ⏷"),
            None => "Vendor ⏷".to_string(),
        };
        ui.menu_button(label, |ui| {
            if ui.radio(b.vendor.is_none(), "All vendors").clicked() {
                b.vendor = None;
                ui.close();
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for v in vendors {
                    let on = b.vendor.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(v));
                    if ui.radio(on, v).clicked() {
                        b.vendor = Some(v.clone());
                        ui.close();
                    }
                }
            });
        });
    });
}

/// The cards of `shown` in rows; only the rows in view are built (and only their pictures are
/// loaded). The width left over after the
/// columns is shared out between them. The grid opens at the tab's saved scroll offset (the
/// first time this tab's grid is shown in this context, e.g. after a reload). Returns what each
/// card did and the scroll offset now.
fn grid(
    app: &SoundApp,
    thumbs: &mut super::thumbs::ThumbCache,
    ui: &mut Ui,
    catalog: &Catalog,
    shown: &[usize],
    height: f32,
    built: &mut usize,
) -> (Vec<(usize, CardResponse)>, Option<f32>) {
    let t = Tokens::current();
    let large = app.ui.browser.large_cards;
    let size = card::card_size(large);
    let width = ui.available_width();
    let cols = (((width + GAP) / (size.x + GAP)).floor() as usize).max(1);
    let rows = shown.len().div_ceil(cols);
    let mut out = Vec::new();
    if shown.is_empty() {
        let why = if catalog.items.is_empty() { "No plugins" } else { "No plugins match" };
        ui.label(egui::RichText::new(why).color(t.text_dim));
        ui.allocate_space(vec2(ui.available_width(), (height - 20.0).max(0.0)));
        return (out, None);
    }
    let spare = (width - cols as f32 * size.x - (cols as f32 - 1.0) * GAP).max(0.0) / cols as f32;
    card::set_on_selected_track(ui.ctx(), on_selected_track(app));
    let b = &app.ui.browser;
    let tab = b.tab.index();
    let restored = Id::new(("browser-scroll-restored", tab));
    let mut area = egui::ScrollArea::vertical().id_salt(("browser-grid", tab)).auto_shrink([false, false]).max_height(height);
    if !ui.ctx().data(|d| d.get_temp::<bool>(restored).unwrap_or(false)) {
        ui.ctx().data_mut(|d| d.insert_temp(restored, true));
        area = area.vertical_scroll_offset(b.scroll.get(tab).copied().unwrap_or_default().max(0.0));
    }
    let output = ui.scope(|ui| {
        ui.spacing_mut().item_spacing = vec2(GAP + spare, GAP);
        area.show_rows(ui, size.y, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    ui.add_space(spare / 2.0);
                    for &index in shown.iter().skip(row * cols).take(cols) {
                        let Some(item) = catalog.items.get(index) else { continue };
                        let starred = b.favourites.contains(&item.id);
                        let overridden = b.category_overrides.get(&item.id).copied();
                        let at = ui.cursor().min;
                        let thumb = thumbs.texture(ui.ctx(), item);
                        let r = match overridden {
                            Some(category) => card::card(ui, &BrowserItem { category, ..item.clone() }, thumb.as_ref(), starred, large),
                            None => card::card(ui, item, thumb.as_ref(), starred, large),
                        };
                        *built += 1;
                        if b.selected.as_deref() == Some(item.id.as_str()) {
                            let sel = Rect::from_min_size(at, card::thumb_size(large)).expand(2.0);
                            ui.painter().rect_stroke(sel, CornerRadius::same(5), Stroke::new(2.0, t.accent), StrokeKind::Outside);
                        }
                        if r != CardResponse::default() {
                            out.push((index, r));
                        }
                    }
                });
            }
        })
    });
    (out, Some(output.inner.state.offset.y))
}

/// What a card's click, double-click, star or menu does here. (Dragging is the card's own, see
/// [`super::drop`]; refreshing a picture, [`super::thumbs`].)
fn act(app: &mut SoundApp, item: &BrowserItem, r: &CardResponse) {
    match r.menu {
        Some(CardAction::InsertOnSelected) => super::drop::place_on_selected(app, item),
        Some(CardAction::OpenEditor) => super::drop::open_on_selected(app, &item.id),
        Some(CardAction::ShowInFolder) => super::drop::show_in_folder(app, item),
        Some(CardAction::RefreshThumbnail) => super::thumbs::refresh_thumbnail(app, item),
        _ => {}
    }
    if r.double_clicked {
        super::drop::place_on_selected(app, item);
    }
    let b = &mut app.ui.browser;
    if r.clicked || r.double_clicked {
        b.selected = Some(item.id.clone());
    }
    let mut changed = false;
    if r.star_toggled {
        toggle_favourite(b, &item.id);
        changed = true;
    }
    match r.menu {
        Some(CardAction::Favourite) => {
            toggle_favourite(b, &item.id);
            changed = true;
        }
        Some(CardAction::SetCategory(c)) => {
            if c == item.category {
                b.category_overrides.remove(&item.id);
            } else {
                b.category_overrides.insert(item.id.clone(), c);
            }
            changed = true;
        }
        _ => {}
    }
    if changed {
        // Saved straight away, as Recent is: curation should survive a crash.
        app.recent_changed = true;
    }
}

/// "n plugins failed to load", opening a window with each one's path and reason.
fn failures_note(ui: &mut Ui, failures: &[ScanFailure]) {
    let ctx = ui.ctx().clone();
    if failures.is_empty() {
        ctx.data_mut(|d| d.remove::<bool>(failures_open_id()));
        return;
    }
    let n = failures.len();
    let text = if n == 1 { "1 plugin failed to load".to_string() } else { format!("{n} plugins failed to load") };
    if ui.link(egui::RichText::new(text).font(regular(11.0))).clicked() {
        ctx.data_mut(|d| d.insert_temp(failures_open_id(), true));
    }
    let mut open = ctx.data(|d| d.get_temp::<bool>(failures_open_id())).unwrap_or(false);
    if !open {
        return;
    }
    let t = Tokens::current();
    egui::Window::new("Plugins that failed to load").id(Id::new("browser-failures")).open(&mut open).default_width(460.0).collapsible(false).show(
        &ctx,
        |ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for f in failures {
                    ui.label(egui::RichText::new(format!("{} — {}", f.path, f.reason)).color(t.text));
                }
            });
        },
    );
    ctx.data_mut(|d| d.insert_temp(failures_open_id(), open));
}

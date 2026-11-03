//! Channel width for many channels at once (spec 2026-11-02 item 4): Slim / Wide / Auto for the
//! Mix window's selected channels or for all of them, from a small bar at the top of the Mix
//! view, a channel's right-click menu (and the Tracks list's) and View › Channel Width. Each runs
//! `mix.strip_size` once, so it is one undo step.

use crate::theme::{Tokens, bold, regular};
use crate::{MainWindow, SoundApp};
use egui::{Color32, Ui, vec2};
use serde_json::{Value, json};
use soundcraft_model::{StripSize, TrackId};

/// The UI commands: (id, size, all channels).
pub const COMMANDS: [(&str, &str, bool); 6] = [
    ("ui.mix.width.slim", "slim", false),
    ("ui.mix.width.wide", "wide", false),
    ("ui.mix.width.auto", "auto", false),
    ("ui.mix.width.slim_all", "slim", true),
    ("ui.mix.width.wide_all", "wide", true),
    ("ui.mix.width.auto_all", "auto", true),
];

/// The id of the command setting `size` on the selection (or on all channels).
pub fn command_id(size: StripSize, all: bool) -> &'static str {
    let name = size_name(size);
    COMMANDS.iter().find(|(_, s, a)| *s == name && *a == all).map_or("ui.mix.width.auto", |c| c.0)
}

fn size_name(size: StripSize) -> &'static str {
    match size {
        StripSize::Auto => "auto",
        StripSize::Wide => "wide",
        StripSize::Slim => "slim",
    }
}

/// The channels the Mix window's selection holds.
fn selected(app: &SoundApp) -> Vec<TrackId> {
    crate::window_sync::selection_of(app, MainWindow::Mix)
}

/// Runs one of [`COMMANDS`]: `{tracks?: [...]}` overrides the selection (not with `_all`).
pub fn run(app: &mut SoundApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    let (_, size, all) = COMMANDS.iter().find(|c| c.0 == id)?;
    if *all {
        return Some(app.run("mix.strip_size", json!({"all": true, "size": size})));
    }
    let tracks: Vec<Value> = match p.get("tracks").and_then(Value::as_array) {
        Some(t) => t.clone(),
        None => selected(app).iter().map(|t| json!(t.0)).collect(),
    };
    if tracks.is_empty() {
        return Some(Err("Select the channels to size first (or use All)".into()));
    }
    Some(app.run("mix.strip_size", json!({"tracks": tracks, "size": size})))
}

/// Whether one of [`COMMANDS`] can run now (the selection ones need selected channels).
pub fn enabled(app: &SoundApp, id: &str) -> bool {
    COMMANDS.iter().find(|c| c.0 == id).is_some_and(|(_, _, all)| *all || !selected(app).is_empty())
}

/// The size every one of `tracks` has, if they agree.
fn common_size(app: &SoundApp, tracks: &[TrackId]) -> Option<StripSize> {
    let s = app.engine.session();
    let mut sizes = tracks.iter().filter_map(|t| s.track(*t)).map(|t| t.mixer.strip_size);
    let first = sizes.next()?;
    sizes.all(|x| x == first).then_some(first)
}

/// The bar's height.
pub const BAR_H: f32 = 24.0;

/// The small bar at the top of the Mix view: "Channel width  Slim | Wide | Auto  for  Selected (n) | All".
pub fn bar(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let sel = selected(app);
    let all = app.ui.mix_width_all || sel.is_empty();
    let shown: Vec<TrackId> = if all { app.engine.session().tracks.iter().map(|x| x.id).collect() } else { sel.clone() };
    let current = common_size(app, &shown);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), egui::Sense::hover());
    crate::vocal_editor::hook(ui.ctx(), "mix-width-bar", r);
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(8.0, 2.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    c.spacing_mut().item_spacing.x = 0.0;
    c.label(egui::RichText::new("Channel width").font(regular(11.0)).color(t.text_dim));
    c.add_space(8.0);
    for (size, label, tip) in [
        (StripSize::Slim, "Slim", "Slim channels"),
        (StripSize::Wide, "Wide", "Wide channels"),
        (StripSize::Auto, "Auto", "Follow View › Narrow Mix"),
    ] {
        let on = current == Some(size);
        if segment(&mut c, label, on, t.accent).on_hover_text(tip).clicked() {
            let id = command_id(size, all);
            if let Err(e) = app.run(id, json!({})) {
                app.ui.status = e;
            }
        }
    }
    c.add_space(10.0);
    c.label(egui::RichText::new("for").font(regular(11.0)).color(t.text_dim));
    c.add_space(8.0);
    let n = sel.len();
    let label = format!("Selected ({n})");
    let resp = c.add_enabled_ui(n > 0, |c| segment(c, &label, !all, t.accent)).inner;
    if resp.on_hover_text("Size the selected channels").on_disabled_hover_text("Select channels first").clicked() {
        let _ = app.run("ui.set", json!({"mix_width_all": false}));
    }
    if segment(&mut c, "All", all, t.accent).on_hover_text("Size every channel").clicked() {
        let _ = app.run("ui.set", json!({"mix_width_all": true}));
    }
}

/// One segment of the bar: a small selectable button.
fn segment(ui: &mut Ui, label: &str, on: bool, accent: Color32) -> egui::Response {
    let t = Tokens::current();
    let text = egui::RichText::new(label).font(if on { bold(11.0) } else { regular(11.0) }).color(if on { Color32::WHITE } else { t.text });
    let b = egui::Button::new(text).fill(if on { accent } else { t.button }).min_size(vec2(0.0, 18.0)).corner_radius(3.0);
    ui.add(b)
}

/// The channel width items of a channel's right-click menu (the strip's name plate or the Tracks
/// list): Slim / Wide / Auto for the Mix window's selection when it holds `track`, else for
/// `track`; then the same for all channels.
pub fn menu(app: &mut SoundApp, ui: &mut Ui, track: TrackId) {
    let sel = selected(app);
    let targets: Vec<TrackId> = if sel.contains(&track) { sel } else { vec![track] };
    let current = common_size(app, &targets);
    let head = if targets.len() > 1 { format!("Channel width ({} channels)", targets.len()) } else { "Channel width".to_string() };
    ui.label(egui::RichText::new(head).small().weak());
    for (size, label) in [(StripSize::Slim, "Slim"), (StripSize::Wide, "Wide"), (StripSize::Auto, "Auto")] {
        if ui.add(egui::Button::selectable(current == Some(size), label)).clicked() {
            let tracks: Vec<u64> = targets.iter().map(|t| t.0).collect();
            if let Err(e) = app.run("mix.strip_size", json!({"tracks": tracks, "size": size_name(size)})) {
                app.ui.status = e;
            }
            ui.close();
        }
    }
    ui.menu_button("All Channels", |ui| {
        for (size, label) in [(StripSize::Slim, "Slim"), (StripSize::Wide, "Wide"), (StripSize::Auto, "Auto")] {
            if ui.button(label).clicked() {
                let _ = app.run(command_id(size, true), json!({}));
                ui.close();
            }
        }
    });
}

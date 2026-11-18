//! Vault's part of the module drawer: the impulse response in use (a file's name, its length
//! and channels, or the built-in space), a menu to load one from a file or clear it
//! (`mix.module_ir`), and the response drawn as its decay.

use crate::SoundApp;
use crate::module_drawer::menu_field;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::SampleSource;
use soundcraft_model::{Insert, SPACE_IR_KEY, Session, SpaceEdit, TrackId};
use std::sync::Arc;

/// The quietest level the decay picture shows, in dB under the response's peak.
const FLOOR_DB: f32 = -96.0;
/// The audio files the picker offers.
const FILES: [&str; 5] = ["wav", "wave", "aif", "aiff", "flac"];

/// The response a Vault module names, as the session has it ready (`None`: none loaded, or its
/// file is missing).
fn loaded(s: &Session, reference: &str, edit: &str) -> Option<Arc<dyn SampleSource>> {
    s.assets.get_edited(SPACE_IR_KEY, reference, edit, s.sample_rate.hz()).and_then(|d| d.0.downcast_ref::<Arc<dyn SampleSource>>()).cloned()
}

/// A response's width in a word.
fn width_name(channels: usize) -> &'static str {
    match channels {
        1 => "mono",
        4 => "true stereo",
        _ => "stereo",
    }
}

/// A response's name as the drawer shows it: the file's stem.
pub fn ir_label(reference: &str) -> String {
    std::path::Path::new(reference).file_stem().and_then(|s| s.to_str()).unwrap_or(reference).to_string()
}

/// Vault's rows under the drawer's graph: the response's picker and its decay. Returns the y
/// under them.
pub(crate) fn drawer_rows(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, inner: Rect, y: f32) -> f32 {
    let t = Tokens::current();
    let p = ui.painter().clone();
    let reference = ins.assets.get(SPACE_IR_KEY).cloned();
    let asset = reference.as_deref().and_then(|r| loaded(app.engine.session(), r, &ins.space_edit));
    let text = match (&reference, &asset) {
        (None, _) => "Built-in space · load an impulse response…".to_string(),
        (Some(r), Some(_)) => ir_label(r),
        (Some(r), None) => format!("{} (missing)", ir_label(r)),
    };
    p.text(pos2(inner.min.x, y + 11.0), Align2::LEFT_CENTER, "IR", bold(9.5), t.text_dim);
    let field = Rect::from_min_max(pos2(inner.min.x + 22.0, y), pos2(inner.max.x, y + 22.0));
    let hover = "The impulse response Vault plays the sound through: a WAV, AIFF or FLAC of a room, a plate, a spring or anything captured with Imprint. Mono, stereo, or four channels for true stereo.";
    let resp = menu_field(ui, field, &text, "vault-ir", true).on_hover_text(hover);
    let mut pick: Option<serde_json::Value> = None;
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let can_pick = app.services.pick_open.is_some();
        if ui.add_enabled(can_pick, egui::Button::new("Load file…")).clicked() {
            if let Some(open) = &app.services.pick_open
                && let Some(path) = open("mix.module_ir", &FILES)
            {
                pick = Some(json!({"path": path}));
            }
            ui.close();
        }
        if reference.is_some() && ui.button("Clear (built-in space)").clicked() {
            pick = Some(serde_json::Value::Null);
            ui.close();
        }
    });
    if let Some(ir) = pick
        && let Err(e) = app.run("mix.module_ir", json!({"track": track.0, "module": ins.id.0, "ir": ir}))
    {
        app.ui.status = e.to_string();
    }
    let mut y = y + 22.0 + 6.0;

    // The response's decay, with its length and width.
    let pic = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), 72.0));
    p.rect_filled(pic, CornerRadius::same(5), t.graph_bg);
    p.rect_stroke(pic, CornerRadius::same(5), Stroke::new(1.0, t.module_tile_border), StrokeKind::Inside);
    let area = pic.shrink(6.0);
    match &asset {
        Some(a) => {
            draw_decay(&p, area, a.as_ref(), t.accent, t.module_tile_border);
            let secs = a.frames() as f32 / a.sample_rate().max(1.0);
            p.text(area.right_top(), Align2::RIGHT_TOP, format!("{secs:.2} s · {}", width_name(a.num_channels())), regular(8.5), t.text_dim);
        }
        None if reference.is_some() => {
            p.text(area.center(), Align2::CENTER_CENTER, "The file is missing: the built-in space plays", regular(9.5), t.text_dim);
        }
        None => {
            p.text(area.center(), Align2::CENTER_CENTER, "Built-in space: a generated hall, 1.8 s", regular(9.5), t.text_dim);
        }
    }
    y = pic.max.y + 8.0;
    if reference.is_some() && asset.is_some() {
        y = edit_row(app, ui, track, ins, inner, y);
    }
    y
}

/// The response's edit under its picture: Reverse, the trim (start and end, seconds of the
/// file) and the stretch (`mix.module_ir_edit`, one undo step per change).
fn edit_row(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, inner: Rect, y: f32) -> f32 {
    let key = ui.id().with(("vault-edit", ins.id.0));
    let before = SpaceEdit::parse(&ins.space_edit);
    // A change being dragged shows as it goes; it is sent once the pointer lets go.
    let mut e: SpaceEdit = ui.ctx().data(|d| d.get_temp(key)).unwrap_or(before);
    let row = Rect::from_min_max(pos2(inner.min.x, y), pos2(inner.max.x, y + 22.0));
    let mut reset = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            crate::widgets::checkbox(ui, &mut e.reverse, "Reverse").on_hover_text("Play the response backwards: the swell before the sound");
            ui.add(egui::DragValue::new(&mut e.start).range(0.0..=SpaceEdit::MAX_SECONDS).speed(0.01).prefix("from ").suffix(" s"))
                .on_hover_text("Where the response starts in the file");
            ui.add(egui::DragValue::new(&mut e.end).range(0.0..=SpaceEdit::MAX_SECONDS).speed(0.01).prefix("to ").suffix(" s").custom_formatter(
                |v, _| {
                    if v <= 0.0 { "end".into() } else { format!("{v:.2}") }
                },
            ))
            .on_hover_text("Where the response ends in the file (end: the whole file)");
            ui.add(egui::DragValue::new(&mut e.stretch).range(SpaceEdit::STRETCH.0..=SpaceEdit::STRETCH.1).speed(0.005).prefix("× ").max_decimals(2))
                .on_hover_text("Stretch: longer is a bigger, darker space, as a tape slowed down");
            if !ins.space_edit.is_empty() && ui.small_button("Reset").on_hover_text("The file as it is").clicked() {
                reset = true;
            }
        });
    });
    // One change per release of a drag (not one per frame of it).
    let dragging = ui.ctx().input(|i| i.pointer.any_down());
    let changed = e.clamped() != before;
    if changed {
        ui.ctx().data_mut(|d| d.insert_temp(key, e));
    }
    let pending: Option<SpaceEdit> = ui.ctx().data(|d| d.get_temp(key));
    let send = if reset {
        Some(serde_json::json!({"track": track.0, "module": ins.id.0, "reset": true}))
    } else {
        pending.filter(|_| !dragging).map(
            |e| serde_json::json!({"track": track.0, "module": ins.id.0, "reverse": e.reverse, "start": e.start, "end": e.end, "stretch": e.stretch}),
        )
    };
    if let Some(p) = send {
        ui.ctx().data_mut(|d| d.remove::<SpaceEdit>(key));
        if let Err(err) = app.run("mix.module_ir_edit", p) {
            app.ui.status = err.to_string();
        }
    }
    row.max.y + 6.0
}

/// The response's level over time: per pixel column the loudest sample of any channel, in dB
/// under the peak.
fn draw_decay(p: &egui::Painter, area: Rect, ir: &dyn SampleSource, colour: Color32, grid: Color32) {
    let frames = ir.frames();
    let channels: Vec<&[f32]> = (0..ir.num_channels()).filter_map(|c| ir.channel(c)).collect();
    let peak = channels.iter().flat_map(|c| c.iter()).fold(0.0f32, |m, x| m.max(x.abs()));
    if frames == 0 || peak <= 0.0 {
        return;
    }
    for db in [-24.0f32, -48.0, -72.0] {
        let gy = area.min.y + area.height() * (db / FLOOR_DB);
        p.hline(area.x_range(), gy, Stroke::new(1.0, grid.gamma_multiply(0.6)));
    }
    let cols = area.width().max(1.0) as usize;
    for col in 0..cols {
        let a = col * frames / cols;
        let b = ((col + 1) * frames / cols).max(a + 1).min(frames);
        let loudest = channels.iter().filter_map(|c| c.get(a..b)).flatten().fold(0.0f32, |m, x| m.max(x.abs()));
        let db = (20.0 * (loudest / peak).max(1.0e-9).log10()).max(FLOOR_DB);
        let x = area.min.x + col as f32 + 0.5;
        let top = area.min.y + area.height() * (db / FLOOR_DB);
        p.vline(x, top..=area.max.y, Stroke::new(1.0, colour.gamma_multiply(0.85)));
    }
}

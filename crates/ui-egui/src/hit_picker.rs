//! Juke's hit picker: the hits found in drum takes (`mix.kit_pad_analyze`) as tiles in four
//! velocity bands, Ghost to Hard, coloured by band. Click a tile to hear it, tick it to keep it
//! (four a band: the layer's round-robins), drag it to another band; Apply builds the pad from
//! the kept hits (`mix.kit_pad_pick`), one undo step.

use crate::SoundApp;
use crate::theme::{bold, regular};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::plugins::juke::{LAYERS, ROBINS};
use soundcraft_model::TrackId;

/// The bands' names, softest first (the pad editor's layer rows, which list them hardest first).
pub const BANDS: [&str; LAYERS] = ["Ghost", "Soft", "Medium", "Hard"];
/// Their colours, softest first.
pub const BAND_COLORS: [Color32; LAYERS] = [
    Color32::from_rgb(0x6F, 0x93, 0xB0),
    Color32::from_rgb(0x7D, 0xB8, 0x6A),
    Color32::from_rgb(0xE8, 0xAA, 0x48),
    Color32::from_rgb(0xDB, 0x5B, 0x4B),
];

const GROUND: Color32 = Color32::from_rgb(0x2A, 0x24, 0x1F);
const TILE: Color32 = Color32::from_rgb(0x3A, 0x32, 0x2B);
const EDGE: Color32 = Color32::from_rgb(0x1E, 0x19, 0x15);
const LABEL: Color32 = Color32::from_rgb(0xD9, 0xCC, 0xB6);
const DIM: Color32 = Color32::from_rgb(0x9A, 0x8C, 0x7A);
const DARK_TEXT: Color32 = Color32::from_rgb(0x1E, 0x19, 0x15);
const AMBER: Color32 = Color32::from_rgb(0xF0, 0xB2, 0x4A);
const OFF: Color32 = Color32::from_rgb(0xE0, 0x6A, 0x5A);

/// The picker open on a Juke track: one analysis of takes for one pad.
#[derive(Debug, Clone)]
pub struct HitPicker {
    pub take: u64,
    pub track: TrackId,
    /// From 0.
    pub pad: usize,
    pub name: String,
    /// The note the drum rings at, when it is pitched.
    pub note: Option<String>,
    pub hits: Vec<PickHit>,
}

/// A hit in the picker.
#[derive(Debug, Clone)]
pub struct PickHit {
    pub hit: usize,
    pub db: f32,
    pub ms: f32,
    pub note: Option<String>,
    pub off_pitch: bool,
    /// Band (layer) from 0, the softest.
    pub band: usize,
    pub keep: bool,
    /// The analysis's own band and choice, for Best 16.
    pub auto: (usize, bool),
    /// Its outline, 0..1.
    pub env: Vec<f32>,
}

impl HitPicker {
    /// Hits kept in `band`.
    pub fn kept(&self, band: usize) -> usize {
        self.hits.iter().filter(|h| h.keep && h.band == band).count()
    }

    /// The picks to apply: each band's kept hits, softest first, as `mix.kit_pad_pick` takes them.
    pub fn picks(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for band in 0..LAYERS {
            let mut kept: Vec<&PickHit> = self.hits.iter().filter(|h| h.keep && h.band == band).collect();
            kept.sort_by(|a, b| a.db.total_cmp(&b.db));
            out.extend(kept.iter().map(|h| json!({"hit": h.hit, "layer": band + 1})));
        }
        out
    }
}

/// Analyses takes for pad `pad` of track `tid` (`params`: `paths`, or `selection: true`) and opens
/// the picker on them. A refusal goes to the status line.
pub fn open(app: &mut SoundApp, tid: TrackId, pad: usize, mut params: Value) {
    params["track"] = json!(tid.0);
    params["pad"] = json!(pad + 1);
    match app.run("mix.kit_pad_analyze", params) {
        Ok(v) => {
            let hits = v
                .get("hits")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|h| {
                            let band = h.get("band").and_then(Value::as_u64).unwrap_or(1).saturating_sub(1) as usize;
                            let keep = h.get("pick").is_some_and(|p| !p.is_null());
                            PickHit {
                                hit: h.get("hit").and_then(Value::as_u64).unwrap_or(0) as usize,
                                db: h.get("db").and_then(Value::as_f64).unwrap_or(-120.0) as f32,
                                ms: h.get("ms").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                                note: h.get("note").and_then(Value::as_str).map(str::to_string),
                                off_pitch: h.get("off_pitch").and_then(Value::as_bool).unwrap_or(false),
                                band: band.min(LAYERS - 1),
                                keep,
                                auto: (band.min(LAYERS - 1), keep),
                                env: h
                                    .get("env")
                                    .and_then(Value::as_array)
                                    .map(|e| e.iter().filter_map(Value::as_f64).map(|x| x as f32).collect())
                                    .unwrap_or_default(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            app.ui.hit_picker = Some(HitPicker {
                take: v.get("take").and_then(Value::as_u64).unwrap_or(0),
                track: tid,
                pad,
                name: v.get("name").and_then(Value::as_str).unwrap_or("Pad").to_string(),
                note: v.get("note").and_then(Value::as_str).map(str::to_string),
                hits,
            });
            let key = soundcraft_mix::SlotKey::Instrument;
            if !app.ui.plugin_windows.contains(&(tid, key)) {
                app.ui.plugin_windows.push((tid, key));
            }
            app.ui.status = "Pick the hits: click to hear, tick to keep (four a layer), drag to another layer".into();
        }
        Err(e) => app.ui.status = e,
    }
}

/// What the picker's controls asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ask {
    Play(usize),
    Toggle(usize),
    Move(usize, usize),
    Apply,
    Cancel,
    Best,
    Clear,
}

/// A hit being dragged to another band.
#[derive(Debug, Clone, Copy)]
struct HitDrag(usize);

/// Draws the picker in `r`; what it asks goes to `asks`.
pub fn draw(ui: &mut Ui, r: Rect, picker: &HitPicker, playing: Option<usize>, asks: &mut Vec<Ask>) {
    let p = ui.painter().clone();
    p.rect_filled(r, 6.0, GROUND);
    p.rect_stroke(r, 6.0, Stroke::new(1.0, EDGE), StrokeKind::Inside);
    // Header: what is picked for, and the actions.
    let head = Rect::from_min_size(r.min, vec2(r.width(), 32.0));
    let total: usize = (0..LAYERS).map(|b| picker.kept(b)).sum();
    let title = format!("Pick hits · Pad {} · {} · {} hits found", picker.pad + 1, picker.name, picker.hits.len());
    p.text(pos2(head.min.x + 10.0, head.center().y - 1.0), Align2::LEFT_CENTER, title, bold(13.0), LABEL);
    if let Some(n) = &picker.note {
        p.text(
            pos2(head.min.x + 10.0, head.max.y + 6.0),
            Align2::LEFT_CENTER,
            format!("Rings at {n}: hits at other notes are marked"),
            regular(10.0),
            DIM,
        );
    }
    let mut x = head.max.x - 8.0;
    let mut button = |ui: &mut Ui, w: f32, label: &str, primary: bool, enabled: bool| {
        let b = Rect::from_min_size(pos2(x - w, head.min.y + 5.0), vec2(w, 22.0));
        x -= w + 6.0;
        let resp = ui.interact(b, egui::Id::new(("hit-picker-btn", label)), if enabled { Sense::click() } else { Sense::hover() });
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
        let fill = match (primary, enabled, resp.hovered()) {
            (_, false, _) => Color32::from_black_alpha(40),
            (true, _, h) => {
                if h {
                    AMBER.gamma_multiply(1.1)
                } else {
                    AMBER
                }
            }
            (false, _, h) => {
                if h {
                    Color32::from_white_alpha(30)
                } else {
                    Color32::from_white_alpha(14)
                }
            }
        };
        ui.painter().rect_filled(b, 4.0, fill);
        let c = if primary && enabled {
            DARK_TEXT
        } else if enabled {
            LABEL
        } else {
            DIM
        };
        ui.painter().text(b.center(), Align2::CENTER_CENTER, label, bold(11.0), c);
        enabled && resp.clicked()
    };
    if button(ui, 120.0, &format!("Apply {total} hits"), true, total > 0) {
        asks.push(Ask::Apply);
    }
    if button(ui, 70.0, "Cancel", false, true) {
        asks.push(Ask::Cancel);
    }
    if button(ui, 64.0, "Clear", false, total > 0) {
        asks.push(Ask::Clear);
    }
    if button(ui, 74.0, "Best 16", false, true) {
        asks.push(Ask::Best);
    }
    let hint = "Click a hit to hear it · ✓ keeps it (four a layer: its round-robins) · drag it to another layer";
    p.text(pos2(head.max.x - 8.0, head.max.y + 6.0), Align2::RIGHT_CENTER, hint, regular(10.0), DIM);
    // The bands, hardest on top as in the pad editor.
    let body = Rect::from_min_max(pos2(r.min.x + 6.0, head.max.y + 16.0), pos2(r.max.x - 6.0, r.max.y - 6.0));
    let row_h = (body.height() / LAYERS as f32).max(40.0);
    for row in 0..LAYERS {
        let band = LAYERS - 1 - row;
        let rr = Rect::from_min_size(pos2(body.min.x, body.min.y + row as f32 * row_h), vec2(body.width(), row_h - 6.0));
        band_row(ui, rr, picker, band, playing, asks);
    }
}

fn band_row(ui: &mut Ui, r: Rect, picker: &HitPicker, band: usize, playing: Option<usize>, asks: &mut Vec<Ask>) {
    let color = BAND_COLORS.get(band).copied().unwrap_or(AMBER);
    let name = BANDS.get(band).copied().unwrap_or_default();
    let p = ui.painter().clone();
    // A drop here moves the dragged hit to this band.
    let zone = ui.interact(r, egui::Id::new(("hit-band", band)), Sense::hover());
    let dragging = egui::DragAndDrop::has_payload_of_type::<HitDrag>(ui.ctx());
    let over = dragging && zone.contains_pointer();
    p.rect_filled(r, 5.0, if over { color.gamma_multiply(0.35) } else { color.gamma_multiply(0.12) });
    p.rect_stroke(r, 5.0, Stroke::new(if over { 2.0 } else { 1.0 }, color.gamma_multiply(0.7)), StrokeKind::Inside);
    if let Some(d) = zone.dnd_release_payload::<HitDrag>() {
        asks.push(Ask::Move(d.0, band));
    }
    // Its label: name, kept count.
    let label = Rect::from_min_size(r.min, vec2(84.0, r.height()));
    p.rect_filled(Rect::from_min_size(label.min + vec2(8.0, 10.0), vec2(6.0, label.height() - 20.0)), 2.0, color);
    p.text(pos2(label.min.x + 20.0, label.center().y - 9.0), Align2::LEFT_CENTER, name, bold(13.0), color);
    let kept = picker.kept(band);
    let count = format!("{kept}/{ROBINS} kept");
    p.text(pos2(label.min.x + 20.0, label.center().y + 9.0), Align2::LEFT_CENTER, count, regular(10.5), if kept == ROBINS { LABEL } else { DIM });
    // Its hits, softest first, in a strip that scrolls.
    let mut hits: Vec<&PickHit> = picker.hits.iter().filter(|h| h.band == band).collect();
    hits.sort_by(|a, b| a.db.total_cmp(&b.db));
    let strip = Rect::from_min_max(pos2(label.max.x, r.min.y + 4.0), pos2(r.max.x - 4.0, r.max.y - 4.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(strip).id_salt(("hit-strip", band)));
    egui::ScrollArea::horizontal().id_salt(("hit-scroll", band)).auto_shrink([false, false]).show(&mut child, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if hits.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("No hits here: drag one in").color(DIM).size(11.0));
            }
            let mut robin = 0;
            for h in hits {
                if h.keep {
                    robin += 1;
                }
                tile(ui, strip.height(), h, color, if h.keep { Some(robin) } else { None }, playing == Some(h.hit), asks);
            }
        });
    });
}

/// A hit's tile: its outline in the band's colour, its loudness and note; filled when kept, with
/// its round-robin number.
fn tile(ui: &mut Ui, h: f32, hit: &PickHit, color: Color32, robin: Option<usize>, playing: bool, asks: &mut Vec<Ask>) {
    let (r, resp) = ui.allocate_exact_size(vec2(84.0, h), Sense::click_and_drag());
    let label = format!("Hit {} {:.1} dB{}", hit.hit + 1, hit.db, if hit.keep { ", kept" } else { "" });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    if resp.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), HitDrag(hit.hit));
    }
    let p = ui.painter();
    let fill = if hit.keep { color.gamma_multiply(0.45) } else { TILE };
    p.rect_filled(r, 5.0, fill);
    let stroke = if playing {
        Stroke::new(2.0, Color32::WHITE)
    } else if hit.keep {
        Stroke::new(2.0, color)
    } else if resp.hovered() {
        Stroke::new(1.0, color.gamma_multiply(0.8))
    } else {
        Stroke::new(1.0, EDGE)
    };
    p.rect_stroke(r, 5.0, stroke, StrokeKind::Inside);
    // Outline.
    let wave = Rect::from_min_max(pos2(r.min.x + 5.0, r.min.y + 22.0), pos2(r.max.x - 5.0, r.max.y - 18.0));
    if wave.height() > 4.0 && !hit.env.is_empty() {
        let n = hit.env.len() as f32;
        let w = wave.width() / n;
        for (i, v) in hit.env.iter().enumerate() {
            let x = wave.min.x + (i as f32 + 0.5) * w;
            let a = v.clamp(0.0, 1.0) * wave.height() * 0.5;
            p.line_segment(
                [pos2(x, wave.center().y - a), pos2(x, wave.center().y + a.max(0.5))],
                Stroke::new(w.max(1.0) * 0.8, if hit.keep { Color32::from_white_alpha(220) } else { color }),
            );
        }
    }
    // Keep box (top left), round-robin number, note badge.
    let check = Rect::from_min_size(r.min + vec2(4.0, 4.0), vec2(15.0, 15.0));
    let cr = ui.interact(check, resp.id.with("keep"), Sense::click());
    let keep_label = format!("Keep hit {}", hit.hit + 1);
    cr.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, hit.keep, &keep_label));
    p.rect_filled(check, 3.0, if hit.keep { color } else { Color32::from_black_alpha(80) });
    p.rect_stroke(check, 3.0, Stroke::new(1.0, if cr.hovered() { Color32::WHITE } else { color }), StrokeKind::Inside);
    if hit.keep {
        p.text(check.center(), Align2::CENTER_CENTER, "✓", bold(11.0), DARK_TEXT);
    }
    if let Some(n) = robin {
        p.text(pos2(check.max.x + 4.0, check.center().y), Align2::LEFT_CENTER, format!("RR{n}"), bold(9.5), Color32::WHITE);
    }
    if let Some(note) = &hit.note {
        let c = if hit.off_pitch { OFF } else { DIM };
        let t = if hit.off_pitch { format!("{note}≠") } else { note.clone() };
        p.text(pos2(r.max.x - 5.0, check.center().y), Align2::RIGHT_CENTER, t, regular(9.5), c);
    }
    p.text(pos2(r.center().x, r.max.y - 9.0), Align2::CENTER_CENTER, format!("{:.1} dB · {:.0} ms", hit.db, hit.ms), regular(9.5), LABEL);
    let tip = if hit.off_pitch { "\nRings at another note than the drum's: it would sound out of tune among the others" } else { "" };
    let resp = resp.on_hover_text(format!("Click to hear it · drag to another layer{tip}"));
    if cr.clicked() {
        asks.push(Ask::Toggle(hit.hit));
    } else if resp.clicked() {
        asks.push(Ask::Play(hit.hit));
    }
}

/// Carries out what the picker asked: plays, keeps, moves, applies or closes.
pub fn apply(app: &mut SoundApp, asks: Vec<Ask>) {
    for ask in asks {
        let Some(picker) = app.ui.hit_picker.as_mut() else { return };
        match ask {
            Ask::Play(i) => {
                let audio =
                    app.engine.take_hits.as_ref().filter(|t| t.id == picker.take).and_then(|t| t.hits.get(i).map(|h| (h.channels.clone(), t.rate)));
                if let (Some((ch, rate)), Some(player)) = (audio, &app.player) {
                    player.preview(ch, rate);
                }
                app.ui.hit_playing = Some(i);
            }
            Ask::Toggle(i) => {
                let Some(band) = picker.hits.iter().find(|h| h.hit == i).map(|h| (h.band, h.keep)) else { continue };
                if !band.1 && picker.kept(band.0) >= ROBINS {
                    app.ui.status = format!("{} holds four hits: untick one first", BANDS.get(band.0).copied().unwrap_or_default());
                    continue;
                }
                if let Some(h) = picker.hits.iter_mut().find(|h| h.hit == i) {
                    h.keep = !h.keep;
                }
            }
            Ask::Move(i, band) => {
                let full = picker.kept(band) >= ROBINS;
                if let Some(h) = picker.hits.iter_mut().find(|h| h.hit == i)
                    && h.band != band
                {
                    h.band = band;
                    // A full band takes it unkept.
                    if h.keep && full {
                        h.keep = false;
                    }
                }
            }
            Ask::Best => {
                for h in &mut picker.hits {
                    (h.band, h.keep) = h.auto;
                }
            }
            Ask::Clear => {
                for h in &mut picker.hits {
                    h.keep = false;
                }
            }
            Ask::Cancel => {
                app.ui.hit_picker = None;
                stop_preview(app);
            }
            Ask::Apply => {
                let (take, pad, track, picks) = (picker.take, picker.pad, picker.track, picker.picks());
                let r = app.run("mix.kit_pad_pick", json!({"take": take, "picks": picks}));
                let ok = r.is_ok();
                app.ui.status = crate::juke_window::import_status(r);
                if ok {
                    app.ui.hit_picker = None;
                    stop_preview(app);
                    app.ui.juke_tabs.insert(track, crate::juke_window::Tab::Pad);
                    app.ui.juke_pad_select = Some((track, pad));
                }
            }
        }
    }
}

fn stop_preview(app: &mut SoundApp) {
    app.ui.hit_playing = None;
    if let Some(p) = &app.player {
        p.preview(Vec::new(), 48_000);
    }
}

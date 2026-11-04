//! The Vocal Studio's Make harmony… form (spec 2026-11-03-vocal-studio-design.md §3, mockup
//! callout 3): the interval and direction, Stay in key, Follow chords, the range, the voice's
//! formant shift, the destination (a new track or an existing one), Preview (the planned notes as
//! dashed green outlines in the roll) and Create (`vocal.harmony`, one undo step). Reachable as
//! `ui.vocal_harmony`.

use super::layout::RollView;
use super::{hook, store};
use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Color32, Painter, Rect, Shape, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_engine::vocal_studio::{HarmonyNote, harmony_plan, midi_name};
use soundcraft_model::{TrackId, TrackKind};
use soundcraft_vocal::harmony::{HarmonyRule, Interval};
use std::sync::Arc;

/// The preview outline's colour (the mockup's dashed green).
pub const PREVIEW_GREEN: Color32 = Color32::from_rgb(72, 200, 150);

/// The form's choices (kept while the studio is open).
#[derive(Debug, Clone, PartialEq)]
pub struct HarmonyForm {
    pub interval: Interval,
    pub up: bool,
    pub stay_in_key: bool,
    pub follow_chords: bool,
    /// The lowest and highest note (MIDI).
    pub range: (i32, i32),
    /// The harmony's formant shift (semitones).
    pub formant: f32,
    /// An existing track to put it on (`None`: a new track).
    pub into: Option<TrackId>,
    /// The planned notes show in the roll.
    pub preview: bool,
}

impl Default for HarmonyForm {
    fn default() -> Self {
        Self { interval: Interval::Third, up: true, stay_in_key: true, follow_chords: true, range: (47, 81), formant: 0.0, into: None, preview: true }
    }
}

impl HarmonyForm {
    pub fn rule(&self) -> HarmonyRule {
        HarmonyRule { interval: self.interval, up: self.up, stay_in_key: self.stay_in_key, follow_chords: self.follow_chords, range: self.range }
    }

    /// The `vocal.harmony` params for `source`.
    pub fn params(&self, source: TrackId) -> Value {
        let mut p = json!({
            "source": source.0,
            "interval": self.interval.name(),
            "direction": if self.up { "up" } else { "down" },
            "stay_in_key": self.stay_in_key,
            "follow_chords": self.follow_chords,
            "range": [self.range.0, self.range.1],
            "formant": self.formant,
        });
        if let Some(t) = self.into {
            p["into"] = json!(t.0);
        }
        p
    }
}

/// The track harmonised: the studio's compared track (the lead).
pub fn source(app: &mut SoundApp) -> Option<TrackId> {
    super::studio::studied(app).against
}

/// The planned notes for the open form (cached per session state and form).
pub fn plan(app: &mut SoundApp) -> Option<Arc<Vec<HarmonyNote>>> {
    let form = app.vocal.studio.harmony.clone()?;
    let src = source(app)?;
    let s = app.engine.session_arc();
    let key = (app.engine.revision, Arc::as_ptr(&s.vocal.analyses) as usize, src, form.clone());
    if let Some((k, v)) = &app.vocal.studio.harmony_plan
        && *k == key
    {
        return Some(Arc::clone(v));
    }
    let v = Arc::new(harmony_plan(&s, src, &form.rule()));
    app.vocal.studio.harmony_plan = Some((key, Arc::clone(&v)));
    Some(v)
}

/// The preview: each planned note as a dashed green outline where the harmony will sing.
pub fn draw_preview(app: &mut SoundApp, ui: &Ui, painter: &Painter, v: &RollView) {
    let shown = app.vocal.studio.harmony.as_ref().is_some_and(|f| f.preview);
    let mut rects = Vec::new();
    if shown && let Some(plan) = plan(app) {
        let h = (v.row_h * 0.7).clamp(6.0, 18.0);
        for n in plan.iter().filter(|n| n.end > v.t0 && n.start < v.t1) {
            let cents = n.pitch + (n.to - n.from) as f32 * 100.0;
            let (x0, x1) = (v.x(n.start), v.x(n.end).max(v.x(n.start) + 4.0));
            let r = Rect::from_min_max(pos2(x0, v.y(cents) - h / 2.0), pos2(x1, v.y(cents) + h / 2.0));
            let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            painter.extend(Shape::dashed_line(&pts, Stroke::new(1.5, PREVIEW_GREEN), 5.0, 3.0));
            rects.push(r);
        }
    }
    hook(ui.ctx(), "harmony-preview-notes", rects);
}

/// A note number as the form shows it ("B3").
fn note(m: f64) -> String {
    midi_name(m.round() as i32)
}

/// A toggle-looking button.
fn pill(ui: &mut Ui, on: bool, text: &str) -> egui::Response {
    let t = Tokens::current();
    ui.add(egui::Button::new(egui::RichText::new(text).font(regular(12.0)).color(if on { t.mode_on_text } else { t.text })).fill(if on {
        Color32::from_rgb(43, 90, 138)
    } else {
        t.button
    }))
}

/// The form, floating at the roll's top right (mockup callout 3).
pub fn dialog(app: &mut SoundApp, ui: &mut Ui, roll: Rect) {
    let Some(mut form) = app.vocal.studio.harmony.clone() else { return };
    let Some(src) = source(app) else { return };
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let src_name = s.track(src).map_or(String::new(), |tr| tr.name.clone());
    let w = 300.0;
    let at = pos2((roll.right() - w - 12.0).max(roll.left()), roll.top() + 10.0);
    let mut create = false;
    let mut close = false;
    let area = egui::Area::new(egui::Id::new("vocal-harmony-form")).order(egui::Order::Foreground).fixed_pos(at).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).fill(t.panel_bg2).stroke(Stroke::new(1.0, Color32::from_rgb(58, 92, 130))).show(ui, |ui| {
            ui.set_width(w - 16.0);
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Make harmony from {src_name}")).font(bold(13.0)).color(t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("\u{d7}").on_hover_text("Close").clicked() {
                        close = true;
                    }
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("Interval").color(t.text_dim));
                for iv in [Interval::Third, Interval::Fifth, Interval::Sixth, Interval::Octave] {
                    if pill(ui, form.interval == iv, iv.name()).clicked() {
                        form.interval = iv;
                    }
                }
                for (up, text) in [(true, "\u{2191} up"), (false, "\u{2193} down")] {
                    let r = pill(ui, form.up == up, text);
                    store(ui.ctx(), if up { "harmony-up" } else { "harmony-down" }, r.rect);
                    if r.clicked() {
                        form.up = up;
                    }
                }
            });
            let key = if soundcraft_engine::vocal::has_key(&s) {
                let (k, sc) = soundcraft_engine::vocal::key_pc_at(&s, app.position());
                super::roll::key_name(k, sc)
            } else {
                "no key set".into()
            };
            ui.checkbox(&mut form.stay_in_key, format!("Stay in key ({key})"));
            ui.checkbox(&mut form.follow_chords, "Follow chords");
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Range").color(t.text_dim));
                let (mut lo, mut hi) = (f64::from(form.range.0), f64::from(form.range.1));
                ui.add(egui::DragValue::new(&mut lo).range(0.0..=127.0).speed(0.1).custom_formatter(|v, _| note(v)));
                ui.label("\u{2013}");
                ui.add(egui::DragValue::new(&mut hi).range(0.0..=127.0).speed(0.1).custom_formatter(|v, _| note(v)));
                form.range = (lo.round() as i32, hi.round() as i32);
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Voice").color(t.text_dim));
                ui.label(egui::RichText::new(format!("{src_name}'s, formant")).color(t.text));
                ui.add(
                    egui::DragValue::new(&mut form.formant)
                        .range(-12.0..=12.0)
                        .speed(0.05)
                        .custom_formatter(|v, _| super::panel::formant_text(v as f32)),
                )
                .on_hover_text("Shift the harmony's formants so it sounds like another singer (0: the source's own voice)");
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Into").color(t.text_dim));
                if pill(ui, form.into.is_none(), "New track").clicked() {
                    form.into = None;
                }
                let others: Vec<(TrackId, String)> =
                    s.tracks.iter().filter(|tr| tr.kind == TrackKind::Audio && tr.id != src).map(|tr| (tr.id, tr.name.clone())).collect();
                let label = form.into.and_then(|i| s.track(i)).map_or("Existing\u{2026}".to_string(), |tr| tr.name.clone());
                ui.menu_button(label, |ui| {
                    for (id, n) in &others {
                        if ui.selectable_label(form.into == Some(*id), n).clicked() {
                            form.into = Some(*id);
                            ui.close();
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                let pv = pill(ui, form.preview, "Preview");
                store(ui.ctx(), "harmony-preview", pv.rect);
                if pv.on_hover_text("Show the harmony's notes as dashed outlines in the roll").clicked() {
                    form.preview = !form.preview;
                }
                let cr = ui.add(
                    egui::Button::new(egui::RichText::new("Create").font(bold(12.5)).color(Color32::WHITE)).fill(Color32::from_rgb(43, 90, 138)),
                );
                store(ui.ctx(), "harmony-create", cr.rect);
                if cr.on_hover_text("Make the harmony part (one undo step)").clicked() {
                    create = true;
                }
            });
        });
    });
    store(ui.ctx(), "harmony-form", area.response.rect);
    if close {
        app.vocal.studio.harmony = None;
        return;
    }
    if app.vocal.studio.harmony.as_ref() != Some(&form) {
        app.vocal.studio.harmony = Some(form);
    }
    if create && let Err(e) = app.run("ui.vocal_harmony", json!({"create": true})) {
        app.ui.status = e;
    }
}

/// `ui.vocal_harmony {open?, interval?, direction?, stay_in_key?, follow_chords?, range?,
/// formant?, into?, preview?, create?}`: opens the Make harmony… form in the Vocal Studio (turning
/// the studio on), sets its choices, and with `create: true` makes the harmony (`vocal.harmony`)
/// and edits the new part. Returns the form and the planned notes (or what was made).
pub fn command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let open = p.get("open").and_then(Value::as_bool).unwrap_or(true);
    if !open {
        app.vocal.studio.harmony = None;
        return Ok(json!({"open": false}));
    }
    if !app.vocal.studio.on {
        super::studio::command(app, &json!({"value": true}))?;
    }
    let mut form = app.vocal.studio.harmony.clone().unwrap_or_default();
    if let Some(v) = p.get("interval") {
        form.interval = v.as_str().and_then(Interval::parse).ok_or("`interval` must be 3rd, 5th, 6th or octave")?;
    }
    if let Some(v) = p.get("direction") {
        form.up = match v.as_str() {
            Some("up") => true,
            Some("down") => false,
            _ => return Err("`direction` must be up or down".into()),
        };
    }
    for (k, dst) in [("stay_in_key", &mut form.stay_in_key), ("follow_chords", &mut form.follow_chords), ("preview", &mut form.preview)] {
        if let Some(v) = p.get(k) {
            *dst = v.as_bool().ok_or(format!("`{k}` must be true or false"))?;
        }
    }
    if let Some(v) = p.get("range") {
        let a = v.as_array().filter(|a| a.len() == 2).and_then(|a| Some((a.first()?.as_i64()?, a.get(1)?.as_i64()?)));
        let (lo, hi) = a.ok_or("`range` must be [low, high] note numbers")?;
        let c = |m: i64| i32::try_from(m.clamp(0, 127)).unwrap_or(60);
        form.range = (c(lo.min(hi)), c(lo.max(hi)));
    }
    if let Some(v) = p.get("formant") {
        form.formant = v.as_f64().filter(|f| f.is_finite() && f.abs() <= 12.0).ok_or("`formant` must be semitones from -12 to 12")? as f32;
    }
    if let Some(v) = p.get("into") {
        form.into = match v {
            Value::Null => None,
            _ => {
                let s = app.engine.session();
                let t =
                    v.as_u64().map(TrackId).filter(|t| s.track(*t).is_some()).or_else(|| v.as_str().and_then(|n| s.track_by_name(n)).map(|t| t.id));
                Some(t.ok_or("`into` must be a track id or name (null: a new track)")?)
            }
        };
    }
    app.vocal.studio.harmony = Some(form.clone());
    let src = source(app).ok_or("no vocal track to harmonise")?;
    if p.get("create").and_then(Value::as_bool).unwrap_or(false) {
        let made = app.run("vocal.harmony", form.params(src))?;
        app.vocal.studio.harmony = None;
        if let Some(t) = made.get("track").and_then(Value::as_u64) {
            super::studio::set_editing(app, TrackId(t));
        }
        app.ui.status = format!("Made {} ({} notes)", made["name"].as_str().unwrap_or("the harmony"), made["notes"]);
        return Ok(made);
    }
    let planned: Vec<Value> =
        plan(app).map_or(Vec::new(), |v| v.iter().map(|n| json!({"clip": n.r.clip, "note": n.r.note, "from": n.from, "to": n.to})).collect());
    let mut out = form.params(src);
    out["open"] = json!(true);
    out["preview"] = json!(form.preview);
    out["notes"] = json!(planned);
    Ok(out)
}

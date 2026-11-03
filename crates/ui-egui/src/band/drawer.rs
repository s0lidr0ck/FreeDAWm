//! The band drawer: one musician's controls, opened sideways right of a musician strip in the
//! Mix window, as a module's drawer opens (`module_drawer`).
//!
//! Top to bottom: Enabled and Mute; Instrument and Player (a persona browser grouped by
//! instrument, with a search box); Energy; the Swing, Humanize, Dynamics, Fills and (Natural feel
//! only) Groove dials; Respect chords; Octave, Length and Velocity; Speed; Voice; the band's Take
//! (MIDI Studio has no per-musician take); the Target (Auto and what it picked, or a name); then
//! the rows the target's driver needs, as MIDI Studio's `MusicianTab` shows them.
//!
//! Every edit is `band.slot` (the Take `band.settings`); a drag is one undo step.

use super::{instrument_name, player_row, slot_player, tags_tip};
use crate::SoundApp;
use crate::editor_kit::VstPalette;
use crate::editor_kit::controls::{
    DialOpts, LabelLayout, PillLook, PillStyle, SelectionStyle, ValueBoxState, ValueSpec, dial, pill_ex, segmented_ex, value_box,
};
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_band::library::{Driver, INSTRUMENT_NAMES, K_TARGET_SLOTS, LibraryData, TargetData, ample_mixed, ample_strums, auto_target};
use soundcraft_model::{MusicianSlot, Track, TrackId};

/// Width of the drawer column (a module drawer's).
pub const BAND_DRAWER_W: f32 = crate::module_drawer::DRAWER_W;
/// The band's accent: the drawer's edge, the lit controls and the strip's Band key.
pub const BAND_ACCENT: Color32 = Color32::from_rgb(0x9A, 0x86, 0xFF);

/// Inner margin of the drawer card.
const PAD: f32 = 10.0;
/// A caption above a control, and the gap after a row.
const CAPTION_H: f32 = 14.0;
const GAP: f32 = 8.0;
/// A field, a segmented switch, a pill.
const FIELD_H: f32 = 22.0;
/// A dial's cell (knob, readout and name) and a value box.
const DIAL_H: f32 = 76.0;
const BOX_H: f32 = 36.0;

/// MIDI Studio's SWAM solo instruments (`swaminst`), in order.
const SWAM: [&str; 13] =
    ["Violin", "Viola", "Cello", "Double Bass", "Trumpet", "Trombone", "Horn", "Tuba", "Flute", "Clarinet", "Oboe", "Bassoon", "Alto Sax"];
/// The Drums instrument (`INSTRUMENT_NAMES`): its octave is fixed, as in MIDI Studio.
const DRUMS: u8 = 5;
const _: () = assert!(matches!(INSTRUMENT_NAMES[DRUMS as usize].as_bytes(), b"Drums"));
/// The four articulation keys' names (`kArtKeys`): C0 to D♯1.
const ART_KEYS: [&str; 16] = ["C0", "C♯0", "D0", "D♯0", "E0", "F0", "F♯0", "G0", "G♯0", "A0", "A♯0", "B0", "C1", "C♯1", "D1", "D♯1"];

/// A row the target's driver adds (MIDI Studio `MusicianTab`'s per-driver rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawerRow {
    /// Keyswitch octave (`ksoctave`).
    KsOctave,
    /// The four articulation keys: long, short, pluck, tremolo (`art`).
    ArtKeys,
    /// What an Ample guitar plays: the player's choice, Strummer or notes (`ample`).
    Ample,
    /// How Ample finds its chords: Detect or Slots (`amplechords`).
    AmpleChords,
    /// The SWAM solo instrument (`swaminst`).
    SwamInstrument,
    /// A plucked library's chords: its own strums or the library's chord keys (`pluckchords`).
    PluckChords,
}

/// The rows a driver can add, in MIDI Studio's order.
pub fn driver_rows(d: Driver) -> &'static [DrawerRow] {
    use DrawerRow::*;
    match d {
        Driver::KeyswitchBass | Driver::Keyswitch | Driver::UjamPhrase => &[KsOctave],
        Driver::Swam => &[KsOctave, SwamInstrument],
        Driver::ArticulationKeys => &[KsOctave, ArtKeys],
        Driver::AmpleGuitar => &[Ample, AmpleChords],
        Driver::IndiginusStrings => &[PluckChords],
        _ => &[],
    }
}

/// The rows a target shows: its driver's, less those it doesn't use (`MusicianTab::refresh`):
/// SWAM's keyswitch octave only with SWAM's keyswitches, the articulation keys only for a target
/// with its own, plucked chords only with chord keys and not The Banjo's layout.
pub fn target_rows(t: &TargetData) -> Vec<DrawerRow> {
    driver_rows(t.driver)
        .iter()
        .copied()
        .filter(|r| match r {
            DrawerRow::KsOctave => t.driver != Driver::Swam || t.swam_keyswitches,
            DrawerRow::ArtKeys => t.art_custom_keys && t.art_keys.iter().any(|k| *k >= 0),
            DrawerRow::PluckChords => t.in_chord_down >= 0 && t.in_layout != 1,
            _ => true,
        })
        .collect()
}

/// The name of the instrument plugin a track plays (its instrument slot, else the first instrument
/// in its chain); empty when there is none.
pub fn loaded_instrument(track: &Track) -> String {
    let instrument = |plugin: &str| crate::mix_window::plugin_info(plugin).filter(|p| p.is_instrument);
    track.instrument.iter().chain(track.mixer.inserts.iter()).find_map(|i| instrument(&i.plugin)).map(|p| p.name.to_string()).unwrap_or_default()
}

/// The target slot a musician plays on this track: "auto" picks by the loaded instrument's name;
/// a name the Library lacks is `None`.
pub fn target_slot(lib: &LibraryData, m: &MusicianSlot, track: &Track) -> Option<usize> {
    if is_auto(m) {
        return Some(auto_target(lib, &loaded_instrument(track)));
    }
    usize::try_from(lib.target_slot_for_name(&m.target)).ok()
}

fn is_auto(m: &MusicianSlot) -> bool {
    m.target.trim().is_empty() || m.target.eq_ignore_ascii_case("auto")
}

/// The Target field's text: "Auto → MODO DRUM", or the chosen name.
fn target_text(lib: &LibraryData, m: &MusicianSlot, track: &Track) -> String {
    if is_auto(m) {
        let name = target_slot(lib, m, track).and_then(|s| lib.target(s as i32)).map_or("Generic", |t| t.name.as_str());
        format!("Auto → {name}")
    } else if lib.target_slot_for_name(&m.target) < 0 {
        format!("{} (missing)", m.target)
    } else {
        m.target.clone()
    }
}

/// The kit's palette drawn from the DAW theme, so the drawer reads as a module drawer in both
/// themes.
pub(super) fn palette() -> VstPalette {
    let t = Tokens::current();
    VstPalette {
        accent: BAND_ACCENT,
        background: t.module_tile_bg,
        panel: t.graph_bg,
        panel_alt: t.graph_bg,
        border: t.module_tile_border,
        border_hover: BAND_ACCENT.gamma_multiply(0.8),
        text_bright: t.text,
        text_body: t.text_dim,
        text_dim: t.text_dim,
        text_label: t.text_dim,
        text_faint: t.text_dim,
        on_accent: Color32::from_rgb(0x16, 0x17, 0x1A),
        field: t.graph_bg,
        well: t.graph_bg,
        ..VstPalette::THEME_DEFAULTS
    }
}

/// What every row needs: which musician, and the palette.
struct Ctx {
    track: TrackId,
    /// 0-5.
    slot: usize,
    pal: VstPalette,
}

impl Ctx {
    fn params(&self, key: &str, v: Value) -> Value {
        let mut p = json!({"slot": self.slot + 1});
        if let Some(o) = p.as_object_mut() {
            o.insert(key.to_string(), v);
        }
        p
    }

    /// `band.slot {key: v}`, one undo step.
    fn set(&self, app: &mut SoundApp, key: &str, v: Value) {
        let r = app.run("band.slot", self.params(key, v));
        report(app, r);
    }

    /// `band.slot {key: v}` merged into the open drag on `key`: one undo step until it ends.
    fn set_merged(&self, app: &mut SoundApp, key: &str, v: Value) {
        let merge = format!("band:{}:{}:{key}", self.track.0, self.slot);
        let r = app.engine.execute_merged("band.slot", &self.params(key, v), &merge).map_err(|e| e.to_string());
        report(app, r);
    }

    /// A gesture's edit: a new one closes any open merge, the change joins it, the end closes it.
    fn gesture(&self, app: &mut SoundApp, key: &str, v: Option<Value>, started: bool, ended: bool) {
        if started {
            app.engine.end_merge();
        }
        if let Some(v) = v {
            self.set_merged(app, key, v);
        }
        if ended {
            app.engine.end_merge();
        }
    }
}

fn report(app: &mut SoundApp, r: Result<Value, String>) {
    if let Err(e) = r {
        app.ui.status = e;
    }
}

/// The drawer of musician `slot` (0-5) for `track`, a column [`BAND_DRAWER_W`] wide and as tall
/// as the strips.
pub fn drawer(app: &mut SoundApp, ui: &mut Ui, track: &Track, slot: u8) {
    let t = Tokens::current();
    let h = ui.available_height().max(560.0);
    let (r, _) = ui.allocate_exact_size(vec2(BAND_DRAWER_W, h), Sense::hover());
    let session = app.engine.session_arc();
    let Some(m) = session.band.slots.get(usize::from(slot)) else { return };
    let lib = app.engine.band_library();
    let natural = session.band.feel.natural;
    let take = session.band.take;

    let mut ui = ui.new_child(UiBuilder::new().id_salt(("band-drawer", track.id.0)).max_rect(r));
    ui.set_clip_rect(r.intersect(ui.clip_rect()));
    let ui = &mut ui;
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

    // The column, then a card lit in the band's accent, as a module drawer's.
    crate::widgets::vgradient(ui.painter(), r, crate::widgets::shade(t.strip_bg, 0.06), t.strip_bg);
    let card = r.shrink2(vec2(4.0, 4.0));
    let p = ui.painter().clone();
    p.rect_filled(card, CornerRadius::same(8), t.module_tile_bg);
    p.rect_stroke(card, CornerRadius::same(8), Stroke::new(1.5, BAND_ACCENT.gamma_multiply(0.85)), StrokeKind::Inside);
    let wash = Rect::from_min_size(card.min + vec2(1.5, 1.5), vec2(card.width() - 3.0, 46.0));
    crate::widgets::vgradient(&p, wash, BAND_ACCENT.gamma_multiply(0.12), BAND_ACCENT.gamma_multiply(0.0));

    let inner = card.shrink(PAD);
    let y = header(app, ui, track, usize::from(slot), m, inner);
    let body = Rect::from_x_y_ranges(inner.x_range(), (y + 8.0)..=inner.max.y);
    let mut body_ui = ui.new_child(UiBuilder::new().id_salt("band-body").max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    body_ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let ctx = Ctx { track: track.id, slot: usize::from(slot), pal: palette() };
    let boxes_id = Id::new(("band-boxes", track.id.0));
    let mut boxes: ValueBoxState = ui.data(|d| d.get_temp(boxes_id)).unwrap_or_default();
    egui::ScrollArea::vertical().id_salt(("band-scroll", track.id.0)).auto_shrink([false, false]).show(&mut body_ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let mut b = Body { app, ui, ctx: &ctx, boxes: &mut boxes, w: inner.width() };
        b.rows(m, &lib, track, natural, take);
    });
    ui.data_mut(|d| d.insert_temp(boxes_id, boxes));
}

/// Track · musician, the player and instrument, and the fold button. Returns the y under it.
fn header(app: &mut SoundApp, ui: &mut Ui, track: &Track, slot: usize, m: &MusicianSlot, inner: Rect) -> f32 {
    let t = Tokens::current();
    let p = ui.painter().clone();
    let y = inner.min.y;
    let fold = Rect::from_min_size(pos2(inner.max.x - 20.0, y), vec2(20.0, 20.0));
    let fresp = ui.interact(fold, ui.id().with("band-fold"), Sense::click());
    fresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Fold the band drawer"));
    if fresp.hovered() {
        p.rect_filled(fold, CornerRadius::same(5), t.text.gamma_multiply(0.08));
    }
    let c = fold.center();
    let xc = if fresp.hovered() { t.text } else { t.text_dim };
    p.line_segment([c - vec2(4.0, 4.0), c + vec2(4.0, 4.0)], Stroke::new(1.5, xc));
    p.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], Stroke::new(1.5, xc));
    if fresp.on_hover_text("Fold the band drawer").clicked() {
        app.ui.band_drawers.remove(&track.id);
    }

    let mut job = egui::text::LayoutJob::default();
    let fmt = |c: Color32| egui::TextFormat { font_id: bold(11.0), color: c, extra_letter_spacing: 0.7, ..Default::default() };
    job.append(&track.name.to_uppercase(), 0.0, fmt(t.text_dim));
    job.append(" · ", 0.0, fmt(t.text_dim));
    job.append(&format!("MUSICIAN {}", slot + 1), 0.0, fmt(t.text));
    let g = p.layout_job(job);
    let title_clip = Rect::from_min_max(pos2(inner.min.x, y), pos2(fold.min.x - 4.0, y + 20.0));
    p.with_clip_rect(title_clip).galley(pos2(inner.min.x, y + 1.0), g, t.text);
    let state = match (m.enabled, m.mute) {
        (false, _) => " · off",
        (true, true) => " · muted",
        _ => "",
    };
    let sub = format!("{} · {}{state}", slot_player(m), instrument_name(m.instrument));
    p.with_clip_rect(title_clip.translate(vec2(0.0, 14.0))).text(pos2(inner.min.x, y + 16.0), Align2::LEFT_TOP, sub, regular(9.5), t.text_dim);
    y + 34.0
}

/// The drawer's rows, laid out top to bottom in a scrolling column.
struct Body<'a, 'u> {
    app: &'a mut SoundApp,
    ui: &'u mut Ui,
    ctx: &'a Ctx,
    boxes: &'a mut ValueBoxState,
    w: f32,
}

impl Body<'_, '_> {
    /// The next `h` points of the column.
    fn take(&mut self, h: f32) -> Rect {
        self.ui.allocate_exact_size(vec2(self.w, h), Sense::hover()).0
    }

    fn gap(&mut self, h: f32) {
        self.ui.add_space(h);
    }

    fn caption(&self, r: Rect, text: &str) {
        let t = Tokens::current();
        self.ui.painter().with_clip_rect(r).text(pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, text, regular(10.0), t.text_dim);
    }

    /// A hairline across the card.
    fn divider(&mut self) {
        let r = self.take(9.0);
        self.ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, Tokens::current().module_tile_border));
    }

    /// A segmented switch under `caption`; returns the clicked index.
    fn switch(&mut self, caption: &str, salt: &str, options: &[&str], selected: Option<usize>) -> Option<usize> {
        self.switch_noted(caption, None, salt, options, selected, false)
    }

    /// [`Self::switch`] with a note at the caption's right.
    /// `greyed`: drawn faint and inert (a row the target doesn't use).
    fn switch_noted(
        &mut self,
        caption: &str,
        note: Option<&str>,
        salt: &str,
        options: &[&str],
        selected: Option<usize>,
        greyed: bool,
    ) -> Option<usize> {
        let c = self.take(CAPTION_H);
        self.caption(c, caption);
        if let Some(note) = note {
            let t = Tokens::current();
            self.ui.painter().with_clip_rect(c).text(pos2(c.max.x, c.center().y), Align2::RIGHT_CENTER, note, regular(9.0), t.text_dim);
        }
        let r = self.take(FIELD_H);
        let mut c = self.ui.new_child(UiBuilder::new().id_salt(("band-switch", salt)).max_rect(r));
        if greyed {
            // A disabled Ui already fades it (as the greyed Octave box reads).
            c.disable();
        }
        let picked = segmented_ex(&mut c, r, salt, &self.ctx.pal, options, 1, selected, SelectionStyle::Filled, false).filter(|_| !greyed);
        self.gap(GAP);
        picked.filter(|i| Some(*i) != selected)
    }

    fn rows(&mut self, m: &MusicianSlot, lib: &LibraryData, track: &Track, natural: bool, take: u8) {
        let t = Tokens::current();
        let pal = self.ctx.pal;
        // 1. Enabled and Mute.
        let r = self.take(25.0);
        let half = (self.w - 8.0) / 2.0;
        let look = PillLook { tracked: false, ..PillLook::default() };
        let en = Rect::from_min_size(r.min, vec2(half, r.height()));
        if pill_ex(self.ui, en, "band-enabled", &pal, "Enabled", m.enabled, PillStyle::Normal, look).on_hover_text("This musician plays").clicked() {
            self.ctx.set(self.app, "enabled", json!(!m.enabled));
        }
        let mr = Rect::from_min_size(pos2(en.max.x + 8.0, r.min.y), vec2(half, r.height()));
        let mute_look = PillLook { active_colour: Some(t.mute), ..look };
        if pill_ex(self.ui, mr, "band-mute", &pal, "Mute", m.mute, PillStyle::Normal, mute_look).clicked() {
            self.ctx.set(self.app, "mute", json!(!m.mute));
        }
        self.gap(GAP + 2.0);

        // 2-3. Instrument and Player.
        let c = self.take(CAPTION_H);
        let iw = (self.w - 8.0) * 0.45;
        self.caption(Rect::from_min_size(c.min, vec2(iw, c.height())), "Instrument");
        self.caption(Rect::from_min_size(pos2(c.min.x + iw + 8.0, c.min.y), vec2(self.w - iw - 8.0, c.height())), "Player");
        let r = self.take(FIELD_H);
        let ir = Rect::from_min_size(r.min, vec2(iw, r.height()));
        let iresp = field(self.ui, ir, instrument_name(m.instrument), "Instrument");
        egui::Popup::menu(&iresp).show(|ui| {
            for (i, name) in INSTRUMENT_NAMES.iter().enumerate() {
                if ui.selectable_label(usize::from(m.instrument) == i, *name).clicked() {
                    self.ctx.set(self.app, "instrument", json!(i));
                    ui.close();
                }
            }
        });
        let pr = Rect::from_min_max(pos2(ir.max.x + 8.0, r.min.y), r.max);
        let presp = field(self.ui, pr, slot_player(m), "Player");
        egui::Popup::menu(&presp)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| player_browser(self.app, ui, self.ctx, lib, m));
        self.gap(GAP);

        // 4. Energy.
        let energy = usize::from(m.energy.clamp(1, 4)) - 1;
        if let Some(i) = self.switch("Energy", "band-energy", &["1", "2", "3", "4"], Some(energy)) {
            self.ctx.set(self.app, "energy", json!(i + 1));
        }

        // 5. The feel dials (Groove is Natural feel's).
        let mut dials: Vec<(&str, &str, u8)> = vec![
            ("Swing", "swing", m.swing),
            ("Humanize", "humanize", m.humanize),
            ("Dynamics", "dynamics", m.dynamics),
            ("Fills", "fills", m.fills),
        ];
        if natural {
            dials.push(("Groove", "groove", m.groove));
        }
        // Four to a row fit their names; five go three and two.
        let cols = if dials.len() == 4 { 4 } else { 3 };
        let rows = dials.len().div_ceil(cols);
        let r = self.take((DIAL_H + 6.0) * rows as f32 - 6.0);
        let cw = r.width() / cols as f32;
        for (i, (label, key, v)) in dials.into_iter().enumerate() {
            let at = pos2(r.min.x + cw * (i % cols) as f32, r.min.y + (DIAL_H + 6.0) * (i / cols) as f32);
            let cell = Rect::from_min_size(at, vec2(cw, DIAL_H)).shrink2(vec2(8.0, 0.0));
            let text = v.to_string();
            let o = DialOpts { label, value_text: &text, default: 0.5, layout: LabelLayout::Below, small: true, ..DialOpts::default() };
            let out = dial(self.ui, cell, ("band-dial", key), &pal, f32::from(v) / 100.0, &o);
            let value = out.value.map(|x| json!((x * 100.0).round().clamp(0.0, 100.0) as u8));
            self.ctx.gesture(self.app, key, value, out.started, out.ended);
        }
        self.gap(GAP);

        // 6. Respect chords.
        let r = self.take(25.0);
        let rr = Rect::from_min_size(r.min, vec2(half, r.height()));
        let tip = "Play only the chord's own notes";
        if pill_ex(self.ui, rr, "band-respect", &pal, "Respect chords", m.respect, PillStyle::Normal, look).on_hover_text(tip).clicked() {
            self.ctx.set(self.app, "respect", json!(!m.respect));
        }
        self.gap(GAP + 2.0);

        // 7. Octave, Length, Velocity; Speed; Voice.
        let r = self.take(BOX_H);
        let bw = (self.w - 16.0) / 3.0;
        let specs: [(ValueSpec, &str, f64, bool); 3] = [
            (spec("Octave", -2.0, 2.0, 0.05, 0.0, Some(signed)), "octave", f64::from(m.octave), m.instrument == DRUMS),
            (spec("Length", 10.0, 100.0, 0.5, 100.0, Some(percent)), "length", f64::from(m.length), false),
            (spec("Velocity", 1.0, 127.0, 0.5, 96.0, None), "velocity", f64::from(m.velocity), false),
        ];
        for (i, (s, key, v, greyed)) in specs.iter().enumerate() {
            let br = Rect::from_min_size(pos2(r.min.x + (bw + 8.0) * i as f32, r.min.y), vec2(bw, r.height()));
            let out = value_box(self.ui, self.boxes, br, ("band-box", *key), &pal, s, *v, *greyed);
            let value = out.changed.map(|x| json!(x.round() as i64));
            self.ctx.gesture(self.app, key, value, out.started, out.ended);
        }
        self.gap(GAP);
        if let Some(i) = self.switch("Speed", "band-speed", &["Half", "Normal", "Double"], Some(usize::from(m.speed.min(2)))) {
            self.ctx.set(self.app, "speed", json!(i));
        }
        // Voice is SWAM's (MS `MusicianTab`): greyed for any other target.
        let swam = target_slot(lib, m, track).and_then(|s| lib.target(s as i32)).is_some_and(|t| t.driver == Driver::Swam);
        let voices = ["Auto", "Top", "2nd", "3rd", "Bass"];
        let voice_note = if swam { None } else { Some("SWAM targets only") };
        if let Some(i) = self.switch_noted("Voice", voice_note, "band-voice", &voices, Some(usize::from(m.voice.min(4))), !swam) {
            self.ctx.set(self.app, "voice", json!(i));
        }

        // 8-9. The band's Take (MIDI Studio has no per-musician take), and the Target.
        let r = self.take(BOX_H);
        let tw = 76.0;
        let take_r = Rect::from_min_size(r.min, vec2(tw, BOX_H));
        let s = spec("Band take", 1.0, 16.0, 0.1, 1.0, None);
        let out = value_box(self.ui, self.boxes, take_r, "band-take", &pal, &s, f64::from(take), false);
        if out.started {
            self.app.engine.end_merge();
        }
        if let Some(v) = out.changed {
            let key = format!("band:{}:take", self.ctx.track.0);
            let p = json!({"take": v.round() as i64});
            let r = self.app.engine.execute_merged("band.settings", &p, &key).map_err(|e| e.to_string());
            report(self.app, r);
        }
        if out.ended {
            self.app.engine.end_merge();
        }
        let right = Rect::from_min_max(pos2(take_r.max.x + 8.0, r.min.y), r.max);
        self.caption(Rect::from_min_size(right.min, vec2(right.width(), CAPTION_H)), "Target");
        let target_r = Rect::from_min_size(pos2(right.min.x, right.min.y + CAPTION_H), vec2(right.width(), FIELD_H));
        let tresp = field(self.ui, target_r, &target_text(lib, m, track), "Target");
        let tresp = tresp.on_hover_text("The instrument this musician plays: Auto picks it by the track's instrument name");
        egui::Popup::menu(&tresp).show(|ui| target_menu(self.app, ui, self.ctx, lib, m, track));
        self.gap(GAP);

        // 10. The driver's rows.
        let target = target_slot(lib, m, track).and_then(|s| lib.target(s as i32));
        let rows = target.map(target_rows).unwrap_or_default();
        if let Some(t) = target.filter(|_| !rows.is_empty()) {
            self.divider();
            for row in rows {
                self.driver_row(row, m, lib, t);
            }
        }
    }

    fn driver_row(&mut self, row: DrawerRow, m: &MusicianSlot, lib: &LibraryData, t: &TargetData) {
        let pal = self.ctx.pal;
        match row {
            DrawerRow::KsOctave => {
                let cur = m.ksoctave.map_or(t.ks_octave_default, i32::from).clamp(-2, 2);
                let sel = usize::try_from(cur + 2).ok();
                if let Some(i) = self.switch("Keyswitch octave", "band-ksoct", &["-2", "-1", "0", "+1", "+2"], sel) {
                    self.ctx.set(self.app, "ksoctave", json!(i as i64 - 2));
                }
            }
            DrawerRow::SwamInstrument => {
                // The target's own instruments, then the current one when it belongs to another family.
                let mut items: Vec<(usize, String)> = SWAM
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| t.swam_instruments.iter().any(|s| s.name == **n))
                    .map(|(i, n)| (i, n.to_string()))
                    .collect();
                let cur = usize::from(m.swaminst).min(SWAM.len() - 1);
                if !items.iter().any(|(i, _)| *i == cur) {
                    items.push((cur, format!("{} (other family)", SWAM.get(cur).copied().unwrap_or_default())));
                }
                let c = self.take(CAPTION_H);
                self.caption(c, "SWAM instrument");
                let r = self.take(FIELD_H);
                let shown = items.iter().find(|(i, _)| *i == cur).map(|(_, n)| n.clone()).unwrap_or_default();
                let resp = field(self.ui, r, &shown, "SWAM instrument");
                egui::Popup::menu(&resp).show(|ui| {
                    for (i, name) in &items {
                        if ui.selectable_label(*i == cur, name.as_str()).clicked() {
                            self.ctx.set(self.app, "swaminst", json!(i));
                            ui.close();
                        }
                    }
                });
                self.gap(GAP);
            }
            DrawerRow::Ample => {
                let how = match lib.persona(i32::from(m.player_seed)) {
                    Some(p) if ample_mixed(p) => "by Energy",
                    Some(p) if ample_strums(p, 0, 0) => "strums",
                    _ => "plays notes",
                };
                let note = format!("player's choice: {how}");
                if let Some(i) = self.switch_noted(
                    "Ample plays",
                    Some(&note),
                    "band-ample",
                    &["Player's choice", "Strummer", "Notes"],
                    Some(usize::from(m.ample.min(2))),
                    false,
                ) {
                    self.ctx.set(self.app, "ample", json!(i));
                }
            }
            DrawerRow::AmpleChords => {
                if let Some(i) = self.switch("Ample chords", "band-amplechords", &["Detect", "Slots"], Some(usize::from(m.amplechords.min(1)))) {
                    self.ctx.set(self.app, "amplechords", json!(i));
                }
            }
            DrawerRow::PluckChords => {
                let sel = Some(usize::from(m.pluckchords.min(1)));
                if let Some(i) = self.switch("Plucked chords", "band-pluck", &["Own strums", "Library chord keys"], sel) {
                    self.ctx.set(self.app, "pluckchords", json!(i));
                }
            }
            DrawerRow::ArtKeys => {
                let c = self.take(CAPTION_H);
                self.caption(c, "Articulation keys");
                let r = self.take(BOX_H);
                let names = ["Long", "Short", "Pluck", "Tremolo"];
                let shown: Vec<usize> = (0..4).filter(|i| t.art_keys.get(*i).is_some_and(|k| *k >= 0)).collect();
                let bw = (self.w - 6.0 * 3.0) / 4.0;
                for (col, i) in shown.into_iter().enumerate() {
                    let br = Rect::from_min_size(pos2(r.min.x + (bw + 6.0) * col as f32, r.min.y), vec2(bw, r.height()));
                    let s = ValueSpec { label: names.get(i).copied().unwrap_or_default(), ..spec("", 0.0, 15.0, 0.08, 0.0, Some(art_key)) };
                    let v = m.art.get(i).copied().unwrap_or(0);
                    let out = value_box(self.ui, self.boxes, br, ("band-art", i), &pal, &s, f64::from(v), false);
                    let value = out.changed.map(|x| {
                        let mut art = m.art;
                        if let Some(a) = art.get_mut(i) {
                            *a = x.round().clamp(0.0, 15.0) as u8;
                        }
                        json!(art)
                    });
                    self.ctx.gesture(self.app, "art", value, out.started, out.ended);
                }
                self.gap(GAP);
            }
        }
    }
}

/// A value box's setup: a whole number from `lo` to `hi`, `fine` per pixel of drag.
fn spec(label: &'static str, lo: f64, hi: f64, fine: f64, default: f64, fmt: Option<fn(f64) -> String>) -> ValueSpec {
    ValueSpec { label, lo, hi, log: false, fine, decimals: 0, unit: "", default, fmt }
}

fn signed(v: f64) -> String {
    let n = v.round() as i64;
    if n > 0 { format!("+{n}") } else { n.to_string() }
}

fn percent(v: f64) -> String {
    format!("{}%", v.round() as i64)
}

fn art_key(v: f64) -> String {
    ART_KEYS.get(v.round().clamp(0.0, 15.0) as usize).copied().unwrap_or_default().to_string()
}

/// A flat menu field (a module drawer's): its text, a small arrow, `label` for assistive tech.
pub(super) fn field(ui: &mut Ui, r: Rect, text: &str, label: &str) -> egui::Response {
    let t = Tokens::current();
    let resp = ui.interact(r, ui.id().with(("band-field", label)), Sense::click());
    let name = label.to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &name));
    let p = ui.painter();
    let fill = if resp.hovered() { t.graph_bg.lerp_to_gamma(t.module_tile_border, 0.5) } else { t.graph_bg };
    p.rect_filled(r, CornerRadius::same(5), fill);
    let edge = if resp.hovered() { BAND_ACCENT.gamma_multiply(0.7) } else { t.module_tile_border };
    p.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, edge), StrokeKind::Inside);
    let text_r = r.with_max_x(r.max.x - 16.0);
    p.with_clip_rect(text_r.shrink(2.0)).text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, text, regular(10.0), t.text);
    let a = pos2(r.max.x - 10.0, r.center().y);
    p.add(Shape::convex_polygon(vec![a + vec2(-3.5, -1.5), a + vec2(3.5, -1.5), a + vec2(0.0, 2.5)], t.text_dim, Stroke::NONE));
    resp
}

/// The persona browser: every player, grouped by instrument (the musician's own first), each with
/// its style under the name and its tags as a tooltip, with a search box (names, styles and tags).
/// Picking another instrument's player moves the musician to that instrument.
fn player_browser(app: &mut SoundApp, ui: &mut Ui, ctx: &Ctx, lib: &LibraryData, m: &MusicianSlot) {
    let search_id = Id::new(("band-player-search", ctx.track.0));
    let mut query: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();
    let resp = ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search players…").desired_width(220.0));
    if resp.changed() {
        ui.data_mut(|d| d.insert_temp(search_id, query.clone()));
    }
    let q = query.trim().to_lowercase();
    let own = usize::from(m.instrument);
    let order = std::iter::once(own).chain((0..INSTRUMENT_NAMES.len()).filter(|i| *i != own));
    egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
        let mut any = false;
        for inst in order {
            let Some(seeds) = lib.players.get(inst) else { continue };
            let inst_name = INSTRUMENT_NAMES.get(inst).copied().unwrap_or_default();
            let players: Vec<&soundcraft_band::library::PersonaData> = seeds
                .iter()
                .filter_map(|s| lib.persona(*s))
                .filter(|p| {
                    q.is_empty()
                        || p.name.to_lowercase().contains(&q)
                        || p.desc.to_lowercase().contains(&q)
                        || p.tags.iter().any(|t| t.contains(&q))
                        || inst_name.to_lowercase().contains(&q)
                })
                .collect();
            if players.is_empty() {
                continue;
            }
            any = true;
            ui.add_space(2.0);
            ui.label(egui::RichText::new(inst_name).strong());
            for p in players {
                let name = p.name.as_str();
                let current = inst == own && name == m.player;
                let row = player_row(ui, name, &p.desc, ui.visuals().text_color());
                let resp = ui.selectable_label(current, row);
                let tags = tags_tip(p);
                let resp = if tags.is_empty() { resp } else { resp.on_hover_text(tags) };
                if resp.clicked() {
                    let r = app.run("band.slot", json!({"slot": ctx.slot + 1, "instrument": inst, "player": name}));
                    report(app, r);
                    ui.data_mut(|d| d.remove::<String>(search_id));
                    ui.close();
                }
            }
        }
        if !any {
            ui.label(egui::RichText::new("No player matches").color(Tokens::current().text_dim));
        }
    });
}

/// Auto (and what it picks here), then the Library's targets for the musician's instrument
/// (Generic always, and the current one).
fn target_menu(app: &mut SoundApp, ui: &mut Ui, ctx: &Ctx, lib: &LibraryData, m: &MusicianSlot, track: &Track) {
    let auto = is_auto(m);
    let auto_m = MusicianSlot { target: "auto".into(), ..m.clone() };
    if ui.selectable_label(auto, target_text(lib, &auto_m, track)).clicked() {
        ctx.set(app, "target", json!("auto"));
        ui.close();
    }
    ui.separator();
    let inst = usize::from(m.instrument);
    for slot in 0..K_TARGET_SLOTS {
        let Some(t) = lib.target(slot as i32) else { continue };
        let current = !auto && t.name == m.target;
        if slot != 0 && !current && !t.instruments.get(inst).copied().unwrap_or(false) {
            continue;
        }
        if ui.selectable_label(current, t.name.as_str()).clicked() {
            ctx.set(app, "target", json!(t.name));
            ui.close();
        }
    }
}

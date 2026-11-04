//! Vocal Studio (spec 2026-11-03-vocal-studio-design.md, mockup `vocal_studio_mock.png`): the
//! Vocal Editor extended to every vocal track at once. The roll shows the **editing track** (the
//! editor's own track: full colour, every tool of the editor) over the other listed tracks' notes
//! as faint **reference**, each track in its own colour; a track list on the left picks the
//! editing track and shows, hides, solos and mutes each one, with a status chip in the Mix pitch
//! box's colours; a bar under the header holds the studio's tools. Window › Vocal Studio (or the
//! editor's Studio button) turns it on; everything is reachable as `ui.vocal_studio`.
//!
//! Edit All (spec 2026-11-04-vocal-studio-edit-all-design.md, mockup
//! `vocal_studio_edit_all_mock.png`): the Edit picker's All vocals makes every shown track
//! editable at once; Tune all tunes them, leaving notes within the tolerance alone; notes of
//! several tracks on one row at one time are striped in each track's colour with a count badge,
//! a click opens a pick list, and an amber "≠" marks them when further apart than the tolerance.

use super::layout::{NoteShape, RollView, note_shapes};
use super::roll::{self, moved_target_of, tuned_centre_of};
use super::{hook, store};
use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, CornerRadius, Painter, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_engine::vocal::NoteRef;
use soundcraft_engine::vocal_studio::{Clash, Stack, TrackStatus, clashes, compared_track, studio_tracks, track_status, unison_stacks};
use soundcraft_model::{Session, TrackId, TrackKind};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The track list's width.
pub const TRACKS_W: f32 = 236.0;
/// The studio bar's height (under the header).
pub const BAR_H: f32 = 34.0;
/// One track row's height.
pub const ROW_H: f32 = 34.0;

/// The studio's colours, by place in the list (the mockup's blue, orange, purple, green…): each
/// track reads apart from its neighbours whatever colour it has in the Edit window.
pub const COLOURS: [Color32; 8] = [
    Color32::from_rgb(86, 160, 232),
    Color32::from_rgb(238, 168, 82),
    Color32::from_rgb(168, 130, 238),
    Color32::from_rgb(78, 186, 138),
    Color32::from_rgb(232, 104, 146),
    Color32::from_rgb(214, 196, 78),
    Color32::from_rgb(88, 198, 218),
    Color32::from_rgb(198, 128, 86),
];

/// The studio's state (not saved).
#[derive(Debug, Clone, Default)]
pub struct StudioState {
    /// The Vocal Editor shows as the Vocal Studio.
    pub on: bool,
    /// Listed tracks whose notes are hidden (the eye).
    pub hidden: BTreeSet<TrackId>,
    /// "Only editing": the reference tracks are not drawn (the bar's All vocals switch off).
    pub only_editing: bool,
    /// The rows are to centre on every listed track's notes (set when the studio turns on).
    pub centre_all: bool,
    /// The track the others are compared with for clashes (`None`: the session's lead).
    pub against: Option<TrackId>,
    /// What was worked out for the list, and what it was worked out for.
    studied: Option<(StudyKey, Arc<Studied>)>,
    /// Each track's colour (an index into [`COLOURS`]), kept once given.
    pub colours: std::collections::BTreeMap<TrackId, usize>,
    /// The Make harmony… form, while open.
    pub harmony: Option<super::harmony::HarmonyForm>,
    /// The form's planned notes, and what they were planned for.
    pub harmony_plan: Option<(HarmonyKey, Arc<Vec<soundcraft_engine::vocal_studio::HarmonyNote>>)>,
    /// Edit All: every shown track is editable (the Edit picker's All vocals).
    pub edit_all: bool,
    /// A stack's pick list, while open.
    pub picker: Option<Picker>,
    /// Tune all waits for an analysis to land.
    pub tune_pending: bool,
}

/// What a harmony plan depends on: the revision, the analysis map, the source and the form.
pub type HarmonyKey = (u64, usize, TrackId, super::harmony::HarmonyForm);

/// What [`Studied`] depends on: the revision, the analysis map, the editing and compared tracks,
/// the editable tracks and the tracks drawn.
type StudyKey = (u64, usize, Option<TrackId>, Option<TrackId>, Vec<TrackId>, Vec<TrackId>);

/// The list's status chips and clash markers, worked out once per state of the session.
#[derive(Debug, Clone, Default)]
pub struct Studied {
    pub statuses: Vec<TrackStatus>,
    /// Each listed track's clash count.
    pub clash_counts: Vec<(TrackId, usize)>,
    /// The compared track.
    pub against: Option<TrackId>,
    /// The editable tracks' clashes (in Edit All every shown track's but the compared one's).
    pub clashes: Vec<Clash>,
    /// The unison stacks among the tracks drawn (single-track: those holding an editing note).
    pub stacks: Vec<Stack>,
    /// Each listed track's count of stacks further apart than the tolerance ("N ≠").
    pub mismatch_counts: Vec<(TrackId, usize)>,
}

/// The listed tracks: the engine's (vocal or tuned audio tracks), plus the editing track when it
/// is none of them.
pub fn listed(app: &SoundApp) -> Vec<TrackId> {
    let s = app.engine.session();
    let mut v = studio_tracks(s);
    if let Some(t) = super::editor_track(app).filter(|t| !v.contains(t) && s.track(*t).is_some_and(|tr| tr.kind == TrackKind::Audio)) {
        v.push(t);
    }
    v
}

/// The colour of `track` in the studio: given the first time the studio sees it (the first
/// colour no listed track has, in list order), then kept, so a track added above it (a new
/// harmony under the lead) doesn't recolour the others.
pub fn colour(app: &mut SoundApp, track: TrackId) -> Color32 {
    let list = listed(app);
    let map = &mut app.vocal.studio.colours;
    for t in list.iter().chain(std::iter::once(&track)) {
        if map.contains_key(t) {
            continue;
        }
        let used: BTreeSet<usize> = list.iter().filter_map(|x| map.get(x)).copied().collect();
        let i = (0..COLOURS.len()).find(|i| !used.contains(i)).unwrap_or(map.len() % COLOURS.len());
        map.insert(*t, i);
    }
    map.get(&track).and_then(|i| COLOURS.get(*i)).copied().unwrap_or(Color32::GRAY)
}

/// The list's statuses and clashes, worked out again only when the session, its analyses, the
/// editing or the compared track changed.
pub fn studied(app: &mut SoundApp) -> Arc<Studied> {
    let s = app.engine.session_arc();
    let editing = super::editor_track(app);
    let editable = editing.map(|e| edit_tracks(app, e)).unwrap_or_default();
    let drawn = editing.map(|e| drawn_tracks(app, e)).unwrap_or_default();
    let key: StudyKey =
        (app.engine.revision, Arc::as_ptr(&s.vocal.analyses) as usize, editing, app.vocal.studio.against, editable.clone(), drawn.clone());
    if let Some((k, v)) = &app.vocal.studio.studied
        && *k == key
    {
        return Arc::clone(v);
    }
    let list = listed(app);
    let against = compared_track(&s, app.vocal.studio.against);
    let mut st = Studied { against, ..Studied::default() };
    let all = editable.len() > 1 || app.vocal.studio.edit_all;
    for t in &list {
        st.statuses.push(track_status(&s, *t));
        let c = clashes(&s, *t, against);
        st.clash_counts.push((*t, c.len()));
        // Edit All compares each track with the compared one, skipping that one itself.
        if (Some(*t) == editing && !all) || (all && editable.contains(t) && Some(*t) != against) {
            st.clashes.extend(c);
        }
    }
    let tol = s.studio_tuning.tolerance();
    st.stacks = unison_stacks(&s, &drawn).into_iter().filter(|g| g.notes.iter().any(|n| editable.contains(&n.track))).collect();
    for g in st.stacks.iter().filter(|g| g.mismatch(tol)) {
        let tracks = g.tracks();
        let lead_in = against.is_some_and(|a| tracks.contains(&a));
        for t in tracks.into_iter().filter(|t| !lead_in || Some(*t) != against) {
            match st.mismatch_counts.iter_mut().find(|(x, _)| *x == t) {
                Some((_, n)) => *n += 1,
                None => st.mismatch_counts.push((t, 1)),
            }
        }
    }
    let v = Arc::new(st);
    app.vocal.studio.studied = Some((key, Arc::clone(&v)));
    v
}

/// Each listed track's status (see [`studied`]).
pub fn statuses(app: &mut SoundApp) -> Vec<TrackStatus> {
    studied(app).statuses.clone()
}

/// The chip's text and colour for a status.
pub fn chip(st: &TrackStatus) -> (String, Color32) {
    let level = st.level();
    let c = match level {
        "in tune" => crate::vocal_mixer::level_colour(crate::vocal_mixer::Level::InTune),
        "drifting" => crate::vocal_mixer::level_colour(crate::vocal_mixer::Level::Drifting),
        "off" => crate::vocal_mixer::level_colour(crate::vocal_mixer::Level::Off),
        _ => Tokens::current().text_dim,
    };
    let text = if level == "analysing" { "analysing\u{2026}".to_string() } else { level.to_string() };
    (text, c)
}

/// The text of a track's chip: "N clash", "N ≠" (shown tracks), else its status.
fn chip_text(study: &Studied, st: Option<&TrackStatus>, t: TrackId, hidden: bool) -> Option<String> {
    let n_clash = study.clash_counts.iter().find(|(x, _)| *x == t).map_or(0, |x| x.1);
    let n_neq = study.mismatch_counts.iter().find(|(x, _)| *x == t).map_or(0, |x| x.1);
    let st = st?;
    Some(if n_clash > 0 {
        format!("{n_clash} clash")
    } else if n_neq > 0 && !hidden {
        format!("{n_neq} \u{2260}")
    } else {
        chip(st).0
    })
}

/// A rounded chip with `text` in `c` on a faint fill of it, right-aligned at `right`.
fn draw_chip(p: &Painter, right: egui::Pos2, text: &str, c: Color32) -> Rect {
    let g = p.layout_no_wrap(text.to_string(), regular(10.5), c);
    let r = Rect::from_min_size(pos2(right.x - g.size().x - 12.0, right.y - 8.0), vec2(g.size().x + 12.0, 16.0));
    p.rect(r, 8.0, c.gamma_multiply(0.18), Stroke::new(1.0, c.gamma_multiply(0.55)), StrokeKind::Inside);
    p.galley(pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0), g, c);
    r
}

/// Switches the editing track (the selection and popups go; the view stays where it is).
pub fn set_editing(app: &mut SoundApp, track: TrackId) {
    if app.vocal.track == Some(track) {
        return;
    }
    app.vocal.track = Some(track);
    app.vocal.selected.clear();
    app.vocal.popup = None;
    app.vocal.band = None;
    app.vocal.drag = None;
    app.vocal.whole_track = false;
    // The rows stay put: the tracks share one grid.
    app.vocal.centred = Some(track);
    app.vocal.studio.hidden.remove(&track);
}

/// The track list (left of the keys).
pub fn track_list(app: &mut SoundApp, ui: &mut Ui, rect: Rect, editing: TrackId) {
    let t = Tokens::current();
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.panel_bg2);
    p.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, t.border));
    let all_mode = app.vocal.studio.edit_all;
    let header = if all_mode { "TRACKS \u{b7} all editable" } else { "TRACKS \u{b7} click to edit" };
    p.text(pos2(rect.left() + 10.0, rect.top() + 14.0), Align2::LEFT_CENTER, header, bold(10.5), t.text_dim);
    let list = listed(app);
    let editable = edit_tracks(app, editing);
    let study = studied(app);
    let stats = &study.statuses;
    let s = app.engine.session_arc();
    let mut rows = Vec::new();
    let mut y = rect.top() + 28.0;
    for (i, id) in list.iter().enumerate() {
        let Some(tr) = s.track(*id) else { continue };
        let row = Rect::from_min_size(pos2(rect.left() + 4.0, y), vec2(rect.width() - 8.0, ROW_H - 2.0));
        y += ROW_H;
        let is_edit = editable.contains(id);
        let hidden = app.vocal.studio.hidden.contains(id);
        let resp = ui.interact(row, ui.id().with(("studio-row", id.0)), Sense::click());
        if is_edit {
            p.rect(row, 4.0, t.button, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
        } else if resp.hovered() {
            p.rect_filled(row, 4.0, t.button.gamma_multiply(0.5));
        }
        let c = colour(app, *id);
        let sw = Rect::from_center_size(pos2(row.left() + 12.0, row.center().y), vec2(11.0, 11.0));
        p.rect_filled(sw, 2.0, if hidden { c.gamma_multiply(0.35) } else { c });
        let name_c = if is_edit {
            t.text
        } else if hidden {
            t.text_dim.gamma_multiply(0.6)
        } else {
            t.text_dim
        };
        let name = tr.name.clone();
        let font = if is_edit { bold(12.5) } else { regular(12.0) };
        let g = p.layout_no_wrap(name, font, name_c);
        let name_w = row.width() - 24.0 - 118.0;
        if is_edit && !all_mode && g.size().x + 14.0 <= name_w {
            // A small pencil after the name: the track being edited.
            let x = (row.left() + 24.0 + g.size().x + 6.0).min(row.left() + 24.0 + name_w - 8.0);
            let (a, b) = (pos2(x, row.center().y + 5.0), pos2(x + 8.0, row.center().y - 3.0));
            p.line_segment([a + vec2(1.5, -1.5), b], Stroke::new(2.4, name_c));
            p.line_segment([a, a + vec2(1.5, -1.5)], Stroke::new(1.2, name_c));
        }
        p.with_clip_rect(Rect::from_min_size(pos2(row.left() + 24.0, row.top()), vec2(name_w.max(10.0), row.height()))).galley(
            pos2(row.left() + 24.0, row.center().y - g.size().y / 2.0),
            g,
            name_c,
        );
        // S and M at the right; the eye left of them; the chip left of the eye.
        let btn = |k: f32| Rect::from_center_size(pos2(row.right() - 10.0 - k * 18.0, row.center().y), vec2(16.0, 16.0));
        let (m_r, s_r, eye_r) = (btn(0.0), btn(1.0), btn(2.0));
        let st = stats.iter().find(|x| x.track == *id);
        let n_clash = study.clash_counts.iter().find(|(t, _)| t == id).map_or(0, |x| x.1);
        let n_neq = study.mismatch_counts.iter().find(|(t, _)| t == id).map_or(0, |x| x.1);
        let chip_r = st.map(|st| {
            let (text, cc) = if n_clash > 0 {
                (format!("{n_clash} clash"), crate::vocal_mixer::level_colour(crate::vocal_mixer::Level::Off))
            } else if n_neq > 0 && !hidden {
                (format!("{n_neq} \u{2260}"), crate::vocal_mixer::level_colour(crate::vocal_mixer::Level::Drifting))
            } else {
                chip(st)
            };
            draw_chip(&p, pos2(eye_r.left() - 4.0, row.center().y), &text, cc)
        });
        let mute = tr.mixer.mute;
        let solo = tr.mixer.solo;
        let small = |r: Rect, label: &str, on: bool, on_c: Color32| {
            if on {
                p.rect_filled(r, 3.0, on_c);
            }
            p.text(r.center(), Align2::CENTER_CENTER, label, bold(10.5), if on { Color32::BLACK } else { t.text_dim });
        };
        small(s_r, "S", solo, t.solo);
        small(m_r, "M", mute, t.mute);
        // The eye: an open eye when shown, a struck-through one when hidden.
        let ec = if hidden { t.text_dim.gamma_multiply(0.6) } else { t.text_dim };
        p.circle_stroke(eye_r.center(), 3.5, Stroke::new(1.2, ec));
        p.circle_filled(eye_r.center(), 1.6, ec);
        if hidden {
            p.line_segment([eye_r.left_bottom() + vec2(2.0, -2.0), eye_r.right_top() + vec2(-2.0, 2.0)], Stroke::new(1.2, ec));
        }
        for (r, what, tip) in
            [(eye_r, "eye", if hidden { "Show this track's notes" } else { "Hide this track's notes" }), (s_r, "solo", "Solo"), (m_r, "mute", "Mute")]
        {
            let b = ui.interact(r, ui.id().with(("studio-btn", what, id.0)), Sense::click()).on_hover_text(tip);
            if b.clicked() {
                let res = match what {
                    "eye" => app.run("ui.vocal_studio", json!({if hidden { "show" } else { "hide" }: id.0})),
                    "solo" => app.run("mix.solo", json!({"tracks": [id.0], "value": !solo})),
                    _ => app.run("mix.mute", json!({"tracks": [id.0], "value": !mute})),
                };
                if let Err(e) = res {
                    app.ui.status = e;
                }
            }
        }
        if let Some(cr) = chip_r {
            let vs = study.against.filter(|a| a != id).and_then(|a| s.track(a)).map_or(String::new(), |a| format!(" against {}", a.name));
            let tip = st.map(|st| match st.avg_offset {
                Some(_) if n_clash == 0 && n_neq > 0 && !hidden => {
                    format!(
                        "{}: {n_neq} unison note{} further apart than the tolerance from another track's",
                        tr.name,
                        if n_neq == 1 { "" } else { "s" }
                    )
                }
                Some(o) if n_clash > 0 => {
                    format!("{}: {n_clash} notes clash{vs} or with the chord ({:.0} c from their targets on average)", tr.name, o)
                }
                Some(o) => format!("{}: notes {:.0} c from their targets on average ({} notes)", tr.name, o, st.notes),
                None => format!("{}: {}", tr.name, st.level()),
            });
            if let Some(tip) = tip {
                ui.interact(cr, ui.id().with(("studio-chip", id.0)), Sense::hover()).on_hover_text(tip);
            }
        }
        // A name: edit just that track (also back from Edit All).
        if resp.clicked() && (all_mode || *id != editing) {
            let _ = app.run("ui.vocal_studio", json!({"edit": id.0}));
        }
        rows.push((*id, row, i));
    }
    hook(ui.ctx(), "studio-rows", rows.iter().map(|(id, r, _)| (*id, *r)).collect::<Vec<_>>());
    // Add a track: an audio track not listed yet (it is marked vocal).
    let others: Vec<(TrackId, String)> =
        s.tracks.iter().filter(|tr| tr.kind == TrackKind::Audio && !list.contains(&tr.id)).map(|tr| (tr.id, tr.name.clone())).collect();
    let add_r = Rect::from_min_size(pos2(rect.left() + 8.0, y + 4.0), vec2(rect.width() - 16.0, 22.0));
    if !others.is_empty() {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(add_r));
        child.menu_button("+ Add track\u{2026}", |ui| {
            for (id, name) in &others {
                if ui.button(name).clicked() {
                    if let Err(e) = app.run("ui.vocal_studio", json!({"add": id.0})) {
                        app.ui.status = e;
                    }
                    ui.close();
                }
            }
        });
        y += 30.0;
    }
    let note = if all_mode {
        "Hidden tracks are left out of All mode and Tune all. Click a name to edit just that track."
    } else {
        "Non-edit tracks show as faint reference. Double-click a note of another track to switch to it."
    };
    let g = p.layout(note.to_string(), regular(11.0), t.text_dim, rect.width() - 20.0);
    p.galley(pos2(rect.left() + 10.0, y + 10.0), g, t.text_dim);
}

/// The reference notes: every listed, shown track but the `editable` ones, faint, in its colour, under the
/// editing track's notes. Returns them (track, note, bar) for the mouse.
pub fn draw_references(app: &mut SoundApp, ui: &Ui, painter: &Painter, v: &RollView, editable: &[TrackId]) -> Vec<(TrackId, NoteRef, Rect)> {
    let mut out = Vec::new();
    if app.vocal.studio.only_editing {
        hook(ui.ctx(), "studio-refs", out.clone());
        return out;
    }
    let list = listed(app);
    let dark = Tokens::current().panel_bg.r() < 128;
    let shown: Vec<TrackId> = list.iter().copied().filter(|t| !editable.contains(t) && !app.vocal.studio.hidden.contains(t)).collect();
    for t in shown {
        let c = colour(app, t);
        let typical = roll::typical(app, t);
        for cv in roll::clip_views(app, t, Some((v.t0, v.t1))).iter().filter(|c| !c.stretched) {
            let shapes: Vec<NoteShape> =
                note_shapes(v, &cv.clip, &cv.a, cv.curves.as_deref(), typical, |n| moved_target_of(cv, n), |n| tuned_centre_of(cv, n));
            for sh in shapes {
                let rad = (sh.rect.height() / 2.0).min(sh.rect.width() / 2.0).min(6.0);
                let fill = if dark { c.gamma_multiply(0.42) } else { c.gamma_multiply(0.5) };
                painter.rect(
                    sh.rect,
                    CornerRadius::same(rad.clamp(0.0, 255.0) as u8),
                    fill,
                    Stroke::new(1.0, c.gamma_multiply(0.55)),
                    StrokeKind::Inside,
                );
                out.push((t, sh.r, sh.rect));
            }
        }
    }
    hook(ui.ctx(), "studio-refs", out.clone());
    out
}

/// The studio bar under the header: All vocals, the editing track, the key, and (right) back to
/// the plain editor.
pub fn bar(app: &mut SoundApp, ui: &mut Ui, rect: Rect, editing: TrackId) {
    let t = Tokens::current();
    ui.painter().rect_filled(rect, 0.0, t.panel_bg2);
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.border));
    let s = app.engine.session_arc();
    let inner = rect.shrink2(vec2(8.0, 4.0));
    let mut tb = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    tb.spacing_mut().item_spacing.x = 6.0;
    let all = !app.vocal.studio.only_editing;
    let b = tb.add(egui::Button::new(egui::RichText::new("All vocals").color(if all { t.mode_on_text } else { t.text })).fill(if all {
        t.mode_on
    } else {
        t.button
    }));
    store(ui.ctx(), "studio-all", b.rect);
    if b.on_hover_text("Show the other vocal tracks as faint reference").clicked() {
        let _ = app.run("ui.vocal_studio", json!({"all": !all}));
    }
    let list = listed(app);
    let name = s.track(editing).map_or(String::new(), |tr| tr.name.clone());
    let mut pick = None;
    let mut pick_all = false;
    let all_on = app.vocal.studio.edit_all;
    let edit_label = if all_on { "Edit: All vocals".to_string() } else { format!("Edit: {name}") };
    let r = tb.menu_button(edit_label, |ui| {
        if ui.selectable_label(all_on, "All vocals").on_hover_text("Every shown track editable at full colour").clicked() {
            pick_all = true;
            ui.close();
        }
        ui.separator();
        for id in &list {
            let n = s.track(*id).map_or(String::new(), |tr| tr.name.clone());
            if ui.selectable_label(!all_on && *id == editing, n).clicked() {
                pick = Some(*id);
                ui.close();
            }
        }
    });
    store(ui.ctx(), "studio-edit", r.response.rect);
    if let Some(id) = pick {
        let _ = app.run("ui.vocal_studio", json!({"edit": id.0}));
    }
    if pick_all {
        let _ = app.run("ui.vocal_studio", json!({"edit": "all"}));
    }
    tb.separator();
    let key = if soundcraft_engine::vocal::has_key(&s) {
        let (k, sc) = soundcraft_engine::vocal::key_pc_at(&s, app.position());
        roll::key_name(k, sc)
    } else {
        "none".into()
    };
    tb.label(egui::RichText::new("Key:").color(t.text_dim));
    tb.label(egui::RichText::new(&key).strong().color(t.text));
    hook(ui.ctx(), "studio-key", key);
    tb.separator();
    // Clashes are checked against…
    let study = studied(app);
    let against_name = study.against.and_then(|a| s.track(a)).map_or("chord & key".to_string(), |tr| tr.name.clone());
    let mut chosen: Option<Option<TrackId>> = None;
    let r = tb.menu_button(format!("Against: {against_name}"), |ui| {
        if ui.selectable_label(app.vocal.studio.against.is_none(), "The lead (automatic)").clicked() {
            chosen = Some(None);
            ui.close();
        }
        for id in &list {
            let n = s.track(*id).map_or(String::new(), |tr| tr.name.clone());
            if ui.selectable_label(app.vocal.studio.against == Some(*id), n).clicked() {
                chosen = Some(Some(*id));
                ui.close();
            }
        }
    });
    store(ui.ctx(), "studio-against", r.response.rect);
    r.response.on_hover_text("The track clash markers compare the editing track with (and Fit to lead fits it to)");
    if let Some(a) = chosen {
        let _ = app.run("ui.vocal_studio", json!({"against": a.map(|t| t.0)}));
    }
    edit_all_tools(app, &mut tb, editing);
    let open = app.vocal.studio.harmony.is_some();
    let amber = crate::vocal_mixer::level_colour(crate::vocal_mixer::Level::Drifting);
    let mh = tb.add(
        egui::Button::new(egui::RichText::new("Make harmony\u{2026}").color(if open { Color32::BLACK } else { amber }))
            .fill(if open { amber } else { t.button })
            .stroke(Stroke::new(1.0, amber)),
    );
    store(ui.ctx(), "studio-harmony", mh.rect);
    if mh.on_hover_text("Make a harmony part from the lead's own voice").clicked() {
        let _ = app.run("ui.vocal_harmony", json!({"open": !open}));
    }
    // Fit to lead fits one track: single-track editing only.
    if !app.vocal.studio.edit_all {
        let n = study.clashes.len();
        let fit = tb.add_enabled(editing_has_notes(app, editing), egui::Button::new("Fit to lead"));
        store(ui.ctx(), "studio-fit", fit.rect);
        if fit
            .on_hover_text(format!(
                "Move the {} notes that clash to the nearest chord or key tone that doesn't ({n} clash now)",
                if app.vocal.selected.is_empty() { "editing track's" } else { "selected" }
            ))
            .clicked()
        {
            fit_to_lead(app, editing);
        }
    }
    // Right side: back to the plain editor.
    let right = Rect::from_min_max(pos2(rect.right() - 110.0, inner.top()), pos2(inner.right(), inner.bottom()));
    let mut rb = ui.new_child(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let ed = rb.button("Editor view");
    store(ui.ctx(), "studio-off", ed.rect);
    if ed.on_hover_text("Back to the Vocal Editor: one track").clicked() {
        let _ = app.run("ui.vocal_studio", json!({"value": false}));
    }
}

/// Whether the editing track has notes (Fit to lead has something to move).
fn editing_has_notes(app: &mut SoundApp, track: TrackId) -> bool {
    super::roll::note_count(app, track) > 0
}

/// Fit to lead on the selected notes (all of the editing track's without a selection).
pub fn fit_to_lead(app: &mut SoundApp, track: TrackId) {
    let against = studied(app).against;
    let mut p = json!({"track": track.0});
    if let Some(a) = against {
        p["against"] = json!(a.0);
    }
    if !app.vocal.selected.is_empty() {
        p["notes"] = json!(app.vocal.selected.iter().collect::<Vec<_>>());
    }
    match app.run("vocal.fit_to_lead", p) {
        Ok(v) => {
            let k = v["moved"].as_array().map_or(0, Vec::len);
            app.ui.status = if k == 0 { "Fit to lead: every note fits already".into() } else { format!("Fit to lead: moved {k} notes") };
        }
        Err(e) => app.ui.status = e,
    }
}

/// The clash markers: a red "!" right of each clashing note of the editing track (its hover
/// says why). Returns them (note, marker, text) for the mouse and the tests.
pub fn draw_clashes(app: &mut SoundApp, ui: &Ui, painter: &Painter, shapes: &[NoteShape], stacks: &[StackMark]) -> Vec<(NoteRef, Rect, String)> {
    let study = studied(app);
    let red = crate::vocal_mixer::level_colour(crate::vocal_mixer::Level::Off);
    let mut out = Vec::new();
    for c in &study.clashes {
        let Some(sh) = shapes.iter().find(|sh| sh.r == c.r) else { continue };
        // Right of a "≠" marker when the note has one.
        let after = stacks.iter().find(|m| m.notes.contains(&c.r)).and_then(|m| m.marker).map_or(sh.rect.right(), |r| r.right() + 1.0);
        let centre = pos2(after.max(sh.rect.right()) + 9.0, sh.rect.center().y);
        let r = Rect::from_center_size(centre, vec2(15.0, 15.0));
        painter.circle(centre, 7.5, red, Stroke::new(1.0, Color32::from_black_alpha(120)));
        painter.text(centre, Align2::CENTER_CENTER, "!", bold(11.0), Color32::WHITE);
        out.push((c.r, r, c.text.clone()));
    }
    hook(ui.ctx(), "studio-clashes", out.clone());
    out
}

/// A track from `v` (id or name).
fn track_of(s: &Session, v: &Value) -> Result<TrackId, String> {
    if let Some(n) = v.as_u64() {
        return s.track(TrackId(n)).map(|t| t.id).ok_or_else(|| format!("no track with id {n}"));
    }
    if let Some(name) = v.as_str() {
        return s.track_by_name(name).map(|t| t.id).ok_or_else(|| format!("no track named `{name}`"));
    }
    Err("a track is its id or name".into())
}

/// Tracks from `v`: one, or a list.
fn tracks_of(s: &Session, v: &Value) -> Result<Vec<TrackId>, String> {
    match v.as_array() {
        Some(a) => a.iter().map(|x| track_of(s, x)).collect(),
        None => Ok(vec![track_of(s, v)?]),
    }
}

/// `ui.vocal_studio {value?, edit?: track | "all", show?, hide?, all?, add?, against?, tolerance?,
/// fix_to?, tune_all?, pick?}`: turns the studio on or off (on when any other key is given), sets
/// the editing track (or Edit All: "all"), the Tune all tolerance and Fix to (`vocal.tolerance`),
/// runs Tune all on the shown tracks, opens unison stack `pick`'s pick list (`null` closes it), shows or hides tracks' notes, the
/// All vocals switch, adds a track to the list (marks it vocal: `track.vocal`) and sets the track
/// clashes are checked against (`null`: the session's lead). Returns the studio's state, with the
/// editing track's clashes.
pub fn command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let s = app.engine.session_arc();
    let edit_all = p.get("edit").and_then(Value::as_str).is_some_and(|x| x.eq_ignore_ascii_case("all"));
    let edit = if edit_all { None } else { p.get("edit").map(|v| track_of(&s, v)).transpose()? };
    if let Some(t) = edit
        && s.track(t).is_none_or(|tr| tr.kind != TrackKind::Audio)
    {
        return Err("the editing track must be an audio track".into());
    }
    let show = p.get("show").map(|v| tracks_of(&s, v)).transpose()?;
    let hide = p.get("hide").map(|v| tracks_of(&s, v)).transpose()?;
    let add = p.get("add").map(|v| track_of(&s, v)).transpose()?;
    let against = match p.get("against") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(track_of(&s, v)?)),
    };
    let all = match p.get("all") {
        None => None,
        Some(v) => Some(v.as_bool().ok_or("`all` must be true or false")?),
    };
    let value = match p.get("value") {
        None => None,
        Some(v) => Some(v.as_bool().ok_or("`value` must be true or false")?),
    };
    let pick = match p.get("pick") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(v.as_u64().ok_or("`pick` must be a stack number (0: the first) or null")? as usize)),
    };
    let tune = match p.get("tune_all") {
        None => false,
        Some(v) => v.as_bool().ok_or("`tune_all` must be true or false")?,
    };
    if let Some(t) = add {
        app.run("track.vocal", json!({"tracks": [t.0], "value": true}))?;
    }
    let mut tol = serde_json::Map::new();
    if let Some(v) = p.get("tolerance") {
        tol.insert("cents".into(), v.clone());
    }
    if let Some(v) = p.get("fix_to") {
        tol.insert("fix_to".into(), v.clone());
    }
    if !tol.is_empty() {
        app.run("vocal.tolerance", Value::Object(tol))?;
    }
    let on = value.unwrap_or(true);
    if on && !app.vocal.studio.on {
        app.vocal.studio.centre_all = true;
    }
    app.vocal.studio.on = on;
    if on {
        app.ui.show_vocal_editor = true;
    }
    if let Some(t) = edit.or(add) {
        set_editing(app, t);
        app.vocal.studio.edit_all = false;
    }
    if edit_all {
        app.vocal.studio.edit_all = true;
        app.vocal.studio.only_editing = false;
        app.vocal.popup = None;
    }
    if edit.is_some() || edit_all || value == Some(false) {
        app.vocal.studio.picker = None;
    }
    for t in show.into_iter().flatten() {
        app.vocal.studio.hidden.remove(&t);
    }
    for t in hide.into_iter().flatten() {
        if Some(t) != super::editor_track(app) {
            app.vocal.studio.hidden.insert(t);
        }
    }
    if let Some(a) = all {
        app.vocal.studio.only_editing = !a;
    }
    if let Some(a) = against {
        app.vocal.studio.against = a;
    }
    match pick {
        Some(Some(i)) => {
            let study = studied(app);
            let g = study.stacks.get(i).ok_or_else(|| format!("no unison stack {i} ({} shown)", study.stacks.len()))?;
            app.vocal.studio.picker =
                Some(Picker { notes: g.notes.iter().map(|n| n.note.r).collect(), at: egui::Pos2::new(f32::NAN, f32::NAN), pass: 0 });
        }
        Some(None) => app.vocal.studio.picker = None,
        None => {}
    }
    let tuned = if tune { Some(tune_all(app)?) } else { None };
    let mut v = state(app);
    if let Some(r) = tuned {
        v["tune_all"] = r;
    }
    Ok(v)
}

/// The studio's state as `ui.vocal_studio` returns it.
pub fn state(app: &mut SoundApp) -> Value {
    let editing = super::editor_track(app);
    let list = listed(app);
    let study = studied(app);
    let stats = &study.statuses;
    let s = app.engine.session();
    let tracks: Vec<Value> = list
        .iter()
        .map(|t| {
            let st = stats.iter().find(|x| x.track == *t);
            json!({
                "track": t,
                "name": s.track(*t).map(|x| x.name.clone()),
                "editing": Some(*t) == editing,
                "hidden": app.vocal.studio.hidden.contains(t),
                "status": st.map(TrackStatus::level),
                "clashes": study.clash_counts.iter().find(|(x, _)| x == t).map_or(0, |x| x.1),
                "chip": chip_text(&study, st, *t, app.vocal.studio.hidden.contains(t)),
            })
        })
        .collect();
    let clashes: Vec<Value> = study.clashes.iter().map(|c| json!({"clip": c.r.clip, "note": c.r.note, "kind": c.kind, "text": c.text})).collect();
    let tu = s.studio_tuning;
    let editable = editing.map(|e| edit_tracks(app, e)).unwrap_or_default();
    let stacks: Vec<Value> = study
        .stacks
        .iter()
        .map(|g| {
            json!({
                "notes": g.notes.iter().map(|n| n.note.r).collect::<Vec<_>>(),
                "tracks": g.tracks(),
                "apart": (g.apart * 10.0).round() / 10.0,
                "mismatch": g.mismatch(tu.tolerance()),
            })
        })
        .collect();
    json!({
        "value": app.vocal.studio.on,
        "edit": if app.vocal.studio.edit_all { json!("all") } else { json!(editing) },
        "editable": editable,
        "tolerance": tu.tolerance(),
        "fix_to": if tu.fix_to_edge { "edge" } else { "centre" },
        "tune_pending": app.vocal.studio.tune_pending,
        "stacks": stacks,
        "editing": editing,
        "against": study.against,
        "all": !app.vocal.studio.only_editing,
        "tracks": tracks,
        "clashes": clashes,
    })
}

// ---- Edit All, Tune all, stacked unison notes (spec 2026-11-04-vocal-studio-edit-all-design.md) ----

/// The shown tracks: listed, eye on, and with any listed track soloed only the soloed ones.
/// Edit All edits them and Tune all tunes them.
pub fn shown(app: &SoundApp) -> Vec<TrackId> {
    let list = listed(app);
    let s = app.engine.session();
    let soloed: Vec<TrackId> = list.iter().copied().filter(|t| s.track(*t).is_some_and(|x| x.mixer.solo)).collect();
    list.into_iter().filter(|t| !app.vocal.studio.hidden.contains(t) && (soloed.is_empty() || soloed.contains(t))).collect()
}

/// The tracks whose notes are editable: in Edit All every shown track, else the editing one.
pub fn edit_tracks(app: &SoundApp, editing: TrackId) -> Vec<TrackId> {
    if app.vocal.studio.on && app.vocal.studio.edit_all {
        let v = shown(app);
        if !v.is_empty() {
            return v;
        }
    }
    vec![editing]
}

/// The tracks whose notes the roll shows: the editable ones and (All vocals on) the shown
/// references.
fn drawn_tracks(app: &SoundApp, editing: TrackId) -> Vec<TrackId> {
    let mut v = edit_tracks(app, editing);
    if !app.vocal.studio.only_editing {
        for t in listed(app) {
            if !v.contains(&t) && !app.vocal.studio.hidden.contains(&t) {
                v.push(t);
            }
        }
    }
    v
}

/// A pitch line in a track's colour: lighter on the dark theme, darker on the light one.
pub fn line_colour(c: Color32, dark: bool) -> Color32 {
    let mix = |v: u8, to: f32, k: f32| (f32::from(v) + (to - f32::from(v)) * k).clamp(0.0, 255.0) as u8;
    if dark {
        Color32::from_rgb(mix(c.r(), 255.0, 0.35), mix(c.g(), 255.0, 0.35), mix(c.b(), 255.0, 0.35))
    } else {
        Color32::from_rgb(mix(c.r(), 0.0, 0.3), mix(c.g(), 0.0, 0.3), mix(c.b(), 0.0, 0.3))
    }
}

/// A unison stack as drawn: its notes, the striped spot (full height, where they overlap), the
/// count badge and the "≠" marker when they are further apart than the tolerance.
#[derive(Debug, Clone, PartialEq)]
pub struct StackMark {
    pub notes: Vec<NoteRef>,
    pub tracks: Vec<TrackId>,
    /// Each note's offset from its target (cents), in `notes`' order.
    pub offsets: Vec<f32>,
    pub midi: i32,
    pub rect: Rect,
    pub badge: Rect,
    pub mismatch: bool,
    pub apart: f32,
    pub marker: Option<Rect>,
}

/// The pick list of a stack, while open.
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub notes: Vec<NoteRef>,
    pub at: egui::Pos2,
    /// The egui pass it opened in (the click that opened it doesn't close it).
    pub pass: u64,
}

/// Thin diagonal stripes over `r`, cycling through `colours` (each track's own: not a blend,
/// which would read as a third track).
fn stripes(p: &Painter, r: Rect, colours: &[Color32]) {
    let vis = r.intersect(p.clip_rect());
    if !vis.is_positive() || colours.is_empty() {
        return;
    }
    let p = p.with_clip_rect(vis);
    const STEP: f32 = 4.0;
    let h = r.height();
    let first = ((vis.left() - h - r.left()) / STEP).floor();
    let mut k = first as i64;
    let mut x = r.left() + first * STEP;
    while x < vis.right() + STEP {
        let c = colours.get(k.rem_euclid(colours.len() as i64) as usize).copied().unwrap_or(Color32::GRAY);
        p.line_segment([pos2(x, r.bottom()), pos2(x + h, r.top())], Stroke::new(STEP * 0.75, c));
        x += STEP;
        k += 1;
    }
}

/// Draws the unison stacks among the roll's notes: where notes of two or more tracks overlap on
/// one row, the shared part full height, striped in each track's colour, with a count badge,
/// and an amber "≠" right of it when their held pitches are further apart than the tolerance.
/// `shapes` are the editable notes, `refs` the reference ones.
pub fn draw_stacks(
    app: &mut SoundApp,
    ui: &Ui,
    painter: &Painter,
    shapes: &[NoteShape],
    refs: &[(TrackId, NoteRef, Rect)],
    selected: &BTreeSet<NoteRef>,
) -> Vec<StackMark> {
    let study = studied(app);
    let s = app.engine.session_arc();
    let tol = s.studio_tuning.tolerance();
    let dark = Tokens::current().panel_bg.r() < 128;
    let sel_c = if dark { Color32::WHITE } else { Color32::from_rgb(25, 25, 28) };
    let amber = crate::vocal_mixer::level_colour(crate::vocal_mixer::Level::Drifting);
    let rect_of = |r: NoteRef| shapes.iter().find(|sh| sh.r == r).map(|sh| sh.rect).or_else(|| refs.iter().find(|x| x.1 == r).map(|x| x.2));
    let mut out = Vec::new();
    for g in &study.stacks {
        let members: Vec<(TrackId, NoteRef, Rect, f32)> =
            g.notes.iter().filter_map(|n| rect_of(n.note.r).map(|rc| (n.track, n.note.r, rc, n.note.offset()))).collect();
        if members.iter().map(|m| m.0).collect::<BTreeSet<_>>().len() < 2 {
            continue;
        }
        // The shared part: where any two of them overlap.
        let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
        for (i, a) in members.iter().enumerate() {
            for b in members.iter().skip(i + 1).filter(|b| b.0 != a.0) {
                let (l, r) = (a.2.left().max(b.2.left()), a.2.right().min(b.2.right()));
                if r > l {
                    x0 = x0.min(l);
                    x1 = x1.max(r);
                }
            }
        }
        // Infinite when no two overlap.
        if !(x0.is_finite() && x1.is_finite() && x1 > x0) {
            continue;
        }
        let top = members.iter().map(|m| m.2.top()).fold(f32::INFINITY, f32::min);
        let bottom = members.iter().map(|m| m.2.bottom()).fold(f32::NEG_INFINITY, f32::max);
        let rect = Rect::from_min_max(pos2(x0, top), pos2(x1, bottom));
        let mut tracks: Vec<TrackId> = Vec::new();
        for m in &members {
            if !tracks.contains(&m.0) {
                tracks.push(m.0);
            }
        }
        let colours: Vec<Color32> = tracks.iter().map(|t| colour(app, *t)).collect();
        stripes(painter, rect, &colours);
        let picked = members.iter().any(|m| selected.contains(&m.1));
        let stroke = if picked { Stroke::new(2.0, sel_c) } else { Stroke::new(1.0, Color32::from_black_alpha(110)) };
        painter.rect_stroke(rect, 2.0, stroke, StrokeKind::Outside);
        // The count badge, at the shared part's right end.
        let n = members.len();
        let bc = pos2((rect.right() - 8.0).max(rect.left() + 6.0), rect.center().y);
        let badge = Rect::from_center_size(bc, vec2(13.0, 13.0));
        painter.circle(bc, 6.5, Color32::from_rgb(24, 26, 30), Stroke::new(1.0, Color32::from_white_alpha(170)));
        painter.text(bc, Align2::CENTER_CENTER, n.to_string(), bold(9.5), Color32::WHITE);
        let mismatch = g.mismatch(tol);
        let marker = mismatch.then(|| {
            let right = members.iter().map(|m| m.2.right()).fold(rect.right(), f32::max);
            let c = pos2(right + 10.0, rect.center().y);
            painter.circle(c, 7.5, amber, Stroke::new(1.0, Color32::from_black_alpha(120)));
            painter.text(c, Align2::CENTER_CENTER, "\u{2260}", bold(11.0), Color32::from_rgb(30, 22, 6));
            Rect::from_center_size(c, vec2(15.0, 15.0))
        });
        out.push(StackMark {
            notes: members.iter().map(|m| m.1).collect(),
            tracks,
            offsets: members.iter().map(|m| m.3).collect(),
            midi: g.midi,
            rect,
            badge,
            mismatch,
            apart: g.apart,
            marker,
        });
    }
    hook(ui.ctx(), "studio-stacks", out.iter().map(|m| (m.notes.clone(), m.rect, m.mismatch)).collect::<Vec<_>>());
    out
}

/// A track's name.
fn name_of(s: &Session, t: TrackId) -> String {
    s.track(t).map_or_else(String::new, |x| x.name.clone())
}

/// What a stack's hover says: the note, each track's offset, and how far apart they are.
pub fn stack_text(app: &SoundApp, m: &StackMark) -> String {
    let s = app.engine.session();
    let mut lines =
        vec![format!("{} \u{b7} {} notes here \u{2014} click to pick one", crate::vocal_style::note_name(m.midi as f32 * 100.0), m.notes.len())];
    for (r, off) in m.notes.iter().zip(&m.offsets) {
        let t = s.find_clip(r.clip).map(|(t, _)| t);
        lines.push(format!("\u{25a0} {} {}", t.map_or_else(String::new, |t| name_of(s, t)), crate::vocal_style::fmt_cents(*off)));
    }
    if m.mismatch {
        lines.push(format!("\u{2260} {:.0}c apart (tolerance {:.0}c)", m.apart, s.studio_tuning.tolerance()));
    }
    lines.join("\n")
}

/// Opens stack `i`'s pick list at `at`.
pub fn open_picker(app: &mut SoundApp, stacks: &[StackMark], i: usize, at: egui::Pos2) {
    if let Some(m) = stacks.get(i) {
        app.vocal.studio.picker = Some(Picker { notes: m.notes.clone(), at, pass: 0 });
    }
}

/// The note a Match lines the others up with: the compared track's (the lead's) when it is in
/// the stack, else the first.
fn anchor_of(app: &mut SoundApp, m: &StackMark) -> Option<NoteRef> {
    let against = studied(app).against;
    let s = app.engine.session_arc();
    m.notes.iter().find(|r| s.find_clip(r.clip).map(|(t, _)| t) == against).or(m.notes.first()).copied()
}

/// The pick list of the open stack: a coloured row per note (select it to edit or drag it),
/// Both (every note, when all are editable), and for a mismatch Match <other> to <lead> and
/// Both to centre. Esc or a click elsewhere closes it.
pub fn picker(app: &mut SoundApp, ui: &mut Ui, d: &super::roll::Drawn) {
    let Some(pk) = app.vocal.studio.picker.clone() else {
        ui.ctx().data_mut(|d| d.remove::<Rect>(egui::Id::new("soundcraft-vocal-picker")));
        return;
    };
    let Some(m) = d.stacks.iter().find(|m| m.notes.iter().any(|r| pk.notes.contains(r))).cloned() else {
        app.vocal.studio.picker = None;
        return;
    };
    let pass = ui.ctx().cumulative_pass_nr();
    if pk.pass == 0
        && let Some(p) = app.vocal.studio.picker.as_mut()
    {
        p.pass = pass;
    }
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let editing = super::editor_track(app);
    let editable = editing.map(|e| edit_tracks(app, e)).unwrap_or_default();
    let all_editable = m.notes.iter().all(|r| s.find_clip(r.clip).is_some_and(|(tr, _)| editable.contains(&tr)));
    let tol = s.studio_tuning.tolerance();
    let anchor = anchor_of(app, &m);
    let anchor_track = anchor.and_then(|a| s.find_clip(a.clip)).map(|(t, _)| t);
    let others: Vec<NoteRef> = m.notes.iter().copied().filter(|r| Some(*r) != anchor).collect();
    let other_names: Vec<String> = others.iter().filter_map(|r| s.find_clip(r.clip)).map(|(t, _)| name_of(&s, t)).collect();
    let match_label = format!("Match {} to {}", other_names.join(" & "), anchor_track.map_or_else(String::new, |t| name_of(&s, t)));
    let centre_label = if m.notes.len() == 2 { "Both to centre".to_string() } else { "All to centre".to_string() };
    let title = format!("{} \u{b7} {} notes here \u{2014} pick one to edit", crate::vocal_style::note_name(m.midi as f32 * 100.0), m.notes.len());
    // Size: the widest line.
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("soundcraft-vocal-picker")));
    let w_of = |text: &str, f: egui::FontId| p.layout_no_wrap(text.to_string(), f, t.text).size().x;
    let mut w = w_of(&title, bold(11.5)) + 20.0;
    for r in &m.notes {
        let n = s.find_clip(r.clip).map_or_else(String::new, |(tr, _)| name_of(&s, tr));
        w = w.max(w_of(&n, regular(11.5)) + 90.0);
    }
    if m.mismatch {
        w = w.max(w_of(&match_label, regular(11.5)) + w_of(&centre_label, regular(11.5)) + 44.0);
    }
    let row_h = 22.0;
    let rows = m.notes.len() + usize::from(all_editable && m.notes.len() > 1);
    let h = 30.0 + rows as f32 * row_h + if m.mismatch { 52.0 } else { 6.0 };
    let roll = d.view.rect;
    // Opened by the command: by the stack's right end.
    let at = if pk.at.x.is_finite() && pk.at.y.is_finite() { pk.at } else { m.rect.right_center() };
    let pk = Picker { at, ..pk };
    let min = pos2(
        (pk.at.x + 12.0).min(roll.right() - w - 4.0).max(roll.left() + 4.0),
        (pk.at.y - 10.0).min(roll.bottom() - h - 4.0).max(roll.top() + 4.0),
    );
    let bx = Rect::from_min_size(min, vec2(w, h));
    let dark = t.panel_bg.r() < 128;
    p.rect(bx, 6.0, if dark { Color32::from_rgb(30, 31, 35) } else { t.panel_bg2 }, Stroke::new(1.0, t.border), StrokeKind::Inside);
    p.text(pos2(bx.left() + 10.0, bx.top() + 15.0), Align2::LEFT_CENTER, &title, bold(11.5), t.text);
    store(ui.ctx(), "picker", bx);
    hook(ui.ctx(), "picker-title", title);
    let mut y = bx.top() + 28.0;
    let mut chosen: Option<Vec<NoteRef>> = None;
    let mut row_rects = Vec::new();
    let area_id = ui.id().with("vocal-picker");
    for (i, (r, off)) in m.notes.iter().zip(&m.offsets).enumerate() {
        let row = Rect::from_min_size(pos2(bx.left() + 6.0, y), vec2(w - 12.0, row_h - 2.0));
        y += row_h;
        let resp = ui.interact(row, area_id.with(("row", i)), Sense::click());
        let sel = app.vocal.selected.contains(r);
        if sel || resp.hovered() {
            p.rect_filled(row, 4.0, if sel { t.mode_on.gamma_multiply(0.6) } else { t.button });
        }
        let tr = s.find_clip(r.clip).map(|(tr, _)| tr);
        let c = tr.map_or(Color32::GRAY, |tr| colour(app, tr));
        p.rect_filled(Rect::from_center_size(pos2(row.left() + 10.0, row.center().y), vec2(10.0, 10.0)), 2.0, c);
        p.text(
            pos2(row.left() + 22.0, row.center().y),
            Align2::LEFT_CENTER,
            tr.map_or_else(String::new, |tr| name_of(&s, tr)),
            regular(11.5),
            t.text,
        );
        p.text(pos2(row.right() - 6.0, row.center().y), Align2::RIGHT_CENTER, crate::vocal_style::fmt_cents(*off), regular(11.5), t.text_dim);
        store(ui.ctx(), &format!("picker-row-{i}"), row);
        row_rects.push(row);
        if resp.clicked() {
            chosen = Some(vec![*r]);
        }
    }
    if all_editable && m.notes.len() > 1 {
        let row = Rect::from_min_size(pos2(bx.left() + 6.0, y), vec2(w - 12.0, row_h - 2.0));
        y += row_h;
        let resp = ui.interact(row, area_id.with("both"), Sense::click());
        if resp.hovered() {
            p.rect_filled(row, 4.0, t.button);
        }
        p.text(
            pos2(row.left() + 22.0, row.center().y),
            Align2::LEFT_CENTER,
            if m.notes.len() == 2 { "Both" } else { "All" },
            regular(11.5),
            t.text_dim,
        );
        store(ui.ctx(), "picker-both", row);
        if resp.clicked() {
            chosen = Some(m.notes.clone());
        }
    }
    let mut action: Option<(serde_json::Value, &'static str)> = None;
    if m.mismatch {
        let amber = crate::vocal_mixer::level_colour(crate::vocal_mixer::Level::Drifting);
        p.text(
            pos2(bx.left() + 10.0, y + 8.0),
            Align2::LEFT_CENTER,
            format!("\u{2260} {:.0}c apart (tolerance {:.0}c)", m.apart, tol),
            regular(11.0),
            amber,
        );
        let by = y + 18.0;
        let mw = w_of(&match_label, regular(11.5)) + 14.0;
        let cw = w_of(&centre_label, regular(11.5)) + 14.0;
        let mr = Rect::from_min_size(pos2(bx.left() + 8.0, by), vec2(mw, 22.0));
        let cr = Rect::from_min_size(pos2(mr.right() + 6.0, by), vec2(cw, 22.0));
        for (r, label, key) in [(mr, &match_label, "picker-match"), (cr, &centre_label, "picker-centre")] {
            let resp = ui.interact(r, area_id.with(key), Sense::click());
            p.rect(r, 4.0, if resp.hovered() { t.button_hi } else { t.button }, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
            p.text(r.center(), Align2::CENTER_CENTER, label, regular(11.5), t.text);
            store(ui.ctx(), key, r);
            if resp.clicked() {
                action = Some(if key == "picker-match" {
                    (json!({"notes": others, "to": anchor}), "Match")
                } else {
                    (json!({"notes": m.notes, "to": "centre"}), "Both to centre")
                });
            }
        }
    }
    hook(ui.ctx(), "picker-rows", row_rects);
    if let Some(notes) = chosen {
        app.vocal.studio.picker = None;
        pick(app, &notes, &editable);
        return;
    }
    if let Some((params, what)) = action {
        app.vocal.studio.picker = None;
        match app.run("vocal.match", params) {
            Ok(v) => {
                let n = v["notes"].as_array().map_or(0, Vec::len);
                app.ui.status = format!("{what}: moved {n} note{}", if n == 1 { "" } else { "s" });
            }
            Err(e) => app.ui.status = e,
        }
        return;
    }
    // Esc, or a press elsewhere (not the one that opened it), closes it.
    let (esc, pressed, at) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.pointer.any_pressed(), i.pointer.interact_pos()));
    let opened_now = pk.pass == 0 || pk.pass == pass;
    if esc || (pressed && !opened_now && at.is_some_and(|a| !bx.contains(a))) {
        app.vocal.studio.picker = None;
    }
}

/// Selects `notes` from the pick list: on an editable track as a click would; a reference
/// track's note (single-track editing) makes its track the editing one first.
fn pick(app: &mut SoundApp, notes: &[NoteRef], editable: &[TrackId]) {
    let s = app.engine.session_arc();
    let tracks: Vec<TrackId> = notes.iter().filter_map(|r| s.find_clip(r.clip).map(|(t, _)| t)).collect();
    if let [t] = tracks[..]
        && !editable.contains(&t)
        && let Err(e) = app.run("ui.vocal_studio", json!({"edit": t.0}))
    {
        app.ui.status = e;
        return;
    }
    let set: BTreeSet<NoteRef> = notes.iter().copied().collect();
    app.vocal.selected = super::roll::with_twins(app, &set);
    app.vocal.whole_track = false;
    if let Some(r) = notes.first() {
        super::preview::request(app, *r);
    }
}

/// The notes the bar's Centre note / Flatten drift / Snap to scale act on: the selection (with
/// its twins), else every note of the editable tracks.
pub fn action_notes(app: &mut SoundApp, editing: TrackId) -> Vec<NoteRef> {
    let sel = super::roll::selection_with_twins(app);
    if !sel.is_empty() {
        return sel.into_iter().collect();
    }
    edit_tracks(app, editing).into_iter().flat_map(|t| super::roll::tunable_notes(app, t)).collect()
}

/// Runs one of the bar's note actions on [`action_notes`] (one undo step across tracks).
fn note_action(app: &mut SoundApp, editing: TrackId, what: &str) {
    let notes = action_notes(app, editing);
    if notes.is_empty() {
        app.ui.status = "No notes to tune yet".into();
        return;
    }
    let (id, p, label) = match what {
        "centre" => ("vocal.step", json!({"notes": notes, "kind": "centre"}), "Centre note"),
        "drift" => ("vocal.step", json!({"notes": notes, "kind": "drift"}), "Flatten drift"),
        _ => (crate::vocal_ids::VOCAL_EDIT, json!({"notes": notes, "snap_to_key": true}), "Snap to scale"),
    };
    match app.run(id, p) {
        Ok(_) => {
            let n = notes.len();
            app.ui.status = format!("{label}: {n} note{}", if n == 1 { "" } else { "s" });
            super::preview::changed(app);
        }
        Err(e) => app.ui.status = e,
    }
}

/// The Tuning panel's chosen macro: its drift and vibrato amounts (%), for Tune all.
fn panel_amounts(app: &mut SoundApp) -> Option<(f32, f32)> {
    if app.vocal.presets.is_none() {
        app.vocal.presets = Some(super::presets::all(app));
    }
    let p = app.vocal.presets.as_ref()?.iter().find(|p| p.name == app.vocal.preset)?;
    Some(((p.drift * 100.0).round(), (p.vibrato * 100.0).round()))
}

/// Tune all on the shown tracks: waits (Tune all · analysing…) while any of them is analysed,
/// then runs `vocal.tune_all` with the Tuning panel's drift and vibrato. Returns its result, or
/// `{pending: true}` while it waits.
pub fn tune_all(app: &mut SoundApp) -> Result<Value, String> {
    let tracks = shown(app);
    if tracks.is_empty() {
        return Err("Tune all: no shown vocal track".into());
    }
    let study = studied(app);
    let waiting = study.statuses.iter().filter(|st| tracks.contains(&st.track) && st.analysing).count();
    if waiting > 0 {
        app.vocal.studio.tune_pending = true;
        app.ui.status = format!("Tune all: analysing {waiting} track{} first\u{2026}", if waiting == 1 { "" } else { "s" });
        return Ok(json!({"pending": true, "analysing": waiting}));
    }
    app.vocal.studio.tune_pending = false;
    let mut p = json!({"tracks": tracks.iter().map(|t| t.0).collect::<Vec<_>>()});
    if let Some((drift, vibrato)) = panel_amounts(app) {
        p["drift"] = json!(drift);
        p["vibrato"] = json!(vibrato);
    }
    let v = app.run("vocal.tune_all", p)?;
    let n = v["tuned"].as_array().map_or(0, Vec::len);
    let alone = v["left_alone"].as_u64().unwrap_or(0);
    let tol = v["tolerance"].as_f64().unwrap_or(10.0);
    app.ui.status = format!(
        "Tune all: tuned {n} note{} on {} track{}, left {alone} within \u{b1}{tol:.0}c alone",
        if n == 1 { "" } else { "s" },
        tracks.len(),
        if tracks.len() == 1 { "" } else { "s" }
    );
    super::preview::changed(app);
    Ok(v)
}

/// The Tolerance choices the bar offers (cents).
pub const TOLERANCES: [f32; 7] = [5.0, 8.0, 10.0, 12.0, 15.0, 20.0, 25.0];

/// The bar's Edit All tools: Centre note, Flatten drift, Snap to scale, Tolerance, Fix to and
/// Tune all.
fn edit_all_tools(app: &mut SoundApp, tb: &mut Ui, editing: TrackId) {
    let t = Tokens::current();
    let sel = !app.vocal.selected.is_empty();
    let whose = if sel {
        "the selected notes"
    } else if app.vocal.studio.edit_all {
        "every shown track's notes"
    } else {
        "the editing track's notes"
    };
    // Across tracks in Edit All only: single-track editing keeps its own tools (and Fit to lead).
    if app.vocal.studio.edit_all {
        for (key, label, what, tip) in [
            ("studio-centre", "Centre note", "centre", "Pull the notes 40 % further to their targets"),
            ("studio-drift", "Flatten drift", "drift", "Take out 30 % more of the notes' slow drift"),
            ("studio-snap", "Snap to scale", "snap", "Target the nearest scale tone instead of the nearest semitone"),
        ] {
            let b = tb.button(label);
            store(tb.ctx(), key, b.rect);
            if b.on_hover_text(format!("{tip}: {whose} (one undo step)")).clicked() {
                note_action(app, editing, what);
            }
        }
        tb.separator();
    }
    let tu = app.engine.session().studio_tuning;
    let tol = tu.tolerance();
    tb.label(egui::RichText::new("Tolerance").color(t.text));
    let mut pick_tol = None;
    let r = tb.menu_button(format!("\u{b1}{tol:.0}c"), |ui| {
        for c in TOLERANCES {
            if ui.selectable_label((c - tol).abs() < 0.05, format!("\u{b1}{c:.0}c")).clicked() {
                pick_tol = Some(c);
                ui.close();
            }
        }
    });
    store(tb.ctx(), "studio-tolerance", r.response.rect);
    r.response.on_hover_text("Tune all leaves notes already this close to their target alone; doubles further apart than this get ≠");
    if let Some(c) = pick_tol
        && let Err(e) = app.run("vocal.tolerance", json!({"cents": c}))
    {
        app.ui.status = e;
    }
    tb.label(egui::RichText::new("Fix to").color(t.text));
    let mut pick_fix = None;
    let r = tb.menu_button(if tu.fix_to_edge { "Edge" } else { "Centre" }, |ui| {
        if ui.selectable_label(!tu.fix_to_edge, "Centre").clicked() {
            pick_fix = Some("centre");
            ui.close();
        }
        if ui.selectable_label(tu.fix_to_edge, "Edge of tolerance").clicked() {
            pick_fix = Some("edge");
            ui.close();
        }
    });
    store(tb.ctx(), "studio-fix", r.response.rect);
    r.response.on_hover_text("Centre: corrected notes go to 0c. Edge: only to the tolerance, keeping more of the singer");
    if let Some(f) = pick_fix
        && let Err(e) = app.run("vocal.tolerance", json!({"fix_to": f}))
    {
        app.ui.status = e;
    }
    let green = crate::vocal_mixer::level_colour(crate::vocal_mixer::Level::InTune);
    let pending = app.vocal.studio.tune_pending;
    let label = if pending { "Tune all \u{b7} analysing\u{2026}" } else { "Tune all" };
    let b = tb.add(egui::Button::new(egui::RichText::new(label).color(green)).fill(green.gamma_multiply(0.16)).stroke(Stroke::new(1.0, green)));
    store(tb.ctx(), "studio-tune-all", b.rect);
    if b.on_hover_text(format!(
        "Tune every shown vocal with the Tuning panel's settings; notes already within \u{b1}{tol:.0}c are left alone (one undo step)"
    ))
    .clicked()
        && let Err(e) = tune_all(app)
    {
        app.ui.status = e;
    }
    // Waiting on an analysis: run once it lands.
    if app.vocal.studio.tune_pending {
        let tracks = shown(app);
        let waiting = studied(app).statuses.iter().any(|st| tracks.contains(&st.track) && st.analysing);
        if !waiting && let Err(e) = tune_all(app) {
            app.ui.status = e;
        }
        tb.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
}

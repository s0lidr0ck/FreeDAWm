//! Ghost notes in the MIDI Editor: the notes of the session's other MIDI clips over the edited
//! clip's time, drawn faintly behind its notes in each source track's colour. View only: they are
//! never selected or edited; hovering one names it, double-clicking one opens its clip.

use crate::SoundApp;
use egui::{Color32, Painter, Pos2, Rect, pos2};
use soundcraft_midi::Note;
use soundcraft_model::{ClipId, Session, TrackId};

/// The most ghost notes one frame gathers.
const MAX_GHOSTS: usize = 20_000;

/// A note of another clip, placed in the edited clip's ticks.
#[derive(Debug, Clone, PartialEq)]
pub struct Ghost {
    pub track: TrackId,
    pub track_name: String,
    pub clip: ClipId,
    pub color: [u8; 3],
    /// Its start in ticks from the edited clip's start (negative: before it).
    pub note: Note,
    /// Its index in its clip's notes.
    pub index: usize,
}

/// The ghost notes over clip `cid`: the notes (as they play) of every other MIDI clip on an
/// unmuted MIDI track, its own track's other clips included, that sound within the clip's time.
/// `only` limits them to those tracks. Muted clips show none.
pub fn ghost_notes(s: &Session, cid: ClipId, only: Option<&[TrackId]>) -> Vec<Ghost> {
    gather(s, cid, only, None)
}

/// The notes of the other MIDI clips (of `among`, when given: the whole of each one's window,
/// not only the part over `cid`'s time, muted or not), placed in `cid`'s ticks.
fn gather(s: &Session, cid: ClipId, only: Option<&[TrackId]>, among: Option<&[ClipId]>) -> Vec<Ghost> {
    let Some((_, clip)) = s.find_clip(cid) else { return Vec::new() };
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(clip.start, sr);
    let end = s.tempo.samples_to_ticks(clip.end(), sr).saturating_sub(base);
    let mut out = Vec::new();
    let wanted = |c: &soundcraft_model::Clip| match among {
        Some(a) => a.contains(&c.id),
        None => !c.muted && c.start < clip.end() && c.end() > clip.start,
    };
    for t in s.tracks.iter().filter(|t| t.kind.is_midi() && (among.is_some() || !t.mixer.mute) && only.is_none_or(|o| o.contains(&t.id))) {
        for c in t.clips().iter().filter(|c| c.id != cid && wanted(c)) {
            let Some(seq) = c.midi_playing() else { continue };
            let other = s.tempo.samples_to_ticks(c.start, sr);
            let shift = other.saturating_sub(base);
            let len = s.tempo.samples_to_ticks(c.end(), sr).saturating_sub(other);
            for (index, n) in seq.notes.iter().enumerate().filter(|(_, n)| n.start >= 0 && n.start < len) {
                let start = n.start.saturating_add(shift);
                let length = n.length.min(len.saturating_sub(n.start)).max(1);
                let over = among.is_some() || (start < end && start.saturating_add(length) > 0);
                if over && out.len() < MAX_GHOSTS {
                    out.push(Ghost {
                        track: t.id,
                        track_name: t.name.clone(),
                        clip: c.id,
                        color: t.color,
                        note: Note { start, length, ..*n },
                        index,
                    });
                }
            }
        }
    }
    out
}

/// The ghosts the editor shows for `cid` now: none when Ghosts is off, else those of the chosen
/// tracks; never those of the other selected clips, which [`siblings`] shows.
pub fn shown(app: &SoundApp, s: &Session, cid: ClipId) -> Vec<Ghost> {
    if !app.midi.ghosts {
        return Vec::new();
    }
    let others = crate::midi_editor::selected_midi_clips(s);
    let mut v = ghost_notes(s, cid, app.midi.ghost_tracks.as_deref());
    v.retain(|g| !others.contains(&g.clip));
    v
}

/// The notes of the other selected MIDI clips, each clip's whole window (the roll spans them
/// all), in `cid`'s ticks: selected, moved, resized and deleted with the edited clip's.
pub fn siblings(s: &Session, cid: ClipId) -> Vec<Ghost> {
    let others = crate::midi_editor::selected_midi_clips(s);
    if others.len() < 2 || !others.contains(&cid) {
        return Vec::new();
    }
    gather(s, cid, None, Some(&others))
}

/// Draws the other selected clips' notes: their track's colour, outlined, plainer than the
/// active clip's but not faint; the selected ones (`picked`) white.
pub fn draw_siblings(painter: &Painter, ghosts: &[Ghost], rects: &[Rect], picked: &dyn Fn(&Ghost) -> bool) {
    let view = painter.clip_rect();
    for (g, r) in ghosts.iter().zip(rects) {
        if r.intersects(view) {
            let [cr, cg, cb] = g.color;
            let fill = match (picked(g), g.note.muted) {
                (true, m) => Color32::from_white_alpha(if m { 70 } else { 230 }),
                (false, m) => Color32::from_rgba_unmultiplied(cr, cg, cb, if m { 50 } else { 150 }),
            };
            painter.rect(*r, 2.0, fill, egui::Stroke::new(1.0, Color32::from_gray(200)), egui::StrokeKind::Inside);
        }
    }
}

/// Each ghost's rect on screen.
pub fn rects(ghosts: &[Ghost], x_of: &dyn Fn(i64) -> f32, y_of: &dyn Fn(i32) -> f32, row_h: f32) -> Vec<Rect> {
    ghosts
        .iter()
        .map(|g| {
            let (x0, y) = (x_of(g.note.start), y_of(i32::from(g.note.pitch)));
            Rect::from_min_max(pos2(x0, y + 1.0), pos2(x_of(g.note.end()).max(x0 + 3.0), y + row_h - 1.0))
        })
        .collect()
}

/// Draws the ghosts: their track's colour, faint, no outline.
pub fn draw(painter: &Painter, ghosts: &[Ghost], rects: &[Rect]) {
    let view = painter.clip_rect();
    for (g, r) in ghosts.iter().zip(rects) {
        if r.intersects(view) {
            let [cr, cg, cb] = g.color;
            painter.rect_filled(*r, 2.0, Color32::from_rgba_unmultiplied(cr, cg, cb, 70));
        }
    }
}

/// The ghost under `p` (the last drawn wins), if any.
pub fn hit(rects: &[Rect], p: Pos2) -> Option<usize> {
    rects.iter().rposition(|r| r.contains(p))
}

/// "Ghost · Bass · F4 · 34.1.1": a ghost's hover details (`base`: the edited clip's first tick).
pub fn details(s: &Session, base: i64, g: &Ghost) -> String {
    format!(
        "Ghost · {} · {} · {}",
        g.track_name,
        soundcraft_midi::note_name(g.note.pitch),
        crate::midi_edit_tools::position_text(s, base.saturating_add(g.note.start))
    )
}

/// The Ghosts menu: every track, or the chosen ones.
pub fn menu(app: &mut SoundApp, ui: &mut egui::Ui, cid: ClipId) {
    let s = app.engine.session_arc();
    let own = s.find_clip(cid).map(|(t, _)| t);
    let tracks: Vec<(TrackId, String, bool)> = s.tracks.iter().filter(|t| t.kind.is_midi()).map(|t| (t.id, t.name.clone(), t.mixer.mute)).collect();
    let mut on = app.midi.ghosts;
    if ui.checkbox(&mut on, "Show ghost notes").changed() {
        app.midi.ghosts = on;
    }
    ui.separator();
    if ui.radio(app.midi.ghost_tracks.is_none(), "All tracks").clicked() {
        app.midi.ghost_tracks = None;
    }
    for (id, name, muted) in tracks {
        let mut shown = app.midi.ghost_tracks.as_ref().is_none_or(|v| v.contains(&id));
        let label = match (Some(id) == own, muted) {
            (true, _) => format!("{name} (this track's other clips)"),
            (false, true) => format!("{name} (muted)"),
            _ => name,
        };
        if ui.checkbox(&mut shown, label).changed() {
            let all: Vec<TrackId> = s.tracks.iter().filter(|t| t.kind.is_midi()).map(|t| t.id).collect();
            let mut list = app.midi.ghost_tracks.clone().unwrap_or(all);
            list.retain(|t| *t != id);
            if shown {
                list.push(id);
            }
            app.midi.ghost_tracks = Some(list);
        }
    }
}

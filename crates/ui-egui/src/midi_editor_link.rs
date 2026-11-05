//! The MIDI Editor's timeline link (spec 2026-11-04-editor-timeline-link): its 🔗 Link button,
//! following the playhead while linked, and its ruler (the main counter's format, drawn by
//! [`crate::timeline_link`]) with the playhead; a click on the ruler locates, a drag scrubs.
//! Kept out of `midi_editor.rs`, which calls in at a few places.

use crate::SoundApp;
use crate::midi_editor::{MidiView, RollGeometry};
use crate::theme::Tokens;
use crate::timeline_link::{EditorFollow, Follow};
use egui::{Rect, Sense, Stroke, Ui, pos2};
use serde_json::{Value, json};
use soundcraft_engine::cmd::editor_link::{MIDI_UNLINKED, linked as is_linked};
use soundcraft_time::{Range, Samples};

/// The ruler's height, over the roll.
pub const RULER_H: f32 = 18.0;

fn follow_id() -> egui::Id {
    egui::Id::new("soundcraft-midi-follow")
}

/// Whether the MIDI Editor is linked (on by default).
pub fn linked(app: &SoundApp) -> bool {
    is_linked(app.engine.session(), MIDI_UNLINKED)
}

/// The toolbar's 🔗 Link button.
pub fn link_button(app: &mut SoundApp, tb: &mut Ui) {
    let on = linked(app);
    let b = crate::timeline_link::link_button(tb, on);
    tb.ctx().data_mut(|d| d.insert_temp(egui::Id::new("soundcraft-midi-link"), b.rect));
    if b.clicked() {
        let _ = app.run("ui.midi_editor", json!({"linked": !on}));
    }
}

/// `ui.midi_editor {linked?}`: sets the Link when given; answers whether the editor is open,
/// its clip and whether it is linked.
pub fn command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    if let Some(v) = p.get("linked") {
        let on = v.as_bool().ok_or("`linked` must be true or false")?;
        app.run("view.midi_editor_link", json!({"value": on}))?;
    }
    Ok(json!({"open": app.ui.show_midi_editor, "clip": app.midi.clip.map(|c| c.0), "linked": linked(app)}))
}

/// Session time of `tick` (from the clip's start at `base` ticks).
fn time_of(app: &SoundApp, base: i64, tick: f64) -> Samples {
    let s = app.engine.session();
    s.tempo.tick_to_samples(base.saturating_add(tick.round().clamp(-1e15, 1e15) as i64), s.sample_rate)
}

/// Linked, moves `view` so the playhead stays in it (Options › Scrolling); called once a frame
/// after the toolbar, wheel and scrollbar had their say (`before`: the view before them, to tell
/// a pan or zoom by hand). A fitted view shows the whole clip and never moves.
pub fn follow(app: &mut SoundApp, ctx: &egui::Context, view: &mut MidiView, before: MidiView, width: f32, base: i64) {
    let mut st: EditorFollow = ctx.data(|d| d.get_temp(follow_id())).unwrap_or_default();
    // A note audition plays and puts the playhead back: not a locate, nothing to follow.
    if app.midi.auditioning.is_none() {
        st.user_moved |= view.zoom != before.zoom || (view.left - before.left).abs() > 0.5;
        let ppt = f64::from(view.zoom.unwrap_or(1.0).max(1e-6));
        let left = time_of(app, base, view.left);
        let span = time_of(app, base, view.left + f64::from(width) / ppt).saturating_sub(left);
        let (mode, jump) = {
            let s = app.engine.session();
            (Follow::of(s), crate::timeline_link::frame_jump(s.sample_rate.hz()))
        };
        let (on, playing, pos) = (linked(app), app.is_playing(), app.position());
        if let Some(to) = st.update(on, mode, playing, pos, left, span, jump)
            && view.zoom.is_some()
        {
            let s = app.engine.session();
            view.left = (s.tempo.samples_to_ticks(to, s.sample_rate).saturating_sub(base)) as f64;
        }
    }
    ctx.data_mut(|d| d.insert_temp(follow_id(), st));
}

/// The ruler over the roll (`row`) and the playhead across it and the roll; a click on the
/// ruler locates, a drag scrubs (inside `clip`, the clip's span: the insertion point it leaves
/// keeps the editor on this clip).
pub fn ruler(app: &mut SoundApp, ui: &Ui, row: Rect, geo: &RollGeometry, base: i64, clip: Range) {
    let s = app.engine.session_arc();
    let sr = s.sample_rate;
    let x_of = |at: Samples| geo.x_of(s.tempo.samples_to_ticks(at, sr).saturating_sub(base));
    let view = Range::new(time_of(app, base, geo.tick_at(row.min.x)), time_of(app, base, geo.tick_at(row.max.x)));
    let pos = app.position();
    crate::timeline_link::editor_ruler(ui, &s, "midi", row, view, &x_of, Some(pos));
    if pos >= view.start && pos <= view.end {
        let x = x_of(pos);
        ui.painter()
            .with_clip_rect(geo.roll)
            .line_segment([pos2(x, geo.roll.min.y), pos2(x, geo.roll.max.y)], Stroke::new(1.5, Tokens::current().playhead));
    }
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("soundcraft-midi-ruler"), row));
    let resp = ui.interact(row, ui.id().with("midi-ruler"), Sense::click_and_drag());
    if (resp.clicked() || resp.dragged())
        && let Some(p) = resp.interact_pointer_pos()
    {
        // An insertion point there, then the locate (as the Edit window's and the Vocal Editor's
        // rulers): a locate alone keeps a selection that holds the time.
        let at = time_of(app, base, geo.tick_at(p.x)).clamp(clip.start, clip.end.saturating_sub(1).max(clip.start)).max(0);
        let _ = app.run("edit.select", json!({"start": at}));
        let _ = app.run("transport.locate", json!({"at": at}));
    }
}

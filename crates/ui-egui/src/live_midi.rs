//! The MIDI take a track is recording, drawn as it grows (as an audio take draws its waveform):
//! a clip in the record colour from where the take started to the playhead, the notes heard so
//! far on the track's input, and an activity light lit while notes arrive. A remote recorder
//! following a booth (`net_ui`) records this way too, so its screen shows the take it records.

use crate::SoundApp;
use crate::theme::{Tokens, clip_colors};
use egui::{Color32, CornerRadius, Rect, Stroke, StrokeKind, pos2};
use soundcraft_model::{Route, Session, Track};
use soundcraft_time::Samples;
use std::time::Duration;

/// How long the activity light stays lit after a message.
const LIGHT: Duration = Duration::from_millis(200);

/// One note of a live take: its pitch, where it starts and (released) ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveNote {
    pub pitch: u8,
    pub from: Samples,
    pub to: Option<Samples>,
}

/// What a track's live MIDI take holds now.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveTake {
    pub start: Samples,
    pub end: Samples,
    pub notes: Vec<LiveNote>,
    /// A message arrived on the track's input in the last moment.
    pub lit: bool,
}

/// Whether `track` records live MIDI (armed, a MIDI or instrument track on a MIDI input) and,
/// if so, the port it listens to (`None`: all inputs).
fn records_midi(track: &Track) -> Option<Option<&str>> {
    if !track.mixer.record_arm || !track.kind.is_midi() || track.inactive {
        return None;
    }
    match &track.mixer.input {
        Route::Midi(m) => Some(m.device.as_deref()),
        _ => None,
    }
}

/// The take `track` is recording now, or `None` (not recording, or not on a MIDI input).
pub fn live_take(app: &SoundApp, track: &Track) -> Option<LiveTake> {
    if !app.engine.transport.recording || app.countoff_status().is_some() {
        return None;
    }
    let port = records_midi(track)?;
    // A remote recorder's track on the booth's own remote port is drawn by `net_ui`.
    if port.is_some_and(|p| p.starts_with("Remote: ")) {
        return None;
    }
    let start = app.take_plan.as_ref().map_or(app.record_start, |p| p.start);
    let end = app.position().max(start);
    let events = crate::midi_devices::recorded_events(app, &app.midi_rec);
    let mine = |name: &str| port.is_none_or(|p| p == name);
    let mut notes: Vec<LiveNote> = Vec::new();
    for e in events.iter().filter(|e| mine(&e.port)) {
        let kind = e.msg[0] & 0xF0;
        let pitch = e.msg[1];
        if kind == 0x90 && e.msg[2] > 0 {
            notes.push(LiveNote { pitch, from: e.at, to: None });
        } else if (kind == 0x80 || kind == 0x90)
            && let Some(n) = notes.iter_mut().rev().find(|n| n.pitch == pitch && n.to.is_none())
        {
            n.to = Some(e.at.max(n.from));
        }
    }
    Some(LiveTake { start, end, notes, lit: lit(app, port) })
}

/// A message arrived on `port` (`None`: any) within [`LIGHT`].
fn lit(app: &SoundApp, port: Option<&str>) -> bool {
    let Some(hub) = &app.midi_hub else { return false };
    let now = hub.now_us();
    let recent = |slot: usize| {
        let at = hub.last_activity(slot);
        at > 0 && now.saturating_sub(at) < LIGHT.as_micros() as u64
    };
    let names: Vec<(usize, String)> = {
        let mut v: Vec<(usize, String)> =
            app.midi_inputs.as_ref().map(|m| m.ports().into_iter().map(|p| (p.slot, p.name)).collect()).unwrap_or_default();
        v.push((soundcraft_playback::midi_in::TYPING_SLOT, soundcraft_playback::midi_in::TYPING_NAME.to_string()));
        v
    };
    names.iter().filter(|(_, n)| port.is_none_or(|p| p == n)).any(|(slot, _)| recent(*slot))
}

/// Draws `track`'s live MIDI take over its lane.
pub fn draw(app: &SoundApp, painter: &egui::Painter, s: &Session, track: &Track, tl: Rect, lane: Rect) {
    let Some(take) = live_take(app, track) else { return };
    let x = |at: Samples| crate::edit_window::x_of(s, tl, at);
    let x0 = x(take.start);
    let x1 = x(take.end).max(x0 + 2.0);
    let r = Rect::from_min_max(pos2(x0, lane.min.y + 1.0), pos2(x1, lane.max.y - 1.0));
    let t = Tokens::current();
    let (body, _, wave) = clip_colors([t.rec.r(), t.rec.g(), t.rec.b()], false);
    painter.rect(r, CornerRadius::same(2), body, Stroke::new(1.0, Color32::from_rgb(10, 10, 10)), StrokeKind::Inside);
    let dot = pos2(r.min.x + 8.0, r.min.y + 8.0);
    if lane.height() >= 18.0 {
        painter.circle(dot, 4.0, if take.lit { Color32::from_rgb(90, 220, 110) } else { Color32::from_gray(60) }, Stroke::new(1.0, Color32::BLACK));
        let n = take.notes.len();
        let label = format!("{} notes", n);
        painter.with_clip_rect(r).text(pos2(dot.x + 8.0, dot.y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(11.0), t.text);
    }
    // The notes over the lane's height (the range played, at least an octave).
    let top = if lane.height() >= 30.0 { 16.0 } else { 2.0 };
    let body_r = Rect::from_min_max(pos2(r.min.x, r.min.y + top), pos2(r.max.x, r.max.y - 3.0));
    if body_r.height() < 4.0 {
        return;
    }
    let lo = take.notes.iter().map(|n| n.pitch).min().unwrap_or(48);
    let hi = take.notes.iter().map(|n| n.pitch).max().unwrap_or(72).max(lo.saturating_add(12));
    let span = f32::from(hi - lo).max(1.0);
    let y = |p: u8| body_r.max.y - f32::from(p.saturating_sub(lo)) / span * body_r.height();
    let p = painter.with_clip_rect(body_r.intersect(lane));
    for n in &take.notes {
        let a = x(n.from);
        let b = n.to.map_or(x1, x).max(a + 2.0);
        p.line_segment([pos2(a, y(n.pitch)), pos2(b, y(n.pitch))], Stroke::new(2.0, wave));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use soundcraft_playback::midi_in::RecordedMidi;

    fn armed_piano() -> SoundApp {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let id = app.engine.execute("track.new", &json!({"kind": "instrument", "name": "Piano"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.engine.execute("track.input", &json!({"tracks": [id], "input": "midi:all"})).unwrap();
        app.engine.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
        app
    }

    #[test]
    fn a_midi_take_grows_with_its_notes_while_it_records() {
        let mut app = armed_piano();
        let track = app.engine.session().tracks[0].clone();
        assert_eq!(live_take(&app, &track), None, "not recording");
        app.engine.transport.recording = true;
        app.engine.transport.playing = true;
        app.record_start = 48_000;
        app.engine.transport.position = 60_000;
        app.midi_rec.push(RecordedMidi { at: 50_000, port: soundcraft_playback::midi_in::TYPING_SLOT as u8, msg: [0x90, 60, 100], pass: 0 });
        let first = live_take(&app, &track).unwrap();
        assert_eq!((first.start, first.end), (48_000, 60_000));
        assert_eq!(first.notes, vec![LiveNote { pitch: 60, from: 50_000, to: None }]);
        app.engine.transport.position = 96_000;
        app.midi_rec.push(RecordedMidi { at: 70_000, port: soundcraft_playback::midi_in::TYPING_SLOT as u8, msg: [0x80, 60, 0], pass: 0 });
        app.midi_rec.push(RecordedMidi { at: 80_000, port: soundcraft_playback::midi_in::TYPING_SLOT as u8, msg: [0x90, 64, 90], pass: 0 });
        let later = live_take(&app, &track).unwrap();
        assert_eq!(later.end, 96_000, "it grows with the playhead");
        assert_eq!(later.notes, vec![LiveNote { pitch: 60, from: 50_000, to: Some(70_000) }, LiveNote { pitch: 64, from: 80_000, to: None }]);
    }

    #[test]
    fn a_track_on_another_port_shows_none_of_its_notes() {
        let mut app = armed_piano();
        let id = app.engine.session().tracks[0].id.0;
        app.engine.execute("track.input", &json!({"tracks": [id], "input": "midi:Digital Piano"})).unwrap();
        let track = app.engine.session().tracks[0].clone();
        app.engine.transport.recording = true;
        app.midi_rec.push(RecordedMidi { at: 10, port: soundcraft_playback::midi_in::TYPING_SLOT as u8, msg: [0x90, 60, 100], pass: 0 });
        assert!(live_take(&app, &track).unwrap().notes.is_empty());
    }
}

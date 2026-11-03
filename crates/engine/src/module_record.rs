//! Record a track's module MIDI input (`Route::Modules`: Shale's hits into an instrument) as a
//! MIDI clip: the session is rendered offline over the range through the mix engine, exactly as
//! playback renders it, and what the track's instrument was given is written down. The events
//! carry the position of the block their source module emitted them in (a module's own lookahead
//! lands a hit that much after the audio); the mix's delay compensation is not applied to them.

use soundcraft_midi::Sequence;
use soundcraft_mix::RangeRender;
use soundcraft_model::{Clip, ClipId, Route, Session, TrackId};
use soundcraft_time::{Range, Samples};

use crate::Engine;

/// The engine's block for offline prints.
const BLOCK: usize = 512;

/// What each of `tracks` (instrument or MIDI tracks whose input is modules' MIDI) is given over
/// `range`, as `(absolute sample, message)` in delivery order. Tracks without such an input get
/// an empty list. Allocates: off the audio thread.
pub fn render_module_midi(s: &Session, range: Range, tracks: &[TrackId]) -> Vec<(TrackId, Vec<(Samples, [u8; 3])>)> {
    let mut out: Vec<(TrackId, Vec<(Samples, [u8; 3])>)> = tracks.iter().map(|t| (*t, Vec::new())).collect();
    if range.is_empty() || !tracks.iter().any(|t| s.track(*t).is_some_and(|t| matches!(t.mixer.input, Route::Modules(_)))) {
        return out;
    }
    let mut rr = RangeRender::new(s, None, range, BLOCK);
    while rr.next_block(s).is_some() {
        let (pos, n) = rr.last_block();
        for (t, events) in &mut out {
            for (offset, msg) in rr.module_midi_in(*t) {
                if (offset as usize) < n {
                    let at = pos.saturating_add(i64::from(offset));
                    if range.contains(at) {
                        events.push((at, msg));
                    }
                }
            }
        }
    }
    out
}

/// Records the module MIDI input of `tracks` (or, with `None`, of every record-armed instrument
/// or MIDI track whose input is modules) over `range` as one MIDI clip per track that received
/// anything (channels kept, as a printed band clip), one undo step "Record". Frozen tracks and
/// tracks without such an input are skipped. Returns the clips.
pub fn record_module_inputs(e: &mut Engine, range: Range, tracks: Option<&[TrackId]>) -> Vec<ClipId> {
    let s = e.session();
    let wanted: Vec<TrackId> = s
        .tracks
        .iter()
        .filter(|t| t.kind.is_midi() && matches!(t.mixer.input, Route::Modules(_)) && t.frozen_audio().is_none())
        .filter(|t| tracks.map_or(t.mixer.record_arm, |w| w.contains(&t.id)))
        .map(|t| t.id)
        .collect();
    if wanted.is_empty() || range.is_empty() {
        return Vec::new();
    }
    let rendered = render_module_midi(s, range, &wanted);
    let before = e.session_arc();
    let mut out = Vec::new();
    let s = e.session_mut();
    for (t, events) in rendered {
        if events.is_empty() {
            continue;
        }
        let seq: Sequence = crate::band_print::events_to_sequence(s, range, &events.iter().map(|(at, m)| (*at, *m, true)).collect::<Vec<_>>());
        if seq.notes.is_empty() && seq.ctrls.is_empty() {
            continue;
        }
        let id = s.new_clip_id();
        let name = s.track(t).map(|x| x.name.clone()).unwrap_or_default();
        let mut clip = Clip::midi(id, name, range.start, range.len(), seq);
        clip.midi_channels = true;
        crate::edit::place_clip(s, t, clip);
        out.push(id);
    }
    if !out.is_empty() {
        e.push_undo("Record", before);
    }
    out
}

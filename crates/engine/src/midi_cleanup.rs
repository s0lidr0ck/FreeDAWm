//! MIDI Cleanup's derived preview: the inputs the cleanup reads from the session (the clip's
//! notes, the band's chord track, the key, the tempo, the clip's settings), a fingerprint of them,
//! and the refresh that recomputes a clip's cleaned notes only when the fingerprint changed.

use soundcraft_band::chord_match::slot_pitch_classes;
use soundcraft_band::chord_theory::{scale_offset, shape_for};
use soundcraft_midi::CtrlKind;
use soundcraft_midi::cleanup::{ChordSpan, CleanupInput, cleanup};
use soundcraft_model::{CleanupPreview, Clip, ClipContent, Session, Track};
use soundcraft_time::TICKS_PER_QUARTER;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// The band's chords that overlap `clip`, in ticks from the clip's start (clipped at neither end:
/// a chord started before the clip has a negative start).
pub fn chord_spans(s: &Session, clip: &Clip) -> Vec<ChordSpan> {
    let band = &s.band;
    let (key, scale) = (i32::from(band.key), i32::from(band.scale));
    let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
    let clip_end = s.tempo.samples_to_ticks(clip.end(), s.sample_rate).saturating_sub(base);
    let mut spans: Vec<ChordSpan> = band
        .chords
        .iter()
        .filter(|c| c.length_ticks > 0)
        .filter_map(|c| {
            let start = c.tick.saturating_sub(base);
            let end = start.saturating_add(c.length_ticks);
            if start >= clip_end || end <= 0 {
                return None;
            }
            let slot = i32::from(c.slot);
            let mut tones = slot_pitch_classes(slot, key, scale);
            if c.bass >= 0 {
                tones[(key + i32::from(c.bass)).rem_euclid(12) as usize] = true;
            }
            let root = shape_for(slot, key, scale, -1).root_pc.rem_euclid(12) as u8;
            Some(ChordSpan { start, end, root, tones })
        })
        .collect();
    spans.sort_by_key(|c| (c.start, c.end));
    spans
}

/// The session key's scale tones: the seven degrees; a minor key also has its raised 7th.
pub fn key_tones(s: &Session) -> [bool; 12] {
    let (key, scale) = (i32::from(s.band.key), i32::from(s.band.scale));
    let mut t = [false; 12];
    for d in 0..7 {
        t[(key + scale_offset(scale, d)).rem_euclid(12) as usize] = true;
    }
    if scale == 1 {
        t[(key + 11).rem_euclid(12) as usize] = true;
    }
    t
}

/// Ticks per millisecond at the tempo in effect where `clip` starts.
pub fn ticks_per_ms(s: &Session, clip: &Clip) -> f64 {
    let at = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
    TICKS_PER_QUARTER as f64 * s.tempo.tempo_at_tick(at) / 60_000.0
}

fn ctrl_key(k: CtrlKind) -> (u8, u8) {
    match k {
        CtrlKind::Cc(n) => (0, n),
        CtrlKind::PitchBend => (1, 0),
        CtrlKind::ChannelPressure => (2, 0),
        CtrlKind::PolyPressure(p) => (3, p),
        CtrlKind::Program => (4, 0),
    }
}

/// Everything a clip's cleanup reads, with the fingerprint of it.
struct Inputs {
    chords: Vec<ChordSpan>,
    tones: [bool; 12],
    tpm: f64,
    print: u64,
}

fn inputs(s: &Session, clip: &Clip, tones: [bool; 12]) -> Option<Inputs> {
    let (ClipContent::Midi { sequence }, Some(m)) = (&clip.content, &clip.midi_cleanup) else { return None };
    let chords = chord_spans(s, clip);
    let tpm = ticks_per_ms(s, clip);
    let mut h = DefaultHasher::new();
    sequence.notes.len().hash(&mut h);
    for n in &sequence.notes {
        (n.pitch, n.velocity, n.release_velocity, n.channel, n.start, n.length).hash(&mut h);
    }
    sequence.ctrls.len().hash(&mut h);
    for c in &sequence.ctrls {
        (c.tick, c.channel, ctrl_key(c.kind), c.value).hash(&mut h);
    }
    chords.len().hash(&mut h);
    for c in &chords {
        (c.start, c.end, c.root, c.tones).hash(&mut h);
    }
    tones.hash(&mut h);
    tpm.to_bits().hash(&mut h);
    m.settings.hash(&mut h);
    Some(Inputs { chords, tones, tpm, print: h.finish() })
}

/// The fingerprint of everything `clip`'s cleanup reads (its notes, the chords over it, the key,
/// the tempo, its settings); `None` for a clip without a cleanup. Two equal prints derive the
/// same cleaned notes.
pub(crate) fn fingerprint(s: &Session, clip: &Clip) -> Option<u64> {
    inputs(s, clip, key_tones(s)).map(|i| i.print)
}

/// Whether a clip's preview is kept as it is: on a frozen track, the render played the cleaned
/// notes of the time, so a chord, key or tempo edit elsewhere must not move them under it (a
/// preview still missing, as after a load, is derived: its inputs are those the render saw).
fn kept(t: &Track, c: &Clip) -> bool {
    t.frozen_audio().is_some() && c.midi_cleanup.as_ref().is_some_and(|m| m.preview.is_some())
}

/// Whether any clip's cleanup preview is missing or made from other inputs than the session's now
/// (a frozen track's previews count as current, see [`kept`]).
pub(crate) fn any_stale(s: &Session) -> bool {
    if !s.all_clips().any(|c| c.midi_cleanup.is_some()) {
        return false;
    }
    let tones = key_tones(s);
    s.tracks.iter().flat_map(|t| t.playlists.iter().flat_map(move |p| p.clips.iter().map(move |c| (t, c)))).any(|(t, c)| {
        !kept(t, c)
            && inputs(s, c, tones).is_some_and(|i| c.midi_cleanup.as_ref().and_then(|m| m.preview.as_ref()).is_none_or(|p| p.inputs != i.print))
    })
}

/// Recomputes the cleanup preview of every clip whose inputs changed (and only those; a frozen
/// track's are kept, see [`kept`]).
pub(crate) fn refresh(s: &mut Session) {
    if !s.all_clips().any(|c| c.midi_cleanup.is_some()) {
        return;
    }
    let tones = key_tones(s);
    let mut fresh: Vec<(usize, usize, usize, Arc<CleanupPreview>)> = Vec::new();
    for (ti, t) in s.tracks.iter().enumerate() {
        for (pi, p) in t.playlists.iter().enumerate() {
            for (ci, c) in p.clips.iter().enumerate() {
                if kept(t, c) {
                    continue;
                }
                let Some(i) = inputs(s, c, tones) else { continue };
                if c.midi_cleanup.as_ref().and_then(|m| m.preview.as_ref()).is_some_and(|p| p.inputs == i.print) {
                    continue;
                }
                let (ClipContent::Midi { sequence }, Some(m)) = (&c.content, &c.midi_cleanup) else { continue };
                let out = cleanup(&CleanupInput {
                    notes: &sequence.notes,
                    ctrls: &sequence.ctrls,
                    chords: &i.chords,
                    key_tones: i.tones,
                    ticks_per_ms: i.tpm,
                    settings: m.settings,
                });
                fresh.push((ti, pi, ci, Arc::new(CleanupPreview { inputs: i.print, out })));
            }
        }
    }
    for (ti, pi, ci, preview) in fresh {
        if let Some(m) =
            s.tracks.get_mut(ti).and_then(|t| t.playlists.get_mut(pi)).and_then(|p| p.clips.get_mut(ci)).and_then(|c| c.midi_cleanup.as_mut())
        {
            m.preview = Some(preview);
        }
    }
}

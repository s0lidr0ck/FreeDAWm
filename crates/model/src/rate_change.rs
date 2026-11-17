//! Changing a session's sample rate after it was made: every position stored in samples is
//! scaled to the new rate ([`scale_positions`]), so each lands where it was in time. The audio
//! itself (the pool's sources) is converted by the engine (`session.set_sample_rate`).
//!
//! Completeness is the risk: a position missed here plays in the wrong place. The test
//! `every_integer_field_is_known` walks a session's saved form and fails on an integer field
//! this module has not classified, so a new `Samples` field cannot be added without it.

use crate::{AutomationLane, Clip, ClipContent, Session};
use soundcraft_time::{Range, Samples};

/// A position at the old rate, at the new one: scaled by `ratio` (new ÷ old) and rounded.
fn at(p: Samples, ratio: f64) -> Samples {
    let v = (p as f64 * ratio).round();
    if v.is_finite() { v.clamp(i64::MIN as f64, i64::MAX as f64) as Samples } else { p }
}

/// A length: as [`at`], at least 1 when it was.
fn len(p: Samples, ratio: f64) -> Samples {
    let v = at(p, ratio);
    if p > 0 { v.max(1) } else { v }
}

fn range(r: Range, ratio: f64) -> Range {
    Range::new(at(r.start, ratio), at(r.end, ratio))
}

fn lanes(l: &mut [AutomationLane], ratio: f64) {
    for lane in l {
        for q in &mut lane.points {
            q.at = at(q.at, ratio);
        }
        lane.points.dedup_by_key(|q| q.at);
    }
}

fn clip(c: &mut Clip, ratio: f64) {
    c.start = at(c.start, ratio);
    c.length = len(c.length, ratio);
    match &mut c.content {
        ClipContent::Audio { offset, .. } | ClipContent::Video { offset, .. } => *offset = at(*offset, ratio),
        ClipContent::Midi { .. } => {}
    }
    c.fade_in.len = len(c.fade_in.len, ratio);
    c.fade_out.len = len(c.fade_out.len, ratio);
    for (t, _) in &mut c.gain_env {
        *t = at(*t, ratio);
    }
    c.sync_point = at(c.sync_point, ratio);
    for (a, b) in &mut c.warp {
        (*a, *b) = (at(*a, ratio), at(*b, ratio));
    }
    c.loop_len = len(c.loop_len, ratio);
    c.loop_phase = at(c.loop_phase, ratio);
    for line in &mut c.lyrics {
        (line.start, line.end) = (at(line.start, ratio), at(line.end, ratio));
        for w in &mut line.words {
            (w.start, w.end) = (at(w.start, ratio), at(w.end, ratio));
        }
    }
    if let Some(alt) = &mut c.clean {
        alt.origin = at(alt.origin, ratio);
    }
    lanes(&mut c.automation, ratio);
    c.clamp_fades();
}

/// Scales every position `s` stores in samples from its rate to `to`, and sets the session's
/// rate. Tick-timebase material (MIDI notes, the tempo map's events, the band) keeps
/// its ticks; the tempo map's origin is a sample position and is scaled. Renders made at the
/// old rate (live stretches, pitch renders, the vocal cache) are dropped to be made again.
pub fn scale_positions(s: &mut Session, to: soundcraft_time::SampleRate) {
    let ratio = to.as_f64() / s.sample_rate.as_f64().max(1.0);
    s.sample_rate = to;
    s.timecode_start = at(s.timecode_start, ratio);
    let origin = s.tempo.origin();
    s.tempo.set_origin(at(origin, ratio));
    for (p, _) in &mut s.key_signatures {
        *p = at(*p, ratio);
    }
    for (p, _) in &mut s.transpose_track {
        *p = at(*p, ratio);
    }
    for t in &mut s.tracks {
        for pl in &mut t.playlists {
            for c in &mut pl.clips {
                clip(c, ratio);
            }
        }
        lanes(&mut t.automation, ratio);
        if let Some(f) = &mut t.freeze {
            f.start = at(f.start, ratio);
        }
        t.played.clear();
        t.refresh_played();
    }
    for c in &mut s.captured {
        clip(c, ratio);
    }
    for m in &mut s.markers {
        (m.start, m.end) = (at(m.start, ratio), at(m.end, ratio));
        if let Some(v) = &mut m.view {
            v.scroll = at(v.scroll, ratio);
            v.samples_per_px *= ratio;
        }
    }
    for n in &mut s.notes {
        n.at = n.at.map(|p| at(p, ratio));
    }
    for p in &mut s.lyric_sheet.placed {
        (p.start, p.end) = (at(p.start, ratio), at(p.end, ratio));
    }
    let e = &mut s.edit;
    e.selection = range(e.selection, ratio);
    e.loop_range = e.loop_range.map(|r| range(r, ratio));
    for r in &mut e.skips {
        *r = range(*r, ratio);
    }
    e.playhead = at(e.playhead, ratio);
    e.pre_roll = len(e.pre_roll, ratio);
    e.post_roll = len(e.post_roll, ratio);
    e.timeline_selection = range(e.timeline_selection, ratio);
    s.live_stretch.clear();
    s.pitch_renders = Default::default();
    s.vocal = Default::default();
}

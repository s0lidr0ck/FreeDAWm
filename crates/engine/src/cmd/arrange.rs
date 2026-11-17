//! The arranger: the song lane's section blocks (the Band's Song lane) as handles on the whole
//! song. Copy, move or remove a section and everything in its time goes with it: every track's
//! clips (the playlist shown) and automation, the Band's chords and the block itself; later
//! material makes room or closes up, as Insert Time and Cut Time do.
//!
//! A section must keep one tempo and meter through it, and a copy goes only where the same tempo
//! and meter are in force: the material is copied in samples, so a different tempo would leave it
//! off the bars. Markers, key changes and take layers other than the one shown are not copied.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{AutomationPoint, ChordEvent, Clip, SectionBlock, TrackId};
use soundcraft_time::Range;
use soundcraft_time::Samples;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "arrange.copy",
            "Copy Section with the Song",
            [],
            None,
            "{block, to?: position (default: right after it; snapped to the bar)} — makes room at `to` and copies the section there: every track's clips and automation, the chords and the block",
            always,
            copy
        ),
        cmd!(
            "arrange.move",
            "Move Section with the Song",
            [],
            None,
            "{block, to: position (snapped to the bar)} — the section and everything in its time go to `to`; what was after it closes up",
            always,
            move_section
        ),
        cmd!(
            "arrange.remove",
            "Remove Section with the Song",
            [],
            None,
            "{block} — cuts the section's time from the song: its clips, automation, chords and block go and what follows closes up",
            always,
            remove
        ),
    ]
}

/// A block's span: (start tick, length in ticks, samples).
fn block_span(e: &Engine, cmd: &str, p: &Value) -> Result<(SectionBlock, Range)> {
    let id = p.get("block").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`block` required"))?;
    let s = e.session();
    let b = s.band.sections.iter().find(|b| u64::from(b.id) == id).cloned().ok_or_else(|| bad(cmd, format!("no section block {id}")))?;
    if b.length_ticks <= 0 {
        return Err(bad(cmd, "the section is empty"));
    }
    let at = |t: i64| s.tempo.tick_to_samples(t, s.sample_rate);
    Ok((b.clone(), Range::new(at(b.start_tick), at(b.start_tick.saturating_add(b.length_ticks)))))
}

/// Whether the tempo and meter stay the same from `from` through `to` (ticks; `to` excluded).
fn uniform(s: &soundcraft_model::Session, from: i64, to: i64) -> bool {
    let t = &s.tempo;
    !t.in_ramp(from) && !t.tempos().iter().any(|e| e.tick > from && e.tick < to) && !t.meters().iter().any(|m| m.tick > from && m.tick < to)
}

/// The bar line nearest `at`.
fn bar_at(s: &soundcraft_model::Session, at: Samples) -> i64 {
    let tick = s.tempo.samples_to_ticks(at.max(0), s.sample_rate).max(0);
    let bb = s.tempo.bar_beat_at_tick(tick);
    let this = s.tempo.bar_start_tick(bb.bar);
    let next = s.tempo.bar_start_tick(bb.bar.saturating_add(1));
    if tick - this <= next - tick { this } else { next }
}

/// What a section holds, copied out before the song is changed.
struct Material {
    clips: Vec<(TrackId, Vec<Clip>)>,
    /// (track, lane index, points relative to the section's start).
    automation: Vec<(TrackId, usize, Vec<AutomationPoint>)>,
    /// Chords relative to the section's start tick.
    chords: Vec<ChordEvent>,
}

fn take(s: &soundcraft_model::Session, b: &SectionBlock, r: Range) -> Material {
    let mut m = Material { clips: Vec::new(), automation: Vec::new(), chords: Vec::new() };
    for t in &s.tracks {
        let clips = crate::edit::copy_range(s, t.id, r);
        if !clips.is_empty() {
            m.clips.push((t.id, clips));
        }
        for (i, lane) in t.automation.iter().enumerate() {
            let pts: Vec<AutomationPoint> =
                lane.points.iter().filter(|p| r.contains(p.at)).map(|p| AutomationPoint { at: p.at - r.start, ..*p }).collect();
            if !pts.is_empty() {
                m.automation.push((t.id, i, pts));
            }
        }
    }
    let end = b.start_tick.saturating_add(b.length_ticks);
    m.chords = s
        .band
        .chords
        .iter()
        .filter(|c| c.tick >= b.start_tick && c.tick < end)
        .map(|c| ChordEvent { tick: c.tick - b.start_tick, length_ticks: c.length_ticks.min(end - c.tick), ..c.clone() })
        .collect();
    m
}

/// Copies block `b` (samples `r`) to tick `to`, making room there. Returns where it went (samples).
fn copy_to(e: &mut Engine, cmd: &str, b: &SectionBlock, r: Range, to: i64) -> Result<Range> {
    let s = e.session();
    let end = b.start_tick.saturating_add(b.length_ticks);
    if !uniform(s, b.start_tick, end) {
        return Err(bad(cmd, "the section changes tempo or meter inside; it can't be copied as a whole"));
    }
    let (bpm, meter) = (s.tempo.tempo_at_tick(b.start_tick), s.tempo.meter_at_tick(b.start_tick));
    let there = s.tempo.meter_at_tick(to);
    if (s.tempo.tempo_at_tick(to) - bpm).abs() > 1e-9
        || there.numerator != meter.numerator
        || there.denominator != meter.denominator
        || s.tempo.in_ramp(to)
    {
        return Err(bad(cmd, "the tempo or meter there differs from the section's; copy it where they match"));
    }
    if s.band.sections.len() >= 4096 {
        return Err(bad(cmd, "the song lane is full"));
    }
    let material = take(s, b, r);
    let at = s.tempo.tick_to_samples(to, s.sample_rate);
    super::event::insert_time(e, &json!({"start": at, "length": r.len()}))?;
    let s = e.session_mut();
    for (t, clips) in &material.clips {
        let _ = crate::edit::paste(s, *t, at, clips, r.len(), false);
    }
    for (t, i, pts) in &material.automation {
        if let Some(lane) = s.track_mut(*t).and_then(|tr| tr.automation.get_mut(*i)) {
            lane.points.extend(pts.iter().map(|p| AutomationPoint { at: p.at + at, ..*p }));
            lane.points.sort_by_key(|p| p.at);
        }
    }
    // The Band's block and chords in the room made for them.
    let to = s.tempo.samples_to_ticks(at, s.sample_rate);
    for c in material.chords {
        let id = super::band::take_id(&mut s.band.next_id);
        s.band.chords.push(ChordEvent { id, tick: c.tick.saturating_add(to), ..c });
    }
    let id = super::band::take_id(&mut s.band.next_id);
    s.band.sections.push(SectionBlock { id, start_tick: to, ..b.clone() });
    s.band.sanitize();
    Ok(Range::new(at, at.saturating_add(r.len())))
}

fn copy(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "arrange.copy";
    let (b, r) = block_span(e, C, p)?;
    let to = match position_param(e, C, p, "to")? {
        Some(at) => bar_at(e.session(), at),
        None => b.start_tick.saturating_add(b.length_ticks),
    };
    let placed = copy_to(e, C, &b, r, to)?;
    Ok(json!({"start": placed.start, "end": placed.end}))
}

fn move_section(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "arrange.move";
    let (b, r) = block_span(e, C, p)?;
    let at = position_param(e, C, p, "to")?.ok_or_else(|| bad(C, "`to` required"))?;
    let to = bar_at(e.session(), at);
    let end = b.start_tick.saturating_add(b.length_ticks);
    if to >= b.start_tick && to <= end {
        return Err(bad(C, "the section is already there"));
    }
    let placed = copy_to(e, C, &b, r, to)?;
    // The original, after the copy made room before it if it went earlier.
    let old = if to < b.start_tick { Range::new(r.start + r.len(), r.end + r.len()) } else { r };
    super::event::cut_time(e, &json!({"start": old.start, "end": old.end}))?;
    let start = if to < b.start_tick { placed.start } else { placed.start - r.len() };
    Ok(json!({"start": start, "end": start + r.len()}))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "arrange.remove";
    let (_, r) = block_span(e, C, p)?;
    super::event::cut_time(e, &json!({"start": r.start, "end": r.end}))?;
    Ok(json!({"removed": r.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{AutoParam, AutomationLane, ChannelFormat, TrackKind};

    /// 120 bpm, 4/4 at 48 kHz: a bar is 96 000 samples and 3 840 ticks. Sections A (bars 1-2)
    /// and B (bars 3-4); a MIDI clip in each, a volume point in A, a chord in B.
    fn song() -> (Engine, TrackId, u64, u64) {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        for (start, pitch) in [(0, 60), (192_000, 67)] {
            let c = e.execute("midi.new_clip", &json!({"track": t.0, "start": start, "end": start + 192_000})).unwrap()["clip"].as_u64().unwrap();
            e.execute("midi.note_add", &json!({"clip": c, "pitch": pitch, "start_ticks": 0})).unwrap();
        }
        let mut lane = AutomationLane::new(AutoParam::Volume);
        lane.points.push(AutomationPoint::new(48_000, 0.5));
        e.session_mut().track_mut(t).unwrap().automation.push(lane);
        for d in ['A', 'B'] {
            if e.session().band.def(d).is_none() {
                e.execute("band.def", &json!({"id": d.to_string()})).unwrap();
            }
        }
        let a = e.execute("band.section.add", &json!({"section": "A", "start": 0, "end": 192_000})).unwrap()["block"].as_u64().unwrap();
        let b = e.execute("band.section.add", &json!({"section": "B", "start": 192_000, "end": 384_000})).unwrap()["block"].as_u64().unwrap();
        (e, t, a, b)
    }

    fn layout(e: &Engine, t: TrackId) -> Vec<(i64, u8)> {
        e.session()
            .track(t)
            .unwrap()
            .clips()
            .iter()
            .map(|c| (c.start, c.midi_playing().and_then(|q| q.notes.first()).map_or(0, |n| n.pitch)))
            .collect()
    }

    fn sections(e: &Engine) -> Vec<(i64, char)> {
        let mut v: Vec<(i64, char)> = e.session().band.sections.iter().map(|b| (b.start_tick, b.section)).collect();
        v.sort();
        v
    }

    /// Parity section 7: copying, moving and removing a section takes the song's clips,
    /// automation, chords and block with it, each one undo step.
    #[test]
    fn a_section_copies_moves_and_goes_with_the_song() {
        let (mut e, t, a, b) = song();
        // Copy A right after itself: A A B.
        e.execute("arrange.copy", &json!({"block": a})).unwrap();
        assert_eq!(layout(&e, t), vec![(0, 60), (192_000, 60), (384_000, 67)]);
        assert_eq!(sections(&e), vec![(0, 'A'), (7_680, 'A'), (15_360, 'B')]);
        let vol = &e.session().track(t).unwrap().automation[0].points;
        assert_eq!(vol.iter().map(|p| p.at).collect::<Vec<_>>(), vec![48_000, 240_000]);
        assert!(e.undo());
        assert_eq!(layout(&e, t), vec![(0, 60), (192_000, 67)]);
        // Move B before A: B A.
        e.execute("band.chord.add", &json!({"chord": "Am7", "at": 192_000, "end": 288_000})).unwrap();
        let chords_before = e.session().band.chords.len();
        assert_eq!(chords_before, 1);
        e.execute("arrange.move", &json!({"block": b, "to": 0})).unwrap();
        assert_eq!(layout(&e, t), vec![(0, 67), (192_000, 60)]);
        assert_eq!(sections(&e), vec![(0, 'B'), (7_680, 'A')]);
        assert_eq!(e.session().band.chords.len(), chords_before);
        assert!(e.session().band.chords.iter().all(|c| c.tick < 7_680), "B's chord moved with it");
        let vol = &e.session().track(t).unwrap().automation[0].points;
        assert_eq!(vol.iter().map(|p| p.at).collect::<Vec<_>>(), vec![240_000], "A's point went with A");
        assert!(e.undo());
        // Remove A: B alone, at the start.
        e.execute("arrange.remove", &json!({"block": a})).unwrap();
        assert_eq!(layout(&e, t), vec![(0, 67)]);
        assert_eq!(sections(&e), vec![(0, 'B')]);
        // Refusals: no block, a move onto itself, a copy where the tempo differs.
        assert!(e.execute("arrange.copy", &json!({"block": 999})).is_err());
        assert!(e.undo());
        assert!(e.execute("arrange.move", &json!({"block": a, "to": 96_000})).is_err());
        e.session_mut().tempo.set_tempo(15_360, 90.0).unwrap();
        assert!(e.execute("arrange.copy", &json!({"block": a, "to": 2_000_000})).is_err());
    }
}

//! A closed folder's blocks edited as clips: its lane draws the members' clips (nested folders
//! too) joined where they overlap; grabbing a block moves every member clip in it, and trimming a
//! block's edge trims the member clips on that edge. One undo step each.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{Clip, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "edit.move_block",
            "Move Folder Block",
            [],
            None,
            "{folder, start, end (the block), by: samples} — moves every member clip of the closed folder inside start..end (a block its lane draws) by `by`, as a Grabber drag of the block",
            always,
            move_block
        ),
        cmd!(
            "edit.trim_block",
            "Trim Folder Block",
            [],
            None,
            "{folder, start, end (the block), edge: start | end, to} — trims the closed folder's block: the member clips on that edge follow it out, and those it passes are trimmed in (never to nothing)",
            always,
            trim_block
        ),
    ]
}

/// The member clips of closed folder `folder` inside `block` (any that overlap it: a block is
/// the union of overlapping clips, so they lie wholly inside).
pub fn block_clips(s: &Session, folder: TrackId, block: Range) -> Vec<Clip> {
    super::track::folder_members(s, folder)
        .into_iter()
        .filter_map(|m| s.track(m))
        .flat_map(|t| t.clips().iter().filter(|c| c.range().overlaps(&block) && !c.range().is_empty()).cloned())
        .collect()
}

/// The folder and block `p` names, and the member clips in it (none: an error).
fn block_of(e: &Engine, p: &Value, id: &str) -> Result<(Range, Vec<Clip>)> {
    let s = e.session();
    let folder = p.get("folder").and_then(Value::as_u64).map(TrackId).ok_or_else(|| bad(id, "`folder` required"))?;
    if !s.track(folder).is_some_and(|t| t.is_folder()) {
        return Err(bad(id, "not a folder track"));
    }
    let start = p.get("start").and_then(Value::as_i64).ok_or_else(|| bad(id, "`start` required"))?;
    let end = p.get("end").and_then(Value::as_i64).ok_or_else(|| bad(id, "`end` required"))?;
    let block = Range::new(start.min(end), start.max(end));
    let clips = block_clips(s, folder, block);
    if clips.is_empty() {
        return Err(bad(id, "no clips in that block"));
    }
    Ok((block, clips))
}

fn move_block(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "edit.move_block";
    let (_, clips) = block_of(e, p, C)?;
    let by = p.get("by").and_then(Value::as_i64).ok_or_else(|| bad(C, "`by` required"))?;
    // Never before the session start.
    let first = clips.iter().map(|c| c.start).min().unwrap_or(0);
    let by = by.max(-first);
    let ids: Vec<ClipId> = clips.iter().map(|c| c.id).collect();
    let n = crate::edit::move_clips(e.session_mut(), &ids, by, &[]);
    Ok(json!({"moved": n, "by": by}))
}

fn trim_block(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "edit.trim_block";
    let (block, clips) = block_of(e, p, C)?;
    let to = p.get("to").and_then(Value::as_i64).ok_or_else(|| bad(C, "`to` required"))?;
    let start = match str_param(p, "edge") {
        Some("start") => true,
        Some("end") => false,
        _ => return Err(bad(C, "`edge` is start or end")),
    };
    let mut trimmed = 0;
    let s = e.session_mut();
    if start {
        // Inward no further than the first clip end: no clip is trimmed to nothing.
        let to = to.max(0).min(clips.iter().map(|c| c.end() - 1).min().unwrap_or(to));
        for c in &clips {
            let on_edge = c.start == block.start;
            if (to < c.start && on_edge) || to > c.start {
                trimmed += usize::from(crate::edit::trim_clip(s, c.id, Some(to), None, false).is_some());
            }
        }
    } else {
        let to = to.max(clips.iter().map(|c| c.start + 1).max().unwrap_or(to));
        for c in &clips {
            let on_edge = c.end() == block.end;
            if (to > c.end() && on_edge) || to < c.end() {
                trimmed += usize::from(crate::edit::trim_clip(s, c.id, None, Some(to), false).is_some());
            }
        }
    }
    Ok(json!({"trimmed": trimmed}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two MIDI tracks in a closed folder: clips 0..96 000 and 48 000..144 000 (one block), and
    /// 400 000..450 000 on the first.
    fn folder() -> (Engine, TrackId, Vec<TrackId>) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 2, "kind": "audio"})).unwrap();
        let members: Vec<TrackId> = e.session().tracks.iter().map(|t| t.id).collect();
        let s = e.session_mut();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 1_000_000]] };
        let src = crate::io::add_source(s, "long", buf, None, soundcraft_audio_io::FileFormat::Wav);
        for (t, (a, b)) in [(members[0], (0, 96_000)), (members[1], (48_000, 144_000)), (members[0], (400_000, 450_000))] {
            let id = s.new_clip_id();
            crate::edit::place_clip(s, t, Clip::audio(id, "c", src, 200_000, a, b - a));
        }
        e.execute("track.move_to_new_folder", &json!({"tracks": [members[0].0, members[1].0]})).unwrap();
        let f = e.session().tracks.iter().find(|t| t.is_folder()).unwrap().id;
        e.execute("track.folder_toggle", &json!({"track": f.0, "open": false})).unwrap();
        (e, f, members)
    }

    fn ranges(e: &Engine, t: TrackId) -> Vec<(Samples, Samples)> {
        e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.end())).collect()
    }

    #[test]
    fn grabbing_a_block_moves_its_clips_only() {
        let (mut e, f, m) = folder();
        e.execute("edit.move_block", &json!({"folder": f.0, "start": 0, "end": 144_000, "by": 10_000})).unwrap();
        assert_eq!(ranges(&e, m[0]), vec![(10_000, 106_000), (400_000, 450_000)], "the later block stays");
        assert_eq!(ranges(&e, m[1]), vec![(58_000, 154_000)]);
        assert!(e.undo());
        assert_eq!(ranges(&e, m[1]), vec![(48_000, 144_000)], "one undo step");
        // Never before zero.
        e.execute("edit.move_block", &json!({"folder": f.0, "start": 0, "end": 144_000, "by": -50_000})).unwrap();
        assert_eq!(ranges(&e, m[0])[0], (0, 96_000));
        assert!(e.execute("edit.move_block", &json!({"folder": f.0, "start": 200_000, "end": 300_000, "by": 5})).is_err(), "an empty block");
        assert!(e.execute("edit.move_block", &json!({"folder": m[0].0, "start": 0, "end": 1, "by": 5})).is_err(), "not a folder");
    }

    #[test]
    fn trimming_a_block_trims_the_clips_on_its_edge() {
        let (mut e, f, m) = folder();
        // The start out: only the clip on the edge follows (it has file before it).
        e.execute("edit.move_block", &json!({"folder": f.0, "start": 0, "end": 144_000, "by": 20_000})).unwrap();
        e.execute("edit.trim_block", &json!({"folder": f.0, "start": 20_000, "end": 164_000, "edge": "start", "to": 5_000})).unwrap();
        assert_eq!(ranges(&e, m[0])[0], (5_000, 116_000));
        assert_eq!(ranges(&e, m[1]), vec![(68_000, 164_000)]);
        // The start in past the second clip's start: both trimmed.
        e.execute("edit.trim_block", &json!({"folder": f.0, "start": 5_000, "end": 164_000, "edge": "start", "to": 80_000})).unwrap();
        assert_eq!(ranges(&e, m[0])[0], (80_000, 116_000));
        assert_eq!(ranges(&e, m[1]), vec![(80_000, 164_000)]);
        // The end in, never past the last clip start (nothing trimmed to nothing).
        e.execute("edit.trim_block", &json!({"folder": f.0, "start": 80_000, "end": 164_000, "edge": "end", "to": 10_000})).unwrap();
        assert_eq!(ranges(&e, m[0])[0], (80_000, 80_001));
        assert_eq!(ranges(&e, m[1]), vec![(80_000, 80_001)]);
        // The end out: only the clips on the edge follow.
        assert!(e.undo());
        e.execute("edit.trim_block", &json!({"folder": f.0, "start": 80_000, "end": 164_000, "edge": "end", "to": 170_000})).unwrap();
        assert_eq!(ranges(&e, m[0])[0], (80_000, 116_000));
        assert_eq!(ranges(&e, m[1]), vec![(80_000, 170_000)]);
        assert!(e.execute("edit.trim_block", &json!({"folder": f.0, "start": 80_000, "end": 170_000, "edge": "middle", "to": 1})).is_err());
    }
}

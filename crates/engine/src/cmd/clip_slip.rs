//! Nudge the audio inside a clip ("slip contents"): the clip stays where it is and plays earlier
//! or later material of its file, by the nudge value. A looped clip turns its loop instead; a
//! reversed clip moves the same way it sounds. Stops at the file's ends.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::ClipContent;
use soundcraft_time::Samples;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "clip.slip_earlier",
            "Nudge Contents Earlier",
            [],
            None,
            "{clips?, count?, by?: samples} — the selected audio clips stay put and their audio moves earlier inside them by the nudge value (or `by` timeline samples)",
            has_selection,
            |e, p| slip(e, p, -1)
        ),
        cmd!(
            "clip.slip_later",
            "Nudge Contents Later",
            [],
            None,
            "{clips?, count?, by?: samples} — the selected audio clips stay put and their audio moves later inside them by the nudge value (or `by` timeline samples)",
            has_selection,
            |e, p| slip(e, p, 1)
        ),
    ]
}

/// Moves the audio of each clip `dir` (−1 earlier, 1 later) by the nudge step. Returns how many
/// clips moved; one that is at its file's end, warped or not audio is left alone.
fn slip(e: &mut Engine, p: &Value, dir: i64) -> Result<Value> {
    let id = if dir < 0 { "clip.slip_earlier" } else { "clip.slip_later" };
    let ids = clip_ids_param(e, p);
    if ids.is_empty() {
        return Err(bad(id, "select an audio clip first"));
    }
    let s = e.session();
    let step = match p.get("by") {
        Some(v) => v.as_i64().filter(|n| *n > 0).ok_or_else(|| bad(id, "`by` is a positive number of samples"))?,
        None => {
            let count = i64_or(p, "count", 1).clamp(1, 1000);
            s.edit.nudge.step_samples(s.edit.selection.start, &s.grid_ctx()).saturating_mul(count)
        }
    };
    let mut moved = 0;
    for cid in ids {
        let s = e.session();
        let Some((_, c)) = s.find_clip(cid) else { continue };
        let frames = c.source().and_then(|src| s.source(src)).map_or(0, |src| Samples::try_from(src.frames).unwrap_or(Samples::MAX));
        if !c.is_audio() || !c.warp.is_empty() || frames <= 0 {
            continue;
        }
        let (loop_len, reversed) = (c.loop_len, c.reversed);
        let file_step = c.source_span(step).max(1);
        // Later on the timeline is earlier in the file, unless the clip plays backwards.
        let file_delta = if reversed { dir * file_step } else { -dir * file_step };
        let window = c.source_span(if loop_len > 0 { loop_len } else { c.length });
        let lo = 0;
        let hi = frames.saturating_sub(window).max(0);
        let Some(c) = e.session_mut().find_clip_mut(cid) else { continue };
        if loop_len > 0 {
            // A loop keeps its material and turns: the clip starts at another point of it.
            c.loop_phase = c.loop_phase.saturating_sub(dir * step).rem_euclid(loop_len);
            moved += 1;
            continue;
        }
        if let ClipContent::Audio { offset, .. } = &mut c.content {
            // Never past a file end (a clip already past one only moves back towards it).
            let to = offset.saturating_add(file_delta);
            let to = if file_delta > 0 { to.min(hi.max(*offset)) } else { to.max(lo.min(*offset)) };
            if to != *offset {
                *offset = to;
                moved += 1;
            }
        }
    }
    Ok(json!({"moved": moved, "by": step * dir}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{Clip, ClipId, Source, SourceId, TrackId};

    fn setup(start: i64, len: i64, offset: i64) -> (Engine, TrackId, ClipId) {
        let mut e = Engine::default();
        let t = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
        let s = e.session_mut();
        s.sources.push(Source {
            id: SourceId(9),
            name: "f".into(),
            path: "f.wav".into(),
            channels: 1,
            frames: 10_000,
            sample_rate: 48_000,
            format: soundcraft_audio_io::FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips = vec![Clip::audio(id, "c", SourceId(9), offset, start, len)];
        e.session_mut().edit.nudge = soundcraft_time::GridValue::Samples(100);
        e.execute("edit.select", &json!({"clips": [id.0]})).unwrap();
        (e, t, id)
    }

    fn state(e: &Engine, id: ClipId) -> (i64, i64, i64) {
        let c = e.session().find_clip(id).unwrap().1;
        (c.start, c.length, c.source_offset())
    }

    #[test]
    fn contents_move_inside_the_clip_and_stop_at_the_file_ends() {
        let (mut e, _, id) = setup(5000, 1000, 500);
        e.execute("clip.slip_later", &json!({})).unwrap();
        assert_eq!(state(&e, id), (5000, 1000, 400), "later on the timeline is earlier in the file");
        e.execute("clip.slip_earlier", &json!({"count": 3})).unwrap();
        assert_eq!(state(&e, id), (5000, 1000, 700));
        e.execute("clip.slip_later", &json!({"count": 50})).unwrap();
        assert_eq!(state(&e, id).2, 0, "stops at the file's start");
        e.execute("clip.slip_earlier", &json!({"by": 1_000_000})).unwrap();
        assert_eq!(state(&e, id).2, 9000, "stops where the clip reaches the file's end");
        // One undo step each.
        e.undo();
        assert_eq!(state(&e, id).2, 0);
    }

    #[test]
    fn a_reversed_clip_slips_the_way_it_sounds_and_a_warped_one_is_left_alone() {
        let (mut e, _, id) = setup(0, 1000, 500);
        e.session_mut().find_clip_mut(id).unwrap().reversed = true;
        e.execute("clip.slip_later", &json!({})).unwrap();
        assert_eq!(state(&e, id).2, 600);
        e.session_mut().find_clip_mut(id).unwrap().warp = vec![(0, 0), (500, 400)];
        assert_eq!(e.execute("clip.slip_later", &json!({})).unwrap()["moved"], 0);
    }
}

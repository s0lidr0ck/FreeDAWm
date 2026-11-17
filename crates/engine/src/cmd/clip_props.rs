//! Real-time clip properties of audio clips: reverse, transpose and loop, heard as the clip plays
//! with no new file ([`soundcraft_model::Clip::reversed`], `transpose_cents`, `loop_len`).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{ClipId, MAX_TRANSPOSE_CENTS};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "clip.play_reversed",
            "Reverse in Place",
            [],
            None,
            "{clips?, value?: bool (toggles when omitted)} — audio clips play backwards in real time, no new file (Process › Reverse writes one)",
            has_selection,
            play_reversed
        ),
        cmd!(
            "clip.transpose",
            "Transpose Clip...",
            [],
            None,
            "{clips?, semitones?: -24..24 (fractions are cents), cents?: -100..100} — audio clips play transposed in real time, keeping their length; 0 takes it off",
            has_selection,
            transpose
        ),
        cmd!(
            "clip.loop_in_place",
            "Loop in Place...",
            [],
            None,
            "{clips?, count?: n, end?: position, off?: bool} — audio clips repeat their material in real time (no copies): `count` times, or (one clip) to `end`, as the loop trim does; the first loop's length is what repeats. `off` ends the loop at its first pass",
            has_selection,
            loop_in_place
        ),
    ]
}

fn audio_ids(e: &Engine, p: &Value) -> Vec<ClipId> {
    clip_ids_param(e, p).into_iter().filter(|id| e.session().find_clip(*id).is_some_and(|(_, c)| c.is_audio())).collect()
}

fn play_reversed(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = audio_ids(e, p);
    let s = e.session_mut();
    let v = p.get("value").and_then(Value::as_bool).unwrap_or_else(|| !ids.iter().all(|id| s.find_clip(*id).is_some_and(|(_, c)| c.reversed)));
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            c.reversed = v;
            // The fades stay at the clip's edges; the sync point marks the same audio.
            if c.sync_point > 0 {
                c.sync_point = (c.length - 1 - c.sync_point).max(0);
            }
        }
    }
    Ok(json!({"value": v, "clips": ids.len()}))
}

fn transpose(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.transpose";
    let ids = audio_ids(e, p);
    let st = p.get("semitones").and_then(Value::as_f64).unwrap_or(0.0);
    let ct = p.get("cents").and_then(Value::as_f64).unwrap_or(0.0);
    if !st.is_finite() || !ct.is_finite() || st.abs() > 24.0 || ct.abs() > 100.0 {
        return Err(bad(C, "semitones from -24 to 24, cents from -100 to 100"));
    }
    let cents = ((st * 100.0 + ct).round() as i32).clamp(-MAX_TRANSPOSE_CENTS, MAX_TRANSPOSE_CENTS);
    let s = e.session_mut();
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            c.transpose_cents = cents;
        }
    }
    Ok(json!({"cents": cents, "clips": ids.len()}))
}

fn loop_in_place(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.loop_in_place";
    let ids = audio_ids(e, p);
    let end = position_param(e, C, p, "end")?;
    let count = p.get("count").and_then(Value::as_i64);
    let off = p.get("off").and_then(Value::as_bool).unwrap_or(false);
    if end.is_some() && ids.len() != 1 {
        return Err(bad(C, "`end` loops one clip"));
    }
    if !off && end.is_none() && count.is_none() {
        return Err(bad(C, "give a `count`, an `end` or `off`"));
    }
    let max = super::more_util::max_render(e.session());
    let mut n = 0;
    for id in ids {
        let Some((t, c)) = e.session().find_clip(id).map(|(t, c)| (t, c.clone())) else { continue };
        let mut nc = c.clone();
        if off {
            if c.loop_len <= 0 {
                continue;
            }
            // The first pass from where the clip starts in the loop.
            let rest = c.loop_len - c.loop_phase;
            if let soundcraft_model::ClipContent::Audio { offset, .. } = &mut nc.content
                && !c.reversed
            {
                *offset = offset.saturating_add(c.source_span(c.loop_phase));
            }
            nc.length = rest.min(c.length).max(1);
            (nc.loop_len, nc.loop_phase) = (0, 0);
        } else {
            if nc.loop_len <= 0 {
                nc.loop_len = c.length.max(1);
                nc.loop_phase = 0;
            }
            let len = match (end, count) {
                (Some(end), _) => end - c.start,
                (None, Some(k)) => nc.loop_len.saturating_mul(k.clamp(1, 1000)),
                (None, None) => c.length,
            };
            if len <= 0 || len > max {
                return Err(bad(C, "the looped clip must end after its start and stay under the longest render"));
            }
            nc.length = len;
        }
        nc.clamp_fades();
        super::more_util::replace_clip(e.session_mut(), t, id, nc);
        n += 1;
    }
    Ok(json!({"clips": n}))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, TrackId, TrackKind};
    use soundcraft_time::Range;

    /// A mono clip of a rising ramp, 0.1 s, at 0.
    fn ramp() -> (crate::Engine, TrackId, ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("A"));
        let v: Vec<f32> = (0..9600).map(|i| i as f32 / 9600.0).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![v] };
        let src = crate::io::add_source(&mut s, "Ramp", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let cid = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(cid, "Ramp", src, 0, 0, 4800));
        // No automatic edge fades: the samples are compared exactly.
        s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
        (crate::Engine::new(s), t, cid)
    }

    fn play(e: &crate::Engine, t: TrackId, a: i64, b: i64) -> Vec<f32> {
        soundcraft_mix::render_clips(e.session(), t, Range { start: a, end: b }).remove(0)
    }

    #[test]
    fn reverse_in_place_plays_the_window_backwards_and_back() {
        let (mut e, t, cid) = ramp();
        let fwd = play(&e, t, 0, 4800);
        e.execute("clip.play_reversed", &json!({"clips": [cid.0]})).unwrap();
        let rev = play(&e, t, 0, 4800);
        for k in [0usize, 100, 2399, 4799] {
            assert_eq!(rev[k], fwd[4799 - k], "{k}");
        }
        // Trimming the reversed clip's start drops the window's end.
        let mut s = e.session().clone();
        s.find_clip_mut(cid).unwrap().trim_start_to(1000);
        assert_eq!(soundcraft_mix::render_clips(&s, t, Range { start: 1000, end: 1001 })[0][0], fwd[4799 - 1000]);
        e.execute("clip.play_reversed", &json!({"clips": [cid.0]})).unwrap();
        assert_eq!(play(&e, t, 0, 4800), fwd, "off again, bit for bit");
    }

    #[test]
    fn loop_in_place_repeats_without_copies_and_a_trim_keeps_the_beat() {
        let (mut e, t, cid) = ramp();
        let one = play(&e, t, 0, 4800);
        e.execute("clip.loop_in_place", &json!({"clips": [cid.0], "count": 3})).unwrap();
        let c = e.session().find_clip(cid).unwrap().1.clone();
        assert_eq!((c.length, c.loop_len), (14_400, 4800));
        assert_eq!(e.session().track(t).unwrap().clips().len(), 1, "no copies");
        let three = play(&e, t, 0, 14_400);
        assert_eq!(&three[4800..9600], &one[..]);
        assert_eq!(&three[9600..], &one[..]);
        // The loop trim: to an end; a start trim keeps each sample where it was.
        e.execute("clip.loop_in_place", &json!({"clip": cid.0, "end": 12_000})).unwrap();
        let mut s = e.session().clone();
        s.find_clip_mut(cid).unwrap().trim_start_to(6000);
        let after = soundcraft_mix::render_clips(&s, t, Range { start: 6000, end: 12_000 }).remove(0);
        assert_eq!(&after[..], &three[6000..12_000]);
        e.execute("clip.loop_in_place", &json!({"clips": [cid.0], "off": true})).unwrap();
        assert_eq!(e.session().find_clip(cid).unwrap().1.length, 4800);
        assert!(e.execute("clip.loop_in_place", &json!({"clips": [cid.0]})).is_err());
    }

    #[test]
    fn transpose_keeps_the_length_and_moves_the_pitch() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("A"));
        let f0 = 440.0f32;
        let v: Vec<f32> = (0..96_000).map(|i| (std::f32::consts::TAU * f0 * i as f32 / 48_000.0).sin() * 0.5).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![v] };
        let src = crate::io::add_source(&mut s, "Tone", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let cid = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(cid, "Tone", src, 0, 0, 96_000));
        let mut e = crate::Engine::new(s);
        e.execute("clip.transpose", &json!({"clips": [cid.0], "semitones": 7})).unwrap();
        assert_eq!(e.session().find_clip(cid).unwrap().1.length, 96_000);
        let y = play(&e, t, 10_000, 26_384);
        // Zero crossings rising: the frequency.
        let ups = y.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32;
        let hz = ups * 48_000.0 / y.len() as f32;
        let want = f0 * 2f32.powf(7.0 / 12.0);
        assert!((hz - want).abs() < want * 0.03, "{hz} Hz for {want}");
        assert!(e.execute("clip.transpose", &json!({"clips": [cid.0], "semitones": 30})).is_err());
    }
}

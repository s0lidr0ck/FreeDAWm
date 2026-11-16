//! A stereo track's width and balance follow their automation lanes, on the pan's grid.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::render_range;
use soundcraft_model::{AutoParam, ChannelFormat, Clip, Session, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

#[test]
fn width_and_balance_lanes_play() {
    let mut s = Session::default();
    s.edit.click = false;
    s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
    let n = 8192;
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.4; n], vec![0.1; n]] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "lr", SourceId(1), 0, 0, n as i64));
    let plain = render_range(&s, Range::new(0, n as i64), 256);
    {
        let tr = s.track_mut(t).unwrap();
        let w = tr.lane_mut(&AutoParam::Width);
        w.set_point(0, 1.0);
        w.set_point(3999, 1.0);
        w.set_point(4000, 0.0);
    }
    let out = render_range(&s, Range::new(0, n as i64), 256);
    assert!((out[0][1000] - plain[0][1000]).abs() < 1e-6, "full width before the step");
    assert!((out[0][6000] - out[1][6000]).abs() < 1e-6, "mono after it: {} {}", out[0][6000], out[1][6000]);
    // The balance lane turned hard left leaves the right side silent.
    {
        let tr = s.track_mut(t).unwrap();
        tr.automation.clear();
        let b = tr.lane_mut(&AutoParam::Balance);
        b.set_point(0, -1.0);
    }
    let left = render_range(&s, Range::new(0, n as i64), 256);
    assert!(left[1][1000].abs() < 1e-6 && left[0][1000] > 0.1, "{} {}", left[0][1000], left[1][1000]);
    // The lanes' ids read back.
    assert_eq!(AutoParam::parse("width"), Some(AutoParam::Width));
    assert_eq!(AutoParam::parse(&AutoParam::Balance.id()), Some(AutoParam::Balance));
}

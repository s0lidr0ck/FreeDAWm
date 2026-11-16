//! Track › Commit keeps what the render leaves to the fader section (mix bug 8).

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_model::{AutoParam, ChannelFormat, Clip, Session, SurroundPan, TrackId, TrackKind};

fn track_with_clip(fmt: ChannelFormat) -> (Engine, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, fmt, Some("Src"));
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4800]; fmt.channels()] };
    let src = soundcraft_engine::io::add_source(&mut s, "src", buf, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Src", src, 0, 0, 4800));
    (Engine::new(s), t)
}

fn commit(e: &mut Engine, t: TrackId) -> TrackId {
    let r = e.execute("track.commit", &json!({"tracks": [t.0]})).unwrap();
    TrackId(r["tracks"][0].as_u64().unwrap())
}

#[test]
fn a_surround_track_commits_to_a_surround_track_with_its_panner() {
    let (mut e, t) = track_with_clip(ChannelFormat::Surround51);
    let pan = SurroundPan { x: 0.5, y: -0.5, ..SurroundPan::default() };
    e.session_mut().track_mut(t).unwrap().mixer.surround = Some(pan);
    let n = commit(&mut e, t);
    let nt = e.session().track(n).unwrap();
    assert_eq!(nt.format, ChannelFormat::Surround51);
    assert_eq!(nt.mixer.surround, Some(pan));
    assert_eq!(nt.clips().len(), 1);
}

#[test]
fn commit_keeps_pan_mute_and_fader_automation_but_not_what_it_baked_in() {
    let (mut e, t) = track_with_clip(ChannelFormat::Stereo);
    {
        let tr = e.session_mut().track_mut(t).unwrap();
        tr.mixer.pan = vec![-0.5, 0.25];
        tr.mixer.mute = true;
        tr.lane_mut(&AutoParam::Volume).set_point(100, -6.0);
        tr.lane_mut(&AutoParam::Pan(0)).set_point(200, -1.0);
        tr.lane_mut(&AutoParam::Trim).set_point(0, -3.0);
    }
    let n = commit(&mut e, t);
    let nt = e.session().track(n).unwrap();
    assert_eq!(nt.mixer.pan, vec![-0.5, 0.25]);
    assert!(nt.mixer.mute);
    assert!(nt.lane(&AutoParam::Volume).is_some_and(|l| l.points.len() == 1));
    assert!(nt.lane(&AutoParam::Pan(0)).is_some_and(|l| l.points.len() == 1));
    assert!(nt.lane(&AutoParam::Trim).is_none_or(|l| l.points.is_empty()), "the trim is in the audio");
}

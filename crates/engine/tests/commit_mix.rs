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

#[test]
fn commit_leaves_a_post_fader_module_live_on_the_new_track() {
    let (mut e, t) = track_with_clip(ChannelFormat::Stereo);
    let r = e.execute("mix.insert", &json!({"track": t.0, "plugin": "gain", "params": {"gain": 6.0}})).unwrap();
    let m = r["module"].as_u64().unwrap();
    assert_eq!(e.execute("mix.module_post_fader", &json!({"track": t.0, "module": m})).unwrap()["post_fader"], true);
    e.session_mut()
        .track_mut(t)
        .unwrap()
        .lane_mut(&AutoParam::Plugin { module: soundcraft_model::ModuleId(m as u32), param: "gain".into() })
        .set_point(0, 3.0);
    let n = commit(&mut e, t);
    let nt = e.session().track(n).unwrap();
    assert_eq!(nt.mixer.inserts.len(), 1, "the post-fader module moved on");
    assert!(nt.mixer.inserts[0].post_fader && nt.mixer.inserts[0].plugin == "gain");
    assert!(nt.automation.iter().any(|l| matches!(&l.param, AutoParam::Plugin { .. })), "with its lane");
    // The commit's audio is without it (0.25 stays 0.25, not +6 dB).
    let src = nt.clips()[0].source().unwrap();
    let peak = e.session().pool.get(src).unwrap().channel(0).unwrap().iter().fold(0.0f32, |p, x| p.max(x.abs()));
    assert!((peak - 0.25).abs() < 1e-3, "{peak}");
}

//! The control room (`soundcraft_model::ControlRoom`): the AFL/PFL listen bus, the monitor
//! path's dim and mono, and talkback into a cue; none of it reaches a bounce or the main mix.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{MixEngine, render_range};
use soundcraft_model::{ChannelFormat, Clip, ControlRoom, CueMix, OutputPath, Session, SourceAudio, SourceId, Talkback, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const BLOCK: usize = 512;

/// Two mono tracks of DC (0.5 and 0.25) and a stereo one (0.4 left, 0.1 right), playing a second.
fn session() -> (Session, [TrackId; 3]) {
    let mut s = Session::default();
    s.edit.click = false;
    s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
    let n = 48_000;
    let mut ids = [TrackId(0); 3];
    for (k, chans) in [vec![0.5f32], vec![0.25], vec![0.4, 0.1]].into_iter().enumerate() {
        let src = SourceId(700 + k as u64);
        let fmt = if chans.len() == 2 { ChannelFormat::Stereo } else { ChannelFormat::Mono };
        s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: chans.iter().map(|v| vec![*v; n]).collect() })));
        let t = s.add_track(TrackKind::Audio, fmt, None);
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "dc", src, 0, 0, n as i64));
        ids[k] = t;
    }
    (s, ids)
}

/// The live output (what the monitor speakers get) of the fourth block.
fn live(s: &Session) -> Vec<Vec<f32>> {
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in 0..4 {
        eng.render(s, b * BLOCK as i64, BLOCK, &mut out);
    }
    out
}

fn close(a: &[Vec<f32>], b: &[Vec<f32>]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.iter().zip(y).all(|(p, q)| (p - q).abs() < 1e-6))
}

fn bounce(s: &Session) -> Vec<Vec<f32>> {
    render_range(s, Range::new(0, 4 * BLOCK as i64), BLOCK)
}

#[test]
fn the_listen_bus_carries_only_the_soloed_track_and_leaves_the_mix_alone() {
    let (mut s, [a, b, c]) = session();
    let full = bounce(&s);
    // What A alone sounds like on the main mix.
    let mut only_a = s.clone();
    for t in [b, c] {
        only_a.track_mut(t).unwrap().mixer.mute = true;
    }
    let a_alone = live(&only_a);
    s.track_mut(a).unwrap().mixer.solo = true;
    for mode in ["afl", "pfl"] {
        s.edit.solo_mode = mode.into();
        assert!(close(&live(&s), &a_alone), "{mode}: the monitor hears A only");
        assert_eq!(bounce(&s), full, "{mode}: a bounce is the whole mix, the solo is only listened to");
    }
    // In place (today's solo) the bounce follows the solo.
    s.edit.solo_mode = "sip".into();
    assert_ne!(bounce(&s), full);
}

#[test]
fn afl_follows_the_fader_and_pfl_does_not() {
    let (mut s, [a, ..]) = session();
    s.track_mut(a).unwrap().mixer.solo = true;
    s.edit.solo_mode = "afl".into();
    let unity = live(&s);
    s.track_mut(a).unwrap().mixer.volume_db = -6.0;
    let afl = live(&s);
    let g = 10f32.powf(-6.0 / 20.0);
    assert!((afl[0][100] - unity[0][100] * g).abs() < 1e-5, "after the fader: {} vs {}", afl[0][100], unity[0][100] * g);
    s.edit.solo_mode = "pfl".into();
    assert!(close(&live(&s), &unity), "before the fader");
}

#[test]
fn dim_and_mono_act_on_the_monitor_only() {
    let (mut s, [a, b, _]) = session();
    for t in [a, b] {
        s.track_mut(t).unwrap().mixer.mute = true;
    }
    let plain = live(&s);
    let full = bounce(&s);
    let mut cr = ControlRoom { dim: true, ..ControlRoom::default() };
    s.control_room = Some(cr.clone());
    let dimmed = live(&s);
    let g = 10f32.powf(-20.0 / 20.0);
    assert!((dimmed[0][100] - plain[0][100] * g).abs() < 1e-5 && (dimmed[1][100] - plain[1][100] * g).abs() < 1e-5);
    cr.dim = false;
    cr.mono = true;
    s.control_room = Some(cr.clone());
    let mono = live(&s);
    let mid = 0.5 * (plain[0][100] + plain[1][100]);
    assert!((mono[0][100] - mid).abs() < 1e-5 && (mono[1][100] - mid).abs() < 1e-5, "{:?}", (mono[0][100], mono[1][100], mid));
    cr.mute = true;
    s.control_room = Some(cr);
    assert!(live(&s).iter().flatten().all(|x| *x == 0.0), "muted");
    assert_eq!(bounce(&s), full, "a bounce is the same with the control room on");
}

#[test]
fn talkback_reaches_a_cue_and_never_the_main_mix() {
    let mut s = Session::default();
    s.edit.click = false;
    s.outputs.push(OutputPath { name: "Phones".into(), first_channel: 2, format: ChannelFormat::Stereo });
    let cr = ControlRoom {
        cues: vec![CueMix { name: "Cue 1".into(), output: "Phones".into(), level_db: 0.0, mute: false, talkback: true }],
        talkback: Talkback { input: "In 1".into(), on: true, ..Talkback::default() },
        ..ControlRoom::default()
    };
    s.control_room = Some(cr);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.input = vec![vec![0.5; BLOCK]];
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    eng.render(&s, 0, BLOCK, &mut out);
    eng.input = vec![vec![0.5; BLOCK]];
    eng.render(&s, BLOCK as i64, BLOCK, &mut out);
    let phones = &eng.hardware_out()[1];
    assert!(phones.iter().all(|c| (c[BLOCK - 1] - 0.5).abs() < 1e-6), "both sides of the cue hear the microphone");
    assert!(out.iter().flatten().all(|x| *x == 0.0), "the main mix never does");
    // Let go: the cue falls silent (after a short ramp).
    s.control_room.as_mut().unwrap().talkback.on = false;
    eng.input = vec![vec![0.5; BLOCK]];
    eng.render(&s, 2 * BLOCK as i64, BLOCK, &mut out);
    assert!(eng.hardware_out()[1].iter().all(|c| c[BLOCK - 1] == 0.0));
}

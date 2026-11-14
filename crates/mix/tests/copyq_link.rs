//! FireStation CopyQ in the mix engine: a Match takes the sound of its group's Send across
//! tracks, whatever the track order; Group members meet in the middle; a module's stored target
//! reaches the plugin prepared; taking the Send away leaves the Match on its own target.
//!
//! Each track's signal is the built-in generator's pink noise, through an EQ where a darker
//! sound is wanted, into the CopyQ under test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_dsp::plugins::fs::copyq::profile::{grid_pos, prepare_state};
use soundcraft_dsp::plugins::fs::copyq::{CopyQState, LIVE_GROUP, LIVE_LINKED, LIVE_MATCHING, LIVE_SENDERS, Profile, live_shown};
use soundcraft_mix::MixEngine;
use soundcraft_model::{ChannelFormat, Insert, ModuleId, PreparedState, Route, Session, TrackId, TrackKind};

const BLOCK: usize = 512;
const SR: u32 = 48_000;

/// A track playing pink noise, darkened by `shelf_db` above 1 kHz, into a CopyQ in `role`
/// (0 Off, 1 Send, 2 Match, 3 Group) in `group`.
fn track(s: &mut Session, name: &str, shelf_db: f32, role: f32, group: Option<u32>) -> (TrackId, ModuleId) {
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(name));
    s.track_mut(t).unwrap().mixer.output = Route::Main;
    let mut noise = Insert::new("signal_generator");
    noise.params.insert("waveform".into(), 5.0);
    noise.params.insert("level".into(), -20.0);
    s.track_mut(t).unwrap().mixer.add_module(noise, None).unwrap();
    let mut eq = Insert::new("eq_7band");
    eq.params.insert("high_shelf_freq".into(), 1_000.0);
    eq.params.insert("high_shelf_gain".into(), shelf_db);
    s.track_mut(t).unwrap().mixer.add_module(eq, None).unwrap();
    let mut q = Insert::new("fs_copyq");
    q.params.insert("link".into(), role);
    q.params.insert("limit".into(), 24.0);
    q.link_group = group;
    let m = s.track_mut(t).unwrap().mixer.add_module(q, None).unwrap();
    (t, m)
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>) {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
        assert!(out.iter().flatten().all(|x| x.is_finite()));
    }
}

fn live(eng: &MixEngine, t: TrackId, m: ModuleId, i: usize) -> f32 {
    eng.meters[&t].analysis.iter().find(|(id, _)| *id == m).unwrap().1.live.get(i)
}

/// How much more the response does at 8 kHz than at 200 Hz, dB.
fn tilt(eng: &MixEngine, t: TrackId, m: ModuleId) -> f32 {
    let at = |hz: f32| live(eng, t, m, live_shown(0, 2) + grid_pos(hz).round() as usize);
    at(8_000.0) - at(200.0)
}

const SECONDS: usize = 8;

#[test]
fn a_match_takes_its_sends_sound_across_tracks() {
    let mut s = Session::default();
    // The Match's track comes first: the host still renders the Send first.
    let (b, matcher) = track(&mut s, "Match", 0.0, 2.0, Some(4));
    let (a, send) = track(&mut s, "Send", -12.0, 1.0, Some(4));
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    let blocks = SECONDS * SR as usize / BLOCK;
    render(&mut eng, &s, 0..blocks);
    let slab = eng.link_slab(4).unwrap();
    assert_eq!((slab.producers(), slab.consumers()), (1, 1));
    assert_eq!(live(&eng, a, send, LIVE_MATCHING), 0.0, "the Send passes its audio");
    assert_eq!((live(&eng, b, matcher, LIVE_SENDERS), live(&eng, b, matcher, LIVE_LINKED), live(&eng, b, matcher, LIVE_MATCHING)), (1.0, 1.0, 1.0));
    let got = tilt(&eng, b, matcher);
    assert!((got + 12.0).abs() < 2.5, "the plain one is darkened as the Send's shelf is: {got}");

    // The Send is deleted: the Match has no group target and, with no profile of its own, rests.
    s.track_mut(a).unwrap().mixer.remove_module(send);
    render(&mut eng, &s, blocks..blocks + 2 * SR as usize / BLOCK);
    assert_eq!(live(&eng, b, matcher, LIVE_LINKED), 0.0);
    assert!(tilt(&eng, b, matcher).abs() < 0.5, "{}", tilt(&eng, b, matcher));
}

#[test]
fn a_group_meets_in_the_middle() {
    let mut s = Session::default();
    let (a, dull) = track(&mut s, "Dull", -12.0, 3.0, Some(1));
    let (b, plain) = track(&mut s, "Plain", 0.0, 3.0, Some(1));
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    render(&mut eng, &s, 0..SECONDS * SR as usize / BLOCK);
    assert_eq!(eng.link_slab(1).unwrap().peers(), 2);
    assert_eq!(live(&eng, a, dull, LIVE_GROUP), 2.0);
    let (up, down) = (tilt(&eng, a, dull), tilt(&eng, b, plain));
    assert!((up - 6.0).abs() < 2.0 && (down + 6.0).abs() < 2.0, "each comes half way: {up} and {down}");
}

#[test]
fn a_stored_target_reaches_the_plugin_prepared() {
    let mut s = Session::default();
    let (t, m) = track(&mut s, "Solo", 0.0, 0.0, None);
    let state = CopyQState { targets: vec![(1.0, Profile::tilt("dark", -2.0))], source: None }.write();
    {
        let ins = s.track_mut(t).unwrap().mixer.module_mut(m).unwrap();
        ins.set_state_bytes(&state);
        // What the engine's commands do off the audio thread.
        ins.prepared = Some(PreparedState::new(prepare_state(&state)));
    }
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    let blocks = SECONDS * SR as usize / BLOCK;
    render(&mut eng, &s, 0..blocks);
    assert_eq!(live(&eng, t, m, LIVE_MATCHING), 1.0);
    // 200 Hz to 8 kHz is 5.3 octaves at -2 dB each.
    let got = tilt(&eng, t, m);
    assert!((got + 10.6).abs() < 1.5, "{got}");
    // The target taken away: back to rest.
    s.track_mut(t).unwrap().mixer.module_mut(m).unwrap().prepared = None;
    render(&mut eng, &s, blocks..blocks + 2 * SR as usize / BLOCK);
    assert_eq!(live(&eng, t, m, LIVE_MATCHING), 0.0);
    assert!(tilt(&eng, t, m).abs() < 0.5);
}

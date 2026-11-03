//! FireStation LevelUp in the mix engine: an OUT follows the IN of its pair across tracks through
//! the link group (B4), whatever the track order; reference audio reaches it as a session asset;
//! deleting the IN leaves the OUT on its own Target knob; nothing crashes.
//!
//! Each track's signal is a LevelUp playing a reference in place of its (silent) input at unity
//! (nothing measured yet, so its reference gain is 0 dB), feeding the LevelUp under test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_dsp::SampleSource;
use soundcraft_dsp::plugins::fs::levelup::ReferenceAudio;
use soundcraft_mix::MixEngine;
use soundcraft_model::{AssetData, ChannelFormat, Insert, ModuleId, Route, Session, TrackId, TrackKind};
use std::sync::Arc;

const BLOCK: usize = 512;
const SR: u32 = 48_000;

fn track(s: &mut Session, name: &str) -> TrackId {
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(name));
    s.track_mut(t).unwrap().mixer.output = Route::Main;
    t
}

/// A reference tone at `db` dBFS, kept in the session under `name`.
fn tone_asset(s: &mut Session, name: &str, db: f32) {
    let a = 10f32.powf(db / 20.0);
    let x: Vec<f32> = (0..SR as usize * 4).map(|i| a * (std::f32::consts::TAU * 997.0 * i as f32 / SR as f32).sin()).collect();
    let r: Arc<dyn SampleSource> = Arc::new(ReferenceAudio::new(SR as f32, vec![x.clone(), x]).unwrap());
    s.assets.insert_for("ref0", name, SR, AssetData(Arc::new(r)));
}

/// A track whose signal is the reference `name`, then a LevelUp in `role` (1 IN, 2 OUT) in `group`.
fn leveled(s: &mut Session, track_name: &str, name: &str, role: f32, group: Option<u32>) -> (TrackId, ModuleId) {
    let t = track(s, track_name);
    let mut src = Insert::new("fs_levelup");
    src.params.insert("mode".into(), 1.0);
    src.params.insert("ceiling".into(), 0.0);
    src.params.insert("reference".into(), 1.0);
    src.assets.insert("ref0".into(), name.into());
    s.track_mut(t).unwrap().mixer.add_module(src, None).unwrap();
    let mut lu = Insert::new("fs_levelup");
    lu.params.insert("link".into(), role);
    lu.params.insert("release".into(), 0.0);
    lu.link_group = group;
    let m = s.track_mut(t).unwrap().mixer.add_module(lu, None).unwrap();
    (t, m)
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>) {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
        assert!(out.iter().flatten().all(|x| x.is_finite()));
    }
}

fn readouts(eng: &MixEngine, t: TrackId, m: ModuleId) -> [f32; 8] {
    eng.meters[&t].modules.iter().find(|x| x.module == m).unwrap().readouts
}

#[test]
fn an_out_follows_its_in_across_tracks() {
    let mut s = Session::default();
    tone_asset(&mut s, "loud", -20.0);
    tone_asset(&mut s, "quiet", -30.0);
    // the OUT's track comes first: the host still renders the IN first
    let (b, out) = leveled(&mut s, "Out", "quiet", 2.0, Some(3));
    let (a, inp) = leveled(&mut s, "In", "loud", 1.0, Some(3));
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    let blocks = 10 * SR as usize / BLOCK;
    render(&mut eng, &s, 0..blocks);
    let ri = readouts(&eng, a, inp);
    let ro = readouts(&eng, b, out);
    assert!((ri[0] + 20.0).abs() < 0.3, "the IN hears -20 LUFS: {ri:?}");
    assert!((ro[7] - ri[0]).abs() < 0.2, "the OUT follows it: {ro:?}");
    assert!((ro[2] + 20.0).abs() < 0.5, "and rides its -30 to -20, not the knob's -18: {ro:?}");
    let slab = eng.link_slab(3).unwrap();
    assert_eq!((slab.producers(), slab.consumers()), (1, 1));

    // another pair: back on the knob
    s.track_mut(b).unwrap().mixer.module_mut(out).unwrap().params.insert("linkGroup".into(), 2.0);
    render(&mut eng, &s, blocks..blocks + 4);
    assert!(readouts(&eng, b, out)[7].is_nan());
    s.track_mut(b).unwrap().mixer.module_mut(out).unwrap().params.insert("linkGroup".into(), 0.0);
    render(&mut eng, &s, blocks + 4..blocks + 8);
    assert!(readouts(&eng, b, out)[7].is_finite());

    // the IN is deleted: the OUT finds no IN, nothing crashes
    s.track_mut(a).unwrap().mixer.remove_module(inp);
    render(&mut eng, &s, blocks + 8..blocks + 12);
    let _ = eng.take_retired();
    render(&mut eng, &s, blocks + 12..blocks + 16);
    assert!(readouts(&eng, b, out)[7].is_nan(), "{:?}", readouts(&eng, b, out));
}

#[test]
fn an_out_alone_or_with_a_missing_reference_keeps_working() {
    let mut s = Session::default();
    // a reference the session does not have (a missing file): the module plays its input
    let (t, m) = leveled(&mut s, "Out", "missing", 2.0, Some(1));
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    render(&mut eng, &s, 0..200);
    let r = readouts(&eng, t, m);
    assert!(r[7].is_nan() && r[0] <= -69.0, "{r:?}");
}

#[test]
fn the_reset_delivery_action_reaches_the_live_instance() {
    let mut s = Session::default();
    tone_asset(&mut s, "loud", -20.0);
    let (t, m) = leveled(&mut s, "In", "loud", 0.0, None);
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    render(&mut eng, &s, 0..200);
    assert!(readouts(&eng, t, m)[3] > -69.0);
    assert!(!eng.module_action(t, m, "nope"));
    assert!(!eng.module_action(t, ModuleId(999), "reset_delivery"));
    assert!(eng.module_action(t, m, "reset_delivery"));
    render(&mut eng, &s, 200..201);
    assert_eq!(readouts(&eng, t, m)[3], -100.0);
}

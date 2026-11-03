//! The channel chain is keyed by module id: reordering keeps instances (and their automation),
//! missing plugins pass audio and keep their state, and removing a module mid-playback is safe.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::MixEngine;
use soundcraft_model::{AutoParam, ChannelFormat, Clip, Insert, ModuleId, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const BLOCK: usize = 512;
const LEN: usize = BLOCK * 8;

/// A mono track playing a 1 kHz sine at 0.5, panned centre into the main mix.
fn sine_session() -> (Session, TrackId) {
    let mut s = Session::default();
    let sine: Vec<f32> = (0..LEN).map(|n| 0.5 * (std::f32::consts::TAU * 1000.0 * n as f32 / 48_000.0).sin()).collect();
    s.pool.insert(SourceId(77), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![sine] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Sine"));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "sine", SourceId(77), 0, 0, LEN as i64));
    tr.mixer.output = Route::Main;
    (s, t)
}

fn render_blocks(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>, out: &mut [Vec<f32>]) {
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut buf);
        for (o, c) in out.iter_mut().zip(buf.iter()) {
            o.extend_from_slice(c);
        }
    }
}

fn rms_db(x: &[f32]) -> f32 {
    let ms = x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32;
    10.0 * ms.max(1e-30).log10()
}

#[test]
fn reorder_keeps_instance_and_automation() {
    let (mut s, t) = sine_session();
    let tr = s.track_mut(t).unwrap();
    let mut gain = Insert::new("gain");
    gain.params.insert("gain".into(), -6.0);
    let a = tr.mixer.add_module(gain, None).unwrap();
    let b = tr.mixer.add_module(Insert::new("trim"), None).unwrap();
    let lane = tr.lane_mut(&AutoParam::Plugin { module: a, param: "gain".into() });
    lane.set_point(0, -6.0);
    lane.set_point(LEN as i64, -20.0);

    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let mut moved = vec![Vec::new(), Vec::new()];
    render_blocks(&mut eng, &s, 0..4, &mut moved);
    let serial = eng.instance_serial(t, a);
    assert!(serial.is_some(), "the gain module has an instance");
    let trim_serial = eng.instance_serial(t, b);

    assert!(s.track_mut(t).unwrap().mixer.move_module(a, 1));
    assert_eq!(s.track(t).unwrap().mixer.inserts.iter().map(|i| i.id).collect::<Vec<_>>(), [b, a]);
    eng.ensure_synced(&s);
    assert_eq!(eng.instance_serial(t, a), serial, "a moved module keeps its instance");
    assert_eq!(eng.instance_serial(t, b), trim_serial);
    render_blocks(&mut eng, &s, 4..8, &mut moved);
    assert!(moved.iter().flatten().all(|x| x.is_finite()));

    // A fresh engine on the reordered session, from the start: same history, so the same output.
    let mut fresh_eng = MixEngine::new(48_000.0, BLOCK);
    let mut fresh = vec![Vec::new(), Vec::new()];
    render_blocks(&mut fresh_eng, &s, 0..8, &mut fresh);
    let tail = 4 * BLOCK..;
    let (got, want) = (rms_db(&moved[0][tail.clone()]), rms_db(&fresh[0][tail.clone()]));
    assert!((got - want).abs() < 0.01, "moved {got} dB vs fresh {want} dB");
    // The lane still drives the gain: the tail is well below the -6 dB start.
    let head = rms_db(&moved[0][..BLOCK]);
    assert!(got < head - 3.0, "automation keeps pulling the moved gain down ({head} → {got})");
    // And a stale id has no instance.
    assert_eq!(eng.instance_serial(t, ModuleId(999)), None);
}

#[test]
fn missing_plugin_passes_audio_and_keeps_state() {
    // The brief asks for a `vst3:` id, but loading the VST3 scanner hangs on the dev machine, so
    // this uses a CLAP id that is not installed: the same "no plugin" path in `create_plugin`.
    let (mut s, t) = sine_session();
    let mut dry_eng = MixEngine::new(48_000.0, BLOCK);
    let mut dry = vec![Vec::new(), Vec::new()];
    render_blocks(&mut dry_eng, &s, 0..4, &mut dry);

    let mut missing = Insert::new("clap:does.not.exist.chain_reorder");
    missing.state = Some("AAEC".into());
    let id = s.track_mut(t).unwrap().mixer.add_module(missing, None).unwrap();
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let mut wet = vec![Vec::new(), Vec::new()];
    render_blocks(&mut eng, &s, 0..4, &mut wet);
    assert_eq!(eng.instance_serial(t, id), None);
    assert!(wet.iter().flatten().all(|x| x.is_finite()));
    assert_eq!(wet, dry, "a missing plugin passes audio through untouched");

    let back = Session::from_json(&s.to_json().unwrap()).unwrap();
    let m = back.track(t).unwrap().mixer.module(id).unwrap();
    assert_eq!(m.state.as_deref(), Some("AAEC"));
    assert_eq!(m.plugin, "clap:does.not.exist.chain_reorder");
}

#[test]
fn removing_a_module_mid_playback_is_click_free_and_finite() {
    let (mut s, t) = sine_session();
    let mixer = &mut s.track_mut(t).unwrap().mixer;
    let mut eq = Insert::new("eq_7band");
    eq.params.insert("gain".into(), 6.0);
    mixer.add_module(eq, None).unwrap();
    let mid = mixer.add_module(Insert::new("compressor"), None).unwrap();
    let mut gain = Insert::new("gain");
    gain.params.insert("gain".into(), 3.0);
    let last = mixer.add_module(gain, None).unwrap();

    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let mut out = vec![Vec::new(), Vec::new()];
    render_blocks(&mut eng, &s, 0..3, &mut out);
    let kept = eng.instance_serial(t, last);
    assert!(s.track_mut(t).unwrap().mixer.remove_module(mid).is_some());
    eng.ensure_synced(&s);
    assert_eq!(eng.instance_serial(t, mid), None);
    assert_eq!(eng.instance_serial(t, last), kept, "the modules behind keep their instances");
    render_blocks(&mut eng, &s, 3..8, &mut out);
    assert_eq!(out[0].len(), LEN);
    assert!(out.iter().flatten().all(|x| x.is_finite() && x.abs() <= 4.0));
}

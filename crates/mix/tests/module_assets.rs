//! Module assets (`Insert::assets`): the engine hands the prepared asset to the plugin in `sync`,
//! keeps its own reference, and everything it or the plugin lets go of comes back out of
//! `take_retired_assets` for the host to drop off the audio thread.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::SampleSource;
use soundcraft_dsp::plugins::fs::amp::ImpulseAsset;
use soundcraft_mix::MixEngine;
use soundcraft_model::{AssetData, ChannelFormat, Clip, Insert, ModuleId, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const BLOCK: usize = 512;
const LEN: usize = BLOCK * 200;

fn asset(len: usize) -> Arc<dyn SampleSource> {
    let ir: Vec<f32> = (1..=len).map(|i| ((i as f32) * 0.2).sin() * (-(i as f32) / 300.0).exp()).collect();
    Arc::new(ImpulseAsset::new(&ir, 48_000.0, 48_000.0).unwrap())
}

/// A guitar-ish tone through an Amp on its IR cabinet, the Twin's IR set to `reference`.
fn session(reference: &str, data: &Arc<dyn SampleSource>) -> (Session, TrackId, ModuleId) {
    let mut s = Session::default();
    let tone: Vec<f32> = (0..LEN).map(|n| 0.3 * (std::f32::consts::TAU * 196.0 * n as f32 / 48_000.0).sin()).collect();
    s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![tone.clone(), tone] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Gtr"));
    let clip = s.new_clip_id();
    let mut amp = Insert::new("fs_amp");
    amp.params.insert("cab".into(), 1.0);
    amp.assets.insert("irPathTwin".into(), reference.into());
    s.assets.insert(reference, 48_000, AssetData(Arc::new(Arc::clone(data))));
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "tone", SourceId(5), 0, 0, LEN as i64));
    tr.mixer.output = Route::Main;
    let m = tr.mixer.add_module(amp, None).unwrap();
    (s, t, m)
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>) {
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut buf);
        assert!(buf.iter().flatten().all(|v| v.is_finite()));
    }
}

fn same(a: &Arc<dyn SampleSource>, b: &Arc<dyn SampleSource>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

#[test]
fn the_plugin_gets_its_asset_and_the_engine_returns_what_it_lets_go_of() {
    let a = asset(2000);
    let (s, t, m) = session("a", &a);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.set_external_instances(true);
    render(&mut eng, &s, 0..4);
    let r = eng.meters[&t].modules[0].readouts;
    assert_eq!((r[5], r[7]), (1.0, 1.0), "the IR cabinet runs on the asset");

    // another IR for the same module: it crossfades in; the old one comes back, every reference
    let b = asset(4000);
    let mut s2 = s.clone();
    s2.track_mut(t).unwrap().mixer.module_mut(m).unwrap().assets.insert("irPathTwin".into(), "b".into());
    s2.assets.insert("b", 48_000, AssetData(Arc::new(Arc::clone(&b))));
    let mut back = Vec::new();
    for blk in 4..16 {
        render(&mut eng, &s2, blk..blk + 1);
        back.extend(eng.take_retired_assets());
    }
    assert!(back.len() >= 2 && back.iter().all(|x| same(x, &a)), "{} returned", back.len());
    drop(back);
    drop(s);
    let mut s2 = s2;
    s2.assets = Default::default();
    assert_eq!(Arc::strong_count(&a), 1, "nobody else holds the old IR");

    // the module goes: its asset comes back with it
    let mut s3 = s2.clone();
    s3.track_mut(t).unwrap().mixer.inserts.clear();
    render(&mut eng, &s3, 16..18);
    let back = eng.take_retired_assets();
    assert!(back.iter().any(|x| same(x, &b)));
    let plugins = eng.take_retired();
    assert_eq!(plugins.len(), 1);
    drop(plugins);
    drop(back);
    drop((s2, s3));
    assert_eq!(Arc::strong_count(&b), 1, "nobody else holds the removed module's IR");
}

#[test]
fn a_cleared_or_missing_asset_plays_the_built_in_cabinet() {
    let a = asset(800);
    let (mut s, t, m) = session("a", &a);
    s.assets = Default::default(); // the file could not be made ready (missing media)
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..4);
    let r = eng.meters[&t].modules[0].readouts;
    assert_eq!((r[5], r[7]), (0.0, 0.0), "built-in, no IR");
    s.track_mut(t).unwrap().mixer.module_mut(m).unwrap().assets.clear();
    render(&mut eng, &s, 4..6);
}

#[test]
fn rendering_with_an_ir_loaded_keeps_every_buffer_in_place() {
    // Steady state on the audio thread: the per-block structure check looks the IR up by `&str`
    // (no key built), nothing re-syncs, nothing reallocates, and taking the retired assets each
    // block (as the playback thread does) hands out nothing with capacity to free.
    let a = asset(3000);
    let (s, t, _) = session("bundled:twin.wav", &a);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.set_external_instances(true);
    render(&mut eng, &s, 0..4);
    let _ = eng.take_retired_assets();
    let before = eng.strip_buffers(t);
    assert!(!before.is_empty());
    let pool = s.assets.get("bundled:twin.wav", 48_000).map(|d| Arc::as_ptr(&d.0) as *const () as usize);
    for blk in 4..190 {
        render(&mut eng, &s, blk..blk + 1);
        let back = eng.take_retired_assets();
        assert!(back.is_empty() && back.capacity() == 0, "block {blk}: {} assets, capacity {}", back.len(), back.capacity());
        assert_eq!(eng.strip_buffers(t), before, "block {blk}: a strip buffer moved or the chain re-synced");
    }
    assert_eq!(s.assets.get("bundled:twin.wav", 48_000).map(|d| Arc::as_ptr(&d.0) as *const () as usize), pool);
    let r = eng.meters[&t].modules[0].readouts;
    assert_eq!((r[5], r[7]), (1.0, 1.0), "still on the IR");
}

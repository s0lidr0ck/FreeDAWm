//! Post-fader inserts (`Insert::post_fader`): they run after the fader and mute, so they are not
//! in the pre-fader copy; delay compensation counts them; a freeze leaves them live.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{add_delay, engine};
use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{MixEngine, render_range, render_track_frozen};
use soundcraft_model::{ChannelFormat, Clip, FrozenAudio, Insert, OutputPath, Route, SendSlot, Session, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const BLOCK: usize = 512;

/// A mono track playing `chan` from 0, added to `s`.
fn track(s: &mut Session, src: u64, chan: Vec<f32>) -> TrackId {
    let n = chan.len();
    s.pool.insert(SourceId(src), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![chan] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "x", SourceId(src), 0, 0, n as i64));
    t
}

fn session() -> Session {
    let mut s = Session::default();
    s.edit.click = false;
    s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
    s.outputs.push(OutputPath { name: "Cue".into(), first_channel: 2, format: ChannelFormat::Stereo });
    s
}

/// A +6 dB gain module, post-fader or not, on `t`.
fn gain(s: &mut Session, t: TrackId, post: bool) {
    let mut ins = Insert::new("gain");
    ins.params.insert("gain".into(), 6.0);
    ins.post_fader = post;
    s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap();
}

/// The main output and the "Cue" output of the 40th block (the gain has settled).
fn play(s: &Session) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in 0..40 {
        eng.render(s, b * BLOCK as i64, BLOCK, &mut out);
    }
    (out, eng.hardware_out()[1].clone())
}

#[test]
fn a_post_fader_module_is_after_the_fader_and_not_in_the_pre_fader_send() {
    let mut s = session();
    let t = track(&mut s, 1, vec![0.25; 48_000]);
    s.track_mut(t).unwrap().mixer.sends[0] = Some(SendSlot { level_db: 0.0, pre_fader: true, ..SendSlot::new(Route::Hardware("Cue".into())) });
    let (dry, dry_cue) = play(&s);
    let g = 10f32.powf(6.0 / 20.0);
    let mut pre = s.clone();
    gain(&mut pre, t, false);
    let (main_pre, cue_pre) = play(&pre);
    assert!(
        (main_pre[0][100] - dry[0][100] * g).abs() < 1e-4 && (cue_pre[0][100] - dry_cue[0][100] * g).abs() < 1e-4,
        "pre-fader: in both: {:?}",
        (main_pre[0][100], dry[0][100], cue_pre[0][100], dry_cue[0][100])
    );
    let mut post = s.clone();
    gain(&mut post, t, true);
    let (main_post, cue_post) = play(&post);
    assert!((main_post[0][100] - dry[0][100] * g).abs() < 1e-4, "in the main path");
    assert_eq!(cue_post, dry_cue, "not in the pre-fader send");
    // After the fader: muted, the module plays nothing (it hears the mute).
    post.track_mut(t).unwrap().mixer.mute = true;
    assert!(play(&post).0.iter().flatten().all(|x| *x == 0.0));
}

#[test]
fn delay_compensation_counts_a_post_fader_module_and_keeps_the_pre_fader_send_in_line() {
    let n = 8192;
    let mut imp = vec![0.0f32; n];
    imp[1000] = 0.5;
    let mut s = session();
    let a = track(&mut s, 1, imp.clone());
    let b = track(&mut s, 2, imp);
    let _ = b;
    let m = add_delay(&mut s, a, 300);
    s.track_mut(a).unwrap().mixer.inserts.iter_mut().find(|i| i.id == m).unwrap().post_fader = true;
    // Only A to the cue, pre-fader: it must line up with the main mix.
    s.track_mut(a).unwrap().mixer.sends[0] = Some(SendSlot { level_db: 0.0, pre_fader: true, ..SendSlot::new(Route::Hardware("Cue".into())) });
    let mut eng = engine(&s, BLOCK);
    let mut main = vec![Vec::new(); 2];
    let mut cue = vec![Vec::new(); 2];
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for blk in 0..(n / BLOCK) {
        eng.render(&s, (blk * BLOCK) as i64, BLOCK, &mut out);
        for (d, o) in main.iter_mut().zip(&out) {
            d.extend_from_slice(o);
        }
        for (d, o) in cue.iter_mut().zip(&eng.hardware_out()[1]) {
            d.extend_from_slice(&o[..BLOCK]);
        }
    }
    let hits = |c: &[f32]| c.iter().enumerate().filter(|(_, v)| v.abs() > 1e-4).map(|(i, _)| i).collect::<Vec<_>>();
    assert_eq!(hits(&main[0]), vec![1300], "A (late by its post-fader module) and B (held back for it) together");
    assert_eq!(hits(&cue[0]), vec![1300], "the pre-fader send waits for the post-fader module too");
}

#[test]
fn a_freeze_leaves_a_post_fader_module_live() {
    let mut s = session();
    let t = track(&mut s, 1, vec![0.25; 4 * BLOCK]);
    let plain = render_track_frozen(&s, t, (4 * BLOCK) as i64, None);
    gain(&mut s, t, true);
    let frozen = render_track_frozen(&s, t, (4 * BLOCK) as i64, None);
    assert_eq!(frozen, plain, "the render holds no post-fader module");
    let live = render_range(&s, Range::new(0, (4 * BLOCK) as i64), BLOCK);
    // Frozen, it plays the render and runs the post-fader module on it.
    s.pool.insert(SourceId(99), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: frozen })));
    let tr = s.track_mut(t).unwrap();
    tr.freeze = Some(FrozenAudio { source: SourceId(99), start: 0, upto: None });
    tr.frozen = true;
    let frozen_play = render_range(&s, Range::new(0, (4 * BLOCK) as i64), BLOCK);
    assert!((frozen_play[0][300] - live[0][300]).abs() < 1e-5, "{} vs {}", frozen_play[0][300], live[0][300]);
}

#[test]
fn the_master_runs_its_post_fader_modules_after_its_fader() {
    let mut s = session();
    track(&mut s, 1, vec![0.25; 48_000]);
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    let (dry, _) = play(&s);
    gain(&mut s, master, true);
    let (wet, _) = play(&s);
    let g = 10f32.powf(6.0 / 20.0);
    assert!((wet[0][100] - dry[0][100] * g).abs() < 1e-4, "{} vs {}", wet[0][100], dry[0][100] * g);
    // The module hears the fader: at -inf there is nothing for it to raise.
    s.track_mut(master).unwrap().mixer.volume_db = -144.0;
    assert!(play(&s).0.iter().flatten().all(|x| x.abs() < 1e-6));
}

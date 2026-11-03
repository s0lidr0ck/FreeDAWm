//! FireStation Shale's Kit Link in the mix engine (B4's link groups): a room mic following the
//! snare on another track opens on the snare's hits whatever the track order (the host renders a
//! Follow after its Lead); the snare's mic and a kick mic share as peers; a deleted member leaves
//! the group at once and nothing crashes; a missing partner leaves each gate on its own; links
//! survive a save; rendering with Kit Link on moves no buffer and replaces no slab.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::LinkRole;
use soundcraft_dsp::plugins::fs::shale::shale_link;
use soundcraft_mix::{LinkProblem, MixEngine, link_refusal, link_routes};
use soundcraft_model::{ChannelFormat, Clip, Insert, ModuleId, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const SR: usize = 48_000;
const BLOCK: usize = 512;

/// A snare-ish hit every `period` samples from `first` (a 200 Hz body decaying in ~60 ms).
fn hits(len: usize, period: usize, first: usize, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| {
            if i < first {
                return 0.0;
            }
            let t = ((i - first) % period) as f32 / SR as f32;
            amp * (-t / 0.06).exp() * (std::f32::consts::TAU * 200.0 * t).sin()
        })
        .collect()
}

/// An audio track playing `signal`.
fn track(s: &mut Session, name: &str, signal: &[f32]) -> TrackId {
    let src = SourceId(1000 + s.tracks.len() as u64);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR as u32, channels: vec![signal.to_vec(); 2] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(name));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, name, src, 0, 0, signal.len() as i64));
    tr.mixer.output = Route::Main;
    t
}

/// A Shale with Kit Link on in `role` (0 Lead, 1 Follow, 2 Pair) for `mic`, in link group `group`.
fn shale(s: &mut Session, t: TrackId, role: f32, mic: f32, group: Option<u32>) -> ModuleId {
    let mut ins = Insert::new("fs_shale");
    for (id, v) in [("link", 1.0), ("linkRole", role), ("instrument", mic), ("reject", 0.0)] {
        ins.params.insert(id.into(), v);
    }
    ins.link_group = group;
    s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap()
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>) -> Vec<f32> {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut all = Vec::new();
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
        assert!(out.iter().flatten().all(|x| x.is_finite()));
        all.extend_from_slice(&out[0]);
    }
    all
}

fn readouts(eng: &MixEngine, t: TrackId, m: ModuleId) -> [f32; 8] {
    eng.meters[&t].modules.iter().find(|x| x.module == m).unwrap().readouts
}

/// The room (Follow) on the first track, the snare (Lead) on the second, both in group 1.
fn kit_session() -> (Session, (TrackId, ModuleId), (TrackId, ModuleId)) {
    let len = SR * 2;
    let snare_sig = hits(len, 12_000, 2000, 0.8);
    let room_sig: Vec<f32> = snare_sig.iter().map(|v| v * 0.25).collect();
    let mut s = Session::default();
    let room = track(&mut s, "Room", &room_sig);
    let snare = track(&mut s, "Snare", &snare_sig);
    let rm = shale(&mut s, room, 1.0, 10.0, Some(1));
    let sm = shale(&mut s, snare, 0.0, 3.0, Some(1));
    (s, (room, rm), (snare, sm))
}

#[test]
fn a_follow_on_an_earlier_track_opens_on_its_lead_across_tracks() {
    let (s, (room, rm), (snare, sm)) = kit_session();
    let routes = link_routes(&s);
    let r = |t: TrackId, m: ModuleId| routes.iter().find(|r| r.track == t && r.module == m).copied().unwrap();
    assert_eq!((r(room, rm).role, r(room, rm).result), (Some(LinkRole::Consumer), Ok(())), "the Follow is ordered after its Lead");
    assert_eq!((r(snare, sm).role, r(snare, sm).result), (Some(LinkRole::Peer), Ok(())), "the Lead has company");
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    render(&mut eng, &s, 0..2 * SR / BLOCK);
    let lead = readouts(&eng, snare, sm)[4];
    assert!(lead >= 7.0, "{lead}");
    assert_eq!(readouts(&eng, room, rm)[4], lead, "the room opened on every snare hit");
    let slab = eng.link_slab(1).unwrap();
    assert_eq!((slab.len(), slab.layout()), (shale_link::WORDS, shale_link::LAYOUT), "sized for Shale when the group was made");
    assert_eq!((slab.peers(), slab.consumers()), (1, 1));

    // Two Leads (a kick and a snare, the usual kit) share as peers: neither is alone.
    let mut k = Session::default();
    let a = track(&mut k, "Kick", &hits(SR, 12_000, 0, 0.8));
    let b = track(&mut k, "Snare", &hits(SR, 12_000, 6000, 0.8));
    let ka = shale(&mut k, a, 0.0, 0.0, Some(2));
    let kb = shale(&mut k, b, 0.0, 3.0, Some(2));
    assert!(link_routes(&k).iter().all(|r| r.result == Ok(())), "{:?}", link_routes(&k));
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    render(&mut eng, &k, 0..SR / BLOCK);
    assert!(readouts(&eng, a, ka)[4] >= 3.0 && readouts(&eng, b, kb)[4] >= 3.0);
}

#[test]
fn a_member_deleted_mid_play_leaves_at_once_and_nothing_crashes() {
    let (mut s, (room, rm), (snare, sm)) = kit_session();
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    render(&mut eng, &s, 0..40);
    let slab = Arc::clone(eng.link_slab(1).unwrap());
    let member = eng.link_member(snare, sm).unwrap();
    let probe = |slab: &Arc<_>| {
        let mut k = shale_link::KitLink::default();
        k.set(Some(Arc::clone(slab)), 15);
        k
    };
    assert!(probe(&slab).presence(member).is_some());
    // the Lead goes: its slot is freed when it is retired, not whenever it is dropped
    s.track_mut(snare).unwrap().mixer.remove_module(sm);
    let before = readouts(&eng, room, rm)[4];
    render(&mut eng, &s, 40..120);
    assert!(probe(&slab).presence(member).is_none(), "the deleted Lead is gone from the group");
    assert_eq!(readouts(&eng, room, rm)[4], before, "the Follow has nobody to follow: it stays shut");
    assert_eq!(link_routes(&s)[0].result, Err(LinkProblem::Alone));
    // a newcomer takes the freed index; the retired instance dropped later never clears it
    let sm2 = shale(&mut s, snare, 0.0, 3.0, Some(1));
    render(&mut eng, &s, 120..200);
    assert_eq!(eng.link_member(snare, sm2), Some(member));
    drop(eng.take_retired());
    render(&mut eng, &s, 200..250);
    assert!(probe(&slab).presence(member).is_some(), "the newcomer's slot survives the old instance");
    assert!(readouts(&eng, room, rm)[4] > before, "and the Follow opens on it");
    // the whole snare track goes, then the room's: no crash, slabs retired
    s.tracks.retain(|t| t.id != snare);
    render(&mut eng, &s, 250..260);
    s.track_mut(room).unwrap().mixer.remove_module(rm);
    render(&mut eng, &s, 260..270);
    assert!(eng.link_slab(1).is_none());
}

#[test]
fn a_missing_partner_and_no_group_keep_each_gate_on_its_own() {
    let len = SR * 2;
    let sig = hits(len, 12_000, 2000, 0.8);
    let mut s = Session::default();
    let a = track(&mut s, "Snare", &sig);
    let alone = shale(&mut s, a, 0.0, 3.0, Some(4));
    let b = track(&mut s, "Snare 2", &sig);
    let ungrouped = shale(&mut s, b, 0.0, 3.0, None);
    let c = track(&mut s, "Room", &sig);
    let follow = shale(&mut s, c, 1.0, 10.0, Some(5));
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    render(&mut eng, &s, 0..len / BLOCK);
    assert_eq!(readouts(&eng, a, alone)[4], readouts(&eng, b, ungrouped)[4], "a Lead alone in its group gates as one in none");
    assert!(readouts(&eng, a, alone)[4] >= 7.0);
    assert_eq!(readouts(&eng, c, follow)[4], 0.0, "a Follow with no Lead never opens");
    let routes = link_routes(&s);
    assert!(routes.iter().all(|r| r.result == Err(LinkProblem::Alone)), "{routes:?}");
}

#[test]
fn kit_link_survives_a_save_and_renders_the_same() {
    let (s, (room, rm), _) = kit_session();
    let json = s.to_json().unwrap();
    let mut back = Session::from_json(&json).unwrap();
    back.pool = s.pool.clone();
    let m = back.track(room).unwrap().mixer.module(rm).unwrap();
    assert_eq!((m.link_group, m.params.get("linkRole").copied()), (Some(1), Some(1.0)));
    let mut e1 = MixEngine::new(SR as f32, BLOCK);
    let mut e2 = MixEngine::new(SR as f32, BLOCK);
    let a = render(&mut e1, &s, 0..150);
    let b = render(&mut e2, &back, 0..150);
    assert_eq!(a, b);
    assert!(readouts(&e2, room, rm)[4] >= 3.0);
}

#[test]
fn rendering_with_kit_link_moves_no_buffer_and_replaces_no_slab() {
    let (s, (room, rm), (snare, _)) = kit_session();
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    eng.set_external_instances(true);
    render(&mut eng, &s, 0..4);
    let state = |eng: &MixEngine| {
        (eng.strip_buffers(room), eng.strip_buffers(snare), eng.instance_serial(room, rm), eng.link_slab(1).map(|x| Arc::as_ptr(x) as usize))
    };
    let before = state(&eng);
    for blk in 4..2 * SR / BLOCK {
        render(&mut eng, &s, blk..blk + 1);
        assert_eq!(state(&eng), before, "block {blk}");
        assert!(eng.take_retired_links().is_empty());
    }
    assert!(readouts(&eng, room, rm)[4] >= 7.0);
    // a Locate (reset) on the audio thread: the same, and the group renders the same again
    let first = render(&mut eng, &s, 0..100);
    eng.reset();
    let again = render(&mut eng, &s, 0..100);
    eng.reset();
    let third = render(&mut eng, &s, 0..100);
    assert_eq!(again, third, "two resets of a linked group render the same");
    assert_eq!(first.len(), again.len());
    assert_eq!(state(&eng), before);
}

#[test]
fn a_follow_that_cannot_render_after_its_lead_reads_it_a_block_late_and_is_not_refused() {
    // A gate on the snare track is keyed from the room track, so the room renders first: the
    // Follow on the room can't be ordered after the snare's Lead (it would close a loop). It reads
    // the Lead's triggers a block late through the 12 ms reach-back, which is how peers work, so
    // it is neither reported nor refused, and it still opens on every hit.
    let (mut s, (room, rm), (snare, sm)) = kit_session();
    let keyed = shale(&mut s, snare, 0.0, 3.0, None);
    s.track_mut(snare).unwrap().mixer.module_mut(keyed).unwrap().sidechain =
        Some(soundcraft_model::SidechainSource { track: room, pre_fader: false });
    s.track_mut(room).unwrap().mixer.module_mut(rm).unwrap().link_group = None;
    assert_eq!(link_refusal(&s, room, rm, Some(1)), None);
    s.track_mut(room).unwrap().mixer.module_mut(rm).unwrap().link_group = Some(1);
    let routes = link_routes(&s);
    assert!(routes.iter().all(|r| r.result == Ok(())), "{routes:?}");
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    render(&mut eng, &s, 0..2 * SR / BLOCK);
    let lead = readouts(&eng, snare, sm)[4];
    assert!(lead >= 7.0, "{lead}");
    assert_eq!(readouts(&eng, room, rm)[4], lead, "the room still opened on every snare hit");
}

#[test]
fn a_follow_resolved_first_does_not_block_a_levelup_pair_the_other_way() {
    // The room track (first in the session) holds the Follow and a LevelUp IN; the snare track
    // holds the Lead and the LevelUp OUT. The Follow asks for snare -> room, the LevelUp pair needs
    // room -> snare. Producer edges are hard (a consumer without one reads a block late) and peer
    // edges soft (Shale reaches back 12 ms), so the pair is ordered first and the Follow, met
    // first in session order, gives way: nothing reads late and nothing is refused.
    let (mut s, _, (snare, _)) = kit_session();
    let room = s.tracks[0].id;
    let levelup = |s: &mut Session, t: TrackId, role: f32| {
        let mut ins = Insert::new("fs_levelup");
        ins.params.insert("link".into(), role);
        ins.link_group = Some(2);
        s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap()
    };
    let lu_in = levelup(&mut s, room, 1.0);
    let lu_out = levelup(&mut s, snare, 2.0);
    let routes = link_routes(&s);
    let out = routes.iter().find(|r| r.track == snare && r.module == lu_out).unwrap();
    assert_eq!(out.role, Some(LinkRole::Consumer));
    assert!(routes.iter().all(|r| r.result == Ok(())), "{routes:?}");
    s.track_mut(snare).unwrap().mixer.module_mut(lu_out).unwrap().link_group = None;
    assert_eq!(link_refusal(&s, snare, lu_out, Some(2)), None);
    s.track_mut(room).unwrap().mixer.module_mut(lu_in).unwrap().link_group = None;
    s.track_mut(snare).unwrap().mixer.module_mut(lu_out).unwrap().link_group = Some(2);
    assert_eq!(link_refusal(&s, room, lu_in, Some(2)), None);
}

//! Instance links (`Insert::link_group`): every module in a group shares one value bus; a
//! group's producers render before its consumers in the same block whatever the track order; a
//! cycle, a wrong chain order or an unorderable phase is reported and reads a block late; a
//! missing or deleted partner never crashes; nothing reallocates while rendering.
//!
//! The probe (`test_link`, feature `test-plugins`): a consumer puts 0 on channel 0 when its
//! producer wrote in the same block (1: a block late or nothing written), and the producer's Value
//! on channel 1.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_mix::{LinkProblem, MixEngine, link_refusal, link_routes};
use soundcraft_model::{ChannelFormat, Insert, ModuleId, Route, Session, TrackId, TrackKind};

const BLOCK: usize = 256;
const OFF: f32 = 0.0;
const PRODUCER: f32 = 1.0;
const CONSUMER: f32 = 2.0;
const PEER: f32 = 3.0;

fn probe(role: f32, value: f32, group: Option<u32>) -> Insert {
    let mut i = Insert::new("test_link");
    i.params.insert("role".into(), role);
    i.params.insert("value".into(), value);
    i.link_group = group;
    i
}

fn track(s: &mut Session, name: &str, kind: TrackKind) -> TrackId {
    let t = s.add_track(kind, ChannelFormat::Stereo, Some(name));
    s.track_mut(t).unwrap().mixer.output = Route::Main;
    t
}

fn add(s: &mut Session, t: TrackId, ins: Insert) -> ModuleId {
    s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap()
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>) {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
        assert!(out.iter().flatten().all(|x| x.is_finite()));
    }
}

/// What the consumer module `m` on `t` put out in the last block (`out_peak` over both channels;
/// the probe's channel 0 is the block lag, channel 1 the producer's value).
fn out(eng: &MixEngine, t: TrackId, m: ModuleId) -> f32 {
    eng.meters[&t].modules.iter().find(|x| x.module == m).unwrap().out_peak
}

fn result(s: &Session, t: TrackId, m: ModuleId) -> Result<(), LinkProblem> {
    link_routes(s).into_iter().find(|r| r.track == t && r.module == m).unwrap().result
}

#[test]
fn a_producer_on_a_later_track_renders_first() {
    let mut s = Session::default();
    let a = track(&mut s, "Out", TrackKind::Audio);
    let b = track(&mut s, "In", TrackKind::Audio);
    let c = add(&mut s, a, probe(CONSUMER, 0.0, Some(7)));
    let p = add(&mut s, b, probe(PRODUCER, 0.0, Some(7)));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..8);
    assert_eq!(out(&eng, a, c), 0.0, "same block, across tracks");
    assert_eq!(result(&s, a, c), Ok(()));
    assert_eq!(result(&s, b, p), Ok(()));
    let slab = eng.link_slab(7).unwrap();
    assert_eq!((slab.producers(), slab.consumers(), slab.peers(), slab.layout()), (1, 1, 0, 0x7E57));
    // the value crosses too (channel 1)
    s.track_mut(b).unwrap().mixer.module_mut(p).unwrap().params.insert("value".into(), 0.25);
    render(&mut eng, &s, 8..9);
    assert_eq!(out(&eng, a, c), 0.25);
}

#[test]
fn within_a_chain_the_producer_must_come_first() {
    let mut s = Session::default();
    let t = track(&mut s, "Bus", TrackKind::Audio);
    let p = add(&mut s, t, probe(PRODUCER, 0.0, Some(1)));
    let c = add(&mut s, t, probe(CONSUMER, 0.0, Some(1)));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..4);
    assert_eq!(out(&eng, t, c), 0.0);
    // consumer first: it reads the previous block, and says so
    assert!(s.track_mut(t).unwrap().mixer.move_module(c, 0));
    assert_eq!(result(&s, t, c), Err(LinkProblem::Order));
    render(&mut eng, &s, 4..8);
    assert_eq!(out(&eng, t, c), 1.0, "a block late");
    // and the command layer would refuse to make that
    let mut fresh = s.clone();
    fresh.track_mut(t).unwrap().mixer.module_mut(c).unwrap().link_group = None;
    assert!(link_refusal(&fresh, t, c, Some(1)).is_some_and(|w| w.contains("chain")));
    assert!(link_refusal(&fresh, t, c, None).is_none());
    let _ = p;
}

#[test]
fn a_link_cycle_is_reported_and_reads_a_block_late() {
    // Track A: IN of group 1, OUT of group 2. Track B: IN of group 2, OUT of group 1.
    let mut s = Session::default();
    let a = track(&mut s, "A", TrackKind::Audio);
    let b = track(&mut s, "B", TrackKind::Audio);
    add(&mut s, a, probe(PRODUCER, 0.0, Some(1)));
    let ca = add(&mut s, a, probe(CONSUMER, 0.0, Some(2)));
    add(&mut s, b, probe(PRODUCER, 0.0, Some(2)));
    let cb = add(&mut s, b, probe(CONSUMER, 0.0, Some(1)));
    // session order: A's consumer (group 2, producer on B) is ordered first, so B's is the cycle
    assert_eq!(result(&s, a, ca), Ok(()));
    assert_eq!(result(&s, b, cb), Err(LinkProblem::Cycle));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..6);
    assert_eq!(out(&eng, a, ca), 0.0);
    assert_eq!(out(&eng, b, cb), 1.0, "the cycled consumer reads the previous block");
    // a sidechain from B to A also orders A after B: the links still never hang or crash
    s.track_mut(a).unwrap().mixer.module_mut(ca).unwrap().sidechain = Some(soundcraft_model::SidechainSource { track: b, pre_fader: false });
    render(&mut eng, &s, 6..10);
}

#[test]
fn a_missing_or_deleted_partner_never_crashes() {
    let mut s = Session::default();
    let a = track(&mut s, "Out", TrackKind::Audio);
    let b = track(&mut s, "In", TrackKind::Audio);
    let c = add(&mut s, a, probe(CONSUMER, 0.0, Some(3)));
    // alone: the slab is there, nobody writes it
    assert_eq!(result(&s, a, c), Err(LinkProblem::Alone));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..4);
    assert_eq!(eng.link_slab(3).unwrap().producers(), 0);
    assert_eq!(out(&eng, a, c), 1.0, "nothing written");
    // a partner arrives
    let p = add(&mut s, b, probe(PRODUCER, 0.5, Some(3)));
    render(&mut eng, &s, 4..8);
    assert_eq!(out(&eng, a, c), 0.5);
    assert_eq!(eng.link_slab(3).unwrap().producers(), 1);
    // and is deleted: the consumer keeps the last value, and the slab says nobody produces
    s.track_mut(b).unwrap().mixer.remove_module(p);
    assert_eq!(result(&s, a, c), Err(LinkProblem::Alone));
    render(&mut eng, &s, 8..12);
    assert_eq!(eng.link_slab(3).unwrap().producers(), 0);
    // the producer's whole track goes
    let p2 = add(&mut s, b, probe(PRODUCER, 0.5, Some(3)));
    render(&mut eng, &s, 12..14);
    s.tracks.retain(|t| t.id != b);
    render(&mut eng, &s, 14..18);
    assert_eq!(eng.link_slab(3).unwrap().producers(), 0);
    let _ = p2;
    // an inactive producer track does not count
    let b2 = track(&mut s, "In 2", TrackKind::Audio);
    add(&mut s, b2, probe(PRODUCER, 0.5, Some(3)));
    s.track_mut(b2).unwrap().inactive = true;
    assert_eq!(result(&s, a, c), Err(LinkProblem::Alone));
    render(&mut eng, &s, 18..20);
    // the consumer leaves the group: no slab, and the group's slab goes once empty
    s.tracks.retain(|t| t.id != b2);
    s.track_mut(a).unwrap().mixer.module_mut(c).unwrap().link_group = None;
    render(&mut eng, &s, 20..22);
    assert_eq!(out(&eng, a, c), 1000.0, "no slab");
    assert!(eng.link_slab(3).is_none());
}

#[test]
fn phases_the_engine_cannot_order_are_reported() {
    let mut s = Session::default();
    let a = track(&mut s, "Audio", TrackKind::Audio);
    let x = track(&mut s, "Aux", TrackKind::Aux);
    let m = s.tracks.iter().find(|t| t.kind == TrackKind::Master).map(|t| t.id);
    let c = add(&mut s, a, probe(CONSUMER, 0.0, Some(5)));
    let p = add(&mut s, x, probe(PRODUCER, 0.0, Some(5)));
    assert!(matches!(result(&s, a, c), Err(LinkProblem::Unsupported(_))));
    // the other way round is fine: auxes render after audio tracks
    let c2 = add(&mut s, x, probe(CONSUMER, 0.0, Some(6)));
    add(&mut s, a, probe(PRODUCER, 0.0, Some(6)));
    assert_eq!(result(&s, x, c2), Ok(()));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..4);
    assert_eq!(out(&eng, x, c2), 0.0);
    assert_eq!(out(&eng, a, c), 1.0);
    // a consumer on a master fader comes after everything
    if let Some(m) = m {
        let cm = add(&mut s, m, probe(CONSUMER, 0.0, Some(6)));
        assert_eq!(result(&s, m, cm), Ok(()));
        render(&mut eng, &s, 4..6);
        assert_eq!(out(&eng, m, cm), 0.0);
    }
    let _ = p;
}

#[test]
fn peers_and_off_members_take_no_order() {
    let mut s = Session::default();
    let a = track(&mut s, "A", TrackKind::Audio);
    let b = track(&mut s, "B", TrackKind::Audio);
    let pa = add(&mut s, a, probe(PEER, 0.0, Some(9)));
    assert_eq!(result(&s, a, pa), Err(LinkProblem::Alone));
    let pb = add(&mut s, b, probe(PEER, 0.0, Some(9)));
    let off = add(&mut s, b, probe(OFF, 0.0, Some(9)));
    assert_eq!(result(&s, a, pa), Ok(()));
    assert_eq!(result(&s, b, pb), Ok(()));
    let r = link_routes(&s).into_iter().find(|r| r.track == b && r.module == off).unwrap();
    assert_eq!((r.role, r.result), (None, Ok(())));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..4);
    assert_eq!(eng.link_slab(9).unwrap().peers(), 2);
    // switching a role is a structure change: the engine re-syncs and re-orders
    s.track_mut(a).unwrap().mixer.module_mut(pa).unwrap().params.insert("role".into(), CONSUMER);
    s.track_mut(b).unwrap().mixer.module_mut(pb).unwrap().params.insert("role".into(), PRODUCER);
    render(&mut eng, &s, 4..8);
    assert_eq!(eng.link_slab(9).unwrap().producers(), 1);
    assert_eq!(out(&eng, a, pa), 0.0, "re-ordered: the new producer on B renders first");
}

#[test]
fn rendering_with_links_keeps_every_buffer_and_slab_in_place() {
    let mut s = Session::default();
    let a = track(&mut s, "Out", TrackKind::Audio);
    let b = track(&mut s, "In", TrackKind::Audio);
    let c = add(&mut s, a, probe(CONSUMER, 0.0, Some(2)));
    add(&mut s, b, probe(PRODUCER, 0.1, Some(2)));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.set_external_instances(true);
    render(&mut eng, &s, 0..4);
    let before =
        (eng.strip_buffers(a), eng.strip_buffers(b), eng.instance_serial(a, c), eng.link_slab(2).map(|x| std::sync::Arc::as_ptr(x) as usize));
    for blk in 4..200 {
        render(&mut eng, &s, blk..blk + 1);
        let now =
            (eng.strip_buffers(a), eng.strip_buffers(b), eng.instance_serial(a, c), eng.link_slab(2).map(|x| std::sync::Arc::as_ptr(x) as usize));
        assert_eq!(now, before, "block {blk}: a buffer moved, an instance or slab was replaced, or the chain re-synced");
        assert_eq!(out(&eng, a, c), 0.1);
        assert!(eng.take_retired_links().is_empty(), "block {blk}: nothing retired in steady state");
    }
}

#[test]
fn links_survive_a_save_round_trip() {
    let mut s = Session::default();
    let a = track(&mut s, "A", TrackKind::Audio);
    let c = add(&mut s, a, probe(CONSUMER, 0.0, Some(42)));
    let json = s.to_json().unwrap();
    let back = Session::from_json(&json).unwrap();
    assert_eq!(back.track(a).unwrap().mixer.module(c).unwrap().link_group, Some(42));
    // an unlinked module writes no field, so older sessions (without it) open unlinked
    s.track_mut(a).unwrap().mixer.module_mut(c).unwrap().link_group = None;
    let old = s.to_json().unwrap();
    assert!(!old.contains("link_group"));
    let back = Session::from_json(&old).unwrap();
    assert_eq!(back.track(a).unwrap().mixer.module(c).unwrap().link_group, None);
}

#[test]
fn members_keep_their_index_and_newcomers_take_a_freed_one() {
    let mut s = Session::default();
    let ts: Vec<TrackId> = ["A", "B", "C", "D"].iter().map(|n| track(&mut s, n, TrackKind::Audio)).collect();
    let ms: Vec<ModuleId> = ts[..3].iter().map(|t| add(&mut s, *t, probe(PEER, 0.0, Some(1)))).collect();
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..2);
    let idx = |eng: &MixEngine, i: usize, m: ModuleId| eng.link_member(ts[i], m);
    assert_eq!((idx(&eng, 0, ms[0]), idx(&eng, 1, ms[1]), idx(&eng, 2, ms[2])), (Some(0), Some(1), Some(2)));
    let slab = eng.link_slab(1).unwrap();
    assert_eq!(slab.members(), 3);
    // each plugin got its index (the probe marks word 100 + index)
    assert_eq!((slab.load(100), slab.load(101), slab.load(102)), (Some(1), Some(1), Some(1)));
    // B leaves: A and C keep theirs, the count stays (a gap)
    s.track_mut(ts[1]).unwrap().mixer.remove_module(ms[1]);
    render(&mut eng, &s, 2..3);
    assert_eq!((idx(&eng, 0, ms[0]), idx(&eng, 2, ms[2])), (Some(0), Some(2)));
    assert_eq!(eng.link_slab(1).unwrap().members(), 3);
    // D joins: it takes the freed index 1
    let md = add(&mut s, ts[3], probe(PEER, 0.0, Some(1)));
    render(&mut eng, &s, 3..4);
    assert_eq!(idx(&eng, 3, md), Some(1));
    // C leaves: the trailing index goes, the count shrinks
    s.track_mut(ts[2]).unwrap().mixer.remove_module(ms[2]);
    render(&mut eng, &s, 4..5);
    assert_eq!(eng.link_slab(1).unwrap().members(), 2);
}

#[test]
fn a_bigger_member_grows_the_slab_and_old_slabs_are_retired_not_freed() {
    let mut s = Session::default();
    let a = track(&mut s, "Out", TrackKind::Audio);
    let b = track(&mut s, "In", TrackKind::Audio);
    let c = add(&mut s, a, probe(CONSUMER, 0.0, Some(4)));
    add(&mut s, b, probe(PRODUCER, 0.75, Some(4)));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.set_external_instances(true);
    render(&mut eng, &s, 0..2);
    let _ = eng.take_retired_links();
    let small = std::sync::Arc::clone(eng.link_slab(4).unwrap());
    assert_eq!(small.len(), 256);
    // a member asking for more words joins: a longer copy replaces the slab
    let mut big = probe(PEER, 0.0, Some(4));
    big.plugin = "test_link_big".into();
    let t3 = track(&mut s, "Big", TrackKind::Audio);
    let mb = add(&mut s, t3, big);
    eng.ensure_synced(&s);
    let grown = std::sync::Arc::clone(eng.link_slab(4).unwrap());
    assert!(grown.len() >= 256 + 65_536);
    assert_eq!(grown.load_f32(1), Some(0.75), "the words were copied");
    let retired = eng.take_retired_links();
    assert!(retired.iter().any(|r| std::sync::Arc::ptr_eq(r, &small)), "the old slab is retired, not dropped here");
    render(&mut eng, &s, 2..4);
    assert_eq!(out(&eng, a, c), 0.75, "every member moved to the grown slab");
    drop(retired);
    // the group empties: its slab and the slots' references are retired too
    s.track_mut(t3).unwrap().mixer.remove_module(mb);
    for t in [a, b] {
        for i in &mut s.track_mut(t).unwrap().mixer.inserts {
            i.link_group = None;
        }
    }
    render(&mut eng, &s, 4..5);
    assert!(eng.link_slab(4).is_none());
    let retired = eng.take_retired_links();
    assert!(retired.iter().any(|r| std::sync::Arc::ptr_eq(r, &grown)));
    // the removed instance still holds it: it goes to the host with the instance
    let plugins = eng.take_retired();
    assert_eq!(plugins.len(), 1);
    // nothing on the audio side holds it any more: the retired lists (and this test) are the last
    drop(retired);
    drop(plugins);
    assert_eq!(std::sync::Arc::strong_count(&grown), 1);
}

use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

const SR: f32 = 48_000.0;

fn sine(len: usize, amp: f32, f: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * f * i as f32 / SR).sin()).collect()
}

/// A level in dBFS as a sine amplitude.
fn amp(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn prepared() -> LevelUp {
    let mut s = LevelUp::new();
    s.prepare(SR, 512, 2);
    s
}

fn run(s: &mut LevelUp, io: &mut [Vec<f32>], block: usize) {
    let len = io.iter().map(Vec::len).min().unwrap_or(0);
    let mut buf: Vec<Vec<f32>> = io.iter().map(|_| vec![0.0; block]).collect();
    let mut pos = 0;
    while pos < len {
        let n = block.min(len - pos);
        for (b, c) in buf.iter_mut().zip(io.iter()) {
            b[..n].copy_from_slice(&c[pos..pos + n]);
        }
        s.process(&mut buf, n);
        for (b, c) in buf.iter().zip(io.iter_mut()) {
            c[pos..pos + n].copy_from_slice(&b[..n]);
        }
        pos += n;
    }
}

/// Runs `secs` of a stereo sine at `db` dBFS; returns the output.
fn feed(s: &mut LevelUp, secs: f32, db: f32) -> Vec<Vec<f32>> {
    let x = sine((secs * SR) as usize, amp(db), 997.0);
    let mut io = vec![x.clone(), x];
    run(s, &mut io, 512);
    io
}

fn all_finite(io: &[Vec<f32>]) -> bool {
    io.iter().flatten().all(|v| v.is_finite())
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn readouts(s: &LevelUp) -> [f32; 8] {
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    r
}

fn playing(at: i64) -> Transport {
    Transport { playing: true, samples: at, tempo: 120.0, numerator: 4, denominator: 4, ..Transport::default() }
}

const HOSTILE: [f32; 9] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0];
const CHOICES: [&str; 13] =
    ["mode", "normalize", "freeze", "ceiling", "learn", "link", "linkGroup", "reference", "refSlot", "platform", "preview", "dither", "shaping"];

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    let ids: Vec<&str> = LEVELUP_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        [
            "attack",
            "release",
            "gate",
            "maxGain",
            "target",
            "mode",
            "normalize",
            "freeze",
            "ceiling",
            "learn",
            "link",
            "linkGroup",
            "reference",
            "refSlot",
            "platform",
            "preview",
            "dither",
            "shaping",
            "bypass"
        ]
    );
    let defaults: Vec<f32> = LEVELUP_PARAMS.iter().map(|p| p.default).collect();
    assert_eq!(defaults, [0.18367, 0.17949, 0.24324, 0.5, 0.72727, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let names: Vec<&str> = LEVELUP_PARAMS.iter().map(|p| p.name).collect();
    assert_eq!((names[10], names[11], names[13], names[17]), ("Role", "Pair", "Ref Slot", "Shaped"));
    assert_eq!(LEVELUP_PARAMS[d::P_PLATFORM].choices, PLATFORM_NAMES);
    for (i, p) in d::PLATFORMS.iter().enumerate() {
        assert_eq!(p.name, PLATFORM_NAMES[i]);
    }
    assert_eq!(LEVELUP_PARAMS[d::P_BYPASS].group, "", "the engine's bypass is hidden");
    // formatParam, through the engine's own maps
    assert_eq!(LEVELUP_PARAMS[d::P_ATTACK].format(0.18367), "1.00 s");
    assert_eq!(LEVELUP_PARAMS[d::P_RELEASE].format(0.17949), "4.00 s");
    assert_eq!(LEVELUP_PARAMS[d::P_GATE].format(0.24324), "-12 dB");
    assert_eq!(LEVELUP_PARAMS[d::P_MAX_GAIN].format(0.5), "12.0 dB");
    assert_eq!(LEVELUP_PARAMS[d::P_TARGET].format(0.72727), "-18 LUFS");
    assert_eq!(LEVELUP_PARAMS[d::P_TARGET].format(1.0), "-6 LUFS");
}

#[test]
fn registered_with_links_references_no_latency_and_no_tail() {
    let mut p = crate::create("fs_levelup").unwrap();
    let info = p.info();
    assert_eq!((info.category, info.references, info.sidechain), (Category::FireStation, 4, false));
    let spec = info.link.unwrap();
    assert_eq!((spec.role_param, spec.layout, spec.words), (Some("link"), level_up_link::LAYOUT, level_up_link::WORDS));
    // Off takes no part, IN produces, OUT consumes
    for (role, want) in [(0.0, None), (1.0, Some(LinkRole::Producer)), (2.0, Some(LinkRole::Consumer))] {
        assert_eq!(crate::link_role(info, |id| (id == "link").then_some(role)), want);
    }
    p.prepare(SR, 512, 2);
    let mut io = vec![sine(512, 0.5, 440.0), sine(512, 0.5, 440.0)];
    p.process(&mut io, 512);
    assert_eq!((p.latency(), p.tail_samples()), (0, 0));
    assert_eq!(factory_presets("fs_levelup").len(), 6);
}

#[test]
fn the_leveler_rides_to_its_target() {
    // A -26 dBFS stereo 997 Hz sine (-26 LUFS, EBU Tech 3341) against the default -18 LUFS
    // target: turned up by 8 dB (Max Gain 12), and the readouts say so.
    let mut s = prepared();
    s.set_param("release", 0.0); // 0.5 s
    let out = feed(&mut s, 12.0, -26.0);
    assert!(all_finite(&out));
    let r = readouts(&s);
    assert!((r[0] + 26.0).abs() < 0.3, "short-term in: {r:?}");
    assert!((r[1] - 8.0).abs() < 0.3, "gain: {r:?}");
    assert!((r[2] + 18.0).abs() < 0.5, "short-term out: {r:?}");
    assert!(r[3] > -27.0 && r[3] < -17.0, "integrated out: {r:?}");
    // (the ride overshoots while the 3 s window fills, and the true peak holds its maximum)
    assert!(r[4] > -18.5 && r[4] < -14.0, "true peak: {r:?}");
    assert!(r[5].is_nan() && r[6].is_nan() && r[7].is_nan(), "{r:?}");
    let tail = &out[0][out[0].len() - 48_000..];
    assert!((rms(tail) - amp(-18.0) * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
}

#[test]
fn static_mode_holds_and_normalize_jumps_once() {
    let mut s = prepared();
    s.set_param("mode", 1.0);
    feed(&mut s, 4.0, -28.0);
    assert_eq!(s.engine().gain_db(), 0.0, "Static rides nothing");
    // the press fires once, on the rising edge
    s.set_param("normalize", 1.0);
    feed(&mut s, 0.2, -28.0);
    let g = s.engine().gain_db();
    assert!((g - 10.0).abs() < 0.3, "{g}");
    feed(&mut s, 1.0, -40.0);
    assert_eq!(s.engine().gain_db(), g, "held, it does not fire again");
    s.set_param("normalize", 0.0);
    feed(&mut s, 0.2, -40.0);
    assert_eq!(s.engine().gain_db(), g);
}

#[test]
fn learn_takes_the_loudness_and_goes_back_on_the_target_knob() {
    let mut s = prepared();
    s.set_param("mode", 1.0);
    feed(&mut s, 3.5, -25.0);
    s.set_param("learn", 1.0);
    feed(&mut s, 0.3, -25.0);
    let r = readouts(&s);
    let want = d::target_to_knob(-25.0);
    assert!((r[6] - want).abs() < 1e-6, "{r:?}");
    assert_eq!(s.param("target"), Some(r[6]), "the plugin's own Target took it (the C++ timer's setParamValue)");
    assert_eq!(s.engine().target_lufs(), -25.0);
    // the next push sends it to the engine: the same value
    feed(&mut s, 0.2, -25.0);
    assert_eq!(s.engine().target_lufs(), -25.0);
    // the host writes it back to the session: nothing left to write
    s.set_param("target", want);
    assert!(readouts(&s)[6].is_nan());
}

fn link_pair() -> (Arc<LinkSlab>, LevelUp, LevelUp) {
    let slab = Arc::new(LinkSlab::new(level_up_link::WORDS));
    slab.set_members(1, 1, 0, level_up_link::LAYOUT);
    let (mut a, mut b) = (prepared(), prepared());
    a.set_param("link", 1.0);
    b.set_param("link", 2.0);
    a.set_link(Some(Arc::clone(&slab)), 0);
    b.set_link(Some(Arc::clone(&slab)), 1);
    (slab, a, b)
}

/// One block each, IN before OUT (the host's order).
fn both(a: &mut LevelUp, b: &mut LevelUp, secs: f32, a_db: f32, b_db: f32) {
    let n = 512;
    let blocks = (secs * SR) as usize / n;
    let xa = sine(n, amp(a_db), 997.0);
    let xb = sine(n, amp(b_db), 997.0);
    for _ in 0..blocks {
        let mut io = vec![xa.clone(), xa.clone()];
        a.process(&mut io, n);
        let mut io = vec![xb.clone(), xb.clone()];
        b.process(&mut io, n);
    }
}

#[test]
fn a_linked_out_follows_its_in_not_the_knob() {
    let (_slab, mut a, mut b) = link_pair();
    b.set_param("release", 0.0);
    both(&mut a, &mut b, 10.0, -20.0, -30.0);
    let ra = readouts(&a);
    let rb = readouts(&b);
    assert!((ra[0] + 20.0).abs() < 0.3 && ra[7].is_nan(), "{ra:?}");
    assert!((rb[7] - ra[0]).abs() < 0.2, "the OUT follows its IN: {rb:?}");
    // so it rides to the IN's -20, not the knob's -18
    assert!((rb[2] + 20.0).abs() < 0.5, "{rb:?}");
    // another pair: back to the knob
    b.set_param("linkGroup", 1.0);
    both(&mut a, &mut b, 0.1, -20.0, -30.0);
    assert!(readouts(&b)[7].is_nan());
    b.set_param("linkGroup", 0.0);
    both(&mut a, &mut b, 0.1, -20.0, -30.0);
    assert!(readouts(&b)[7].is_finite());
    // the IN goes (deleted: dropped): the OUT finds no IN
    drop(a);
    let mut io = vec![sine(512, 0.1, 997.0); 2];
    b.process(&mut io, 512);
    assert!(readouts(&b)[7].is_nan());
}

#[test]
fn link_with_a_missing_partner_or_no_group_uses_the_knob() {
    // an OUT alone in its group, and one in none
    let slab = Arc::new(LinkSlab::new(level_up_link::WORDS));
    slab.set_members(0, 1, 0, level_up_link::LAYOUT);
    let mut b = prepared();
    b.set_param("link", 2.0);
    b.set_link(Some(Arc::clone(&slab)), 0);
    feed(&mut b, 1.0, -30.0);
    assert!(readouts(&b)[7].is_nan());
    b.set_link(None, 0);
    feed(&mut b, 1.0, -30.0);
    assert!(readouts(&b)[7].is_nan());
    assert_eq!(slab.load(0), Some(0), "leaving the group cleared its slot");
    // a group of mixed plugins (layout 0): unavailable, nothing read or written
    let mixed = Arc::new(LinkSlab::new(level_up_link::WORDS));
    b.set_link(Some(Arc::clone(&mixed)), 0);
    feed(&mut b, 0.5, -30.0);
    assert!((0..level_up_link::WORDS).all(|i| mixed.load(i) == Some(0)));
}

fn reference(len: usize, db: f32) -> Arc<dyn SampleSource> {
    let l: Vec<f32> = (0..len).map(|i| amp(db) * (i as f32 * 0.071).sin()).collect();
    let r: Vec<f32> = l.iter().map(|x| -x).collect();
    Arc::new(ReferenceAudio::new(SR, vec![l, r]).unwrap())
}

#[test]
fn a_reference_plays_at_the_mix_loudness_and_follows_the_song_position() {
    let refa = reference(96_000, -8.0);
    let ref_lufs = as_reference(refa.as_ref()).unwrap().lufs();
    let mut s = prepared();
    s.set_param("mode", 1.0); // the mix as it comes
    s.set_param("ceiling", 0.0);
    assert!(s.set_asset("ref1", Some(Arc::clone(&refa))));
    s.set_param("refSlot", 1.0);
    // the mix at -23 LUFS for long enough to have an integrated reading
    feed(&mut s, 3.0, -23.0);
    s.set_param("reference", 1.0);
    let mix = readouts(&s)[3];
    assert!((mix + 23.0).abs() < 0.5, "{mix}");
    // played with the transport at 150 000: the reference from 150 000 % 96 000
    let at = 150_000i64;
    let n = 512;
    let x = sine(n, amp(-23.0), 997.0);
    let mut last = Vec::new();
    for b in 0..200 {
        s.set_transport(&playing(at + (b * n) as i64));
        let mut io = vec![x.clone(), x.clone()];
        s.process(&mut io, n);
        last = io;
    }
    let r = readouts(&s);
    let want_db = (mix - ref_lufs).clamp(-24.0, 24.0);
    assert!((r[5] - want_db).abs() < 0.3, "{r:?} want {want_db}");
    // the samples are the reference's, at the song position, played at that gain
    let src = refa.channel(0).unwrap();
    let start = (at as usize + 199 * n) % 96_000;
    let g = amp(r[5]);
    for i in 0..n {
        let want = d::ref_to_float(d::ref_from_float(src[(start + i) % 96_000])) * g;
        assert!((last[0][i] - want).abs() < 2e-3, "sample {i}: {} vs {want}", last[0][i]);
    }
    // the mix is still measured underneath: its loudness has not become the reference's
    assert!((readouts(&s)[2] + 23.0).abs() < 0.5);
    // stopped: it runs on from where it was
    s.set_transport(&Transport::default());
    let mut io = vec![x.clone(), x.clone()];
    s.process(&mut io, n);
    assert!(all_finite(&io));
}

#[test]
fn a_reference_that_is_not_usable_plays_the_mix() {
    let x = sine(4800, 0.3, 997.0);
    for asset in [
        // another kind of asset
        Arc::new(crate::SampleBuffer { sample_rate: SR, channels: vec![vec![0.9; 4800]; 2] }) as Arc<dyn SampleSource>,
        // at another rate than the plugin's (the host remakes it)
        Arc::new(ReferenceAudio::new(44_100.0, vec![sine(24_000, 0.5, 440.0); 2]).unwrap()) as Arc<dyn SampleSource>,
    ] {
        let mut s = prepared();
        s.set_param("reference", 1.0);
        s.set_param("ceiling", 0.0);
        s.set_param("mode", 1.0);
        s.set_asset("ref0", Some(asset));
        let mut io = vec![x.clone(), x.clone()];
        run(&mut s, &mut io, 480);
        assert_eq!(io[0], x);
        assert!(readouts(&s)[5].is_nan());
    }
    // an empty slot selected
    let mut s = prepared();
    s.set_param("reference", 1.0);
    s.set_param("refSlot", 3.0);
    s.set_asset("ref0", Some(reference(24_000, -10.0)));
    let mut io = vec![x.clone(), x.clone()];
    run(&mut s, &mut io, 480);
    assert!(readouts(&s)[5].is_nan());
}

#[test]
fn replaced_references_go_back_to_the_host() {
    let mut s = prepared();
    assert!(!s.set_asset("irPathTwin", Some(reference(24_000, -10.0))));
    let a = reference(24_000, -10.0);
    let b = reference(24_000, -12.0);
    assert!(s.set_asset("ref2", Some(Arc::clone(&a))));
    assert!(s.take_released_asset().is_none());
    s.set_asset("ref2", Some(Arc::clone(&b)));
    let back = s.take_released_asset().unwrap();
    assert!(std::ptr::addr_eq(Arc::as_ptr(&back), Arc::as_ptr(&a)));
    s.set_asset("ref2", None);
    assert!(s.take_released_asset().is_some());
    assert!(s.take_released_asset().is_none());
}

#[test]
fn preview_plays_the_platform_gain_and_dither_lands_on_the_grid() {
    // a loud master (-10 LUFS) previewed on Spotify: turned down 4 dB
    let mut s = prepared();
    s.set_param("mode", 1.0);
    s.set_param("ceiling", 0.0);
    s.set_param("platform", 1.0);
    s.set_param("preview", 1.0);
    let out = feed(&mut s, 4.0, -10.0);
    let r = readouts(&s);
    let g = d::platform_gain_db(1, r[3], r[4]);
    assert!((g + 4.0).abs() < 0.3, "{g} {r:?}");
    // the editor's "Previewing" line reads the engine's preview gain
    let live = s.analysis().unwrap().live.get(LIVE_PREVIEW_GAIN);
    assert_eq!(live, s.dsp.preview_gain_out);
    assert!(live < -3.0, "{live}");
    let tail = &out[0][out[0].len() - 4800..];
    assert!((rms(tail) / (amp(-10.0) * std::f32::consts::FRAC_1_SQRT_2) - amp(g)).abs() < 0.02);
    // 16-bit dither: on the grid, about half an LSB rms of noise
    let mut s = prepared();
    s.set_param("mode", 1.0);
    s.set_param("ceiling", 0.0);
    s.set_param("dither", 2.0);
    let x = sine(48_000, 0.25, 997.0);
    let mut io = vec![x.clone(), x.clone()];
    run(&mut s, &mut io, 512);
    let lsb = 1.0 / 32768.0;
    assert!(io[0].iter().all(|v| ((v / lsb).round() * lsb - v).abs() < 1e-9));
    let noise: Vec<f32> = io[0].iter().zip(&x).map(|(a, b)| a - b).collect();
    let n = rms(&noise) / lsb;
    assert!(n > 0.3 && n < 0.8, "{n}");
    // shaped too
    s.set_param("shaping", 1.0);
    let mut io = vec![x.clone(), x.clone()];
    run(&mut s, &mut io, 512);
    assert!(all_finite(&io) && io[0].iter().all(|v| ((v / lsb).round() * lsb - v).abs() < 1e-9));
}

#[test]
fn every_factory_preset_parses() {
    for (name, json) in PRESETS {
        let p = FsPreset::parse(json, &LEVELUP_INFO).unwrap();
        assert_eq!(&p.name, name);
        assert!(p.values.len() >= 12, "{name}: {:?}", p.values);
    }
    let out = FsPreset::parse(PRESETS[1].1, &LEVELUP_INFO).unwrap();
    assert_eq!(out.values["link"], 2.0);
    let cd = FsPreset::parse(PRESETS[4].1, &LEVELUP_INFO).unwrap();
    assert_eq!((cd.values["dither"], cd.values["shaping"], cd.values["mode"]), (2.0, 1.0, 1.0));
    assert!(PRESETS.iter().all(|(_, j)| PRESET_SKIPS.iter().all(|k| !FsPreset::parse(j, &LEVELUP_INFO).unwrap().values.contains_key(*k))));
}

#[test]
fn hostile_params_stay_finite() {
    for id in LEVELUP_PARAMS.iter().map(|p| p.id).filter(|id| *id != "bypass") {
        for v in HOSTILE {
            let mut s = prepared();
            s.set_param(id, v);
            s.set_asset("ref0", Some(reference(24_000, -10.0)));
            s.set_transport(&playing(i64::MAX - 10));
            let out = feed(&mut s, 0.25, -6.0);
            assert!(all_finite(&out), "{id} = {v}");
            assert!(readouts(&s).iter().all(|r| r.is_finite() || r.is_nan()));
        }
    }
}

#[test]
fn hostile_choice_params_stay_finite() {
    let mut s = prepared();
    s.set_asset("ref0", Some(reference(24_000, -10.0)));
    for (k, v) in HOSTILE.iter().enumerate() {
        for id in CHOICES {
            s.set_param(id, *v);
        }
        s.set_transport(&playing(if k % 2 == 0 { -5 } else { 1 << 40 }));
        let out = feed(&mut s, 0.1, 0.0);
        assert!(all_finite(&out), "{v}");
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for v in HOSTILE {
        let mut s = prepared();
        s.set_param("bypass", v);
        let out = feed(&mut s, 0.1, -6.0);
        assert!(all_finite(&out), "{v}");
    }
}

#[test]
fn bypass_passes_the_input_bit_exactly() {
    let mut s = prepared();
    s.set_param("bypass", 1.0);
    s.set_param("dither", 2.0);
    let x = sine(9600, 0.9, 997.0);
    let mut io = vec![x.clone(), x.clone()];
    run(&mut s, &mut io, 256);
    assert_eq!(io, vec![x.clone(), x]);
}

#[test]
fn switching_quality_and_model_mid_stream_is_safe() {
    let mut s = prepared();
    s.set_asset("ref0", Some(reference(24_000, -10.0)));
    let x = sine(256, 0.8, 997.0);
    for blk in 0..600 {
        s.set_param("mode", (blk % 2) as f32);
        s.set_param("reference", ((blk / 3) % 2) as f32);
        s.set_param("dither", (blk % 3) as f32);
        s.set_param("platform", (blk % 7) as f32);
        s.set_param("preview", ((blk / 5) % 2) as f32);
        s.set_param("link", (blk % 3) as f32);
        s.set_transport(&playing(blk * 256));
        let mut io = vec![x.clone(), x.clone()];
        s.process(&mut io, 256);
        assert!(all_finite(&io));
    }
}

#[test]
fn mono_and_wide_buffers() {
    // mono: the left read as both sides, only it written
    let mut s = LevelUp::new();
    s.prepare(SR, 512, 1);
    let x = sine(4 * 48_000, amp(-30.0), 997.0);
    let mut io = vec![x.clone()];
    run(&mut s, &mut io, 512);
    assert!(all_finite(&io));
    // a mono -30 dBFS sine read as both sides measures as the stereo one does
    assert!((readouts(&s)[0] + 30.0).abs() < 0.3, "{:?}", readouts(&s));
    // six channels: the first two processed, the rest untouched
    let mut s = LevelUp::new();
    s.prepare(SR, 512, 6);
    s.set_param("dither", 2.0);
    let mut io = vec![x.clone(); 6];
    run(&mut s, &mut io, 512);
    assert!(all_finite(&io));
    assert_ne!(io[0], x);
    for c in &io[2..] {
        assert_eq!(c, &x);
    }
    // no channels, zero frames
    let mut empty: Vec<Vec<f32>> = Vec::new();
    s.process(&mut empty, 512);
    let mut io = vec![x.clone(); 2];
    s.process(&mut io, 0);
    // a frame count past the buffers
    let mut short = vec![vec![0.1; 16]; 2];
    s.process(&mut short, 4096);
    assert!(all_finite(&short));
}

#[test]
fn silence_tail_is_cheap() {
    let mut s = prepared();
    let t0 = Instant::now();
    feed(&mut s, 12.0, -10.0);
    let loud = t0.elapsed();
    let mut s = prepared();
    feed(&mut s, 2.0, -10.0);
    let t1 = Instant::now();
    let mut io = vec![vec![0.0; 480_000]; 2];
    run(&mut s, &mut io, 512);
    let quiet = t1.elapsed();
    assert!(quiet < loud * 4, "{quiet:?} vs {loud:?}");
    assert!(all_finite(&io));
}

#[test]
fn process_does_not_allocate() {
    // No counting allocator in this crate (`unsafe_code` is forbidden): the engine is plain data
    // (no `Vec`) in the one box made in `new`, the references and the slab are the host's, so
    // the box and the host buffers keeping their addresses across 1000 blocks is the whole story.
    let (_slab, mut a, mut b) = link_pair();
    b.set_asset("ref0", Some(reference(24_000, -10.0)));
    let engine_ptr: *const LevelUpDsp = b.engine();
    let src = sine(256, 0.7, 997.0);
    let mut io = vec![src.clone(), src.clone()];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        for c in &mut io {
            c.copy_from_slice(&src);
        }
        a.process(&mut io, 256);
        b.set_param("reference", ((blk / 7) % 2) as f32);
        b.set_param("dither", (blk % 3) as f32);
        b.set_param("learn", ((blk / 50) % 2) as f32);
        b.set_transport(&playing(blk * 256));
        b.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, b.engine() as *const LevelUpDsp);
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread: the engine is prepared in its own box, never
    // re-made. `LevelUpDsp` has no heap fields, so the box pointer is the whole check.
    let mut s = prepared();
    feed(&mut s, 0.1, -10.0);
    let engine_ptr: *const LevelUpDsp = s.engine();
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, s.engine() as *const LevelUpDsp);
        feed(&mut s, 0.05, -10.0);
    }
    s.prepare(96_000.0, 1024, 2);
    assert_eq!(engine_ptr, s.engine() as *const LevelUpDsp);
}

/// A LevelUp with non-default settings and a history of its own (`secs` of `db`).
fn warmed(secs: f32, db: f32) -> LevelUp {
    let mut s = prepared();
    s.set_param("target", 0.6);
    s.set_param("release", 0.1);
    s.set_param("platform", 2.0);
    s.set_param("preview", 1.0);
    s.set_param("dither", 2.0);
    s.set_param("shaping", 1.0);
    s.set_param("reference", 1.0);
    s.set_asset("ref0", Some(reference(48_000, -9.0)));
    s.set_transport(&playing(12_345));
    feed(&mut s, secs, db);
    s
}

fn db(x: f32) -> f32 {
    20.0 * x.log10()
}

#[test]
fn reset_twice_from_the_same_state_renders_identically() {
    // two instances with the same settings and the same history: after a reset they render bit
    // for bit the same over 3 s (more than 4096 samples), readouts included
    let mut a = warmed(2.37, -12.0);
    let mut b = warmed(2.37, -12.0);
    a.reset();
    b.reset();
    a.set_transport(&playing(777));
    b.set_transport(&playing(777));
    let oa = feed(&mut a, 3.0, -20.0);
    let ob = feed(&mut b, 3.0, -20.0);
    assert_eq!(oa, ob);
    assert_eq!(readouts(&a).map(f32::to_bits), readouts(&b).map(f32::to_bits));
    // and again
    a.reset();
    b.reset();
    assert_eq!(feed(&mut a, 0.2, -20.0), feed(&mut b, 0.2, -20.0));
}

/// The reference segment `[at, at + n)` (wrapping) of slot 0, as played (16-bit).
fn ref_rms(s: &LevelUp, at: usize, n: usize) -> f32 {
    let src = s.refs[0].as_ref().unwrap().channel(0).unwrap();
    let seg: Vec<f32> = (0..n).map(|i| d::ref_to_float(d::ref_from_float(src[(at + i) % src.len()]))).collect();
    rms(&seg)
}

#[test]
fn reset_keeps_the_parameters_and_the_ride_and_restarts_the_delivery_meter() {
    let mut s = warmed(4.0, -14.0);
    s.set_param("dither", 0.0);
    s.set_param("ceiling", 0.0);
    // the reference as heard before the reset (its gain to the mix, times Apple Music's preview)
    let n = 4096;
    s.set_transport(&playing(0));
    let mut io = vec![vec![0.3; n]; 2];
    s.process(&mut io, n);
    let before_gain = rms(&io[0]) / ref_rms(&s, 0, n);
    let r0 = readouts(&s);
    assert!(r0[3] > -69.0 && r0[1] != 0.0, "{r0:?}");
    assert!(db(before_gain).abs() > 3.0, "not unity: {}", db(before_gain));
    let params: Vec<f32> = LEVELUP_PARAMS.iter().map(|p| s.param(p.id).unwrap()).collect();
    s.reset();
    assert_eq!(params, LEVELUP_PARAMS.iter().map(|p| s.param(p.id).unwrap()).collect::<Vec<_>>());
    // the ride and the short-term loudness are kept; integrated loudness and true peak restart
    // (the C++ `prepareToPlay`'s `resetDeliveryNow`)
    let r = readouts(&s);
    assert_eq!((r[0], r[1], r[2], r[3], r[4]), (r0[0], r0[1], r0[2], -100.0, -120.0));
    // The reference does not jump to unity: its smoother and Preview's gain carry on from where
    // they were (the first 64 samples sit at the level before the reset)...
    s.set_transport(&playing(10_000));
    let mut io = vec![vec![0.3; n]; 2];
    s.process(&mut io, n);
    let head = rms(&io[0][..64]) / ref_rms(&s, 10_000, 64);
    assert!((db(head) - db(before_gain)).abs() < 0.2, "{} vs {}", db(head), db(before_gain));
    // ...and with the integrated reading restarted, it is matched to the kept short-term loudness
    // until there is one again (the C++'s own fallback), reached through the smoother
    let lufs = as_reference(s.refs[0].as_deref().unwrap()).unwrap().lufs();
    let want = (r0[2] - lufs).clamp(-24.0, 24.0);
    assert!((readouts(&s)[5] - want).abs() < 0.3, "{:?} want {want}", readouts(&s));
}

#[test]
fn preview_holds_its_gain_across_a_reset_until_the_meter_measures_again() {
    let mut s = prepared();
    s.set_param("mode", 1.0);
    s.set_param("ceiling", 0.0);
    s.set_param("platform", 1.0);
    s.set_param("preview", 1.0);
    feed(&mut s, 4.0, -10.0);
    let g = s.engine().preview_gain_out;
    assert!((g + 4.0).abs() < 0.3, "{g}");
    s.reset();
    // the first 4096 samples are still previewed at -4 dB (no drop to 0 dB and glide back)
    let out = feed(&mut s, 4096.0 / SR, -10.0);
    let want = amp(-10.0) * std::f32::consts::FRAC_1_SQRT_2 * amp(g);
    assert!((db(rms(&out[0])) - db(want)).abs() < 0.1, "{} vs {}", db(rms(&out[0])), db(want));
    // and once it has measured again, the platform's gain as measured
    feed(&mut s, 1.0, -10.0);
    assert!((s.engine().preview_gain_out - g).abs() < 0.3);
}

#[test]
fn a_reset_keeps_the_ride_and_makes_no_level_step() {
    // steady program, the leveler riding about +8 dB: one instance is reset, its twin is not, and
    // the next 4096 samples come out the same (no step, no re-ride from 0 dB)
    let mut a = prepared();
    let mut b = prepared();
    for s in [&mut a, &mut b] {
        s.set_param("release", 0.0);
        feed(s, 8.0, -26.0);
    }
    let x = sine(4096, amp(-26.0), 997.0);
    let mut last = vec![x.clone(), x.clone()];
    a.process(&mut last, 4096);
    let mut skip = vec![x.clone(), x.clone()];
    b.process(&mut skip, 4096);
    a.reset();
    let mut oa = vec![x.clone(), x.clone()];
    let mut ob = vec![x.clone(), x.clone()];
    a.process(&mut oa, 4096);
    b.process(&mut ob, 4096);
    let peak = ob[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let diff = oa[0].iter().zip(&ob[0]).fold(0.0f32, |m, (p, q)| m.max((p - q).abs()));
    assert!(diff < 1e-3 * peak, "{diff} vs peak {peak}");
    assert!((db(rms(&oa[0])) - db(rms(&last[0]))).abs() < 0.05, "a level step after the reset");
    assert!(readouts(&a)[1] > 7.0, "{:?}", readouts(&a));
}

#[test]
fn the_reset_delivery_action_restarts_the_meter_at_the_next_block() {
    let mut s = prepared();
    feed(&mut s, 2.0, -12.0);
    let r = readouts(&s);
    assert!(r[3] > -69.0 && r[4] > -20.0);
    assert!(!s.action("nonsense"));
    assert!(s.action("reset_delivery"));
    let mut io = vec![vec![0.0; 64]; 2];
    s.process(&mut io, 64);
    let r = readouts(&s);
    assert_eq!((r[3], r[4]), (-100.0, -120.0));
    // the leveler is untouched
    assert!(r[0] > -20.0);
    assert_eq!(crate::plugin_actions("fs_levelup"), ["reset_delivery"]);
    assert!(crate::plugin_actions("fs_comp").is_empty());
}

#[test]
fn params_are_pushed_only_when_they_move() {
    // Learn and Normalize fire on their rising edge: a value pushed every block would not matter,
    // but a forced push after a reset must not fire a held press again.
    let mut s = prepared();
    s.set_param("mode", 1.0);
    feed(&mut s, 3.5, -30.0);
    s.set_param("normalize", 1.0);
    feed(&mut s, 0.2, -30.0);
    let g = s.engine().gain_db();
    assert!(g > 5.0);
    s.reset();
    feed(&mut s, 3.5, -40.0);
    assert_eq!(s.engine().gain_db(), g, "the ride is kept and the held press did not fire again");
}

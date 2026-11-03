use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

const SR: f32 = 48_000.0;

fn sine(len: usize, hz: f32, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / SR).sin()).collect()
}

/// A snare-ish hit every `period` samples (a 200 Hz body decaying in ~60 ms) over a steady hi-hat
/// wash at 7 kHz (the bleed): the snare-top detector hears the hits and not the hats.
fn kit(len: usize, period: usize, hit: f32, hat: f32) -> Vec<f32> {
    (0..len)
        .map(|i| {
            let t = (i % period) as f32 / SR;
            let x = i as f32 / SR;
            hit * (-t / 0.06).exp() * (std::f32::consts::TAU * 200.0 * t).sin() + hat * (std::f32::consts::TAU * 7000.0 * x).sin()
        })
        .collect()
}

fn prepared() -> Shale {
    let mut s = Shale::new();
    s.prepare(SR, 512, 2);
    s
}

fn run_keyed(s: &mut Shale, io: &mut [Vec<f32>], key: Option<&[Vec<f32>]>, block: usize) {
    let len = io.iter().map(Vec::len).min().unwrap_or(0);
    let mut buf: Vec<Vec<f32>> = io.iter().map(|_| vec![0.0; block]).collect();
    let mut kbuf: Vec<Vec<f32>> = key.map_or(Vec::new(), |k| k.iter().map(|_| vec![0.0; block]).collect());
    let mut pos = 0;
    while pos < len {
        let n = block.min(len - pos);
        for (b, c) in buf.iter_mut().zip(io.iter()) {
            b[..n].copy_from_slice(&c[pos..pos + n]);
        }
        if let Some(k) = key {
            for (b, c) in kbuf.iter_mut().zip(k.iter()) {
                b[..n].copy_from_slice(&c[pos..pos + n]);
            }
            s.process_with_sidechain(&mut buf, n, Some(&kbuf));
        } else {
            s.process(&mut buf, n);
        }
        for (b, c) in buf.iter().zip(io.iter_mut()) {
            c[pos..pos + n].copy_from_slice(&b[..n]);
        }
        pos += n;
    }
}

fn run(s: &mut Shale, io: &mut [Vec<f32>], block: usize) {
    run_keyed(s, io, None, block);
}

fn all_finite(io: &[Vec<f32>]) -> bool {
    io.iter().flatten().all(|v| v.is_finite())
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn readouts(s: &Shale) -> [f32; 8] {
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    r
}

const HOSTILE: [f32; 9] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0];
const CHOICES: [&str; 18] = [
    "instrument",
    "ghost",
    "closeMode",
    "lookahead",
    "debleedMode",
    "autoBand",
    "scSource",
    "scListen",
    "monitor",
    "link",
    "linkGroup",
    "linkRole",
    "midiOut",
    "phaseRef",
    "align",
    "alignFlip",
    "alignTo",
    "bypass",
];

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    // `getParamDefs`: id, name, kind, default, choices.
    let want: [(&str, &str, f32, usize); NUM_PARAMS] = [
        ("bypass", "Bypass", 0.0, 0),
        ("instrument", "Mic", 3.0, 11),
        ("threshold", "Threshold", 0.55, 0),
        ("sens", "Onset", 0.5, 0),
        ("ghost", "Ghost Notes", 0.0, 0),
        ("reduction", "Reduction", 1.0, 0),
        ("length", "Length", 0.4, 0),
        ("closeMode", "Close", 0.0, 2),
        ("lookahead", "Lookahead", 2.0, 5),
        ("hold", "Hold", 0.35, 0),
        ("retrig", "Retrigger", 0.2, 0),
        ("debleed", "DeBleed", 0.0, 0),
        ("debleedMode", "DeBleed Mode", 0.0, 2),
        ("xover", "Crossover", 0.5, 0),
        ("velTrack", "Vel Track", 0.5, 0),
        ("autoBand", "Auto Band", 1.0, 0),
        ("detLow", "Det Low", 0.4, 0),
        ("detHigh", "Det High", 0.6, 0),
        ("scSource", "Trigger From", 0.0, 2),
        ("scListen", "SC Listen", 0.0, 0),
        ("monitor", "Monitor", 0.0, 3),
        ("mix", "Mix", 1.0, 0),
        ("trim", "Input", 0.3333, 0),
        ("output", "Output", 0.5, 0),
        ("link", "Kit Link", 0.0, 0),
        ("linkGroup", "Link Group", 0.0, 4),
        ("linkRole", "Link Role", 0.0, 3),
        ("reject", "Reject", 0.5, 0),
        ("midiOut", "MIDI Out", 0.0, 0),
        ("midiNote", "MIDI Note", 0.1667, 0),
        ("phaseRef", "Phase Reference", 0.0, 0),
        ("align", "Align", 0.0, 0),
        ("alignOffset", "Align Offset", 0.5, 0),
        ("alignFlip", "Align Polarity", 0.0, 0),
        ("alignTo", "Align To", 0.0, 12),
    ];
    for (i, (p, (id, name, def, choices))) in SHALE_PARAMS.iter().zip(want).enumerate() {
        assert_eq!((p.id, p.name, p.default, p.choices.len()), (id, name, def, choices), "param {i}");
        assert!(p.min == 0.0 && p.max >= 1.0 || choices > 0, "{id}");
    }
    assert_eq!(SHALE_PARAMS[d::P_INSTRUMENT].choices[d::I_SNARE_BTM as usize], "Snare Btm");
    assert_eq!(SHALE_PARAMS[d::P_ALIGN_TO].choices[11], "Room");
    assert_eq!(SHALE_PARAMS[d::P_BYPASS].group, "", "the engine's bypass is hidden");
    for (i, m) in d::MODES.iter().enumerate() {
        assert_eq!(m.name, SHALE_PARAMS[d::P_INSTRUMENT].choices[i]);
    }
    assert!(SHALE_INFO.sidechain);
    assert!(INERT_PARAMS.iter().all(|id| SHALE_INFO.param(id).is_some()));
}

#[test]
fn engine_maps_match_the_header() {
    assert_eq!(d::knob_to_thresh_db(0.0), -66.0);
    assert_eq!(d::knob_to_flux_db(1.0), 1.5);
    assert_eq!(d::knob_to_red_db(1.0), -60.0);
    assert!(d::is_full_mute(0.995) && !d::is_full_mute(0.994));
    assert!((d::knob_to_fixed_ms(1.0) - 360.0).abs() < 1e-3);
    assert!((d::knob_to_xover_hz(1.0) - 15000.0).abs() < 0.1);
    assert!((d::knob_to_det_hz(1.0) - 12000.0).abs() < 0.1);
    assert_eq!(d::knob_to_midi_note(0.1667), 38);
    assert_eq!(d::lookahead_ms(9), 5.0);
    assert_eq!(d::lookahead_ms(-3), 0.0);
    // Spot checks of `mode()`'s table against the header, and its clamped index.
    let k = d::mode(d::I_KICK_OUT);
    assert_eq!((k.det_low_hz, k.det_high_hz, k.xover_hz, k.min_hold_ms, k.retrig_ms, k.coinc_ms), (28.0, 90.0, 220.0, 55.0, 36.0, 26.0));
    assert_eq!((k.follow_min_db, k.follow_max_db, k.flux_bias_db), (8.0, 36.0, -2.5));
    let r = d::mode(99);
    assert_eq!((r.name, r.det_low_hz, r.follow_max_db, r.flux_bias_db), ("Room", 60.0, 50.0, -3.0));
    assert_eq!(d::mode(-1).name, "Kick In");
    assert_eq!(d::follow_db(d::I_SNARE_TOP, 2.0), 45.0);
}

#[test]
fn latency_follows_lookahead_and_align() {
    let mut s = prepared();
    // 2 ms at 48 kHz.
    assert_eq!(s.latency(), 96);
    let mut io = vec![vec![0.0f32; 256], vec![0.0f32; 256]];
    for (choice, want) in [(0.0, 0), (1.0, 48), (4.0, 240), (3.0, 144)] {
        s.set_param("lookahead", choice);
        s.process(&mut io, 256);
        assert_eq!(s.latency(), want, "lookahead {choice}");
    }
    // Align on adds its 10 ms base (+4), off takes it away again.
    s.set_param("align", 1.0);
    s.process(&mut io, 256);
    assert_eq!(s.latency(), 144 + 484);
    s.set_param("align", 0.0);
    s.process(&mut io, 256);
    assert_eq!(s.latency(), 144);
    // Set before prepare, it is reported straight after prepare (before any block).
    let mut s = Shale::new();
    s.set_param("lookahead", 4.0);
    s.set_param("align", 1.0);
    s.prepare(SR, 512, 2);
    assert_eq!(s.latency(), 240 + 484);
    assert!(s.tail_samples() >= s.latency());
}

#[test]
fn the_default_lookahead_stays_96_samples_at_every_rate() {
    // C++ quirk kept: the lookahead in samples is worked out only when the choice CHANGES.
    for (sr, moved) in [(44_100.0, 132), (96_000.0, 288)] {
        let mut s = Shale::new();
        s.prepare(sr, 512, 2);
        let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
        s.process(&mut io, 64);
        assert_eq!(s.latency(), 96, "{sr}");
        s.set_param("lookahead", 3.0);
        s.process(&mut io, 64);
        assert_eq!(s.latency(), moved, "{sr}");
    }
    // A host preparing twice before any audio is still the C++'s one prepare at the last rate.
    let mut s = Shale::new();
    s.set_param("lookahead", 4.0);
    s.prepare(48_000.0, 512, 2);
    s.prepare(96_000.0, 512, 2);
    assert_eq!(s.latency(), 480);
}

#[test]
fn the_gate_opens_on_the_hits_and_shuts_out_the_hats() {
    let mut s = prepared();
    let src = kit(96_000, 24_000, 0.8, 0.05);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    assert!(all_finite(&io));
    let r = readouts(&s);
    // Four hits in a second, each counted once.
    assert_eq!(r[4], 4.0, "triggers");
    // Each hit passes (delayed by the 96-sample lookahead) ...
    let hit = &io[0][96..96 + 2400];
    assert!(rms(hit) > 0.5 * rms(&src[..2400]), "{} vs {}", rms(hit), rms(&src[..2400]));
    // ... and the hats between them are muted (Reduction is a full mute by default).
    let gap = &io[0][21_000..23_500];
    assert!(rms(gap) < 1e-6, "between hits: {}", rms(gap));
    // Hats alone never open it.
    let mut s = prepared();
    let mut io = vec![kit(24_000, 12_000, 0.0, 0.3), kit(24_000, 12_000, 0.0, 0.3)];
    run(&mut s, &mut io, 512);
    assert_eq!(readouts(&s)[4], 0.0);
    assert!(rms(&io[0][4800..]) < 1e-6);
}

#[test]
fn the_sidechain_keys_the_gate_as_the_cpp_bus_does() {
    let steady = sine(96_000, 300.0, 0.5);
    let hits = kit(96_000, 24_000, 0.8, 0.0);
    let count = |sc_source: f32, key: Option<&[Vec<f32>]>| {
        let mut s = prepared();
        s.set_param("scSource", sc_source);
        let mut io = vec![steady.clone(), steady.clone()];
        run_keyed(&mut s, &mut io, key, 512);
        assert!(all_finite(&io));
        (readouts(&s)[4], rms(&io[0][21_000..23_500]), rms(&io[0][96..2496]))
    };
    let key = vec![hits.clone(), hits.clone()];
    // Trigger From Sidechain with a key: the gate opens on the key's hits, shut between them.
    let (n, gap, open) = count(1.0, Some(&key));
    assert_eq!(n, 4.0);
    assert!(gap < 1e-6 && open > 0.2, "gap {gap}, open {open}");
    // A mono key keys a stereo module the same way.
    assert_eq!(count(1.0, Some(&key[..1])).0, 4.0);
    // Trigger From Internal ignores the key: a steady tone opens it once and holds it.
    assert_eq!(count(0.0, Some(&key)).0, 1.0);
    // No source chosen (the C++ with no sidechain bus): keys from the track whatever the switch.
    assert_eq!(count(1.0, None).0, 1.0);
    // A source that is muted, deleted or would loop gives a silent key: never opens.
    let silent = vec![vec![0.0f32; 96_000], vec![0.0f32; 96_000]];
    let (n, _, open) = count(1.0, Some(&silent));
    assert_eq!(n, 0.0);
    assert!(open < 1e-6);
}

#[test]
fn bypass_delays_by_the_latency_and_monitor_modes_work() {
    let src = kit(9600, 4800, 0.7, 0.1);
    let mut s = prepared();
    s.set_param("bypass", 1.0);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    assert_eq!(&io[0][96..], &src[..src.len() - 96], "the engine bypass keeps the lookahead delay");
    assert!(io[0][..96].iter().all(|v| *v == 0.0));
    // Bleed is exactly what the gate removes: output + bleed = the delayed input.
    let mut out = prepared();
    let mut bleed = prepared();
    bleed.set_param("monitor", 1.0);
    let mut a = vec![src.clone(), src.clone()];
    let mut b = vec![src.clone(), src.clone()];
    run(&mut out, &mut a, 512);
    run(&mut bleed, &mut b, 512);
    let trim = d::db_to_lin(d::knob_to_trim_db(0.3333));
    for i in 2000..9600 {
        // (The trim's smoothing, as the C++'s float steps, settles within a few ulps of its target.)
        assert!(
            (a[0][i] + b[0][i] - src[i - 96] * trim).abs() < 1e-4 * src[i - 96].abs().max(1e-3),
            "{i}: {} + {} vs {}",
            a[0][i],
            b[0][i],
            src[i - 96] * trim
        );
    }
    // Detector: the band-passed key, not the drum.
    let mut det = prepared();
    det.set_param("monitor", 2.0);
    let mut c = vec![src.clone(), src.clone()];
    run(&mut det, &mut c, 512);
    assert!(all_finite(&c) && c[0] != a[0]);
}

#[test]
fn readouts_report_the_gate_detector_and_hits() {
    let mut s = prepared();
    s.set_param("debleed", 0.6);
    let r0 = readouts(&s);
    assert_eq!(r0[4], 0.0);
    let mut io = vec![kit(6000, 12_000, 0.8, 0.1), kit(6000, 12_000, 0.8, 0.1)];
    run(&mut s, &mut io, 512);
    let r = readouts(&s);
    assert!(r[0] > 0.99, "open gate: {}", r[0]);
    assert!(r[1] > 0.01, "detector {}", r[1]);
    assert!(r[2].is_finite());
    assert!(r[3] >= 0.0);
    assert_eq!(r[4], 1.0);
    assert_eq!(r[5], 2.0, "open");
    assert!(r[6] > 0.0 && r[6] <= 1.0);
    assert_eq!(r[7], 0.0);
    assert_eq!(s.gain_reduction_db(), 0.0);
    // The scope history reaches the analysis ring: [peak, gain, det] per 4 ms bin.
    let a = s.analysis().unwrap();
    assert_eq!(a.post.written(), 6000 / 192 * d::SCOPE_BIN_VALUES);
    assert_eq!(a.pre.written(), 0);
    let mut last = [0.0f32; 3];
    a.post.latest(&mut last);
    assert_eq!(last, {
        let b = s.engine().scope[(s.engine().scope_write + d::SCOPE_BINS - 1) % d::SCOPE_BINS];
        [b.peak, b.gain, b.det]
    });
    // Closed, a full mute: 90 dB of gain reduction.
    let mut io = vec![vec![0.0f32; 24_000], vec![0.0f32; 24_000]];
    run(&mut s, &mut io, 512);
    assert_eq!(s.gain_reduction_db(), 90.0);
}

#[test]
fn params_are_pushed_only_when_they_move() {
    // Re-pushing the Mic every block would clear the learned DeBleed profile and never learn.
    let mut s = prepared();
    s.set_param("debleed", 1.0);
    s.set_param("debleedMode", 1.0);
    s.set_param("instrument", 3.0);
    let mut io = vec![kit(96_000, 24_000, 0.8, 0.2), kit(96_000, 24_000, 0.8, 0.2)];
    run(&mut s, &mut io, 512);
    let learned = s.engine().profile();
    assert!(learned.iter().any(|p| *p > 0.0), "{learned:?}");
    s.set_param("instrument", 3.0);
    // Zero frames: the block's push runs, the engine processes nothing.
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    s.process(&mut io, 0);
    assert_eq!(s.engine().profile(), learned, "same Mic: profile kept");
    s.set_param("instrument", 5.0);
    s.process(&mut io, 0);
    assert_eq!(s.engine().profile(), [0.0; d::NUM_BANDS], "a new Mic clears it");
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_shale");
    assert_eq!(presets.len(), 23);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &SHALE_INFO).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(&p.name, name);
        assert!(p.values.contains_key("instrument"), "{name}");
        assert!(!p.values.contains_key("bypass"));
        let mut s = prepared();
        for (k, v) in &p.values {
            assert!(s.set_param(k, *v), "{name}: {k}");
        }
        let mut io = vec![kit(4800, 2400, 0.8, 0.1), kit(4800, 2400, 0.8, 0.1)];
        run(&mut s, &mut io, 512);
        assert!(all_finite(&io), "{name}");
    }
    let p = FsPreset::parse(presets.iter().find(|(n, _)| *n == "Room - Gated Eighties").unwrap().1, &SHALE_INFO).unwrap();
    assert_eq!(p.values["instrument"], d::I_ROOM as f32);
}

#[test]
fn hostile_params_stay_finite() {
    // Bypass stays off (it would hide the wet path); the choices are set per case.
    for bad in HOSTILE {
        for (instrument, mode) in [(0.0, 0.0), (3.0, 1.0), (7.0, 0.0), (10.0, 1.0)] {
            let mut s = prepared();
            s.set_param("instrument", instrument);
            s.set_param("debleedMode", mode);
            s.set_param("align", 1.0);
            for info in SHALE_PARAMS.iter().filter(|p| !CHOICES.contains(&p.id)) {
                assert!(s.set_param(info.id, bad));
            }
            assert_eq!(s.param("bypass"), Some(0.0));
            let mut io = vec![kit(9600, 2400, 0.9, 0.3), sine(9600, 150.0, -0.9)];
            let key = vec![kit(9600, 1200, 0.9, 0.0)];
            run_keyed(&mut s, &mut io, Some(&key), 256);
            assert!(all_finite(&io), "instrument {instrument}, value {bad}");
            assert!(io.iter().flatten().all(|v| v.abs() < 100.0));
        }
    }
}

#[test]
fn hostile_choice_params_stay_finite() {
    for bad in HOSTILE {
        let mut s = prepared();
        s.set_param("debleed", 1.0);
        for id in CHOICES.iter().filter(|id| **id != "bypass") {
            assert!(s.set_param(id, bad));
        }
        assert_eq!(s.param("bypass"), Some(0.0));
        let mut io = vec![kit(9600, 2400, 0.9, 0.3), kit(9600, 2400, 0.9, 0.3)];
        let key = vec![kit(9600, 1200, 0.9, 0.0), sine(9600, 90.0, 0.4)];
        run_keyed(&mut s, &mut io, Some(&key), 256);
        assert!(all_finite(&io), "value {bad}");
        assert!(s.latency() < 2048);
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
        let mut s = prepared();
        s.set_param("align", 1.0);
        s.set_param("alignFlip", 1.0);
        let src = [kit(4096, 2048, 0.9, 0.2), sine(4096, 300.0, -0.9)];
        let mut a = vec![src[0][..2048].to_vec(), src[1][..2048].to_vec()];
        run(&mut s, &mut a, 256);
        assert!(s.set_param("bypass", bad));
        let mut b = vec![src[0][2048..].to_vec(), src[1][2048..].to_vec()];
        run(&mut s, &mut b, 256);
        assert!(all_finite(&a) && all_finite(&b), "bypass {bad}");
    }
}

// ---------------------------------------------------------------- Kit Link

/// A slab as the host makes one for a group of Shales.
fn slab() -> Arc<LinkSlab> {
    let s = Arc::new(LinkSlab::new(shale_link::WORDS));
    s.set_members(0, 0, 2, shale_link::LAYOUT);
    s
}

/// Another instance of the group, publishing by hand (Link Group A).
fn other(s: &Arc<LinkSlab>, member: usize, role: i32, instrument: i32) -> KitLink {
    let mut k = KitLink::default();
    k.set(Some(Arc::clone(s)), member);
    k.publish_presence(0, role, instrument, 0);
    k
}

/// A Shale in the group as member `member`, Kit Link on, in `role` (0 Lead, 1 Follow, 2 Pair).
fn linked(s: &Arc<LinkSlab>, member: usize, role: f32, instrument: f32) -> Shale {
    let mut x = prepared();
    x.set_param("link", 1.0);
    x.set_param("linkRole", role);
    x.set_param("instrument", instrument);
    x.set_link(Some(Arc::clone(s)), member);
    x
}

fn trigger_at(pos: i64, velocity: f32) -> LinkEvent {
    LinkEvent { pos, velocity, kind: EV_TRIGGER, clock: CLK_TRANSPORT, ..LinkEvent::default() }
}

fn beacon_at(pos: i64, level_db: f32) -> LinkEvent {
    LinkEvent { pos, level_db, kind: EV_BEACON, clock: CLK_TRANSPORT, ..LinkEvent::default() }
}

/// Runs `blocks` of `block` samples of `src` (both channels) through `x` from timeline position
/// `at`, returning the output (left).
fn play(x: &mut Shale, src: &[f32], at: i64, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(src.len());
    for (b, chunk) in src.chunks(block).enumerate() {
        x.set_transport(&Transport { samples: at + (b * block) as i64, ..Transport::default() });
        let mut io = vec![chunk.to_vec(), chunk.to_vec()];
        x.process(&mut io, chunk.len());
        out.extend_from_slice(&io[0]);
    }
    out
}

#[test]
fn hostile_keys_and_link_events_stay_finite() {
    let s = slab();
    let mut x = linked(&s, 0, 2.0, 3.0);
    x.set_param("scSource", 1.0);
    let mut io = vec![kit(512, 256, 0.9, 0.1), kit(512, 256, 0.9, 0.1)];
    // A short key, an empty key, a key of six channels: no panic.
    x.process_with_sidechain(&mut io, 512, Some(&[vec![0.5; 10]]));
    x.process_with_sidechain(&mut io, 512, Some(&[]));
    let six: Vec<Vec<f32>> = (0..6).map(|_| vec![0.2; 512]).collect();
    x.process_with_sidechain(&mut io, 512, Some(&six));
    // Partners and neighbours publishing nonsense far outside the block, by the hundred.
    let mut pair = other(&s, 1, shale_link::ROLE_PAIR, d::I_SNARE_BTM);
    let mut lead = other(&s, 2, shale_link::ROLE_LEAD, d::I_KICK_IN);
    let ev = |i: i64| LinkEvent {
        pos: if i % 2 == 0 { i64::MAX } else { i64::MIN },
        velocity: if i % 3 == 0 { f32::NAN } else { 2.0 },
        level_db: f32::INFINITY,
        kind: (i % 3) as i32 - 1,
        clock: CLK_TRANSPORT,
    };
    let many: Vec<LinkEvent> = (0..500).map(ev).collect();
    pair.publish_events(&many);
    lead.publish_events(&many);
    for at in [i64::MIN, i64::MAX, -5, 0] {
        x.set_transport(&Transport { samples: at, ..Transport::default() });
        x.process(&mut io, 512);
        // and in the window, with hostile values
        pair.publish_events(&[LinkEvent { pos: at, velocity: f32::NAN, ..trigger_at(at, 0.0) }, beacon_at(at, f32::NAN)]);
        lead.publish_events(&[beacon_at(at.saturating_add(3), f32::INFINITY), beacon_at(at, -f32::INFINITY)]);
        x.process(&mut io, 512);
    }
    assert!(all_finite(&io));
    // A slab of another layout, or too short, is not Shale's: nothing is read or written.
    for bad in [Arc::new(LinkSlab::new(shale_link::WORDS)), Arc::new(LinkSlab::new(64))] {
        bad.set_members(0, 0, 2, if bad.len() == 64 { shale_link::LAYOUT } else { 7 });
        let mut y = linked(&bad, 0, 0.0, 3.0);
        assert!(y.kit().is_unavailable());
        play(&mut y, &kit(4096, 1000, 0.9, 0.0), 0, 512);
        assert!((0..bad.len()).all(|i| bad.load(i) == Some(0)));
    }
}

#[test]
fn kit_link_partners_open_and_neighbours_defer() {
    // A Follow does not decide for itself: it opens when a partner's trigger arrives, there.
    let s = slab();
    let mut f = linked(&s, 0, 1.0, d::I_ROOM as f32);
    let mut lead = other(&s, 1, shale_link::ROLE_LEAD, d::I_SNARE_TOP);
    f.set_transport(&Transport { samples: 10_000, ..Transport::default() });
    let mut io = vec![kit(512, 12_000, 0.8, 0.0), kit(512, 12_000, 0.8, 0.0)];
    let mut quiet = io.clone();
    f.process(&mut quiet, 512);
    assert_eq!(readouts(&f)[4], 0.0, "a Follow ignores its own detector");
    f.set_transport(&Transport { samples: 10_512, ..Transport::default() });
    lead.publish_events(&[trigger_at(10_600, 0.7), beacon_at(10_590, 10.0)]);
    f.process(&mut io, 512);
    let r = readouts(&f);
    assert_eq!(r[4], 1.0);
    assert!((r[6] - 0.7).abs() < 1e-6, "the partner's velocity");
    assert_eq!(r[7], 0.0, "a partner's beacon is never evidence against a hit");
    // A Follow publishes its beacons but never its triggers (no echo between partners).
    assert!(f.link_events_out().iter().all(|e| e.kind == EV_BEACON));

    // A Lead publishes its own triggers once they outlive the 12 ms retraction window, and its
    // beacons, on the host timeline; a Follow of the group reads them back.
    let mut lead = linked(&s, 2, 0.0, d::I_SNARE_TOP as f32);
    let reader = other(&s, 3, shale_link::ROLE_FOLLOW, d::I_ROOM);
    let mut kinds = Vec::new();
    for blk in 0..4i64 {
        lead.set_transport(&Transport { samples: 48_000 + blk * 512, ..Transport::default() });
        let mut io = vec![kit(2048, 12_000, 0.8, 0.0)[blk as usize * 512..(blk as usize + 1) * 512].to_vec(); 2];
        lead.process(&mut io, 512);
        kinds.extend(lead.link_events_out().iter().map(|e| (e.kind, e.pos)));
    }
    let trig: Vec<i64> = kinds.iter().filter(|(k, _)| *k == EV_TRIGGER).map(|(_, p)| *p).collect();
    assert_eq!(trig.len(), 1, "{kinds:?}");
    assert!(trig[0] >= 48_000 && trig[0] < 48_000 + 256, "at the hit, not where it was published: {trig:?}");
    assert!(kinds.iter().any(|(k, _)| *k == EV_BEACON));
    let (mut p, mut n) = (Vec::with_capacity(64), Vec::with_capacity(64));
    reader.collect(0, shale_link::ROLE_FOLLOW, CLK_TRANSPORT, 48_000, 50_048, &mut p, &mut n, d::I_ROOM);
    assert_eq!(p.iter().map(|e| e.pos).collect::<Vec<_>>(), trig, "the Lead's trigger reaches its Follow");
    assert!(n.is_empty(), "a partner's beacons are not a neighbour's");
    // Another Link Group (B) hears nothing of group A.
    reader.publish_presence(1, shale_link::ROLE_FOLLOW, d::I_ROOM, 0);
    reader.collect(1, shale_link::ROLE_FOLLOW, CLK_TRANSPORT, 48_000, 50_048, &mut p, &mut n, d::I_ROOM);
    assert!(p.is_empty() && n.is_empty());

    // A neighbour that heard the same transient much louder takes the hit away (deferral).
    let s2 = slab();
    let mut snare = linked(&s2, 0, 0.0, d::I_SNARE_TOP as f32);
    snare.set_param("reject", 1.0);
    let mut kick = other(&s2, 1, shale_link::ROLE_LEAD, d::I_KICK_IN);
    kick.publish_events(&[beacon_at(0, 20.0)]);
    snare.set_transport(&Transport { samples: 0, ..Transport::default() });
    let mut io = vec![kit(512, 12_000, 0.8, 0.0), kit(512, 12_000, 0.8, 0.0)];
    snare.process(&mut io, 512);
    assert!(readouts(&snare)[7] >= 1.0 || readouts(&snare)[4] == 0.0, "deferred to the louder mic");

    // With Kit Link off nothing is collected or published, and the slot is not live.
    let s3 = slab();
    let mut off = linked(&s3, 0, 0.0, d::I_SNARE_TOP as f32);
    play(&mut off, &kit(48_000, 12_000, 0.8, 0.0), 0, 512);
    assert!(off.kit().presence(0).is_some());
    off.set_param("link", 0.0);
    let follower = other(&s3, 1, shale_link::ROLE_FOLLOW, d::I_ROOM);
    let before = readouts(&off)[4];
    play(&mut off, &kit(48_000, 12_000, 0.8, 0.0), 48_000, 512);
    assert!(off.link_events_out().is_empty());
    assert!(readouts(&off)[4] > before, "it still gates on its own");
    assert!(follower.presence(0).is_none(), "switched off: no longer live to the others");
    follower.collect(0, shale_link::ROLE_FOLLOW, CLK_TRANSPORT, 48_000, 96_000, &mut p, &mut n, d::I_ROOM);
    assert!(p.is_empty());
}

/// A kick drum (a 60 Hz thump with a click) every `period` from `first`.
fn kick_hits(len: usize, period: usize, first: usize, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| {
            if i < first {
                return 0.0;
            }
            let t = ((i - first) % period) as f32 / SR;
            amp * (-t / 0.09).exp() * (std::f32::consts::TAU * 60.0 * t).sin() + amp * 0.3 * (-t / 0.004).exp()
        })
        .collect()
}

/// The translated `processBlock` link input for one block, by hand: the other instance's events
/// as its ring holds them (the newest 64), filtered as `collect` does, fed at their offsets.
#[allow(clippy::too_many_arguments)]
fn feed_by_hand(dsp: &mut ShaleDsp, published: &[LinkEvent], partner: bool, start: i64, n: usize) {
    let ring = &published[published.len().saturating_sub(shale_link::EVENTS_PER_SLOT)..];
    let (mut p, mut nb) = (Vec::with_capacity(192), Vec::with_capacity(192));
    let guard = (0.012 * f64::from(SR)) as i64;
    shale_link::collect_from(ring, partner, CLK_TRANSPORT, start - guard, start + n as i64, &mut p, &mut nb);
    let at = |pos: i64| (pos - start).clamp(0, n as i64 - 1) as i32;
    for e in &p {
        dsp.add_partner_hit(e.velocity, at(e.pos));
    }
    for e in &nb {
        dsp.add_neighbour_beacon(e.level_db, at(e.pos));
    }
}

#[test]
fn a_kick_and_snare_in_one_group_defer_as_the_translated_cpp() {
    // The snare mic hears the kick 12 dB down; the kick mic hears it full. Linked as two Leads of
    // group A, the snare hands the kick's hits to the kick mic (deferral), and its output is the
    // C++'s: the translated DSP fed the kick's events by hand, through `collect`'s filter and
    // `processBlock`'s offsets, renders the same to the bit.
    let len = 48_000 * 3;
    let kick_src = kit(len, 12_000, 0.9, 0.0);
    let bleed: Vec<f32> = kick_src.iter().map(|v| v * 0.25).collect();
    // the snare's own hits halfway between the kick's
    let hits = kit(len + 6000, 12_000, 0.8, 0.0);
    let snare_src: Vec<f32> = (0..len).map(|i| bleed[i] + hits[i + 6000]).collect();
    let params = |x: &mut Shale| {
        x.set_param("threshold", 0.25);
        x.set_param("reject", 0.8);
        x.set_param("lookahead", 4.0);
    };
    let s = slab();
    let mut kick = linked(&s, 0, 0.0, d::I_KICK_IN as f32);
    let mut snare = linked(&s, 1, 0.0, d::I_SNARE_TOP as f32);
    params(&mut kick);
    params(&mut snare);
    // the translated reference: the snare's engine with the processor's pushes, no slab
    let mut reference = ShaleDsp::new();
    reference.prepare(f64::from(SR), 512);
    for (i, p) in SHALE_PARAMS.iter().enumerate() {
        reference.set_parameter(i, snare.param(p.id).unwrap());
    }
    let mut published: Vec<LinkEvent> = Vec::new();
    let (mut linked_out, mut by_hand) = (Vec::new(), Vec::new());
    for b in 0..len / 512 {
        let at = (b * 512) as i64;
        let r = b * 512..(b + 1) * 512;
        // the host renders the group in session order: the kick, then the snare
        kick.set_transport(&Transport { samples: at, ..Transport::default() });
        kick.process(&mut vec![kick_src[r.clone()].to_vec(); 2], 512);
        published.extend_from_slice(kick.link_events_out());
        snare.set_transport(&Transport { samples: at, ..Transport::default() });
        let mut io = vec![snare_src[r.clone()].to_vec(); 2];
        snare.process(&mut io, 512);
        linked_out.extend_from_slice(&io[0]);
        let mut io = vec![snare_src[r].to_vec(); 2];
        feed_by_hand(&mut reference, &published, shale_link::is_partner(0, d::I_SNARE_TOP, 0, d::I_KICK_IN), at, 512);
        reference.process(&mut io, 2, 512, None, 0, None);
        reference.clear_triggers();
        by_hand.extend_from_slice(&io[0]);
    }
    assert_eq!(linked_out, by_hand, "the linked snare is the translated C++ fed the kick's events");
    assert_eq!(reference.defer_seq & COUNT_MASK, readouts(&snare)[7] as i32);
    assert_eq!(reference.trig_seq & COUNT_MASK, readouts(&snare)[4] as i32);
    // Alone, the same snare also opens on the kick's bleed: linked, the kick's louder beacon takes
    // those hits (`defer`: the kick renders first, so its beacon is there before the snare's own
    // detector fires, and nothing has to be retracted); only the snare's own hits are left.
    let mut alone = prepared();
    params(&mut alone);
    alone.set_param("instrument", d::I_SNARE_TOP as f32);
    play(&mut alone, &snare_src, 0, 512);
    let (solo, linked_hits) = (readouts(&alone)[4], readouts(&snare)[4]);
    let snare_hits = ((len - 6000) / 12_000 + 1) as f32;
    assert_eq!(linked_hits, snare_hits, "linked: the snare's own hits only (solo {solo})");
    assert!(solo > linked_hits, "solo: the kick's bleed opens it too ({solo})");
    assert!(published.iter().any(|e| e.kind == EV_TRIGGER) && published.iter().any(|e| e.kind == EV_BEACON));
}

#[test]
fn a_follow_rendered_after_its_lead_opens_in_the_same_block() {
    // The host renders a Follow after its Lead (a consumer after a peer): the room mic opens on
    // the snare's hit in the block where it lands, not a block late at the block's first sample.
    let len = 48_000;
    let snare_src = kit(len, 12_000, 0.8, 0.0);
    let room_src: Vec<f32> = snare_src.iter().map(|v| v * 0.3).collect();
    let s = slab();
    let mut snare = linked(&s, 0, 0.0, d::I_SNARE_TOP as f32);
    let mut room = linked(&s, 1, 1.0, d::I_ROOM as f32);
    let mut lead_trig = Vec::new();
    let mut room_open = Vec::new();
    for b in 0..len / 256 {
        let at = (b * 256) as i64;
        let r = b * 256..(b + 1) * 256;
        snare.set_transport(&Transport { samples: at, ..Transport::default() });
        snare.process(&mut vec![snare_src[r.clone()].to_vec(); 2], 256);
        lead_trig.extend(snare.link_events_out().iter().filter(|e| e.kind == EV_TRIGGER).map(|e| e.pos));
        let before = readouts(&room)[4];
        room.set_transport(&Transport { samples: at, ..Transport::default() });
        room.process(&mut vec![room_src[r].to_vec(); 2], 256);
        if readouts(&room)[4] > before {
            room_open.push(at);
        }
    }
    assert!(lead_trig.len() >= 3, "{lead_trig:?}");
    assert_eq!(room_open.len(), lead_trig.len(), "{room_open:?} vs {lead_trig:?}");
    for (t, at) in lead_trig.iter().zip(&room_open) {
        assert!(*t >= *at - (0.012 * f64::from(SR)) as i64 && *t < *at + 256, "opened in the block it was published: {t} at {at}");
    }
}

#[test]
fn a_missing_partner_leaves_each_gate_on_its_own() {
    // A Follow with nobody to follow never opens; a Pair or a Lead alone gates on its own hits,
    // exactly as with Kit Link off. Nothing crashes, linked or not.
    let src = kit(96_000, 12_000, 0.8, 0.05);
    let s = slab();
    let mut follow = linked(&s, 0, 1.0, d::I_ROOM as f32);
    play(&mut follow, &src, 0, 512);
    assert_eq!(readouts(&follow)[4], 0.0);
    for role in [0.0, 2.0] {
        let s = slab();
        let mut alone = linked(&s, 0, role, d::I_SNARE_TOP as f32);
        // the same with Kit Link on in no group: nothing to hear, nothing heard
        let mut nobody = prepared();
        nobody.set_param("link", 1.0);
        nobody.set_param("linkRole", role);
        nobody.set_param("instrument", d::I_SNARE_TOP as f32);
        let a = play(&mut alone, &src, 0, 512);
        let b = play(&mut nobody, &src, 0, 512);
        assert_eq!(a, b, "role {role}: alone in its group gates as it would on its own");
        assert!(readouts(&alone)[4] >= 7.0, "{:?}", readouts(&alone));
        assert!(a.iter().all(|v| v.is_finite()));
    }
    // In no group at all (Kit Link on, no slab): the same.
    let mut nogroup = prepared();
    nogroup.set_param("link", 1.0);
    nogroup.set_param("linkRole", 1.0);
    play(&mut nogroup, &src, 0, 512);
    assert_eq!(readouts(&nogroup)[4], 0.0, "a Follow in no group follows nobody");
    assert!(nogroup.kit().is_unavailable());
    assert_eq!(nogroup.align_report().state, AL_LINK_OFF);
}

#[test]
fn a_member_that_leaves_frees_its_slot_and_a_retired_one_never_clears_a_newcomer() {
    let s = slab();
    let mut a = linked(&s, 0, 0.0, d::I_KICK_IN as f32);
    let mut b = linked(&s, 1, 0.0, d::I_SNARE_TOP as f32);
    play(&mut a, &kit(2048, 1000, 0.8, 0.0), 0, 512);
    play(&mut b, &kit(2048, 1000, 0.8, 0.0), 0, 512);
    assert!(a.kit().presence(1).is_some());
    // b is deleted: the host takes the slab away; its slot goes.
    b.set_link(None, 0);
    assert!(a.kit().presence(1).is_none());
    // A newcomer takes index 1; an old instance that still thought it held 1 lets go late.
    let mut ghost = KitLink::default();
    ghost.set(Some(Arc::clone(&s)), 2);
    let mut c = linked(&s, 2, 0.0, d::I_ROOM as f32);
    play(&mut c, &kit(1024, 1000, 0.8, 0.0), 0, 512);
    drop(ghost);
    assert!(a.kit().presence(2).is_some(), "the newcomer's slot survives the old instance's release");
    // More than 16 members: the 17th has no slot and gates on its own.
    let mut late = linked(&s, shale_link::MAX_INSTANCES, 0.0, d::I_HAT as f32);
    assert!(late.kit().is_unavailable());
    play(&mut late, &kit(4096, 1000, 0.8, 0.1), 0, 512);
    drop(c);
    assert!(a.kit().presence(2).is_none(), "dropping releases");
}

/// A decaying noise burst every `period` from `first` (a snare's crack), inverted when `sign` < 0.
fn bursts(len: usize, period: usize, first: usize, sign: f32) -> Vec<f32> {
    let mut seed = 0x1234_5678u32;
    let noise: Vec<f32> = (0..period)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        })
        .collect();
    (0..len)
        .map(|i| {
            if i < first {
                return 0.0;
            }
            let k = (i - first) % period;
            let t = k as f32 / SR;
            sign * (1.4 * noise[k] + (std::f32::consts::TAU * 190.0 * t).sin()) * (-t / 0.05).exp()
        })
        .collect()
}

#[test]
fn phase_alignment_measures_against_the_reference_and_line_up_has_it() {
    // The overheads (Phase Reference) hear each snare hit 100 samples after the snare mic, upside
    // down. The snare mic, Align To Group Ref, measures that over its own hits: the report says
    // +100 samples and flip, as the C++ `runAlignment` would, and Line Up's values follow.
    let len = 48_000 * 5;
    let close = bursts(len, 14_400, 2000, 1.0);
    let far: Vec<f32> = bursts(len + 100, 14_400, 2100, -0.6).into_iter().take(len).collect();
    let s = slab();
    let mut oh = linked(&s, 0, 0.0, d::I_OVERHEADS as f32);
    oh.set_param("phaseRef", 1.0);
    let mut sn = linked(&s, 1, 0.0, d::I_SNARE_TOP as f32);
    sn.set_param("reject", 0.0);
    assert_eq!(sn.align_report().state, AL_LINK_OFF);
    let mut states = Vec::new();
    for b in 0..len / 512 {
        let at = (b * 512) as i64;
        let r = b * 512..(b + 1) * 512;
        oh.set_transport(&Transport { samples: at, ..Transport::default() });
        oh.process(&mut vec![far[r.clone()].to_vec(); 2], 512);
        sn.set_transport(&Transport { samples: at, ..Transport::default() });
        sn.process(&mut vec![close[r].to_vec(); 2], 512);
        states.push(sn.align_report().state);
    }
    assert_eq!(oh.align_report().state, AL_IS_REFERENCE);
    assert!(states.contains(&AL_LISTENING), "listening before the first measurement");
    let rep = sn.align_report();
    assert_eq!(rep.state, AL_MEASURED, "{rep:?}");
    assert!(rep.hits >= LINE_UP_HITS, "{rep:?}");
    assert_eq!(rep.ref_instrument, d::I_OVERHEADS);
    assert!((rep.offset_ms * 48.0 - 100.0).abs() < 1.0, "{rep:?}");
    assert_eq!(rep.polarity, -1);
    assert!(rep.corr > 0.8 && rep.spread_ms < 0.05, "{rep:?}");
    // (a reset drops a measurement under way, so the states below are the next pass's)
    sn.reset();
    oh.reset();
    // The report reaches the UI through the analysis handle.
    let v = sn.analysis().unwrap().report();
    assert_eq!(v[REPORT_MARK], REPORT_VALID);
    assert_eq!((v[REPORT_STATE], v[REPORT_POLARITY], v[REPORT_REF]), (AL_MEASURED as f32, -1.0, d::I_OVERHEADS as f32));
    assert!((f64::from(v[REPORT_OFFSET_MS]) - rep.offset_ms).abs() < 1e-5);
    // Line Up's knob: `offsetMsToKnob`.
    assert!((shale_align::offset_ms_to_knob(rep.offset_ms) - (0.5 + 100.0 / 48.0 / 20.0)).abs() < 0.002);

    // Align To a named mic finds it in any group; nobody of that name: no reference.
    sn.set_param("alignTo", 1.0 + d::I_ROOM as f32);
    play(&mut sn, &close[..2048], len as i64, 512);
    assert_eq!(sn.align_report().state, AL_NO_REFERENCE);
    sn.set_param("alignTo", 1.0 + d::I_OVERHEADS as f32);
    oh.set_param("linkGroup", 2.0);
    play(&mut oh, &far[..512], len as i64 + 2048, 512);
    play(&mut sn, &close[..512], len as i64 + 2048, 512);
    assert_eq!(sn.align_report().ref_instrument, d::I_OVERHEADS);
    assert_ne!(sn.align_report().state, AL_NO_REFERENCE);
    // A Phase Ref that is not pointed at anyone is the reference; Kit Link off: off.
    sn.set_param("phaseRef", 1.0);
    sn.set_param("alignTo", 0.0);
    play(&mut sn, &close[..512], len as i64 + 4096, 512);
    assert_eq!(sn.align_report().state, AL_IS_REFERENCE);
    sn.set_param("link", 0.0);
    play(&mut sn, &close[..512], len as i64 + 8192, 512);
    assert_eq!(sn.align_report().state, AL_LINK_OFF);
}

#[test]
fn shared_audio_reads_only_what_is_there() {
    let s = slab();
    let mut a = KitLink::default();
    a.set(Some(Arc::clone(&s)), 3);
    let reader = other(&s, 4, 0, 0);
    let block: Vec<f32> = (0..512).map(|i| i as f32).collect();
    assert_eq!(reader.audio_newest(3, 48_000.0), -1, "nothing written yet");
    a.audio_write(&block, 1000, CLK_TRANSPORT, 48_000.0);
    a.audio_write(&block, 1512, CLK_TRANSPORT, 48_000.0);
    assert_eq!(reader.audio_newest(3, 48_000.0), 2024);
    assert_eq!(reader.audio_newest(3, 44_100.0), -1, "another rate");
    let mut out = [0.0f32; 600];
    assert!(reader.audio_read(3, 1100, &mut out, 48_000.0));
    assert_eq!((out[0], out[411], out[412], out[599]), (100.0, 511.0, 0.0, 187.0));
    assert!(!reader.audio_read(3, 999, &mut out, 48_000.0), "before the run");
    assert!(!reader.audio_read(3, 1500, &mut out, 48_000.0), "past the newest");
    assert!(!reader.audio_read(3, 1100, &mut out, 96_000.0));
    // A seek starts a new run: the old audio is no longer readable.
    a.audio_write(&block, 90_000, CLK_TRANSPORT, 48_000.0);
    assert!(!reader.audio_read(3, 1100, &mut out, 48_000.0));
    let mut small = [0.0f32; 100];
    assert!(reader.audio_read(3, 90_100, &mut small, 48_000.0));
    // The wall clock is never read back; a restart (the DAW's reset) empties the run.
    a.audio_write(&block, 90_512, shale_link::CLK_WALL, 48_000.0);
    assert_eq!(reader.audio_newest(3, 48_000.0), -1);
    a.audio_write(&block, 0, CLK_TRANSPORT, 48_000.0);
    a.restart();
    assert!(!reader.audio_read(3, 0, &mut small, 48_000.0));
    // Hostile: huge reads, far positions, other slots.
    let mut huge = vec![0.0f32; shale_link::RING_SIZE];
    assert!(!reader.audio_read(3, 0, &mut huge, 48_000.0));
    a.audio_write(&block, i64::MAX - 100, CLK_TRANSPORT, 48_000.0);
    assert!(!reader.audio_read(3, i64::MAX - 50, &mut small, 48_000.0));
    assert!(!reader.audio_read(99, 0, &mut small, 48_000.0));
    assert_eq!(reader.audio_newest(99, 48_000.0), -1);
}

#[test]
fn align_delays_flips_and_follows_bypass() {
    let mut s = prepared();
    s.set_param("lookahead", 0.0);
    s.set_param("reduction", 0.0);
    s.set_param("mix", 0.0);
    s.set_param("align", 1.0);
    s.set_param("alignOffset", 0.75); // +5 ms: this mic later by 240 samples
    s.set_param("alignFlip", 1.0);
    // Settle the glides (80 ms per 10 ms of change) on silence.
    let mut io = vec![vec![0.0f32; 48_000], vec![0.0f32; 48_000]];
    run(&mut s, &mut io, 512);
    assert!((s.align_delay().current() - (484.0 + 240.0)).abs() < 1e-9);
    assert_eq!(s.latency(), 484);
    let mut imp = vec![vec![0.0f32; 2048], vec![0.0f32; 2048]];
    imp[0][0] = 1.0;
    imp[1][0] = 1.0;
    run(&mut s, &mut imp, 512);
    let at = imp[0].iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i).unwrap();
    assert_eq!(at, 724);
    assert!(imp[0][724] < -0.99, "flipped: {}", imp[0][724]);
    // The engine bypass keeps the base delay but not the offset or the flip.
    s.set_param("bypass", 1.0);
    let mut io = vec![vec![0.0f32; 96_000], vec![0.0f32; 96_000]];
    run(&mut s, &mut io, 512);
    assert!((s.align_delay().current() - 484.0).abs() < 1e-9);
    assert!((s.align_delay().gain_now() - 1.0).abs() < 1e-6);
}

#[test]
fn switching_quality_and_model_mid_stream_is_safe() {
    // No quality setting: the Mic, Lookahead (latency), the modes, Monitor, Align switch.
    let mut s = prepared();
    s.set_param("debleed", 0.8);
    let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    let src = kit(128 * 700, 3000, 0.9, 0.2);
    let mut latencies = std::collections::BTreeSet::new();
    for blk in 0..700 {
        s.set_param("instrument", (blk % 11) as f32);
        s.set_param("lookahead", ((blk / 3) % 5) as f32);
        s.set_param("debleedMode", ((blk / 2) % 2) as f32);
        s.set_param("closeMode", ((blk / 5) % 2) as f32);
        s.set_param("monitor", ((blk / 7) % 3) as f32);
        s.set_param("autoBand", ((blk / 11) % 2) as f32);
        s.set_param("align", ((blk / 13) % 2) as f32);
        s.set_param("alignOffset", (blk % 17) as f32 / 16.0);
        s.set_param("ghost", (blk % 2) as f32);
        for b in &mut buf {
            b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
        }
        s.process(&mut buf, 128);
        latencies.insert(s.latency());
        assert!(all_finite(&buf), "block {blk}");
        assert!(buf.iter().flatten().all(|v| v.abs() < 100.0), "block {blk}");
    }
    assert!(latencies.len() >= 8, "{latencies:?}");
}

#[test]
fn mono_and_wide_buffers() {
    // Mono: one channel processed.
    let mut s = Shale::new();
    s.prepare(SR, 512, 1);
    let src = kit(48_000, 24_000, 0.8, 0.05);
    let mut mono = vec![src.clone()];
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_eq!(readouts(&s)[4], 2.0);
    assert!(rms(&mono[0][21_000..23_500]) < 1e-6);

    // Six channels: the first two processed, the rest untouched.
    let mut s = Shale::new();
    s.prepare(SR, 512, 6);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| kit(9600, 4800, 0.1 * (c + 1) as f32, 0.05)).collect();
    let orig = wide.clone();
    run(&mut s, &mut wide, 512);
    assert!(all_finite(&wide));
    assert_ne!(wide[0], orig[0]);
    assert_ne!(wide[1], orig[1]);
    assert_eq!(wide[2..], orig[2..]);

    // Fewer buffers than prepared, short buffers, zero frames: no panic.
    let mut s = prepared();
    s.set_param("align", 1.0);
    let mut one = vec![vec![0.3f32; 10]];
    s.process(&mut one, 512);
    s.process(&mut [], 64);
    let mut empty = vec![Vec::new(), Vec::new()];
    s.process(&mut empty, 64);
    let mut ragged = vec![vec![0.3f32; 64], vec![0.3f32; 10]];
    s.process(&mut ragged, 64);
    assert!(all_finite(&ragged));
}

#[test]
fn silence_tail_is_cheap() {
    let sr = 48_000usize;
    for (debleed, mode) in [(0.0, 0.0), (0.8, 0.0), (0.8, 1.0)] {
        let time = |signal_s: usize, silence_s: usize| {
            let mut s = prepared();
            s.set_param("debleed", debleed);
            s.set_param("debleedMode", mode);
            s.set_param("reduction", 0.3);
            let mut io = vec![kit(signal_s * sr, 12_000, 0.9, 0.1), kit(signal_s * sr, 12_000, 0.9, 0.1)];
            for c in &mut io {
                c.resize((signal_s + silence_s) * sr, 0.0);
            }
            let t = Instant::now();
            run(&mut s, &mut io, 512);
            assert!(all_finite(&io));
            t.elapsed().as_secs_f64()
        };
        let tail = time(2, 10);
        let full = time(12, 0);
        assert!(tail < 4.0 * full, "debleed {debleed}/{mode}: silence tail {tail:.3}s vs signal {full:.3}s");
    }
}

#[test]
fn denormal_levels_never_trigger() {
    // The C++ runs under flush-to-zero; here denormals survive. No decision can see them.
    let mut s = prepared();
    s.set_param("threshold", 0.0); // the most sensitive line, -66 dB
    s.set_param("ghost", 1.0); // and 13 dB below it
    s.set_param("debleed", 1.0);
    s.set_param("debleedMode", 1.0);
    let mut io = vec![kit(24_000, 12_000, 0.5, 0.1), kit(24_000, 12_000, 0.5, 0.1)];
    run(&mut s, &mut io, 512);
    let before = readouts(&s)[4];
    let tiny: Vec<f32> = (0..96_000).map(|i| if i % 2 == 0 { 1e-39 } else { -1e-39 }).collect();
    let mut io = vec![tiny.clone(), tiny];
    run(&mut s, &mut io, 512);
    assert_eq!(readouts(&s)[4], before, "no hit from denormal input");
    assert!(all_finite(&io));
}

/// Every heap block the plugin owns, as (pointer, capacity).
fn heap(s: &Shale) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = s.engine().heap().iter().chain(s.align_delay().heap().iter()).map(|(p, c)| (*p as usize, *c)).collect();
    v.push((s.engine() as *const ShaleDsp as usize, 0));
    v.push((s.engine().scope.as_ptr() as usize, 0));
    v.push((s.partner_events.as_ptr() as usize, s.partner_events.capacity()));
    v.push((s.neighbour_events.as_ptr() as usize, s.neighbour_events.capacity()));
    v.push((s.out_events.as_ptr() as usize, s.out_events.capacity()));
    v.push((s.mono.as_ptr() as usize, s.mono.capacity()));
    v.extend(s.al.measurer.heap());
    v
}

#[test]
fn process_does_not_allocate() {
    // No counting allocator in this crate (`unsafe_code` is forbidden, so no `GlobalAlloc`): the
    // plugin's heap memory is the engine's box, its scope box and two delay lines, the align
    // rings, the three link event lists, the link's mono scratch and the measurement's windows,
    // all made in `new`/`prepare` (the link group's slab is the host's). They keep their
    // addresses and capacities across 1000 blocks of switching, keys and link traffic: a
    // partner and a neighbour publishing hundreds of events, shared audio, and the phase
    // measurement running against a reference.
    let sl = slab();
    let mut s = linked(&sl, 0, 0.0, 3.0);
    let mut reference = linked(&sl, 1, 0.0, d::I_OVERHEADS as f32);
    reference.set_param("phaseRef", 1.0);
    let mut pair = other(&sl, 2, shale_link::ROLE_PAIR, d::I_SNARE_BTM);
    let mut kick = other(&sl, 3, shale_link::ROLE_LEAD, d::I_KICK_IN);
    let before = heap(&s);
    let ref_before = heap(&reference);
    let long = kit(1000 * 256, 4800, 0.8, 0.1);
    let src = kit(256, 128, 0.7, 0.1);
    let mut io = vec![src.clone(), src.clone()];
    let mut rio = vec![src.clone(), src.clone()];
    let key = vec![src.clone()];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    let mut measured = false;
    for blk in 0..1000i64 {
        let at = blk * 256;
        for c in io.iter_mut().chain(rio.iter_mut()) {
            c.copy_from_slice(&long[at as usize..at as usize + 256]);
        }
        if blk >= 600 {
            s.set_param("instrument", (blk % 11) as f32);
            s.set_param("lookahead", ((blk / 4) % 5) as f32);
            s.set_param("debleedMode", (blk % 2) as f32);
            s.set_param("debleed", 0.7);
            s.set_param("align", ((blk / 10) % 2) as f32);
            s.set_param("linkRole", (blk % 3) as f32);
            s.set_param("scSource", ((blk / 3) % 2) as f32);
        }
        let events: Vec<LinkEvent> = (0..150).map(|i| if i % 2 == 0 { trigger_at(at + i, 0.5) } else { beacon_at(at + i, -6.0) }).collect();
        pair.publish_events(&events);
        kick.publish_events(&events);
        reference.set_transport(&Transport { samples: at, ..Transport::default() });
        reference.process(&mut rio, 256);
        s.set_transport(&Transport { samples: at, ..Transport::default() });
        if blk % 2 == 0 {
            s.process_with_sidechain(&mut io, 256, Some(&key));
        } else {
            s.process(&mut io, 256);
        }
        measured |= s.al.measurer.busy();
    }
    assert!(measured, "the measurement ran");
    assert_eq!(before, heap(&s));
    assert_eq!(ref_before, heap(&reference));
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread: the live prepare refills every buffer in place
    // (Kit Link's restart is atomic stores into the host's slab).
    let sl = slab();
    let mut s = linked(&sl, 0, 0.0, 3.0);
    s.set_param("align", 1.0);
    let mut io = vec![kit(512, 256, 0.5, 0.1), kit(512, 256, 0.5, 0.1)];
    s.process(&mut io, 512);
    let before = heap(&s);
    for _ in 0..3 {
        s.reset();
        assert_eq!(before, heap(&s));
        s.process(&mut io, 512);
    }
    // A live prepare at the same rate and block size does not allocate either.
    s.prepare(SR, 512, 2);
    assert_eq!(before, heap(&s));
    assert!(all_finite(&io));
}

#[test]
fn two_linked_resets_render_identically() {
    // Live-reset determinism with the link: a kick and a snare in one group (the snare deferring
    // to the kick, a room following the snare), reset together as the host does on Play, Stop
    // and Locate, render the same twice over, whatever played and was published in between.
    let len = 48_000;
    let kick_src = kick_hits(len, 9600, 300, 0.9);
    let hits = kit(len + 4800, 9600, 0.8, 0.0);
    let snare_src: Vec<f32> = (0..len).map(|i| kick_src[i] * 0.25 + hits[i + 4800]).collect();
    let room_src: Vec<f32> = (0..len).map(|i| kick_src[i] * 0.3 + snare_src[i] * 0.3).collect();
    let sl = slab();
    let mut kick = linked(&sl, 0, 0.0, d::I_KICK_IN as f32);
    let mut snare = linked(&sl, 1, 0.0, d::I_SNARE_TOP as f32);
    let mut room = linked(&sl, 2, 1.0, d::I_ROOM as f32);
    for x in [&mut kick, &mut snare, &mut room] {
        x.set_param("threshold", 0.3);
        x.set_param("reject", 0.9);
        x.set_param("align", 1.0);
        x.set_param("alignOffset", 0.6);
    }
    room.set_param("phaseRef", 1.0);
    let render = |kick: &mut Shale, snare: &mut Shale, room: &mut Shale, from: i64| {
        let mut out = Vec::new();
        for b in 0..len / 512 {
            let at = from + (b * 512) as i64;
            let r = b * 512..(b + 1) * 512;
            for (x, src) in [(&mut *kick, &kick_src), (&mut *snare, &snare_src), (&mut *room, &room_src)] {
                x.set_transport(&Transport { samples: at, ..Transport::default() });
                let mut io = vec![src[r.clone()].to_vec(); 2];
                x.process(&mut io, 512);
                out.extend_from_slice(&io[0]);
            }
        }
        out
    };
    render(&mut kick, &mut snare, &mut room, 0);
    for x in [&mut kick, &mut snare, &mut room] {
        x.reset();
    }
    let first = render(&mut kick, &mut snare, &mut room, 96_000);
    assert!(readouts(&snare)[7] >= 1.0, "the snare deferred: {:?}", readouts(&snare));
    assert!(readouts(&room)[4] >= 3.0, "the room followed: {:?}", readouts(&room));
    // something else in between, at another place
    render(&mut snare, &mut kick, &mut room, 500_000);
    for x in [&mut kick, &mut snare, &mut room] {
        x.reset();
    }
    let second = render(&mut kick, &mut snare, &mut room, 96_000);
    assert_eq!(first, second, "two resets of a linked group render the same");
}

#[test]
fn a_lower_rate_re_prepare_keeps_the_latency_the_real_delay() {
    // 5 ms of lookahead worked out at 192 kHz is 960 samples; prepared live at 44.1 kHz with a
    // small block, the delay line holds 221 + 8 at most. The C++ keeps 960 (the delay wraps and
    // the reported latency lies); the port clamps it again (a documented safety deviation).
    let mut s = Shale::new();
    s.prepare(192_000.0, 64, 2);
    s.set_param("lookahead", 4.0);
    s.set_param("mix", 0.0);
    s.set_param("reduction", 0.0);
    s.process(&mut vec![vec![0.0; 64]; 2], 64);
    assert_eq!(s.latency(), 960);
    s.prepare(44_100.0, 64, 2);
    let la = s.latency();
    assert_eq!(la, (0.005f64 * 44_100.0).ceil() as usize + 8);
    // the real delay is the reported one: an impulse comes out `latency` samples later
    let mut io = vec![vec![0.0f32; 64]; 2];
    let mut out = Vec::new();
    for b in 0..10 {
        for c in &mut io {
            c.fill(0.0);
            if b == 0 {
                c[0] = 1.0;
            }
        }
        s.process(&mut io, 64);
        out.extend_from_slice(&io[0]);
    }
    let peak = out.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i).unwrap();
    assert_eq!(peak, la, "{:?}", &out[..la + 4]);
    // A re-prepare at the same or a higher rate changes nothing (the C++ quirk stays).
    let mut q = Shale::new();
    q.prepare(44_100.0, 512, 2);
    q.process(&mut vec![vec![0.0; 512]; 2], 512);
    q.prepare(96_000.0, 512, 2);
    assert_eq!(q.latency(), 96, "the default 2 ms stays 96 samples");
}

#[test]
fn a_short_key_is_silent_not_the_tracks_own_input() {
    // Trigger From = Sidechain with a key buffer shorter than the block: the samples it lacks are
    // silence, so the hits on the track never open the gate through it.
    let mut keyed = prepared();
    keyed.set_param("scSource", 1.0);
    let src = kit(512, 128, 0.9, 0.0);
    for _ in 0..8 {
        let mut io = vec![src.clone(), src.clone()];
        keyed.process_with_sidechain(&mut io, 512, Some(&[vec![0.0; 3]]));
    }
    assert_eq!(readouts(&keyed)[4], 0.0, "silent key: never opens");
    // A short key keys it where it has samples.
    let mut ok = prepared();
    ok.set_param("scSource", 1.0);
    for _ in 0..4 {
        let mut io = vec![vec![0.0; 512]; 2];
        ok.process_with_sidechain(&mut io, 512, Some(&[src[..300].to_vec()]));
    }
    assert!(readouts(&ok)[4] >= 1.0);
}

/// A module with non-default parameters that has played (and is mid-hit) for a while.
fn warmed(history: usize) -> Shale {
    let mut s = Shale::new();
    s.prepare(44_100.0, 512, 2);
    for (id, v) in [
        ("instrument", 5.0),
        ("threshold", 0.45),
        ("sens", 0.6),
        ("reduction", 0.5),
        ("length", 0.6),
        ("closeMode", 0.0),
        ("lookahead", 4.0),
        ("hold", 0.5),
        ("retrig", 0.3),
        ("debleed", 0.7),
        ("debleedMode", 1.0),
        ("velTrack", 0.8),
        ("mix", 0.9),
        ("trim", 0.6),
        ("output", 0.4),
        ("align", 1.0),
        ("alignOffset", 0.3),
        ("alignFlip", 1.0),
    ] {
        assert!(s.set_param(id, v));
    }
    let mut warm = vec![kit(9000 + history, 6100 + history, 0.9, 0.2), sine(9000 + history, 180.0, -0.3)];
    run(&mut s, &mut warm, 333);
    s
}

#[test]
fn reset_twice_from_the_same_state_renders_identically() {
    // The DAW's reset is the C++ prepareToPlay on the live instance: two resets of the same
    // state render bit for bit the same, whatever played before.
    let src = vec![kit(65_536, 13_000, 0.8, 0.15), kit(65_536, 13_000, 0.8, 0.15)];
    let mut a = warmed(0);
    a.reset();
    let hits_before = readouts(&a)[4];
    let mut first = src.clone();
    run(&mut a, &mut first, 512);
    assert!(all_finite(&first));
    assert!(readouts(&a)[4] - hits_before >= 4.0, "the window crosses several hits: {hits_before} -> {}", readouts(&a)[4]);
    // Play something else (a loud hit last, so the velocity tracking moved), then reset again.
    let mut other = vec![sine(5000, 220.0, 0.9), kit(5000, 1000, 1.0, 0.0)];
    run(&mut a, &mut other, 256);
    a.reset();
    let mut second = src.clone();
    run(&mut a, &mut second, 512);
    assert_eq!(first, second, "two resets render the same");
    // A module with a different history (and a Mic moved and back) renders the same too.
    let mut b = warmed(1234);
    b.set_param("instrument", 0.0);
    let mut io = vec![kit(4000, 1500, 0.4, 0.3), kit(4000, 1500, 0.4, 0.3)];
    run(&mut b, &mut io, 200);
    b.set_param("instrument", 5.0);
    run(&mut b, &mut io, 200);
    b.reset();
    let mut third = src.clone();
    run(&mut b, &mut third, 512);
    assert_eq!(first, third, "a reset is independent of the history");
}

#[test]
fn reset_keeps_the_parameters_and_glides_nowhere() {
    let mut s = warmed(77);
    s.reset();
    // The smoothing of Input and Output starts at its target; the align delay and polarity are
    // already where the parameters put them (no 80 ms glide, no 10 ms flip crossfade).
    for (now, target) in s.engine().smoothing() {
        assert_eq!(now, target);
    }
    let al = s.align_delay();
    assert_eq!(al.current(), al.target());
    assert_eq!(al.gain_now(), al.gain_want());
    assert_eq!(al.gain_want(), -1.0);
    assert_eq!(s.param("instrument"), Some(5.0));
    assert_eq!(s.latency(), 221 + 445, "5 ms at 44.1 kHz plus the align base");
    // Dry (Mix 0) and fully open: from the first block the output is the input, delayed and
    // scaled, exactly: nothing mutes it and nothing glides in.
    let mut s = warmed(77);
    s.set_param("mix", 0.0);
    s.set_param("alignOffset", 0.5);
    s.set_param("alignFlip", 0.0);
    s.reset();
    let src = kit(8192, 2000, 0.6, 0.1);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    let g = d::db_to_lin(d::knob_to_trim_db(0.6)) * d::db_to_lin(d::knob_to_out_db(0.4));
    let lat = s.latency();
    for i in 0..4000 {
        assert!((io[0][i + lat] - src[i] * g).abs() < 1e-5, "sample {i}: {} vs {}", io[0][i + lat], src[i] * g);
    }
    assert!(io[0][..lat].iter().all(|v| v.abs() < 1e-6));
    // The C++-faithful first prepare, by contrast, glides the align delay in from its base.
    let mut f = Shale::new();
    f.set_param("align", 1.0);
    f.set_param("alignOffset", 1.0);
    f.prepare(SR, 512, 2);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    f.process(&mut io, 64);
    assert!(f.align_delay().current() < f.align_delay().target());
}

/// The sidechain and Kit Link paths against the C++, which `Shale_Render.exe` cannot drive (its
/// bus layout has no sidechain and it runs one instance). `ShaleDSP.h` itself, built read-only
/// into a scratch harness (`crates/dsp/tests/shale_harness/harness.cpp`), is fed a main bus, an
/// external key and partner hits / neighbour beacons at fixed positions; this test drives the
/// port's engine the same way and compares.
///
/// Regenerate (Windows, Visual Studio; never inside the FireStation tree):
/// 1. `set SHALE_HARNESS_DIR=<an empty scratch folder>` and run this test once
///    (`cargo test -p soundcraft-dsp --lib shale_matches_the_cpp_harness -- --ignored`): it
///    writes `in.raw` there and says what is missing;
/// 2. copy `harness.cpp` and `build.bat` from `crates/dsp/tests/shale_harness/` to that folder
///    and run `build.bat` (it includes `C:\projects\A18\FireStation\Shale\Source` read-only);
/// 3. for k in 0 1 2 3: `harness.exe k 96000 in.raw out_k.raw`;
/// 4. run the test again (`--nocapture` prints each): every scenario must match at -140 dB or
///    better. 2026-10-20: sidechain bit-exact, Pair -170 dB, Follow bit-exact, Lead with Learned
///    DeBleed -159 dB; deferrals as the harness prints them (0, 318, 1, 0).
#[test]
#[ignore = "needs the scratch C++ harness's output (see the doc comment)"]
fn shale_matches_the_cpp_harness() {
    const N: usize = 96_000;
    let Some(dir) = std::env::var_os("SHALE_HARNESS_DIR").map(std::path::PathBuf::from) else {
        panic!("set SHALE_HARNESS_DIR (see the doc comment)");
    };
    // mainL mainR scL scR, each N
    let main = kit(N, 9000, 0.8, 0.12);
    let key = kit(N + 3000, 7000, 0.7, 0.0);
    let input: Vec<f32> = main.iter().chain(&main).chain(&key[3000..]).chain(&key[3000..]).copied().collect();
    let bytes: Vec<u8> = input.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(dir.join("in.raw"), bytes).unwrap();
    let defs: [f32; NUM_PARAMS] = std::array::from_fn(|i| SHALE_PARAMS[i].default);
    let mut missing = Vec::new();
    for scenario in 0..4 {
        let Ok(raw) = std::fs::read(dir.join(format!("out_{scenario}.raw"))) else {
            missing.push(scenario);
            continue;
        };
        let want: Vec<f32> = raw.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        assert_eq!(want.len(), 2 * N);
        let mut v = defs;
        match scenario {
            0 => {
                v[d::P_SC_SOURCE] = 1.0;
                v[d::P_THRESHOLD] = 0.5;
                v[d::P_DEBLEED] = 0.5;
            }
            1 => {
                v[d::P_LINK] = 1.0;
                v[d::P_LINK_ROLE] = 2.0;
                v[d::P_REJECT] = 0.8;
                v[d::P_LOOKAHEAD] = 4.0;
                v[d::P_REDUCTION] = 0.6;
            }
            2 => {
                v[d::P_LINK] = 1.0;
                v[d::P_LINK_ROLE] = 1.0;
                v[d::P_INSTRUMENT] = 10.0;
                v[d::P_REDUCTION] = 0.5;
            }
            _ => {
                v[d::P_LINK] = 1.0;
                v[d::P_REJECT] = 1.0;
                v[d::P_DEBLEED] = 0.6;
                v[d::P_DEBLEED_MODE] = 1.0;
                v[d::P_REDUCTION] = 0.7;
            }
        }
        let mut dsp = ShaleDsp::new();
        dsp.prepare(48_000.0, 512);
        for (i, x) in v.iter().enumerate() {
            dsp.set_parameter(i, *x);
        }
        let mut got = vec![0.0f32; 2 * N];
        let mut pos = 0;
        while pos < N {
            let len = 512.min(N - pos);
            for i in 0..len {
                let t = pos + i;
                if scenario >= 1 && t % 7000 == 100 {
                    dsp.add_partner_hit(0.6, i as i32);
                }
                if scenario >= 1 && t % 11_000 == 50 {
                    dsp.add_neighbour_beacon(0.0, i as i32);
                }
            }
            let mut io = vec![main[pos..pos + len].to_vec(), main[pos..pos + len].to_vec()];
            let sc = vec![key[3000 + pos..3000 + pos + len].to_vec(), key[3000 + pos..3000 + pos + len].to_vec()];
            dsp.process(&mut io, 2, len, Some(&sc), 2, None);
            dsp.clear_triggers();
            got[pos..pos + len].copy_from_slice(&io[0]);
            got[N + pos..N + pos + len].copy_from_slice(&io[1]);
            pos += len;
        }
        let err: f64 = got.iter().zip(&want).map(|(a, b)| f64::from(a - b).powi(2)).sum::<f64>();
        let sig: f64 = want.iter().map(|b| f64::from(*b).powi(2)).sum::<f64>();
        let db = 10.0 * (err / sig.max(1e-30)).max(1e-30).log10();
        eprintln!("scenario {scenario}: {db:.1} dB, {} deferred", dsp.defer_seq);
        assert!(db <= -140.0, "scenario {scenario}: {db:.1} dB");
    }
    assert!(missing.is_empty(), "no harness output for scenarios {missing:?}: build and run the harness (see the doc comment)");
}

#[test]
fn the_measurement_budget_scales_with_the_block() {
    // The correlation is a share of the block's time: a 64-sample block does an eighth of what a
    // 512-sample block does, so a small buffer spreads the same measurement over 8x the blocks
    // (and the result is still `measure`'s to the bit).
    assert_eq!(measure_budget(512), 65_536);
    assert_eq!(measure_budget(64), 8192);
    let (max_lag, pre, post) = align_windows(48_000.0);
    let n = pre + post;
    let x: Vec<f32> = (0..n).map(|i| ((i * 7919) % 211) as f32 / 211.0 - 0.5).collect();
    let y: Vec<f32> = (0..n + 2 * max_lag).map(|i| ((i * 104_729) % 307) as f32 / 307.0 - 0.5).collect();
    let want = shale_align::measure(&x, n, &y, max_lag, 0.3);
    let blocks = |frames: usize| {
        let mut m = shale_align::Measurer::default();
        m.reserve(n, max_lag);
        let (wx, wy) = m.windows(n, max_lag).unwrap();
        wx.copy_from_slice(&x);
        wy.copy_from_slice(&y);
        m.start(n, max_lag, 0.3);
        let mut count = 0usize;
        loop {
            count += 1;
            if let Some(l) = m.step(measure_budget(frames)) {
                assert_eq!(l, want, "{frames}");
                return count;
            }
        }
    };
    let lags = 2 * max_lag + 1;
    let (b64, b512) = (blocks(64), blocks(512));
    assert_eq!(b512, lags.div_ceil(65_536 / n));
    assert_eq!(b64, lags.div_ceil(8192 / n));
    assert!(b64 >= 7 * b512, "{b64} vs {b512}");
    // A block too small for a whole lag still does one, so a measurement always finishes.
    assert_eq!(blocks(1), lags);
}

#[test]
fn the_report_says_which_member_got_no_slot() {
    // Only a member past the 16 slots is told so; a member with a slot, or one in no group, is not.
    let s = slab();
    let sig = vec![0.0f32; 1024];
    let mut late = linked(&s, shale_link::MAX_INSTANCES, 0.0, 3.0);
    let mut early = linked(&s, 2, 0.0, 0.0);
    play(&mut late, &sig, 0, 512);
    play(&mut early, &sig, 0, 512);
    assert_eq!(late.analysis().unwrap().report()[REPORT_NO_SLOT], 1.0);
    assert_eq!(early.analysis().unwrap().report()[REPORT_NO_SLOT], 0.0);
    late.set_link(None, 0);
    play(&mut late, &sig, 1024, 512);
    assert_eq!(late.analysis().unwrap().report()[REPORT_NO_SLOT], 0.0);
}

/// Runs `src` through `s` block by block, collecting the MIDI it emits at absolute positions.
fn run_midi(s: &mut Shale, src: &[f32], block: usize, tempo: f64) -> Vec<(usize, [u8; 3])> {
    let mut out = Vec::new();
    let mut ring = MidiOut::new();
    let mut buf = vec![vec![0.0; block]; 2];
    let mut pos = 0;
    while pos < src.len() {
        let n = block.min(src.len() - pos);
        for b in buf.iter_mut() {
            b[..n].copy_from_slice(&src[pos..pos + n]);
        }
        s.set_transport(&Transport { tempo, samples: pos as i64, ..Transport::default() });
        s.process(&mut buf, n);
        ring.clear();
        s.midi_out(&mut ring);
        assert_eq!(ring.dropped(), 0);
        out.extend(ring.events().iter().map(|e| (pos + e.offset as usize, e.msg)));
        pos += n;
    }
    out
}

#[test]
fn midi_out_emits_a_note_per_hit_on_channel_10() {
    let src = kit(96_000, 24_000, 0.8, 0.05);
    // Off (the default): nothing, even with hits.
    let mut s = prepared();
    assert!(run_midi(&mut s, &src, 512, 120.0).is_empty());
    // On: a note-on per hit at its offset, and its note-off a sixteenth later (120 bpm: 6000).
    let mut s = prepared();
    assert!(s.set_param("midiOut", 1.0));
    let ev = run_midi(&mut s, &src, 512, 120.0);
    let note = d::knob_to_midi_note(0.1667) as u8;
    let ons: Vec<usize> = ev.iter().filter(|(_, m)| m[0] == 0x99).map(|(at, _)| *at).collect();
    let offs: Vec<usize> = ev.iter().filter(|(_, m)| m[0] == 0x89).map(|(at, _)| *at).collect();
    assert_eq!(ons.len(), 4, "{ev:?}");
    assert_eq!(offs.len(), 4, "{ev:?}");
    for (k, (on, off)) in ons.iter().zip(&offs).enumerate() {
        let hit = k * 24_000;
        assert!(*on >= hit && *on < hit + 600, "note-on {on} for the hit at {hit}");
        assert_eq!(*off, on + 6000, "note-off a sixteenth after {on}");
    }
    for (_, m) in &ev {
        assert_eq!(m[1], note);
        assert!(m[0] != 0x99 || (1..=127).contains(&m[2]), "velocity {}", m[2]);
    }
    // A hit before the sixteenth is up closes the note early: the note-off lands at the next
    // hit, never after it (20 bpm: a sixteenth is 36 000 samples, longer than the 24 000 period).
    let mut s = prepared();
    s.set_param("midiOut", 1.0);
    let ev = run_midi(&mut s, &kit(96_000, 24_000, 0.8, 0.0), 256, 20.0);
    let mut sounding = false;
    for (_, m) in &ev {
        match m[0] {
            0x99 => {
                assert!(!sounding, "two note-ons without a note-off: {ev:?}");
                sounding = true;
            }
            0x89 => sounding = false,
            _ => panic!("{m:?}"),
        }
    }
    assert_eq!(ev.iter().filter(|(_, m)| m[0] == 0x99).count(), 4, "{ev:?}");
    assert!(ev.windows(2).any(|w| w[0].1[0] == 0x89 && w[1].1[0] == 0x99 && w[0].0 == w[1].0), "a note-off at the next hit: {ev:?}");
    // The tempo sets the length: 240 bpm halves it.
    let mut s = prepared();
    s.set_param("midiOut", 1.0);
    let ev = run_midi(&mut s, &kit(24_000, 24_000, 0.8, 0.0), 512, 240.0);
    let on = ev.iter().find(|(_, m)| m[0] == 0x99).map(|(a, _)| *a).unwrap();
    assert_eq!(ev.iter().find(|(_, m)| m[0] == 0x89).map(|(a, _)| *a), Some(on + 3000));
}

#[test]
fn midi_out_switched_off_still_closes_its_note() {
    let mut s = prepared();
    s.set_param("midiOut", 1.0);
    let src = kit(2048, 100_000, 0.8, 0.0);
    let ev = run_midi(&mut s, &src, 512, 120.0);
    assert_eq!(ev.len(), 1, "{ev:?}");
    s.set_param("midiOut", 0.0);
    let ev = run_midi(&mut s, &vec![0.0; 8192], 512, 120.0);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].1[0], 0x89);
}

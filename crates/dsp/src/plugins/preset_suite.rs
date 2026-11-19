//! (Tests) The factory suites of Burley, Cicada and Steeple kept honest: what each preset
//! declares, and what it measurably does when played (through [`super::preset_lab`]).
//!
//! The presets were designed by measurement; tuned by ear later. The first presets of each
//! synth (pinned by the golden fingerprints) keep their sound, so the loudness window applies
//! to the suite's own presets; every preset, old or new, must stay under full scale, never fall
//! silent when held, stay finite and name its macros' targets.

use super::preset_lab::{self as lab, HIGH_CHORD, LOW_CHORD, MID_CHORD, Macros, Note, Preset, State, chord, secs};
use super::synth::presets::{PatchedPreset, Tier};
use super::{fm, synth, wavetable};

/// The suite (the presets added for the release) and the menu's groups of `plugin`.
fn suite_of(plugin: &str) -> (&'static [PatchedPreset], &'static [&'static str]) {
    match plugin {
        "subtractive_synth" => (&synth::suite::SUITE, &synth::suite::GROUPS),
        "wavetable_synth" => (&wavetable::suite::SUITE, &wavetable::suite::GROUPS),
        _ => (&fm::suite::SUITE, &fm::suite::GROUPS),
    }
}

fn is_new(p: &Preset) -> bool {
    suite_of(p.plugin).0.iter().any(|s| s.preset.name == p.name)
}

/// Runs `f` over every preset of every synth on a few threads; returns what it complained of.
fn each(f: impl Fn(&Preset) -> Vec<String> + Sync) -> Vec<String> {
    let all: Vec<Preset> = lab::SYNTHS.iter().flat_map(|id| lab::presets(id)).collect();
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
    let per = all.len().div_ceil(threads);
    let f = &f;
    std::thread::scope(|s| {
        let jobs: Vec<_> = all.chunks(per.max(1)).map(|part| s.spawn(move || part.iter().flat_map(f).collect::<Vec<String>>())).collect();
        jobs.into_iter().flat_map(|j| j.join().unwrap()).collect()
    })
}

fn check(problems: Vec<String>) {
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}

/// Groups whose presets need not play the key's pitch (bells, metal, noise, effects).
fn unpitched(p: &Preset) -> bool {
    matches!(p.group.as_str(), "Bells" | "Percussion" | "Effects" | "Texture") || p.tier == "Wild"
}

#[test]
fn every_synth_has_a_full_suite_in_four_tiers_with_unique_names() {
    for id in lab::SYNTHS {
        let list = lab::presets(id);
        let (suite, groups) = suite_of(id);
        assert_eq!(list.len(), super::fs::factory_presets(id).len(), "{id}: every preset reads");
        assert!(list.len() >= 48, "{id}: {} presets", list.len());
        let mut names = std::collections::HashSet::new();
        for p in &list {
            assert!(names.insert(p.name.to_lowercase()), "{id}: {} twice", p.name);
            assert!(groups.contains(&p.group.as_str()), "{id} {}: group {}", p.name, p.group);
            assert!(Tier::ALL.iter().any(|t| t.name() == p.tier), "{id} {}: tier {}", p.name, p.tier);
            assert!(!p.desc.trim().is_empty(), "{id} {}", p.name);
            for (name, targets) in &p.macros {
                assert!(*targets > 0, "{id} {}: macro {name} drives nothing", p.name);
                assert!(!name.trim().is_empty(), "{id} {}: a macro without a name", p.name);
            }
        }
        for t in Tier::ALL {
            let n = list.iter().filter(|p| p.tier == t.name()).count();
            assert!(n >= 8, "{id}: {n} {} presets", t.name());
        }
        // The menu lists by group, then tier.
        let order: Vec<(usize, usize)> = list
            .iter()
            .map(|p| {
                let g = groups.iter().position(|g| *g == p.group).unwrap_or(usize::MAX);
                (g, Tier::ALL.iter().position(|t| t.name() == p.tier).unwrap_or(usize::MAX))
            })
            .collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "{id}: menu order");
        assert_eq!(list.first().map(|p| p.name.as_str()), Some("Init"), "{id}: Init first");
        let info = crate::plugin_info(id).unwrap();
        for s in suite {
            let p = &s.preset;
            for (k, v) in p.set {
                let pi = info.param(k).unwrap_or_else(|| panic!("{id} {}: {k}", p.name));
                assert_eq!(pi.clamp(*v), *v, "{id} {}: {k}", p.name);
            }
            for (src, d, a) in s.slots {
                assert!(super::synth_mod::check_slot(id, src, d).is_ok() && a.abs() <= 1.0 && *a != 0.0, "{id} {}: {src} -> {d}", p.name);
            }
            for (_, targets) in s.macros {
                for (d, f, to) in *targets {
                    assert!(super::synth_mod::dests(id).contains(d) && f != to, "{id} {}: {d}", p.name);
                }
            }
            // Special and Wild presets set every macro they use to a position of their own;
            // a Wild one's first macro takes it from tame to wild.
            if matches!(p.tier, Tier::Special | Tier::Wild) {
                assert!(!s.macros.is_empty(), "{id} {}: no macros", p.name);
                for i in 0..s.macros.len() {
                    let m = super::synth_mod::MACRO_PARAMS[i];
                    assert!(p.set.iter().any(|(k, _)| *k == m), "{id} {}: {m} not set", p.name);
                }
            }
            assert!(p.set.iter().any(|(k, _)| *k == "level"), "{id} {}: its level is set", p.name);
        }
    }
    // Cicada's suite uses every one of its tables.
    let tables: std::collections::BTreeSet<i32> =
        wavetable::suite::SUITE.iter().filter_map(|s| s.preset.set.iter().find(|(k, _)| *k == "table").map(|(_, v)| *v as i32)).collect();
    // A preset that sets no table plays the Classic one.
    let classic = wavetable::suite::SUITE.iter().any(|s| !s.preset.set.iter().any(|(k, _)| *k == "table"));
    assert_eq!(tables.len() + usize::from(classic && !tables.contains(&0)), wavetable::tables::BANKS.len(), "{tables:?}");
}

/// Levels: the held middle chord at velocity 100 within the window for the suite (or, for a
/// percussive preset, within 4 dB of the peak line), every state's peak under −1 dBFS, no held
/// state silent, nothing infinite, no DC.
#[test]
fn every_preset_plays_at_the_suites_level_in_every_state() {
    let states = [
        State::SAVED,
        State { macros: Macros::Saved, wheel: 1.0 },
        State { macros: Macros::Down, wheel: 0.0 },
        State { macros: Macros::Up, wheel: 0.0 },
        State { macros: Macros::Up, wheel: 1.0 },
    ];
    check(each(|p| {
        let mut out = Vec::new();
        let Some(x) = lab::render(p, State::SAVED, &chord(&MID_CHORD, 100, secs(2.5)), secs(2.5)) else {
            return vec![format!("{}: no synth", p.name)];
        };
        let loud = lab::loudness_db(&x);
        let mut worst = lab::db(lab::peak(&x));
        // DC as saved (audio-rate modulation pushed to its extremes by the macros and the wheel
        // may lean a wave to one side: the report prints the worst state's).
        let dc = lab::dc_db(&x);
        for state in states {
            for keys in [&LOW_CHORD[..], &MID_CHORD, &HIGH_CHORD] {
                let Some(y) = lab::render(p, state, &chord(keys, 127, secs(1.0)), secs(1.0)) else { continue };
                worst = worst.max(lab::db(lab::peak(&y)));
                let l = lab::loudness_db(&y);
                if !(l > -45.0) {
                    out.push(format!("{} {}: {state:?} {keys:?}: {l:.1} dBFS, silent", p.plugin, p.name));
                }
            }
        }
        // The suite's presets stay under −1 dBFS; the first presets (whose sound is pinned)
        // under full scale.
        let line = if is_new(p) { -1.0 } else { 0.0 };
        if !(worst < line) {
            out.push(format!("{} {}: peak {worst:.1} dBFS", p.plugin, p.name));
        }
        if is_new(p) && dc > -30.0 {
            out.push(format!("{} {}: DC {dc:.1} dBFS", p.plugin, p.name));
        }
        let in_window = (-17.0..=-9.0).contains(&loud) || (worst >= -5.0 && loud >= -30.0);
        if is_new(p) && !in_window {
            out.push(format!("{} {}: {loud:.1} dBFS held, peaks {worst:.1}", p.plugin, p.name));
        }
        eprintln!("{} {}: {loud:.1} dBFS, worst peak {worst:.1} dBFS", p.plugin, p.name);
        out
    }));
}

/// Every macro a preset names changes what it plays.
#[test]
fn every_named_macro_changes_the_sound() {
    check(each(|p| {
        let notes = chord(&MID_CHORD, 110, secs(1.0));
        (0..p.macros.len())
            .filter_map(|i| {
                let a = lab::render(p, State { macros: Macros::One(i, 0.0), wheel: 0.0 }, &notes, secs(1.0))?;
                let b = lab::render(p, State { macros: Macros::One(i, 100.0), wheel: 0.0 }, &notes, secs(1.0))?;
                let diff = a.iter().zip(&b).flat_map(|(x, y)| x.iter().zip(y).map(|(u, v)| (u - v).abs())).fold(0.0f32, f32::max);
                (diff < 1e-3).then(|| format!("{} {}: macro {} ({}) does nothing", p.plugin, p.name, i + 1, p.macros[i].0))
            })
            .collect()
    }));
}

/// Pads swell and ring on; plucks start at once; melodic presets play the key.
#[test]
fn pads_swell_plucks_snap_and_melodic_presets_play_the_note() {
    check(each(|p| {
        let mut out = Vec::new();
        let Some(x) = lab::render(p, State::SAVED, &chord(&MID_CHORD, 100, secs(1.5)), secs(1.5)) else { return out };
        let attack = lab::attack_ms(&lab::envelope(&x));
        if p.group == "Pad" && attack < 100.0 {
            out.push(format!("{} {}: a pad reaching half its level in {attack} ms", p.plugin, p.name));
        }
        if p.group == "Pluck" && attack > 20.0 {
            out.push(format!("{} {}: a pluck taking {attack} ms", p.plugin, p.name));
        }
        if p.group == "Pad" {
            let short = lab::render(p, State::SAVED, &[Note { at: 0, len: secs(1.0), key: 60, vel: 100 }], secs(4.0));
            let tail = short.map_or(0.0, |y| lab::tail_ms(&lab::envelope(&y), secs(1.0)));
            if tail < 400.0 {
                out.push(format!("{} {}: a pad gone {tail} ms after the key", p.plugin, p.name));
            }
        }
        if !unpitched(p) {
            let held = lab::render(p, State::SAVED, &[Note { at: 0, len: secs(1.0), key: 60, vel: 100 }], secs(0.5)).map(|y| lab::mono(&y));
            let hz = held.and_then(|h| lab::lowest_partial(h.get(secs(0.05)..).unwrap_or(&[])));
            let cents = hz.map_or(600.0, |hz| lab::cents_off_octave(hz, crate::midi_to_hz(60.0) as f32));
            if cents.abs() > 35.0 {
                out.push(format!("{} {}: plays {hz:?} Hz for middle C ({cents:.0} cents)", p.plugin, p.name));
            }
        }
        out
    }));
}

/// The wild presets stay bounded with every macro and the wheel at full for ten seconds.
#[test]
fn wild_presets_stay_bounded_at_full_tilt() {
    check(each(|p| {
        if p.tier != "Wild" {
            return Vec::new();
        }
        let x = lab::render(p, State { macros: Macros::Up, wheel: 1.0 }, &chord(&MID_CHORD, 127, secs(10.0)), secs(10.0));
        let peak = x.as_ref().map_or(f32::INFINITY, lab::peak);
        let last = x.as_ref().map_or(0.0, |x| lab::rms2(x, secs(9.0), secs(10.0)));
        let mut out = Vec::new();
        if !(peak < 0.891) {
            out.push(format!("{} {}: peak {:.1} dBFS at full tilt", p.plugin, p.name, lab::db(peak)));
        }
        // A held drone or lead keeps sounding (an impact or a key dies away, as it should).
        if matches!(p.group.as_str(), "Texture" | "Lead") && !(lab::db(last) > -50.0) {
            out.push(format!("{} {}: silent after nine seconds at full tilt", p.plugin, p.name));
        }
        out
    }));
}

/// What folds back at high notes stays down in the suite's presets, and what their names and
/// descriptions claim about brightness and movement is measurably so.
#[test]
fn the_suite_keeps_aliasing_down_and_its_claims_true() {
    // (synth, preset, what): "bright" (centroid of a held middle C over 2 kHz), "dark" (under
    // 1.5 kHz) or "moves" (its centroid moves at least half an octave while held).
    const CLAIMS: &[(&str, &str, &str)] = &[
        ("subtractive_synth", "Porch Light Pad", "bright"),
        ("subtractive_synth", "Supersaw Sky", "bright"),
        ("subtractive_synth", "Muddy Reese", "dark"),
        ("subtractive_synth", "Bottomland Bass", "dark"),
        ("subtractive_synth", "Slow Sweep Pad", "moves"),
        ("subtractive_synth", "Dusk Sweep", "moves"),
        ("subtractive_synth", "Heat Haze", "moves"),
        ("subtractive_synth", "Silo Drone", "moves"),
        ("wavetable_synth", "Plain Saw", "bright"),
        ("wavetable_synth", "Morning Fog", "dark"),
        ("wavetable_synth", "Cottonwood Pad", "moves"),
        ("wavetable_synth", "Choir Pad", "moves"),
        ("wavetable_synth", "Kudzu Bloom", "moves"),
        ("wavetable_synth", "Heat Shimmer", "moves"),
        ("wavetable_synth", "Dust Devil", "moves"),
        ("wavetable_synth", "Harmonic Riser", "moves"),
        ("fm_synth", "Triangle Ting", "bright"),
        ("fm_synth", "Soft Tine Piano", "dark"),
        ("fm_synth", "Bell Tower Drone", "moves"),
    ];
    let mut out = each(|p| {
        if !is_new(p) {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut worst = -200.0f32;
        for key in [84u8, 96] {
            let note = [Note { at: 0, len: secs(1.0), key, vel: 100 }];
            let (Some(a), Some(b)) =
                (lab::render_at(p, lab::RATE, State::SAVED, &note, secs(0.5)), lab::render_at(p, lab::RATE * 4.0, State::SAVED, &note, secs(0.5)))
            else {
                continue;
            };
            let (a, b) = (lab::mono(&a), lab::mono(&b));
            worst = worst.max(lab::alias_db(a.get(secs(0.15)..).unwrap_or(&[]), b.get(secs(0.15) * 4..).unwrap_or(&[])));
        }
        if worst > -30.0 {
            out.push(format!("{} {}: aliasing {worst:.1} dB at C6/C7", p.plugin, p.name));
        }
        for (_, _, what) in CLAIMS.iter().filter(|(id, n, _)| *id == p.plugin && *n == p.name) {
            let held = lab::render(p, State::SAVED, &[Note { at: 0, len: secs(3.0), key: 60, vel: 100 }], secs(3.0))
                .map(|y| lab::mono(&y))
                .unwrap_or_default();
            let late = lab::centroid(held.get(secs(1.5)..secs(2.5)).unwrap_or(&[]), lab::RATE).unwrap_or(0.0);
            let moves = lab::movement_oct(&lab::centroid_track(held.get(secs(0.05)..).unwrap_or(&[])));
            let ok = match *what {
                "bright" => late > 2000.0,
                "dark" => late < 1500.0,
                _ => moves >= 0.5,
            };
            if !ok {
                out.push(format!("{} {}: not {what} (centroid {late:.0} Hz, moves {moves:.2} octaves)", p.plugin, p.name));
            }
        }
        out
    });
    // Every claim names a preset that exists.
    for (id, name, _) in CLAIMS {
        if !lab::presets(id).iter().any(|p| p.name == *name) {
            out.push(format!("{id}: no preset {name}"));
        }
    }
    check(out);
}

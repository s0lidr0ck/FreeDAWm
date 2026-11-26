//! Plumb (the real-time insert) against the feasibility study's numbers for the same engines
//! (Studio mode: its "PV-2048" column; Live mode: its "Splice" column, with the study's 70 Hz
//! floor, the Low voice range): the study's synthetic fixtures run through the real module block
//! by block, scored with the Vocal Editor's tracker. Settings are named "Gentle", "Medium", "Hard"
//! (Studio) and "Live Gentle", "Live Medium", "Live Hard". `cargo test -p soundcraft-vocal --test
//! autopitch_quality -- --nocapture` prints the whole table.

mod autopitch_fixtures;

use autopitch_fixtures as fx;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Every fixture at every setting, scored once for all the tests.
fn table() -> &'static BTreeMap<(String, String), fx::Scores> {
    static T: OnceLock<BTreeMap<(String, String), fx::Scores>> = OnceLock::new();
    T.get_or_init(|| {
        let mut out = BTreeMap::new();
        let mut text = String::from(
            "| fixture | setting | centre err c | body rms c | settle ms | settled % | flips/min | vib kept |\n|---|---|---|---|---|---|---|---|\n",
        );
        for name in fx::NAMES {
            let Some(f) = fx::by_name(name) else { continue };
            for (mode, extra) in [("", vec![]), ("Live ", vec![("mode", 1.0), ("range", 0.0)])] {
                for preset in fx::SETTINGS {
                    let mut set = fx::setting(preset);
                    set.extend(extra.iter().copied());
                    let setting = format!("{mode}{preset}");
                    let (y, log, _, _) = fx::run(&f.x, &set);
                    let s = fx::score(&f, &y[0], &log, fx::keeps_vibrato(preset));
                    text.push_str(&format!(
                        "| {name} | {setting} | {:.1} | {:.1} | {} | {:.0} | {:.0} | {} |\n",
                        s.centre_err,
                        s.body_rms,
                        s.settle_med_ms.map_or("-".into(), |v| format!("{v:.0}")),
                        s.settled_pct,
                        s.flips_per_min,
                        s.vib_ret.map_or("-".into(), |v| format!("{:.0} %", v * 100.0)),
                    ));
                    out.insert((name.to_string(), setting), s);
                }
            }
        }
        println!("{text}");
        out
    })
}

fn get(name: &str, setting: &str) -> fx::Scores {
    table().get(&(name.to_string(), setting.to_string())).cloned().unwrap_or_default()
}

#[test]
fn a_note_40_cents_flat_lands_on_the_key() {
    // Study (PV-2048): hard 0.0 c, natural 0.4 c, medium 0.0 c.
    let hard = get("flat40", "Hard");
    assert!(hard.centre_err <= 1.0, "{hard:?}");
    let gentle = get("flat40", "Gentle");
    assert!(gentle.centre_err <= 1.0, "{gentle:?}");
    assert!(get("flat40", "Medium").centre_err <= 1.0);
    for (name, setting) in [("flat40_vib", "Gentle"), ("flat40_vib", "Hard"), ("low_male", "Hard"), ("breathy", "Hard"), ("slides", "Hard")] {
        let s = get(name, setting);
        assert!(s.centre_err <= 2.0 && s.settled_pct == 100.0, "{name} {setting}: {s:?}");
    }
}

#[test]
fn notes_settle_as_fast_as_the_study_measured() {
    // Study (PV-2048, flat40): 5 / 50 / 125 ms (hard / medium / natural).
    let ms = |s: &str| get("flat40", s).settle_med_ms.unwrap_or(f64::INFINITY);
    assert!(ms("Hard") <= 20.0, "{}", ms("Hard"));
    assert!((30.0..=75.0).contains(&ms("Medium")), "{}", ms("Medium"));
    assert!((90.0..=170.0).contains(&ms("Gentle")), "{}", ms("Gentle"));
}

#[test]
fn natural_keeps_the_vibrato_and_hard_tune_flattens_it() {
    // Study (PV-2048): natural 100 %, hard 8-11 %.
    for name in ["flat40_vib", "vib50", "gospel"] {
        let keep = get(name, "Gentle").vib_ret.unwrap_or(0.0);
        assert!(keep >= 0.95, "{name}: {keep}");
        let medium = get(name, "Medium").vib_ret.unwrap_or(0.0);
        assert!(medium >= 0.95, "{name} medium: {medium}");
    }
    for name in ["flat40_vib", "vib50"] {
        let flat = get(name, "Hard").vib_ret.unwrap_or(1.0);
        assert!(flat <= 0.2, "{name}: {flat}");
    }
}

#[test]
fn a_wide_vibrato_next_to_a_semitone_neighbour_does_not_flip() {
    // Study: 244 flips a minute at hard and 75 at medium with its note hold; none here, at every
    // starting point.
    for setting in fx::SETTINGS {
        let s = get("high_female", setting);
        assert_eq!(s.flips_per_min, 0.0, "{setting}: {s:?}");
    }
    for name in fx::NAMES {
        for setting in fx::SETTINGS {
            assert_eq!(get(name, setting).flips_per_min, 0.0, "{name} {setting}");
        }
    }
    // Note hold is what does it: the study's hard-mode hold (12 c) warbles on the same line.
    let f = fx::by_name("high_female").unwrap();
    let low = [("style", fx::CUSTOM), ("retune", 0.0), ("humanize", 0.0), ("vibrato", 0.0), ("hold", 12.0)];
    let (y, log, _, _) = fx::run(&f.x, &low);
    let s = fx::score(&f, &y[0], &log, false);
    println!("high_female, hard with 12 c note hold: {} flips/min", s.flips_per_min);
    assert!(s.flips_per_min > 100.0, "{s:?}");
}

#[test]
fn each_period_keeps_its_shape_and_the_formants_stay() {
    // tuning_quality.rs' bars: period shape <= 0.4 rad; formants within 1 dB of the input's.
    let f = fx::by_name("flat40_vib").unwrap();
    for setting in fx::SETTINGS {
        let (y, _, _, _) = fx::run(&f.x, &fx::setting(setting));
        let (shape, out_db, in_db) = fx::shape_and_formants(&f, &y[0]);
        println!("{setting}: shape {shape:.3} rad, formants {out_db:.2} / {in_db:.2} dB");
        assert!(shape <= 0.4, "{setting}: {shape}");
        assert!(out_db <= in_db + 1.0, "{setting}: {out_db} vs {in_db}");
    }
}

#[test]
fn a_two_microphone_voice_keeps_its_stereo_image() {
    // Study (PV-2048): 0.992-0.995 from 0.999.
    let f = fx::by_name("stereo_2mic").unwrap();
    let n = f.x[0].len();
    let before = fx::coherence(&f.x[0], &f.x[1], 4800, n - 4800);
    for setting in fx::SETTINGS {
        let (y, _, _, _) = fx::run(&f.x, &fx::setting(setting));
        let after = fx::coherence(&y[0], &y[1], 4800, n - 4800);
        println!("{setting}: coherence {before:.4} -> {after:.4}");
        assert!(after >= 0.99, "{setting}: {after}");
    }
}

#[test]
fn every_sample_rate_corrects_the_same() {
    // 44.1 and 96 kHz: the same 43 ms engine (2048 and 4096 points), the same landing.
    for sr in [44_100.0f32, 96_000.0] {
        let secs = 2.0;
        let n = (secs * f64::from(sr)) as usize;
        let mut ph = 0.0f64;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                ph += std::f64::consts::TAU * fx::cents_to_hz(5660.0) / f64::from(sr);
                let e = (i.min(n - 1 - i) as f64 / (0.03 * f64::from(sr))).min(1.0);
                ((1..=10).map(|k| (k as f64 * ph).sin() / k as f64).sum::<f64>() * 0.15 * e) as f32
            })
            .collect();
        let (y, _, d, _) = fx::run_with(&[x], &fx::setting("Hard"), 256, sr);
        assert_eq!(d, if sr > 50_000.0 { 4096 } else { 2048 });
        // Resample-free check: the Vocal Editor's tracker runs at 44.1 and 96 kHz too.
        let t = soundcraft_vocal::track_pitch(&y[0], sr as u32, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        let hop = f64::from(t.hop_samples) / f64::from(sr);
        let vals: Vec<f64> = t
            .frames
            .iter()
            .enumerate()
            .filter(|(i, _)| (0.6..1.6).contains(&(*i as f64 * hop)))
            .filter_map(|(_, f)| f.cents)
            .map(f64::from)
            .collect();
        let mean = vals.iter().sum::<f64>() / vals.len().max(1) as f64;
        assert!((mean - 5700.0).abs() <= 1.5, "{sr}: {mean}");
    }
}

// ------------------------------------------------------------------------------- Live mode

fn live(preset: &str) -> Vec<(&'static str, f32)> {
    let mut set = fx::setting(preset);
    set.extend([("mode", 1.0), ("range", 0.0)]);
    set
}

#[test]
fn live_mode_lands_and_settles_as_the_studys_splice_engine() {
    // Study (Splice): flat40 error 0.0 / 0.4 c (hard / natural); settle 10 / 60 / 135 ms.
    let hard = get("flat40", "Live Hard");
    assert!(hard.centre_err <= 1.0, "{hard:?}");
    assert!(get("flat40", "Live Gentle").centre_err <= 1.0);
    let ms = |s: &str| get("flat40", s).settle_med_ms.unwrap_or(f64::INFINITY);
    assert!(ms("Live Hard") <= 25.0, "{}", ms("Live Hard"));
    assert!((35.0..=85.0).contains(&ms("Live Medium")), "{}", ms("Live Medium"));
    assert!((100.0..=175.0).contains(&ms("Live Gentle")), "{}", ms("Live Gentle"));
    for name in ["low_male", "slides", "breathy"] {
        let s = get(name, "Live Hard");
        assert!(s.centre_err <= 2.0 && s.settled_pct == 100.0, "{name}: {s:?}");
    }
}

#[test]
fn live_mode_keeps_the_vibrato_at_natural_and_does_not_flip() {
    // Study (Splice): natural 101 %; hard keeps about half (it hears the pitch late).
    for name in ["flat40_vib", "vib50", "gospel"] {
        let keep = get(name, "Live Gentle").vib_ret.unwrap_or(0.0);
        assert!(keep >= 0.95, "{name}: {keep}");
    }
    for name in fx::NAMES {
        for preset in fx::SETTINGS {
            let s = get(name, &format!("Live {preset}"));
            assert_eq!(s.flips_per_min, 0.0, "{name} {preset}");
        }
    }
}

#[test]
fn live_mode_keeps_the_stereo_image_and_the_period_shape() {
    // Study (Splice): coherence 0.990-0.996; period shape 0.01-0.03 rad.
    let f = fx::by_name("stereo_2mic").unwrap();
    let n = f.x[0].len();
    for preset in fx::SETTINGS {
        let (y, _, _, _) = fx::run(&f.x, &live(preset));
        let after = fx::coherence(&y[0], &y[1], 4800, n - 4800);
        println!("Live {preset}: coherence {after:.4}");
        assert!(after >= 0.99, "{preset}: {after}");
    }
    let f = fx::by_name("flat40_vib").unwrap();
    for preset in fx::SETTINGS {
        let (y, _, _, _) = fx::run(&f.x, &live(preset));
        let (shape, out_db, in_db) = fx::shape_and_formants(&f, &y[0]);
        println!("Live {preset}: shape {shape:.3} rad, formants {out_db:.2} / {in_db:.2} dB");
        assert!(shape <= 0.4, "{preset}: {shape}");
    }
}

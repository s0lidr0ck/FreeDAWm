//! The factory presets of Burley, Cicada and Steeple measured (see `plugins::preset_lab`): a
//! line per preset and a summary per synth. Designed by measurement; tuned by ear later.
//!
//! `cargo run --release -p soundcraft-dsp --example preset_report [-- <synth id> [name filter]] [--quick] [--calibrate]`
//!
//! `--quick` plays the held chords at hard velocity only; `--calibrate` also prints, per
//! preset, the level that would put it at -13 dBFS with its peaks under -1.5 dBFS. The filter
//! matches a part of a preset's name or its whole group (`patched`).
//!
//! `--patching` counts the presets that patch the matrix and their slots by source and
//! destination; `--slots` scores what each slot does when its source moves
//! (`preset_lab::slot_effect`); `--macros` what each macro does.

use soundcraft_dsp::plugins::preset_lab::{self as lab, HEADER, Report, SYNTHS, line, measure};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let quick = args.iter().any(|a| a == "--quick");
    // Prints `CAL <synth> <preset> <level>` lines: the level each preset would need.
    let calibrate = args.iter().any(|a| a == "--calibrate");
    let words: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let only = words.first().map(|s| s.as_str());
    // `--markdown`: the table of every preset for `docs/synth-presets.md` (each macro measured).
    if args.iter().any(|a| a == "--markdown") {
        println!("| Synth | Tier | Group | Preset | What it is for | Macros (clearly-heard score) |\n|---|---|---|---|---|---|");
        for id in SYNTHS {
            let synth = soundcraft_dsp::plugin_info(id).map_or(id, |i| i.name);
            for p in lab::presets(id) {
                // Each macro with its clearly-audible score (1.0 is the line; see
                // `preset_lab::audible`).
                let macros: Vec<String> =
                    (0..p.macros.len()).map(|i| format!("{} ({:.1})", p.macros[i].0, lab::macro_effect(&p, i).map_or(0.0, |(s, _)| s))).collect();
                let macros = if macros.is_empty() { "–".to_string() } else { macros.join(", ") };
                println!("| {synth} | {} | {} | {} | {} | {macros} |", p.tier, p.group, p.name, p.desc.replace('|', "/"));
            }
        }
        return;
    }
    let filter = words.get(1).map(|s| s.to_lowercase());
    // `--patching`: per synth, how many presets patch the matrix (slots, not only macros), and
    // the slots counted by source and by destination.
    if args.iter().any(|a| a == "--patching") {
        for id in SYNTHS.iter().filter(|id| only.is_none_or(|o| o == **id)) {
            let list = lab::presets(id);
            let count = |min: usize| list.iter().filter(|p| p.slots >= min).count();
            let sources: std::collections::BTreeSet<&str> = list.iter().flat_map(|p| p.slot_list.iter().map(|s| s.0.as_str())).collect();
            println!(
                "{id}: {} presets; with any slot {}, with 3 or more {}, with 3 or more sources {}; slots {}",
                list.len(),
                count(1),
                count(3),
                list.iter().filter(|p| p.slot_list.iter().map(|s| &s.0).collect::<std::collections::BTreeSet<_>>().len() >= 3).count(),
                list.iter().map(|p| p.slots).sum::<usize>()
            );
            for s in sources {
                let n = list.iter().map(|p| p.slot_list.iter().filter(|x| x.0 == s).count()).sum::<usize>();
                let in_presets = list.iter().filter(|p| p.slot_list.iter().any(|x| x.0 == s)).count();
                println!("  source {s}: {n} slots in {in_presets} presets");
            }
            let mut dests: Vec<(String, usize)> = Vec::new();
            for d in list.iter().flat_map(|p| p.slot_list.iter().map(|s| s.1.clone())) {
                match dests.iter_mut().find(|(x, _)| *x == d) {
                    Some((_, n)) => *n += 1,
                    None => dests.push((d, 1)),
                }
            }
            dests.sort_by_key(|d| std::cmp::Reverse(d.1));
            println!("  destinations: {}", dests.iter().map(|(d, n)| format!("{d} {n}")).collect::<Vec<_>>().join(", "));
        }
        return;
    }
    // `--slots`: what each slot of each preset does when its source moves (the `audible` score,
    // see `preset_lab::slot_effect`).
    if args.iter().any(|a| a == "--slots") {
        for id in SYNTHS.iter().filter(|id| only.is_none_or(|o| o == **id)) {
            let list: Vec<lab::Preset> = lab::presets(id)
                .into_iter()
                .filter(|p| filter.as_ref().is_none_or(|f| p.name.to_lowercase().contains(f) || p.group.to_lowercase() == *f))
                .collect();
            let jobs: Vec<(usize, usize)> = list.iter().enumerate().flat_map(|(k, p)| (0..p.slots).map(move |i| (k, i))).collect();
            let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).min(4);
            let mut out: Vec<String> = vec![String::new(); jobs.len()];
            let per = jobs.len().div_ceil(threads).max(1);
            std::thread::scope(|s| {
                for (part, js) in out.chunks_mut(per).zip(jobs.chunks(per)) {
                    let list = &list;
                    s.spawn(move || {
                        for (o, &(k, i)) in part.iter_mut().zip(js) {
                            let p = &list[k];
                            let (src, dest, amount) = &p.slot_list[i];
                            let (score, what) = lab::slot_effect(p, i).unwrap_or((0.0, "no render".into()));
                            let flag = if score < 1.0 { "below" } else { "heard" };
                            *o = format!("SLOT {id} | {} | {} | {src} -> {dest} {amount} | {score:.2} | {flag} | {what}", p.tier, p.name);
                        }
                    });
                }
            });
            out.iter().for_each(|l| println!("{l}"));
        }
        return;
    }
    // `--macros`: what each named macro does between 0 and 100 % (the `audible` score: 1.0 is
    // the clearly-heard line), and whether it follows while a chord is held.
    // `--pitch`: a held middle C's pitch traits with each macro at 0 and 100 % (the tracker's view).
    if args.iter().any(|a| a == "--pitch") {
        for id in SYNTHS.iter().filter(|id| only.is_none_or(|o| o == **id)) {
            for p in
                lab::presets(id).iter().filter(|p| filter.as_ref().is_none_or(|f| p.name.to_lowercase().contains(f) || p.group.to_lowercase() == *f))
            {
                for i in 0..p.macros.len() {
                    let one = [lab::Note { at: 0, len: lab::secs(2.5), key: 60, vel: 100 }];
                    let t = |v: f32| {
                        let x = lab::render(p, lab::State { macros: lab::Macros::One(i, v), wheel: 0.0 }, &one, lab::secs(2.5))?;
                        let m = lab::mono(&x);
                        let track = lab::f0_track(m.get(lab::secs(0.3)..).unwrap_or(&[]), 261.6);
                        let found = track.iter().flatten().count() as f32 / track.len().max(1) as f32;
                        Some((found, lab::pitch_traits(m.get(lab::secs(0.3)..).unwrap_or(&[]), 261.6)))
                    };
                    println!("{} {}: 0 -> {:?}, 100 -> {:?}", p.name, p.macros[i].0, t(0.0), t(100.0));
                }
            }
        }
        return;
    }
    if args.iter().any(|a| a == "--macros") {
        for id in SYNTHS.iter().filter(|id| only.is_none_or(|o| o == **id)) {
            let list: Vec<lab::Preset> = lab::presets(id)
                .into_iter()
                .filter(|p| filter.as_ref().is_none_or(|f| p.name.to_lowercase().contains(f) || p.group.to_lowercase() == *f))
                .collect();
            let jobs: Vec<(usize, usize)> = list.iter().enumerate().flat_map(|(k, p)| (0..p.macros.len()).map(move |i| (k, i))).collect();
            let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).min(4);
            let mut out: Vec<String> = vec![String::new(); jobs.len()];
            let per = jobs.len().div_ceil(threads).max(1);
            std::thread::scope(|s| {
                for (part, js) in out.chunks_mut(per).zip(jobs.chunks(per)) {
                    let list = &list;
                    s.spawn(move || {
                        for (o, &(k, i)) in part.iter_mut().zip(js) {
                            let p = &list[k];
                            let (score, what) = lab::macro_effect(p, i).unwrap_or((0.0, "no render".into()));
                            let during = if args_during() {
                                lab::macro_during(p, i).map_or(String::new(), |(a, b)| format!(" during {a:.2}/{b:.2}"))
                            } else {
                                String::new()
                            };
                            let flag = if score < 1.0 {
                                "FAIL"
                            } else if score < 1.5 {
                                "thin"
                            } else {
                                "ok"
                            };
                            *o = format!("MACRO {id} | {} | {} | {} | {score:.2} | {flag} | {what}{during}", p.tier, p.name, p.macros[i].0);
                        }
                    });
                }
            });
            out.iter().for_each(|l| println!("{l}"));
        }
        return;
    }
    for id in SYNTHS.iter().filter(|id| only.is_none_or(|o| o == **id)) {
        let list = lab::presets(id);
        let base = lab::defaults(id).and_then(|p| measure(&p, false)).map_or(1.0, |r| r.seconds.max(1e-9));
        println!("\n{id}: {} presets\n{HEADER}", list.len());
        // Measured on a few threads at once.
        let chosen: Vec<&lab::Preset> =
            list.iter().filter(|p| filter.as_ref().is_none_or(|f| p.name.to_lowercase().contains(f) || p.group.to_lowercase() == *f)).collect();
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).min(4);
        let mut reports: Vec<Option<Report>> = vec![None; chosen.len()];
        std::thread::scope(|s| {
            for (k, part) in reports.chunks_mut(chosen.len().div_ceil(threads).max(1)).enumerate() {
                let chosen = &chosen;
                s.spawn(move || {
                    let start = k * chosen.len().div_ceil(threads).max(1);
                    for (i, out) in part.iter_mut().enumerate() {
                        *out = chosen.get(start + i).and_then(|p| measure(p, !quick));
                    }
                });
            }
        });
        let mut got = Vec::new();
        for (p, r) in chosen.iter().zip(reports) {
            match r {
                Some(mut r) => {
                    r.cost = (r.seconds / base) as f32;
                    println!("{}", line(&r));
                    if calibrate {
                        // The level that puts the held chord at -13 dBFS with every peak under
                        // -1.6 dBFS (a little under the line, for the machines' maths).
                        let delta = (-13.0 - r.loudness_db).min(-1.6 - r.worst_peak_db.max(r.peak_db));
                        let now = p.value("level").unwrap_or(-6.0);
                        let level = ((now + delta) * 2.0).round() / 2.0;
                        println!("CAL\t{id}\t{}\t{:.1}\tworst peak in {}", p.name, level.clamp(-48.0, 6.0), r.worst_state);
                    }
                    got.push(r);
                }
                None => println!("{}: did not render", p.name),
            }
        }
        summary(&got);
    }
}

fn summary(rs: &[Report]) {
    if rs.is_empty() {
        return;
    }
    let n = rs.len() as f32;
    let mean = rs.iter().map(|r| r.loudness_db).sum::<f32>() / n;
    let sd = (rs.iter().map(|r| (r.loudness_db - mean).powi(2)).sum::<f32>() / n).sqrt();
    let lo = rs.iter().map(|r| r.loudness_db).fold(f32::INFINITY, f32::min);
    let hi = rs.iter().map(|r| r.loudness_db).fold(f32::NEG_INFINITY, f32::max);
    let worst = rs.iter().map(|r| r.worst_peak_db).fold(f32::NEG_INFINITY, f32::max);
    let quiet = rs.iter().map(|r| r.quietest_db).fold(f32::INFINITY, f32::min);
    let alias = rs.iter().map(|r| r.alias_db).fold(f32::NEG_INFINITY, f32::max);
    let cost = rs.iter().map(|r| r.cost).fold(0.0f32, f32::max);
    let over = rs.iter().filter(|r| r.worst_peak_db > -1.0).count();
    let off = rs.iter().filter(|r| (r.loudness_db + 12.0).abs() > 3.0).count();
    let mut tiers: Vec<(String, usize)> = Vec::new();
    for r in rs {
        match tiers.iter_mut().find(|(t, _)| *t == r.tier) {
            Some((_, c)) => *c += 1,
            None => tiers.push((r.tier.clone(), 1)),
        }
    }
    println!(
        "summary: {} presets {tiers:?}; loudness mean {mean:.1} dBFS (sd {sd:.1}, {lo:.1} to {hi:.1}; {off} more than 3 dB from -12); worst peak {worst:.1} dBFS ({over} over -1); quietest state {quiet:.1} dBFS; worst aliasing {alias:.1} dB; most costly {cost:.1} x Init",
        rs.len()
    );
}

/// `--during`: also turn each macro while a chord is held (slower).
fn args_during() -> bool {
    std::env::args().any(|a| a == "--during")
}

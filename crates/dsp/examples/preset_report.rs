//! The factory presets of Burley, Cicada and Steeple measured (see `plugins::preset_lab`): a
//! line per preset and a summary per synth. Designed by measurement; tuned by ear later.
//!
//! `cargo run --release -p soundcraft-dsp --example preset_report [-- <synth id> [name filter]] [--quick] [--calibrate]`
//!
//! `--quick` plays the held chords at hard velocity only; `--calibrate` also prints, per
//! preset, the level that would put it at -13 dBFS with its peaks under -1.5 dBFS.

use soundcraft_dsp::plugins::preset_lab::{self as lab, HEADER, Report, SYNTHS, line, measure};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let quick = args.iter().any(|a| a == "--quick");
    // Prints `CAL <synth> <preset> <level>` lines: the level each preset would need.
    let calibrate = args.iter().any(|a| a == "--calibrate");
    let words: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let only = words.first().map(|s| s.as_str());
    // `--markdown`: the table of every preset for `docs/synth-presets.md`, nothing rendered.
    if args.iter().any(|a| a == "--markdown") {
        println!("| Synth | Tier | Group | Preset | What it is for | Macros |\n|---|---|---|---|---|---|");
        for id in SYNTHS {
            let synth = soundcraft_dsp::plugin_info(id).map_or(id, |i| i.name);
            for p in lab::presets(id) {
                let macros: Vec<&str> = p.macros.iter().map(|(n, _)| n.as_str()).collect();
                let macros = if macros.is_empty() { "–".to_string() } else { macros.join(", ") };
                println!("| {synth} | {} | {} | {} | {} | {macros} |", p.tier, p.group, p.name, p.desc.replace('|', "/"));
            }
        }
        return;
    }
    let filter = words.get(1).map(|s| s.to_lowercase());
    for id in SYNTHS.iter().filter(|id| only.is_none_or(|o| o == **id)) {
        let list = lab::presets(id);
        let base = lab::defaults(id).and_then(|p| measure(&p, false)).map_or(1.0, |r| r.seconds.max(1e-9));
        println!("\n{id}: {} presets\n{HEADER}", list.len());
        // Measured on a few threads at once.
        let chosen: Vec<&lab::Preset> = list.iter().filter(|p| filter.as_ref().is_none_or(|f| p.name.to_lowercase().contains(f))).collect();
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
                        println!("CAL\t{id}\t{}\t{:.1}", p.name, level.clamp(-48.0, 6.0));
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

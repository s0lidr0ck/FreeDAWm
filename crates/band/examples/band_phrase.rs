//! How much each musician repeats himself: the factory song (and the same song with eight-bar sections) played by
//! the Factory Band and by the Gospel Church band with its electric keys on the Hammond (an organ), measured bar by
//! bar (onsets and pitches from the chord's root, velocity and micro-timing ignored).
//!
//! `cargo run --release -p soundcraft-band --example band_phrase -- [classic|natural|band ...] [--take N]`
//!
//! Prints Markdown: per musician, distinct bars of bars played in each eight-bar stretch and over the whole song, and
//! how alike the passes of a section are (onsets in common, 100 % = played identically); each song twice, first
//! with pitches as chord notes (octaves ignored), then with the octave counted.

#[path = "../tests/feel_support/mod.rs"]
mod feel_support;

use feel_support::{Mode, factory_song, phrase_report, play};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut modes = Vec::new();
    let mut take = 1;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "classic" => modes.push(Mode::Classic),
            "natural" => modes.push(Mode::Natural),
            "band" => modes.push(Mode::Band),
            "--take" => {
                take = args.get(i + 1).and_then(|t| t.parse().ok()).unwrap_or(1);
                i += 1;
            }
            other => eprintln!("ignored `{other}`"),
        }
        i += 1;
    }
    if modes.is_empty() {
        modes = vec![Mode::Band];
    }
    let songs = [
        factory_song("Factory Band", false, &[]),
        factory_song("Factory Band", true, &[]),
        factory_song("Gospel Church", false, &[(1, "Hammond B-3X")]),
        factory_song("Gospel Church", true, &[(1, "Hammond B-3X")]),
    ];
    if args.iter().any(|a| a == "--summary") {
        // Takes 1..=8 of every song: per musician, the mean and the most distinct bars in an eight-bar stretch, and
        // how alike the passes are, in each mode.
        println!(
            "| song | musician | {} |",
            modes
                .iter()
                .map(|m| format!("{}: mean (octaves counted) / most / passes alike / bar 5 = bar 1", m.name()))
                .collect::<Vec<_>>()
                .join(" | ")
        );
        println!("|---|---|{}", "---|".repeat(modes.len()));
        for s in &songs {
            let mut rows: Vec<(String, Vec<String>)> = Vec::new();
            for (mi, &mode) in modes.iter().enumerate() {
                let mut per: Vec<(String, Vec<f64>, usize, Vec<f64>, Vec<f64>, Vec<f64>)> = Vec::new();
                for take in 1..=8 {
                    let t = play(s, mode, take);
                    for k in 0..6 {
                        if !t.enabled[k] {
                            continue;
                        }
                        let shapes = feel_support::bar_shapes(&t, k, false);
                        let strict = feel_support::bar_shapes(&t, k, true);
                        let bars = shapes.len();
                        if per.iter().all(|p| p.0 != t.names[k]) {
                            per.push((t.names[k].clone(), Vec::new(), 0, Vec::new(), Vec::new(), Vec::new()));
                        }
                        let Some(p) = per.iter_mut().find(|p| p.0 == t.names[k]) else { continue };
                        for w in (0..bars).step_by(8) {
                            let (d, played) = feel_support::distinct_bars(&shapes, w, (w + 8).min(bars));
                            if played >= 6 {
                                p.1.push(d as f64);
                                p.2 = p.2.max(d);
                                p.5.push(feel_support::distinct_bars(&strict, w, (w + 8).min(bars)).0 as f64);
                            }
                        }
                        p.3.push(feel_support::passes_alike(&t, k));
                        // Eight-bar sections: the second half opens as the first did (bar 5 = bar 1).
                        for (id, starts) in feel_support::section_passes(&t) {
                            if feel_support::section_len(&t, id) == 8 {
                                for &b in &starts {
                                    let (a, c) = (shapes.get(b as usize), shapes.get(b as usize + 4));
                                    if let (Some(a), Some(c)) = (a, c)
                                        && !a.is_empty()
                                    {
                                        p.4.push(if a == c { 1.0 } else { 0.0 });
                                    }
                                }
                            }
                        }
                    }
                }
                for (name, d, most, alike, restated, strict) in per {
                    if mi == 0 {
                        rows.push((name.clone(), Vec::new()));
                    }
                    if let Some(r) = rows.iter_mut().find(|r| r.0 == name) {
                        let rs = if restated.is_empty() { "-".to_string() } else { format!("{:.0}%", feel_support::mean(&restated) * 100.0) };
                        r.1.push(format!(
                            "{:.2} ({:.2}) / {most} / {:.0}% / {rs}",
                            feel_support::mean(&d),
                            feel_support::mean(&strict),
                            feel_support::mean(&alike) * 100.0
                        ));
                    }
                }
            }
            for (name, cells) in rows {
                println!("| {} | {name} | {} |", s.name, cells.join(" | "));
            }
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--bars") {
        // Every bar of one song, musician by musician: `--bars <song 0..3>`.
        let s = args.get(i + 1).and_then(|x| x.parse::<usize>().ok()).and_then(|x| songs.get(x)).unwrap_or(&songs[0]);
        let t = play(s, modes.first().copied().unwrap_or(Mode::Band), take);
        for k in 0..6 {
            if !t.enabled[k] {
                continue;
            }
            println!("## {}", t.names[k]);
            for (b, shape) in feel_support::bar_shapes(&t, k, true).iter().enumerate() {
                println!("bar {:2}: {:?}", b + 1, shape);
            }
        }
        return;
    }
    for &mode in &modes {
        println!("## {} (Take {take})\n", mode.name());
        for s in &songs {
            let t = play(s, mode, take);
            println!("{}", phrase_report(&t, false));
            println!("{}", phrase_report(&t, true));
        }
    }
}

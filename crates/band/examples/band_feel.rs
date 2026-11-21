//! Renders the band over the band-feel lab's test songs (a 12-bar shuffle, a pop verse / chorus / bridge, a slow
//! ballad, a 6/8 tune and a country two-beat) to event lists, Standard MIDI Files and a measured report.
//!
//! `cargo run --release -p soundcraft-band --example band_feel -- <out dir> [classic|natural|band ...] [--take N]`
//!
//! For each song and mode: `<song>-<mode>.mid` (one track per musician, the drummer on channel 10),
//! `<song>-<mode>.txt` (every event: ms, quarter, musician, status, data) and `<song>-<mode>-report.txt`.

#[path = "../tests/feel_support/mod.rs"]
mod feel_support;

use std::fmt::Write;
use std::path::Path;

use feel_support::{Mode, is_drums, play, report, spec_for, test_songs};
use soundcraft_band::offline::smf_bytes;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first() else {
        eprintln!("usage: band_feel <out dir> [classic|natural|band ...] [--take N]");
        std::process::exit(2);
    };
    let mut modes = Vec::new();
    let mut take = 1;
    let mut i = 1;
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
        modes = vec![Mode::Classic, Mode::Natural, Mode::Band];
    }
    let _ = std::fs::create_dir_all(dir);
    for song in test_songs() {
        for &mode in &modes {
            let t = play(&song, mode, take);
            let base = format!("{}-{}", song.name, mode.name());
            let drums = (0..6).find(|&k| is_drums(&t, k) && t.enabled[k]);
            let mid = smf_bytes(&t.events, &spec_for(&song), &t.names, drums, false);
            let mut text = String::new();
            for e in &t.events {
                let _ = writeln!(
                    text,
                    "{:.2}\t{:.4}\tm{}\t{:02X}\t{}\t{}{}",
                    e.ms,
                    e.ppq,
                    e.musician + 1,
                    e.status,
                    e.d1,
                    e.d2,
                    if e.keyswitch { "\tks" } else { "" }
                );
            }
            let path = Path::new(dir);
            let ok = std::fs::write(path.join(format!("{base}.mid")), &mid).is_ok()
                && std::fs::write(path.join(format!("{base}.txt")), &text).is_ok()
                && std::fs::write(path.join(format!("{base}-report.txt")), report(&t)).is_ok();
            println!("{base}: {} events{}", t.events.len(), if ok { "" } else { " (write failed)" });
        }
    }
}

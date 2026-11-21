//! A rough listen to the Band: the band-feel lab's test songs played by the band and rendered through the built-in
//! instruments (the drum machine's synth kit, the FM and subtractive synths) to WAV files.
//!
//! `cargo run --release -p soundcraft-mix --example band_audio -- <out dir> [natural|band|classic ...] [factory]
//! [--tag <name>]`
//!
//! `factory`: the factory song instead (played by the Factory Band, and by the Gospel Church band with its electric
//! keys on the Hammond); `--tag` adds a name to the files (`<song>-<mode>-<tag>.wav`), to keep renders apart.
//!
//! The sounds are stand-ins (a synth piano, a synth bass, a pad for strings, a pluck for guitars, a synth kit): judge
//! the playing (time, dynamics, phrasing), not the instruments. The sustain pedal holds notes; other controllers and
//! keyswitches are ignored; MODO DRUM's extra keys are played on the nearest General MIDI drum.

#[path = "../../band/tests/feel_support/mod.rs"]
mod feel_support;

use std::io::Write;
use std::path::Path;

use feel_support::{Mode, Take, factory_song, is_drums, play, test_songs};
use soundcraft_dsp::plugins::preset_lab::{Note, Preset, RATE, State, defaults, presets, render};

/// The stand-in instrument for a Band instrument index.
fn instrument(inst: i32) -> Option<Preset> {
    let pick = |plugin: &'static str, name: &str| presets(plugin).into_iter().find(|p| p.name == name);
    match inst {
        5 | 11 => defaults("juke"),
        0 => pick("fm_synth", "Tine Piano"),
        3 => pick("fm_synth", "Drawn Organ"),
        2 => pick("fm_synth", "Solid Bass"),
        4 => pick("fm_synth", "Marimba"),
        6 | 10 | 12 => pick("subtractive_synth", "Pluck"),
        7 => pick("subtractive_synth", "Warm Pad"),
        9 => pick("fm_synth", "Brass Section"),
        8 => pick("wavetable_synth", "Pulse Lead"),
        _ => pick("subtractive_synth", "Soft Keys"),
    }
}

/// MODO DRUM's keys on the synth kit's General MIDI pads.
fn gm_drum(n: u8) -> Option<u8> {
    Some(match n {
        35 | 36 => 36,
        37 => 37,
        38 | 40 | 62 | 64 => 38,
        39 => 39,
        22 | 25 | 26 | 27 | 29 | 42 => 42,
        44 | 32 => 44,
        30 | 33 | 34 | 46 => 46,
        41 | 43 | 65 | 67 => 41,
        45 | 47 | 48 | 69 | 71 | 72 => 47,
        50 | 74 => 50,
        49 | 52 | 55 | 57 => 49,
        51 | 53 | 59 => 51,
        54 | 56 | 70 | 75 => n,
        _ => return None,
    })
}

/// One musician's notes as frames at the lab rate, the sustain pedal holding note-offs.
fn musician_notes(t: &Take, k: usize, drums: bool) -> Vec<Note> {
    let frames = |ms: f64| (ms / 1000.0 * f64::from(RATE)).max(0.0) as usize;
    let mut open: Vec<(u8, usize, u8)> = Vec::new();
    let mut held: Vec<(u8, usize, u8)> = Vec::new();
    let mut pedal = false;
    let mut out = Vec::new();
    for e in t.events.iter().filter(|e| e.musician as usize == k && e.musician >= 0 && !e.keyswitch) {
        let at = frames(e.ms);
        match e.status & 0xF0 {
            0xB0 if e.d1 == 64 => {
                let down = e.d2 >= 64;
                if pedal && !down {
                    for (key, on, vel) in held.drain(..) {
                        out.push(Note { at: on, len: at.saturating_sub(on).max(1), key, vel });
                    }
                }
                pedal = down;
            }
            0x90 if e.d2 > 0 => {
                let key = if drums { gm_drum(e.d1) } else { Some(e.d1) };
                if let Some(key) = key {
                    open.push((key, at, e.d2));
                }
            }
            0x80 | 0x90 => {
                let key = if drums { gm_drum(e.d1) } else { Some(e.d1) };
                if let Some(i) = key.and_then(|key| open.iter().position(|o| o.0 == key)) {
                    let o = open.remove(i);
                    if pedal && !drums {
                        held.push(o);
                    } else {
                        out.push(Note { at: o.1, len: at.saturating_sub(o.1).max(1), key: o.0, vel: o.2 });
                    }
                }
            }
            _ => {}
        }
    }
    let end = frames(t.events.last().map_or(0.0, |e| e.ms));
    for (key, on, vel) in open.into_iter().chain(held) {
        out.push(Note { at: on, len: end.saturating_sub(on).max(1), key, vel });
    }
    out
}

fn wav(path: &Path, x: &[Vec<f32>; 2]) -> std::io::Result<()> {
    let n = x[0].len().min(x[1].len());
    let data = (n * 4) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&(RATE as u32).to_le_bytes())?;
    f.write_all(&((RATE as u32) * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data.to_le_bytes())?;
    for i in 0..n {
        for c in x {
            f.write_all(&((c[i].clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first() else {
        eprintln!("usage: band_audio <out dir> [natural|band|classic ...]");
        std::process::exit(2);
    };
    let mut modes: Vec<Mode> = args[1..]
        .iter()
        .filter_map(|a| match a.as_str() {
            "classic" => Some(Mode::Classic),
            "natural" => Some(Mode::Natural),
            "band" => Some(Mode::Band),
            _ => None,
        })
        .collect();
    if modes.is_empty() {
        modes = vec![Mode::Natural, Mode::Band];
    }
    let tag = args.iter().position(|a| a == "--tag").and_then(|i| args.get(i + 1)).map(|t| format!("-{t}")).unwrap_or_default();
    let songs = if args.iter().any(|a| a == "factory") {
        let mut gospel = factory_song("Gospel Church", false, &[(1, "Hammond B-3X")]);
        gospel.name = "factory-song-gospel-church";
        vec![factory_song("Factory Band", false, &[]), gospel]
    } else {
        test_songs()
    };
    let _ = std::fs::create_dir_all(dir);
    for song in songs {
        for &mode in &modes {
            let t = play(&song, mode, 1);
            let len = ((t.events.last().map_or(0.0, |e| e.ms) / 1000.0 + 2.5) * f64::from(RATE)) as usize;
            let mut mix = [vec![0.0f32; len], vec![0.0f32; len]];
            for k in (0..6).filter(|&k| t.enabled[k]) {
                let drums = is_drums(&t, k);
                let Some(p) = instrument(t.instruments[k]) else { continue };
                let notes = musician_notes(&t, k, drums);
                if notes.is_empty() {
                    continue;
                }
                let gain = if drums {
                    0.5
                } else if t.instruments[k] == 2 {
                    0.45
                } else {
                    0.28
                };
                if let Some(x) = render(&p, State::SAVED, &notes, len) {
                    for (m, c) in mix.iter_mut().zip(&x) {
                        for (a, b) in m.iter_mut().zip(c) {
                            *a += b * gain;
                        }
                    }
                }
            }
            let peak = mix.iter().flatten().fold(0.0f32, |a, b| a.max(b.abs()));
            if peak > 0.0 {
                let g = 0.89 / peak;
                mix.iter_mut().flatten().for_each(|x| *x *= g);
            }
            let file = format!("{}-{}{tag}.wav", song.name, mode.name());
            match wav(&Path::new(dir).join(&file), &mix) {
                Ok(()) => println!("wrote {file}"),
                Err(e) => eprintln!("{file}: {e}"),
            }
        }
    }
}

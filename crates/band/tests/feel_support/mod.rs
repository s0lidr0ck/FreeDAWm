//! The band-feel lab: test songs, the bands that play them, and measures of how the band plays (timing against the
//! grid and the drummer, velocity shape, repetition, fills, note lengths, register). Shared by `tests/band_feel.rs`
//! and `examples/band_feel.rs`.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use soundcraft_band::band_preset::factory_band_presets;
use soundcraft_band::library::load::embedded_library;
use soundcraft_band::library::{INSTRUMENT_NAMES, LibraryData};
use soundcraft_band::offline::{RenderSpec, TimedEvent, render};
use soundcraft_band::runtime::BandSettings;
use soundcraft_band::song::{Song, SongChord, SongMusician, SongSection, SongTimeline, build_timeline};
use soundcraft_band::song_feel::{factory_feel_presets, feel_from_preset};

/// How the band is asked to play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Classic,
    Natural,
    /// Natural with the band playing as one (the band-feel layer).
    Band,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Classic => "classic",
            Mode::Natural => "natural",
            Mode::Band => "band",
        }
    }
}

/// A test song: the song, how it is played and by whom.
#[derive(Clone, Debug)]
pub struct TestSong {
    pub name: &'static str,
    pub song: Song,
    pub key: i32,
    pub scale: i32,
    pub bpm: f64,
    pub num: i32,
    pub den: i32,
    /// Bars to play (the song loops when it has no Ending).
    pub bars: i32,
    pub feel: &'static str,
    pub band: &'static str,
}

fn ch(slot: i32, length: i32) -> SongChord {
    SongChord { slot, bass: -1, length }
}

fn section(id: char, name: &str, energy_shift: i32, role: i32, chords: Vec<SongChord>) -> SongSection {
    SongSection { id, name: name.into(), energy_shift, role, chords, musicians: [SongMusician::default(); 6], ..SongSection::default() }
}

/// The five songs the band is judged on.
pub fn test_songs() -> Vec<TestSong> {
    // Slots: degree * 16 + flavour (0 triad, 2 dom7, 1 maj7 ...). In C: 0 C, 48 F, 64 G, 66 G7, 80 Am, 2 C7, 50 F7.
    let blues = Song {
        sections: vec![section(
            'A',
            "Blues",
            0,
            0,
            vec![ch(2, 2), ch(50, 2), ch(2, 2), ch(2, 2), ch(50, 2), ch(50, 2), ch(2, 2), ch(2, 2), ch(66, 2), ch(50, 2), ch(2, 2), ch(66, 2)],
        )],
        arrangement: vec!['A'],
        ..Song::default()
    };
    let pop = Song {
        sections: vec![
            section('A', "Verse", 0, 0, vec![ch(0, 2), ch(64, 2), ch(80, 2), ch(48, 2)]),
            section('B', "Chorus", 1, 0, vec![ch(48, 2), ch(0, 2), ch(64, 2), ch(80, 2)]),
            section('C', "Bridge", -1, 0, vec![ch(80, 2), ch(48, 2), ch(0, 2), ch(64, 2)]),
        ],
        arrangement: vec!['A', 'B', 'A', 'B', 'C', 'B'],
        ..Song::default()
    };
    let ballad = Song {
        sections: vec![
            section('A', "Intro", 0, 1, vec![ch(0, 2), ch(48, 2)]),
            section('B', "Verse", 0, 0, vec![ch(0, 2), ch(80, 2), ch(48, 2), ch(64, 2)]),
            section('C', "Chorus", 1, 0, vec![ch(48, 2), ch(64, 2), ch(80, 2), ch(64, 2)]),
            section('D', "Ending", 0, 2, vec![ch(48, 2), ch(64, 2), ch(0, 4)]),
        ],
        arrangement: vec!['A', 'B', 'C', 'B', 'C', 'D'],
        ..Song::default()
    };
    let six_eight = Song {
        sections: vec![
            section('A', "Verse", 0, 0, vec![ch(0, 2), ch(80, 2), ch(48, 2), ch(64, 2)]),
            section('B', "Chorus", 1, 0, vec![ch(48, 2), ch(0, 2), ch(64, 2), ch(64, 2)]),
        ],
        arrangement: vec!['A', 'B', 'A', 'B'],
        meter_override: 1,
        ..Song::default()
    };
    let country = Song {
        sections: vec![
            section('A', "Verse", 0, 0, vec![ch(0, 2), ch(0, 2), ch(48, 2), ch(0, 2), ch(64, 2), ch(64, 2), ch(0, 2), ch(0, 2)]),
            section('B', "Chorus", 1, 0, vec![ch(48, 2), ch(48, 2), ch(0, 2), ch(0, 2), ch(64, 2), ch(64, 2), ch(0, 2), ch(0, 2)]),
        ],
        arrangement: vec!['A', 'B', 'A', 'B'],
        ..Song::default()
    };
    vec![
        TestSong {
            name: "blues-shuffle",
            song: blues,
            key: 7,
            scale: 0,
            bpm: 100.0,
            num: 4,
            den: 4,
            bars: 24,
            feel: "Blues shuffle",
            band: "Delta Blues",
        },
        TestSong {
            name: "pop-verse-chorus-bridge",
            song: pop,
            key: 0,
            scale: 0,
            bpm: 104.0,
            num: 4,
            den: 4,
            bars: 24,
            feel: "Pop",
            band: "Factory Band",
        },
        TestSong {
            name: "slow-ballad",
            song: ballad,
            key: 5,
            scale: 0,
            bpm: 70.0,
            num: 4,
            den: 4,
            bars: 22,
            feel: "Ballad",
            band: "Acoustic Worship",
        },
        TestSong {
            name: "six-eight",
            song: six_eight,
            key: 2,
            scale: 0,
            bpm: 108.0,
            num: 6,
            den: 8,
            bars: 16,
            feel: "Folk / acoustic",
            band: "Folk Roots",
        },
        TestSong {
            name: "country-two-beat",
            song: country,
            key: 7,
            scale: 0,
            bpm: 168.0,
            num: 4,
            den: 4,
            bars: 32,
            feel: "Country two-step",
            band: "Nashville Country",
        },
    ]
}

/// Targets the lab renders on: the plug-ins that play ordinary notes as written (the owner's MODO, Pianoteq and the
/// Hammond); every other target is played as Generic, so what is measured is what the musician plays.
fn lab_target(preset_target: &str, instrument: &str) -> &'static str {
    match (preset_target, instrument) {
        ("MODO DRUM", _) => "MODO DRUM",
        ("MODO BASS 2", _) => "MODO BASS 2",
        ("Pianoteq", _) | ("Pianoverse", "Piano") => "Pianoteq",
        ("Hammond B-3X", _) => "Hammond B-3X",
        _ => "Generic",
    }
}

/// The settings for a test song: its band (players, Energy, knobs, lab targets), key, Take, song feel and mode.
pub fn settings_for(t: &TestSong, mode: Mode, take: i32, lib: &Arc<LibraryData>) -> BandSettings {
    let mut s = BandSettings::processor_default(Arc::clone(lib));
    let presets = factory_band_presets();
    if let Some(b) = presets.iter().find(|b| b.name == t.band) {
        for (k, m) in b.musicians.iter().enumerate() {
            let p = |f: &str| format!("m{}_{f}", k + 1);
            if let Some(i) = INSTRUMENT_NAMES.iter().position(|n| n.eq_ignore_ascii_case(&m.instrument)) {
                s.set_param(&p("instrument"), i as f32);
            }
            s.set_player(k, &m.player);
            s.set_target(k, lab_target(&m.target, &m.instrument));
            for (f, v) in &m.params {
                if f != "channel" {
                    s.set_param(&p(f), *v);
                }
            }
            s.set_param(&p("enabled"), if m.enabled { 1.0 } else { 0.0 });
            s.set_param(&p("mute"), 0.0);
        }
    }
    let mut song = t.song.clone();
    if let Some(fp) = factory_feel_presets().iter().find(|p| p.name == t.feel) {
        song.feel = feel_from_preset(fp);
    }
    s.song = Some(build_timeline(&song));
    s.set_param("key", t.key as f32);
    s.set_param("scale", t.scale as f32);
    s.set_param("take", take as f32);
    s.feel.natural = mode != Mode::Classic;
    s.feel.tension = mode != Mode::Classic;
    set_band_mode(&mut s, mode == Mode::Band);
    s
}

/// Turns the band-feel layer on or off in a settings value (kept in one place so the lab follows the runtime's field).
pub fn set_band_mode(s: &mut BandSettings, on: bool) {
    s.feel.band = on;
}

pub fn spec_for(t: &TestSong) -> RenderSpec {
    let bar_q = if t.num == 4 { 4.0 } else { 3.0 };
    RenderSpec { bpm: t.bpm, num: t.num, den: t.den, quarters: f64::from(t.bars) * bar_q, ..RenderSpec::default() }
}

/// A test song played in a mode, with its Take.
pub struct Take {
    pub song: TestSong,
    pub timeline: Arc<SongTimeline>,
    pub events: Vec<TimedEvent>,
    pub names: [String; 6],
    /// instrument index per musician (5 = Drums)
    pub instruments: [i32; 6],
    pub enabled: [bool; 6],
}

pub fn play(t: &TestSong, mode: Mode, take: i32) -> Take {
    let lib = embedded_library();
    let s = settings_for(t, mode, take, &lib);
    let mut names: [String; 6] = Default::default();
    let mut instruments = [0; 6];
    let mut enabled = [false; 6];
    for k in 0..6 {
        let inst = s.param(&format!("m{}_instrument", k + 1)).unwrap_or(0.0).round() as i32;
        instruments[k] = inst;
        enabled[k] = s.param(&format!("m{}_enabled", k + 1)).unwrap_or(0.0) > 0.5;
        let player = lib.persona(s.seeds[k]).map(|p| p.name.clone()).unwrap_or_default();
        names[k] = format!("{} ({})", player, INSTRUMENT_NAMES.get(inst as usize).copied().unwrap_or("?"));
    }
    let timeline = s.song.clone().unwrap_or_else(|| build_timeline(&Song::default()));
    let events = render(s, &spec_for(t));
    Take { song: t.clone(), timeline, events, names, instruments, enabled }
}

// ---- measures ----------------------------------------------------------------------------------------------------

/// A played note: onset, length, pitch, velocity, who.
#[derive(Clone, Copy, Debug)]
pub struct Note {
    pub ms: f64,
    pub ppq: f64,
    pub len_ms: f64,
    pub pitch: u8,
    pub vel: u8,
    pub musician: usize,
}

pub fn notes(events: &[TimedEvent]) -> Vec<Note> {
    let mut open: BTreeMap<(i8, u8, u8), Vec<usize>> = BTreeMap::new();
    let mut out: Vec<Note> = Vec::new();
    for e in events {
        if e.musician < 0 {
            continue;
        }
        let key = (e.musician, e.status & 0x0F, e.d1);
        if e.is_note_on() {
            open.entry(key).or_default().push(out.len());
            out.push(Note { ms: e.ms, ppq: e.ppq, len_ms: -1.0, pitch: e.d1, vel: e.d2, musician: e.musician as usize });
        } else if e.is_note_off()
            && let Some(v) = open.get_mut(&key)
            && !v.is_empty()
        {
            let i = v.remove(0);
            if let Some(n) = out.get_mut(i) {
                n.len_ms = e.ms - n.ms;
            }
        }
    }
    out
}

pub fn is_drums(t: &Take, k: usize) -> bool {
    t.instruments.get(k).is_some_and(|&i| i == 5 || i == 11)
}

/// What kind of drum a note is (GM and the MODO DRUM map's extra keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kit {
    Kick,
    Snare,
    Hat,
    Ride,
    Crash,
    Tom,
    Other,
}

pub fn kit(p: u8) -> Kit {
    match p {
        35 | 36 => Kit::Kick,
        37 | 38 | 39 | 40 | 62 | 64 => Kit::Snare,
        22 | 25 | 26 | 27 | 29 | 30 | 32 | 33 | 34 | 42 | 44 | 46 => Kit::Hat,
        51 | 53 | 59 => Kit::Ride,
        49 | 52 | 55 | 57 => Kit::Crash,
        41 | 43 | 45 | 47 | 48 | 50 | 65 | 67 | 69 | 71 | 72 | 74 => Kit::Tom,
        _ => Kit::Other,
    }
}

pub fn ms_per_quarter(t: &Take) -> f64 {
    60000.0 / t.song.bpm
}

pub fn bar_quarters(t: &Take) -> f64 {
    if t.song.num == 4 { 4.0 } else { 3.0 }
}

pub fn mean(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
}

pub fn sd(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = mean(v);
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (v.len() - 1) as f64).sqrt()
}

/// The note's offset from the nearest 16th (ms) and that 16th's index in the bar.
pub fn grid_offset(t: &Take, n: &Note) -> (f64, i64) {
    let q = ms_per_quarter(t) / 4.0;
    let k = (n.ms / q).round();
    let spb = (bar_quarters(t) * 4.0) as i64;
    (n.ms - k * q, (k as i64).rem_euclid(spb))
}

/// Bar index of a note (by its sounding time, nudged so a note a little early still counts in its bar).
pub fn bar_of(t: &Take, n: &Note) -> i64 {
    let bar_ms = ms_per_quarter(t) * bar_quarters(t);
    ((n.ms + 60.0) / bar_ms).floor() as i64
}

/// The bars at which a new arrangement entry (section pass) starts, over the whole render (the song loops).
pub fn section_starts(t: &Take) -> Vec<i64> {
    let tl = &t.timeline;
    let mut starts = Vec::new();
    let total_bars = (tl.total_half.max(1) + 1) / 2;
    let mut last_arr = -1;
    let mut loop_base = 0i64;
    while loop_base < i64::from(t.song.bars) {
        for (i, e) in tl.entries.iter().enumerate() {
            if loop_base > 0 && e.start_half < tl.intro_half {
                continue;
            }
            if e.arr_pos != last_arr || i == 0 {
                starts.push(loop_base + i64::from(e.start_half / 2));
                last_arr = e.arr_pos;
            }
        }
        if tl.has_ending {
            break;
        }
        loop_base += i64::from(total_bars - tl.intro_half / 2);
        last_arr = -1;
    }
    starts.retain(|&b| b < i64::from(t.song.bars));
    starts.dedup();
    starts
}

/// One musician's bar as a fingerprint: (16th, pitch, velocity / 8) for every onset.
fn bar_print(t: &Take, ns: &[&Note], bar: i64) -> Vec<(i64, u8, u8)> {
    let bar_ms = ms_per_quarter(t) * bar_quarters(t);
    let q = ms_per_quarter(t) / 4.0;
    let mut v: Vec<(i64, u8, u8)> =
        ns.iter().filter(|n| bar_of(t, n) == bar).map(|n| (((n.ms - bar as f64 * bar_ms) / q).round() as i64, n.pitch, n.vel / 8)).collect();
    v.sort();
    v
}

/// One musician's bar's rhythm only: the 16ths it plays on.
fn bar_rhythm(t: &Take, ns: &[&Note], bar: i64) -> Vec<i64> {
    let mut v: Vec<i64> = bar_print(t, ns, bar).into_iter().map(|x| x.0).collect();
    v.dedup();
    v
}

/// Repetition: (bars identical to the bar before, of bars compared), (4-bar phrases identical to the phrase before, of
/// phrases compared), distinct bars / bars, rhythm-identical consecutive bars.
pub struct Repetition {
    pub same_bar: (usize, usize),
    pub same_phrase: (usize, usize),
    pub distinct_ratio: f64,
    pub same_rhythm: (usize, usize),
}

pub fn repetition(t: &Take, k: usize) -> Repetition {
    let all = notes(&t.events);
    let ns: Vec<&Note> = all.iter().filter(|n| n.musician == k).collect();
    let bars = i64::from(t.song.bars);
    let prints: Vec<Vec<(i64, u8, u8)>> = (0..bars).map(|b| bar_print(t, &ns, b)).collect();
    let rhythms: Vec<Vec<i64>> = (0..bars).map(|b| bar_rhythm(t, &ns, b)).collect();
    let mut same = 0;
    let mut cmp = 0;
    let mut same_r = 0;
    for b in 1..prints.len() {
        if prints[b].is_empty() || prints[b - 1].is_empty() {
            continue;
        }
        cmp += 1;
        if prints[b] == prints[b - 1] {
            same += 1;
        }
        if rhythms[b] == rhythms[b - 1] {
            same_r += 1;
        }
    }
    let mut psame = 0;
    let mut pcmp = 0;
    let mut p = 4;
    while p + 4 <= prints.len() {
        let a = &prints[p - 4..p];
        let b = &prints[p..p + 4];
        if a.iter().all(|x| !x.is_empty()) && b.iter().all(|x| !x.is_empty()) {
            pcmp += 1;
            if a == b {
                psame += 1;
            }
        }
        p += 4;
    }
    let nonempty: Vec<&Vec<(i64, u8, u8)>> = prints.iter().filter(|x| !x.is_empty()).collect();
    let mut distinct: Vec<&Vec<(i64, u8, u8)>> = nonempty.clone();
    distinct.sort();
    distinct.dedup();
    Repetition {
        same_bar: (same, cmp),
        same_phrase: (psame, pcmp),
        distinct_ratio: if nonempty.is_empty() { 0.0 } else { distinct.len() as f64 / nonempty.len() as f64 },
        same_rhythm: (same_r, cmp),
    }
}

/// Bass onsets against the kick they share a 16th with: (bass - kick) in ms for every pair within 40 ms.
pub fn bass_vs_kick(t: &Take) -> Vec<f64> {
    let all = notes(&t.events);
    let Some(d) = (0..6).find(|&k| is_drums(t, k) && t.enabled[k]) else { return Vec::new() };
    let Some(b) = (0..6).find(|&k| t.instruments[k] == 2 && t.enabled[k]) else { return Vec::new() };
    let kicks: Vec<f64> = all.iter().filter(|n| n.musician == d && kit(n.pitch) == Kit::Kick).map(|n| n.ms).collect();
    let mut out = Vec::new();
    for n in all.iter().filter(|n| n.musician == b) {
        if let Some(&k) = kicks.iter().min_by(|a, c| (*a - n.ms).abs().total_cmp(&(*c - n.ms).abs()))
            && (k - n.ms).abs() < 40.0
        {
            out.push(n.ms - k);
        }
    }
    out
}

/// Every onset of musician `k` against the drummer's nearest onset within 35 ms (ms, + = after the drums).
pub fn vs_drums(t: &Take, k: usize) -> Vec<f64> {
    let all = notes(&t.events);
    let Some(d) = (0..6).find(|&i| is_drums(t, i) && t.enabled[i]) else { return Vec::new() };
    let drums: Vec<f64> = all.iter().filter(|n| n.musician == d).map(|n| n.ms).collect();
    let mut out = Vec::new();
    for n in all.iter().filter(|n| n.musician == k) {
        if let Some(&x) = drums.iter().min_by(|a, c| (*a - n.ms).abs().total_cmp(&(*c - n.ms).abs()))
            && (x - n.ms).abs() < 35.0
        {
            out.push(n.ms - x);
        }
    }
    out
}

/// Mean velocity of musician `k` per bar.
pub fn bar_velocity(t: &Take, k: usize) -> Vec<f64> {
    let all = notes(&t.events);
    (0..i64::from(t.song.bars))
        .map(|b| {
            let v: Vec<f64> = all.iter().filter(|n| n.musician == k && bar_of(t, n) == b).map(|n| f64::from(n.vel)).collect();
            mean(&v)
        })
        .collect()
}

/// The chords a musician strikes: groups of onsets within 45 ms, (first onset, spread ms, pitches low to high in
/// onset order).
pub fn strikes(t: &Take, k: usize) -> Vec<(f64, f64, Vec<u8>)> {
    let all = notes(&t.events);
    let ns: Vec<&Note> = all.iter().filter(|n| n.musician == k).collect();
    let mut out: Vec<(f64, f64, Vec<u8>)> = Vec::new();
    let mut i = 0;
    while i < ns.len() {
        let start = ns[i].ms;
        let mut j = i;
        let mut ps = Vec::new();
        while j < ns.len() && ns[j].ms - start < 45.0 {
            ps.push(ns[j].pitch);
            j += 1;
        }
        let spread = ns[j - 1].ms - start;
        if ps.len() >= 3 {
            out.push((start, spread, ps));
        }
        i = j;
    }
    out
}

/// Drum fills: bars whose tom / snare-run count stands out, and the bars before a section start that have one.
pub fn fill_bars(t: &Take) -> Vec<i64> {
    let all = notes(&t.events);
    let Some(d) = (0..6).find(|&i| is_drums(t, i) && t.enabled[i]) else { return Vec::new() };
    let mut out = Vec::new();
    for b in 0..i64::from(t.song.bars) {
        let toms = all.iter().filter(|n| n.musician == d && bar_of(t, n) == b && kit(n.pitch) == Kit::Tom).count();
        // Snare strokes in the bar's second half off the backbeat (a run, not the groove's 3 and 4).
        let spb = (bar_quarters(t) * 4.0) as i64;
        let snares_late = all
            .iter()
            .filter(|n| {
                n.musician == d && bar_of(t, n) == b && kit(n.pitch) == Kit::Snare && {
                    let (_, s) = grid_offset(t, n);
                    s > spb / 2 && s != 3 * spb / 4
                }
            })
            .count();
        if toms >= 1 || snares_late >= 2 {
            out.push(b);
        }
    }
    out
}

/// The text report of one take: per musician, then the band.
pub fn report(t: &Take) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let all = notes(&t.events);
    let _ = writeln!(
        s,
        "# {}  ({} bpm, {}/{}, {} bars, feel {}, band {})",
        t.song.name, t.song.bpm, t.song.num, t.song.den, t.song.bars, t.song.feel, t.song.band
    );
    let starts = section_starts(t);
    let _ = writeln!(s, "section starts (bar): {starts:?}");
    for k in 0..6 {
        let ns: Vec<&Note> = all.iter().filter(|n| n.musician == k).collect();
        if !t.enabled[k] || ns.is_empty() {
            continue;
        }
        let _ = writeln!(s, "\n## m{} {}", k + 1, t.names[k]);
        let offs: Vec<f64> = ns.iter().map(|n| grid_offset(t, n).0).collect();
        let lo = ns.iter().map(|n| n.pitch).min().unwrap_or(0);
        let hi = ns.iter().map(|n| n.pitch).max().unwrap_or(0);
        let vels: Vec<f64> = ns.iter().map(|n| f64::from(n.vel)).collect();
        let _ = writeln!(s, "notes {}  pitch {lo}..{hi}  vel mean {:.1} sd {:.1}", ns.len(), mean(&vels), sd(&vels));
        let _ = writeln!(
            s,
            "timing vs 16th grid: mean {:+.1} ms, sd {:.1} ms, |mean| {:.1} ms, on grid (<2 ms) {:.0}%",
            mean(&offs),
            sd(&offs),
            mean(&offs.iter().map(|x| x.abs()).collect::<Vec<_>>()),
            100.0 * offs.iter().filter(|x| x.abs() < 2.0).count() as f64 / offs.len().max(1) as f64
        );
        // Per 16th position: mean offset and spread (a player's own push / drag map vs plain jitter).
        let spb = (bar_quarters(t) * 4.0) as i64;
        let mut by_pos: Vec<Vec<f64>> = vec![Vec::new(); spb as usize];
        let mut vel_pos: Vec<Vec<f64>> = vec![Vec::new(); spb as usize];
        for n in &ns {
            let (o, p) = grid_offset(t, n);
            by_pos[p as usize].push(o);
            vel_pos[p as usize].push(f64::from(n.vel));
        }
        let pos_line: Vec<String> = by_pos.iter().map(|v| if v.is_empty() { "  .  ".into() } else { format!("{:+4.0}", mean(v)) }).collect();
        let _ = writeln!(s, "offset by 16th:   {}", pos_line.join(" "));
        let sd_line: Vec<String> = by_pos.iter().map(|v| if v.len() < 2 { "  .  ".into() } else { format!("{:4.1}", sd(v)) }).collect();
        let _ = writeln!(s, "spread by 16th:   {}", sd_line.join(" "));
        let vp: Vec<String> = vel_pos.iter().map(|v| if v.is_empty() { "  . ".into() } else { format!("{:4.0}", mean(v)) }).collect();
        let _ = writeln!(s, "velocity by 16th: {}", vp.join(" "));
        let bv = bar_velocity(t, k);
        let _ = writeln!(s, "velocity by bar:  {}", bv.iter().map(|v| format!("{v:.0}")).collect::<Vec<_>>().join(" "));
        // Phrase shape: mean velocity by bar-in-4-bar-phrase.
        let mut ph = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for (b, v) in bv.iter().enumerate() {
            if *v > 0.0 {
                ph[b % 4].push(*v);
            }
        }
        let _ = writeln!(s, "velocity by bar of 4-bar phrase: {:.1} {:.1} {:.1} {:.1}", mean(&ph[0]), mean(&ph[1]), mean(&ph[2]), mean(&ph[3]));
        let r = repetition(t, k);
        let _ = writeln!(
            s,
            "repetition: bar = previous bar {}/{}, rhythm = previous {}/{}, 4-bar phrase = previous {}/{}, distinct bars {:.0}%",
            r.same_bar.0,
            r.same_bar.1,
            r.same_rhythm.0,
            r.same_rhythm.1,
            r.same_phrase.0,
            r.same_phrase.1,
            r.distinct_ratio * 100.0
        );
        let lens: Vec<f64> = ns.iter().filter(|n| n.len_ms >= 0.0).map(|n| n.len_ms / ms_per_quarter(t)).collect();
        if !lens.is_empty() {
            let mut sorted = lens.clone();
            sorted.sort_by(f64::total_cmp);
            let q = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
            let _ = writeln!(s, "length (quarters): p10 {:.2} median {:.2} p90 {:.2}", q(0.1), q(0.5), q(0.9));
        }
        if !is_drums(t, k) {
            let vd = vs_drums(t, k);
            if !vd.is_empty() {
                let _ = writeln!(s, "vs drummer (shared hits {}): mean {:+.1} ms sd {:.1} ms", vd.len(), mean(&vd), sd(&vd));
            }
            let st = strikes(t, k);
            if !st.is_empty() {
                let spreads: Vec<f64> = st.iter().map(|x| x.1).collect();
                let up = st.iter().filter(|x| x.2.first() < x.2.last()).count();
                let _ = writeln!(
                    s,
                    "chords struck {}: spread mean {:.1} ms (0 ms: {:.0}%), lowest first {:.0}%",
                    st.len(),
                    mean(&spreads),
                    100.0 * spreads.iter().filter(|x| **x < 1.0).count() as f64 / spreads.len() as f64,
                    100.0 * up as f64 / st.len() as f64
                );
                // Voice leading: mean |movement| of the nearest pitch between successive chords.
                let mut mv = Vec::new();
                for w in st.windows(2) {
                    let (a, b) = (&w[0].2, &w[1].2);
                    for &p in b {
                        let d = a.iter().map(|&x| (i32::from(x) - i32::from(p)).abs()).min().unwrap_or(0);
                        mv.push(f64::from(d));
                    }
                }
                let _ = writeln!(s, "voice leading: mean move to nearest previous note {:.2} semitones", mean(&mv));
            }
        } else {
            let mut by_kit: BTreeMap<Kit, Vec<f64>> = BTreeMap::new();
            for n in &ns {
                by_kit.entry(kit(n.pitch)).or_default().push(f64::from(n.vel));
            }
            for (kk, v) in &by_kit {
                let ghosts = v.iter().filter(|x| **x < 50.0).count();
                let _ = writeln!(s, "  {kk:?}: {} hits, vel mean {:.0} sd {:.1}, ghosts(<50) {ghosts}", v.len(), mean(v), sd(v));
            }
            let mut pitches: BTreeMap<u8, usize> = BTreeMap::new();
            for n in &ns {
                *pitches.entry(n.pitch).or_default() += 1;
            }
            let _ = writeln!(s, "  keys used: {pitches:?}");
            let fills = fill_bars(t);
            let before: Vec<i64> = starts.iter().map(|b| b - 1).filter(|b| *b >= 0).collect();
            let at_end = fills.iter().filter(|b| before.contains(b)).count();
            let _ = writeln!(s, "  fill bars {fills:?}; {at_end} of {} sit just before a section start", fills.len());
            let crashes: Vec<i64> = ns.iter().filter(|n| kit(n.pitch) == Kit::Crash).map(|n| bar_of(t, n)).collect();
            let _ = writeln!(s, "  crash bars {crashes:?}");
        }
    }
    let bk = bass_vs_kick(t);
    if !bk.is_empty() {
        let _ = writeln!(
            s,
            "\n## band\nbass vs kick on shared hits ({}): mean {:+.1} ms, sd {:.1} ms, within 5 ms {:.0}%",
            bk.len(),
            mean(&bk),
            sd(&bk),
            100.0 * bk.iter().filter(|x| x.abs() <= 5.0).count() as f64 / bk.len() as f64
        );
    }
    // Section downbeats: who lands within 30 ms of the bar line.
    let bar_ms = ms_per_quarter(t) * bar_quarters(t);
    let mut land = Vec::new();
    for b in &starts {
        let at = *b as f64 * bar_ms;
        let who: Vec<usize> = (0..6).filter(|&k| all.iter().any(|n| n.musician == k && (n.ms - at).abs() < 30.0)).collect();
        land.push(format!("bar {b}: {who:?}"));
    }
    let _ = writeln!(s, "section downbeats played by: {}", land.join("; "));
    // Collisions: two musicians (not drums) sounding a minor 2nd apart inside the same octave band at the same time.
    let mut clashes = 0;
    let mut pairs = 0;
    let melodic: Vec<&Note> = all.iter().filter(|n| !is_drums(t, n.musician) && n.len_ms > 0.0).collect();
    for (i, a) in melodic.iter().enumerate() {
        for b in melodic.iter().skip(i + 1) {
            if b.ms > a.ms + a.len_ms {
                break;
            }
            if a.musician != b.musician && b.ms < a.ms + a.len_ms && a.ms < b.ms + b.len_ms.max(1.0) {
                pairs += 1;
                let d = (i32::from(a.pitch) - i32::from(b.pitch)).abs();
                if d == 1 || d == 13 || d == 0 {
                    clashes += 1;
                }
            }
        }
    }
    let _ = writeln!(s, "overlapping notes between players {pairs}: unisons and minor 2nds/9ths {clashes}");
    s
}

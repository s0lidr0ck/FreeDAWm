//! Port of `MidiChordImport.h` / `MidiChordImport.cpp`.
//!
//! MidiChordImport — chords from a MIDI file (Phase 5c): a chord per half-bar (and the key), merged and
//! split into sections at the file's markers (or every 8 bars). A preview only: nothing changes until accepted.
//!
//! The file is read with `soundcraft_midi::read_smf` instead of `juce::MidiFile`. Differences, all on unusual files:
//! - Only Marker meta events (type 6) are seen; JUCE also read Cue Points (type 7), which `read_smf` drops.
//! - Ticks come rescaled to 960 PPQ (exact for the usual divisions).
//! - A repeated note-on of a sounding pitch is paired first-in first-out, and a note-on with no note-off ends at
//!   the track's end (JUCE: the next note-on of that pitch ends it, an unmatched one sounds half a quarter).
//! - The first time signature is the earliest one in any track (JUCE: the first in track order).
//! - Notes are sorted stably by start (C++ `std::sort`, which is not stable).
//!
//! Never-crash: the per-half-bar table starts at the first note's half bar, so a file whose notes start very
//! late cannot make it huge (the C++ allocated every half bar from 0).

use soundcraft_midi::{PPQ, read_smf};

use crate::chord_match::{MatchResult, match_profile};
use crate::meter::Meter;
use crate::song::{MAX_CHORDS, MAX_SECTIONS, Song, SongChord, SongSection};

const K_MAX_SECTIONS: usize = MAX_SECTIONS as usize;
const K_MAX_CHORDS: usize = MAX_CHORDS as usize;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportSection {
    pub name: String,
    pub chords: Vec<SongChord>,
    pub snapped: Vec<bool>,
}

/// The C++ `ok` / `error` pair is the `Result` of [`preview_midi_import`].
#[derive(Clone, Debug, PartialEq)]
pub struct ImportPreview {
    pub key: i32,
    pub scale: i32,
    /// a 3/4 or 6/8 file sets the song's meter (a Meter); -1 = follow the host
    pub meter_override: i32,
    pub sections: Vec<ImportSection>,
    /// "Read as 4/4 (the file is in 3/4)", "Only the first 8 sections..."
    pub notes: Vec<String>,
}

impl Default for ImportPreview {
    fn default() -> Self {
        Self { key: 0, scale: 0, meter_override: -1, sections: Vec::new(), notes: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug)]
struct Note {
    start: f64,
    end: f64,
    pitch: i32,
    vel: f32,
}

/// Krumhansl–Kessler: the key whose profile best correlates with how long each pitch class sounds.
fn detect_key(hist: &[f64; 12]) -> (i32, i32) {
    const MAJOR: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MINOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let corr = |prof: &[f64; 12], key: usize| -> f64 {
        let mut mx = 0.0;
        let mut my = 0.0;
        for i in 0..12 {
            mx += hist[i];
            my += prof[i];
        }
        mx /= 12.0;
        my /= 12.0;
        let mut sxy = 0.0;
        let mut sxx = 0.0;
        let mut syy = 0.0;
        for i in 0..12 {
            let x = hist[(i + key) % 12] - mx;
            let y = prof[i] - my;
            sxy += x * y;
            sxx += x * x;
            syy += y * y;
        }
        if sxx > 0.0 && syy > 0.0 { sxy / (sxx * syy).sqrt() } else { -1.0 }
    };
    let mut best = (0, 0);
    let mut best_r = -2.0;
    for k in 0..12 {
        for sc in 0..2 {
            let r = corr(if sc == 0 { &MAJOR } else { &MINOR }, k);
            if r > best_r + 1.0e-9 {
                best_r = r;
                best = (k as i32, sc);
            }
        }
    }
    best
}

/// JUCE `String::trim`: whitespace off both ends.
fn juce_trim(s: &str) -> String {
    s.trim().to_string()
}

pub fn preview_midi_import(bytes: &[u8], key_override: Option<(i32, i32)>) -> Result<ImportPreview, String> {
    let mut p = ImportPreview::default();
    let Ok(mf) = read_smf(bytes) else { return Err("This isn't a MIDI file MIDI Studio can read.".into()) };
    // The header's division (bytes 12..14): its top bit set is SMPTE time (JUCE's negative time format).
    let smpte = bytes.get(12).is_some_and(|b| b & 0x80 != 0);
    if smpte {
        return Err("This MIDI file uses SMPTE time, which can't be read as bars.".into());
    }
    let tpq = f64::from(PPQ);

    let mut notes: Vec<Note> = Vec::new();
    let mut markers: Vec<(f64, String)> = Vec::new();
    let (mut num, mut den) = (4, 4);
    if let Some(&(_, n, d)) = mf.meters.first() {
        num = i32::from(n);
        den = i32::from(d);
    }
    for (tick, text) in &mf.markers {
        markers.push((*tick as f64 / tpq, juce_trim(text)));
    }
    for track in &mf.tracks {
        for n in &track.sequence.notes {
            if n.channel == 9 {
                continue; // MIDI channel 10: drums
            }
            let ppq = n.start as f64 / tpq;
            let end = n.end() as f64 / tpq;
            if end > ppq {
                notes.push(Note { start: ppq, end, pitch: i32::from(n.pitch), vel: f32::from(n.velocity) * (1.0f32 / 127.0f32) });
            }
        }
    }
    if notes.is_empty() {
        return Err("This MIDI file has no notes to read chords from.".into());
    }
    // 3/4 and 6/8 set the song's meter and are read in 3-quarter bars; any other meter is read as 4/4.
    let twelve = (num == 3 && den == 4) || (num == 6 && den == 8);
    let hq = if twelve { 1.5 } else { 2.0 }; // a half-bar, in quarter notes
    if twelve {
        p.meter_override = if num == 6 { Meter::M68 as i32 } else { Meter::M34 as i32 };
        p.notes.push(format!("Song set to {num}/{den} (from the file)."));
    } else if num != 4 || den != 4 {
        p.notes.push(format!("Read as 4/4 (the file is in {num}/{den})."));
    }

    // The key, from how long each pitch class sounds.
    let mut hist = [0.0f64; 12];
    let mut end_ppq = 0.0f64;
    for n in &notes {
        hist[(n.pitch % 12) as usize] += (n.end - n.start) * (0.5 + 0.5 * f64::from(n.vel));
        end_ppq = if end_ppq < n.end { n.end } else { end_ppq };
    }
    let key = key_override.unwrap_or_else(|| detect_key(&hist));
    p.key = key.0;
    p.scale = key.1;

    // A chord per half-bar (2 quarter notes; 1.5 in 3/4 and 6/8): weighted pitch classes and the lowest note sounding.
    // A song holds at most 8 sections x 32 chords of up to 2 bars: nothing past that can be used.
    notes.sort_by(|x, y| x.start.total_cmp(&y.start));
    let first_half = (notes[0].start / hq + 1.0e-9).floor() as i64;
    const K_MAX_HALVES: i64 = (K_MAX_SECTIONS * K_MAX_CHORDS * 4) as i64;
    let mut halves = (end_ppq / hq - 1.0e-9).ceil() as i64;
    if halves - first_half > K_MAX_HALVES {
        halves = first_half + K_MAX_HALVES;
        p.notes.push(format!("Only the first {} bars were read (the most a song can hold).", K_MAX_HALVES / 2));
    }
    // half[h - base]: no note sounds in a half bar before the first note's (C++: the table from 0).
    let base = first_half.clamp(0, halves.max(0));
    let mut half: Vec<Option<MatchResult>> = vec![None; usize::try_from(halves - base).unwrap_or(0)];
    let at = |half: &Vec<Option<MatchResult>>, h: i64| -> Option<MatchResult> {
        if h < base { None } else { usize::try_from(h - base).ok().and_then(|i| half.get(i).copied().flatten()) }
    };
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0usize;
    for h in base..halves {
        let a = h as f64 * hq;
        let b = a + hq;
        while next < notes.len() && notes[next].start < b {
            active.push(next);
            next += 1;
        }
        active.retain(|&i| notes[i].end > a + 1.0e-6);
        let mut w = [0.0f32; 12];
        let mut lowest = 128;
        let mut any_lowest = 128;
        for &i in &active {
            let n = notes[i];
            let hi = if b < n.end { b } else { n.end };
            let lo = if a < n.start { n.start } else { a };
            let ov = hi - lo;
            if ov <= 1.0e-6 {
                continue;
            }
            w[(n.pitch % 12) as usize] += (ov * (0.5 + 0.5 * f64::from(n.vel))) as f32;
            if ov >= 0.25 && n.pitch < lowest {
                lowest = n.pitch;
            }
            any_lowest = any_lowest.min(n.pitch);
        }
        let mut total = 0.0f32;
        for x in w {
            total += x;
        }
        if total <= 1.0e-6 {
            continue;
        }
        if lowest == 128 {
            lowest = any_lowest;
        }
        if let Some(slot) = usize::try_from(h - base).ok().and_then(|i| half.get_mut(i)) {
            *slot = Some(match_profile(&w, lowest % 12, p.key, p.scale));
        }
    }
    // Leading silence goes; any other silent half-bar continues the chord before it.
    let mut first = base;
    while first < halves && at(&half, first).is_none() {
        first += 1;
    }
    for h in (first + 1)..halves {
        if at(&half, h).is_none() {
            let prev = at(&half, h - 1);
            if let Some(slot) = usize::try_from(h - base).ok().and_then(|i| half.get_mut(i)) {
                *slot = prev;
            }
        }
    }

    // Section boundaries (half-bar indices): the markers, else every 8 bars.
    let mut bounds: Vec<(i64, String)> = Vec::new();
    markers.sort_by(|x, y| x.0.total_cmp(&y.0));
    for (ppq, name) in markers {
        let h = ((ppq / hq).round() as i64).clamp(0, halves.max(0));
        if h >= halves {
            continue;
        }
        if let Some(last) = bounds.last_mut()
            && last.0 == h
        {
            last.1 = name;
            continue;
        }
        bounds.push((h, name));
    }
    if bounds.is_empty() {
        let mut h = first;
        let mut i = 1;
        while h < halves {
            bounds.push((h, format!("Part {i}")));
            h += 16;
            i += 1;
        }
    }
    if let Some(front) = bounds.first_mut()
        && front.0 > first
    {
        front.0 = first; // chords before the first marker belong to it
    }
    if bounds.len() > K_MAX_SECTIONS {
        p.notes.push(format!("Only the first 8 sections were imported (the file has {}).", bounds.len()));
        bounds.truncate(K_MAX_SECTIONS + 1); // the 9th start ends the 8th section
    }

    for si in 0..bounds.len().min(K_MAX_SECTIONS) {
        let a = first.max(bounds[si].0);
        let b = if si + 1 < bounds.len() { bounds[si + 1].0 } else { halves };
        let mut sec = ImportSection {
            name: if !bounds[si].1.is_empty() { bounds[si].1.clone() } else { format!("Part {}", si + 1) },
            ..ImportSection::default()
        };
        let mut h = a;
        while h < b {
            let Some(c) = at(&half, h) else {
                h += 1;
                continue;
            };
            let mut run = 1;
            let mut exact = c.exact;
            while h + run < b {
                let Some(n) = at(&half, h + run) else { break };
                if n.slot != c.slot || n.bass != c.bass {
                    break;
                }
                exact = exact && n.exact;
                run += 1;
            }
            h += run;
            while run > 0 {
                let len = if run >= 4 {
                    4
                } else if run >= 2 {
                    2
                } else {
                    1
                };
                sec.chords.push(SongChord { slot: c.slot, bass: c.bass, length: len as i32 });
                sec.snapped.push(!exact);
                run -= len;
            }
        }
        if sec.chords.len() > K_MAX_CHORDS {
            p.notes.push(format!("{}: only the first 32 chords were imported.", sec.name));
            sec.chords.truncate(K_MAX_CHORDS);
            sec.snapped.truncate(K_MAX_CHORDS);
        }
        if !sec.chords.is_empty() {
            p.sections.push(sec);
        }
    }
    if p.sections.is_empty() {
        return Err("No chords could be read from this file.".into());
    }
    Ok(p)
}

/// A new song: sections A.. in order, the arrangement plays them once each.
pub fn song_from_preview(p: &ImportPreview) -> Song {
    let mut s = Song { meter_override: p.meter_override, ..Song::default() };
    for (i, ps) in p.sections.iter().take(K_MAX_SECTIONS).enumerate() {
        let sec = SongSection { id: char::from(b'A' + i as u8), name: ps.name.clone(), chords: ps.chords.clone(), ..SongSection::default() };
        s.arrangement.push(sec.id);
        s.sections.push(sec);
    }
    s
}

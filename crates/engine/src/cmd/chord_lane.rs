//! Detected chords onto the Band's chord lane (Detect Chords, `ml.chords`).
//!
//! The detector's segments (seconds from the source's start, chord names like `Em`, `Cmaj7/B`) are
//! put on the session's tempo map and snapped to a grid of half bars (default) or bars: each grid
//! cell takes the chord heard longest in it (none when chords cover less than half of it), and
//! equal neighbours merge into one chord. "As played" ([`Grid::Free`]) keeps each change where it
//! was heard instead: changes closer than an eighth of a bar merge (the later chord takes the
//! earlier change's time) and each chord runs to the next change. Names become key-relative slots with
//! `chord_match::match_chord` in the band's key; with `key: auto` and a lane holding no other
//! chords, the key is first estimated from the detected chords (Krumhansl-Kessler profiles over
//! the chord tones, weighted by length) and set on the band. "Replace" clears the lane over the
//! source's cells first; "fill" puts chords only where the lane is empty.

use super::band::{carve, take_id};
use serde_json::{Value, json};
use soundcraft_band::chord_match::{match_chord, parse_chord_name};
use soundcraft_mix::half_ticks;
use soundcraft_model::{ChordEvent, MAX_CHORDS, Session};
use soundcraft_time::Samples;

/// A detected chord: seconds from the source's start, and its name (`Em`, `Cmaj7/B`).
#[derive(Debug, Clone, PartialEq)]
pub struct TimedChord {
    pub start: f64,
    pub end: f64,
    pub name: String,
}

/// The grid detected chords snap to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grid {
    /// Half a bar of the meter the band plays (the lane's own resolution).
    #[default]
    HalfBar,
    Bar,
    /// As played: no grid; each detected change at its own time.
    Free,
}

impl Grid {
    pub fn parse(s: &str) -> Option<Grid> {
        match s.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "half_bar" | "half" | "" => Some(Grid::HalfBar),
            "bar" => Some(Grid::Bar),
            "free" | "as_played" | "played" => Some(Grid::Free),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Grid::HalfBar => "half_bar",
            Grid::Bar => "bar",
            Grid::Free => "free",
        }
    }
}

/// Which key the chords are matched in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyChoice {
    /// Estimated from the chords and set on the band when the lane holds no other chords; else
    /// the band's key.
    #[default]
    Auto,
    /// The band's key as it is.
    Keep,
    /// This key (0..=11) and scale (0 major, 1 minor), set on the band.
    Set(u8, u8),
}

/// How detected chords go onto the lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LaneOptions {
    pub grid: Grid,
    /// Replace the lane's chords over the source's range (else only fill empty places).
    pub replace: bool,
    pub key: KeyChoice,
}

const KEY_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// `key` (`auto`, `keep`, `C`..`B`, `Db`… or 0..=11) and `scale` (`major`, `minor`).
pub fn key_choice(p: &Value) -> Result<KeyChoice, String> {
    let scale = match p.get("scale").and_then(Value::as_str).map(|s| s.trim().to_ascii_lowercase()) {
        None => 0,
        Some(s) if s == "major" || s == "maj" => 0,
        Some(s) if s == "minor" || s == "min" => 1,
        Some(s) => return Err(format!("`scale` must be major or minor (got {s})")),
    };
    match p.get("key") {
        None | Some(Value::Null) => Ok(KeyChoice::Auto),
        Some(Value::String(s)) if s.eq_ignore_ascii_case("auto") => Ok(KeyChoice::Auto),
        Some(Value::String(s)) if s.eq_ignore_ascii_case("keep") || s.eq_ignore_ascii_case("band") => Ok(KeyChoice::Keep),
        Some(Value::String(s)) => {
            const FLATS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
            let t = s.trim();
            match KEY_NAMES.iter().position(|k| k.eq_ignore_ascii_case(t)).or_else(|| FLATS.iter().position(|k| k.eq_ignore_ascii_case(t))) {
                Some(k) => Ok(KeyChoice::Set(k as u8, scale)),
                None => Err(format!("`key` must be auto, keep, a note name or 0-11 (got {s})")),
            }
        }
        Some(v) => match v.as_u64() {
            Some(n) if n <= 11 => Ok(KeyChoice::Set(n as u8, scale)),
            _ => Err(format!("`key` must be auto, keep, a note name or 0-11 (got {v})")),
        },
    }
}

/// The key of a set of chords: `(tonic 0..=11, scale 0 major / 1 minor)`, from the duration-weighted
/// chord tones (roots counted twice) against the Krumhansl-Kessler profiles.
pub fn estimate_key(chords: &[(i64, String)]) -> Option<(u8, u8)> {
    const MAJOR: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MINOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let mut h = [0.0f64; 12];
    for (len, name) in chords {
        let Some(c) = parse_chord_name(name) else { continue };
        let w = *len as f64;
        for (i, on) in c.pcs.iter().enumerate() {
            if *on {
                h[i] += w;
            }
        }
        if let Some(r) = usize::try_from(c.root_pc).ok().and_then(|r| h.get_mut(r)) {
            *r += w;
        }
    }
    if h.iter().sum::<f64>() <= 0.0 {
        return None;
    }
    let corr = |prof: &[f64; 12], tonic: usize| -> f64 {
        let x: Vec<f64> = (0..12).map(|i| h[(i + tonic) % 12]).collect();
        let (mx, mp) = (x.iter().sum::<f64>() / 12.0, prof.iter().sum::<f64>() / 12.0);
        let (mut num, mut dx, mut dp) = (0.0, 0.0, 0.0);
        for i in 0..12 {
            num += (x[i] - mx) * (prof[i] - mp);
            dx += (x[i] - mx).powi(2);
            dp += (prof[i] - mp).powi(2);
        }
        if dx <= 0.0 { 0.0 } else { num / (dx * dp).sqrt() }
    };
    let mut best = (0u8, 0u8, f64::NEG_INFINITY);
    for tonic in 0..12 {
        for (scale, prof) in [(0u8, &MAJOR), (1u8, &MINOR)] {
            let c = corr(prof, tonic);
            if c > best.2 {
                best = (tonic as u8, scale, c);
            }
        }
    }
    Some((best.0, best.1))
}

/// The chords snapped to the grid: `(start tick, end tick, name)`, equal neighbours merged, over
/// the cells `first..last` of the source `start..start+len`. Also returns the cells' span.
pub fn snap_chords(s: &Session, start: Samples, len: Samples, chords: &[TimedChord], grid: Grid) -> (Vec<(i64, i64, String)>, (i64, i64)) {
    if grid == Grid::Free {
        return as_played(s, start, len, chords);
    }
    let sr = s.sample_rate;
    let rate = sr.as_f64();
    let half = half_ticks(s).max(1);
    let cell = if grid == Grid::Bar { half.saturating_mul(2) } else { half };
    let end = start.saturating_add(len.max(0));
    let tick = |at: Samples| s.tempo.samples_to_ticks(at, sr);
    let (a_t, b_t) = (tick(start), tick(end));
    let at = |secs: f64| -> Samples {
        if secs.is_finite() { start.saturating_add((secs * rate).round().clamp(0.0, len.max(0) as f64) as Samples) } else { start }
    };
    let spans: Vec<(i64, i64, &str)> = chords
        .iter()
        .filter(|c| !c.name.is_empty())
        .map(|c| (tick(at(c.start)), tick(at(c.end)), c.name.as_str()))
        .filter(|(a, b, _)| b > a)
        .collect();
    let k0 = a_t.div_euclid(cell);
    let k1 = b_t.saturating_add(cell - 1).div_euclid(cell);
    let mut cells: Vec<(i64, i64, Option<&str>)> = Vec::new();
    for k in k0..k1.max(k0) {
        let (ca, cb) = (k.saturating_mul(cell), k.saturating_add(1).saturating_mul(cell));
        let (ea, eb) = (ca.max(a_t), cb.min(b_t));
        if eb <= ea {
            continue;
        }
        // Overlap per name (first heard wins a tie).
        let mut by: Vec<(&str, i64)> = Vec::new();
        for (a, b, name) in &spans {
            let o = (*b).min(eb) - (*a).max(ea);
            if o > 0 {
                match by.iter_mut().find(|(n, _)| n == name) {
                    Some((_, v)) => *v += o,
                    None => by.push((name, o)),
                }
            }
        }
        let total: i64 = by.iter().map(|(_, v)| *v).sum();
        let pick = by.iter().fold(None::<(&str, i64)>, |best, &(n, v)| if best.is_none_or(|b| v > b.1) { Some((n, v)) } else { best });
        let chosen = pick.filter(|_| total.saturating_mul(2) >= eb - ea).map(|p| p.0);
        cells.push((ca, cb, chosen));
    }
    let mut out: Vec<(i64, i64, String)> = Vec::new();
    for (ca, cb, name) in cells {
        let Some(name) = name else { continue };
        match out.last_mut() {
            Some(last) if last.2 == name && last.1 == ca => last.1 = cb,
            _ => out.push((ca, cb, name.to_string())),
        }
    }
    (out, (k0.saturating_mul(cell), k1.max(k0).saturating_mul(cell)))
}

/// [`Grid::Free`]: the changes at their own ticks. A chord shorter than an eighth of a bar gives
/// its start to the chord after it (or, the last one, its span to the chord before); equal
/// neighbours merge; each chord runs to the next change. The span is the source's, in ticks.
fn as_played(s: &Session, start: Samples, len: Samples, chords: &[TimedChord]) -> (Vec<(i64, i64, String)>, (i64, i64)) {
    let sr = s.sample_rate;
    let rate = sr.as_f64();
    let eighth = (half_ticks(s) / 4).max(1);
    let end = start.saturating_add(len.max(0));
    let tick = |at: Samples| s.tempo.samples_to_ticks(at, sr);
    let at = |secs: f64| -> Samples {
        if secs.is_finite() { start.saturating_add((secs * rate).round().clamp(0.0, len.max(0) as f64) as Samples) } else { start }
    };
    let mut spans: Vec<(i64, i64, &str)> = chords
        .iter()
        .filter(|c| !c.name.is_empty())
        .map(|c| (tick(at(c.start)), tick(at(c.end)), c.name.as_str()))
        .filter(|(a, b, _)| b > a)
        .collect();
    spans.sort_by_key(|x| x.0);
    let mut out: Vec<(i64, i64, String)> = Vec::new();
    for (a, b, name) in spans {
        if let Some(last) = out.last_mut() {
            // The last chord runs to this change.
            last.1 = a.max(last.0);
            if last.2 == name {
                last.1 = b.max(last.1);
                continue;
            }
            if last.1 - last.0 < eighth {
                // Two changes closer than an eighth of a bar: this chord takes the earlier time.
                let from = last.0;
                out.pop();
                match out.last_mut() {
                    Some(prev) if prev.2 == name => prev.1 = b.max(prev.1),
                    _ => out.push((from, b, name.to_string())),
                }
                continue;
            }
        }
        out.push((a, b, name.to_string()));
    }
    // A last chord too short to stand gives its span to the one before.
    if out.len() > 1
        && let Some(last) = out.pop()
    {
        if last.1 - last.0 < eighth {
            if let Some(prev) = out.last_mut() {
                prev.1 = last.1;
            }
        } else {
            out.push(last);
        }
    }
    (out, (tick(start), tick(end)))
}

/// `[a, b)` minus every span in `taken`.
fn gaps(a: i64, b: i64, taken: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out = vec![(a, b)];
    for &(x, y) in taken {
        out = out
            .into_iter()
            .flat_map(
                |(p, q)| {
                    if y <= p || x >= q { vec![(p, q)] } else { [(p, x.min(q)), (y.max(p), q)].into_iter().filter(|(u, v)| v > u).collect() }
                },
            )
            .collect();
    }
    out
}

/// Puts `chords` (detected over `start..start+len`) on the session's chord lane.
pub fn place_chords(s: &mut Session, start: Samples, len: Samples, chords: &[TimedChord], o: &LaneOptions) -> Result<Value, String> {
    let (snapped, (ca, cb)) = snap_chords(s, start, len, chords, o.grid);
    let mut st = s.band.clone();
    let (old_key, old_scale) = (st.key, st.scale);
    // Only chords elsewhere on the lane stop the key from being estimated.
    let others = st.chords.iter().any(|c| !o.replace || c.tick < ca || c.tick.saturating_add(c.length_ticks) > cb);
    let mut key_set = false;
    match o.key {
        KeyChoice::Set(k, sc) => {
            (st.key, st.scale) = (k.min(11), sc.min(1));
            key_set = true;
        }
        KeyChoice::Auto if !others => {
            if let Some((k, sc)) = estimate_key(&snapped.iter().map(|(a, b, n)| (b - a, n.clone())).collect::<Vec<_>>()) {
                (st.key, st.scale) = (k, sc);
                key_set = true;
            }
        }
        _ => {}
    }
    if o.replace {
        carve(&mut st.chords, ca, cb, None, &mut st.next_id);
    }
    if (st.key, st.scale) != (old_key, old_scale) {
        rekey(&mut st.chords, (old_key, old_scale), (st.key, st.scale), s)?;
    }
    let taken: Vec<(i64, i64)> = st.chords.iter().map(|c| (c.tick, c.tick.saturating_add(c.length_ticks))).collect();
    let mut placed = Vec::new();
    let mut unread = Vec::new();
    for (a, b, name) in &snapped {
        let Some(pcs) = parse_chord_name(name) else {
            unread.push(name.clone());
            continue;
        };
        let m = match_chord(&pcs, i32::from(st.key), i32::from(st.scale));
        let pieces = if o.replace { vec![(*a, *b)] } else { gaps(*a, *b, &taken) };
        for (x, y) in pieces {
            if st.chords.len() >= MAX_CHORDS {
                return Err(format!("the chord lane is full ({MAX_CHORDS} chords)"));
            }
            let id = take_id(&mut st.next_id);
            st.chords.push(ChordEvent {
                id,
                tick: x,
                length_ticks: y - x,
                slot: u8::try_from(m.slot.clamp(0, 111)).unwrap_or(0),
                bass: i8::try_from(m.bass.clamp(-1, 11)).unwrap_or(-1),
            });
            let at = s.tempo.tick_to_samples(x, s.sample_rate);
            placed.push(
                json!({"chord": id, "name": name, "slot": m.slot, "bass": m.bass, "exact": m.exact, "at": at, "tick": x, "length_ticks": y - x}),
            );
        }
    }
    st.sanitize();
    if st != s.band {
        s.band = st;
    }
    let key = format!("{} {}", KEY_NAMES.get(usize::from(s.band.key)).copied().unwrap_or("C"), if s.band.scale == 1 { "minor" } else { "major" });
    Ok(json!({"chords": placed, "key": key, "key_set": key_set, "grid": o.grid.id(), "replace": o.replace, "unreadable": unread}))
}

/// Re-expresses the lane's chords in a new key so they keep sounding the same (slots and bass
/// are key-relative). A chord the new key can't hold exactly refuses the change.
fn rekey(chords: &mut [ChordEvent], from: (u8, u8), to: (u8, u8), s: &Session) -> Result<(), String> {
    use soundcraft_band::chord_match::{ChordPcs, slot_pitch_classes};
    let (fk, fs, tk, ts) = (i32::from(from.0), i32::from(from.1), i32::from(to.0), i32::from(to.1));
    let mut bad = Vec::new();
    let mut out = Vec::with_capacity(chords.len());
    for c in chords.iter() {
        let slot = i32::from(c.slot);
        let root = soundcraft_band::chord_theory::shape_for(slot, fk, fs, -1).root_pc;
        let bass = if c.bass >= 0 { (i32::from(c.bass) + fk).rem_euclid(12) } else { root };
        let pcs = ChordPcs { root_pc: root, pcs: slot_pitch_classes(slot, fk, fs), bass_pc: bass };
        let m = match_chord(&pcs, tk, ts);
        if !m.exact {
            let bar = c.tick.div_euclid(half_ticks(s).saturating_mul(2).max(1)).saturating_add(1);
            bad.push(format!("bar {bar}"));
        }
        out.push((u8::try_from(m.slot.clamp(0, 111)).unwrap_or(0), i8::try_from(m.bass.clamp(-1, 11)).unwrap_or(-1)));
    }
    if !bad.is_empty() {
        let name =
            |(k, sc): (u8, u8)| format!("{} {}", KEY_NAMES.get(usize::from(k)).copied().unwrap_or("C"), if sc == 1 { "minor" } else { "major" });
        return Err(format!(
            "the chords already on the lane at {} can't be written in {} without changing them: choose the band's key (keep) or Auto, or clear those chords first",
            bad.join(", "),
            name(to)
        ));
    }
    for (c, (slot, bass)) in chords.iter_mut().zip(out) {
        (c.slot, c.bass) = (slot, bass);
    }
    Ok(())
}

/// Shows the Sections and Chords rulers, as the band's own chord commands do.
pub fn show_rulers(e: &mut crate::Engine) {
    super::band::show_lane_rulers(e);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_estimates_and_params() {
        let ch = |n: &str| (1i64, n.to_string());
        assert_eq!(estimate_key(&[ch("Em"), ch("C"), ch("D"), ch("Em"), ch("B")]), Some((4, 1)));
        assert_eq!(estimate_key(&[ch("C"), ch("F"), ch("G"), ch("C")]), Some((0, 0)));
        assert_eq!(estimate_key(&[]), None);
        assert_eq!(key_choice(&json!({})), Ok(KeyChoice::Auto));
        assert_eq!(key_choice(&json!({"key": "keep"})), Ok(KeyChoice::Keep));
        assert_eq!(key_choice(&json!({"key": "E", "scale": "minor"})), Ok(KeyChoice::Set(4, 1)));
        assert_eq!(key_choice(&json!({"key": "Bb"})), Ok(KeyChoice::Set(10, 0)));
        assert_eq!(key_choice(&json!({"key": 7})), Ok(KeyChoice::Set(7, 0)));
        assert!(key_choice(&json!({"key": "H"})).is_err());
        assert!(key_choice(&json!({"key": 12})).is_err());
        assert_eq!(gaps(0, 10, &[(2, 4), (8, 12)]), vec![(0, 2), (4, 8)]);
    }
}

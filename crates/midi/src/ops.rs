//! MIDI editing operations (Event Operations): quantize, transpose, velocity, duration, split.
//!
//! All operations work in place on a slice of notes and never change the slice length, so they
//! can be applied to a selection. Randomised operations use a deterministic [`XorShift`] seeded
//! with [`DEFAULT_SEED`] (or an explicit seed via the `*_with_seed` variants).

use crate::{Note, XorShift};

/// Seed used by [`quantize`] and [`change_velocity`] for their random components.
pub const DEFAULT_SEED: u64 = 0x5EED_50DC_4AF7_0001;

fn unit(x: f32) -> f64 {
    if x.is_finite() { f64::from(x).clamp(0.0, 1.0) } else { 0.0 }
}

fn round_i64(x: f64) -> i64 {
    if x.is_finite() { x.round().clamp(i64::MIN as f64, i64::MAX as f64) as i64 } else { 0 }
}

/// Options for [`quantize`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantizeOptions {
    /// Grid spacing in ticks (e.g. 240 = 1/16 note at 960 PPQ). Values <= 0 make quantize a no-op.
    pub grid_ticks: i64,
    /// How far notes move towards the grid line, 0..=1 (1 = all the way).
    pub strength: f32,
    /// Swing, 0..=1: every second grid line is delayed by `swing * grid / 3`, so 1.0 gives a
    /// triplet feel.
    pub swing: f32,
    /// Only notes within this fraction of half a grid step of their target are moved
    /// (1 = all notes).
    pub include_within: f32,
    /// Notes closer than this fraction of half a grid step to their target are left alone
    /// (0 = none excluded).
    pub exclude_within: f32,
    /// Also quantize note ends to the grid.
    pub quantize_ends: bool,
    /// Keep each note's length when its start moves (otherwise the end stays put). Ignored when
    /// `quantize_ends` is set.
    pub preserve_duration: bool,
    /// Random offset after quantizing, 0..=1 fraction of half a grid step.
    pub random: f32,
}

impl Default for QuantizeOptions {
    fn default() -> Self {
        Self {
            grid_ticks: 240,
            strength: 1.0,
            swing: 0.0,
            include_within: 1.0,
            exclude_within: 0.0,
            quantize_ends: false,
            preserve_duration: true,
            random: 0.0,
        }
    }
}

/// The swung grid line nearest to `t`.
fn nearest_line(t: i64, grid: i64, swing: f64) -> i64 {
    let swing_off = round_i64(swing * grid as f64 / 3.0);
    let k = t.saturating_add(grid / 2).div_euclid(grid);
    let mut best = i64::MAX;
    let mut best_d = u64::MAX;
    for kk in [k.saturating_sub(1), k, k.saturating_add(1)] {
        let line = kk.saturating_mul(grid).saturating_add(if kk.rem_euclid(2) == 1 { swing_off } else { 0 });
        let d = line.abs_diff(t);
        if d < best_d {
            best_d = d;
            best = line;
        }
    }
    best
}

/// The line nearest to `t` on a grid counted (and swung) from the start `a` of the span `(a, b)`
/// holding `t`; `b` (the next span's start, itself a line) is a candidate when `b > a`.
fn nearest_line_in(t: i64, grid: i64, swing: f64, (a, b): (i64, i64)) -> i64 {
    let line = a.saturating_add(nearest_line(t.saturating_sub(a), grid, swing));
    if b > a && b.abs_diff(t) < line.abs_diff(t) { b } else { line }
}

/// Quantizes note starts (and optionally ends) to a grid counted from tick 0. See
/// [`QuantizeOptions`].
pub fn quantize(notes: &mut [Note], opts: &QuantizeOptions) {
    quantize_with_seed(notes, opts, DEFAULT_SEED);
}

/// [`quantize`] with an explicit seed for the `random` component.
pub fn quantize_with_seed(notes: &mut [Note], opts: &QuantizeOptions, seed: u64) {
    quantize_on(notes, opts, seed, |_| (0, 0));
}

/// [`quantize_with_seed`] on a grid counted from the start of each span (a bar) rather than from
/// tick 0: `span(t)` gives the start of the span holding `t` and the next span's start, in the
/// notes' own ticks. Swing counts from each span's start too.
pub fn quantize_on(notes: &mut [Note], opts: &QuantizeOptions, seed: u64, span: impl Fn(i64) -> (i64, i64)) {
    let grid = opts.grid_ticks;
    if grid <= 0 {
        return;
    }
    let swing = unit(opts.swing);
    quantize_toward(notes, opts, seed, |t| nearest_line_in(t, grid, swing, span(t)), |_| 0);
}

/// [`quantize_on`] toward the points of `groove` instead of grid lines (its period counted from
/// each bar: `bar_start(t)` is the start of the bar holding `t`). The window, ends, length and
/// random options measure in half of `opts.grid_ticks`, as on a grid. A groove with accents
/// also moves each note's velocity by its point's accent, by the strength.
pub fn quantize_to_groove(notes: &mut [Note], groove: &Groove, opts: &QuantizeOptions, seed: u64, bar_start: impl Fn(i64) -> i64) {
    if opts.grid_ticks <= 0 {
        return;
    }
    quantize_toward(notes, opts, seed, |t| groove.nearest(t, bar_start(t)), |t| i64::from(groove.accent(t, bar_start(t))));
}

/// The quantize loop: each note's start (and end, with `quantize_ends`) moves toward
/// `target(tick)` by the options' strength, within their window, and its velocity by
/// `accent(start)` (by the strength too).
fn quantize_toward(notes: &mut [Note], opts: &QuantizeOptions, seed: u64, target: impl Fn(i64) -> i64, accent: impl Fn(i64) -> i64) {
    let grid = opts.grid_ticks;
    if grid <= 0 {
        return;
    }
    let strength = unit(opts.strength);
    let include = unit(opts.include_within);
    let exclude = unit(opts.exclude_within);
    let random = unit(opts.random);
    let half = grid as f64 / 2.0;
    let mut rng = XorShift::new(seed);

    for n in notes.iter_mut() {
        let old_start = n.start;
        let old_end = n.end();
        let d = target(old_start).saturating_sub(old_start);
        let dist = d.unsigned_abs() as f64;
        if include < 1.0 && dist > include * half {
            continue;
        }
        if dist < exclude * half {
            continue;
        }
        let a = accent(old_start);
        if a != 0 {
            n.velocity = accented(n.velocity, a, strength);
        }
        let mut new_start = old_start.saturating_add(round_i64(d as f64 * strength));
        if random > 0.0 {
            new_start = new_start.saturating_add(round_i64(rng.next_signed() * random * half));
        }
        n.start = new_start;
        n.length = if opts.quantize_ends {
            let end_target = target(old_end);
            let new_end = old_end.saturating_add(round_i64(end_target.saturating_sub(old_end) as f64 * strength));
            new_end.saturating_sub(new_start)
        } else if opts.preserve_duration {
            n.length
        } else {
            old_end.saturating_sub(new_start)
        }
        .max(1);
    }
}

/// A groove: where the notes of a repeating span (`period` ticks, counted from each bar line)
/// fall, as offsets into it, and how much louder or softer each point is played. Quantizing to a
/// groove moves each note toward its nearest point.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Groove {
    pub name: String,
    /// Ticks the groove repeats every (counted from each bar line).
    pub period: i64,
    /// Offsets into the period, sorted, each in `0..period`.
    pub points: Vec<i64>,
    /// Velocity accent of each point (with `points`), added to a note quantized or played onto
    /// it. Empty: timing only (the built-in grooves, and every groove saved before accents).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accents: Vec<i8>,
}

impl Groove {
    /// A groove from `points` (sorted, deduplicated, kept in `0..period`); `None` without a
    /// positive period or points.
    pub fn new(name: &str, period: i64, points: &[i64]) -> Option<Groove> {
        let p: Vec<(i64, i8)> = points.iter().map(|x| (*x, 0)).collect();
        Groove::with_accents(name, period, &p)
    }

    /// [`Groove::new`] with a velocity accent on each point (the first kept where two points
    /// fall together); no accents are kept when all are 0.
    pub fn with_accents(name: &str, period: i64, points: &[(i64, i8)]) -> Option<Groove> {
        if period <= 0 {
            return None;
        }
        let mut p: Vec<(i64, i8)> = points.iter().map(|(x, a)| (x.rem_euclid(period), *a)).collect();
        p.sort_by_key(|x| x.0);
        p.dedup_by_key(|x| x.0);
        let accents: Vec<i8> = if p.iter().any(|x| x.1 != 0) { p.iter().map(|x| x.1).collect() } else { Vec::new() };
        (!p.is_empty()).then(|| Groove { name: name.to_string(), period, points: p.iter().map(|x| x.0).collect(), accents })
    }

    /// The groove point nearest to tick `t`, the period counted from `bar_start` (the start of
    /// the bar holding `t`).
    pub fn nearest(&self, t: i64, bar_start: i64) -> i64 {
        groove_nearest(&self.points, self.period, t, bar_start)
    }

    /// The accent of the point nearest to `t` (0 without accents). No allocation.
    pub fn accent(&self, t: i64, bar_start: i64) -> i8 {
        if self.accents.is_empty() {
            return 0;
        }
        groove_point(&self.points, self.period, t, bar_start).and_then(|(_, i)| self.accents.get(i).copied()).unwrap_or(0)
    }
}

/// `velocity` moved by `accent` times `strength` (0..=1), kept in 1..=127.
fn accented(velocity: u8, accent: i64, strength: f64) -> u8 {
    i64::from(velocity).saturating_add(round_i64(accent as f64 * strength)).clamp(1, 127) as u8
}

/// [`Groove::nearest`] on bare points (no allocation: for the audio thread).
pub fn groove_nearest(points: &[i64], period: i64, t: i64, bar_start: i64) -> i64 {
    groove_point(points, period, t, bar_start).map_or(t, |(line, _)| line)
}

/// The groove point nearest to `t` and its index in `points`; `None` without a period or points.
fn groove_point(points: &[i64], period: i64, t: i64, bar_start: i64) -> Option<(i64, usize)> {
    if period <= 0 || points.is_empty() {
        return None;
    }
    let rel = t.saturating_sub(bar_start);
    let cycle = bar_start.saturating_add(rel.div_euclid(period).saturating_mul(period));
    let mut best = None;
    let mut best_d = u64::MAX;
    // This cycle's points, the last of the cycle before and the first of the next.
    let last = points.len().saturating_sub(1);
    let before = points.last().map(|p| (cycle.saturating_sub(period).saturating_add(*p), last));
    let after = points.first().map(|p| (cycle.saturating_add(period).saturating_add(*p), 0));
    for (line, i) in points.iter().enumerate().map(|(i, p)| (cycle.saturating_add(*p), i)).chain(before).chain(after) {
        let d = line.abs_diff(t);
        if d < best_d {
            best_d = d;
            best = Some((line, i));
        }
    }
    best
}

/// The grooves built in: swung 8ths and 16ths (the second of each pair placed at 54 to 66 % of
/// the pair, 66 % being a triplet feel), and 16ths pushed or laid back by 20 ticks. Made here,
/// not loaded from anywhere.
pub fn builtin_grooves() -> Vec<Groove> {
    let mut v = Vec::new();
    for pct in [54, 58, 62, 66] {
        // A pair of 8ths is a quarter note (960 ticks); of 16ths, an 8th (480).
        v.extend(Groove::new(&format!("Swing 8ths {pct}%"), 960, &[0, 960 * pct / 100]));
        v.extend(Groove::new(&format!("Swing 16ths {pct}%"), 480, &[0, 480 * pct / 100]));
    }
    v.extend(Groove::new("Pushed 16ths", 480, &[0, 240 - 20]));
    v.extend(Groove::new("Laid-Back 16ths", 480, &[0, 240 + 20]));
    v
}

/// A groove taken from notes: the period is cut into `slot`-tick steps, and each step's point is
/// where the notes near it fall on average (the median offset of those within half a step;
/// a step no note is near stays straight), its accent how much louder or softer than the steps'
/// average they are played (their median velocity; 0 for a step no note is near). `bar_start(t)`
/// is the start of the bar holding `t`.
/// `None` when no note is near any step or the sizes are not positive.
pub fn groove_from_notes(name: &str, notes: &[Note], period: i64, slot: i64, bar_start: impl Fn(i64) -> i64) -> Option<Groove> {
    if period <= 0 || slot <= 0 || slot > period {
        return None;
    }
    let steps = (period / slot).clamp(1, 256);
    let mut offsets: Vec<Vec<i64>> = vec![Vec::new(); steps as usize];
    let mut velocities: Vec<Vec<u8>> = vec![Vec::new(); steps as usize];
    for n in notes.iter().filter(|n| n.start >= 0) {
        let pos = n.start.saturating_sub(bar_start(n.start)).rem_euclid(period);
        let k = ((pos + slot / 2) / slot).rem_euclid(steps);
        let mut off = pos - k * slot;
        // A note just before the period's end belongs to the next period's first step.
        if off > period / 2 {
            off -= period;
        }
        if off.abs() <= slot / 2
            && let (Some(v), Some(w)) = (offsets.get_mut(k as usize), velocities.get_mut(k as usize))
        {
            v.push(off);
            w.push(n.velocity);
        }
    }
    if offsets.iter().all(Vec::is_empty) {
        return None;
    }
    let medians: Vec<Option<i64>> = velocities
        .iter_mut()
        .map(|w| {
            w.sort_unstable();
            w.get(w.len() / 2).map(|v| i64::from(*v))
        })
        .collect();
    let played: Vec<i64> = medians.iter().flatten().copied().collect();
    let mean = played.iter().sum::<i64>() / (played.len().max(1) as i64);
    let points: Vec<(i64, i8)> = offsets
        .iter_mut()
        .zip(0i64..)
        .zip(&medians)
        .map(|((v, k), m)| {
            v.sort_unstable();
            let accent = m.map_or(0, |m| (m - mean).clamp(-64, 63) as i8);
            (k * slot + v.get(v.len() / 2).copied().unwrap_or(0), accent)
        })
        .collect();
    Groove::with_accents(name, period, &points)
}

/// Quantize applied as a clip or track plays: the notes stay where they are; each plays from
/// where quantizing its start would put it (toward the grid, or the groove when there is one),
/// keeping its length.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlayQuantize {
    /// Grid spacing in ticks (ignored with a groove).
    pub grid: i64,
    /// 0..=1: how far notes move toward their line.
    pub strength: f32,
    /// 0..=1, as [`QuantizeOptions::swing`].
    #[serde(default)]
    pub swing: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groove: Option<Groove>,
}

impl PlayQuantize {
    /// Where a note starting at `t` plays; `bar` is the bar holding `t` (its start and the next
    /// bar's start). No allocation.
    pub fn start(&self, t: i64, bar: (i64, i64)) -> i64 {
        let target = match &self.groove {
            Some(g) => groove_nearest(&g.points, g.period, t, bar.0),
            None if self.grid > 0 => nearest_line_in(t, self.grid, unit(self.swing), bar),
            None => return t,
        };
        t.saturating_add(round_i64(target.saturating_sub(t) as f64 * unit(self.strength)))
    }

    /// The velocity a note starting at `t` with `velocity` plays at: its groove point's accent
    /// added, by the strength (unchanged without a groove with accents). No allocation.
    pub fn velocity(&self, t: i64, bar: (i64, i64), velocity: u8) -> u8 {
        match &self.groove {
            Some(g) if !g.accents.is_empty() => accented(velocity, i64::from(g.accent(t, bar.0)), unit(self.strength)),
            _ => velocity,
        }
    }

    /// Whether a groove with accents changes velocities ([`PlayQuantize::velocity`]).
    pub fn accents(&self) -> bool {
        self.groove.as_ref().is_some_and(|g| !g.accents.is_empty())
    }
}

/// One chord of the chord lane as a following clip keeps it: `[start, end)` in ticks from the
/// clip's start, its root pitch class and its pitch classes (bit `pc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FollowChord {
    pub start: i64,
    pub end: i64,
    pub root: u8,
    pub tones: u16,
}

/// Where `pitch`, written under `old`, goes under `new`: a chord tone to the nearest tone of the
/// new chord (it stays when it is one; the lower on a tie), any other note as the root moves,
/// the shortest way.
pub fn follow_chord_pitch(pitch: u8, old: &FollowChord, new: &FollowChord) -> u8 {
    let pc = i32::from(pitch % 12);
    let has = |tones: u16, pc: i32| (tones >> pc.rem_euclid(12)) & 1 == 1;
    let d = if has(old.tones, pc) && new.tones != 0 {
        (0..=6).flat_map(|k| [-k, k]).find(|k| has(new.tones, pc + k)).unwrap_or(0)
    } else {
        (i32::from(new.root) - i32::from(old.root) + 6).rem_euclid(12) - 6
    };
    (i32::from(pitch) + d).clamp(0, 127) as u8
}

/// Transposes notes by `semitones`. Pitches are **clamped** to 0..=127 (notes are never
/// dropped, so the slice length is unchanged); notes pushed past either end pile up on 0 or 127.
pub fn transpose(notes: &mut [Note], semitones: i32) {
    for n in notes.iter_mut() {
        n.pitch = i32::from(n.pitch).saturating_add(semitones).clamp(0, 127) as u8;
    }
}

/// The part of a `semitones` shift the notes can all take and stay in 0..=127: so a chord moved
/// past the top or bottom keeps its shape rather than piling onto one pitch.
pub fn fitting_shift(notes: &[Note], semitones: i64) -> i64 {
    let (Some(lo), Some(hi)) = (notes.iter().map(|n| i64::from(n.pitch)).min(), notes.iter().map(|n| i64::from(n.pitch)).max()) else {
        return 0;
    };
    semitones.clamp(-lo, 127 - hi)
}

/// [`transpose`] by as much of `semitones` as every note can take ([`fitting_shift`]): notes
/// keep their intervals. Returns the shift made.
pub fn transpose_within(notes: &mut [Note], semitones: i32) -> i32 {
    let s = fitting_shift(notes, i64::from(semitones)) as i32;
    transpose(notes, s);
    s
}

/// Velocity change for [`change_velocity`]. Results are clamped to 1..=127.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VelocityOp {
    Set(u8),
    Add(i32),
    /// Multiply (non-finite factors leave velocities unchanged).
    Scale(f32),
    /// Clamp into `min..=max` (bounds are swapped if reversed).
    Limit {
        min: u8,
        max: u8,
    },
    /// Linear ramp over the notes' time span: the earliest note gets `from`, the latest `to`.
    Ramp {
        from: u8,
        to: u8,
    },
    /// Random offset of up to ± the amount.
    Humanize(u8),
    /// Each velocity's distance from `center` (the notes' average when `None`) times `factor`:
    /// below 1 compresses the dynamics towards it, above 1 expands them (non-finite factors leave
    /// velocities unchanged).
    Compress {
        center: Option<u8>,
        factor: f32,
    },
}

fn clamp_vel(v: i64) -> u8 {
    v.clamp(1, 127) as u8
}

/// Changes note-on velocities. See [`VelocityOp`].
pub fn change_velocity(notes: &mut [Note], op: VelocityOp) {
    change_velocity_with_seed(notes, op, DEFAULT_SEED);
}

/// [`change_velocity`] with an explicit seed for [`VelocityOp::Humanize`].
pub fn change_velocity_with_seed(notes: &mut [Note], op: VelocityOp, seed: u64) {
    match op {
        VelocityOp::Set(v) => notes.iter_mut().for_each(|n| n.velocity = clamp_vel(i64::from(v))),
        VelocityOp::Add(d) => notes.iter_mut().for_each(|n| n.velocity = clamp_vel(i64::from(n.velocity) + i64::from(d))),
        VelocityOp::Scale(f) => {
            if f.is_finite() {
                notes.iter_mut().for_each(|n| n.velocity = clamp_vel(round_i64(f64::from(n.velocity) * f64::from(f))));
            }
        }
        VelocityOp::Limit { min, max } => {
            let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
            notes.iter_mut().for_each(|n| n.velocity = clamp_vel(i64::from(n.velocity.clamp(lo, hi))));
        }
        VelocityOp::Ramp { from, to } => {
            let first = notes.iter().map(|n| n.start).min().unwrap_or(0);
            let last = notes.iter().map(|n| n.start).max().unwrap_or(0);
            let span = last.saturating_sub(first) as f64;
            for n in notes.iter_mut() {
                let t = if span > 0.0 { n.start.saturating_sub(first) as f64 / span } else { 0.0 };
                let v = f64::from(from) + (f64::from(to) - f64::from(from)) * t;
                n.velocity = clamp_vel(round_i64(v));
            }
        }
        VelocityOp::Humanize(amount) => {
            let mut rng = XorShift::new(seed);
            for n in notes.iter_mut() {
                let d = round_i64(rng.next_signed() * f64::from(amount));
                n.velocity = clamp_vel(i64::from(n.velocity) + d);
            }
        }
        VelocityOp::Compress { center, factor } => {
            if !factor.is_finite() || notes.is_empty() {
                return;
            }
            let c = match center {
                Some(c) => f64::from(c),
                None => notes.iter().map(|n| f64::from(n.velocity)).sum::<f64>() / notes.len() as f64,
            };
            for n in notes.iter_mut() {
                n.velocity = clamp_vel(round_i64(c + (f64::from(n.velocity) - c) * f64::from(factor)));
            }
        }
    }
}

/// Duration change for [`change_duration`]. Lengths are always kept >= 1 tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DurationOp {
    Set(i64),
    Add(i64),
    /// Multiply (non-finite factors leave lengths unchanged).
    Scale(f32),
    /// Each note extends to `gap` ticks before the next later note start (any pitch). The last
    /// note(s) keep their length.
    Legato {
        gap: i64,
    },
    /// Shortens notes so they end no later than the next note of the same pitch and channel.
    RemoveOverlap,
}

/// Changes note lengths. See [`DurationOp`].
pub fn change_duration(notes: &mut [Note], op: DurationOp) {
    match op {
        DurationOp::Set(l) => notes.iter_mut().for_each(|n| n.length = l.max(1)),
        DurationOp::Add(d) => notes.iter_mut().for_each(|n| n.length = n.length.saturating_add(d).max(1)),
        DurationOp::Scale(f) => {
            if f.is_finite() {
                notes.iter_mut().for_each(|n| n.length = round_i64(n.length as f64 * f64::from(f)).max(1));
            }
        }
        DurationOp::Legato { gap } => {
            let mut starts: Vec<i64> = notes.iter().map(|n| n.start).collect();
            starts.sort_unstable();
            starts.dedup();
            for n in notes.iter_mut() {
                let idx = starts.partition_point(|&s| s <= n.start);
                if let Some(&next) = starts.get(idx) {
                    n.length = next.saturating_sub(gap).saturating_sub(n.start).max(1);
                }
            }
        }
        DurationOp::RemoveOverlap => {
            let mut order: Vec<usize> = (0..notes.len()).collect();
            order.sort_by_key(|&i| notes.get(i).map(|n| (n.channel, n.pitch, n.start)));
            for w in order.windows(2) {
                let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else { continue };
                let Some(&nb) = notes.get(b) else { continue };
                if let Some(na) = notes.get_mut(a)
                    && na.channel == nb.channel
                    && na.pitch == nb.pitch
                    && na.end() > nb.start
                {
                    na.length = nb.start.saturating_sub(na.start).max(1);
                }
            }
        }
    }
}

/// Plays the notes backwards in the span they cover: a note ending `d` ticks before the last end
/// starts `d` ticks after the first start. Lengths, pitches and velocities stay.
pub fn reverse(notes: &mut [Note]) {
    let (Some(first), Some(last)) = (notes.iter().map(|n| n.start).min(), notes.iter().map(Note::end).max()) else { return };
    for n in notes.iter_mut() {
        n.start = first.saturating_add(last.saturating_sub(n.end()));
    }
}

/// Stretches the notes in time by `factor` from the first one's start: 2 is half time (twice as
/// long), 0.5 double time. Lengths stay at least one tick; non-finite or non-positive factors
/// change nothing.
pub fn scale_time(notes: &mut [Note], factor: f64) {
    if !factor.is_finite() || factor <= 0.0 {
        return;
    }
    let Some(first) = notes.iter().map(|n| n.start).min() else { return };
    for n in notes.iter_mut() {
        n.start = first.saturating_add(round_i64(n.start.saturating_sub(first) as f64 * factor));
        n.length = round_i64(n.length as f64 * factor).max(1);
    }
}

/// Strums each chord (notes starting together): each note after the first in pitch order (lowest
/// first when `up`, as a downward strum on a guitar plays them; highest first otherwise) starts
/// `step` ticks after the one before, keeping its end.
pub fn strum(notes: &mut [Note], step: i64, up: bool) {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by_key(|&i| notes.get(i).map(|n| (n.start, if up { i32::from(n.pitch) } else { -i32::from(n.pitch) })));
    let mut k = 0i64;
    let mut chord_start = None;
    for i in order {
        let Some(n) = notes.get_mut(i) else { continue };
        if chord_start != Some(n.start) {
            chord_start = Some(n.start);
            k = 0;
        }
        let end = n.end();
        n.start = n.start.saturating_add(step.saturating_mul(k));
        n.length = end.saturating_sub(n.start).max(1);
        k += 1;
    }
}

/// Humanise: each note's start moves by up to ± `timing` ticks and its velocity by up to
/// ± `velocity`, from a seeded generator (the same seed gives the same result).
pub fn humanize(notes: &mut [Note], timing: i64, velocity: u8, seed: u64) {
    let mut rng = XorShift::new(seed);
    for n in notes.iter_mut() {
        if timing > 0 {
            n.start = n.start.saturating_add(round_i64(rng.next_signed() * timing as f64));
        }
        if velocity > 0 {
            n.velocity = clamp_vel(i64::from(n.velocity) + round_i64(rng.next_signed() * f64::from(velocity)));
        }
    }
}

/// Select/Split Notes by pitch: returns `(notes with pitch in pitch_from..=pitch_to, the rest)`,
/// both in input order. The bounds are swapped if reversed.
pub fn split_notes(notes: &[Note], pitch_from: u8, pitch_to: u8) -> (Vec<Note>, Vec<Note>) {
    let (lo, hi) = if pitch_from <= pitch_to { (pitch_from, pitch_to) } else { (pitch_to, pitch_from) };
    notes.iter().partition(|n| (lo..=hi).contains(&n.pitch))
}

/// A rule picking notes by ranges (each inclusive, either order): pitch, velocity, length and
/// start in ticks, and optionally the start's position inside its bar (ticks from the bar line).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteRule {
    pub pitch: (u8, u8),
    pub velocity: (u8, u8),
    pub length: (i64, i64),
    pub start: (i64, i64),
    pub in_bar: Option<(i64, i64)>,
}

impl Default for NoteRule {
    fn default() -> Self {
        NoteRule { pitch: (0, 127), velocity: (1, 127), length: (1, i64::MAX), start: (0, i64::MAX), in_bar: None }
    }
}

fn within<T: Ord + Copy>(x: T, (a, b): (T, T)) -> bool {
    if a <= b { a <= x && x <= b } else { b <= x && x <= a }
}

impl NoteRule {
    /// Whether `n` matches; `bar_start(t)` is the start of the bar holding tick `t`.
    pub fn matches(&self, n: &Note, bar_start: impl Fn(i64) -> i64) -> bool {
        within(n.pitch, self.pitch)
            && within(n.velocity, self.velocity)
            && within(n.length, self.length)
            && within(n.start, self.start)
            && self.in_bar.is_none_or(|r| within(n.start.saturating_sub(bar_start(n.start)), r))
    }
}

/// The indices of the notes matching `rule` (see [`NoteRule::matches`]).
pub fn select_matching(notes: &[Note], rule: &NoteRule, bar_start: impl Fn(i64) -> i64) -> Vec<usize> {
    notes.iter().enumerate().filter(|(_, n)| rule.matches(n, &bar_start)).map(|(i, _)| i).collect()
}

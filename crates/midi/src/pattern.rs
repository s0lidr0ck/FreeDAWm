//! Step patterns (Juke patterns): lanes of steps that expand into MIDI notes.
//!
//! A [`Pattern`] is the source of truth of a pattern clip; the clip's notes are [`expand`]ed from
//! it whenever it, the clip's length or the bar length changes. Expansion is a pure function: the
//! same pattern and [`Expand`] always give the same notes, so playback and every bounce agree.
//! Chance (probability, humanize, the random direction) is drawn from a generator seeded by the
//! pattern's seed, the lane and the step's absolute position, so a step's dice don't depend on the
//! other lanes or on where the clip was trimmed.

use serde::{Deserialize, Serialize};

use crate::{Note, PPQ, Sequence, XorShift};

/// Most lanes a pattern holds.
pub const MAX_LANES: usize = 32;
/// Most steps a lane holds (its loop length).
pub const MAX_STEPS: usize = 64;
/// The grid resolutions a pattern takes, in steps per bar.
pub const STEPS_PER_BAR: [u8; 5] = [8, 12, 16, 24, 32];
/// Most hits a step's roll plays.
pub const MAX_ROLL: u8 = 4;
/// The MIDI channel pattern notes are written on (channel 10, drums).
pub const CHANNEL: u8 = 9;

/// The order a lane plays its steps in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Forward,
    Reverse,
    /// Forward then back, the end steps played once per turn.
    PingPong,
    /// A step of the lane at random, each time.
    Random,
}

/// How long a lane's steps are against the pattern's grid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rate {
    /// Steps twice as long: the lane runs at half speed.
    Half,
    #[default]
    Normal,
    /// Steps half as long: double speed.
    Double,
}

impl Rate {
    fn factor(self) -> f64 {
        match self {
            Rate::Half => 2.0,
            Rate::Normal => 1.0,
            Rate::Double => 0.5,
        }
    }
}

/// When a step plays, beyond its probability.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Condition {
    #[default]
    Always,
    /// Only on the lane's `nth` time through out of every `of` (1-based: `{nth: 1, of: 2}` plays
    /// on the first, third, fifth… pass).
    Every { nth: u8, of: u8 },
    /// Only on the lane's first pass in the clip.
    First,
    /// On every pass but the first.
    NotFirst,
    /// Only in fill bars.
    Fill,
    /// Everywhere but fill bars.
    NotFill,
}

/// One step of a lane.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Step {
    pub on: bool,
    /// Note-on velocity 1–127.
    pub velocity: u8,
    /// Adds the pattern's accent amount to the velocity.
    pub accent: bool,
    /// Chance the step plays, 0–100 %.
    pub probability: u8,
    /// Timing nudge, -50 to +50 % of a step.
    pub offset: i8,
    /// Hits the step plays, evenly spaced across it (1 = a single hit).
    pub roll: u8,
    /// A quieter grace hit just before the step.
    pub flam: bool,
    pub condition: Condition,
}

impl Default for Step {
    fn default() -> Step {
        Step { on: false, velocity: 100, accent: false, probability: 100, offset: 0, roll: 1, flam: false, condition: Condition::Always }
    }
}

impl Step {
    /// A step that plays at `velocity`.
    pub fn hit(velocity: u8) -> Step {
        Step { on: true, velocity, ..Step::default() }
    }

    fn sanitize(&mut self) {
        self.velocity = self.velocity.clamp(1, 127);
        self.probability = self.probability.min(100);
        self.offset = self.offset.clamp(-50, 50);
        self.roll = self.roll.clamp(1, MAX_ROLL);
        if let Condition::Every { nth, of } = &mut self.condition {
            *of = (*of).clamp(1, 16);
            *nth = (*nth).clamp(1, *of);
        }
    }
}

/// One row of the grid: a note and its steps, looping over its own length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lane {
    /// The MIDI note the lane plays (Juke pads default to General MIDI drums).
    pub note: u8,
    /// The lane's steps; their count is the lane's loop length (1–[`MAX_STEPS`]). Lanes of
    /// different lengths make polymeters.
    pub steps: Vec<Step>,
    pub direction: Direction,
    pub rate: Rate,
    pub mute: bool,
}

impl Default for Lane {
    fn default() -> Lane {
        Lane { note: 36, steps: vec![Step::default(); 16], direction: Direction::Forward, rate: Rate::Normal, mute: false }
    }
}

impl Lane {
    /// An empty lane of `length` steps on `note`.
    pub fn new(note: u8, length: usize) -> Lane {
        Lane { note: note.min(127), steps: vec![Step::default(); length.clamp(1, MAX_STEPS)], ..Lane::default() }
    }
}

/// A step pattern: global settings and its lanes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pattern {
    /// Grid resolution, one of [`STEPS_PER_BAR`].
    pub steps_per_bar: u8,
    /// Swing on the off-beat steps, 0–100 % (0 straight; 100 delays them by half a step).
    pub shuffle: u8,
    /// Velocity added to accented steps, 0–127.
    pub accent: u8,
    /// Random timing and velocity per hit, 0–100 %.
    pub humanize: u8,
    /// Seeds every chance the pattern takes ([`Pattern::reroll`] changes it).
    pub seed: u64,
    /// Fill bars come every this many bars (the last of each group); 0: none.
    pub fill_every: u8,
    pub lanes: Vec<Lane>,
}

impl Default for Pattern {
    fn default() -> Pattern {
        Pattern { steps_per_bar: 16, shuffle: 0, accent: 20, humanize: 0, seed: 1, fill_every: 0, lanes: Vec::new() }
    }
}

impl Pattern {
    /// Clamps every field into range, so a pattern from a file or a script expands safely.
    pub fn sanitize(&mut self) {
        if !STEPS_PER_BAR.contains(&self.steps_per_bar) {
            self.steps_per_bar = 16;
        }
        self.shuffle = self.shuffle.min(100);
        self.accent = self.accent.min(127);
        self.humanize = self.humanize.min(100);
        self.lanes.truncate(MAX_LANES);
        for lane in &mut self.lanes {
            lane.note = lane.note.min(127);
            lane.steps.truncate(MAX_STEPS);
            if lane.steps.is_empty() {
                lane.steps.push(Step::default());
            }
            lane.steps.iter_mut().for_each(Step::sanitize);
        }
    }

    /// A new seed, so every chance is drawn again.
    pub fn reroll(&mut self) {
        let mut r = XorShift::new(self.seed ^ 0xA076_1D64_78BD_642F);
        self.seed = r.next_u64() | 1;
    }
}

/// What [`expand`] needs to know about the clip the pattern fills.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Expand {
    /// Ticks in a bar (the meter at the clip's start: 4/4 is 4 × [`PPQ`]).
    pub ticks_per_bar: i64,
    /// The clip's length in ticks: notes start in `0..length`.
    pub length: i64,
    /// Where in the pattern the clip starts, in ticks (a clip trimmed from its start keeps its
    /// place in the bar; negative before the pattern's first step, which the lanes loop back into).
    pub phase: i64,
    /// Most notes to make (the expansion stops there).
    pub max_notes: usize,
}

/// Mixes the pattern seed, a lane and a step position into one generator seed.
fn step_rng(seed: u64, lane: usize, step: i64, salt: u64) -> XorShift {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    for v in [lane as u64, step as u64, salt] {
        h ^= v.wrapping_add(0x9E37_79B9_7F4A_7C15).wrapping_add(h << 6).wrapping_add(h >> 2);
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    }
    XorShift::new(h)
}

fn unit(r: &mut XorShift) -> f64 {
    (r.next_signed() + 1.0) / 2.0
}

/// Which step of a lane of `len` steps plays at its `k`-th position.
fn lane_index(dir: Direction, len: usize, k: i64, rng: &mut XorShift) -> usize {
    let n = len.max(1) as i64;
    let i = match dir {
        Direction::Forward => k.rem_euclid(n),
        Direction::Reverse => n - 1 - k.rem_euclid(n),
        Direction::PingPong if n <= 1 => 0,
        Direction::PingPong => {
            let period = 2 * n - 2;
            let p = k.rem_euclid(period);
            if p < n { p } else { period - p }
        }
        Direction::Random => (unit(rng) * n as f64) as i64,
    };
    usize::try_from(i.clamp(0, n - 1)).unwrap_or(0)
}

/// Expands `pattern` into the notes of a clip described by `at`, sorted, every note starting in
/// `0..at.length`.
pub fn expand(pattern: &Pattern, at: &Expand) -> Sequence {
    let mut p = pattern.clone();
    p.sanitize();
    let mut seq = Sequence::default();
    if at.length <= 0 || at.ticks_per_bar <= 0 {
        return seq;
    }
    let grid = at.ticks_per_bar as f64 / f64::from(p.steps_per_bar);
    let flam_ticks = f64::from(PPQ) / 16.0;
    let humanize = f64::from(p.humanize) / 100.0;
    'lanes: for (li, lane) in p.lanes.iter().enumerate() {
        if lane.mute {
            continue;
        }
        let step_len = grid * lane.rate.factor();
        let len = lane.steps.len();
        // Positions whose (nudged) hits can land in the clip: one step of slack each side.
        let first = ((at.phase as f64) / step_len).floor() as i64 - 1;
        let last = (((at.phase + at.length) as f64) / step_len).ceil() as i64 + 1;
        for k in first..=last {
            let pass = k.div_euclid(len.max(1) as i64);
            let mut chance = step_rng(p.seed, li, k, 0);
            let idx = lane_index(lane.direction, len, k, &mut chance);
            let Some(step) = lane.steps.get(idx) else { continue };
            if !step.on {
                continue;
            }
            let start = k as f64 * step_len;
            let bar = (start / at.ticks_per_bar as f64).floor() as i64;
            let fill = p.fill_every > 0 && (bar + 1).rem_euclid(i64::from(p.fill_every)) == 0;
            let plays = match step.condition {
                Condition::Always => true,
                Condition::Every { nth, of } => pass.rem_euclid(i64::from(of.max(1))) == i64::from(nth) - 1,
                Condition::First => pass == 0,
                Condition::NotFirst => pass > 0,
                Condition::Fill => fill,
                Condition::NotFill => !fill,
            };
            if !plays || unit(&mut chance) * 100.0 >= f64::from(step.probability) {
                continue;
            }
            // Swing moves the off-beat steps of the pattern's own grid.
            let grid_pos = (start / grid).round() as i64;
            let swing = if grid_pos.rem_euclid(2) == 1 { f64::from(p.shuffle) / 100.0 * grid / 2.0 } else { 0.0 };
            let nudge = f64::from(step.offset) / 100.0 * step_len;
            let base_vel = i32::from(step.velocity) + if step.accent { i32::from(p.accent) } else { 0 };
            let hits = u32::from(step.roll.max(1));
            let sub = step_len / f64::from(hits);
            let mut human = step_rng(p.seed, li, k, 1);
            for h in 0..hits {
                let jitter_t = humanize * human.next_signed() * step_len / 10.0;
                let jitter_v = humanize * human.next_signed() * 12.0;
                let t = start + swing + nudge + f64::from(h) * sub + jitter_t - at.phase as f64;
                let vel = (f64::from(base_vel) + jitter_v).round().clamp(1.0, 127.0) as u8;
                let length = (sub / 2.0).max(1.0).round() as i64;
                let mut push = |t: f64, velocity: u8| -> bool {
                    let tick = t.round() as i64;
                    if (0..at.length).contains(&tick) {
                        if seq.notes.len() >= at.max_notes {
                            return false;
                        }
                        seq.notes.push(Note { pitch: lane.note, velocity, release_velocity: 64, channel: CHANNEL, start: tick, length });
                    }
                    true
                };
                if step.flam && h == 0 && !push(t - flam_ticks, ((f64::from(vel) * 0.6).round() as u8).max(1)) {
                    break 'lanes;
                }
                if !push(t, vel) {
                    break 'lanes;
                }
            }
        }
    }
    seq.sort();
    seq
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const BAR: i64 = 4 * PPQ as i64;

    fn at(bars: i64) -> Expand {
        Expand { ticks_per_bar: BAR, length: bars * BAR, phase: 0, max_notes: 1 << 20 }
    }

    fn four_on_the_floor() -> Pattern {
        let mut lane = Lane::new(36, 16);
        for i in [0, 4, 8, 12] {
            lane.steps[i] = Step::hit(100);
        }
        Pattern { lanes: vec![lane], ..Pattern::default() }
    }

    #[test]
    fn a_kick_on_every_beat() {
        let seq = expand(&four_on_the_floor(), &at(2));
        let starts: Vec<i64> = seq.notes.iter().map(|n| n.start).collect();
        assert_eq!(starts, (0..8).map(|b| b * PPQ as i64).collect::<Vec<_>>());
        assert!(seq.notes.iter().all(|n| n.pitch == 36 && n.velocity == 100 && n.channel == CHANNEL));
    }

    #[test]
    fn lanes_of_different_lengths_drift_apart() {
        let mut p = Pattern::default();
        let mut three = Lane::new(38, 3);
        three.steps[0] = Step::hit(90);
        p.lanes.push(three);
        let seq = expand(&p, &at(1));
        let step = BAR / 16;
        let starts: Vec<i64> = seq.notes.iter().map(|n| n.start).collect();
        assert_eq!(starts, vec![0, 3 * step, 6 * step, 9 * step, 12 * step, 15 * step]);
    }

    #[test]
    fn shuffle_delays_only_the_off_beats() {
        let mut p = Pattern::default();
        let mut hats = Lane::new(42, 16);
        hats.steps.iter_mut().for_each(|s| *s = Step::hit(80));
        p.lanes.push(hats);
        p.shuffle = 100;
        let seq = expand(&p, &at(1));
        let step = BAR / 16;
        for (i, n) in seq.notes.iter().enumerate() {
            let want = i as i64 * step + if i % 2 == 1 { step / 2 } else { 0 };
            assert_eq!(n.start, want, "step {i}");
        }
    }

    #[test]
    fn accent_roll_and_flam() {
        let mut p = Pattern { accent: 27, ..Pattern::default() };
        let mut lane = Lane::new(38, 16);
        lane.steps[4] = Step { accent: true, roll: 2, flam: true, ..Step::hit(100) };
        p.lanes.push(lane);
        let seq = expand(&p, &at(1));
        let step = BAR / 16;
        let notes: Vec<(i64, u8)> = seq.notes.iter().map(|n| (n.start, n.velocity)).collect();
        assert_eq!(notes, vec![(4 * step - i64::from(PPQ / 16), 76), (4 * step, 127), (4 * step + step / 2, 127)]);
    }

    #[test]
    fn conditions_follow_the_lanes_passes_and_fill_bars() {
        let mut p = Pattern { fill_every: 4, ..Pattern::default() };
        let mut lane = Lane::new(49, 16);
        lane.steps[0] = Step { condition: Condition::Every { nth: 1, of: 2 }, ..Step::hit(100) };
        lane.steps[8] = Step { condition: Condition::Fill, ..Step::hit(100) };
        p.lanes.push(lane);
        let seq = expand(&p, &at(8));
        let bars: Vec<i64> = seq.notes.iter().map(|n| n.start / BAR).collect();
        // Bars 0, 2, 4, 6 from the first step; the fill step in bars 3 and 7.
        assert_eq!(bars, vec![0, 2, 3, 4, 6, 7]);
    }

    #[test]
    fn a_trimmed_clip_keeps_its_place_in_the_bar() {
        let whole = expand(&four_on_the_floor(), &at(2));
        let trimmed = expand(&four_on_the_floor(), &Expand { phase: PPQ as i64 / 2, length: BAR, ..at(1) });
        let want: Vec<i64> = whole.notes.iter().map(|n| n.start - PPQ as i64 / 2).filter(|t| (0..BAR).contains(t)).collect();
        assert_eq!(trimmed.notes.iter().map(|n| n.start).collect::<Vec<_>>(), want);
    }

    #[test]
    fn reroll_changes_the_dice_and_nothing_else() {
        let mut p = Pattern::default();
        let mut lane = Lane::new(42, 16);
        lane.steps.iter_mut().for_each(|s| *s = Step { probability: 50, ..Step::hit(90) });
        p.lanes.push(lane);
        let a = expand(&p, &at(16));
        assert_eq!(a, expand(&p, &at(16)), "the same pattern expands the same way");
        let mut q = p.clone();
        q.reroll();
        assert_ne!(a, expand(&q, &at(16)));
        // About half the steps play.
        let n = a.notes.len() as f64 / (16.0 * 16.0);
        assert!((0.35..0.65).contains(&n), "{n}");
    }

    #[test]
    fn note_limit_is_respected() {
        let mut p = four_on_the_floor();
        p.lanes[0].steps.iter_mut().for_each(|s| *s = Step { roll: 4, ..Step::hit(100) });
        let seq = expand(&p, &Expand { max_notes: 10, ..at(4) });
        assert_eq!(seq.notes.len(), 10);
    }

    #[test]
    fn hostile_patterns_are_sanitized() {
        let mut p = Pattern { steps_per_bar: 7, shuffle: 255, humanize: 255, ..Pattern::default() };
        p.lanes = vec![Lane { note: 200, steps: Vec::new(), ..Lane::default() }; 40];
        p.lanes[0].steps =
            vec![Step { velocity: 0, probability: 255, offset: -128, roll: 0, condition: Condition::Every { nth: 9, of: 0 }, ..Step::hit(1) }; 100];
        let seq = expand(&p, &at(2));
        assert!(seq.notes.iter().all(|n| n.pitch <= 127 && (1..=127).contains(&n.velocity) && (0..2 * BAR).contains(&n.start)));
    }

    #[test]
    fn serde_fills_missing_fields() {
        let p: Pattern = match serde_json::from_str(r#"{"lanes":[{"note":38,"steps":[{"on":true}]}]}"#) {
            Ok(p) => p,
            Err(e) => panic!("{e}"),
        };
        assert_eq!(p.steps_per_bar, 16);
        assert_eq!(p.lanes[0].steps[0], Step { on: true, ..Step::default() });
    }

    fn arb_step() -> impl Strategy<Value = Step> {
        (any::<bool>(), any::<u8>(), any::<bool>(), any::<u8>(), any::<i8>(), any::<u8>(), any::<bool>()).prop_map(
            |(on, velocity, accent, probability, offset, roll, flam)| Step {
                on,
                velocity,
                accent,
                probability,
                offset,
                roll,
                flam,
                condition: Condition::Always,
            },
        )
    }

    proptest! {
        #[test]
        fn every_note_is_inside_the_clip(
            steps in prop::collection::vec(arb_step(), 1..40),
            spb in prop::sample::select(STEPS_PER_BAR.to_vec()),
            shuffle in 0u8..=100,
            humanize in 0u8..=100,
            seed in any::<u64>(),
            phase in 0i64..10_000,
            bars in 1i64..6,
            numerator in 1i64..8,
            dir in prop::sample::select(vec![Direction::Forward, Direction::Reverse, Direction::PingPong, Direction::Random]),
            rate in prop::sample::select(vec![Rate::Half, Rate::Normal, Rate::Double]),
        ) {
            let lane = Lane { note: 40, steps, direction: dir, rate, mute: false };
            let p = Pattern { steps_per_bar: spb, shuffle, humanize, seed, lanes: vec![lane], ..Pattern::default() };
            let at = Expand { ticks_per_bar: numerator * PPQ as i64, length: bars * numerator * PPQ as i64, phase, max_notes: 100_000 };
            let seq = expand(&p, &at);
            prop_assert_eq!(&seq, &expand(&p, &at));
            for n in &seq.notes {
                prop_assert!((0..at.length).contains(&n.start));
                prop_assert!(n.length > 0 && (1..=127).contains(&n.velocity));
            }
        }
    }
}

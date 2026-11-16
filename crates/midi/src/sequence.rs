use serde::{Deserialize, Serialize};

/// A MIDI note. Times are in model ticks ([`crate::PPQ`] per quarter note).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// MIDI pitch 0..=127 (60 = middle C, shown as "C3").
    pub pitch: u8,
    /// Note-on velocity 1..=127.
    pub velocity: u8,
    /// Note-off (release) velocity 0..=127; 64 when the source had none.
    pub release_velocity: u8,
    /// MIDI channel 0..=15.
    pub channel: u8,
    /// Start in ticks.
    pub start: i64,
    /// Length in ticks; always > 0.
    pub length: i64,
    /// A muted note stays in the clip and the editor (drawn faint) but does not play and is
    /// left out of MIDI export. Absent in sessions saved before notes could be muted.
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    /// The chance, in percent (0-100), that the note plays on a pass: rolled each pass by a
    /// seeded generator, so a bounce repeats. 100 (always) in sessions saved before.
    #[serde(default = "always", skip_serializing_if = "is_always")]
    pub probability: u8,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn always() -> u8 {
    100
}

fn is_always(p: &u8) -> bool {
    *p >= 100
}

impl Note {
    /// Whether the note plays on pass `pass` (its probability rolled by a generator seeded from
    /// the note, its clip `salt` and the pass; the same answer every time for the same inputs).
    pub fn plays_on(&self, salt: u64, pass: u64) -> bool {
        if self.probability >= 100 {
            return true;
        }
        if self.probability == 0 {
            return false;
        }
        // SplitMix64 over the inputs: stable across runs and platforms.
        let mut x =
            salt ^ (self.start as u64).rotate_left(17) ^ (u64::from(self.pitch) << 48) ^ (u64::from(self.channel) << 56) ^ pass.rotate_left(31);
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 31;
        (x % 100) < u64::from(self.probability)
    }
}

impl Note {
    /// Tick at which the note ends (`start + length`, saturating).
    pub fn end(&self) -> i64 {
        self.start.saturating_add(self.length)
    }
}

/// Kind of a continuous / channel controller event.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum CtrlKind {
    /// Control change with the controller number 0..=127. Value 0..=127.
    Cc(u8),
    /// Pitch bend. Value is centred: -8192..=8191 (0 = no bend).
    PitchBend,
    /// Channel (mono) aftertouch. Value 0..=127.
    ChannelPressure,
    /// Polyphonic aftertouch for the given pitch. Value 0..=127.
    PolyPressure(u8),
    /// Program change. Value is the program number 0..=127.
    Program,
}

/// A controller / channel event at a tick.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CtrlEvent {
    pub tick: i64,
    pub channel: u8,
    pub kind: CtrlKind,
    pub value: i32,
    /// How the value goes on to the next event of this lane and channel: held (the default, and
    /// every event saved before ramps existed) or a ramp.
    #[serde(default, skip_serializing_if = "CtrlShape::is_step")]
    pub shape: CtrlShape,
}

/// How a controller's value goes from one event to the next event of its lane and channel.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum CtrlShape {
    /// Held until the next event, as MIDI itself does.
    #[default]
    Step,
    /// A ramp to the next event's value, bent by the curve (-1..1: 0 straight, above 0 starts
    /// fast, below 0 starts slow). It plays as a stream of values; MIDI files get it as steps
    /// ([`Sequence::stepped_ctrls`]).
    Ramp(f32),
}

impl CtrlShape {
    pub fn is_step(&self) -> bool {
        matches!(self, CtrlShape::Step)
    }

    /// The share of the way to the next value at `x` (0..1) along the segment (0 for a step).
    pub fn at(self, x: f64) -> f64 {
        let x = if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 };
        match self {
            CtrlShape::Step => 0.0,
            CtrlShape::Ramp(c) => {
                let k = if c.is_finite() { f64::from(c.clamp(-1.0, 1.0)) * 5.0 } else { 0.0 };
                if k.abs() < 1e-6 { x } else { (1.0 - (-k * x).exp()) / (1.0 - (-k).exp()) }
            }
        }
    }

    /// The value at `x` (0..1) of a segment from `a` to `b`, rounded.
    pub fn value(self, a: i32, b: i32, x: f64) -> i32 {
        (f64::from(a) + (f64::from(b) - f64::from(a)) * self.at(x)).round() as i32
    }
}

impl CtrlEvent {
    /// Whether `other` is on the same lane (kind) and channel.
    pub fn same_lane(&self, other: &CtrlEvent) -> bool {
        self.kind == other.kind && self.channel == other.channel
    }
}

/// The MIDI content of a region or track.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    /// Notes, sorted by `(start, pitch)` after [`Sequence::sort`].
    pub notes: Vec<Note>,
    /// Controller events, sorted by tick after [`Sequence::sort`].
    pub ctrls: Vec<CtrlEvent>,
}

fn kind_key(k: CtrlKind) -> (u8, u8) {
    match k {
        CtrlKind::Cc(n) => (0, n),
        CtrlKind::PitchBend => (1, 0),
        CtrlKind::ChannelPressure => (2, 0),
        CtrlKind::PolyPressure(p) => (3, p),
        CtrlKind::Program => (4, 0),
    }
}

impl Sequence {
    /// Sorts notes by `(start, pitch, channel, length, velocity)` and controller events by tick
    /// (stable, so same-tick events keep their order).
    pub fn sort(&mut self) {
        self.notes.sort_by_key(|a| (a.start, a.pitch, a.channel, a.length, a.velocity));
        self.ctrls.sort_by_key(|c| c.tick);
    }

    /// The last tick covered by any note end or controller event (0 when empty).
    pub fn end_tick(&self) -> i64 {
        let n = self.notes.iter().map(Note::end).max();
        let c = self.ctrls.iter().map(|c| c.tick).max();
        match (n, c) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => 0,
        }
    }

    /// The event after `ctrls[i]` on its lane and channel (where a ramp from it goes).
    pub fn next_in_lane(&self, i: usize) -> Option<&CtrlEvent> {
        let a = self.ctrls.get(i)?;
        self.ctrls.iter().skip(i + 1).find(|b| b.same_lane(a))
    }

    /// The controller events with every ramp written out as steps: the value where it changes, at
    /// most one every `step` ticks (as a MIDI file or a device without ramps takes them). Every
    /// event comes back as a step; sorted by tick.
    pub fn stepped_ctrls(&self, step: i64) -> Vec<CtrlEvent> {
        const MAX: usize = 1_000_000;
        let step = step.clamp(1, 3840);
        let mut out: Vec<CtrlEvent> = Vec::with_capacity(self.ctrls.len());
        for (i, a) in self.ctrls.iter().enumerate() {
            out.push(CtrlEvent { shape: CtrlShape::Step, ..*a });
            if a.shape.is_step() {
                continue;
            }
            let Some(b) = self.next_in_lane(i).filter(|b| b.tick > a.tick) else { continue };
            let span = (b.tick - a.tick) as f64;
            let mut last = a.value;
            let mut t = a.tick.saturating_add(step);
            while t < b.tick && out.len() < MAX {
                let v = a.shape.value(a.value, b.value, (t - a.tick) as f64 / span);
                if v != last {
                    out.push(CtrlEvent { tick: t, value: v, shape: CtrlShape::Step, ..*a });
                    last = v;
                }
                t = t.saturating_add(step);
            }
        }
        out.sort_by_key(|c| c.tick);
        out
    }

    /// Notes whose start lies in the half-open range `[start, end)`.
    pub fn notes_in(&self, start: i64, end: i64) -> impl Iterator<Item = &Note> + '_ {
        self.notes.iter().filter(move |n| n.start >= start && n.start < end)
    }

    /// Notes that sound anywhere inside the half-open range `[start, end)`.
    pub fn notes_overlapping(&self, start: i64, end: i64) -> impl Iterator<Item = &Note> + '_ {
        self.notes.iter().filter(move |n| n.start < end && n.end() > start)
    }

    /// Removes duplicate notes (same channel, pitch and start; the longest one is kept) and
    /// duplicate controller events (same tick, channel and kind; the last one is kept, matching
    /// what a synth would end up with). Returns how many events were removed. The sequence is
    /// left sorted.
    pub fn remove_duplicates(&mut self) -> usize {
        let before = self.notes.len() + self.ctrls.len();
        // Longest first within each (start, pitch, channel) group, then keep the first.
        self.notes.sort_by(|a, b| (a.start, a.pitch, a.channel, b.length).cmp(&(b.start, b.pitch, b.channel, a.length)));
        self.notes.dedup_by(|later, kept| later.start == kept.start && later.pitch == kept.pitch && later.channel == kept.channel);

        self.ctrls.sort_by_key(|c| c.tick);
        let mut kept: Vec<CtrlEvent> = Vec::with_capacity(self.ctrls.len());
        for c in self.ctrls.drain(..) {
            // Within a tick, replace an earlier event of the same channel+kind.
            // Only events at the same tick (the tail of `kept`) can match.
            let slot =
                kept.iter_mut().rev().take_while(|k| k.tick == c.tick).find(|k| k.channel == c.channel && kind_key(k.kind) == kind_key(c.kind));
            match slot {
                Some(slot) => *slot = c,
                None => kept.push(c),
            }
        }
        self.ctrls = kept;
        self.sort();
        before - (self.notes.len() + self.ctrls.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(tick: i64, value: i32, shape: CtrlShape) -> CtrlEvent {
        CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(1), value, shape }
    }

    #[test]
    fn events_saved_before_ramps_load_as_steps_and_steps_save_as_before() {
        let old: CtrlEvent = serde_json::from_str(r#"{"tick":5,"channel":0,"kind":{"Cc":1},"value":9}"#).unwrap();
        assert_eq!(old.shape, CtrlShape::Step);
        assert_eq!(serde_json::to_string(&old).unwrap(), r#"{"tick":5,"channel":0,"kind":{"Cc":1},"value":9}"#);
        let ramp = ev(0, 0, CtrlShape::Ramp(0.5));
        let back: CtrlEvent = serde_json::from_str(&serde_json::to_string(&ramp).unwrap()).unwrap();
        assert_eq!(back, ramp);
    }

    #[test]
    fn a_notes_probability_defaults_to_always_and_rolls_the_same_for_the_same_pass() {
        let old: Note = serde_json::from_str(r#"{"pitch":60,"velocity":100,"release_velocity":64,"channel":0,"start":0,"length":10}"#).unwrap();
        assert_eq!(old.probability, 100);
        assert!(!serde_json::to_string(&old).unwrap().contains("probability"));
        let half = Note { probability: 50, ..old };
        let rolls = |pass: u64| (0..1000).filter(|i| Note { start: *i, ..half }.plays_on(7, pass)).count();
        assert!((400..=600).contains(&rolls(1)), "{}", rolls(1));
        assert_eq!(rolls(1), rolls(1));
        assert!((0..1000).any(|i| Note { start: i, ..half }.plays_on(7, 1) != Note { start: i, ..half }.plays_on(7, 2)));
        assert!((0..100).all(|i| Note { start: i, ..old }.plays_on(1, i as u64)));
        assert!((0..100).all(|i| !Note { start: i, probability: 0, ..old }.plays_on(1, i as u64)));
    }

    #[test]
    fn ramps_write_out_as_thinned_steps() {
        let other = CtrlEvent { kind: CtrlKind::Cc(7), ..ev(50, 3, CtrlShape::Step) };
        let seq = Sequence { notes: Vec::new(), ctrls: vec![ev(0, 0, CtrlShape::Ramp(0.0)), other, ev(1000, 100, CtrlShape::Step)] };
        assert_eq!(seq.next_in_lane(0).map(|c| c.tick), Some(1000));
        let out = seq.stepped_ctrls(10);
        let cc1: Vec<(i64, i32)> = out.iter().filter(|c| c.kind == CtrlKind::Cc(1)).map(|c| (c.tick, c.value)).collect();
        assert!(out.iter().all(|c| c.shape.is_step()));
        assert_eq!(cc1.first(), Some(&(0, 0)));
        assert_eq!(cc1.last(), Some(&(1000, 100)));
        // One event a value at most, no closer than 10 ticks, rising in a straight line.
        assert!(cc1.windows(2).all(|w| w[1].0 - w[0].0 >= 10 && w[1].1 > w[0].1), "{cc1:?}");
        assert_eq!(cc1.iter().find(|c| c.0 == 500).map(|c| c.1), Some(50));
        assert!(out.iter().any(|c| c.kind == CtrlKind::Cc(7)));
        // A step stays one event; a curve that starts fast is above the line halfway.
        let steps = Sequence { notes: Vec::new(), ctrls: vec![ev(0, 0, CtrlShape::Step), ev(1000, 100, CtrlShape::Step)] };
        assert_eq!(steps.stepped_ctrls(10).len(), 2);
        assert!(CtrlShape::Ramp(1.0).value(0, 100, 0.5) > 60 && CtrlShape::Ramp(-1.0).value(0, 100, 0.5) < 40);
    }
}

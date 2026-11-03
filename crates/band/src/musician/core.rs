//! `MusicianCore.cpp`: utilities, harmony, step buffer, feel, interplay, dispatch, parameters, transport.
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.
//! Differences of form: the C++ `sink` is `out` (`sink == nullptr` never holds); `song` / `shared` are `cx.song` /
//! `cx.shared`; `cur` is a copy of the entry; a C++ `vector[(size_t) i]` on the song reads through `get` (an index
//! the C++ never lets go out of range reads a default here instead of being UB).

// Line-by-line translation: the C++ shapes (a default then field writes, a counter beside a loop, a late init) are kept.
#![allow(clippy::field_reassign_with_default, clippy::explicit_counter_loop, clippy::needless_late_init)]

use super::*;
use crate::presets::PROGRESSION_PRESETS;
use crate::song_feel::FeelOverride;
use crate::song_feel::{effective_energy, effective_fills, effective_pickups, effective_space, effective_speed, resolve, shift_for};
use crate::tension;

/// `song->sections[(size_t) i]` (None out of range).
fn section_at(song: &SongTimeline, i: i32) -> Option<&SongSection> {
    usize::try_from(i).ok().and_then(|k| song.sections.get(k))
}

/// `song->entries[(size_t) i]` (a default entry out of range).
fn entry_at(song: &SongTimeline, i: i32) -> TimelineEntry {
    usize::try_from(i).ok().and_then(|k| song.entries.get(k)).copied().unwrap_or_default()
}

fn len_i32<T>(v: &[T]) -> i32 {
    i32::try_from(v.len()).unwrap_or(i32::MAX)
}

impl Musician {
    // ---- Utilities -----------------------------------------------------------
    /// `int positiveModulo (int value, int divisor)`
    pub(crate) fn positive_modulo(&self, value: i32, divisor: i32) -> i32 {
        if divisor <= 0 {
            return 0;
        }
        let mut result = value % divisor;
        if result < 0 {
            result += divisor;
        }
        result
    }

    /// `int floorToInt (float64 value)`
    pub(crate) fn floor_to_int(&self, value: f64) -> i32 {
        let mut result = value as i32;
        if f64::from(result) > value {
            result -= 1;
        }
        result
    }

    /// `int floorDiv (int value, int divisor)`
    pub(crate) fn floor_div(&self, value: i32, divisor: i32) -> i32 {
        self.floor_to_int(f64::from(value) / f64::from(divisor))
    }

    /// `float getBarLengthPpq ()`
    pub(crate) fn get_bar_length_ppq(&self) -> f32 {
        info(self.meter).bar_ppq
    }

    // Speed: native 12-step grids are per bar. A player on its own grids12 in this meter steps a 16th per step whatever
    // Speed says (one grid bar = one heard bar); the adapted (mapped) path keeps Half, and plays
    // Double as Normal (the adapted map assumes one grid bar per heard bar; at Double the backbeat landed on 3 / 9).
    /// `float tickPpq ()`
    pub(crate) fn tick_ppq(&self, lib: &LibraryData) -> f32 {
        if self.native_here(self.active_player(lib), lib) || (is12(self.meter) && self.speed == 2) {
            return 0.25;
        }
        if self.speed == 0 {
            0.5
        } else if self.speed == 2 {
            0.125
        } else {
            0.25
        }
    }

    /// `float64 ppqPerMs ()`
    pub(crate) fn ppq_per_ms(&self) -> f64 {
        f64::from(self.host_tempo) / 60000.0
    }

    /// `bool isPhraseEnd (int progBar)`
    pub(crate) fn is_phrase_end(&self, prog_bar: i32) -> bool {
        if self.force_phrase_end && prog_bar == self.cur_prog_bar {
            return true;
        }
        let n = self.num_bars();
        prog_bar == n - 1 || (n >= 8 && (prog_bar % 8 == 3 || prog_bar % 8 == 7))
    }

    // Hemiola at phrase ends (spec 6, Task 17): on an isPhraseEnd bar a 12-step meter may regroup the bar for the bass and
    // the keys, 3/4 as two dotted quarters (2 x 3 eighths: 0, 6), 6/8 as three quarters (0, 4, 8). One band-wide draw per
    // bar (salt 1999, only here, so 4/4 never draws it), chance up to 25 % at Energy 4. Native grids12 play as written.
    /// `bool hemiolaBar (int who, int pass, int progBar)`
    pub(crate) fn hemiola_bar(&self, who: i32, pass: i32, prog_bar: i32, cx: &Ctx<'_>) -> bool {
        if !is12(self.meter) || self.native_here(who, cx.lib) || !self.is_phrase_end(prog_bar) {
            return false;
        }
        self.rand01(pass, prog_bar, 1999, 0, cx) < 0.25f32 * clamp(self.energy, 1, 4) as f32 / 4.0f32
    }

    /// `int numBars ()`
    pub(crate) fn num_bars(&self) -> i32 {
        match self.cur {
            Some(c) => c.section_chords,
            None => {
                if self.bars_mode == 1 {
                    8
                } else {
                    4
                }
            }
        }
    }

    /// `bool isChordal (int p)`
    pub(crate) fn is_chordal(&self, p: i32) -> bool {
        p == 0 || p == 3 || p == 4 || p == 6 || p == 7 || p == 9
    }

    /// `int activePlayer ()`
    pub(crate) fn active_player(&self, lib: &LibraryData) -> i32 {
        // The plugin picks players by seed (-1 = the Amorph slot path); a seed with no persona plays as Session.
        if self.seed_override >= 0 {
            if self.seed_override == 0 || lib.persona(self.seed_override).is_none() {
                return 0;
            }
            if !lib.plays(self.pattern, self.seed_override) {
                // the instrument just changed: its first player until the
                // message thread picks (no other persona on it meanwhile)
                let k = usize::try_from(clamp(self.pattern, 0, K_INSTRUMENTS as i32 - 1)).unwrap_or(0);
                for &s in lib.players.get(k).map_or(&[][..], |v| v.as_slice()) {
                    if s > 0 && lib.persona(s).is_some() {
                        return s;
                    }
                }
                return 0;
            }
            return self.seed_override;
        }
        if self.player < 1 || self.player > K_PLAYER_SLOTS as i32 {
            return 0;
        }
        let k = usize::try_from(clamp(self.pattern, 0, K_INSTRUMENTS as i32 - 1)).unwrap_or(0);
        lib.roster.get(k).map_or(0, |r| at_or_default(r, self.player - 1))
    }

    /// `void annotate (int kind, float64 ppq)`
    pub(crate) fn annotate(&mut self, kind: i32, ppq: f64, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = cx;
        if !self.rendering || self.dry_run {
            return;
        }
        let slot = if kind == 1 {
            0
        } else if kind == 2 {
            1
        } else if kind == 4 {
            2
        } else {
            3
        };
        let bar = self.floor_to_int((ppq - self.bar_origin + 1.0e-6) / max(1.0e-6, f64::from(self.get_bar_length_ppq())));
        if self.annotated_bar[slot] == bar {
            return;
        }
        self.annotated_bar[slot] = bar;
        out.annotate(self.index, kind, ppq);
    }

    /// `int effectiveTake () const`
    pub(crate) fn effective_take(&self, cx: &Ctx<'_>) -> i32 {
        // A/B Takes: the looped section plays Take A or B, per pass (the switch lands where the loop does).
        if self.perf.ab_on
            && let Some(c) = self.cur
            && !self.live_mode
            && let Some(song) = cx.song
            && c.section >= 0
            && c.section < len_i32(&song.sections)
            && section_at(song, c.section).is_some_and(|s| s.id == self.perf.ab_section)
        {
            return if (self.perf.ab_flip_at >= 0.0 && self.step_ppq_now >= self.perf.ab_flip_at - 1.0e-9) != self.perf.ab_now_b {
                self.perf.ab_b
            } else {
                self.perf.ab_a
            };
        }
        match self.cur {
            Some(c) if !self.live_mode && c.take > 0 => c.take,
            _ => self.take,
        }
    }

    /// `float rand01 (int a, int b, int c, int d)`
    pub(crate) fn rand01(&self, a: i32, b: i32, c: i32, d: i32, cx: &Ctx<'_>) -> f32 {
        let mut x: i64 = i64::from(a) * 73856093
            + i64::from(b) * 19349663
            + i64::from(c) * 83492791
            + i64::from(d) * 2654435
            + i64::from(self.effective_take(cx)) * 961748927;
        x &= 0xffffffff;
        x = (((x >> 16) ^ x) * 0x45d9f3b) & 0xffffffff;
        x = (((x >> 16) ^ x) * 0x45d9f3b) & 0xffffffff;
        x = (x >> 16) ^ x;
        (x & 0xffffff) as f32 / 16777216.0f32
    }

    /// `float randBell (int a, int b, int c, int d)`
    pub(crate) fn rand_bell(&self, a: i32, b: i32, c: i32, d: i32, cx: &Ctx<'_>) -> f32 {
        self.rand01(a, b, c, d, cx) + self.rand01(a, b, c, d + 7919, cx) - 1.0f32
    }

    // ---- Harmony -------------------------------------------------------------
    /// `int scaleOffsetIn (int mode, int degree)`
    pub(crate) fn scale_offset_in(&self, mode: i32, degree: i32) -> i32 {
        const MAJOR: CArr<i32, 7> = CArr::new([0, 2, 4, 5, 7, 9, 11]);
        const MINOR: CArr<i32, 7> = CArr::new([0, 2, 3, 5, 7, 8, 10]);
        let d = self.positive_modulo(degree, 7);
        let oct = self.floor_to_int(f64::from(degree) / 7.0);
        12 * oct + (if mode == 0 { MAJOR.val(d) } else { MINOR.val(d) })
    }

    /// `int scaleOffset (int degree)`
    pub(crate) fn scale_offset(&self, degree: i32) -> i32 {
        self.scale_offset_in(self.scale_mode, degree)
    }

    /// `bool inScaleMask (int pitch, int mask)`
    pub(crate) fn in_scale_mask(&self, pitch: i32, mask: i32) -> bool {
        // mask 0 = current scale, 1 = blues (1 b3 4 b5 5 b7) on the key root
        const MAJOR: CArr<i32, 12> = CArr::new([1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 1]);
        const MINOR: CArr<i32, 12> = CArr::new([1, 0, 1, 1, 0, 1, 0, 1, 1, 0, 1, 0]);
        const BLUES: CArr<i32, 12> = CArr::new([1, 0, 0, 1, 0, 1, 1, 1, 0, 0, 1, 0]);
        let pc = self.positive_modulo(pitch - self.key_root, 12);
        if mask == 1 {
            return BLUES.val(pc) == 1;
        }
        if self.scale_mode == 0 { MAJOR.val(pc) == 1 } else { MINOR.val(pc) == 1 }
    }

    /// `int stepUp (int pitch, int mask)`
    pub(crate) fn step_up(&self, pitch: i32, mask: i32) -> i32 {
        let mut p = pitch + 1;
        for _ in 0..12 {
            if self.in_scale_mask(p, mask) {
                return p;
            }
            p += 1;
        }
        pitch + 1
    }

    /// `int stepDown (int pitch, int mask)`
    pub(crate) fn step_down(&self, pitch: i32, mask: i32) -> i32 {
        let mut p = pitch - 1;
        for _ in 0..12 {
            if self.in_scale_mask(p, mask) {
                return p;
            }
            p -= 1;
        }
        pitch - 1
    }

    /// `Chord buildChord (int slotValue)`
    pub(crate) fn build_chord(&self, slot_value: i32) -> Chord {
        let mut c = Chord::default();
        let v = clamp(slot_value, 0, 111);
        let degree = clamp(v / 16, 0, 6);
        let mut flavor = v % 16;
        if flavor > 8 {
            flavor = 0;
        }
        let mode = if flavor == 7 || flavor == 8 { 1 - self.scale_mode } else { self.scale_mode };
        let r = self.scale_offset_in(mode, degree);
        let third = self.scale_offset_in(mode, degree + 2) - r;
        let fifth = self.scale_offset_in(mode, degree + 4) - r;
        let seventh = self.scale_offset_in(mode, degree + 6) - r;

        c.degree = degree;
        c.flavor = flavor;
        c.root_pc = self.positive_modulo(r, 12);
        c.seventh = if flavor == 2 { 10 } else { seventh };
        c.count = 3;
        c.tones.set(0, 0);
        c.tones.set(1, third);
        c.tones.set(2, fifth);

        if flavor == 1 || flavor == 8 {
            c.tones.set(3, seventh);
            c.count = 4;
        } else if flavor == 2 {
            c.tones.set(1, 4);
            c.tones.set(2, 7);
            c.tones.set(3, 10);
            c.count = 4;
        } else if flavor == 3 {
            c.tones.set(1, 2);
        } else if flavor == 4 {
            c.tones.set(1, 5);
        } else if flavor == 5 {
            c.tones.set(3, 14);
            c.count = 4;
        } else if flavor == 6 {
            c.tones.set(3, 9);
            c.count = 4;
        }
        c
    }

    /// `int slotForBar (int progBar)`
    pub(crate) fn slot_for_bar(&self, prog_bar: i32, cx: &Ctx<'_>) -> i32 {
        if self.cur.is_some() {
            let e = self.entry_rel(prog_bar - self.cur_prog_bar, cx);
            if !self.live_mode && cx.song.and_then(|s| section_at(s, e.section)).is_some_and(|s| s.follow_idea) {
                return AUTHORED_ROOTS.val(self.variation * 4 + self.positive_modulo(e.chord_index, 4)) * 16;
            }
            return e.slot;
        }
        if self.progression == 0 {
            return AUTHORED_ROOTS.val(self.variation * 4 + self.positive_modulo(prog_bar, 4)) * 16;
        }
        self.slots.val(self.positive_modulo(prog_bar, self.num_bars()))
    }

    /// `Chord chordForBar (int progBar)`
    pub(crate) fn chord_for_bar(&self, prog_bar: i32, cx: &Ctx<'_>) -> Chord {
        if self.cur.is_some() {
            let mut c = self.build_chord(self.slot_for_bar(prog_bar, cx));
            c.bass_pc = self.entry_rel(prog_bar - self.cur_prog_bar, cx).bass;
            return c;
        }
        self.build_chord(self.slot_for_bar(self.positive_modulo(prog_bar, self.num_bars()), cx))
    }

    /// `bool sameChord (Chord a, Chord b)`
    pub(crate) fn same_chord(&self, a: Chord, b: Chord) -> bool {
        a.root_pc == b.root_pc && a.flavor == b.flavor && a.tones.val(1) == b.tones.val(1)
    }

    /// `Chord rawChord (int rootPc, int t1, int t2, int t3, int count)`
    pub(crate) fn raw_chord(&self, root_pc: i32, t1: i32, t2: i32, t3: i32, count: i32) -> Chord {
        let mut c = Chord::default();
        c.degree = 0;
        c.flavor = 0;
        c.root_pc = self.positive_modulo(root_pc, 12);
        c.count = count;
        c.seventh = t3;
        c.tones.set(0, 0);
        c.tones.set(1, t1);
        c.tones.set(2, t2);
        c.tones.set(3, t3);
        c
    }

    /// `Chord susOf (Chord c)`
    pub(crate) fn sus_of(&self, c: Chord) -> Chord {
        let mut s = c;
        s.tones.set(1, 5);
        s.flavor = 4;
        s
    }

    /// `int wrapShift (int d)`
    pub(crate) fn wrap_shift(&self, d: i32) -> i32 {
        let s = self.positive_modulo(d, 7);
        if s > 3 { s - 7 } else { s }
    }

    // ---- Step note buffer ----------------------------------------------------
    /// `void addStepNote (int note, float vel, float lenPpq, int hand, float shiftMs)`
    pub(crate) fn add_step_note(&mut self, note: i32, vel: f32, len_ppq: f32, hand: i32, shift_ms: f32) {
        self.add_step_note_f(note, vel, len_ppq, hand, shift_ms, self.note_intent);
    }

    /// `void addStepNoteF (int note, float vel, float lenPpq, int hand, float shiftMs, int flags)`
    pub(crate) fn add_step_note_f(&mut self, note: i32, vel: f32, len_ppq: f32, hand: i32, shift_ms: f32, flags: i32) {
        if self.step_count >= 16 {
            return;
        }
        for i in 0..self.step_count {
            if self.step_note.val(i) == note {
                return;
            }
        }
        let n = self.step_count;
        self.step_flags.set(n, flags | self.step_extra_flags);
        self.step_note.set(n, note);
        self.step_vel.set(n, vel);
        self.step_len.set(n, len_ppq);
        self.step_hand.set(n, hand);
        self.step_shift_ms.set(n, shift_ms);
        self.step_count += 1;
    }

    /// `int mapDrum (int note, int vel, int step)`
    pub(crate) fn map_drum(&self, note: i32, vel: i32, step: i32, lib: &LibraryData) -> i32 {
        let right = (step % 2) == 1;
        for r in &self.current_target(lib).drum_rules {
            if r.note != note || vel < r.min_vel || vel > r.max_vel {
                continue;
            }
            if (r.side == 1 && right) || (r.side == 2 && !right) {
                continue;
            }
            return clamp(if r.out >= 0 { r.out } else { note + r.offset }, 0, 127);
        }
        note
    }

    /// `float64 swingDelay (int stepInBar, int who)`
    pub(crate) fn swing_delay(&self, step_in_bar: i32, who: i32, lib: &LibraryData) -> f64 {
        let base = if self.step_shift.swing_centre >= 0.0f32 { self.step_shift.swing_centre } else { self.feel_for(who, lib).swing_base }; // the song's Groove
        let amt = if self.swing_amt <= 50.0f32 {
            50.0f32 + (base - 50.0f32) * (self.swing_amt / 50.0f32)
        } else {
            base + (75.0f32 - base) * ((self.swing_amt - 50.0f32) / 50.0f32)
        };
        if self.meter == Meter::M68 {
            // 6/8 lilt (spec 2.3, Task 17 polish): no pairs. Lean on each dotted beat's 1st eighth (the 2nd sits late), a slight
            // rush on the 2nd (the 3rd sits late by less, so the 2nd is the short one). 0 at swing 50 whatever the Feel; above
            // it the Feel's swing base scales it (a swung Feel lilts more). Max ~a quarter of a 16th.
            if self.swing_amt <= 50.0f32 {
                return 0.0;
            }
            let feel_k = clamp(0.5f32 + (base - 50.0f32) / 25.0f32, 0.5f32, 1.5f32);
            let lean = f64::from((min(self.swing_amt, 100.0f32) - 50.0f32) / 50.0f32 * 0.25f32 * (2.0f32 / 3.0f32) * feel_k * self.tick_ppq(lib));
            let e = self.positive_modulo(step_in_bar, 6);
            return if e == 2 || e == 3 {
                lean
            } else if e == 4 || e == 5 {
                lean * 0.35
            } else {
                0.0
            };
        }
        let ratio = clamp(amt, 50.0f32, 75.0f32) / 100.0f32;
        if ratio <= 0.5f32 {
            return 0.0;
        }
        let mut pair_steps = self.feel_for(who, lib).swing_steps;
        // 3/4: a pair must divide the 12-step bar (8 -> quarter pairs, 16 -> the bar), or steps 8-11 would wrap mid-pair.
        if self.meter == Meter::M34 && 12i32.checked_rem(pair_steps).unwrap_or(0) != 0 {
            pair_steps = if pair_steps > 12 {
                12
            } else if pair_steps == 8 || pair_steps == 5 {
                4
            } else {
                6
            };
        }
        let x = self.positive_modulo(step_in_bar, pair_steps) as f32 / pair_steps as f32;
        let warped = if x < 0.5f32 { x / 0.5f32 * ratio } else { ratio + (x - 0.5f32) / 0.5f32 * (1.0f32 - ratio) };
        f64::from((warped - x) * pair_steps as f32 * self.tick_ppq(lib))
    }

    // ---- Natural feel -----------------------------------------------------------------------------------------
    /// `float64 chordEndFor (float64 stepPpq)`
    pub(crate) fn chord_end_for(&self, step_ppq: f64) -> f64 {
        if self.cur.is_some() {
            return self.cur_end_ppq;
        }
        let len = max(1.0e-6, self.chord_len_ppq());
        (((step_ppq + 1.0e-6) / len).floor() + 1.0) * len
    }

    /// `bool pushHere (int who, int pass, float64 stepPpq)`
    pub(crate) fn push_here(&self, who: i32, pass: i32, step_ppq: f64, cx: &Ctx<'_>) -> bool {
        let amount = self.feel_for(who, cx.lib).groove.push * self.groove_now() / 50.0f32 * self.step_shift.push_mul; // the song's Pocket
        if amount <= 0.0f32 {
            return false;
        }
        let end = self.chord_end_for(step_ppq);
        if (step_ppq - (end - 0.5)).abs() > 1.0e-4 {
            return false; // the step on the last 8th, once
        }
        if let Some(c) = self.cur
            && let Some(song) = cx.song
            && !song.entries.is_empty()
        {
            // Inside one section only: a push never anticipates the next section, a kept take or a section this player
            // sits out, nor the song's final chord, a jump, an Ending or a Break landing on that downbeat.
            let nx = self.entry_rel(1, cx);
            if nx.arr_pos != c.arr_pos || self.jump_next_idx >= 0 {
                return false;
            }
            if let Some(sec) = section_at(song, clamp(nx.section, 0, len_i32(&song.sections) - 1))
                && sec.role == 2
                && nx.chord_index == nx.section_chords - 1
            {
                return false;
            }
        }
        if (self.perf.end_at >= 0.0 && (self.perf.end_at - end).abs() < 1.0e-3)
            || (self.perf.break_at >= 0.0 && (self.perf.break_at - end).abs() < 1.0e-3)
        {
            return false;
        }
        self.rand01(pass, self.floor_to_int(end * 4.0 + 0.5), 1401, 7, cx) < amount
    }

    /// `int grooveLead (std::array<float, 16>& out)`
    pub(crate) fn groove_lead(&self, out: &mut [f32; 16], lib: &LibraryData) -> i32 {
        if !self.enabled || self.muted || (!self.is_drums() && self.pattern != 2) {
            return -1;
        }
        let who = self.active_player(lib);
        for i in 0..self.spb() {
            if let Some(o) = usize::try_from(i).ok().and_then(|k| out.get_mut(k)) {
                *o = self.timing_at(who, i, lib) * self.groove_now() / 50.0f32;
            }
        }
        self.pattern
    }

    /// `int sectionBarOf (float64 stepPpq, int& bars)`
    pub(crate) fn section_bar_of(&self, step_ppq: f64, bars: &mut i32, cx: &Ctx<'_>) -> i32 {
        let (Some(c), Some(song)) = (self.cur, cx.song) else { return 0 };
        let bar = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
        let first = self.cur_index - c.chord_index;
        let n = len_i32(&song.entries);
        let e0 = entry_at(song, self.positive_modulo(first, n));
        let mut halves = 0;
        for i in 0..c.section_chords {
            halves += entry_at(song, self.positive_modulo(first + i, n)).len_half;
        }
        *bars = max(1, (halves + 1) / 2);
        let off = f64::from(c.start_half - e0.start_half) * bar * 0.5 + (step_ppq - self.cur_start_ppq);
        clamp(self.floor_to_int(off / bar + 1.0e-6), 0, *bars - 1)
    }

    /// `void decidePhrasing (int who, int pass, int progBar, float64 stepPpq)`
    pub(crate) fn decide_phrasing(&mut self, who: i32, pass: i32, prog_bar: i32, step_ppq: f64, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.phrase_mode = 0;
        self.pickup_now = false;
        self.pickup_prog_bar = prog_bar;
        let Some(c) = self.cur else { return };
        let Some(song) = cx.song else { return };
        if !self.perf.natural || self.perf.live || song.entries.is_empty() {
            return;
        }
        let g = &self.feel_for(who, cx.lib).groove;
        let space = effective_space(g.space, self.step_shift.space_add);
        let pickups = effective_pickups(g.pickups, self.step_shift.pickups_mul); // the song's Density
        if g.variation <= 0.0f32 && space <= 0.0f32 && pickups <= 0.0f32 {
            return;
        }
        let mut bars = 1;
        let bar = self.section_bar_of(step_ppq, &mut bars, cx);
        let key = c.arr_pos * 64 + bar;
        let fills_scale = min(2.0f32, self.fills / 50.0f32);
        if (bar % 8 == 3 || bar % 8 == 7) && g.variation > 0.0f32 && self.rand01(pass, key, 1501, who, cx) < g.variation * fills_scale {
            self.phrase_mode = 1 + min(2, self.floor_to_int(f64::from(self.rand01(pass, key, 1503, who, cx) * 3.0f32)));
        } else if space > 0.0f32 && bar % 8 != 7 && bar != bars - 1 && self.rand01(pass, key, 1505, who, cx) < space {
            self.phrase_mode = 3;
        }
        // The bar before the next section (another section, or this one again): a lead-in.
        if pickups > 0.0f32 && bar == bars - 1 && self.rand01(pass, key, 1507, who, cx) < pickups * min(1.0f32, fills_scale) {
            self.pickup_now = true;
        }
        if self.pickup_now {
            self.annotate(2, step_ppq, cx, out);
        }
        if self.phrase_mode != 0 {
            self.annotate(8, step_ppq, cx, out);
        }
    }

    /// `int sixteenthOf (float64 stepPpq)`
    pub(crate) fn sixteenth_of(&self, step_ppq: f64) -> i32 {
        let bar = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
        let in_bar = step_ppq - self.bar_origin - ((step_ppq - self.bar_origin + 1.0e-6) / bar).floor() * bar;
        self.positive_modulo(self.floor_to_int(in_bar * 4.0 + 0.5), self.spb())
    }

    /// `float grooveMsFor (int who, float64 stepPpq)`
    pub(crate) fn groove_ms_for(&self, who: i32, step_ppq: f64, cx: &Ctx<'_>) -> f32 {
        let scale = self.groove_now() / 50.0f32;
        let q = self.sixteenth_of(step_ppq);
        let own = self.timing_at(who, q, cx.lib) * scale;
        if self.is_drums() {
            return own;
        }
        let sh = &*cx.shared;
        let ref_;
        if sh.drum_known {
            ref_ = at_or_default(&sh.drum_groove, q);
        } else if self.pattern != 2 && sh.bass_known {
            ref_ = at_or_default(&sh.bass_groove, q);
        } else {
            return own;
        }
        let lock = if self.pattern == 2 {
            0.8f32
        } else if self.pattern == 0 || self.pattern == 3 || self.pattern == 4 || self.pattern == 6 {
            0.5f32
        } else {
            0.25f32
        };
        own + lock * (ref_ - own)
    }

    // ---- Interplay (Band feel switches; all off = none of this runs) ---------------
    /// `bool chordalInstrument (int p)`
    pub(crate) fn chordal_instrument(p: i32) -> bool {
        p == 0 || p == 3 || p == 4 || p == 6 || p == 7 || p == 12
    }

    /// `void applyInterplay (int who, int sib, float64 stepPpq)`
    pub(crate) fn apply_interplay(&mut self, who: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        let _ = sib;
        let lib = cx.lib;
        let me_i = usize::try_from(clamp(self.index, 0, 5)).unwrap_or(0);
        let tick = f64::from(self.tick_ppq(lib));
        let bar_l = f64::from(self.get_bar_length_ppq());
        let on_beat = |p: f64, grid: f64| {
            let r = p % grid;
            r < 1.0e-3 || grid - r < 1.0e-3
        };
        // What I tell the others: my part per bar (how busy, where it sits), my channel, what I am.
        let bar = self.floor_to_int((step_ppq - self.bar_origin) / bar_l + 1.0e-9);
        if bar != self.bar_seen {
            if self.bar_seen > -1000000 {
                let me = &mut cx.shared.m[me_i];
                me.busy = self.bar_notes;
                me.mean = if self.bar_notes > 0 { self.bar_note_sum / self.bar_notes as f32 } else { me.mean };
            }
            self.bar_seen = bar;
            self.bar_notes = 0;
            self.bar_note_sum = 0.0f32;
        }
        {
            let chordal = Self::chordal_instrument(self.pattern);
            let drums = self.is_drums();
            let me = &mut cx.shared.m[me_i];
            me.chordal = chordal;
            me.drums = drums;
            me.channel = self.midi_channel0 + 1;
            me.seen = step_ppq;
        }
        if self.is_drums() {
            for i in 0..self.step_count {
                if self.step_note.val(i) == 35 || self.step_note.val(i) == 36 {
                    cx.shared.add_kick(step_ppq);
                }
            }
        }

        // Fills don't collide: while the drummer fills, everyone else keeps to the beats.
        if self.perf.fills_yield
            && !self.is_drums()
            && !self.lick_now.on
            && step_ppq >= cx.shared.drum_fill_from - 1.0e-9
            && step_ppq < cx.shared.drum_fill_to
            && !on_beat(step_ppq, 1.0)
        {
            self.step_count = 0; // (a fiddle lick is a unit: it plays through)
        }
        // Bass locks to the kick: lean on it, play the root where it lands, soften the rest.
        if self.perf.bass_lock && self.pattern == 2 && cx.shared.drummer_around(step_ppq, bar_l) {
            let kick = cx.shared.kick_near(step_ppq, tick * 0.5);
            let kick_before = cx.shared.kick_near(step_ppq - tick, tick * 0.5);
            let in_drum_fill = self.perf.fills_yield
                && step_ppq >= cx.shared.drum_fill_from - 1.0e-9
                && step_ppq < cx.shared.drum_fill_to
                && !on_beat(step_ppq, 1.0);
            if kick && self.step_count == 0 && on_beat(step_ppq, 0.5) && !in_drum_fill {
                let ch = self.chord_for_bar(self.cur_prog_bar, cx);
                let b = self.lh_bass(who, ch, lib);
                self.add_step_note(b, 84.0f32, tick as f32 * 0.9f32, 0, 0.0f32);
            }
            for i in 0..self.step_count {
                let v = self.step_vel.val(i);
                self.step_vel.set(
                    i,
                    if kick {
                        min(127.0f32, v + 10.0f32)
                    } else if kick_before {
                        v
                    } else {
                        v * 0.7f32
                    },
                );
            }
        }
        // Leave space: when two chordal players are both busy, the lower-priority one moves up or thins out.
        let me = cx.shared.m[me_i];
        if self.perf.leave_space && me.chordal && me.busy > 8 {
            for j in 0..6i32 {
                if j == self.index {
                    continue;
                }
                let o = cx.shared.m[usize::try_from(j).unwrap_or(0)];
                if !o.chordal || o.busy <= 8 || step_ppq - o.seen > bar_l {
                    continue;
                }
                let they_lead = o.channel < me.channel || (o.channel == me.channel && j < self.index);
                if !they_lead {
                    continue;
                }
                if (me.mean - o.mean).abs() <= 7.0f32 && me.mean + 12.0f32 <= self.feel_for(who, lib).rh_high as f32 {
                    for i in 0..self.step_count {
                        if self.step_hand.val(i) == 1 {
                            let n = self.step_note.val(i);
                            self.step_note.set(i, n + 12);
                        }
                    }
                } else if !on_beat(step_ppq, 0.5) {
                    self.step_count = 0;
                }
                break;
            }
        }
        if step_ppq > self.busy_counted_ppq + 1.0e-9 || step_ppq < self.busy_counted_ppq - bar_l {
            // a regenerated step was counted already
            for i in 0..self.step_count {
                self.bar_notes += 1;
                self.bar_note_sum += self.step_note.val(i) as f32;
            }
            self.busy_counted_ppq = step_ppq;
        }
    }

    // ---- Step dispatch -------------------------------------------------------
    /// `void generateStep (int step)`
    pub(crate) fn generate_step(&mut self, step: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.generate_step_body(step, cx, out);
        self.jb_step_end(step, cx.lib);
        self.ks_step_end(step, cx.lib);
    }

    /// `void generateStepBody (int step)`
    pub(crate) fn generate_step_body(&mut self, step: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        if self.sits_out_meter(self.active_player(lib), lib) {
            self.last_sib_seen = max(self.last_sib_seen, self.step_in_bar(step, lib));
            return; // a 12-only player outside its meters: silent
        }
        let step_ppq = f64::from(step) * f64::from(self.tick_ppq(lib));
        if cx.song.is_some() {
            self.generate_song_step(step, step_ppq, cx, out);
            return;
        }
        let host_bar = self.floor_to_int((step_ppq + 0.0001) / self.chord_len_ppq());
        let pass = self.floor_div(host_bar, self.num_bars());
        let prog_bar = self.positive_modulo(host_bar, self.num_bars());
        let sib = self.step_in_bar(step, lib);
        self.last_sib_seen = max(self.last_sib_seen, sib);
        // Pickups and approach notes only make sense when this pattern's last step meets the bar change.
        self.pattern_meets_bar =
            self.floor_to_int((step_ppq + f64::from(self.spb() - sib) * f64::from(self.tick_ppq(lib)) + 0.0001) / self.chord_len_ppq()) != host_bar;
        let who = self.active_player(lib);

        if self.ujam_active(lib) {
            self.ujam_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
            return;
        }
        if who == 0 {
            self.session_step(step, pass, prog_bar, sib, step_ppq, cx, out);
        } else {
            self.persona_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
        }
    }

    /// `void generateSongStep (int step, float64 stepPpq)`
    pub(crate) fn generate_song_step(&mut self, step: i32, step_ppq: f64, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        let Some(song) = cx.song else { return };
        let sib = self.step_in_bar(step, lib);
        self.last_sib_seen = max(self.last_sib_seen, sib);
        self.step_ppq_now = step_ppq;
        let perf = self.perf;
        // Performance keys (all of these are off when nothing is performed: the Phase 2 path below).
        if perf.end_at >= 0.0 && step_ppq > perf.end_at + 1.0e-6 {
            // Ending: the downbeat rings for a bar, then everything stops (until the transport restarts).
            if !self.song_silent {
                self.song_silent = true;
                self.silence_at = perf.end_at + perf.bar_len;
            }
            return;
        }
        if perf.break_at >= 0.0 && step_ppq > perf.break_at + 1.0e-6 && step_ppq < perf.break_at + perf.bar_len - 1.0e-6 {
            return; // Break: rest
        }
        self.perf_fill = perf.fill_from >= 0.0 && step_ppq >= perf.fill_from - 1.0e-9 && step_ppq < perf.fill_to - 1.0e-9;
        self.perf_crash = (perf.fill_to >= 0.0 && (step_ppq - perf.fill_to).abs() < 1.0e-6)
            || (perf.break_at >= 0.0 && (step_ppq - perf.break_at).abs() < 1.0e-6)
            || (perf.end_at >= 0.0 && (step_ppq - perf.end_at).abs() < 1.0e-6); // the band hits it together
        self.hold_step = perf.end_at >= 0.0 && (step_ppq - perf.end_at).abs() < 1.0e-6;
        self.hold_len = perf.bar_len;
        self.stop_hit = (perf.break_at >= 0.0 && (step_ppq - perf.break_at).abs() < 1.0e-6) || self.hold_step; // the band stops on it
        self.ten_now = 0.0;
        self.ten_ramp = 0.0;
        self.ten_tail = false;
        self.rel_down = false; // tension is the song path's (never Live)
        let key_nudge = if perf.energy_land >= 0.0 && step_ppq >= perf.energy_land - 1.0e-9 { perf.energy_next } else { perf.energy_offset };
        let wheel_nudge = if perf.wheel_land >= 0.0 && step_ppq >= perf.wheel_land - 1.0e-9 { perf.wheel_next } else { perf.wheel_offset };
        let nudge = clamp(key_nudge + wheel_nudge, -3, 3);
        if perf.live {
            self.generate_live_step(step, step_ppq, sib, nudge, cx, out);
            return;
        }

        let off = if perf.jump_land >= 0.0 && step_ppq >= perf.jump_land - 1.0e-9 { perf.jump_new_offset } else { perf.jump_offset };
        let loc = self.song_locate(step_ppq + off, cx);
        if loc.ended {
            // Past the Ending: the last chord has rung out; silence until the transport restarts or moves back.
            if !self.song_silent {
                self.song_silent = true;
                self.silence_at = loc.start - off;
            }
            return;
        }
        let e = entry_at(song, loc.idx);
        self.cur = Some(e);
        self.cur_index = loc.idx;
        self.cur_prog_bar = e.chord_index;
        self.cur_global = loc.global;
        self.cur_start_ppq = loc.start - off;
        self.cur_end_ppq = loc.end - off; // host positions
        if song.has_ending {
            let last = len_i32(&song.entries) - 1;
            if loc.idx == last {
                // The Ending's last chord: hit it together (a crash) and let it ring to the end of the song.
                if step_ppq >= self.cur_start_ppq + f64::from(self.tick_ppq(lib)) - 1.0e-9 {
                    self.cur = None;
                    return;
                } // only its first step
                self.perf_crash = true;
                self.hold_step = true;
                self.hold_len = self.cur_end_ppq - step_ppq;
                self.stop_hit = true;
            } else if loc.idx == last - 1 && step_ppq >= self.cur_end_ppq - perf.bar_len - 1.0e-9 {
                self.perf_fill = true; // a fill into it
            }
        }
        let pass = loc.loop_ * song.arrangement_length + e.arr_pos;
        let prog_bar = e.chord_index;
        self.step_pass = pass;
        self.step_prog_bar = prog_bar;
        // A jump queued for the end of this chord: next is the target's first chord, and this is a phrase end.
        self.jump_next_idx = -1;
        self.force_phrase_end = false;
        if perf.jump_land >= 0.0 && step_ppq < perf.jump_land - 1.0e-9 && self.cur_end_ppq >= perf.jump_land - 1.0e-6 {
            self.jump_next_idx = perf.jump_entry;
            self.force_phrase_end = true;
            if step_ppq >= perf.jump_land - perf.bar_len - 1.0e-9 {
                self.perf_fill = true; // the band fills into a jump
            }
        }
        if perf.jump_land >= 0.0 && (step_ppq - perf.jump_land).abs() < 1.0e-6 {
            self.perf_crash = true; // ... and crashes where it lands
        }
        // Tension and release: how much this chord pulls, whether this is its last beat before the release, and
        // whether this step is a release's downbeat.
        self.ten_now = 0.0;
        self.ten_ramp = 0.0;
        self.ten_tail = false;
        self.rel_down = false;
        if perf.tension {
            self.ten_now = tension::tension_for(song, loc.idx, self.scale_mode);
            let bar_l = f64::from(self.get_bar_length_ppq());
            if self.ten_now > 0.0f32 {
                self.ten_ramp = clamp((step_ppq - max(self.cur_start_ppq, self.cur_end_ppq - bar_l)) / bar_l, 0.0, 1.0) as f32;
            }
            let ni = if self.jump_next_idx >= 0 { self.jump_next_idx } else { tension::next_entry(song, loc.idx) };
            self.ten_tail = ni >= 0 && tension::resolves_into(song, loc.idx, ni, self.scale_mode) && step_ppq >= self.cur_end_ppq - 1.0 - 1.0e-6;
            self.rel_down =
                step_ppq < self.cur_start_ppq + f64::from(self.tick_ppq(lib)) - 1.0e-6 && tension::is_release(song, loc.idx, self.scale_mode);
        }
        let pattern_end = step_ppq + f64::from(self.spb() - sib) * f64::from(self.tick_ppq(lib));
        if song.uniform_len_half > 0 {
            self.pattern_meets_bar = self.floor_to_int((pattern_end + off + 0.0001) / self.chord_len_ppq()) != loc.global;
        } else {
            self.pattern_meets_bar = pattern_end + 0.0001 >= self.cur_end_ppq;
        }
        if self.jump_next_idx >= 0 {
            self.pattern_meets_bar = pattern_end + 0.0001 >= perf.jump_land;
        }
        let who = self.active_player(lib);

        // Section feel: who plays, and the Energy this step plays at.
        let Some(sec) = section_at(song, e.section) else {
            self.cur = None;
            self.jump_next_idx = -1;
            self.force_phrase_end = false;
            return;
        };
        let mi = usize::try_from(clamp(self.index, 0, 5)).unwrap_or(0);
        let sm = sec.musicians[mi];
        if !sm.plays {
            if !self.song_silent {
                self.song_silent = true;
                self.silence_at = self.cur_start_ppq;
            } // notes stop where the section starts
            self.cur = None;
            self.jump_next_idx = -1;
            self.force_phrase_end = false;
            return;
        }
        self.song_silent = false;
        if self.silence_at >= 0.0 && step_ppq >= self.silence_at - 1.0e-9 {
            self.silence_at = -1.0; // it plays here after all
        }
        if let Some(fp) = sec.frozen[mi].as_deref()
            && fp.meter == self.meter as u8
        {
            // a take kept in another meter is refused: live plays
            let first = std::cmp::max(0, loc.idx - e.chord_index);
            let half = f64::from(self.get_bar_length_ppq()) * 0.5;
            let occ_start = self.cur_start_ppq - f64::from(e.start_half - entry_at(song, first).start_half) * half;
            let mut occ_len = 0.0f64;
            let mut i = first;
            while i < len_i32(&song.entries) && entry_at(song, i).arr_pos == e.arr_pos {
                occ_len += f64::from(entry_at(song, i).len_half) * half;
                i += 1;
            }
            self.play_frozen(fp, occ_start, occ_len, step_ppq, cx);
            self.cur = None;
            self.jump_next_idx = -1;
            self.force_phrase_end = false;
            return;
        }
        let base_energy = self.energy;
        self.og_base_energy = base_energy;
        self.og_nudge = nudge;
        self.energy = if sm.energy > 0 { clamp(sm.energy, 1, 4) } else { clamp(base_energy + sec.energy_shift + e.energy_add, 1, 4) };
        self.step_shift = self.feel_shift_for(who, Some(sec), cx); // the song's feel, this section's dials
        if sm.energy <= 0 && self.step_shift.energy_add != 0 {
            self.energy = effective_energy(self.energy, self.step_shift.energy_add); // a fixed Energy stays
        }
        if nudge != 0 {
            self.energy = clamp(self.energy + nudge, 1, 4);
        }
        let kept_fills = self.fills;
        let kept_length = self.length;
        let kept_dynamics = self.dynamics;
        let kept_velocity = self.velocity;
        if self.step_shift.any {
            self.fills = effective_fills(self.fills, self.step_shift.fills_mul);
            self.velocity = clamp((self.velocity as f32 * self.step_shift.vel_mul).round() as i32, 1, 127);
            self.length = clamp(self.length * self.step_shift.len_mul, 0.1f32, 1.0f32);
            self.dynamics = clamp(self.dynamics * self.step_shift.dyn_mul, 0.0f32, 100.0f32);
        }

        if self.ujam_active(lib) {
            self.ujam_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
        } else if who == 0 {
            self.session_step(step, pass, prog_bar, sib, step_ppq, cx, out);
        } else {
            self.persona_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
        }
        self.energy = base_energy;
        self.fills = kept_fills;
        self.length = kept_length;
        self.dynamics = kept_dynamics;
        self.velocity = kept_velocity;
        self.step_shift = FeelShift::default();
        self.cur = None;
        self.jump_next_idx = -1;
        self.force_phrase_end = false;
    }

    /// `void generateLiveStep (int step, float64 stepPpq, int sib, int nudge)`
    pub(crate) fn generate_live_step(&mut self, step: i32, step_ppq: f64, sib: i32, nudge: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        let perf = self.perf;
        let bar = if perf.bar_len > 0.0 { perf.bar_len } else { 4.0 }; // 16-only: fallback when Live hasn't set barLen
        let bar_idx = self.floor_to_int((step_ppq - perf.live_start + 0.0001) / bar);
        let every = if perf.fill_every == 8 { 8 } else { 4 };
        let prog_bar = self.positive_modulo(bar_idx, every);
        let pass = self.floor_div(bar_idx, every);
        let landed = perf.next_land >= 0.0 && step_ppq >= perf.next_land - 1.0e-9;
        let now_c = if landed { perf.next } else { perf.now };
        let next_c = if perf.next_land >= 0.0 && !landed { perf.next } else { now_c };
        let bar_start = perf.live_start + f64::from(bar_idx) * bar;
        let mut a = TimelineEntry::default();
        a.len_half = 2;
        a.chord_index = prog_bar;
        a.section_chords = every;
        a.slot = now_c.slot;
        a.bass = now_c.bass;
        let mut b = a;
        b.chord_index = prog_bar + 1;
        b.slot = next_c.slot;
        b.bass = next_c.bass;
        let prev_c = if landed { perf.now } else { perf.prev };
        let mut c = a;
        c.chord_index = prog_bar - 1;
        c.slot = prev_c.slot;
        c.bass = prev_c.bass;
        self.live_entries = [a, b, c];
        self.live_mode = true;
        self.cur = Some(a);
        self.cur_index = 0;
        self.cur_prog_bar = prog_bar;
        self.cur_global = bar_idx;
        self.cur_start_ppq = if landed && perf.next_land > bar_start { perf.next_land } else { bar_start };
        self.cur_end_ppq = bar_start + bar;
        self.step_pass = pass;
        self.step_prog_bar = prog_bar;
        self.pattern_meets_bar = step_ppq + f64::from(self.spb() - sib) * f64::from(self.tick_ppq(lib)) + 0.0001 >= self.cur_end_ppq;
        let who = self.active_player(lib);
        self.song_silent = false;
        let base_energy = self.energy;
        self.step_shift = self.feel_shift_for(who, None, cx); // Live: the song's feel (no section)
        self.energy = clamp(effective_energy(base_energy, self.step_shift.energy_add) + nudge, 1, 4);
        let kept_fills = self.fills;
        let kept_length = self.length;
        let kept_dynamics = self.dynamics;
        let kept_velocity = self.velocity;
        if self.step_shift.any {
            self.fills = effective_fills(self.fills, self.step_shift.fills_mul);
            self.velocity = clamp((self.velocity as f32 * self.step_shift.vel_mul).round() as i32, 1, 127);
            self.length = clamp(self.length * self.step_shift.len_mul, 0.1f32, 1.0f32);
            self.dynamics = clamp(self.dynamics * self.step_shift.dyn_mul, 0.0f32, 100.0f32);
        }
        if self.ujam_active(lib) {
            self.ujam_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
        } else if who == 0 {
            self.session_step(step, pass, prog_bar, sib, step_ppq, cx, out);
        } else {
            self.persona_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
        }
        self.energy = base_energy;
        self.fills = kept_fills;
        self.length = kept_length;
        self.dynamics = kept_dynamics;
        self.velocity = kept_velocity;
        self.step_shift = FeelShift::default();
        self.cur = None;
        self.live_mode = false;
    }

    /// `void switchMeterAt (int step)`
    pub(crate) fn switch_meter_at(&mut self, step: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        // The bar line of our clock where the new meter starts: everything sounding (and every held key: Evolution,
        // keyswitch latches, The Fiddle) is let go, and the grid is re-anchored here; bars keep counting.
        // Steps count in tickPpq() units, which the meter changes (Double plays as Normal in 6/8; a native 12-step player
        // always steps a 16th): what was made from `at` on is dropped in the old size, the step clock carried over in the new.
        let at = f64::from(step) * f64::from(self.tick_ppq(cx.lib));
        let old_bar = f64::from(self.get_bar_length_ppq());
        let bars = ((at - self.bar_origin) / old_bar).round();
        self.stop_all_notes(cx, out);
        self.regenerate_from(at, cx, out);
        let mut u = false;
        self.meter = self.pending_meter;
        let _ = self.effective_meter(&mut u, cx);
        self.host_meter_unsupported = u;
        self.bar_origin = at - bars * f64::from(self.get_bar_length_ppq());
        self.last_emitted_step = (at / f64::from(self.tick_ppq(cx.lib))).round() as i32 - 1; // the step at the bar line is made again, in the new meter
        if self.capture.is_some() && !self.rendering && cx.song.is_some() {
            // Kept takes: the recorder switches exactly here (its position: the song's, as captured notes are).
            let c = CaptureEvent {
                ppq: at
                    + (if self.perf.jump_land >= 0.0 && at >= self.perf.jump_land - 1.0e-9 {
                        self.perf.jump_new_offset
                    } else {
                        self.perf.jump_offset
                    }),
                musician: -1,
                d1: self.meter as u8,
                d2: 2,
                ..CaptureEvent::default()
            };
            if let Some(capture) = self.capture.as_mut() {
                capture.push(c);
            }
        }
    }

    /// `void resetScheduler ()`
    pub(crate) fn reset_scheduler(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.stop_all_notes(cx, out);
        self.last_emitted_step = NO_STEP;
        self.silence_at = -1.0;
        self.song_silent = false;
        self.latch_meter(cx);
    }

    // ---- Parameters ----------------------------------------------------------
    /// `void on_param1 (float v)`
    pub(crate) fn on_param1(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let next = clamp(v as i32, 0, K_INSTRUMENTS as i32 - 1);
        if next != self.pattern {
            self.pattern = next;
            self.reset_scheduler(cx, out);
            self.apply_speed(false, cx, out);
        }
    }

    /// `void on_param2 (float v)`
    pub(crate) fn on_param2(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.key_root = clamp(v as i32, 0, 11);
        self.reset_scheduler(cx, out);
    }

    /// `void on_param3 (float v)`
    pub(crate) fn on_param3(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.scale_mode = clamp(v as i32, 0, 1);
        self.reset_scheduler(cx, out);
    }

    /// `void on_param4 (float v)`
    pub(crate) fn on_param4(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.octave_shift = clamp(v as i32, -2, 2);
        self.reset_scheduler(cx, out);
    }

    /// `void on_param5 (float v)`
    pub(crate) fn on_param5(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.speed_knob = clamp(v as i32, 0, 2);
        self.apply_speed(true, cx, out);
    }

    // ---- Song feel ------------------------------------------------------------------------------------------------
    /// `FeelShift feelShiftFor (int who, const SongSection* sec)`
    pub(crate) fn feel_shift_for(&self, who: i32, sec: Option<&SongSection>, cx: &Ctx<'_>) -> FeelShift {
        let Some(song) = cx.song else { return FeelShift::default() };
        if !song.feel.on || ((i32::from(song.feel_ignore) >> clamp(self.index, 0, 5)) & 1) != 0 {
            return FeelShift::default();
        }
        let none = FeelOverride::default();
        shift_for(
            &resolve(&song.feel, sec.map_or(&none, |s| &s.feel_override)),
            self.feel_for(who, cx.lib).role,
            self.is_drums(),
            is12(self.meter),
            self.expressive(),
        )
    }

    /// `int songSpeedStep ()`
    pub(crate) fn song_speed_step(&self, cx: &Ctx<'_>) -> i32 {
        self.feel_shift_for(self.active_player(cx.lib), None, cx).speed_step
    }

    /// `void applySpeed (bool always)`
    pub(crate) fn apply_speed(&mut self, always: bool, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let want = effective_speed(self.speed_knob, self.song_speed_step(cx));
        if want == self.speed && !always {
            return;
        }
        self.speed = want;
        if always || self.last_emitted_step != NO_STEP {
            self.reset_scheduler(cx, out); // nothing played yet: nothing to stop or re-anchor
        }
    }

    /// `void on_param6 (float v)`
    pub(crate) fn on_param6(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.length = clamp(v * 0.01f32, 0.1f32, 1.0f32);
    }

    /// `void on_param7 (float v)`
    pub(crate) fn on_param7(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.velocity = clamp(v as i32, 1, 127);
    }

    /// `void on_param8 (float v)`
    pub(crate) fn on_param8(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.reset_scheduler(cx, out);
        self.midi_channel0 = clamp(v as i32 - 1, 0, 15);
    }

    /// `void on_param9 (float v)`
    pub(crate) fn on_param9(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.enabled = (v + 0.5f32) as i32 == 1;
        self.reset_scheduler(cx, out);
    }

    /// `void on_param36 (float v)`
    pub(crate) fn on_param36(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.muted = v >= 0.5f32;
        self.reset_scheduler(cx, out);
    }

    /// `void on_param38 (float v)`
    pub(crate) fn on_param38(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.groove_amt = clamp(v, 0.0f32, 100.0f32);
    }

    /// `void on_param37 (float v)`
    pub(crate) fn on_param37(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let next = clamp((v + 0.5f32) as i32, 0, 2);
        if next == self.ample_mode {
            return;
        }
        self.ample_mode = next;
        if self.last_emitted_step != NO_STEP {
            self.regenerate_from(self.current_ppq, cx, out); // what was made for the other mode isn't played
        }
    }

    /// `void on_param39 (float v)`
    pub(crate) fn on_param39(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        let next = clamp((v + 0.5f32) as i32, 0, 1);
        if next == self.ample_chords {
            return;
        }
        self.ample_chords = next;
        self.ample_chord_sent = -1; // ampleSig carries it: setup and chord go again
    }

    /// `void on_param40 (float v)`
    pub(crate) fn on_param40(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let next = clamp((v + 0.5f32) as i32, 0, 1);
        if next == self.pluck_chords {
            return;
        }
        self.pluck_chords = next;
        self.in_slot_sig = -1;
        if self.last_emitted_step != NO_STEP {
            self.regenerate_from(self.current_ppq, cx, out); // what was made for the other way isn't played
        }
    }

    /// `void on_param10 (float v)`
    pub(crate) fn on_param10(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let next = clamp(v as i32, 0, 3);
        if next != self.variation {
            self.variation = next;
            self.reset_scheduler(cx, out);
        }
    }

    /// `void on_param11 (float v)`
    pub(crate) fn on_param11(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let next = clamp(v as i32, 0, K_PLAYER_SLOTS as i32);
        if next != self.player {
            self.player = next;
            self.reset_scheduler(cx, out);
            self.apply_speed(false, cx, out);
        }
    }

    /// `void on_param12 (float v)`
    pub(crate) fn on_param12(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.energy = clamp(v as i32, 1, 4);
    }

    /// `void on_param13 (float v)`
    pub(crate) fn on_param13(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.progression = clamp(v as i32, 0, 17);
        if self.progression >= 1 && self.progression <= 16 {
            let k = usize::try_from(self.progression - 1).unwrap_or(0);
            if let Some(preset) = PROGRESSION_PRESETS.get(k) {
                // was presetSlots / presetBars
                for i in 0..8i32 {
                    self.slots.set(i, at_or_default(&preset.slots, i));
                }
                self.bars_mode = if preset.bars == 8 { 1 } else { 0 };
            }
        }
    }

    /// `void on_param14 (float v)`
    pub(crate) fn on_param14(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.bars_mode = clamp(v as i32, 0, 1);
    }

    /// `void on_param15 (float v)`
    pub(crate) fn on_param15(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.slots.set(0, clamp(v as i32, 0, 111));
    }

    /// `void on_param16 (float v)`
    pub(crate) fn on_param16(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.slots.set(1, clamp(v as i32, 0, 111));
    }

    /// `void on_param17 (float v)`
    pub(crate) fn on_param17(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.slots.set(2, clamp(v as i32, 0, 111));
    }

    /// `void on_param18 (float v)`
    pub(crate) fn on_param18(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.slots.set(3, clamp(v as i32, 0, 111));
    }

    /// `void on_param19 (float v)`
    pub(crate) fn on_param19(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.slots.set(4, clamp(v as i32, 0, 111));
    }

    /// `void on_param20 (float v)`
    pub(crate) fn on_param20(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.slots.set(5, clamp(v as i32, 0, 111));
    }

    /// `void on_param21 (float v)`
    pub(crate) fn on_param21(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.slots.set(6, clamp(v as i32, 0, 111));
    }

    /// `void on_param22 (float v)`
    pub(crate) fn on_param22(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.slots.set(7, clamp(v as i32, 0, 111));
    }

    /// `void on_param23 (float v)`
    pub(crate) fn on_param23(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.swing_amt = clamp(v, 0.0f32, 100.0f32);
    }

    /// `void on_param24 (float v)`
    pub(crate) fn on_param24(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.humanize = clamp(v, 0.0f32, 100.0f32);
    }

    /// `void on_param25 (float v)`
    pub(crate) fn on_param25(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.dynamics = clamp(v, 0.0f32, 100.0f32);
    }

    /// `void on_param26 (float v)`
    pub(crate) fn on_param26(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.fills = clamp(v, 0.0f32, 100.0f32);
    }

    /// `void on_param27 (float v)`
    pub(crate) fn on_param27(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.respect = v >= 0.5f32;
    }

    /// `void on_param28 (float v)`
    pub(crate) fn on_param28(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.take = clamp(v as i32, 1, 16);
    }

    /// `void on_param29 (float v)`
    pub(crate) fn on_param29(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let next = clamp(v as i32, 0, K_TARGET_SLOTS as i32 - 1);
        if next != self.target {
            // reset against the old target (its pedal, return key, swell), as on_param8 does
            self.reset_scheduler(cx, out);
            self.og_lower_ch_used = -1;
            self.target = next;
        }
    }

    /// `void on_param31 (float v)`
    pub(crate) fn on_param31(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.swam_inst = clamp(v as i32, 0, 12);
    }

    /// `void on_param34 (float v)`
    pub(crate) fn on_param34(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.chord_bars_mode = clamp(v as i32, 0, 2);
    }

    /// `void on_param35 (float v)`
    pub(crate) fn on_param35(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.art_map = clamp(v as i32, 0, 65535);
    }

    /// `void on_param33 (float v)`
    pub(crate) fn on_param33(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let bars = clamp(v as i32, 0, 1024); // Amorph capped at 32; a whole song can be longer (values <= 32 behave as before)
        // Any non-zero value starts (or, if one is already running, silently restarts) a render:
        // the UI may send 0 then N within one block and the host may only pass the last value.
        if bars > 0 {
            if self.rendering {
                self.stop_all_notes(cx, out);
                self.rendering = false;
            }
            self.start_render(bars, cx, out); // restart a render
        } else if self.rendering {
            self.finish_render(cx, out);
        }
    }

    /// `void on_param32 (float v)`
    pub(crate) fn on_param32(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.swam_voice = clamp(v as i32, 0, 4);
    }

    /// `void on_param30 (float v)`
    pub(crate) fn on_param30(&mut self, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let _ = (cx, out);
        self.ks_octave = clamp(v as i32, -2, 2);
    }

    // ---- Transport (v0.2 packet handling) ------------------------------------
    /// `void on_transportIn (float value)`
    pub(crate) fn on_transport_in(&mut self, value: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if value.is_nan() || value.abs() > 100000000.0f32 {
            self.transport_slot = (self.transport_slot + 1) % 6;
            return;
        }
        self.transport_seen = true;
        if self.rendering && self.transport_slot != 1 {
            self.transport_slot = (self.transport_slot + 1) % 6;
            return;
        }

        if self.transport_slot == 0 {
            let next_playing = value > 0.5f32;
            if next_playing && !self.host_playing {
                self.awaiting_start_ppq = true;
                self.raw_ppq_seen = false;
                self.reset_scheduler(cx, out);
            } else if !next_playing && self.host_playing {
                self.stop_all_notes(cx, out);
                self.last_emitted_step = NO_STEP;
            }
            self.host_playing = next_playing;
            self.ui_playing = self.host_playing; // was transportStateOut
        } else if self.transport_slot == 1 {
            self.host_tempo = clamp(value, 20.0f32, 999.0f32);
            self.ui_tempo = self.host_tempo; // was tempoOut
        } else if self.transport_slot == 2 {
            // Noted only: the denominator (the next slot of the same packet) applies both, so a 4/4 -> 6/8 change never
            // passes through 6/4.
            let next_numerator = max(1, (value + 0.5f32) as i32);
            if next_numerator != self.time_numerator {
                self.time_numerator = next_numerator;
                self.meter_changed = true;
            }
        } else if self.transport_slot == 3 {
            let next_denominator = max(1, (value + 0.5f32) as i32);
            if next_denominator != self.time_denominator {
                self.time_denominator = next_denominator;
                self.meter_changed = true;
            }
            if self.meter_changed {
                // Stopped: from scratch in the new meter. Playing: it's played from the next bar line (mainFrame / switchMeterAt).
                self.meter_changed = false;
                if self.last_emitted_step == NO_STEP || self.rendering {
                    self.reset_scheduler(cx, out);
                } else {
                    self.note_meter(cx);
                }
            }
        } else if self.transport_slot == 4 {
            let incoming_ppq = f64::from(value);
            let backwards_jump = self.raw_ppq_seen && incoming_ppq < self.previous_raw_ppq - 0.000001;
            let forward_seek = self.raw_ppq_seen && incoming_ppq > self.current_ppq + 0.25;

            // Every valid host packet is authoritative. Discontinuity is
            // detected against the previous raw packet, never the locally
            // extrapolated position, so block re-anchors cannot double-fire.
            self.previous_raw_ppq = incoming_ppq;
            self.current_ppq = incoming_ppq;
            self.raw_ppq_seen = true;
            self.awaiting_start_ppq = false;

            if backwards_jump || forward_seek {
                self.reset_scheduler(cx, out);
                self.lick_forget(); // a kick-off's landing never plays without its pickup
            }
        } else if self.transport_slot == 5 {
            // Packet terminator; loops are fixed 4/4 phrases.
        }

        self.transport_slot = (self.transport_slot + 1) % 6;
    }

    /// `void on_midiIn (const uint8_t* data, int size)`
    pub(crate) fn on_midi_in(&mut self, data: &[u8], cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        // Generator owns note output, so incoming clips cannot double the loop.
        // isAllNotesOff / isAllSoundOff: a controller message with CC 123 / CC 120.
        if let [s, d1, _, ..] = *data
            && (s & 0xf0) == 0xB0
            && (d1 == 123 || d1 == 120)
        {
            self.reset_scheduler(cx, out);
        }
    }

    /// `void startRender (int bars)`
    pub(crate) fn start_render(&mut self, bars: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.stop_all_notes(cx, out);
        self.rendering = true;
        self.render_bars = bars;
        self.last_emitted_step = NO_STEP;
        self.current_ppq = 0.0;
        self.latch_meter(cx);
        self.apply_speed(false, cx, out); // the song's Time in this meter (none in 3/4 / 6/8)
        self.render_end = f64::from(bars) * f64::from(self.get_bar_length_ppq());
        self.render_budget = 8;
        self.render_refill = 0;
        out.render_marker(true, bars); // render start marker
    }

    /// `void finishRender ()`
    pub(crate) fn finish_render(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.stop_all_notes(cx, out); // closes render notes in the log
        out.render_marker(false, 0); // render end marker
        self.rendering = false;
        self.render_bars = 0;
        self.reset_scheduler(cx, out); // and a clean live restart
        self.awaiting_start_ppq = true;
        self.raw_ppq_seen = false;
    }

    /// `float64 genAheadMs ()`
    pub(crate) fn gen_ahead_ms(&self, cx: &Ctx<'_>) -> f64 {
        if self.driver_is(Driver::Organ, cx.lib)
            && self.expressive()
            && cx.song.is_some()
            && !self.live_mode
            && self.feel_for(self.active_player(cx.lib), cx.lib).organ.smear > 0.0f32
        {
            150.0
        } else {
            40.0
        }
    }

    /// `void renderSlice ()`
    pub(crate) fn render_slice(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.render_refill += 1;
        if self.render_refill >= 512 {
            self.render_refill = 0;
            self.render_budget = 8;
        }
        let mut iterations = 0;
        let mut tick = f64::from(self.tick_ppq(cx.lib));
        let dt = f64::from(self.host_tempo) / 60.0 / max(1.0, self.sample_rate);

        while self.render_budget >= 6 && iterations < 3000 && self.current_ppq < self.render_end {
            iterations += 1;
            self.note_meter(cx);
            if self.silence_at >= 0.0 && self.current_ppq >= self.silence_at {
                self.silence_at = -1.0;
                self.stop_all_notes(cx, out);
            } // a section it sits out
            self.stop_expired_notes(self.current_ppq, cx, out);
            let play_step = self.floor_to_int(self.current_ppq / tick);
            if self.enabled {
                let eval_step = self.floor_to_int((self.current_ppq + self.gen_ahead_ms(cx) * self.ppq_per_ms()) / tick);
                if self.last_emitted_step == NO_STEP {
                    self.last_emitted_step = play_step - 1;
                }
                while self.last_emitted_step < eval_step && f64::from(self.last_emitted_step + 1) * tick < self.render_end {
                    self.last_emitted_step += 1;
                    if self.meter_switch_due(self.last_emitted_step, cx.lib) {
                        self.switch_meter_at(self.last_emitted_step, cx, out);
                        tick = f64::from(self.tick_ppq(cx.lib));
                        break;
                    } // the step clock: new size
                    self.generate_step(self.last_emitted_step, cx, out);
                }
            }
            self.fire_due_events(self.current_ppq, cx, out);
            self.current_ppq += dt;
        }

        if self.current_ppq >= self.render_end {
            // Tail: notes still sounding end one by one across the next two steps (so the throttle
            // spreads their note-offs over several blocks), then the render closes with a fresh budget.
            let tail_end = self.render_end + 2.0 * tick;
            for i in 0..64 {
                if self.active_slots.val(i) && self.active_off_ppq.val(i) > tail_end - 0.5 * tick {
                    self.active_off_ppq.set(i, self.render_end + 0.25 * tick + f64::from(i) * 0.02 * tick);
                }
            }
            self.clear_queue();
            while self.render_budget > 0 && self.current_ppq < tail_end {
                self.stop_expired_notes(self.current_ppq, cx, out);
                self.current_ppq += dt;
            }
            if self.current_ppq >= tail_end && self.render_budget >= 6 {
                self.finish_render(cx, out);
            }
        }
    }

    /// `void mainFrame ()`
    pub(crate) fn main_frame(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if self.rendering {
            self.render_slice(cx, out);
        } else if self.host_playing && self.transport_seen && self.raw_ppq_seen && !self.awaiting_start_ppq {
            if self.last_emitted_step == NO_STEP {
                self.latch_meter(cx);
                self.apply_speed(false, cx, out);
            }
            // start / relocate (the song's Time in this meter)
            else {
                self.note_meter(cx); // a change: at the next bar line
            }
            if self.silence_at >= 0.0 && self.current_ppq >= self.silence_at {
                self.silence_at = -1.0;
                self.stop_all_notes(cx, out);
            } // a section it sits out
            self.stop_expired_notes(self.current_ppq, cx, out);
            let tick = f64::from(self.tick_ppq(cx.lib));
            let play_step = self.floor_to_int(self.current_ppq / tick);

            if play_step != self.last_display_step {
                self.last_display_step = play_step;
                self.ui_pulse = self.step_in_bar(play_step, cx.lib); // was pulseOut
                if let Some(song) = cx.song {
                    let off = if self.perf.jump_land >= 0.0 && self.current_ppq >= self.perf.jump_land {
                        self.perf.jump_new_offset
                    } else {
                        self.perf.jump_offset
                    };
                    let loc = self.song_locate(self.current_ppq + off, cx); // after a jump: where the band is, not the host
                    let e = entry_at(song, loc.idx);
                    self.ui_bar = e.chord_index;
                    self.ui_section = e.section;
                    self.ui_ordinal = loc.idx;
                } else {
                    self.ui_bar = self.positive_modulo(self.floor_to_int(self.current_ppq / self.chord_len_ppq()), self.num_bars()); // was barOut
                }
            }

            if self.enabled && !self.muted {
                let eval_step = self.floor_to_int((self.current_ppq + self.gen_ahead_ms(cx) * self.ppq_per_ms()) / tick);
                if self.last_emitted_step == NO_STEP || eval_step - self.last_emitted_step > 8 {
                    self.last_emitted_step = play_step - 1;
                }
                while self.last_emitted_step < eval_step {
                    self.last_emitted_step += 1;
                    if self.meter_switch_due(self.last_emitted_step, cx.lib) {
                        self.switch_meter_at(self.last_emitted_step, cx, out);
                        break;
                    } // the next frame counts in the new size
                    self.generate_step(self.last_emitted_step, cx, out);
                }
            }

            self.fire_due_events(self.current_ppq, cx, out);
            if self.swam_active(cx.lib)
                && self.expr_level > 0
                && self.last_note_end >= 0.0
                && self.current_ppq > self.last_note_end + 150.0 * self.ppq_per_ms()
                && self.q.next_due_ppq > self.current_ppq + 60.0 * self.ppq_per_ms()
            {
                let mut any_sounding = false;
                for i in 0..64 {
                    if self.active_slots.val(i) {
                        any_sounding = true;
                    }
                }
                if !any_sounding {
                    if self.swam_expr_cc_sent >= 0 {
                        self.send_midi(0xB0 + self.midi_channel0, self.swam_expr_cc_sent, 0, cx, out);
                    }
                    self.expr_level = 0;
                    self.sounding_since = -1.0;
                    if self.vib_level > 0 {
                        if self.swam_vib_cc_sent >= 0 {
                            self.send_midi(0xB0 + self.midi_channel0, self.swam_vib_cc_sent, 0, cx, out);
                        }
                        self.vib_level = 0;
                    }
                }
            }
            self.current_ppq += f64::from(self.host_tempo) / 60.0 / max(1.0, self.sample_rate);
        }
    }
}

//! The phrase layer's step (not in MIDI Studio): where the step sits in its section's phrases, and the device a
//! musician plays there (`band_phrase`), applied to the notes the step's pattern made. Only with "As a band".

use super::*;
use crate::band_phrase::{self as bp, Device, PhraseAt, PhraseStep};

fn section_at(song: &SongTimeline, i: i32) -> Option<&SongSection> {
    usize::try_from(i).ok().and_then(|i| song.sections.get(i))
}

impl Musician {
    /// The part of the band this musician is in, for the spotlight.
    fn phrase_group(&self, who: i32, lib: &LibraryData) -> i32 {
        if self.is_drums() {
            return bp::GROUP_DRUMS;
        }
        match self.feel_for(who, lib).role {
            2 => bp::GROUP_DRUMS,
            1 => bp::GROUP_BASS,
            0 | 3 => bp::GROUP_CHORDS,
            _ if self.organ_active(lib) => bp::GROUP_CHORDS,
            _ => bp::GROUP_SECTION,
        }
    }

    /// The phrase step at `step` (the song path, with the timeline entry `e` of loop `loop_` current).
    pub(crate) fn phrase_step_here(&self, who: i32, step: i32, step_ppq: f64, e: TimelineEntry, loop_: i32, cx: &Ctx<'_>) -> PhraseStep {
        let Some(song) = cx.song else { return PhraseStep::default() };
        if section_at(song, e.section).is_none() {
            return PhraseStep::default();
        }
        let mut bars = 1;
        let bar = self.section_bar_of(step_ppq, &mut bars, cx);
        // Passes of this section before this one: earlier in the arrangement, and in every earlier loop.
        let mut before = 0;
        let mut per_loop = 0;
        for x in &song.entries {
            if x.chord_index == 0 && x.section == e.section {
                per_loop += 1;
                if x.arr_pos < e.arr_pos {
                    before += 1;
                }
            }
        }
        let occurrence = before + max(0, loop_) * per_loop;
        let tick = f64::from(self.tick_ppq(cx.lib));
        let bar_l = f64::from(self.get_bar_length_ppq());
        let in_bar = if tick > 1.0e-9 && bar_l > 1.0e-9 { self.floor_to_int((step_ppq - self.bar_origin).rem_euclid(bar_l) / tick + 0.5) } else { 0 };
        let model = bp::model_bar(bar, bars);
        let base = (e.section + 1).wrapping_mul(8192).wrapping_add(model.wrapping_mul(64));
        PhraseStep {
            on: true,
            at: PhraseAt {
                take: self.effective_take(cx),
                musician: self.index,
                group: self.phrase_group(who, cx.lib),
                section: e.section,
                occurrence,
                bar,
                bars,
                density: -1,
            },
            hit_step_delta: base.wrapping_sub(step.wrapping_sub(in_bar)),
            hit_pass: 977 + e.section,
        }
    }

    /// The keys a pattern's chance hits are drawn with: the step and pass, or with "As a band" the model bar's (so a
    /// restatement and every pass of a section play the part's same hits).
    pub(crate) fn hit_keys(&self, step: i32, pass: i32) -> (i32, i32) {
        if self.ph.on { (step.wrapping_add(self.ph.hit_step_delta), self.ph.hit_pass) } else { (step, pass) }
    }

    /// Keeps the step's notes for which `keep` is true.
    fn phrase_keep(&mut self, keep: impl Fn(&Self, i32) -> bool) {
        let mut w = 0;
        for k in 0..self.step_count {
            if !keep(self, k) {
                continue;
            }
            if w != k {
                self.step_note.set(w, self.step_note.val(k));
                self.step_vel.set(w, self.step_vel.val(k));
                self.step_len.set(w, self.step_len.val(k));
                self.step_hand.set(w, self.step_hand.val(k));
                self.step_shift_ms.set(w, self.step_shift_ms.val(k));
                self.step_flags.set(w, self.step_flags.val(k));
            }
            w += 1;
        }
        self.step_count = w;
    }

    fn step_has(&self, f: impl Fn(i32) -> bool) -> bool {
        (0..self.step_count).any(|k| f(self.step_note.val(k)))
    }

    /// `pc` (absolute pitch class) in the octave nearest `near`.
    fn nearest_pc(&self, pc: i32, near: i32) -> i32 {
        near + (self.positive_modulo(pc - near + 6, 12) - 6)
    }

    /// A pushed chord (the next bar's, struck early): kept, so the next bar can strike it again.
    pub(crate) fn phrase_note_push(&mut self) {
        self.ph_push_n = 0;
        let rh = (0..self.step_count).any(|k| self.step_hand.val(k) == 1);
        for k in 0..self.step_count {
            if (rh && self.step_hand.val(k) != 1) || self.ph_push_n >= 8 {
                continue;
            }
            self.ph_push.set(self.ph_push_n, self.step_note.val(k));
            self.ph_push_hand.set(self.ph_push_n, self.step_hand.val(k));
            self.ph_push_vel = if self.ph_push_n == 0 { self.step_vel.val(k) } else { max(self.ph_push_vel, self.step_vel.val(k)) };
            self.ph_push_n += 1;
        }
    }

    /// The phrase layer on this step's notes: the device this musician plays in this bar of the phrase.
    pub(crate) fn phrase_vary(&mut self, prog_bar: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let tick = f64::from(self.tick_ppq(lib));
        let bar_l = f64::from(self.get_bar_length_ppq());
        if tick <= 1.0e-9 || bar_l <= 1.0e-9 {
            return;
        }
        let n = max(4, self.floor_to_int(bar_l / tick + 0.5)); // steps in the bar as heard
        let beat = max(1, n / max(1, info(self.meter).beats_per_bar));
        let pos = self.floor_to_int((step_ppq - self.bar_origin).rem_euclid(bar_l) / tick + 0.5);
        if pos < 0 || pos >= n {
            return;
        }
        if self.perf.natural && self.pushed_at >= 0.0 && step_ppq > self.pushed_from + 1.0e-6 && step_ppq < self.pushed_at + 1.0e-6 {
            return; // a pushed chord rings here (this step's notes are dropped after us)
        }
        let bar_no = self.floor_to_int((step_ppq - self.bar_origin) / bar_l + 1.0e-6);
        if bar_no != self.ph_bar {
            self.ph_bar = bar_no;
            self.ph_struck = false;
            self.ph_prev_strikes = self.ph_strikes;
            self.ph_strikes = 0;
            // A chord pushed into this bar is the bar's chord, until the downbeat strikes one.
            self.ph_chord_n = self.ph_push_n;
            for i in 0..self.ph_push_n {
                self.ph_chord.set(i, self.ph_push.val(i));
                self.ph_chord_hand.set(i, self.ph_push_hand.val(i));
            }
            self.ph_chord_vel = self.ph_push_vel;
            self.ph_pushed_in = self.ph_push_n > 0;
            self.ph_push_n = 0;
        }
        if self.step_count > 0 {
            self.ph_strikes = self.ph_strikes.saturating_add(1);
        }
        let at = self.ph.at;
        // What this player has been playing: the bass's downbeat note, the top voice, how hard.
        if self.step_count > 0 {
            let mut lo = i32::MAX;
            let mut hi = i32::MIN;
            let mut loud = 0.0f32;
            for k in 0..self.step_count {
                lo = min(lo, self.step_note.val(k));
                hi = max(hi, self.step_note.val(k));
                loud = max(loud, self.step_vel.val(k));
            }
            if at.group == bp::GROUP_BASS {
                if pos == 0 {
                    self.ph_low = lo;
                    self.ph_vel = loud;
                }
            } else if at.group != bp::GROUP_DRUMS {
                self.ph_top = hi;
                self.ph_low = lo;
                self.ph_vel = loud;
            }
        }
        let at = PhraseAt { density: self.ph_prev_strikes, ..at };
        let (dev, var) = bp::device(&at);
        let sparse = at.sparse();
        // Strike again on beat 3 (a sparse part) or the "and" of 3, or on the "and" of 2.
        let restrike_at = if var == 1 {
            beat + beat / 2
        } else if sparse {
            n / 2
        } else {
            n / 2 + beat / 2
        };
        if dev == Device::None || pos == 0 {
            if pos == 0 && matches!(dev, Device::Hold | Device::Restrike) {
                if !self.phrase_whole_chord(step_ppq, bar_l) || self.ample_active(lib) {
                    // a chord change inside the bar: nothing to hold or strike again
                } else if dev == Device::Hold {
                    self.phrase_hold(step_ppq, bar_l, beat, tick);
                } else {
                    self.phrase_keep_chord(restrike_at, tick);
                }
            }
            return; // the downbeat is played as written
        }
        let in_drum_fill = step_ppq >= cx.shared.drum_fill_from - 1.0e-9 && step_ppq < cx.shared.drum_fill_to;
        if in_drum_fill && (dev.busy() || at.group == bp::GROUP_DRUMS) {
            return; // the drummer's fill has the bar
        }
        let t = tick as f32;
        let and4 = n - beat / 2;
        let bar_end = self.bar_floor(step_ppq, bar_l) + bar_l;
        // The chord the next bar starts on (the current one when it rings on into it).
        let next = if self.cur_end_ppq > bar_end + 1.0e-6 { self.chord_for_bar(prog_bar, cx) } else { self.chord_for_bar(prog_bar + 1, cx) };
        let next_root = self.key_root + next.root_pc;
        let snare = |x: i32| (37..=40).contains(&x) || x == 62 || x == 64;
        match dev {
            Device::KickAnd4 if pos == and4 && !self.step_has(|x| x == 35 || x == 36) => self.add_step_note(36, 80.0, t, 0, 0.0),
            Device::KickAnd3 if pos == n / 2 + beat / 2 && !self.step_has(|x| x == 35 || x == 36) => self.add_step_note(36, 82.0, t, 0, 0.0),
            Device::Ghosts => {
                let (a, b) = if var == 0 { (2 * beat - 1, n - 1) } else { (beat - 1, n / 2 + beat - 1) };
                if (pos == a || pos == b) && !self.step_has(snare) {
                    self.add_step_note(38, if pos == a { 30.0 } else { 36.0 }, t, 1, 0.0);
                }
            }
            Device::HatOpen if pos == and4 => {
                let closed = |x: i32| matches!(x, 42 | 22 | 25 | 26 | 27 | 29 | 30);
                if self.step_has(closed) {
                    for k in 0..self.step_count {
                        if closed(self.step_note.val(k)) {
                            self.step_note.set(k, 46);
                            self.step_len.set(k, t * 2.0);
                        }
                    }
                } else {
                    self.add_step_note(46, 54.0, t * 2.0, 2, 0.0);
                }
            }
            Device::SnarePickup if pos >= n - 2 && !self.step_has(snare) => self.add_step_note(38, if pos == n - 2 { 56.0 } else { 70.0 }, t, 1, 0.0),
            Device::DropKick if pos > n / 2 && pos % beat != 0 => self.phrase_keep(|m, k| !matches!(m.step_note.val(k), 35 | 36)),
            Device::Approach | Device::Walk if pos == and4 && self.ph_low >= 0 => self.phrase_approach(next_root, var, t),
            Device::Walk if pos == n - beat && self.ph_low >= 0 && self.step_count == 0 => {
                let root = self.nearest_pc(self.key_root + self.chord_for_bar(prog_bar, cx).root_pc, self.ph_low);
                let fifth = if root + 7 <= 60 { root + 7 } else { root - 5 };
                self.add_step_note(fifth, self.ph_vel * 0.8, t * 1.6, 0, 0.0);
            }
            Device::Octave if pos == n / 2 && self.ph_low >= 0 => {
                let root = self.nearest_pc(self.key_root + self.chord_for_bar(prog_bar, cx).root_pc, self.ph_low);
                if root + 12 <= 67 {
                    self.clear_step();
                    self.add_step_note(root + 12, self.ph_vel * 0.85, t * 1.8, 0, 0.0);
                }
            }
            Device::Thin if pos % beat != 0 => self.step_count = 0,
            // (a chord pushed into the bar rings as it was struck: nothing to hold)
            Device::Hold if self.phrase_whole_chord(step_ppq, bar_l) && !self.ample_active(lib) && !self.ph_pushed_in => {
                self.phrase_hold(step_ppq, bar_l, beat, tick)
            }
            Device::Restrike if pos == restrike_at && self.step_count == 0 && self.ph_chord_n > 0 && self.phrase_whole_chord(step_ppq, bar_l) => {
                let len = max(1.0, f64::from(and4 - restrike_at) - 0.5) as f32 * t;
                for i in 0..self.ph_chord_n {
                    self.add_step_note(self.ph_chord.val(i), self.ph_chord_vel * 0.72, len, self.ph_chord_hand.val(i), 0.0);
                }
            }
            Device::Inversion => self.phrase_invert(var),
            Device::Lead if pos == (if var == 0 { and4 } else { n - beat }) && self.ph_top >= 0 && !self.ample_active(lib) => {
                // The next chord's note nearest the top voice, reached from a scale step above or below.
                let mut target = self.nearest_pc(next_root, self.ph_top);
                let mut best = (target - self.ph_top).abs();
                for i in 1..next.count {
                    let c = self.nearest_pc(next_root + next.tones.val(i), self.ph_top);
                    if (c - self.ph_top).abs() < best {
                        best = (c - self.ph_top).abs();
                        target = c;
                    }
                }
                let lead = if var == 0 { self.step_down(target, 0) } else { self.step_up(target, 0) };
                let (lo, hi) = crate::band_feel::playable_range(self.pattern);
                if (lo..=hi).contains(&lead) {
                    self.add_step_note(lead, self.ph_vel * 0.8, t * 1.5, 1, 0.0);
                }
            }
            _ => {}
        }
    }

    /// Bass: an approach note into `next_root` on this step (the note here, or one added).
    fn phrase_approach(&mut self, next_root: i32, var: i32, t: f32) {
        let target = self.nearest_pc(next_root, self.ph_low);
        let mut appr = if var == 0 { target - 1 } else { target + 1 };
        if !(28..=67).contains(&appr) {
            appr = if var == 0 { target + 1 } else { target - 1 };
        }
        if !(23..=67).contains(&appr) {
            return;
        }
        self.clear_step();
        self.add_step_note(appr, self.ph_vel * 0.8, t * 1.5, 0, 0.0);
    }

    /// One chord covers the whole bar the step is in.
    fn phrase_whole_chord(&self, step_ppq: f64, bar_l: f64) -> bool {
        let bar_start = self.bar_floor(step_ppq, bar_l);
        self.cur_start_ppq <= bar_start + 1.0e-6 && self.cur_end_ppq >= bar_start + bar_l - 1.0e-6
    }

    /// The downbeat chord kept for striking again at step `at` of the bar (its notes end just before; a left hand
    /// stays as it is).
    fn phrase_keep_chord(&mut self, at: i32, tick: f64) {
        if self.step_count == 0 {
            return; // a pushed chord rings here: it is the one kept
        }
        self.ph_chord_n = 0;
        let mut loud = 0.0f32;
        let rh = (0..self.step_count).any(|k| self.step_hand.val(k) == 1);
        let end = ((f64::from(at) - 0.6) * tick) as f32;
        for k in 0..self.step_count {
            if rh && self.step_hand.val(k) != 1 {
                continue;
            }
            if self.ph_chord_n < 8 {
                self.ph_chord.set(self.ph_chord_n, self.step_note.val(k));
                self.ph_chord_hand.set(self.ph_chord_n, self.step_hand.val(k));
                self.ph_chord_n += 1;
            }
            loud = max(loud, self.step_vel.val(k));
            self.step_len.set(k, min(self.step_len.val(k), max(end, 0.05)));
        }
        self.ph_chord_vel = loud;
    }

    /// The bar's first chord held (through to the "and" of 4), the comping after it left out.
    fn phrase_hold(&mut self, step_ppq: f64, bar_l: f64, beat: i32, tick: f64) {
        let bar_end = self.bar_floor(step_ppq, bar_l) + bar_l;
        if !self.ph_struck {
            if self.step_count > 0 {
                let hold = ((bar_end - f64::from(beat) * 0.5 * tick - step_ppq) * 0.95) as f32;
                for k in 0..self.step_count {
                    self.step_len.set(k, max(self.step_len.val(k), hold));
                }
                self.ph_struck = true;
            }
        } else {
            self.step_count = 0;
        }
    }

    /// Another inversion: the top voice an octave down (variant 0), or the lowest right-hand note an octave up (1);
    /// the bass note and a left hand stay.
    fn phrase_invert(&mut self, var: i32) {
        let rh = (0..self.step_count).any(|k| self.step_hand.val(k) == 1);
        let cand = |m: &Self, k: i32| !rh || m.step_hand.val(k) == 1;
        let mut count = 0;
        let mut top = -1;
        let mut low = -1;
        let mut lowest = i32::MAX;
        for k in 0..self.step_count {
            lowest = min(lowest, self.step_note.val(k));
            if !cand(self, k) {
                continue;
            }
            count += 1;
            if top < 0 || self.step_note.val(k) > self.step_note.val(top) {
                top = k;
            }
            if low < 0 || self.step_note.val(k) < self.step_note.val(low) {
                low = k;
            }
        }
        if count < 3 || top < 0 || low < 0 {
            return;
        }
        let (k, to) = if var == 1 && rh { (low, self.step_note.val(low) + 12) } else { (top, self.step_note.val(top) - 12) };
        let lowest_other = (0..self.step_count).filter(|&j| j != k).map(|j| self.step_note.val(j)).min().unwrap_or(lowest);
        if to <= lowest_other || self.step_has(|x| x == to) {
            return;
        }
        self.step_note.set(k, to);
    }
}

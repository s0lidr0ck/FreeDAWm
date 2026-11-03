//! `MusicianEvolution.cpp`: Musician: Orange Tree Evolution (the plucked-string engine), ported line by line.
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.

#![allow(clippy::needless_late_init)] // the C++ shape: declared, then set in an if / else

use super::*;
use crate::guitar_shapes as guitar;
use crate::library::{
    EV_BUZZ_TRILL, EV_GRACE_DOWN, EV_GRACE_UP, EV_HALF_PALM_MUTE, EV_HARMONICS, EV_MUTE, EV_PALM_MUTE, EV_RELEASE_SLIDE_DOWN, EV_SLIDE4, EV_SLIDE8,
    EV_SLIDE16, EV_SQUEAL, EV_VELOCITY_SLIDE, EV_WS_HAMMER, ample_strums,
};
use crate::plucked_voice::{self as plucked, Orn, OrnContext, StrumNote};

/// `a[from .. from + n]` (`ptr + from`, `n` long), clipped to the array.
fn sub(a: &[i32], from: i32) -> &[i32] {
    usize::try_from(from).ok().and_then(|k| a.get(k..)).unwrap_or(&[])
}

/// `std::sort (a, a + n)` on ints.
fn sort_n(a: &mut [i32], n: i32) {
    if let Some(s) = a.get_mut(..usize::try_from(n).unwrap_or(0)) {
        s.sort_unstable();
    }
}

/// `evTrem.start + sn[i].offsetMs * ms` for i < c, else `evTrem.end` (evTremContinue's `at` lambda).
fn trem_at(t: &EvTrem, sn: &[StrumNote], c: i32, i: i32, ms: f64) -> f64 {
    if i < c { t.start + at_or_default(sn, i).offset_ms * ms } else { t.end }
}

impl Musician {
    /// `void evReset ()`
    pub(crate) fn ev_reset(&mut self) {
        self.ev_count = 0;
        self.ev_held = HeldKey::default();
        self.ev_held_pitch = -1;
        self.ev_up_key = -1;
        self.ev_up_at = -1.0e9;
        self.ev_prev = EvPrev::default();
        self.ev_hist_count = 0;
        self.ev_vib_start = -1.0e9;
        self.ev_vib_end = -1.0e9;
        self.ev_sec_held = false;
        self.ev_sec_id = -1;
        self.ev_own_from = -1.0e9;
        self.ev_own_until = -1.0e9;
        self.ev_trem = EvTrem::default();
        self.ev_trem_sec = -1;
        self.ev_trem_first_at = -1.0e9;
        self.ev_trem_chord_at = -1.0e9;
        self.ev_harm_sec = -1;
        self.ev_harm_at = -1.0e9;
        self.ev_bend_end = -1.0e9;
        self.ev_phrase_bar = -1.0e9;
    }

    /// `void evAdd (int note, float vel, float64 on, float len, int flags, int hand)`
    pub(crate) fn ev_add(&mut self, note: i32, vel: f32, on: f64, len: f32, flags: i32, hand: i32) {
        if self.ev_count >= self.ev_notes.len() as i32 {
            return;
        }
        put_at(&mut self.ev_notes, self.ev_count, AmpleNote { note, vel, len, on, flags, hand });
        self.ev_count += 1;
    }

    /// `void evPushPrev (int note, float64 on, float64 end)`
    pub(crate) fn ev_push_prev(&mut self, note: i32, on: f64, end: f64) {
        if self.ev_hist_count == self.ev_hist.len() as i32 {
            for i in 1..self.ev_hist_count {
                let v = at_or_default(&self.ev_hist, i);
                put_at(&mut self.ev_hist, i - 1, v);
            }
            self.ev_hist_count -= 1;
        }
        put_at(&mut self.ev_hist, self.ev_hist_count, EvPrev { note, on, end });
        self.ev_hist_count += 1;
        self.ev_prev = EvPrev { note, on, end };
    }

    /// `std::array<int, 6> evOpenStrings ()`
    pub(crate) fn ev_open_strings(&mut self, lib: &LibraryData) -> [i32; 6] {
        if self.current_target(lib).ev_open[0] >= 0 {
            return self.current_target(lib).ev_open; // Indiginus: its own tuning
        }
        let n = self.current_target(lib).ev_string_count;
        if n == 4 {
            return [55, 62, 69, 76, -1, -1];
        }
        if n == 5 {
            return [50, 55, 59, 62, 67, -1];
        }
        [40, 45, 50, 55, 59, 64]
    }

    /// `void evBegin (float64 stepPpq)`
    pub(crate) fn ev_begin(&mut self, step_ppq: f64, lib: &LibraryData) {
        self.ev_count = 0;
        let cc = self.current_target(lib).ev_vibrato_cc;
        let section_start = self.cur.is_some_and(|c| c.chord_index == 0) && step_ppq - f64::from(self.tick_ppq(lib)) + 0.0001 < self.cur_start_ppq;
        if section_start && cc >= 0 && self.ev_vib_end <= step_ppq - 20.0 * self.ppq_per_ms() {
            self.schedule(1, cc, 0, max(self.current_ppq, step_ppq - 20.0 * self.ppq_per_ms()), 0.0f32);
        }
        self.ev_trem_continue(step_ppq, false, lib);
    }

    /// `void evVibStop (float64 at)`
    pub(crate) fn ev_vib_stop(&mut self, at: f64, lib: &LibraryData) {
        if self.ev_vib_end < -1.0e8 || self.ev_vib_end <= at + 1.0e-9 {
            return;
        }
        let cc = self.current_target(lib).ev_vibrato_cc;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 1 && q.d1 == cc && q.on_ppq >= at - 1.0e-9 {
                self.q.queue.at_mut(i).used = false;
            }
        }
        if self.ev_vib_start < at {
            self.schedule(1, cc, 0, at, 0.0f32);
        }
        self.ev_vib_start = -1.0e9;
        self.ev_vib_end = -1.0e9;
    }

    /// evTrim's `moved` lambda: a note of the held key's owner pitch now ends at `end`: the key goes up with it.
    fn ev_trim_moved(&mut self, n: i32, end: f64) {
        if self.ev_held.key >= 0 && n == self.ev_held_pitch && self.ev_held.key_off > end + 5.0 * self.ppq_per_ms() {
            self.ev_held.key_off = max(self.current_ppq, end + 5.0 * self.ppq_per_ms());
            self.ev_held.owner_end = (end * 960.0) as i32;
            self.fd_move_off(self.ev_held.key, self.ev_held.key_off);
        }
    }

    /// `void evTrim (float64 at, int pitch, int range)`
    pub(crate) fn ev_trim(&mut self, at: f64, pitch: i32, range: i32) {
        let to = max(at, self.current_ppq);
        let near = |n: i32| range < 0 || (n - pitch).abs() <= range;
        for i in 0..64 {
            if self.active_slots.val(i)
                && self.active_channels.val(i) == self.midi_channel0
                && (self.active_flags.val(i) & 8192) == 0
                && self.active_off_ppq.val(i) > to
                && near(self.active_notes.val(i))
            {
                self.active_off_ppq.set(i, to);
                self.ev_trim_moved(self.active_notes.val(i), to);
            }
        }
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if !q.used || q.kind != 0 || q.ch != 0 || (q.flags & 8192) != 0 || q.on_ppq >= at || !near(q.d1) {
                continue;
            }
            if q.on_ppq + f64::from(q.dur_ppq) > to {
                let dur = max(0.01, to - q.on_ppq) as f32;
                self.q.queue.at_mut(i).dur_ppq = dur;
                self.ev_trim_moved(q.d1, q.on_ppq + f64::from(dur));
            }
        }
    }

    /// `bool evHold (int key, float64 ownerOn, float64 ownerOff, int ownerPitch)`
    pub(crate) fn ev_hold(&mut self, key: i32, owner_on: f64, owner_off: f64, owner_pitch: i32, lib: &LibraryData) -> bool {
        if key < 0 {
            return false;
        }
        let ms = self.ppq_per_ms();
        let key_off = owner_off + 5.0 * ms;
        let lead = if self.in_drv(lib) { 10.0 } else { 15.0 }; // Indiginus: the key 10 ms before its note (spec 4)
        if self.ev_held.key == key && self.ev_held.key_off >= owner_on - lead * ms - 1.0e-9 && self.ev_held.key_off > self.current_ppq {
            self.ev_held.key_off = max(self.ev_held.key_off, key_off);
            self.ev_held.owner_on = owner_on;
            self.ev_held.owner_end = (owner_off * 960.0) as i32;
            self.ev_held_pitch = owner_pitch;
            self.fd_move_off(key, self.ev_held.key_off);
            return true;
        }
        let mut key_on = max(self.current_ppq, owner_on - lead * ms);
        if key == self.ev_up_key {
            key_on = max(key_on, self.ev_up_at + 1.0 * ms);
        }
        if self.ev_held.key >= 0 && self.ev_held.key_off > self.current_ppq {
            key_on = max(key_on, self.current_ppq + 0.5 * ms);
        }
        if key_on > owner_on - 10.0 * ms {
            return false;
        }
        if self.ev_held.key >= 0 && self.ev_held.key_off > key_on {
            self.fd_move_off(self.ev_held.key, key_on);
        }
        let flags = 8192 | self.in_key_flags(lib);
        {
            let now = self.current_ppq;
            let mut s = SchedAt { q: &mut self.q, now };
            let mut c = kit::DriverCtx { sink: &mut s, now, ms };
            kit::hold_key(&mut c, key, 100, key_on, (key_off - key_on) as f32, flags);
        }
        self.ev_held.key = key;
        self.ev_held.key_on = key_on;
        self.ev_held.key_off = key_off;
        self.ev_held.owner_on = owner_on;
        self.ev_held.owner_end = (owner_off * 960.0) as i32;
        self.ev_held_pitch = owner_pitch;
        self.ev_sec_held = false;
        true
    }

    /// `void evKeyUpBefore (float64 on, bool trimOwner)`
    pub(crate) fn ev_key_up_before(&mut self, on: f64, trim_owner: bool) {
        if self.ev_held.key < 0 || self.ev_held.key_off <= on - 1.0 * self.ppq_per_ms() {
            return;
        }
        let ms = self.ppq_per_ms();
        if trim_owner && self.ev_held_pitch >= 0 {
            self.ev_trim(on - 3.0 * ms, self.ev_held_pitch, 0);
        }
        let off = max(self.current_ppq, on - 1.0 * ms);
        self.fd_move_off(self.ev_held.key, off);
        self.ev_up_key = self.ev_held.key;
        self.ev_up_at = off;
        self.ev_held = HeldKey::default();
        self.ev_held_pitch = -1;
        self.ev_sec_held = false;
    }

    /// `void evKeyClear (float64 on)`
    pub(crate) fn ev_key_clear(&mut self, on: f64) {
        let ms = self.ppq_per_ms();
        if self.ev_held.key < 0 || self.ev_held.key_off <= on - 16.0 * ms + 1.0e-9 {
            return;
        }
        if !self.ev_sec_held {
            self.ev_trim(on - 21.0 * ms, self.ev_held_pitch, -1); // all notes: the owner's release moves with it
        }
        let off = max(self.current_ppq, min(self.ev_held.key_off, on - 16.0 * ms));
        self.fd_move_off(self.ev_held.key, off);
        self.ev_up_key = self.ev_held.key;
        self.ev_up_at = off;
        self.ev_held = HeldKey::default();
        self.ev_held_pitch = -1;
        self.ev_sec_held = false;
    }

    /// `void evolutionDropFrom (float64 cut)`
    pub(crate) fn evolution_drop_from(&mut self, cut: f64, lib: &LibraryData) {
        if !self.plucked_driver(lib) {
            return;
        }
        self.in_drop_from(cut, lib);
        while self.ev_hist_count > 0 && at_or_default(&self.ev_hist, self.ev_hist_count - 1).on >= cut {
            self.ev_hist_count -= 1;
        }
        self.ev_prev = if self.ev_hist_count > 0 { at_or_default(&self.ev_hist, self.ev_hist_count - 1) } else { EvPrev::default() };
        if self.ev_held.key >= 0 && (self.ev_held.key_on >= cut - 1.0e-9 || self.ev_held.owner_on >= cut - 1.0e-9) {
            if self.ev_held.key_on < cut - 1.0e-9 && self.ev_held.key_off > cut {
                self.fd_move_off(self.ev_held.key, cut);
                self.ev_up_key = self.ev_held.key;
                self.ev_up_at = max(cut, self.current_ppq);
            }
            self.ev_held = HeldKey::default();
            self.ev_held_pitch = -1;
            self.ev_sec_held = false;
        }
        // Task 3: a roll's notes after the cut went (the steps from it play their own); a tremolo / harmonics / the
        // section's first tremolo chord made after the cut decide again; a bend whose return went is centred now.
        if self.ev_own_until > cut {
            self.ev_own_until = if self.ev_own_from >= cut - 1.0e-9 { -1.0e9 } else { cut };
        }
        if self.ev_trem.on && self.ev_trem.start >= cut - 1.0e-9 {
            self.ev_trem.on = false;
        }
        if self.ev_trem_first_at >= cut - 1.0e-9 {
            self.ev_trem_sec = -1;
            self.ev_trem_first_at = -1.0e9;
        }
        if self.ev_trem_chord_at >= cut - f64::from(self.tick_ppq(lib)) {
            self.ev_trem_chord_at = -1.0e9; // its chord made again (or not): its steps play
        }
        if self.ev_harm_at >= cut - 1.0e-9 {
            self.ev_harm_sec = -1;
            self.ev_harm_at = -1.0e9;
        }
        if self.ev_phrase_bar >= cut - f64::from(self.get_bar_length_ppq()) {
            self.ev_phrase_bar = -1.0e9;
        }
        if self.ev_bend_end >= cut - 1.0e-9 {
            self.schedule(4, 0, 64, cut, 0.0f32);
            self.ev_bend_end = -1.0e9;
        }
        if self.ev_vib_end >= cut {
            if self.ev_vib_start < cut {
                self.schedule(1, self.current_target(lib).ev_vibrato_cc, 0, cut, 0.0f32);
            }
            self.ev_vib_start = -1.0e9;
            self.ev_vib_end = -1.0e9;
        }
    }

    /// `void evolutionOff ()`
    pub(crate) fn evolution_off(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        if self.plucked_driver(lib) {
            for i in 0..64 {
                let q = *self.q.queue.at(i);
                if q.used && q.kind == 0 && q.ch == 0 && (q.flags & 8192) != 0 {
                    self.q.queue.at_mut(i).used = false;
                }
            }
            for i in 0..64 {
                if self.active_slots.val(i) && self.active_channels.val(i) == self.midi_channel0 && (self.active_flags.val(i) & 8192) != 0 {
                    self.send_slot_note_off(i, cx, out);
                }
            }
            for i in 0..64 {
                let q = *self.q.queue.at(i);
                if q.used && q.kind == 4 {
                    self.q.queue.at_mut(i).used = false; // a blues bend
                }
            }
            if self.wheel_bent {
                self.send_midi(0xE0 + self.midi_channel0, 0, 64, cx, out);
                self.wheel_bent = false;
            }
            if self.ev_vib_end > -1.0e8 && self.current_target(lib).ev_vibrato_cc >= 0 {
                let cc = self.current_target(lib).ev_vibrato_cc;
                for i in 0..64 {
                    let q = *self.q.queue.at(i);
                    if q.used && q.kind == 1 && q.d1 == cc {
                        self.q.queue.at_mut(i).used = false;
                    }
                }
                self.send_midi(0xB0 + self.midi_channel0, cc, 0, cx, out);
            }
        }
        self.ev_reset();
        self.in_reset();
    }

    /// `void evDrone (int vel, float64 at, float len)`
    pub(crate) fn ev_drone(&mut self, vel: i32, at: f64, len: f32, lib: &LibraryData) {
        let t = self.current_target(lib);
        if t.ev_drone_key >= 0 {
            self.schedule(0, t.ev_drone_key, clamp(vel, t.ev_vel_lo, t.ev_vel_hi), at, len);
        } else if t.ev_drone_note >= 0 {
            self.schedule(0, t.ev_drone_note, clamp(vel, t.ev_vel_lo, t.ev_vel_hi), at, len);
        }
    }

    /// `void evolutionPerform (int who, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn evolution_perform(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        if self.ev_count == 0 {
            return;
        }
        let lib = cx.lib;
        let t = self.current_target(lib);
        for i in 0..self.ev_count {
            let mut x = at_or_default(&self.ev_notes, i);
            while x.note < t.ev_low {
                x.note += 12;
            }
            while x.note > t.ev_high {
                x.note -= 12;
            }
            put_at(&mut self.ev_notes, i, x);
        }
        if self.in_drv(lib) && t.ev_string_count == 6 {
            self.in_resonator_line();
        }
        let pl = &self.feel_for(who, lib).plucked;
        let ms = self.ppq_per_ms();
        let strums = self.ev_count >= 3 && (pl.mode == 1 || (pl.mode == 0 && ample_strums(self.feel_for(who, lib), 0, self.energy)));
        let lick = self.lick_now.on; // a lick: plain notes (legato / grace rules), nothing below
        let mut first = 1.0e18;
        let mut max_len = 0.0f32;
        for i in 0..self.ev_count {
            let x = at_or_default(&self.ev_notes, i);
            first = min(first, x.on);
            max_len = max(max_len, x.len);
        }
        self.ev_trem_stop(first); // a new note ends repeated tremolo strokes
        if first < self.ev_own_until - 1.0e-9 {
            // A roll / a clawhammer beat owns its steps; a lick takes the rest of it back.
            if !lick {
                return;
            }
            for i in 0..64 {
                let q = *self.q.queue.at(i);
                if q.used && q.kind == 0 && q.ch == 0 && (q.flags & 8192) == 0 && q.on_ppq >= first - 3.0 * ms {
                    self.q.queue.at_mut(i).used = false;
                }
            }
            self.ev_own_until = -1.0e9;
        }
        if !lick {
            let t = self.current_target(lib);
            let q = self.positive_modulo(self.floor_to_int((step_ppq - (step_ppq + 1.0e-6).floor()) * 4.0 + 0.5), 4);
            let gstep = self.positive_modulo(prog_bar, 2) * self.spb() + sib;
            let held = self.ev_count >= 2 && max_len >= 0.85f32; // a chord of a beat or more (as written: humanize is +-10 %)
            let chank = self.ev_count >= 2 && max_len <= self.tick_ppq(lib) * 0.6f32;
            let bar_l = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
            let beat = self.floor_to_int(step_ppq - self.bar_floor(step_ppq, bar_l) + 1.0e-6);
            // The chop's backbeat: 4/4 beats 2 and 4, 3/4 beats 2 and 3, 6/8 the second dotted beat.
            let back = if self.meter == Meter::M68 {
                sib == half_step(self.meter)
            } else {
                q == 0 && (if self.meter == Meter::M34 { beat >= 1 } else { beat % 2 == 1 })
            };
            if chank && back && pl.chop > 0.0f32 && self.rand01(pass, gstep, if self.in_drv(lib) { 1994 } else { 1976 }, prog_bar, cx) < pl.chop {
                self.ev_chop(who, pass, prog_bar, sib, step_ppq, first, cx);
                return;
            }
            if pl.clawhammer > 0.0f32 && t.ev_string_count == 5 && q == 0 && self.rand01(pass, gstep, 1980, prog_bar + 512, cx) < pl.clawhammer {
                self.ev_claw(who, pass, prog_bar, sib, step_ppq, first, cx);
                return;
            }
            if held && pl.rolls > 0 {
                self.ev_roll(who, pass, prog_bar, sib, step_ppq, pl.rolls, false, cx);
                return;
            }
            if held && pl.rolls == 0 && pl.chop > 0.0f32 && self.rand01(pass, gstep, 1979, prog_bar, cx) < 0.4f32 {
                self.ev_roll(who, pass, prog_bar, sib, step_ppq, 1, true, cx);
                return;
            } // Monroe's cross-picking
            if strums && max_len >= 0.85f32 && pl.tremolo > 0.0f32 && self.rand01(pass, gstep, 1977, prog_bar, cx) < pl.tremolo {
                let mut v = [0i32; 8];
                let mut n = 0;
                let mut i = 0;
                while i < self.ev_count && n < 8 {
                    put_at(&mut v, n, at_or_default(&self.ev_notes, i).note);
                    n += 1;
                    i += 1;
                }
                sort_n(&mut v, n);
                let lo = max(0, n - clamp(t.ev_string_count, 1, 6));
                self.ev_tremolo_chord(who, pass, prog_bar, sib, step_ppq, sub(&v, lo), n - lo, first + 10.0 * ms, first + f64::from(max_len), lib);
                return;
            }
        }
        if strums {
            self.ev_strum(who, pass, prog_bar, sib, step_ppq, cx);
        } else {
            self.ev_play_notes(who, pass, prog_bar, sib, step_ppq, cx);
        }
    }

    /// `float64 evSectionEnd ()`
    pub(crate) fn ev_section_end(&mut self, cx: &mut Ctx<'_>) -> f64 {
        let Some(c) = self.cur else { return 1.0e18 };
        let mut e = self.cur_end_ppq;
        if cx.song.is_some() {
            for d in 1..(c.section_chords - c.chord_index) {
                e += f64::from(self.entry_rel(d, cx).len_half) * f64::from(self.get_bar_length_ppq()) * 0.5;
            }
        }
        e
    }

    /// `int evChordVoicing (int progBar, int from, int to, int* out)`
    pub(crate) fn ev_chord_voicing(&mut self, prog_bar: i32, from: i32, to: i32, out: &mut [i32], cx: &mut Ctx<'_>) -> i32 {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let ch = self.chord_for_bar(prog_bar, cx);
        let root_pc = self.positive_modulo(self.key_root + ch.root_pc, 12);
        let mut pcs: u32 = 0;
        for i in 0..ch.count {
            pcs |= 1u32 << self.positive_modulo(root_pc + ch.tones.val(i), 12);
        }
        let open = self.ev_open_strings(lib);
        let mut n = 0;
        for s in max(0, from)..=min(5, to) {
            let o = at_or_default(&open, s);
            if o < 0 {
                continue;
            }
            for f in 0..=5 {
                if ((pcs >> self.positive_modulo(o + f, 12) as u32) & 1u32) != 0 && o + f >= t.ev_low && o + f <= t.ev_high {
                    put_at(out, n, o + f);
                    n += 1;
                    break;
                }
            }
        }
        n
    }

    /// `int evChord (int who, const int* voicing, int n, float64 on, float64 end, int key, int strumMs, int vel, int velLo)`
    pub(crate) fn ev_chord(
        &mut self,
        who: i32,
        voicing: &[i32],
        n: i32,
        on: f64,
        end: f64,
        key: i32,
        strum_ms: i32,
        vel: i32,
        vel_lo: i32,
        lib: &LibraryData,
    ) -> i32 {
        let t = self.current_target(lib);
        let pl = &self.feel_for(who, lib).plucked;
        let ms = self.ppq_per_ms();
        if n <= 0 {
            return -1;
        }
        let mut v = [0i32; 8];
        let mut m = 0;
        let mut i = 0;
        while i < n && m < 8 {
            put_at(&mut v, m, at_or_default(voicing, i));
            m += 1;
            i += 1;
        }
        sort_n(&mut v, m);
        let on = max(on, self.current_ppq + (if key >= 0 { 16.0 } else { 4.0 }) * ms);
        let end = max(end, on + 20.0 * ms);
        let mut sn = [StrumNote::default(); 8];
        let c = plucked::strum_plan(&v, m, true, strum_ms, pl.up_strings, pl.up_weight, clamp(vel, 1, 127), self.energy, 0.5, false, &mut sn);
        if c == 0 {
            return -1;
        }
        if key >= 0 {
            self.ev_key_clear(on);
        } else {
            self.ev_key_up_before(on, true);
        }
        self.ev_vib_stop(on - 3.0 * ms, lib);
        self.ev_trim(on - 3.0 * ms, -1, -1); // before the hold: an earlier note of the top's pitch isn't its owner
        if key >= 0 {
            self.ev_hold(key, on, end, at_or_default(&sn, c - 1).pitch, lib);
        }
        {
            let now = self.current_ppq;
            let mut s = SchedAt { q: &mut self.q, now };
            let mut cc = kit::DriverCtx { sink: &mut s, now, ms };
            kit::strum_notes(&mut cc, &sn, c, on, end, if vel_lo >= 0 { vel_lo } else { t.ev_vel_lo }, t.ev_vel_hi, 0.01);
        }
        self.ev_prev = EvPrev::default();
        self.ev_push_prev(-1, on, end);
        at_or_default(&sn, c - 1).pitch
    }

    /// `void evChop (int who, int pass, int progBar, int sib, float64 stepPpq, float64 first)`
    pub(crate) fn ev_chop(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, first: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let gstep = self.positive_modulo(prog_bar, 2) * self.spb() + sib;
        let mut v = [0i32; 6];
        let sc = clamp(t.ev_string_count, 1, 6);
        let top = if sc == 5 { 3 } else { sc - 1 };
        let mut n = self.ev_chord_voicing(prog_bar, max(if sc == 4 { 1 } else { 0 }, top - 2), top, &mut v, cx);
        if n < 2 {
            n = 0;
            let mut i = 0;
            while i < self.ev_count && n < 6 {
                put_at(&mut v, n, at_or_default(&self.ev_notes, i).note);
                n += 1;
                i += 1;
            }
        }
        let salt = if self.in_drv(lib) { 1994 } else { 1976 };
        let mut vel = 105 + (self.rand01(pass, gstep, salt, prog_bar + 128, cx) * 15.99f32) as i32;
        let spread = 2 + (self.rand01(pass, gstep, salt, prog_bar + 256, cx) * 2.99f32) as i32;
        let ms = self.ppq_per_ms();
        let on = max(max(first, step_ppq - 20.0 * ms), self.current_ppq + 16.0 * ms);
        let mute = at_or_default(&t.ev_art, EV_MUTE as i32);
        // Indiginus (spec 4): the mutes key held over a short chord when the return is confirmed, else the velocity
        // grid's Low zone (Mutes): 30-38, the one place a note goes below 40.
        // Torch (evolution data, no keys at all): the same velocity-layer mute, 30-38.
        if self.in_drv(lib) && mute >= 0 {
            vel = 60 + (self.rand01(pass, gstep, salt, prog_bar + 128, cx) * 15.99f32) as i32;
        } else if mute < 0 {
            vel = 30 + (self.rand01(pass, gstep, salt, prog_bar + 128, cx) * 8.99f32) as i32;
        }
        self.ev_chord(who, &v, n, on, on + 60.0 * ms, mute, spread, vel, if self.in_drv(lib) || mute < 0 { 1 } else { -1 }, lib);
    }

    /// `void evTremoloChord (int who, int pass, int progBar, int sib, float64 stepPpq, const int* v, int n, float64 on, float64 end)`
    pub(crate) fn ev_tremolo_chord(
        &mut self,
        who: i32,
        pass: i32,
        prog_bar: i32,
        sib: i32,
        step_ppq: f64,
        v: &[i32],
        n: i32,
        on: f64,
        end: f64,
        lib: &LibraryData,
    ) {
        let t = self.current_target(lib);
        let pl = &self.feel_for(who, lib).plucked;
        let ms = self.ppq_per_ms();
        let mut end = end;
        if self.cur.is_some() {
            end = min(end, self.cur_end_ppq - 20.0 * ms);
        }
        let vel = clamp((self.velocity as f32 * 0.8f32) as i32 + 4 * self.energy, 50, 110);
        let keys = t.ev_trem_in >= 0 || t.ev_trem_on >= 0 || t.ev_trem_out >= 0;
        if keys {
            let sec = self.ev_section_id(pass);
            let first_in_sec = self.ev_trem_sec != sec || on < self.ev_trem_first_at - 1.0e-9;
            let bar_l = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
            let bar_start = self.bar_floor(step_ppq, bar_l);
            let end_bar = prog_bar + max(0, self.floor_to_int((max(end, on) - bar_start) / bar_l - 1.0e-6));
            let phrase_last = self.is_phrase_end(end_bar)
                && self.cur.is_some()
                && self.cur_end_ppq >= bar_start + f64::from(end_bar - prog_bar + 1) * bar_l - 1.0e-6;
            let mut key = if phrase_last {
                t.ev_trem_out
            } else if first_in_sec {
                t.ev_trem_in
            } else {
                t.ev_trem_on
            };
            if key < 0 {
                key = if t.ev_trem_on >= 0 {
                    t.ev_trem_on
                } else if t.ev_trem_in >= 0 {
                    t.ev_trem_in
                } else {
                    t.ev_trem_out
                };
            }
            if first_in_sec {
                self.ev_trem_sec = sec;
                self.ev_trem_first_at = on;
            }
            self.ev_chord(who, v, n, on, end, key, pl.strum_ms, vel, -1, lib);
            return;
        }
        let top = self.ev_chord(who, v, n, on, end, -1, pl.strum_ms, vel, -1, lib);
        if top < 0 {
            return;
        }
        self.ev_trem = EvTrem {
            on: true,
            pitch: top,
            start: max(on, self.current_ppq + 4.0 * ms),
            end,
            seed: (self.positive_modulo(pass, 4096) * 65536 + self.positive_modulo(prog_bar, 256) * 64 + sib) as u32,
        };
        self.ev_trem_continue(step_ppq, true, lib);
    }

    /// `void evTremStop (float64 from)`
    pub(crate) fn ev_trem_stop(&mut self, from: f64) {
        if !self.ev_trem.on {
            return;
        }
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 0 && q.ch == 0 && (q.flags & 8192) == 0 && q.d1 == self.ev_trem.pitch && q.on_ppq >= from - 3.0 * self.ppq_per_ms()
            {
                self.q.queue.at_mut(i).used = false;
            }
        }
        self.ev_trem.on = false;
    }

    /// `void evTremContinue (float64 stepPpq, bool fromChord)`
    pub(crate) fn ev_trem_continue(&mut self, step_ppq: f64, from_chord: bool, lib: &LibraryData) {
        if !self.ev_trem.on {
            return;
        }
        let t = self.current_target(lib);
        let ms = self.ppq_per_ms();
        let tick = f64::from(self.tick_ppq(lib));
        let bpm = ms * 60000.0;
        let sub = if bpm <= 110.0 { 0.125 } else { 0.25 };
        let mut sn = [StrumNote::default(); 128];
        let c = plucked::tremolo_plan(self.ev_trem.end - self.ev_trem.start, bpm, 85, 70, &mut sn, 128, self.ev_trem.seed);
        if c > 1 && from_chord {
            let off = trem_at(&self.ev_trem, &sn, c, 1, ms) - 3.0 * ms;
            self.fd_move_off(self.ev_trem.pitch, off); // the chord's top note gives way to the first stroke
        }
        while !from_chord && step_ppq >= self.ev_trem.start + 120.0 * sub {
            self.ev_trem.start += 64.0 * sub; // past the plan's 128 strokes: on, in step
        }
        let lo = if from_chord { self.ev_trem.start } else { step_ppq };
        for i in 1..c {
            let base = self.ev_trem.start + f64::from(i) * sub;
            if base < lo - 1.0e-9 || base >= step_ppq + tick - 1.0e-9 {
                continue;
            }
            let on = max(trem_at(&self.ev_trem, &sn, c, i, ms), self.current_ppq);
            let off = trem_at(&self.ev_trem, &sn, c, i + 1, ms) - 3.0 * ms;
            if off <= on + 1.0e-6 {
                continue;
            }
            self.schedule(0, self.ev_trem.pitch, clamp(at_or_default(&sn, i).velocity, t.ev_vel_lo, t.ev_vel_hi), on, (off - on) as f32);
        }
        if step_ppq + tick >= self.ev_trem.end - 1.0e-9 {
            self.ev_trem.on = false;
        }
    }

    /// `bool evTremoloSectionStep (int who, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn ev_tremolo_section_step(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) -> bool {
        let lib = cx.lib;
        let pl = &self.feel_for(who, lib).plucked;
        let Some(cur) = self.cur else { return false };
        if self.lick_now.on || pl.tremolo <= 0.0f32 || self.energy > 2 {
            return false;
        }
        if self.rand01(pass, cur.arr_pos, 1988, who, cx) >= pl.tremolo * 0.5f32 {
            return false; // ruling: half the tremolo chance
        }
        let t = self.current_target(lib);
        let tick = f64::from(self.tick_ppq(lib));
        if (step_ppq - self.cur_start_ppq).abs() >= tick * 0.5 {
            if step_ppq > self.cur_start_ppq && (self.ev_trem_chord_at - self.cur_start_ppq).abs() < 1.0e-6 {
                self.step_count = 0;
                return true; // only when its chord played
            }
            return false;
        }
        let mut grid_len = 0.0f32;
        for i in 0..self.step_count {
            grid_len = max(grid_len, self.step_len.val(i));
        }
        if self.step_count >= 2 && grid_len >= 0.85f32 {
            return false; // ruling: the grid's own held chords play (their tremolo by chance)
        }
        let mut v = [0i32; 6];
        let sc = clamp(t.ev_string_count, 1, 6);
        let n = self.ev_chord_voicing(prog_bar, if sc == 6 { 2 } else { 0 }, if sc == 5 { 3 } else { sc - 1 }, &mut v, cx);
        if n < 2 {
            return false;
        }
        self.ev_count = 0;
        self.ev_trem_stop(step_ppq);
        let on = max(step_ppq + 10.0 * self.ppq_per_ms(), self.in_ready_at);
        let end = self.cur_end_ppq;
        self.ev_tremolo_chord(who, pass, prog_bar, sib, step_ppq, &v, n, on, end, lib);
        self.ev_trem_chord_at = self.cur_start_ppq;
        self.step_count = 0;
        true
    }

    /// `void evRoll (int who, int pass, int progBar, int sib, float64 stepPpq, int rolls, bool cross)`
    pub(crate) fn ev_roll(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, rolls: i32, cross: bool, cx: &mut Ctx<'_>) {
        let _ = step_ppq; // unused in the C++ too
        let lib = cx.lib;
        let t = self.current_target(lib);
        let pl = &self.feel_for(who, lib).plucked;
        let ms = self.ppq_per_ms();
        let gstep = self.positive_modulo(prog_bar, 2) * self.spb() + sib;
        let mut p = [0i32; 8];
        let mut n = 0;
        let mut on = 1.0e18;
        let mut loud = 0.0f32;
        let mut len = 0.0f32;
        for i in 0..self.ev_count {
            let x = at_or_default(&self.ev_notes, i);
            on = min(on, x.on);
            loud = max(loud, x.vel);
            len = max(len, x.len);
            let mut dup = false;
            for j in 0..n {
                if at_or_default(&p, j) == x.note {
                    dup = true;
                }
            }
            if !dup && n < 8 {
                put_at(&mut p, n, x.note);
                n += 1;
            }
        }
        if n == 0 {
            return;
        }
        sort_n(&mut p, n);
        on = max(on, self.current_ppq + 4.0 * ms);
        let sixteenths = clamp((f64::from(len) * 4.0 + 0.5).floor() as i32, 4, 16) / 4 * 4;
        let sixteenth = 0.25;
        let base = clamp(loud as i32, 40, 106);
        let cap = min(118, t.ev_vel_hi);
        let top = at_or_default(&p, n - 1);
        let mut rp = [0i32; 16];
        let mut rv = [0i32; 16];
        let mut ra = [0.0f64; 16];
        let mut re = [0.0f64; 16];
        let mut m = 0;
        for g in 0..sixteenths / 4 {
            let mut sn = [StrumNote::default(); 4];
            let mut thumb = at_or_default(&p, 0);
            let mut strings = 1; // p + 1
            let mut ns = n - 1;
            if cross {
                thumb = at_or_default(&p, max(0, n - 3));
                strings = max(1, n - 2);
                ns = min(2, n - 1);
            } else if t.ev_drone_note >= 0 && g > 0 {
                thumb = t.ev_drone_note;
                strings = 0;
                ns = n;
            }
            if ns <= 0 {
                strings = 0;
                ns = n;
            }
            let c = plucked::roll_plan(rolls, self.positive_modulo(prog_bar, 2) * 4 + sib / 4 + g, sub(&p, strings), ns, thumb, &mut sn);
            let mut accented = false;
            let mut k = 0;
            while k < c && m < 16 {
                let s = at_or_default(&sn, k);
                put_at(&mut rp, m, s.pitch);
                put_at(&mut ra, m, on + f64::from(g * 4 + s.offset_ms as i32) * sixteenth);
                let mut v = base + s.velocity;
                if s.pitch == top && s.velocity == 0 && !accented {
                    v = base + 12;
                    accented = true;
                } // the melody, wherever the roll puts it (once a group)
                put_at(&mut rv, m, clamp(min(v, cap), t.ev_vel_lo, t.ev_vel_hi));
                k += 1;
                m += 1;
            }
        }
        if m == 0 {
            return;
        }
        let end = on + f64::from(sixteenths) * sixteenth;
        for i in 0..m {
            put_at(&mut re, i, end);
            for j in (i + 1)..m {
                if (at_or_default(&rp, j) - at_or_default(&rp, i)).abs() <= 2 {
                    put_at(&mut re, i, at_or_default(&ra, j) - 3.0 * ms);
                    break;
                }
            }
            let iv = if i + 1 < m { (at_or_default(&rp, i + 1) - at_or_default(&rp, i)).abs() } else { 0 };
            if (1..=2).contains(&iv) && self.rand01(pass, gstep, 1978, i * 128 + at_or_default(&rp, i), cx) < pl.hammer {
                put_at(&mut re, i, at_or_default(&ra, i + 1) + 20.0 * ms);
            }
        }
        self.ev_key_up_before(on, true);
        self.ev_vib_stop(on - 3.0 * ms, lib);
        self.ev_trim(on - 3.0 * ms, -1, -1);
        for i in 0..m {
            self.schedule(
                0,
                at_or_default(&rp, i),
                at_or_default(&rv, i),
                at_or_default(&ra, i),
                max(0.01, at_or_default(&re, i) - at_or_default(&ra, i)) as f32,
            );
        }
        self.ev_own_from = on;
        self.ev_own_until = end - 0.1;
        self.ev_prev = EvPrev::default();
        self.ev_push_prev(-1, on, end);
    }

    /// `void evClaw (int who, int pass, int progBar, int sib, float64 stepPpq, float64 first)`
    pub(crate) fn ev_claw(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, first: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let pl = &self.feel_for(who, lib).plucked;
        let ms = self.ppq_per_ms();
        let gstep = self.positive_modulo(prog_bar, 2) * self.spb() + sib;
        let mut melody = -1;
        for i in 0..self.ev_count {
            melody = max(melody, at_or_default(&self.ev_notes, i).note);
        }
        let mut v = [0i32; 6];
        let n = self.ev_chord_voicing(prog_bar, 1, 3, &mut v, cx);
        let on = max(first, self.current_ppq + 4.0 * ms);
        self.ev_key_up_before(on, true);
        self.ev_vib_stop(on - 3.0 * ms, lib);
        self.ev_trim(on - 3.0 * ms, -1, -1);
        let brush_at = on + 0.5;
        let thumb_at = on + 0.75;
        let end = on + 1.0 - 3.0 * ms;
        self.schedule(0, melody, clamp(100, t.ev_vel_lo, t.ev_vel_hi), on, (brush_at - 3.0 * ms - on) as f32);
        if n >= 2 {
            let mut sn = [StrumNote::default(); 8];
            let c = plucked::strum_plan(&v, n, true, 8, pl.up_strings, pl.up_weight, 80, 2, 0.5, false, &mut sn);
            let now = self.current_ppq;
            let mut s = SchedAt { q: &mut self.q, now };
            let mut cc = kit::DriverCtx { sink: &mut s, now, ms };
            kit::strum_notes(&mut cc, &sn, c, brush_at, thumb_at - 3.0 * ms, t.ev_vel_lo, t.ev_vel_hi, -1.0e300); // no floor
        }
        let mut thumb = if t.ev_drone_note >= 0 { t.ev_drone_note } else { self.ev_open_strings(lib)[4] };
        if n >= 3 && self.rand01(pass, gstep, 1980, prog_bar, cx) < pl.drop_thumb {
            sort_n(&mut v, n);
            thumb = at_or_default(&v, 1);
        }
        if thumb >= t.ev_low && thumb <= t.ev_high {
            self.schedule(0, thumb, clamp(70, t.ev_vel_lo, t.ev_vel_hi), thumb_at, (end - thumb_at) as f32);
        }
        self.ev_own_from = on;
        self.ev_own_until = on + 0.9;
        self.ev_prev = EvPrev::default();
        self.ev_push_prev(-1, on, end);
        let _ = step_ppq;
    }

    /// `int evPalmKey (int who, int pass)`
    pub(crate) fn ev_palm_key(&mut self, who: i32, pass: i32, cx: &mut Ctx<'_>) -> i32 {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let pl = &self.feel_for(who, lib).plucked;
        let Some(cur) = self.cur else { return -1 };
        if t.ev_string_count != 6 || pl.palm_mute <= 0.0f32 || self.energy > 2 || self.lick_now.on {
            return -1;
        }
        if self.rand01(pass, cur.arr_pos, 1985, who, cx) >= pl.palm_mute {
            return -1;
        }
        let half = at_or_default(&t.ev_art, EV_HALF_PALM_MUTE as i32);
        if self.energy == 2 && half >= 0 { half } else { at_or_default(&t.ev_art, EV_PALM_MUTE as i32) }
    }

    /// `bool evSectionHold (int key, int sec, float64& on, float64 len)`
    pub(crate) fn ev_section_hold(&mut self, key: i32, sec: i32, on: &mut f64, len: f64, cx: &mut Ctx<'_>) -> bool {
        let ms = self.ppq_per_ms();
        if key < 0 {
            return false;
        }
        if self.ev_sec_held && self.ev_held.key == key && self.ev_sec_id == sec && self.ev_held.key_off > self.current_ppq + 0.5 * ms {
            let off = *on + len + 5.0 * ms;
            if off > self.ev_held.key_off {
                self.ev_held.key_off = off;
                self.fd_move_off(key, off);
            }
            self.ev_held.owner_on = *on;
            return true;
        }
        *on = max(*on, self.current_ppq + 16.0 * ms);
        self.ev_key_clear(*on);
        let until = max(*on + len, self.ev_section_end(cx) - 5.0 * ms);
        if !self.ev_hold(key, *on, until, -1, cx.lib) {
            return false;
        }
        self.ev_sec_held = true;
        self.ev_sec_id = sec;
        true
    }

    /// `void evStrum (int who, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn ev_strum(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let pl = &self.feel_for(who, lib).plucked;
        let ms = self.ppq_per_ms();
        let gstep = self.positive_modulo(prog_bar, 2) * self.spb() + sib;
        let mut on = 1.0e18;
        let mut loud = 0.0f32;
        let mut len = 0.0f32;
        let mut voic = [0i32; 8];
        let mut n = 0;
        for i in 0..self.ev_count {
            let x = at_or_default(&self.ev_notes, i);
            on = min(on, x.on);
            loud = max(loud, x.vel);
            len = max(len, x.len);
            if n < 8 {
                put_at(&mut voic, n, x.note);
                n += 1;
            }
        }
        if t.ev_string_count == 6 {
            // A guitarist's fingering of the bar's chord (GuitarShapes.h, as Ample's notes mode), when the step is that chord.
            let ch = self.chord_for_bar(prog_bar, cx);
            let root_pc = self.positive_modulo(self.key_root + ch.root_pc, 12);
            let mut pcs: u16 = 0;
            for i in 0..ch.count {
                pcs = (u32::from(pcs) | 1u32 << self.positive_modulo(root_pc + ch.tones.val(i), 12)) as u16;
            }
            let mut outside = 0;
            for i in 0..n {
                if (u32::from(pcs) >> self.positive_modulo(at_or_default(&voic, i), 12) & 1) == 0 {
                    outside += 1;
                }
            }
            if outside * 2 <= n {
                let required =
                    (1u32 << root_pc | (if ch.count > 1 { 1u32 << self.positive_modulo(root_pc + ch.tones.val(1), 12) } else { 0u32 })) as u16;
                let bass_pc = if ch.bass_pc >= 0 { self.positive_modulo(self.key_root + ch.bass_pc, 12) } else { root_pc };
                let open = self.ev_open_strings(lib);
                let r = self.rand01(pass, gstep, 1901, prog_bar, cx);
                let prev = self.guitar_prev;
                let shape = guitar::best(pcs, required, bass_pc, 0, &open, if prev.valid { Some(&prev) } else { None }, r);
                if shape.valid {
                    self.guitar_prev = shape;
                    let ns = guitar::notes(&shape, &open, 0);
                    let mut m = 0;
                    for s in 0..6 {
                        let x = at_or_default(&ns, s);
                        if x >= t.ev_low && x <= t.ev_high {
                            put_at(&mut voic, m, x);
                            m += 1;
                        }
                    }
                    if m >= 2 {
                        n = m;
                    }
                }
            }
        }
        // No more tones than strings: the top ones (the melody side of the voicing).
        sort_n(&mut voic, n);
        let strings = clamp(t.ev_string_count, 1, 6);
        let lo = max(0, n - strings);
        // The hand, as the guitars (Task 5): down on the beat; 6/8 counts the 16th in the dotted beat, down on its first and
        // third eighth (0 / 4), up otherwise. 3/4 keeps the quarter's hand.
        let down;
        if self.meter == Meter::M68 {
            let b = step_ppq / 1.5;
            let p = self.positive_modulo(self.floor_to_int((b - (b + 1.0e-6).floor()) * 6.0 + 0.5), 6);
            down = p == 0 || p == 4;
        } else {
            down = self.positive_modulo(self.floor_to_int((step_ppq - (step_ppq + 1.0e-6).floor()) * 4.0 + 0.5), 4) == 0;
        }
        let palm = self.ev_palm_key(who, pass, cx); // a palm-muted section: its key, no bypass
        let mut bypass = t.ev_bypass_legato_key >= 0 && palm < 0;
        on = max(on, self.current_ppq + (if bypass { 16.0 } else { 4.0 }) * ms); // room for the 3 ms release / the bypass key's 15 ms
        let muted = palm >= 0 && {
            let sec = self.ev_section_id(pass);
            self.ev_section_hold(palm, sec, &mut on, f64::from(max(0.02f32, len)), cx)
        };
        let end = on + f64::from(max(0.02f32, len));
        if bypass && !self.ev_hold(t.ev_bypass_legato_key, on, end, at_or_default(&voic, n - 1), lib) {
            bypass = false; // no room for the key: a plain strum
        }
        if !bypass && !muted {
            self.ev_key_up_before(on, true); // a slide key from the notes before: up first
        }
        let mut sn = [StrumNote::default(); 8];
        let hum = f64::from(self.rand01(pass, gstep, 1972, prog_bar, cx));
        let c = plucked::strum_plan(
            sub(&voic, lo),
            n - lo,
            down,
            pl.strum_ms,
            pl.up_strings,
            pl.up_weight,
            clamp(loud as i32, 1, 127),
            self.energy,
            hum,
            bypass,
            &mut sn,
        );
        if c == 0 {
            return;
        }
        self.ev_vib_stop(on - 3.0 * ms, lib);
        if !bypass {
            self.ev_trim(on - 3.0 * ms, -1, -1); // the last chord ends 3 ms before (no hammer across chords)
        }
        {
            let now = self.current_ppq;
            let mut s = SchedAt { q: &mut self.q, now };
            let mut cc = kit::DriverCtx { sink: &mut s, now, ms };
            kit::strum_notes(&mut cc, &sn, c, on, end, t.ev_vel_lo, t.ev_vel_hi, 0.02);
        }
        self.ev_prev = EvPrev::default(); // no hammer out of a chord
        self.ev_push_prev(-1, on, end);
    }

    /// `void evPlayNotes (int who, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn ev_play_notes(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let pl = &self.feel_for(who, lib).plucked;
        let ms = self.ppq_per_ms();
        let tick = self.tick_ppq(lib);
        let gstep = self.positive_modulo(prog_bar, 2) * self.spb() + sib;
        let bypass = t.ev_bypass_legato_key >= 0;
        if let Some(s) = self.ev_notes.get_mut(..usize::try_from(self.ev_count).unwrap_or(0)) {
            // stable: at most 16 notes here, and below its 32-element threshold MSVC's std::sort is an insertion sort (stable)
            s.sort_by(|a, b| a.on.partial_cmp(&b.on).unwrap_or(std::cmp::Ordering::Equal).then(a.note.cmp(&b.note)));
        }
        let single = self.ev_count == 1;
        let q = self.positive_modulo(self.floor_to_int((step_ppq - (step_ppq + 1.0e-6).floor()) * 4.0 + 0.5), 4);
        let off_step = q != 0;
        let leg_range = max(2, t.ev_legato_st);
        let palm = self.ev_palm_key(who, pass, cx); // a palm-muted section: every note under its key, no legato
        let mut played = [0i32; 16];
        let mut played_n = 0;
        for i in 0..self.ev_count {
            let x = at_or_default(&self.ev_notes, i);
            let note = x.note;
            let mut skip = false; // two tones of one step 0-2 ST apart: the lower only
            for j in 0..played_n {
                if (at_or_default(&played, j) - note).abs() <= (if bypass { 0 } else { 2 }) {
                    skip = true;
                }
            }
            if skip {
                continue;
            }
            put_at(&mut played, played_n, note);
            played_n += 1;
            let mut on = max(x.on, self.current_ppq + 4.0 * ms);
            let len = max(0.02f32, x.len);
            let mut vel = clamp(x.vel as i32, t.ev_vel_lo, t.ev_vel_hi);
            let mut join = Join::Gap;
            let muted = palm >= 0 && {
                let sec = self.ev_section_id(pass);
                self.ev_section_hold(palm, sec, &mut on, f64::from(len), cx)
            };
            if single
                && !muted
                && self.ev_prev.note >= 0
                && on > self.ev_prev.on
                && on - self.ev_prev.end <= f64::from(tick) * 0.5
                && self.ev_prev.end > self.current_ppq
            {
                let ind = self.in_drv(lib);
                let hammer_roll = self.rand01(pass, gstep, if ind { 1985 } else { 1973 }, note, cx);
                let slide_roll = self.rand01(pass, gstep, if ind { 1986 } else { 1974 }, note, cx);
                join = plucked::legato_join(
                    self.ev_prev.note,
                    note,
                    off_step,
                    hammer_roll,
                    slide_roll,
                    pl.hammer,
                    pl.slides,
                    t.ev_legato_st,
                    t.ev_slide_range,
                );
            }
            let mut in_own = false; // Indiginus: a hammer / pull-off / slide key of this note's own
            if self.in_drv(lib) && join != Join::Gap {
                self.in_join_key(&mut join, note, &mut on, len, &mut vel, leg_range, pass, gstep, &mut in_own, cx);
            }
            if join == Join::Slide {
                // Tempo-synced slides when the step from the last note is that value, else the velocity slide.
                let d = on - self.ev_prev.on;
                let art = if (d - 1.0).abs() < 0.06 {
                    EV_SLIDE4
                } else if (d - 0.5).abs() < 0.04 {
                    EV_SLIDE8
                } else if (d - 0.25).abs() < 0.03 {
                    EV_SLIDE16
                } else {
                    EV_VELOCITY_SLIDE
                };
                let slide_key = if at_or_default(&t.ev_art, art as i32) >= 0 {
                    at_or_default(&t.ev_art, art as i32)
                } else {
                    at_or_default(&t.ev_art, EV_VELOCITY_SLIDE as i32)
                };
                on = max(on, self.current_ppq + 16.0 * ms);
                if slide_key < 0 || !self.ev_hold(slide_key, on, on + f64::from(len), note, lib) {
                    join = Join::Gap;
                } else {
                    let fast = self.rand01(pass, gstep, 1975, note, cx) < 0.5f32;
                    let r = (self.rand01(pass, gstep, 1975, note + 128, cx) * 20.99f32) as i32;
                    vel = clamp((if fast { 100 } else { 60 }) + r, t.ev_vel_lo, t.ev_vel_hi);
                }
            }
            let mut own = muted || in_own; // a key of this note's own is held (a section's, or below)
            if single && !own && join == Join::Gap && x.hand != 0 {
                let key = self.ev_note_key(who, pass, prog_bar, sib, step_ppq, &x, on, len, cx);
                if key >= 0 {
                    let at = max(on, self.current_ppq + 16.0 * ms);
                    self.ev_key_clear(at);
                    self.ev_trim(at - 3.0 * ms, note, leg_range); // before the hold: an earlier note of this pitch isn't its owner
                    if self.ev_hold(key, at, at + f64::from(len), note, lib) {
                        on = at;
                        own = true;
                        if key == at_or_default(&t.ev_art, EV_HARMONICS as i32) {
                            self.ev_harm_sec = self.ev_section_id(pass);
                            self.ev_harm_at = on;
                        }
                    }
                }
            }
            self.ev_vib_stop(on - 3.0 * ms, lib);
            if join != Join::Slide && !own {
                self.ev_key_up_before(on, join == Join::Gap); // not this note's key: up before it
            }
            if join != Join::Gap {
                self.fd_move_off(self.ev_prev.note, on + 20.0 * ms); // the 20 ms overlap Evolution hears as legato
            } else {
                self.ev_trim(on - 3.0 * ms, note, leg_range); // never a hammer by accident
            }
            if x.hand == 0 {
                // A bass note on its string (spec 3.8): the string-select key from 5 ms before to the note's start.
                let open = self.ev_open_strings(lib);
                let mut s = 0;
                for k in 0..clamp(t.ev_string_count, 1, 6) {
                    if at_or_default(&open, k) >= 0 && at_or_default(&open, k) <= note {
                        s = k;
                    }
                }
                let key = at_or_default(&t.ev_string_keys, s);
                let key_down = self.ev_held.key >= 0 && self.ev_held.key_off > on; // a held key (or a legato join) wins: no string tap
                if key >= 0 && on - 5.0 * ms >= self.current_ppq && !key_down && join == Join::Gap {
                    let now = self.current_ppq;
                    let mut sa = SchedAt { q: &mut self.q, now };
                    let mut c = kit::DriverCtx { sink: &mut sa, now, ms };
                    kit::tap_key(&mut c, key, 100, on - 5.0 * ms, 5.0);
                }
            }
            self.schedule(0, note, vel, on, len);
            let end = on + f64::from(len);
            if single && !own && join == Join::Gap && x.hand != 0 {
                self.ev_bend(who, pass, gstep, note, on, end, cx);
            }
            if single && x.hand != 0 && pl.vibrato > 0.0f32 && t.ev_vibrato_cc >= 0 && x.len >= (if self.in_drv(lib) { 1.0f32 } else { 0.9f32 }) {
                // Indiginus: notes of a beat or more
                // CC1 over the note's second half (1/16-beat steps, at most 8), back to 0 at its end.
                let half = f64::from(len) * 0.5;
                let step_len_ppq = max(1.0 / 16.0, half / 8.0);
                let mut last = 0;
                let mut at = on + half + step_len_ppq;
                while at < end - 1.0e-6 {
                    let v = clamp(
                        (plucked::vibrato_curve((at - on) / f64::from(len), pl.vibrato)
                            * (if self.in_drv(lib) { 100.0f32 / 90.0f32 } else { 1.0f32 })) as i32,
                        0,
                        127,
                    );
                    if v != last {
                        self.schedule(1, t.ev_vibrato_cc, v, at, 0.0f32);
                        last = v;
                    }
                    at += step_len_ppq;
                }
                if last > 0 {
                    self.schedule(1, t.ev_vibrato_cc, 0, end, 0.0f32);
                    self.ev_vib_start = on + half;
                    self.ev_vib_end = end;
                }
            }
            if single {
                self.ev_push_prev(note, on, end);
            } else {
                self.ev_prev = EvPrev::default();
                self.ev_push_prev(-1, on, end);
            }
        }
    }

    /// `int evNoteKey (int who, int pass, int progBar, int sib, float64 stepPpq, const AmpleNote& x, float64 on, float len)`
    pub(crate) fn ev_note_key(
        &mut self,
        who: i32,
        pass: i32,
        prog_bar: i32,
        sib: i32,
        step_ppq: f64,
        x: &AmpleNote,
        on: f64,
        len: f32,
        cx: &mut Ctx<'_>,
    ) -> i32 {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let pl = &self.feel_for(who, lib).plucked;
        let gstep = self.positive_modulo(prog_bar, 2) * self.spb() + sib;
        let note = x.note;
        let tick = f64::from(self.tick_ppq(lib));
        let q = self.positive_modulo(self.floor_to_int((step_ppq - (step_ppq + 1.0e-6).floor()) * 4.0 + 0.5), 4);
        if self.ev_prev.note >= 0 && on - self.ev_prev.end > tick * 0.5 && q != 0 && at_or_default(&t.ev_art, EV_WS_HAMMER as i32) >= 0 {
            let iv = (note - self.ev_prev.note).abs();
            if (1..=2).contains(&iv) && self.rand01(pass, gstep, 1995, note + 256, cx) < pl.hammer {
                return at_or_default(&t.ev_art, EV_WS_HAMMER as i32);
            }
        }
        let bar_l = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
        let bar_end = self.bar_floor(step_ppq, bar_l) + bar_l;
        let last_end = if self.ev_hist_count > 0 { at_or_default(&self.ev_hist, self.ev_hist_count - 1).end } else { -1.0e9 };
        let mut c = OrnContext::default();
        // A phrase starts after a rest of a beat or more, or on the first melody note of a phrase's first bar.
        let bar_start = bar_end - bar_l;
        let phrase_bar = prog_bar == 0
            || self.is_phrase_end(prog_bar - 1)
            || (self.cur.is_some_and(|c| c.chord_index == 0) && step_ppq < self.cur_start_ppq + bar_l - 1.0e-6);
        let first_in_bar = phrase_bar && (self.ev_phrase_bar - bar_start).abs() > 1.0e-6;
        if phrase_bar {
            self.ev_phrase_bar = bar_start;
        }
        c.phrase_start = on - last_end >= 1.0 - 1.0e-6 || first_in_bar;
        c.lick_end = self.lick_now.land;
        c.phrase_end = self.is_phrase_end(prog_bar) && on + f64::from(len) >= bar_end - tick * 0.5;
        c.section_end = self.cur.is_some_and(|cu| cu.chord_index == cu.section_chords - 1)
            && on >= self.cur_end_ppq - 1.0
            && self.ev_harm_sec != self.ev_section_id(pass);
        let pc = self.positive_modulo(note - self.key_root, 12);
        c.tonic_or_fifth = pc == 0 || pc == 7;
        c.accented_high = x.vel >= 95.0f32 && note >= 72;
        c.from_below = self.ev_prev.note < 0 || note >= self.ev_prev.note;
        c.release_target = t.ev_bend_blues;
        c.len_beats = len;
        c.energy = self.energy;
        c.graces = pl.graces;
        c.buzz = pl.buzz;
        c.slides = pl.slides;
        c.harmonics = pl.harmonics;
        c.has_grace_up = at_or_default(&t.ev_art, EV_GRACE_UP as i32) >= 0;
        c.has_grace_down = at_or_default(&t.ev_art, EV_GRACE_DOWN as i32) >= 0;
        c.has_buzz = at_or_default(&t.ev_art, EV_BUZZ_TRILL as i32) >= 0;
        c.has_harmonics = at_or_default(&t.ev_art, EV_HARMONICS as i32) >= 0;
        c.has_release_slide = at_or_default(&t.ev_art, EV_RELEASE_SLIDE_DOWN as i32) >= 0;
        c.has_squeal = at_or_default(&t.ev_art, EV_SQUEAL as i32) >= 0;
        let ind = self.in_drv(lib);
        let rolls: [f32; 4] = [
            self.rand01(pass, gstep, if ind { 1987 } else { 1981 }, note, cx),
            self.rand01(pass, gstep, if ind { 1987 } else { 1982 }, if ind { note + 128 } else { note }, cx),
            self.rand01(pass, gstep, 1983, note, cx),
            self.rand01(pass, gstep, 1984, note, cx),
        ]; // Indiginus: grace / trill 1987
        match plucked::choose_ornament(&c, &rolls) {
            Orn::GraceUp => at_or_default(&t.ev_art, EV_GRACE_UP as i32),
            Orn::GraceDown => at_or_default(&t.ev_art, EV_GRACE_DOWN as i32),
            Orn::Buzz => at_or_default(&t.ev_art, EV_BUZZ_TRILL as i32),
            Orn::Harmonics => at_or_default(&t.ev_art, EV_HARMONICS as i32),
            Orn::ReleaseSlide => at_or_default(&t.ev_art, EV_RELEASE_SLIDE_DOWN as i32),
            Orn::Squeal => at_or_default(&t.ev_art, EV_SQUEAL as i32),
            Orn::None => -1,
        }
    }

    /// `void evBend (int who, int pass, int gstep, int note, float64 on, float64 end)`
    pub(crate) fn ev_bend(&mut self, who: i32, pass: i32, gstep: i32, note: i32, on: f64, end: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let slides_now = self.feel_for(who, lib).plucked.slides;
        if !t.ev_bend_blues || slides_now <= 0.0f32 {
            return;
        }
        let pc = self.positive_modulo(note - self.key_root, 12);
        if pc != 3 && pc != 10 {
            return;
        }
        if self.rand01(pass, gstep, 1986, note, cx) >= slides_now {
            return;
        }
        let ms = self.ppq_per_ms();
        let start = max(self.current_ppq, on - 1.0 * ms);
        let span = min(0.25, max(0.02, (end - on) * 0.8));
        self.schedule(4, 0, 0, start, 0.0f32);
        for k in 1..=4 {
            let v = min(8192, 2048 * k);
            self.schedule(4, v & 127, v >> 7, on + span * f64::from(k) / 4.0, 0.0f32);
        }
        self.ev_bend_end = on + span;
    }
}

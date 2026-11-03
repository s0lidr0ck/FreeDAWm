//! `MusicianIndiginus.cpp`: Musician: Indiginus plucked strings (Banjo / Mandolin / Ukulele / Resonator), ported
//! line by line.
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.

use super::*;
use crate::library::{IN_DOWN_SLIDE, IN_HAMMER, IN_PULL_OFF, IN_UP_SLIDE};

impl Musician {
    /// `void inReset ()`
    pub(crate) fn in_reset(&mut self) {
        self.in_init_pass = -1;
        self.in_init_at = -1.0e9;
        self.in_ready_at = -1.0e18;
        self.in_expr.reset();
        self.in_slot_sig = -1;
        self.in_slot_at = -1.0e18;
    }

    /// `void inSlotsStrum (int progBar, float64 stepPpq, float64 on, float loud, float len)`
    pub(crate) fn in_slots_strum(&mut self, prog_bar: i32, step_ppq: f64, on: f64, loud: f32, len: f32, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let ms = self.ppq_per_ms();
        let ch = self.chord_for_bar(prog_bar, cx);
        let root_pc = self.positive_modulo(self.key_root + ch.root_pc, 12);
        let third = ch.tones.val(1);
        let fifth = ch.tones.val(2);
        let quality = if third == 4 && ch.count >= 4 && ch.tones.val(3) == 10 {
            2
        } else if third == 3 && fifth == 7 {
            1
        } else {
            0
        };
        let sig = root_pc * 3 + quality;
        let q = self.positive_modulo(self.floor_to_int((step_ppq - (step_ppq + 1.0e-6).floor()) * 4.0 + 0.5), 4);
        let base = if q == 0 { t.in_chord_down } else { t.in_chord_up };
        let on = max(on, self.current_ppq + (if sig != self.in_slot_sig { 23.0 } else { 4.0 }) * ms);
        self.q.schedule_flags = 8192; // keys below the range
        if sig != self.in_slot_sig {
            // Key layout is target data (chordKeys): a -1 root-select / variation key is never sent (Ukulele: the strum key carries the root).
            if t.in_root_select >= 0 {
                self.schedule(0, t.in_root_select + root_pc, 100, on - 22.0 * ms, (5.0 * ms) as f32);
            }
            let variation = at_or_default(&t.in_variation, quality);
            if variation >= 0 {
                self.schedule(0, variation, 100, on - 20.0 * ms, (5.0 * ms) as f32);
            }
            self.in_slot_sig = sig;
            self.in_slot_at = on;
        }
        self.schedule(0, base + root_pc, clamp(loud as i32, t.ev_vel_lo, t.ev_vel_hi), on, max(0.02f32, len - (3.0 * ms) as f32));
        self.q.schedule_flags = 0;
    }

    /// `int inKeyFlags ()`
    pub(crate) fn in_key_flags(&mut self, lib: &LibraryData) -> i32 {
        if !self.in_drv(lib) {
            return 0;
        }
        let mode = self.current_target(lib).in_return_mode;
        if mode == 2 {
            K_IN_LATCH_FLAG
        } else if mode == 3 {
            K_IN_SAME_FLAG
        } else {
            0
        }
    }

    /// `bool inBlocked (int note, int flags)`
    pub(crate) fn in_blocked(&mut self, note: i32, flags: i32, lib: &LibraryData) -> bool {
        let t = self.current_target(lib);
        self.in_never(note, lib) || ((flags & 8192) == 0 && (note < t.in_low || note > t.in_high))
    }

    /// `void inSetup (int pass)`
    pub(crate) fn in_setup(&mut self, pass: i32, lib: &LibraryData) {
        if self.in_init_pass == pass {
            return;
        }
        self.in_init_pass = pass;
        let t = self.current_target(lib);
        let ms = self.ppq_per_ms();
        let at = self.current_ppq + 0.5 * ms;
        let k = {
            let now = self.current_ppq;
            let mut s = SchedAt { q: &mut self.q, now };
            let mut c = kit::DriverCtx { sink: &mut s, now, ms };
            kit::send_init_keys(&mut c, &t.in_init_keys, t.in_init_keys.len() as i32, 100, at, 10.0, 5.0)
        };
        if k > 0 {
            self.in_init_at = at;
            self.in_ready_at = at + 30.0 * ms;
        }
    }

    /// `void inDropFrom (float64 cut)`
    pub(crate) fn in_drop_from(&mut self, cut: f64, lib: &LibraryData) {
        if !self.in_drv(lib) {
            return;
        }
        if self.in_init_at >= cut - 1.0e-9 {
            self.in_init_pass = -1;
            self.in_init_at = -1.0e9;
        }
        if self.in_slot_at >= cut - 1.0e-9 {
            self.in_slot_sig = -1;
            self.in_slot_at = -1.0e18;
        }
        self.in_expr.drop_from(cut);
    }

    /// `void inExpression (int progBar, float64 stepPpq)`
    #[allow(clippy::approx_constant)] // MS's own literal 3.14159265358979 (not f64::consts::PI): the same float
    pub(crate) fn in_expression(&mut self, prog_bar: i32, step_ppq: f64, lib: &LibraryData) {
        let t = self.current_target(lib);
        if t.in_expr_cc < 0 {
            return;
        }
        let beat = self.beat_ppq();
        let mut ons = [0.0f64; 64];
        let mut n = 0usize;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 0 && q.ch == 0 && (q.flags & 8192) == 0 && q.on_ppq > self.in_expr.covered + 1.0e-9 {
                if let Some(o) = ons.get_mut(n) {
                    *o = q.on_ppq;
                }
                n += 1;
            }
        }
        if n == 0 {
            return;
        }
        if let Some(s) = ons.get_mut(..n) {
            s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        }
        let bar_l = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
        let in_bar = step_ppq - self.bar_floor(step_ppq, bar_l);
        let pos = (f64::from(self.positive_modulo(prog_bar, 4)) + in_bar / bar_l) / 4.0; // the 4-bar phrase, 0..1
        let base = 70 + 15 * (clamp(self.energy, 1, 4) - 1); // 70 / 85 / 100 / 115
        let value = clamp(base - 6 + (12.0 * (3.14159265358979 * pos).sin()).round() as i32, 70, 115);
        let ms = self.ppq_per_ms();
        let now = self.current_ppq;
        let mut s = SchedAt { q: &mut self.q, now };
        let mut c = kit::DriverCtx { sink: &mut s, now, ms };
        kit::cc_before_notes(&mut c, &mut self.in_expr, t.in_expr_cc, value, &ons, n as i32, 5.0, beat / 12.0);
    }

    /// `void inJoinKey (plucked::Join& join, int note, float64& on, float len, int& vel, int legRange, int pass, int gstep, bool& own)`
    pub(crate) fn in_join_key(
        &mut self,
        join: &mut Join,
        note: i32,
        on: &mut f64,
        len: f32,
        vel: &mut i32,
        leg_range: i32,
        pass: i32,
        gstep: i32,
        own: &mut bool,
        cx: &mut Ctx<'_>,
    ) {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let ms = self.ppq_per_ms();
        let up = note > self.ev_prev.note;
        let slide = *join == Join::Slide;
        let mut key = -1;
        if t.in_return_mode != 0 {
            let which = if slide {
                if up { IN_UP_SLIDE } else { IN_DOWN_SLIDE }
            } else if up {
                IN_HAMMER
            } else {
                IN_PULL_OFF
            };
            key = at_or_default(&t.in_art, which as i32);
        }
        if key < 0 {
            if slide {
                *join = Join::Gap;
            }
            return;
        }
        let at = max(*on, self.current_ppq + 11.0 * ms);
        self.ev_key_clear(at);
        self.ev_trim(at - 3.0 * ms, note, leg_range); // before the hold: an earlier note of this pitch isn't its owner
        if !self.ev_hold(key, at, at + f64::from(max(0.02f32, len)), note, lib) {
            if slide {
                *join = Join::Gap;
            }
            return;
        }
        *on = at;
        *own = true;
        *join = Join::Gap; // the key plays the join: the note after a 3 ms gap
        if slide {
            let fast = self.rand01(pass, gstep, 1986, note + 128, cx) < 0.5f32;
            *vel = clamp((if fast { 90 } else { 60 }) + (self.rand01(pass, gstep, 1986, note + 256, cx) * 20.99f32) as i32, t.ev_vel_lo, t.ev_vel_hi);
        }
    }

    /// `void inResonatorLine ()`
    pub(crate) fn in_resonator_line(&mut self) {
        if let Some(s) = self.ev_notes.get_mut(..usize::try_from(self.ev_count).unwrap_or(0)) {
            s.sort_by_key(|a| std::cmp::Reverse(a.note)); // stable: at most 16 notes here, and below its 32-element threshold MSVC's std::sort is an insertion sort (stable)
        }
        let keep = if self.energy >= 4 { 3 } else { 1 };
        let mut w = 0usize;
        let count = usize::try_from(self.ev_count).unwrap_or(0);
        let mut i = 0usize;
        while i < count && w < keep {
            let x = self.ev_notes.get(i).copied().unwrap_or_default();
            let mut dup = false;
            for j in 0..w {
                if self.ev_notes.get(j).is_some_and(|y| y.note == x.note) {
                    dup = true;
                }
            }
            if !dup {
                if let Some(y) = self.ev_notes.get_mut(w) {
                    *y = x;
                }
                w += 1;
            }
            i += 1;
        }
        self.ev_count = w as i32;
    }
}

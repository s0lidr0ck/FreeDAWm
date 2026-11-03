//! `MusicianKeyswitch.cpp`: the data-driven keyswitch driver (Tasks 14, 14b), ported line by line.
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.
//! Differences of form: `kitCtx()` is built in place for each kit call (a `SchedAt` over `self.q` at `currentPpq`);
//! `t.ksArts[(size_t) i]` / `t.ksUse[(size_t) i]` / `ksHoldHist[(size_t) i]` read through `at_or_default` / `get`.
//
// Everything comes from the target's data: named articulations (key, mode, minLen), which articulation plays each
// musical intent (and a default), the dynamics / expression / vibrato CCs and their leads, the values sent on stop,
// a note range, a velocity clamp, setup keys and never-send keys. performStep phrases a section for it exactly as
// for articulationKeys (one articulation per step, CC dynamics and expression), so a library re-expressed on it
// plays the same events (Crow Hill Pocket Strings; keyswitchtest proves it).
// Modes: tap    on and off at the same instant when the articulation changes (articulationKeys' tap, at the note)
//        hold   held from keyLeadMs before its note to 5 ms after the note's end (at least minLen)
//        latch  a 5 ms tap when it changes; the previous latch is returned first (return: same / key)
//        same   a 5 ms tap before each note and the same key again 5 ms after it
// Audio thread: no allocation, no virtual dispatch. Kit parts: holdKey, tapKey, ReturnMode, sendInitKeys,
// neverSend, plucked::vibratoCurve.

use super::*;
use crate::library::{KS_GHOST, KS_HOLD, KS_LATCH, KS_LEGATO_UP, KS_SAME, KS_SLIDE_IN, KS_TAP};

impl Musician {
    /// `int ksArtFor (int flags)`
    pub(crate) fn ks_art_for(&mut self, flags: i32, lib: &LibraryData) -> i32 {
        let t = self.current_target(lib);
        let mapped = |intent: i32| at_or_default(&t.ks_use, intent);
        if (flags & K_HAMMER_FLAG) != 0 && mapped(KS_LEGATO_UP as i32) >= 0 {
            return mapped(KS_LEGATO_UP as i32);
        }
        if (flags & K_SLIDE_FLAG) != 0 && mapped(KS_SLIDE_IN as i32) >= 0 {
            return mapped(KS_SLIDE_IN as i32);
        }
        if (flags & 1) != 0 && mapped(KS_GHOST as i32) >= 0 {
            return mapped(KS_GHOST as i32);
        }
        let art = mapped(Self::ks_intent_of(flags));
        if art >= 0 { art } else { t.ks_default }
    }

    /// `bool ksIsKey (int note)`
    pub(crate) fn ks_is_key(&mut self, note: i32, lib: &LibraryData) -> bool {
        let t = self.current_target(lib);
        for i in 0..t.ks_art_count {
            if self.ks(t.ks_key_base + at_or_default(&t.ks_arts, i).key) == note {
                return true;
            }
        }
        if t.ks_return_key >= 0 && self.ks(t.ks_return_key) == note {
            return true;
        }
        for &k in &t.ks_init {
            if k >= 0 && self.ks(k) == note {
                return true;
            }
        }
        false
    }

    /// `void ksSetup (int pass)`
    pub(crate) fn ks_setup(&mut self, pass: i32, lib: &LibraryData) {
        // The setup keys once per pass (as Indiginus sends its init keys): 5 ms taps 10 ms apart.
        if self.ks_init_pass == pass {
            return;
        }
        self.ks_init_pass = pass;
        let t = self.current_target(lib);
        let mut keys = [0i32; 8];
        for (k, &init) in keys.iter_mut().zip(t.ks_init.iter()) {
            *k = if init >= 0 { self.ks(init) } else { -1 };
        }
        let sf = self.q.schedule_flags;
        self.ks_init_at = self.current_ppq + 0.5 * self.ppq_per_ms();
        let (now, ms, at) = (self.current_ppq, self.ppq_per_ms(), self.ks_init_at);
        let mut s = SchedAt { q: &mut self.q, now };
        let mut c = kit::DriverCtx { sink: &mut s, now, ms };
        kit::send_init_keys(&mut c, &keys, 8, 100, at, 10.0, 5.0);
        self.q.schedule_flags = sf;
    }

    /// `bool ksNote (int& outNote, float& v, float64 onPpq, float len, int flags)`
    pub(crate) fn ks_note(&mut self, out_note: &mut i32, v: &mut f32, on_ppq: f64, len: f32, flags: i32, lib: &LibraryData) -> bool {
        let t = self.current_target(lib);
        // Range: fold by octave; a folded note never lands on a key.
        if *out_note < t.ks_low || *out_note > t.ks_high {
            let mut n = *out_note;
            while n < t.ks_low {
                n += 12;
            }
            while n > t.ks_high {
                n -= 12;
            }
            if n < t.ks_low || !(0..=127).contains(&n) || self.ks_is_key(n, lib) {
                return false;
            }
            *out_note = n;
        }
        if self.ks_never(*out_note, lib) {
            return false; // a dropped note: no key, no vibrato for it
        }
        *v = clamp(*v, t.ks_vel_lo as f32, t.ks_vel_hi as f32);

        // `const auto c = kitCtx()`: built in place at each kit call below (currentPpq does not move in here).
        let now = self.current_ppq;
        let ms = self.ppq_per_ms();
        let sf = self.q.schedule_flags; // the note's flags: the kit resets them after each key (nesting-safe here)
        let art = self.ks_art_for(flags, lib);
        if art >= 0 && art < t.ks_art_count {
            let a = at_or_default(&t.ks_arts, art);
            let key = self.ks(t.ks_key_base + a.key);
            let at = on_ppq - f64::from(t.ks_key_lead_ms) * ms;
            let end = on_ppq + f64::from(len);
            if a.mode == KS_HOLD {
                // One held key covers overlapping notes (a chord): pressed again only when a note outlasts it.
                let up_at = max(end + 5.0 * ms, at + f64::from(a.min_len_ms) * ms);
                if key != self.ks_hold_key
                    && self.ks_hold_key >= 0
                    && self.ks_hold_until > at
                    && (self.ks_hold_owner_end <= on_ppq + 1.0e-9 || self.pattern == 2)
                {
                    /* a mono line: the next note ends the last */
                    self.ks_set_hold_off(self.ks_hold_key, max(self.current_ppq, at - 1.0 * ms), true); // a new articulation: the old key up first (not inside a chord)
                }
                if key != self.ks_hold_key || at > self.ks_hold_until {
                    {
                        let mut s = SchedAt { q: &mut self.q, now };
                        let mut c = kit::DriverCtx { sink: &mut s, now, ms };
                        kit::hold_key(&mut c, key, 100, at, (up_at - at) as f32, kit::K_BELOW_RANGE);
                    }
                    self.ks_hold_key = key;
                    self.ks_hold_at = at;
                    self.ks_hold_until = up_at;
                    self.ks_hold_owner_on = on_ppq;
                    self.ks_hold_owner_end = end;
                    self.ks_hold_hist_n = 0;
                    self.ks_hold_hist[0] = KsHoldStep { owner_on: on_ppq, until: up_at };
                    self.ks_hold_hist_n += 1;
                } else if up_at > self.ks_hold_until + 1.0e-9 || at < self.ks_hold_at {
                    // Overlapping and longer (or earlier): every copy of the held key (queued or sounding) is released at
                    // the later time; an earlier note gets its own press (closed by the planned one, which carries it on).
                    let until = max(up_at, self.ks_hold_until);
                    let held = self.ks_set_hold_off(key, until, false);
                    if at < self.ks_hold_at || !held {
                        // or its key came up already
                        let from = max(at, self.current_ppq);
                        let mut s = SchedAt { q: &mut self.q, now };
                        let mut c = kit::DriverCtx { sink: &mut s, now, ms };
                        kit::hold_key(&mut c, key, 100, from, (until - from) as f32, kit::K_BELOW_RANGE);
                    }
                    self.ks_hold_at = min(self.ks_hold_at, at);
                    self.ks_hold_until = until;
                    if up_at >= until - 1.0e-9 {
                        self.ks_hold_owner_on = on_ppq;
                        self.ks_hold_owner_end = end;
                        if self.ks_hold_hist_n == self.ks_hold_hist.len() as i32 {
                            for i in 1..self.ks_hold_hist_n {
                                let x = at_or_default(&self.ks_hold_hist, i);
                                if let Some(d) = usize::try_from(i - 1).ok().and_then(|k| self.ks_hold_hist.get_mut(k)) {
                                    *d = x;
                                }
                            }
                            self.ks_hold_hist_n -= 1;
                        }
                        if let Some(d) = usize::try_from(self.ks_hold_hist_n).ok().and_then(|k| self.ks_hold_hist.get_mut(k)) {
                            *d = KsHoldStep { owner_on: on_ppq, until };
                        }
                        self.ks_hold_hist_n += 1;
                    }
                }
            } else if a.mode == KS_SAME {
                // One tap before a chord (overlapping notes) and one after its latest end.
                let after = end + 5.0 * ms;
                if key == self.ks_same_key && at < self.ks_same_end_at {
                    if at < self.ks_same_at && self.ks_move_queued(key, self.ks_same_at, at) {
                        self.ks_same_at = at;
                    }
                    if after > self.ks_same_end_at {
                        if !self.ks_move_queued(key, self.ks_same_end_at, after) {
                            let mut s = SchedAt { q: &mut self.q, now };
                            let mut c = kit::DriverCtx { sink: &mut s, now, ms };
                            kit::tap_key(&mut c, key, 100, after, 5.0);
                        }
                        self.ks_same_end_at = after;
                    }
                } else {
                    {
                        let mut s = SchedAt { q: &mut self.q, now };
                        let mut c = kit::DriverCtx { sink: &mut s, now, ms };
                        kit::tap_key(&mut c, key, 100, at, 5.0);
                        kit::tap_key(&mut c, key, 100, after, 5.0);
                    }
                    self.ks_same_key = key;
                    self.ks_same_at = at;
                    self.ks_same_end_at = after;
                }
            } else if a.mode == KS_LATCH && self.ks_ret.latched != key {
                let ret_key = if t.ks_return_key >= 0 { self.ks(t.ks_return_key) } else { -1 };
                let mut s = SchedAt { q: &mut self.q, now };
                let mut c = kit::DriverCtx { sink: &mut s, now, ms };
                if self.ks_ret.latched >= 0 {
                    self.ks_ret_fire_at = at - 6.0 * ms;
                    self.ks_ret.fire(&mut c, self.ks_ret_fire_at, t.ks_return, ret_key);
                }
                self.ks_ret.latch(&mut c, key, at, end);
            } else if a.mode == KS_LATCH {
                self.ks_ret.due = max(self.ks_ret.due, end);
            }
        }
        // Vibrato over a note of a beat or more (one curve at a time): plucked::vibratoCurve from half way.
        if t.ks_vibrato_cc >= 0 && self.expressive() && len >= 1.0f32 && on_ppq >= self.ks_vib_until {
            let mut s = SchedAt { q: &mut self.q, now };
            let mut c = kit::DriverCtx { sink: &mut s, now, ms };
            let mut last = 0;
            for k in 5..=8 {
                let val = clamp(crate::plucked_voice::vibrato_curve(f64::from(k) / 8.0, t.ks_vibrato_depth) as i32, 0, 127);
                if val != last {
                    c.send(1, t.ks_vibrato_cc, val, on_ppq + f64::from(len) * f64::from(k) / 8.0 - 2.0 * ms, 0.0f32, kit::K_KEEP_FLAGS);
                    last = val;
                }
            }
            if last != 0 {
                c.send(1, t.ks_vibrato_cc, 0, on_ppq + f64::from(len) + 1.0 * ms, 0.0f32, kit::K_KEEP_FLAGS);
            }
            self.ks_vib_until = on_ppq + f64::from(len) + 1.0 * ms;
            self.ks_vib_start = on_ppq + f64::from(len) * 5.0 / 8.0 - 2.0 * ms;
        }
        self.q.schedule_flags = sf;
        true
    }

    /// `bool ksSetHoldOff (int key, double until, bool shorten)`
    pub(crate) fn ks_set_hold_off(&mut self, key: i32, until: f64, shorten: bool) -> bool {
        let mut any = false;
        for i in 0..64 {
            let current_ppq = self.current_ppq;
            let q = self.q.queue.at_mut(i);
            if q.used && q.kind == 0 && q.d1 == key && q.on_ppq < until {
                q.dur_ppq = (if shorten { max(0.01, until - q.on_ppq) } else { max(f64::from(q.dur_ppq), until - q.on_ppq) }) as f32;
                any = true;
            }
            if self.active_slots.val(i)
                && self.active_channels.val(i) == self.midi_channel0
                && self.active_notes.val(i) == key
                && self.active_off_ppq.val(i) > current_ppq
            {
                let off = if shorten { max(until, current_ppq) } else { max(self.active_off_ppq.val(i), until) };
                self.active_off_ppq.set(i, off);
                any = true;
            }
        }
        any
    }

    /// `bool ksMoveQueued (int key, double from, double to)`
    pub(crate) fn ks_move_queued(&mut self, key: i32, from: f64, to: f64) -> bool {
        for i in 0..64 {
            let p = *self.q.queue.at(i);
            if p.used && p.kind == 0 && p.d1 == key && (p.on_ppq - from).abs() < 1.0e-9 {
                self.q.queue.at_mut(i).on_ppq = max(to, self.current_ppq);
                return true;
            }
        }
        false
    }

    /// `void ksStepEnd (int step)`
    pub(crate) fn ks_step_end(&mut self, step: i32, lib: &LibraryData) {
        // A latch with a return comes back after its note when no later note kept it (a rest): due + 5 ms.
        let t = self.current_target(lib);
        if !self.ks_active(lib) || t.ks_return == 0 || self.ks_ret.latched < 0 || self.ks_ret.due < 0.0 {
            return;
        }
        let ms = self.ppq_per_ms();
        let fire_at = self.ks_ret.due + 5.0 * ms;
        if f64::from(step + 1) * f64::from(self.tick_ppq(lib)) < fire_at {
            return;
        }
        let sf = self.q.schedule_flags;
        self.ks_ret_fire_at = max(fire_at, self.current_ppq);
        let ret_key = if t.ks_return_key >= 0 { self.ks(t.ks_return_key) } else { -1 };
        let (now, at) = (self.current_ppq, self.ks_ret_fire_at);
        let mut s = SchedAt { q: &mut self.q, now };
        let mut c = kit::DriverCtx { sink: &mut s, now, ms };
        self.ks_ret.fire(&mut c, at, t.ks_return, ret_key);
        self.q.schedule_flags = sf;
    }

    /// `void ksVibratoOff ()`
    pub(crate) fn ks_vibrato_off(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let t = self.current_target(cx.lib);
        if t.driver != Driver::Keyswitch || t.ks_vibrato_cc < 0 || self.ks_vib_until <= self.current_ppq {
            return;
        }
        for i in 0..64 {
            let p = *self.q.queue.at(i);
            if p.used && p.kind == 1 && p.d1 == t.ks_vibrato_cc {
                self.q.queue.at_mut(i).used = false;
            }
        }
        if self.ks_vib_start <= self.current_ppq {
            self.send_midi(0xB0 + self.midi_channel0, t.ks_vibrato_cc, 0, cx, out);
        }
        self.ks_vib_until = -1.0e18;
        self.ks_vib_start = -1.0e18;
    }

    /// `void ksDropFrom (float64 cut)`
    pub(crate) fn ks_drop_from(&mut self, cut: f64, lib: &LibraryData) {
        if !self.driver_is(Driver::Keyswitch, lib) {
            return;
        }
        // Hold: rebuilt from what was really sent.
        if self.ks_hold_key >= 0 {
            if self.ks_hold_at >= cut - 1.0e-9 {
                self.ks_hold_key = -1;
                self.ks_hold_until = -1.0e18;
            } else if self.ks_hold_owner_on >= cut - 1.0e-9 {
                // Its owner went: back to the release the notes before the cut asked for (at the cut at the earliest).
                while self.ks_hold_hist_n > 0 && at_or_default(&self.ks_hold_hist, self.ks_hold_hist_n - 1).owner_on >= cut - 1.0e-9 {
                    self.ks_hold_hist_n -= 1;
                }
                let last = if self.ks_hold_hist_n > 0 { at_or_default(&self.ks_hold_hist, self.ks_hold_hist_n - 1).until } else { cut };
                let up = max(max(cut, self.current_ppq), last);
                self.ks_set_hold_off(self.ks_hold_key, up, true);
                self.ks_hold_until = up;
                self.ks_hold_owner_on =
                    if self.ks_hold_hist_n > 0 { at_or_default(&self.ks_hold_hist, self.ks_hold_hist_n - 1).owner_on } else { -1.0e18 };
                self.ks_hold_owner_end = up;
            }
        }
        // Same: a group whose taps went decides again.
        if self.ks_same_at >= cut - 1.0e-9 {
            self.ks_same_key = -1;
            self.ks_same_at = -1.0e18;
            self.ks_same_end_at = -1.0e18;
        } else if self.ks_same_end_at >= cut - 1.0e-9 {
            {
                let (now, ms, key) = (self.current_ppq, self.ppq_per_ms(), self.ks_same_key);
                let mut s = SchedAt { q: &mut self.q, now };
                let mut c = kit::DriverCtx { sink: &mut s, now, ms };
                kit::tap_key(&mut c, key, 100, max(cut, now), 5.0);
            }
            self.q.schedule_flags = 0;
            self.ks_same_key = -1;
            self.ks_same_at = -1.0e18;
            self.ks_same_end_at = -1.0e18;
        }
        // Latch / return.
        let press_queued = self.ks_ret.latch_at >= cut - 1.0e-9;
        let ret_queued = self.ks_ret_fire_at >= cut - 1.0e-9;
        self.ks_ret.after_drop(press_queued, ret_queued, cut);
        if self.ks_ret_fire_at >= cut - 1.0e-9 {
            self.ks_ret_fire_at = -1.0e18;
        }
        // Vibrato: a curve cut part way comes back to 0 at the cut.
        if self.ks_vib_until >= cut && self.ks_vib_start < cut && self.current_target(lib).ks_vibrato_cc >= 0 {
            let sf = self.q.schedule_flags;
            self.q.schedule_flags = 0;
            self.schedule(1, self.current_target(lib).ks_vibrato_cc, 0, max(cut, self.current_ppq), 0.0f32);
            self.q.schedule_flags = sf;
        }
        self.ks_vib_until = -1.0e18;
        self.ks_vib_start = -1.0e18;
        self.ks_dyn = -1; // a duplicate CC is harmless
        self.ks_expr = -1;
        if self.ks_init_at >= cut - 1.0e-9 {
            self.ks_init_pass = -1;
            self.ks_init_at = -1.0e18;
        }
    }

    /// `void ksArticulate (int flags)`
    pub(crate) fn ks_articulate(&mut self, flags: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let t = self.current_target(cx.lib);
        let art = self.ks_art_for(flags, cx.lib);
        if art < 0 || art >= t.ks_art_count || at_or_default(&t.ks_arts, art).mode != KS_TAP {
            return;
        }
        if t.ks_send_on_change_only && art == self.ks_art_sent {
            return;
        }
        let key = self.ks(t.ks_key_base + at_or_default(&t.ks_arts, art).key);
        // A tap: on and off at the same instant (as articulationKeys sends it).
        self.send_keyswitch(0x90 + self.midi_channel0, key, 100, cx, out);
        self.send_keyswitch(0x80 + self.midi_channel0, key, 0, cx, out);
        self.ks_art_sent = art;
    }

    /// `void ksCC (int cc, int value, float64 when)`
    pub(crate) fn ks_cc(&mut self, cc: i32, value: i32, when: f64, lib: &LibraryData) {
        let v = clamp(value, 0, 127);
        if cc == 1 {
            if v == self.ks_dyn {
                return;
            }
            self.ks_dyn = v;
        } else {
            if v == self.ks_expr {
                return;
            }
            self.ks_expr = v;
        }
        let number = if cc == 1 { self.current_target(lib).ks_dyn_cc } else { self.current_target(lib).ks_expr_cc };
        if number < 0 {
            return;
        }
        if cc == 1 {
            self.ks_dyn_sent = number;
        } else {
            self.ks_expr_sent = number;
        }
        self.schedule(1, number, v, when, 0.0f32);
    }

    /// `void ksReset ()`
    pub(crate) fn ks_reset(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        // The reset values are the current target's; the CC numbers the ones in force when the controllers were sent.
        // Held keys are ordinary sounding notes: stopAllNotes releases them.
        let t = self.current_target(cx.lib);
        let is_ks = t.driver == Driver::Keyswitch;
        let dyn_ = if is_ks { t.ks_reset_dyn } else { 64 };
        let expr = if is_ks { t.ks_reset_expr } else { 127 };
        if self.ks_dyn >= 0 && self.ks_dyn_sent >= 0 && dyn_ >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.ks_dyn_sent, dyn_, cx, out);
        }
        if self.ks_expr >= 0 && self.ks_expr_sent >= 0 && expr >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.ks_expr_sent, expr, cx, out);
        }
        if is_ks && t.ks_vibrato_cc >= 0 && self.ks_vib_until > self.current_ppq {
            self.send_midi(0xB0 + self.midi_channel0, t.ks_vibrato_cc, 0, cx, out);
        }
        if is_ks && self.ks_ret.latched >= 0 {
            let k = ReturnMode::return_for(t.ks_return, self.ks_ret.latched, if t.ks_return_key >= 0 { self.ks(t.ks_return_key) } else { -1 });
            if k >= 0 {
                self.send_keyswitch(0x90 + self.midi_channel0, k, 100, cx, out);
                self.send_keyswitch(0x80 + self.midi_channel0, k, 0, cx, out);
            }
        }
        self.ks_art_sent = -1;
        self.ks_dyn = -1;
        self.ks_expr = -1;
        self.ks_init_pass = -1;
        self.ks_vib_until = -1.0e18;
        self.ks_ret.reset();
        self.ks_hold_key = -1;
        self.ks_hold_at = -1.0e18;
        self.ks_hold_until = -1.0e18;
        self.ks_hold_owner_on = -1.0e18;
        self.ks_hold_owner_end = -1.0e18;
        self.ks_hold_hist_n = 0;
        self.ks_same_key = -1;
        self.ks_same_at = -1.0e18;
        self.ks_same_end_at = -1.0e18;
        self.ks_ret_fire_at = -1.0e18;
        self.ks_vib_start = -1.0e18;
        self.ks_init_at = -1.0e18;
    }
}

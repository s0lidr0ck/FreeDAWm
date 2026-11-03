//! `MusicianBowed.cpp`: bowed strings and winds (SWAM, Embertone, The Fiddle), ported line by line.
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.
//! Differences of form:
//!   * `kitCtx()` is built in place for each kit call (a `SchedAt` over `self.q` at `currentPpq`).
//!   * customArticulate's `instant` lambda is [`Musician::custom_instant`] (it writes `swamTrem` / `swamHarm` and their
//!     notes, picked by `harm`).
//!   * fiddlePerform's `doubleStopPartner` predicate reads a table of `isChordTone` for pitches 0-127, made first
//!     (`isChordTone` takes `&mut self`); every pitch it asks about is inside the Fiddle's range (0-127).
//!   * `bowed::Band` travels in `LickMark::band` as its index ([`band_from`] reads it back).
//!   * Fixed-table reads (`arts[(size_t) i]`, `fit[i]`, `pts[i]`, `sn[i]`, …) go through `at_or_default` / `get`; a
//!     lick set's `notes[0]` / `notes[n - 1]` through `first` / `last` (a set that passes `lickMeterOk` has notes).

use super::*;
use crate::fiddle_voice::{self as bowed, Band, Ornament};
use crate::library::{FD_GRACE_DOWN, FD_GRACE_SLIDE_DOWN, FD_GRACE_SLIDE_UP, FD_GRACE_UP, FD_SCOOP, FD_STACCATO, FD_TRILL_DOWN, FD_TRILL_UP};

/// `(bowed::Band) i` (LickMark::band holds the enum's index).
fn band_from(i: i32) -> Band {
    match i {
        0 => Band::Slide,
        1 => Band::Plain,
        2 => Band::Sing,
        _ => Band::Bow,
    }
}

impl Musician {
    // ---- SWAM state helpers (range, expression, keyswitches, reset) ------------

    /// `void swamRangeFor (int globalIndex, int& lo, int& hi)`
    pub(crate) fn swam_range_for(&mut self, global_index: i32, lo: &mut i32, hi: &mut i32, lib: &LibraryData) {
        *lo = 0;
        *hi = 127;
        if self.driver_is(Driver::Fiddle, lib) {
            *lo = self.current_target(lib).fd_low;
            *hi = self.current_target(lib).fd_high;
            return;
        }
        let name = at_or_default(&K_SWAM_NAMES, clamp(global_index, 0, 12));
        for s in &self.current_target(lib).swam_instruments {
            if s.name == name {
                *lo = s.low;
                *hi = s.high;
                return;
            }
        }
        // Not in this target's family: use the full 13-instrument table the file came from, if listed elsewhere.
        for slot in 0..K_TARGET_SLOTS as i32 {
            if let Some(t) = lib.target(slot)
                && t.driver == Driver::Swam
            {
                for s in &t.swam_instruments {
                    if s.name == name {
                        *lo = s.low;
                        *hi = s.high;
                        return;
                    }
                }
            }
        }
    }

    /// `int swamRange (int note)`
    pub(crate) fn swam_range(&mut self, note: i32, lib: &LibraryData) -> i32 {
        let mut lo = 0;
        let mut hi = 0;
        self.swam_range_for(self.swam_inst, &mut lo, &mut hi, lib);
        let mut n = note;
        while n < lo {
            n += 12;
        }
        while n > hi {
            n -= 12;
        }
        clamp(n, lo, hi)
    }

    /// `void swamExpr (int value, float64 when)`
    pub(crate) fn swam_expr(&mut self, value: i32, when: f64, lib: &LibraryData) {
        let v = clamp(value, 0, 127);
        if v == self.expr_level && when <= self.current_ppq {
            return;
        }
        self.swam_expr_cc_sent =
            if self.driver_is(Driver::Fiddle, lib) { self.current_target(lib).fd_expr_cc } else { self.current_target(lib).swam_expr_cc };
        if self.swam_expr_cc_sent >= 0 {
            self.schedule(1, self.swam_expr_cc_sent, v, when, 0.0f32); // -1: dynamics are on velocity
        }
        self.expr_level = v;
    }

    /// `void swamVib (int value, float64 when)`
    pub(crate) fn swam_vib(&mut self, value: i32, when: f64, lib: &LibraryData) {
        if self.driver_is(Driver::Fiddle, lib) {
            return; // The Fiddle: vibrato comes from velocity, never CC1
        }
        let v = clamp(value, 0, 127);
        if v == self.vib_level && when <= self.current_ppq {
            return;
        }
        self.swam_vib_cc_sent = self.current_target(lib).swam_vib_cc;
        if self.swam_vib_cc_sent >= 0 {
            self.schedule(1, self.swam_vib_cc_sent, v, when, 0.0f32);
        }
        self.vib_level = v;
    }

    /// `void swamKs (int note, int vel, bool hold)`
    pub(crate) fn swam_ks(&mut self, note: i32, vel: i32, hold: bool, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.swam_ks_offset_sent = self.current_target(cx.lib).swam_ks_offset;
        let k = self.ks(note + self.swam_ks_offset_sent);
        self.send_keyswitch(0x90 + self.midi_channel0, k, vel, cx, out);
        if !hold {
            let k = self.ks(note + self.swam_ks_offset_sent);
            self.send_keyswitch(0x80 + self.midi_channel0, k, 0, cx, out);
        }
    }

    /// `void customArticulate (int flags)`
    pub(crate) fn custom_articulate(&mut self, flags: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let arts = &self.current_target(cx.lib).swam_arts;
        let want = if (flags & 32) != 0 {
            2
        } else if (flags & 1024) != 0 {
            1
        } else {
            0
        };
        let latch = if at_or_default(arts, want).key >= 0 { want } else { 0 };
        if latch != self.custom_latch && at_or_default(arts, latch).key >= 0 {
            self.send_keyswitch(0x90 + self.midi_channel0, at_or_default(arts, latch).key, 100, cx, out);
            self.send_keyswitch(0x80 + self.midi_channel0, at_or_default(arts, latch).key, 0, cx, out);
            self.custom_latch = latch;
        }
        self.custom_instant(flags, 3, 64, false, cx, out);
        self.custom_instant(flags, 4, 512, true, cx, out);
    }

    /// customArticulate's `instant (index, flag, state, note)` lambda: `state` / `note` are swamTrem / swamTremNote
    /// (`harm` false) or swamHarm / swamHarmNote (`harm` true).
    fn custom_instant(&mut self, flags: i32, index: i32, flag: i32, harm: bool, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let arts = &self.current_target(cx.lib).swam_arts;
        let want_on = if (flags & flag) != 0 && at_or_default(arts, index).key >= 0 { 1 } else { 0 };
        let state = if harm { self.swam_harm } else { self.swam_trem };
        if want_on == state {
            return;
        }
        if want_on == 1 {
            let note = at_or_default(arts, index).key;
            if harm {
                self.swam_harm_note = note;
            } else {
                self.swam_trem_note = note;
            }
            self.send_keyswitch(0x90 + self.midi_channel0, note, 100, cx, out);
        } else {
            let note = if harm { self.swam_harm_note } else { self.swam_trem_note };
            self.send_keyswitch(0x80 + self.midi_channel0, note, 0, cx, out);
        }
        if harm {
            self.swam_harm = want_on;
        } else {
            self.swam_trem = want_on;
        }
    }

    /// `void swamArticulate (int flags)`
    pub(crate) fn swam_articulate(&mut self, flags: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        if self.driver_is(Driver::Fiddle, lib) {
            return; // no SWAM keyswitches on The Fiddle
        }
        if self.current_target(lib).swam_custom_arts {
            self.custom_articulate(flags, cx, out);
            return;
        }
        if !self.current_target(lib).swam_keyswitches {
            return; // SWAM's own keyswitch layout switched off
        }
        if self.swam_family(lib) == 0 {
            let want_mode = if (flags & 32) != 0 { 64 } else { 20 }; // C: low = bow, mid = pizzicato
            if want_mode != self.swam_play_mode {
                self.swam_ks(0, want_mode, false, cx, out);
                self.swam_play_mode = want_mode;
            }
            let want_trem = if (flags & 64) != 0 { 1 } else { 0 }; // G#: held = tremolo
            if want_trem != self.swam_trem {
                if want_trem == 1 {
                    self.swam_ks(8, 100, true, cx, out);
                    self.swam_trem_note = self.ks(8 + self.swam_ks_offset_sent);
                } else {
                    self.send_keyswitch(0x80 + self.midi_channel0, self.swam_trem_note, 0, cx, out);
                }
                self.swam_trem = want_trem;
            }
            let want_harm = if (flags & 512) != 0 { 1 } else { 0 }; // F#: harmonics
            if want_harm != self.swam_harm {
                if want_harm == 1 {
                    self.swam_ks(6, 30, true, cx, out);
                    self.swam_harm_note = self.ks(6 + self.swam_ks_offset_sent);
                } else {
                    self.send_keyswitch(0x80 + self.midi_channel0, self.swam_harm_note, 0, cx, out);
                }
                self.swam_harm = want_harm;
            }
        } else if self.swam_family(lib) == 1 {
            if (flags & 128) != 0 {
                self.swam_ks(1, 90, false, cx, out); // C#: fall down (velocity = speed)
            }
            if (flags & 256) != 0 {
                self.swam_ks(2, 90, false, cx, out); // D: doit
            }
        } else if self.swam_family(lib) == 2 && (flags & 128) != 0 {
            self.swam_ks(0, 90, false, cx, out); // C: short fall
        }
    }

    /// `void swamReset ()`
    pub(crate) fn swam_reset(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        // Levels track what was *scheduled*, so always send the zeros on a reset.
        if self.expr_level >= 0 && self.swam_expr_cc_sent >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.swam_expr_cc_sent, 0, cx, out);
        }
        if self.vib_level >= 0 && self.swam_vib_cc_sent >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.swam_vib_cc_sent, 0, cx, out);
        }
        if self.swam_trem == 1 {
            self.send_keyswitch(0x80 + self.midi_channel0, self.swam_trem_note, 0, cx, out);
        }
        if self.swam_harm == 1 {
            self.send_keyswitch(0x80 + self.midi_channel0, self.swam_harm_note, 0, cx, out);
        }
        self.expr_level = -1;
        self.vib_level = -1;
        self.swam_play_mode = -1;
        self.swam_trem = 0;
        self.swam_harm = 0;
        self.custom_latch = -1;
        self.swam_note = -1;
        self.sounding_since = -1.0;
        self.last_note_end = -1.0;
        if self.jb_partner >= 0 {
            self.fd_move_off(self.jb_partner, self.current_ppq);
        }
        self.jb_reset(cx.lib);
        if self.fd_partner >= 0 {
            self.fd_move_off(self.fd_partner, self.current_ppq);
        }
        self.fd_return_now(cx, out);
        self.fd_partner = -1;
        self.fd_chop_dip = false;
        self.fd_subs = 0;
        self.fd_prev_note = -1;
        self.fd_prev_on = -1.0;
        self.fd_prev_off = -1.0;
        self.fd_prev_end = false;
        self.fd_prev_fast = false;
    }

    // ---- The fiddler's vocabulary (licks made as steps), performStep -----------

    /// `bool bowedLicksOn ()`
    pub(crate) fn bowed_licks_on(&mut self, cx: &mut Ctx<'_>) -> bool {
        let lib = cx.lib;
        self.perf.natural
            && !self.live_mode
            && !self.perf.live
            && self.cur.is_some()
            && cx.song.is_some()
            && (self.fiddle_active(lib) || (self.swam_active(lib) && self.current_target(lib).swam_velocity_dynamics) || self.ev_natural(lib))
    }

    /// `float lickAmount (int who, int which)`
    pub(crate) fn lick_amount(&mut self, who: i32, which: i32, lib: &LibraryData) -> f32 {
        if self.plucked_active(lib) {
            let pl = &self.feel_for(who, lib).plucked;
            return if which == 0 {
                pl.kickoff
            } else if which == 1 {
                pl.fill_licks
            } else {
                pl.turnaround
            };
        }
        let fd = &self.feel_for(who, lib).fiddle;
        if which == 0 {
            fd.kickoff
        } else if which == 1 {
            fd.fill_licks
        } else {
            fd.turnaround
        }
    }

    /// `int gridRestFrom (int who, int step, int pass, int progBar, int sib)`
    pub(crate) fn grid_rest_from(&mut self, who: i32, step: i32, pass: i32, prog_bar: i32, sib: i32, cx: &mut Ctx<'_>) -> i32 {
        let lib = cx.lib;
        let role = self.feel_for(who, lib).role;
        let n = self.spb(); // the bar's 16ths (grid steps keep bar * 16 + step)
        let mut lanes = [0i32, 1, 2];
        let mut nl = 3;
        if role == 5 {
            lanes[0] = self.melody_lane_for(who, pass, prog_bar / 2, cx);
            lanes[1] = 0;
            nl = 2;
        }
        // the lane melodyStep plays
        else if role != 4 {
            return n;
        }
        let par = self.positive_modulo(prog_bar, 2) * 16;
        let mut from = 0;
        let gr = if role == 5 { GridRole::Melodic } else { GridRole::Ensemble };
        for s in 0..n {
            let g = self.grid_step(who, s, gr, lib); // 4/4 and native grids12: s; adapted 12-step: the 16-step source
            if g < 0 {
                continue;
            }
            for l in 0..nl {
                let lane = at_or_default(&lanes, l);
                if self.lane_hit(who, lane, par + g, step - sib + s, pass, cx) {
                    from = max(from, min(n, s + max(1, self.grid_at(who, lane, par + g, 2, lib))));
                }
            }
        }
        from
    }

    /// `bool gridHoldsNextDownbeat (int who, int step, int pass, int progBar, int sib)`
    pub(crate) fn grid_holds_next_downbeat(&mut self, who: i32, step: i32, pass: i32, prog_bar: i32, sib: i32, cx: &mut Ctx<'_>) -> bool {
        let lib = cx.lib;
        let role = self.feel_for(who, lib).role;
        let nb = prog_bar + 1;
        let mut lanes = [0i32, 1, 2];
        let mut nl = 3;
        if role == 5 {
            lanes[0] = self.melody_lane_for(who, pass, nb / 2, cx);
            lanes[1] = 0;
            nl = 2;
        } else if role != 4 {
            return false;
        }
        let par = self.positive_modulo(nb, 2) * 16;
        for l in 0..nl {
            let lane = at_or_default(&lanes, l);
            if self.lane_hit(who, lane, par, step - sib + self.spb(), pass, cx) && self.grid_at(who, lane, par, 2, lib) >= 4 {
                return true;
            }
        }
        false
    }

    /// `int lickNextEnergy (bool& ending)`
    pub(crate) fn lick_next_energy(&mut self, ending: &mut bool, cx: &mut Ctx<'_>) -> i32 {
        *ending = false;
        let mut arr_pos = -1;
        let en = self.organ_next_energy(&mut arr_pos, cx);
        if en < 0 {
            return -1;
        }
        let Some(song) = cx.song else { return en };
        let n = i32::try_from(song.entries.len()).unwrap_or(i32::MAX);
        for d in 1..=n {
            let e = self.entry_rel(d, cx);
            if e.arr_pos != arr_pos {
                continue;
            }
            let last = i32::try_from(song.sections.len()).unwrap_or(i32::MAX) - 1;
            *ending = usize::try_from(clamp(e.section, 0, last)).ok().and_then(|k| song.sections.get(k)).is_some_and(|s| s.role == 2);
            break;
        }
        en
    }

    /// `int kickoffFor (int who, int step, int pass, int progBar, int sib, float64 stepPpq, int& restFrom)`
    pub(crate) fn kickoff_for(
        &mut self,
        who: i32,
        step: i32,
        pass: i32,
        prog_bar: i32,
        sib: i32,
        step_ppq: f64,
        rest_from: &mut i32,
        cx: &mut Ctx<'_>,
    ) -> i32 {
        let lib = cx.lib;
        let bar_l = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
        let bar_end = self.bar_floor(step_ppq, bar_l) + bar_l;
        if (self.cur_end_ppq - bar_end).abs() > 1.0e-6 || self.jump_next_idx >= 0 {
            return -1; // the section ends with this bar
        }
        *rest_from = self.grid_rest_from(who, step, pass, prog_bar, sib, cx);
        if !self.lick_meter_ok(&bowed::licks::kickoff_set(self.meter, 0)) {
            return -1;
        }
        if *rest_from > self.lick_rest_max() {
            return -1; // rests 2 beats (6/8: a dotted beat) or more
        }
        let mut ending = false;
        let ne = self.lick_next_energy(&mut ending, cx);
        if ne < 0 || ending {
            return -1;
        }
        let chance = self.lick_amount(who, 0, lib) * (if ne > self.energy { 1.5f32 } else { 1.0f32 });
        if self.rand01(pass, prog_bar, 1961, who, cx) >= chance {
            return -1;
        }
        let mut fit = [0i32; 3];
        let mut nf = 0;
        for w in 0..3 {
            let set = bowed::licks::kickoff_set(self.meter, w);
            if self.lick_meter_ok(&set)
                && set.notes.first().is_some_and(|n0| n0.start_beats * 4.0f32 + self.spb() as f32 >= *rest_from as f32 - 1.0e-4f32)
            {
                put_at(&mut fit, nf, w);
                nf += 1;
            }
        }
        if nf == 0 {
            return -1;
        }
        let pick = if is12(self.meter) { self.rand01(pass, prog_bar, 1998, who, cx) } else { self.rand01(pass, prog_bar, 1962, who, cx) };
        at_or_default(&fit, min(nf - 1, self.floor_to_int(f64::from(pick * nf as f32))))
    }

    /// `int lickPitch (const Chord& tc, int degreeOffset)`
    pub(crate) fn lick_pitch(&mut self, tc: &Chord, degree_offset: i32) -> i32 {
        let root = 62 + self.positive_modulo(self.key_root + tc.root_pc - 62, 12);
        root + self.scale_offset_in(self.scale_mode, tc.degree + degree_offset) - self.scale_offset_in(self.scale_mode, tc.degree)
    }

    /// `int lickPartnerInt (const Chord& tc, const bowed::licks::LickNote& n)`
    pub(crate) fn lick_partner_int(&mut self, tc: &Chord, n: &LickNote) -> i32 {
        if !n.double_stop {
            return 0;
        }
        if n.degree_offset == 4 {
            return 7;
        }
        if n.degree_offset < 0 {
            return self.lick_pitch(tc, n.degree_offset - 2) - self.lick_pitch(tc, n.degree_offset); // a turnaround's 3rd below (a 6th inverted)
        }
        let mut x = self.lick_pitch(tc, -1) - self.lick_pitch(tc, n.degree_offset);
        while x > -3 {
            x -= 12;
        }
        if x >= -9 { x } else { 0 }
    }

    /// `int lickPartnerFor (int out, int lo, int hi)`
    pub(crate) fn lick_partner_for(&mut self, out: i32, lo: i32, hi: i32) -> i32 {
        if !self.lick_now.on || self.lick_now.partner_int == 0 {
            return -1;
        }
        let mut p = out + self.lick_now.partner_int;
        if p > hi {
            p -= 12;
        }
        if p < lo {
            p += 12;
        }
        if p >= lo && p <= hi && p != out { p } else { -1 }
    }

    /// `LickMark lickMarkFor (int which, const Chord& tc, const bowed::licks::LickNote& l)`
    pub(crate) fn lick_mark_for(&mut self, which: i32, tc: &Chord, l: &LickNote) -> LickMark {
        // potatoes: the Bow band; a turnaround keeps its register (no fold to the line)
        LickMark {
            on: true,
            land: l.start_beats >= 0.0f32,
            accent: l.accent,
            slide_in: l.slide_in,
            band: if which == 0 || which == K_TURNAROUND_LICK { Band::Bow as i32 } else { -1 },
            partner_int: self.lick_partner_int(tc, l),
            absolute: which == K_TURNAROUND_LICK,
        }
    }

    /// `void bowedFillStep (int who, int step, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn bowed_fill_step(&mut self, who: i32, step: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let fill_licks = self.lick_amount(who, 1, lib);
        if fill_licks <= 0.0f32
            || self.fills <= 0.0f32
            || !self.lick_meter_ok(&bowed::licks::fill_set(self.meter, 0))
            || !self.is_phrase_end(prog_bar)
        {
            return;
        }
        let bar_l = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
        let bar_start = self.bar_floor(step_ppq, bar_l);
        if self.lick_land_at >= 0.0 && (self.lick_land_at - (bar_start + bar_l)).abs() < 1.0e-6 {
            return; // a kick-off has this bar
        }
        if self.lick_fill_at > bar_start + 1.0e-6 {
            self.lick_fill_at = -1.0; // a seek back
            self.lick_fill_prev_at = -1.0;
        }
        let this_bar = self.lick_fill_at >= 0.0 && (self.lick_fill_at - bar_start).abs() < 1.0e-6;
        if self.lick_fill_at >= 0.0 && !this_bar && bar_start - self.lick_fill_at < 4.0 * bar_l - 1.0e-6 {
            return;
        }
        let rest_from = self.grid_rest_from(who, step, pass, prog_bar, sib, cx);
        if rest_from > self.lick_rest_max() || sib < rest_from {
            return; // rests 2 beats (6/8: the dotted beat) or more
        }
        if self.rand01(pass, prog_bar, 1963, who, cx) >= fill_licks * self.fills / 50.0f32 {
            return;
        }
        let nf = bowed::licks::fill_count(self.meter);
        let r = if is12(self.meter) { self.rand01(pass, prog_bar, 1998, who, cx) } else { self.rand01(pass, prog_bar, 1964, who, cx) };
        let which = min(nf - 1, self.floor_to_int(f64::from(r * nf as f32)));
        let set = bowed::licks::fill_set(self.meter, which);
        if !self.lick_meter_ok(&set) {
            return;
        }
        let n = set.count;
        if !this_bar {
            self.lick_fill_prev_at = self.lick_fill_at;
            self.lick_fill_at = bar_start;
        }
        let tc = self.chord_for_bar(prog_bar, cx);
        self.clear_step();
        self.push_now = false; // the rest is the fill's
        for (i, l) in (0i32..).zip(set.notes.iter()) {
            if self.lick_step(l) != sib {
                continue;
            }
            let sing = i == n - 1 && self.grid_holds_next_downbeat(who, step, pass, prog_bar, sib, cx);
            let p = self.lick_pitch(&tc, l.degree_offset) + l.semis;
            self.add_step_note_f(p, if l.accent { 96.0f32 } else { 80.0f32 }, l.len_beats, 1, 0.0f32, 0);
            self.lick_struck(&set);
            self.lick_now = LickMark {
                on: true,
                land: false,
                accent: l.accent,
                slide_in: false,
                band: if sing { Band::Sing as i32 } else { Band::Plain as i32 },
                partner_int: self.lick_partner_int(&tc, l),
                absolute: false,
            };
        }
    }

    /// `bool bowedTurnaroundStep (int who, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn bowed_turnaround_step(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) -> bool {
        let lib = cx.lib;
        let chance = self.lick_amount(who, 2, lib);
        let set = bowed::licks::turnaround_set(self.meter, 0);
        let Some(cur) = self.cur else { return false };
        if chance <= 0.0f32 || self.jump_next_idx >= 0 || !self.lick_meter_ok(&set) {
            return false;
        }
        let bar_l = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
        let bar_end = self.bar_floor(step_ppq, bar_l) + bar_l;
        if (self.cur_end_ppq - bar_end).abs() > 1.0e-6 {
            return false; // the chord goes on past this bar
        }
        let tc = self.chord_for_bar(prog_bar + 1, cx);
        if tc.degree != 0 {
            return false;
        }
        // Once per section: the change into a new section's I wins; else the section's first V -> I.
        if cur.chord_index < cur.section_chords - 1 {
            if self.chord_for_bar(prog_bar, cx).degree != 4 {
                return false;
            }
            if self.chord_for_bar(prog_bar + cur.section_chords - cur.chord_index, cx).degree == 0 {
                return false; // the section's end takes it
            }
            let mut j = prog_bar - cur.chord_index;
            while j + 1 < prog_bar {
                if self.chord_for_bar(j, cx).degree == 4 && self.chord_for_bar(j + 1, cx).degree == 0 {
                    return false; // an earlier V -> I had it
                }
                j += 1;
            }
        }
        if self.rand01(pass, cur.arr_pos, 1965, who, cx) >= chance {
            return false;
        }
        let n = set.count;
        if self.lick_land_at != bar_end || self.lick_land_which != K_TURNAROUND_LICK {
            // decided once for the bar
            self.lick_land_which = K_TURNAROUND_LICK;
            self.lick_land_set_at = step_ppq;
            self.lick_land_at = bar_end;
        }
        self.clear_step();
        self.push_now = false;
        for (i, l) in (0i32..).zip(set.notes.iter()) {
            if i + 1 >= n {
                break;
            }
            if self.lick_step(l) != sib {
                continue;
            }
            let p = self.lick_pitch(&tc, l.degree_offset);
            self.add_step_note_f(p, 80.0f32, l.len_beats, 1, 0.0f32, 0);
            self.lick_struck(&set);
            self.lick_now = self.lick_mark_for(K_TURNAROUND_LICK, &tc, l);
        }
        true
    }

    /// `void bowedLickStep (int who, int step, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn bowed_lick_step(&mut self, who: i32, step: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        self.lick_now = LickMark::default();
        if self.dry_run {
            return;
        }
        let bar_l = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
        if self.lick_land_at >= 0.0 && (step_ppq > self.lick_land_at + bar_l || step_ppq < self.lick_land_at - bar_l - 1.0e-6) {
            self.lick_land_forget(); // long past, or a seek
        }
        if self.lick_land_at >= 0.0 && (step_ppq - self.lick_land_at).abs() < 1.0e-6 {
            let set = if self.lick_land_which == K_TURNAROUND_LICK {
                bowed::licks::turnaround_set(self.meter, 0)
            } else {
                bowed::licks::kickoff_set(self.meter, self.lick_land_which)
            };
            if !self.bowed_licks_on(cx) || !self.lick_meter_ok(&set) {
                return; // the set of this meter (none after a meter change)
            }
            let Some(l) = set.notes.last() else { return };
            let tc = self.chord_for_bar(prog_bar, cx); // the chord it lands on, as it is now
            self.clear_step();
            self.push_now = false;
            let p = self.lick_pitch(&tc, l.degree_offset);
            self.add_step_note_f(p, if l.accent { 96.0f32 } else { 80.0f32 }, l.len_beats, 1, 0.0f32, 0);
            self.lick_struck(&set);
            self.lick_now = self.lick_mark_for(self.lick_land_which, &tc, l);
            return;
        }
        if !self.bowed_licks_on(cx) {
            return;
        }
        if self.bowed_turnaround_step(who, pass, prog_bar, sib, step_ppq, cx) {
            return;
        }
        if !self.pickup_now || prog_bar != self.pickup_prog_bar || self.lick_amount(who, 0, lib) <= 0.0f32 {
            self.bowed_fill_step(who, step, pass, prog_bar, sib, step_ppq, cx);
            return;
        }
        if !self.lick_meter_ok(&bowed::licks::kickoff_set(self.meter, 0)) {
            self.bowed_fill_step(who, step, pass, prog_bar, sib, step_ppq, cx);
            return;
        } // no kick-off for this meter
        let mut rest_from = self.spb();
        let which = self.kickoff_for(who, step, pass, prog_bar, sib, step_ppq, &mut rest_from, cx);
        if which < 0 || sib < rest_from {
            // This step plays no kick-off note: a landing this bar had decided (an earlier pass, before a change) goes.
            let bar_end = self.bar_floor(step_ppq, bar_l) + bar_l;
            if self.lick_land_at >= 0.0 && (self.lick_land_at - bar_end).abs() < 1.0e-6 {
                self.lick_land_forget();
            }
            if which < 0 {
                self.bowed_fill_step(who, step, pass, prog_bar, sib, step_ppq, cx); // never in a kick-off bar
            }
            return;
        }
        let set = bowed::licks::kickoff_set(self.meter, which);
        let tc = self.chord_for_bar(prog_bar + 1, cx);
        self.clear_step();
        self.push_now = false; // the rest (and any fill) is the lick's
        for l in set.notes {
            let mark = self.lick_mark_for(which, &tc, l);
            let vel = if l.accent { 96.0f32 } else { 80.0f32 };
            if l.start_beats >= 0.0f32 {
                self.lick_land_which = which;
                self.lick_land_set_at = step_ppq;
                self.lick_land_at = self.bar_floor(step_ppq, bar_l) + bar_l;
            } else if self.lick_step(l) == sib {
                let p = self.lick_pitch(&tc, l.degree_offset);
                self.add_step_note_f(p, vel, l.len_beats, 1, 0.0f32, 0);
                self.lick_struck(&set);
                self.lick_now = mark;
            }
        }
    }

    /// `void performStep (int who, int step, int pass, int progBar, int sib, float64 stepPpq)`
    /// (ported in Task 11: every step's notes go through it; its other drivers' callees are their lanes' stubs)
    pub(crate) fn perform_step(
        &mut self,
        who: i32,
        step: i32,
        pass: i32,
        prog_bar: i32,
        sib: i32,
        step_ppq: f64,
        cx: &mut Ctx<'_>,
        out: &mut dyn MidiSink,
    ) {
        let lib = cx.lib;
        self.bowed_lick_step(who, step, pass, prog_bar, sib, step_ppq, cx);
        // Tension: chordal players thin to their outer notes in the beat before a release.
        if self.ten_tail && self.pattern != 2 && !self.is_drums() && Self::chordal_instrument(self.pattern) && self.step_count >= 3 {
            let mut lo = 0;
            let mut hi = 0;
            for k in 1..self.step_count {
                if self.step_note.val(k) < self.step_note.val(lo) {
                    lo = k;
                }
                if self.step_note.val(k) > self.step_note.val(hi) {
                    hi = k;
                }
            }
            let mut w = 0;
            for k in 0..self.step_count {
                if k == lo || k == hi {
                    self.step_note.set(w, self.step_note.val(k));
                    self.step_vel.set(w, self.step_vel.val(k));
                    self.step_len.set(w, self.step_len.val(k));
                    self.step_hand.set(w, self.step_hand.val(k));
                    self.step_shift_ms.set(w, self.step_shift_ms.val(k));
                    self.step_flags.set(w, self.step_flags.val(k));
                    w += 1;
                }
            }
            self.step_count = w;
        }
        if self.dry_run {
            // a push looking ahead: keep the notes, play nothing
            self.dry_count = self.step_count;
            for i in 0..self.step_count {
                self.dry_note.set(i, self.step_note.val(i));
                self.dry_vel.set(i, self.step_vel.val(i));
                self.dry_len.set(i, self.step_len.val(i));
                self.dry_hand.set(i, self.step_hand.val(i));
                self.dry_shift.set(i, self.step_shift_ms.val(i));
                self.dry_flags.set(i, self.step_flags.val(i));
            }
            return;
        }
        let pushed = self.push_now;
        self.push_now = false;
        self.ample_final_hit = self.hold_step;
        let final_hold = self.hold_step;
        if self.hold_step {
            for i in 0..self.step_count {
                self.step_len.set(i, max(self.step_len.val(i), self.hold_len as f32 * 0.98f32));
            }
            self.hold_step = false;
        }
        if self.perf.interplay() && self.cur.is_some() {
            self.apply_interplay(who, sib, step_ppq, cx);
        }
        let ample = self.ample_active(lib);
        if ample {
            // A push strums the next chord. A pre-roll (the host playing before bar 1) holds the song's first chord, not
            // the loop's last: Ample latches the last chord pressed, and a DAW recording the pre-roll stacks the two
            // (Bbsus2 over Eb, read as Eb 6/9).
            let bar = self.floor_to_int(step_ppq / self.chord_len_ppq() + 1.0e-6) + (if pushed { 1 } else { 0 });
            self.ample_begin(who, prog_bar + (if pushed { 1 } else { 0 }) + max(0, -bar), step_ppq, cx);
        }
        let evo = self.ev_natural(lib);
        let in_on = self.indiginus_active(lib); // Indiginus (Classic and Natural): its setup keys each pass, its filters
        let brass = self.brass_active(lib); // Blue Street Brass: setup each pass, the step played as a section
        if brass {
            self.br_n = 0;
            self.brass_setup(pass, who, lib);
        }
        if evo {
            self.ev_begin(step_ppq, lib);
        }
        if in_on {
            self.in_setup(pass, lib);
        }
        let slots_mode = in_on && self.in_slots_on(lib); // the library's own chord keys: no notes of ours
        if evo && !slots_mode && self.ev_tremolo_section_step(who, pass, prog_bar, sib, step_ppq, cx) {
            if in_on {
                self.in_expression(prog_bar, step_ppq, lib);
            }
            return;
        }
        if self.perf.natural && self.pushed_at >= 0.0 && (step_ppq - self.pushed_at).abs() < 1.0e-6 {
            // already struck, ringing
            if !self.push_verify || self.same_chord(self.chord_for_bar(prog_bar, cx), self.pushed_chord) {
                self.step_count = 0; // ... unless the chord changed since
            }
            self.pushed_at = -1.0;
            self.push_verify = false;
        } else if self.perf.natural && self.pushed_at >= 0.0 && step_ppq > self.pushed_from + 1.0e-6 && step_ppq < self.pushed_at - 1.0e-6 {
            self.step_count = 0; // the anticipation rings
        }
        if self.step_count == 0 {
            if evo && in_on {
                self.in_expression(prog_bar, step_ppq, lib);
            }
            return;
        }
        let natural = self.perf.natural;
        if pushed {
            self.pushed_from = step_ppq;
            self.pushed_at = self.chord_end_for(step_ppq);
            self.push_ring_to = self.pushed_at + max(0.5, f64::from(self.tick_ppq(lib)) * 2.0);
            self.pushed_chord = self.push_now_chord;
            self.push_verify = false;
        }
        let groove_ms = if natural { self.groove_ms_for(who, step_ppq, cx) } else { 0.0f32 };

        let h = self.humanize / 50.0f32;
        let dyn_ = self.dynamics / 50.0f32;
        let t = (prog_bar * self.spb() + sib) as f32 / (self.num_bars() * self.spb()) as f32;
        // Dynamics together: one moderate arc for the whole band (+-20%, swelling into the section's end) instead of each player's own.
        let arc_mult = if self.perf.dynamics_together && self.cur.is_some() {
            1.0f32 + 0.2f32 * min(dyn_, 1.0f32) * (t - 0.5f32) * 2.0f32
        } else {
            1.0f32 + self.feel_for(who, lib).arc * dyn_ * (t - 0.5f32) * 2.0f32
        };
        let band_accent = if self.perf.dynamics_together
            && self.cur.is_some_and(|c| c.chord_index == 0)
            && step_ppq >= self.cur_start_ppq - 1.0e-9
            && step_ppq < self.cur_start_ppq + f64::from(self.tick_ppq(lib)) - 1.0e-9
        {
            6.0f32
        } else {
            0.0f32
        };
        let accent = 1.0f32 + (self.accent_at(who, self.positive_modulo(sib, self.spb()), lib) - 1.0f32) * dyn_;
        let swing = self.swing_delay(sib, who, lib);
        let drums = self.is_drums();
        let role = self.feel_for(who, lib).role;

        if self.fiddle_active(lib) && self.perf.natural && !self.live_mode {
            self.fiddle_perform(who, step, pass, prog_bar, sib, step_ppq, swing, accent * arc_mult, cx, out);
            return;
        }
        if self.swam_active(lib) {
            self.swam_perform(who, step, pass, prog_bar, sib, step_ppq, swing, accent * arc_mult, cx);
            return;
        }

        let mut crow_step = 0;
        if self.ks_active(lib) {
            self.ks_setup(pass, lib);
        }
        if self.section_ks(lib) {
            // One articulation per step: pluck if any note is plucked, short if nothing is held, else long.
            let mut any_pizz = false;
            let mut longest_all = 0.0f32;
            for i in 0..self.step_count {
                if (self.step_flags.val(i) & 32) != 0 {
                    any_pizz = true;
                }
                longest_all = max(longest_all, self.step_len.val(i));
            }
            let mut any_trem = false;
            for i in 0..self.step_count {
                if (self.step_flags.val(i) & 64) != 0 {
                    any_trem = true;
                }
            }
            crow_step = if any_pizz {
                32
            } else if any_trem && self.section_tremolo(lib) {
                64
            } else if longest_all < self.tick_ppq(lib) * 1.2f32 {
                1024
            } else {
                0
            };

            // CC1 follows the loudest note of the step (accents and the phrase arc already shaped it);
            // CC11 breathes slowly: a little lift into strong beats and over long notes.
            let mut loud = 0.0f32;
            let mut longest = 0.0f32;
            for i in 0..self.step_count {
                loud = max(loud, self.step_vel.val(i));
                longest = max(longest, self.step_len.val(i));
            }
            let lv = loud * self.velocity as f32 / 96.0f32 * accent * arc_mult;
            let dyn_level = clamp((18.0f32 + lv * 0.78f32 + (self.energy - 3) as f32 * 6.0f32) as i32, 8, 127);
            let on_ppq = step_ppq + swing;
            self.crow_cc(1, dyn_level, on_ppq - self.section_lead_ms(1, lib) * self.ppq_per_ms(), lib); // only when the target has a dynamics CC (Crow Hill)
            let expr_base = clamp(
                (96.0f32 + 20.0f32 * (arc_mult - 1.0f32) * 4.0f32 + (if crate::meter::strong_step(self.meter, sib) { 8.0f32 } else { 0.0f32 }))
                    as i32,
                60,
                127,
            );
            self.crow_cc(11, expr_base, on_ppq - self.section_lead_ms(11, lib) * self.ppq_per_ms(), lib);
            if longest >= 3.0f32 * self.tick_ppq(lib) {
                self.crow_cc(11, min(127, expr_base + 14), on_ppq + f64::from(longest) * 0.55, lib);
                self.crow_cc(11, max(60, expr_base - 6), on_ppq + f64::from(longest) * 0.92, lib);
            }
        }

        // Natural on Pianoteq (spec §13): staccato steps get the harmonic pedal; the Ending's keys come up early
        // enough for the sustain pedal to lift slowly (6 steps over 400 ms from +150 ms) before the song stops.
        let pt_nat = natural && self.continuous_pedal(lib);
        let written_scale = 1.0f32 / max(0.1f32, self.length);
        let mut harmonic = pt_nat
            && !pushed
            && !final_hold
            && self.current_target(lib).harmonic_pedal_cc >= 0
            && self.energy >= 3
            && self.feel_for(who, lib).pedal_mode != 0;
        let mut i = 0;
        while harmonic && i < self.step_count {
            if (self.step_flags.val(i) & 8192) != 0 || self.step_len.val(i) * written_scale > self.tick_ppq(lib) * 4.0f32 * 0.25f32 + 1.0e-4f32 {
                harmonic = false;
            }
            i += 1;
        }
        let mut h_first_on = 1.0e18f64;
        let mut h_last_off = -1.0f64;
        let end_keys_off = if pt_nat && final_hold && self.pedal_level > 0 {
            step_ppq + max(self.hold_len * 0.5, self.hold_len * 0.98 - 560.0 * self.ppq_per_ms())
        } else {
            -1.0
        };
        let organ = self.organ_active(lib);
        self.og_loud = 0.0f32;
        self.og_first_on = 1.0e18;
        self.og_folded_n = 0;
        // Hammond: a section start whose registration wasn't sent ahead (song start, Live, a jump) waits 30 ms for it;
        // otherwise notes keep their time, kept only off "now" by 5.5 ms so the swell can still go 5 ms ahead of them
        // (it only bites on a note pulled ~35 ms early by the groove, generated as it is ~40 ms ahead).
        // Vocabulary 2.5 (Natural): the left hand on the lower manual on a share (lowerHand) of bars; its registration
        // key waits 30 ms ahead of it like the upper's.
        let og_low_ch = if organ && natural { self.organ_lower_channel(lib) } else { 0 };
        let og_lh = if og_low_ch > 0 { self.feel_for(who, lib).organ.lower_hand } else { 0.0f32 };
        let mut og_left = og_lh > 0.0f32 && (og_lh >= 1.0f32 || self.rand01(pass, prog_bar, 1971, who, cx) < og_lh);
        if og_left {
            og_left = false;
            for i in 0..self.step_count {
                if self.step_hand.val(i) == 0 {
                    og_left = true;
                }
            }
        }
        self.og_low_first_on = 1.0e18;
        self.og_low_folded_n = 0;
        let og_lead = (if organ
            && ((self.organ_sec_key(pass) != self.og_sec_key && clamp(self.energy, 1, 4) != self.og_reg_sent)
                || (og_left && clamp(self.energy, 1, 4) != self.og_lower_reg_sent))
        {
            30.5
        } else {
            5.5
        }) * self.ppq_per_ms();
        let pt_bar_len = f64::from(self.get_bar_length_ppq());
        let pt_bar_end = ((step_ppq / pt_bar_len + 1.0e-9).floor() + 1.0) * pt_bar_len;

        // Voice balance for personas: top right-hand note sings, inner voices sit back.
        let mut top_rh = -1;
        let mut rh_count = 0;
        for i in 0..self.step_count {
            if self.step_hand.val(i) == 1 {
                rh_count += 1;
                if top_rh < 0 || self.step_note.val(i) > self.step_note.val(top_rh) {
                    top_rh = i;
                }
            }
        }

        // slots mode: this step's notes, as one strum
        let mut sl_loud = 0.0f32;
        let mut sl_len = 0.0f32;
        let mut sl_on = 1.0e18f64;
        let mut sl_n = 0;
        // Evolution, Classic / Live: this step's notes as folded
        let mut ev_fold: CArr<i32, 16> = CArr::default();
        let mut ev_fold_n = 0;
        for i in 0..self.step_count {
            let note = self.step_note.val(i);
            let mut rank = 0;
            for j in 0..self.step_count {
                if self.step_hand.val(j) == self.step_hand.val(i) && self.step_note.val(j) < note {
                    rank += 1;
                }
            }

            let fw = self.feel_for(who, lib);
            let mut ms = fw.timing_ms + self.step_shift_ms.val(i) + self.step_shift.pocket_ms; // + the song's Pocket (Natural; 0 otherwise)
            let mut jitter = fw.jitter_ms;
            if drums && who == 0 {
                jitter = 4.0f32;
            }
            if natural {
                ms += self.rand_bell(step, pass, 11, note, cx) * jitter * h * 0.5f32; // Natural: the groove carries the feel
            } else {
                ms += self.rand_bell(step, pass, 11, note, cx) * jitter * h;
            }
            ms += at_or_default(&fw.lane_ms, clamp(self.step_hand.val(i), 0, 3)) * h;
            if !drums && role != 3 && (who > 0 || self.is_chordal(self.pattern)) {
                ms += fw.roll_ms * rank as f32 * 0.5f32 * h;
            }
            if natural {
                ms = clamp(ms + groove_ms, -40.0f32, 90.0f32); // never earlier than the step is made (~40 ms ahead)
            } else {
                ms = clamp(ms, -40.0f32, 60.0f32);
            }

            let mut v = self.step_vel.val(i) * self.velocity as f32 / 96.0f32;
            v = v * accent * arc_mult + band_accent;
            if role == 0 || role == 4 {
                if self.step_hand.val(i) == 0 {
                    v *= 0.92f32;
                } else if i == top_rh {
                    v += 8.0f32;
                } else if rh_count >= 3 {
                    v -= 6.0f32;
                }
            }
            let mut spread = fw.vel_var;
            if drums && who == 0 {
                spread = if note == 42 || note == 44 || note == 46 || note == 51 { 12.0f32 } else { 5.0f32 };
            }
            if role == 2 && self.step_hand.val(i) == 2 {
                spread *= 1.5f32;
            }
            v += self.rand_bell(step, pass, 23, note, cx) * spread * h;

            if self.ten_now > 0.0f32 {
                v += 6.0f32 * min(1.0f32, self.ten_now / 0.8f32) * self.ten_ramp; // tension: swell into the release
            }
            if self.rel_down {
                v += 8.0f32; // ... and land it
            }
            if drums && self.meter == Meter::M34 && (35..=40).contains(&note) {
                // 3/4 waltz kit: beats 2 and 3 never go over the downbeat kick/snare (no backbeat accent, spec 6) -- any kick,
                // snare, side stick or clap (GM 35..40; a target's own keys are mapped later). A bar without a downbeat hit
                // keeps the last one heard. The bar as heard (at Half / Double speed the pattern's step isn't the bar's).
                let hs = self.sixteenth_of(step_ppq);
                let bar = self.floor_to_int((step_ppq - self.bar_origin + 1.0e-6) / f64::from(self.get_bar_length_ppq()));
                if hs == 0 {
                    if bar != self.dr_down_bar {
                        self.dr_down_bar = bar;
                        self.dr_down_vel = v;
                    } else {
                        self.dr_down_vel = max(self.dr_down_vel, v);
                    }
                } else if crate::meter::is_light_beat(self.meter, hs) && self.dr_down_vel > 0.0f32 {
                    v = min(v, self.dr_down_vel);
                }
            }
            let mut len = self.step_len.val(i) * (1.0f32 + (self.rand01(step, pass, 37, note, cx) - 0.5f32) * 0.2f32 * h);
            if pushed {
                len = max(len, (self.push_ring_to - step_ppq) as f32); // rings through the downbeat it anticipated
            }
            if is12(self.meter) && !pushed {
                // 12-step meters: a note written to end in its bar never overruns it (Half speed's doubled holds and humanize's
                // stretch don't carry it over); a written tie across the bar line survives. Written = the grid length in 16ths,
                // before the Half / Double speed multiplier.
                let bl = f64::from(self.get_bar_length_ppq());
                let to_end = ((step_ppq / bl + 1.0e-9).floor() + 1.0) * bl - step_ppq;
                if f64::from(self.step_len.val(i)) * 0.25 / f64::from(self.tick_ppq(lib)) <= to_end + 1.0e-3 {
                    len = min(len, to_end as f32);
                }
            }
            let mut on_ppq = step_ppq + swing + f64::from(ms) * self.ppq_per_ms();
            if organ {
                // Hammond: never before the swell (5 ms) / a section's registration (30 ms) can go ahead; a note held to the
                // chord change overlaps the next by 10 ms.
                on_ppq = max(on_ppq, self.current_ppq + og_lead);
                let ch_end = if self.cur.is_some() { self.cur_end_ppq } else { self.chord_end_for(step_ppq) };
                if !final_hold && !pushed && step_ppq < ch_end && step_ppq + f64::from(self.step_len.val(i) * written_scale) >= ch_end - 1.0e-3 {
                    len = max(len, (ch_end + 10.0 * self.ppq_per_ms() - on_ppq) as f32);
                }
            }
            if brass {
                self.br_add(note, v, on_ppq, max(0.02f32, len), self.step_len.val(i) * written_scale, self.step_hand.val(i));
                continue;
            }
            if ample {
                self.ample_add(note, v, on_ppq, max(0.02f32, len), self.step_flags.val(i), self.step_hand.val(i));
                continue;
            }
            if in_on {
                // Indiginus: after its setup keys (30 ms), room for CC11 5 ms ahead; velocity 50-110 (below 40 is its Mutes zone).
                on_ppq = max(on_ppq, max(self.in_ready_at, self.current_ppq + 6.0 * self.ppq_per_ms()));
                v = clamp(v, 50.0f32, 110.0f32);
            }
            if slots_mode {
                sl_n += 1;
                sl_loud = max(sl_loud, v);
                sl_len = max(sl_len, len);
                sl_on = min(sl_on, on_ppq);
                continue;
            }
            if evo {
                self.ev_add(note, v, on_ppq, max(0.02f32, len), self.step_flags.val(i), self.step_hand.val(i));
                continue;
            }
            let dr_flags = if drums { self.step_flags.val(i) & K_DRUM_FLAGS } else { 0 };
            let mut out_note = if drums && self.driver_is(Driver::DrumMap, lib) && self.target_plays(self.pattern, lib) {
                self.map_drum(note, v as i32, if (dr_flags & K_DRUM_RIGHT_FLAG) != 0 { 1 } else { step }, lib)
            } else {
                note
            };
            if self.bass_target(lib) {
                let kb = self.current_target(lib);
                out_note += kb.kb_pitch_shift;
                while out_note < kb.kb_min_note {
                    out_note += 12;
                }
            } // MODO BASS 2 reads written pitch: low E = 40
            if self.bass_target(lib) && self.expressive() {
                len = max(0.02f32, len);
                // C++ passes stepFlags.at (i) by reference: copied in, written back below. Task 15: natural_bass_note must
                // change only `flags`, never self.step_flags[i] itself (the write-back would undo it).
                let mut fl = self.step_flags.val(i);
                self.natural_bass_note(&mut fl, on_ppq, &mut len, out_note, lib);
                self.step_flags.set(i, fl);
            }
            if self.plucked_active(lib) {
                // Evolution / Indiginus: below the range are its keys; two notes folded onto one: play it once.
                let et = self.current_target(lib);
                while out_note < et.ev_low {
                    out_note += 12;
                }
                while out_note > et.ev_high {
                    out_note -= 12;
                }
                let mut dup = false;
                for j in 0..ev_fold_n {
                    if ev_fold.val(j) == out_note {
                        dup = true;
                    }
                }
                if dup {
                    self.q.schedule_flags = 0;
                    self.q.schedule_off_vel = -1;
                    continue;
                }
                if ev_fold_n < 16 {
                    ev_fold.set(ev_fold_n, out_note);
                    ev_fold_n += 1;
                }
            }
            if (dr_flags & K_FLAM_FLAG) != 0 && v >= 60.0f32 {
                // no grace under a soft hit
                // A flam: the left-hand grace 22-28 ms before (never before now: the main stroke waits for it).
                let grace_ms = 22.0f32 + 6.0f32 * self.rand01(step, pass, 1943, note, cx);
                on_ppq = max(on_ppq, self.current_ppq + f64::from(grace_ms + 0.5f32) * self.ppq_per_ms());
                self.q.schedule_flags = 0;
                self.schedule(0, 38, clamp(v as i32 - 30, 1, 127), on_ppq - f64::from(grace_ms) * self.ppq_per_ms(), max(0.02f32, len));
            }
            if (dr_flags & K_APERTURE_FLAG) != 0 && self.current_target(lib).dr_aperture_cc >= 0 {
                // Continuous aperture: the hat's opening (sloshier with the player's amount) 5 ms before the CC key.
                let ap = (30.0f32 + 25.0f32 * self.feel_for(who, lib).drums.aperture + (self.rand01(step, pass, 1949, note, cx) - 0.5f32) * 12.0f32)
                    .round() as i32;
                on_ppq = max(on_ppq, self.current_ppq + 5.5 * self.ppq_per_ms());
                self.q.schedule_flags = 0;
                self.schedule(1, self.current_target(lib).dr_aperture_cc, clamp(ap, 0, 127), on_ppq - 5.0 * self.ppq_per_ms(), 0.0f32);
            }
            self.q.schedule_flags = if self.section_ks(lib) {
                (self.step_flags.val(i) & !(32 | 64 | 1024)) | crow_step | 4096
            } else {
                self.step_flags.val(i) & !K_DRUM_FLAGS
            };
            // Pocket Strings: Pluck at high velocity is a Bartók snap — keep it for accents at energy 4 only.
            if self.section_ks(lib) && crow_step == 32 && self.energy < self.section_cap_below(lib) {
                v = min(v, self.section_cap_vel(lib) as f32);
            }
            if self.ks_active(lib) {
                let flags = self.q.schedule_flags;
                if !self.ks_note(&mut out_note, &mut v, on_ppq, max(0.02f32, len), flags, lib) {
                    self.q.schedule_flags = 0;
                    self.q.schedule_off_vel = -1;
                    continue;
                }
            }
            if pt_nat && (self.step_flags.val(i) & 8192) == 0 {
                if end_keys_off >= 0.0 {
                    len = max(0.02f32, (end_keys_off - on_ppq) as f32);
                }
                let written = self.step_len.val(i) * written_scale;
                let phrase_end = self.is_phrase_end(prog_bar) && step_ppq + f64::from(written) >= pt_bar_end - f64::from(self.tick_ppq(lib)) * 0.5;
                self.q.schedule_off_vel = self.piano_off_vel(written, phrase_end, final_hold, out_note, on_ppq + f64::from(max(0.02f32, len)), lib);
                if harmonic {
                    h_first_on = min(h_first_on, on_ppq);
                    h_last_off = max(h_last_off, on_ppq + f64::from(max(0.02f32, len)));
                }
            }
            let og_low_note = og_left && self.step_hand.val(i) == 0;
            if organ && og_low_note {
                while out_note < 48 {
                    out_note += 12; // the lower manual: its preset keys are 36-47
                }
                while out_note > 96 {
                    out_note -= 12;
                }
                let mut dup = false;
                for j in 0..self.og_low_folded_n {
                    if self.og_low_folded.val(j) == out_note {
                        dup = true;
                    }
                }
                if dup {
                    self.q.schedule_flags = 0;
                    self.q.schedule_off_vel = -1;
                    continue;
                }
                self.og_low_folded.set(self.og_low_folded_n, out_note);
                self.og_low_folded_n = min(self.og_low_folded_n + 1, 15);
                self.og_loud = max(self.og_loud, v);
                self.og_first_on = min(self.og_first_on, on_ppq);
                self.og_low_first_on = min(self.og_low_first_on, on_ppq);
            } else if organ {
                while out_note < 36 {
                    out_note += 12; // the upper manual C2-C7: below it are the preset keys
                }
                while out_note > 96 {
                    out_note -= 12;
                }
                let mut dup = false; // two voices folded onto one key: play it once
                for j in 0..self.og_folded_n {
                    if self.og_folded.val(j) == out_note {
                        dup = true;
                    }
                }
                if dup {
                    self.q.schedule_flags = 0;
                    self.q.schedule_off_vel = -1;
                    continue;
                }
                self.og_folded.set(self.og_folded_n, out_note);
                self.og_folded_n = min(self.og_folded_n + 1, 15);
                self.og_loud = max(self.og_loud, v);
                self.og_first_on = min(self.og_first_on, on_ppq);
            }
            if organ && og_low_note {
                self.q.schedule_ch = og_low_ch;
                self.og_lower_ch_used = og_low_ch - 1;
            }
            let vel = if organ { self.current_target(lib).og_note_vel } else { clamp(v as i32, 1, 127) };
            self.schedule(0, out_note, vel, on_ppq, max(0.02f32, len));
            self.q.schedule_flags = 0;
            self.q.schedule_off_vel = -1;
            self.q.schedule_ch = 0;
        }
        if slots_mode && sl_n >= 3 {
            self.in_slots_strum(prog_bar + (if pushed { 1 } else { 0 }), step_ppq, sl_on, sl_loud, sl_len, cx); // a strum; melody steps play nothing
        }
        if organ && self.og_low_first_on < 1.0e17 {
            self.organ_lower_step(og_low_ch, lib);
        }
        if organ && self.og_first_on < 1.0e17 {
            self.organ_step(who, pass, prog_bar, step_ppq, cx, out);
        }
        if brass {
            self.brass_perform(who, pass, prog_bar, sib, step_ppq, cx);
        }
        if harmonic && h_last_off >= 0.0 {
            self.harmonic_step(min(step_ppq, h_first_on) - 10.0 * self.ppq_per_ms(), h_last_off + 20.0 * self.ppq_per_ms(), cx, out);
        }
        if end_keys_off >= 0.0 {
            let ms = self.ppq_per_ms();
            for k in 0..6 {
                self.schedule(1, 64, self.pedal_level * (5 - k) / 6, end_keys_off + (150.0 + 80.0 * f64::from(k)) * ms, 0.0f32);
            }
            self.pedal_level = 0;
        }
        if ample && crate::library::ample_strums(self.feel_for(who, lib), self.ample_mode, self.energy) {
            self.ample_strum(who, pass, prog_bar, sib, step_ppq, cx);
        } else if ample {
            self.ample_pick(who, pass, prog_bar, sib, step_ppq, cx);
        }
        if evo {
            self.evolution_perform(who, pass, prog_bar, sib, step_ppq, cx);
        }
        if evo && in_on {
            self.in_expression(prog_bar, step_ppq, lib);
        }
    }

    // ---- SWAM / Embertone Joshua Bell / The Fiddle: voices ---------------------

    /// `int swamChoose (int who)`
    pub(crate) fn swam_choose(&mut self, who: i32, lib: &LibraryData) -> i32 {
        let role = self.feel_for(who, lib).role;
        let mut voice = self.swam_voice;
        if voice == 0 {
            #[allow(clippy::if_same_then_else)] // the C++ shape: both arms are 1
            let auto = if role == 5 || role == 1 { 1 } else { 1 }; // Auto: top voice / the melody
            voice = auto;
        }

        // Candidates: melody/right-hand notes for Top/2nd/3rd, bass-hand notes for Bass.
        let mut chosen = -1;
        if voice == 4 {
            for i in 0..self.step_count {
                if self.step_hand.val(i) == 0 && (chosen < 0 || self.step_note.val(i) < self.step_note.val(chosen)) {
                    chosen = i;
                }
            }
            if chosen < 0 {
                for i in 0..self.step_count {
                    if chosen < 0 || self.step_note.val(i) < self.step_note.val(chosen) {
                        chosen = i;
                    }
                }
            }
        } else {
            let want = voice; // 1 = top, 2 = second from top, 3 = third
            let mut used: CArr<bool, 16> = CArr::default();
            for _ in 0..want {
                let mut best = -1;
                for i in 0..self.step_count {
                    if !used.val(i) && self.step_hand.val(i) == 1 && (best < 0 || self.step_note.val(i) > self.step_note.val(best)) {
                        best = i;
                    }
                }
                if best < 0 {
                    break;
                }
                used.set(best, true);
                chosen = best;
            }
            if chosen < 0 {
                for i in 0..self.step_count {
                    if chosen < 0 || self.step_note.val(i) > self.step_note.val(chosen) {
                        chosen = i;
                    }
                }
            }
        }
        chosen
    }

    /// `void swamPerform (int who, int step, int pass, int progBar, int sib, float64 stepPpq, float64 swing, float dynMult)`
    pub(crate) fn swam_perform(
        &mut self,
        who: i32,
        step: i32,
        pass: i32,
        prog_bar: i32,
        sib: i32,
        step_ppq: f64,
        swing: f64,
        dyn_mult: f32,
        cx: &mut Ctx<'_>,
    ) {
        let lib = cx.lib;
        let role = self.feel_for(who, lib).role;
        let chosen = self.swam_choose(who, lib);
        if chosen < 0 {
            return;
        }
        if self.current_target(lib).swam_velocity_dynamics && self.perf.natural {
            self.jb_perform(who, chosen, step, pass, prog_bar, sib, step_ppq, swing, dyn_mult, cx);
            return;
        }

        let flags = self.step_flags.val(chosen);
        let h = self.humanize / 50.0f32;
        let mut ms = self.feel_for(who, lib).timing_ms + self.step_shift_ms.val(chosen) + self.step_shift.pocket_ms; // + the song's Pocket
        if self.perf.natural {
            ms = clamp(
                ms + self.rand_bell(step, pass, 11, self.step_note.val(chosen), cx) * self.feel_for(who, lib).jitter_ms * h * 0.5f32
                    + self.groove_ms_for(who, step_ppq, cx),
                -40.0f32,
                90.0f32,
            );
        } else {
            ms =
                clamp(ms + self.rand_bell(step, pass, 11, self.step_note.val(chosen), cx) * self.feel_for(who, lib).jitter_ms * h, -40.0f32, 60.0f32);
        }
        let mut on_ppq = step_ppq + swing + f64::from(ms) * self.ppq_per_ms();
        let mut len = self.step_len.val(chosen);
        let staccato = (flags & 1024) != 0 || len < self.tick_ppq(lib) * 1.2f32;
        if staccato {
            len = min(len, self.tick_ppq(lib) * 0.8f32);
        }
        // Bowed / blown notes are held past their written end: the next note hands off legato and cuts them.
        else if (flags & 32) == 0 {
            len = len / max(0.1f32, self.length) + self.tick_ppq(lib) * 0.5f32;
        }
        let mut off_ppq = on_ppq + f64::from(max(0.05f32, len));
        let ms_ppq = self.ppq_per_ms();

        // Breath: winds and brass rest for 180 ms after ~7 seconds of continuous playing.
        if self.swam_wind(lib)
            && self.sounding_since >= 0.0
            && (on_ppq - self.sounding_since) > 7000.0 * ms_ppq
            && (on_ppq - self.last_note_end) < 250.0 * ms_ppq
        {
            on_ppq += 180.0 * ms_ppq;
            if off_ppq - on_ppq < 0.1 {
                off_ppq = on_ppq + 0.1;
            }
            self.swam_expr(0, on_ppq - 150.0 * ms_ppq, lib);
            self.sounding_since = -1.0;
        }

        // Expression target from the shaped velocity; velocity itself only shapes the attack.
        let mut v = self.step_vel.val(chosen) * self.velocity as f32 / 96.0f32 * dyn_mult;
        v += self.rand_bell(step, pass, 23, self.step_note.val(chosen), cx) * self.feel_for(who, lib).vel_var * h;
        let expr_target = clamp((34.0f32 + v * 0.72f32) as i32, 20, 127);
        let mut attack_vel = clamp((40.0f32 + v * 0.5f32) as i32, 20, 120);
        // A library whose dynamics follow velocity gets the shaped velocity itself, kept inside the range
        // its legato reads as a normal note (Embertone: outside 18-118 means portamento).
        if self.current_target(lib).swam_velocity_dynamics {
            attack_vel = clamp(v as i32, self.current_target(lib).swam_vel_lo, self.current_target(lib).swam_vel_hi);
        }
        let attack_ms = if staccato {
            25.0f32
        } else if (flags & 32) != 0 {
            10.0f32
        } else {
            70.0f32 + 60.0f32 * (1.0f32 - expr_target as f32 / 127.0f32)
        };

        let legato = self.swam_note >= 0 && (on_ppq - self.swam_note_off) < 90.0 * ms_ppq && !staccato && (flags & 32) == 0;
        let from = if legato {
            max(self.expr_level, 30)
        } else if self.expr_level > 0 {
            self.expr_level
        } else {
            18
        };
        if !legato && self.expr_level != 18 {
            self.swam_expr(18, on_ppq - 25.0 * ms_ppq, lib);
        }
        self.swam_expr((from + expr_target) / 2, on_ppq, lib);
        self.swam_expr(expr_target, on_ppq + f64::from(attack_ms) * ms_ppq, lib);
        if off_ppq - on_ppq > 2.0 * f64::from(self.tick_ppq(lib)) && !staccato {
            // A gentle swell over the middle of long notes, then ease down before the release.
            self.swam_expr(min(127, expr_target + 10), on_ppq + (off_ppq - on_ppq) * 0.6, lib);
            self.swam_expr(max(20, expr_target - 14), off_ppq - 40.0 * ms_ppq, lib);
        }
        self.swam_expr(max(16, expr_target * 2 / 5), off_ppq + 10.0 * ms_ppq, lib);

        // Vibrato: delayed onset on held notes, none on short ones.
        if !staccato && off_ppq - on_ppq > 1.5 * f64::from(self.tick_ppq(lib)) {
            let depth = clamp(18 + self.energy * 12 + (if role == 5 { 12 } else { 0 }), 0, 100);
            self.swam_vib(0, on_ppq, lib);
            self.swam_vib(depth / 2, on_ppq + 260.0 * ms_ppq, lib);
            self.swam_vib(depth, on_ppq + 520.0 * ms_ppq, lib);
            self.swam_vib(0, off_ppq, lib);
        } else {
            self.swam_vib(0, on_ppq, lib);
        }

        let out_note = self.swam_range(self.step_note.val(chosen), lib);
        if self.swam_note >= 0 && self.swam_note_off > on_ppq - 90.0 * ms_ppq {
            // Mono: the previous note always ends. Legato releases it 20 ms after the new one starts,
            // a detached note cuts it 5 ms before. (A repeated pitch is re-attacked by emitNote itself.)
            if out_note != self.swam_note {
                self.schedule(3, self.swam_note, 0, if legato { on_ppq + 20.0 * ms_ppq } else { on_ppq - 5.0 * ms_ppq }, 0.0f32);
            }
            for i in 0..64 {
                if self.active_slots.val(i) && self.active_notes.val(i) == self.swam_note && out_note != self.swam_note {
                    self.active_slots.set(i, false);
                }
            }
        }

        self.q.schedule_flags = flags | 2048 | (if self.current_target(lib).swam_custom_arts && staccato { 1024 } else { 0 }); // 2048 = SWAM articulation hook at fire time
        self.schedule(0, out_note, attack_vel, on_ppq, (off_ppq - on_ppq) as f32);
        self.q.schedule_flags = 0;
        self.swam_note = out_note;
        self.swam_note_off = off_ppq;
        self.last_note_end = off_ppq;
        if self.sounding_since < 0.0 {
            self.sounding_since = on_ppq;
        }
    }

    /// `bool jbDropQueuedCC (int cc, float64 from, int onlyValue)`
    pub(crate) fn jb_drop_queued_cc(&mut self, cc: i32, from: f64, only_value: i32) -> bool {
        let mut any = false;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 1 && q.d1 == cc && q.on_ppq >= from - 1.0e-9 && (only_value < 0 || q.d2 == only_value) {
                self.q.queue.at_mut(i).used = false;
                any = true;
            }
        }
        any
    }

    /// `void jbReset ()`
    pub(crate) fn jb_reset(&mut self, lib: &LibraryData) {
        if self.driver_is(Driver::Swam, lib) {
            self.jb_drop_queued_cc(64, -1.0e18, -1); // a queued run sustain never fires after
        }
        self.lick_forget(); // (swamReset comes here too)
        self.jb_sustain_on = false;
        self.jb_run_end = -1.0;
        self.jb_port_bar = -1000;
        self.jb_prev_end = false;
        self.jb_prev_fast = false;
        self.jb_partner = -1;
        self.jb_prev_on = -1.0;
        self.jb_lens_n = 0;
        self.jb_sustain_sent = false;
        self.jb_subs = 0;
        self.jb_hist_n = 0;
        self.jb_rel_floor = -1.0e18;
        self.jb_stray_down = false;
    }

    /// `void jbPerform (int who, int chosen, int step, int pass, int progBar, int sib, float64 stepPpq, float64 swing, float dynMult)`
    pub(crate) fn jb_perform(
        &mut self,
        who: i32,
        chosen: i32,
        step: i32,
        pass: i32,
        prog_bar: i32,
        sib: i32,
        step_ppq: f64,
        swing: f64,
        dyn_mult: f32,
        cx: &mut Ctx<'_>,
    ) {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let vs = &self.feel_for(who, lib).violin;
        let ms_ppq = self.ppq_per_ms();
        let flags = self.step_flags.val(chosen);
        let h = self.humanize / 50.0f32;
        let mut ms = self.feel_for(who, lib).timing_ms + self.step_shift_ms.val(chosen) + self.step_shift.pocket_ms; // + the song's Pocket
        ms = clamp(
            ms + self.rand_bell(step, pass, 11, self.step_note.val(chosen), cx) * self.feel_for(who, lib).jitter_ms * h * 0.5f32
                + self.groove_ms_for(who, step_ppq, cx),
            -34.0f32,
            90.0f32,
        ); // room for CC1 5 ms before
        if self.lick_now.land {
            ms = 0.0f32; // a kick-off lands on the downbeat
        }
        let on_ppq = step_ppq + swing + f64::from(ms) * ms_ppq;
        let mut len = max(0.05f32, self.step_len.val(chosen)); // the step's length already carries the Length knob
        let staccato = (flags & 1024) != 0; // written staccato only: short notes stay connected (runs)
        if staccato {
            len = min(len, self.tick_ppq(lib) * 0.8f32);
        }
        // The phrase's last note: a phrase-end bar, and the note's written end reaches the bar's end. It ends 30 ms early.
        let bar_len = f64::from(self.get_bar_length_ppq());
        let bar_start = self.bar_floor(step_ppq, bar_len);
        let phrase_end = self.is_phrase_end(prog_bar)
            && step_ppq + f64::from(len / max(0.1f32, self.length)) >= bar_start + bar_len - f64::from(self.tick_ppq(lib)) * 0.5;
        if phrase_end && !staccato {
            len = max(0.05f32, len - (30.0 * ms_ppq) as f32);
        }
        let off_ppq = on_ppq + f64::from(len);
        let len_beats = len; // ppq are quarter notes
        let fast = len < (150.0 * ms_ppq) as f32;
        let accented = self.lick_now.accent || self.accent_at(who, self.positive_modulo(sib, self.spb()), lib) > 1.0f32;
        let out_note = self.swam_range(self.step_note.val(chosen), lib);
        // The tag (1.5), as on The Fiddle: the Ending's held final note, a double stop sliding a whole step down (40 %) and back (70 %).
        let tag_now = self.ample_final_hit
            && self.cur.is_some()
            && !staccato
            && len_beats >= 2.0f32
            && self.feel_for(who, lib).fiddle.tag > 0.0f32
            && self.rand01(pass, self.cur.map_or(0, |c| c.arr_pos), 1966, who, cx) < self.feel_for(who, lib).fiddle.tag;
        let tag_a1 = on_ppq + (off_ppq - on_ppq) * 0.4;
        let tag_a2 = on_ppq + (off_ppq - on_ppq) * 0.7;
        // A new phrase starts on a fresh bow (a phrase's last note cut short by a later note of its bar did not end it);
        // a rest of half a 16th (or 90 ms) is never bridged.
        let new_phrase = self.jb_prev_end && on_ppq >= self.swam_note_off - 60.0 * ms_ppq;
        let connected = self.swam_note >= 0
            && !new_phrase
            && (on_ppq - self.swam_note_off) < min(90.0 * ms_ppq, f64::from(self.tick_ppq(lib)) * 0.5)
            && !staccato
            && (flags & 32) == 0;

        // Velocity: the shaped velocity inside 18-118; a portamento goes below it (12-16), at most one in two bars.
        let mut v = self.step_vel.val(chosen) * self.velocity as f32 / 96.0f32 * dyn_mult;
        v += self.rand_bell(step, pass, 23, self.step_note.val(chosen), cx) * self.feel_for(who, lib).vel_var * h;
        let base_vel = clamp(v as i32, t.swam_vel_lo, t.swam_vel_hi);
        let mut attack_vel = base_vel;
        let mut line_avg = 0.0f32;
        for i in 0..self.jb_lens_n {
            line_avg += self.jb_lens.val(i);
        }
        line_avg = if self.jb_lens_n > 0 { line_avg / self.jb_lens_n as f32 } else { 0.0f32 };
        let porta = if self.lick_now.on {
            self.lick_now.slide_in && connected && !self.jb_prev_fast // a lick slides where it says
        } else {
            connected
                && !self.jb_prev_fast
                && prog_bar - self.jb_port_bar >= 2
                && bowed::portamento_here(
                    out_note - self.swam_note,
                    len_beats,
                    line_avg,
                    false,
                    self.energy,
                    vs.portamento,
                    self.rand01(pass, step, 1916, out_note, cx),
                )
        };
        if porta {
            attack_vel = 12 + min(4, (5.0f32 * self.rand01(pass, step, 1917, out_note, cx)) as i32);
            self.jb_port_bar = prog_bar;
        }

        // Vibrato: the sample set is chosen at note-on, so CC1 is set 5 ms before. Queued CC1 from the note before
        // (its ride or drift) at or after that point gives way.
        if self.jb_drop_queued_cc(t.swam_vib_cc, on_ppq - 5.0 * ms_ppq, -1) {
            self.vib_level = -1;
        }
        let phrase_start = self.swam_note < 0 || new_phrase;
        let soft_start = phrase_start && self.energy <= 2 && !accented && v < 64.0f32; // the first note of a soft phrase: plain
        let nat = !soft_start && bowed::wants_natural_vibrato(len_beats, accented, self.energy);
        let rv = self.rand01(pass, step, 1920, out_note, cx);
        let cc = bowed::vibrato_cc(nat, vs.vibrato_depth, len_beats, accented, rv);
        self.swam_vib(cc, on_ppq - 5.0 * ms_ppq, lib);
        if !nat && len_beats >= 0.75f32 && 400.0 * ms_ppq < f64::from(len) - 20.0 * ms_ppq {
            self.swam_vib(30 + (15.0f32 * rv) as i32, on_ppq + 400.0 * ms_ppq, lib); // simulated ride
        }
        if nat && len_beats >= 2.0f32 {
            self.swam_vib(clamp(cc + (if rv < 0.5f32 { -6 } else { 6 }), 64, 127), on_ppq + f64::from(len) * 0.5, lib); // never stagnant, never under 64
        }

        // Double stop: a sustained 3rd or 6th below, struck detached.
        let mut partner = -1;
        if !staccato && !connected && len_beats >= 1.0f32 && self.rand01(pass, step, 1919, out_note, cx) < vs.double_stops {
            let mut lo = 0;
            let mut hi = 127;
            self.swam_range_for(self.swam_inst, &mut lo, &mut hi, lib);
            let ch = self.chord_for_bar(prog_bar, cx);
            for d in [3, 4, 8, 9] {
                if out_note - d >= lo && self.is_chord_tone(out_note - d, ch) {
                    partner = out_note - d;
                    break;
                }
            }
        }
        if self.lick_now.on {
            // a lick's own double stops
            let mut lo = 0;
            let mut hi = 127;
            self.swam_range_for(self.swam_inst, &mut lo, &mut hi, lib);
            partner = self.lick_partner_for(out_note, lo, hi);
        }
        if tag_now {
            // a chord tone: the 5th above, else a 3rd above, else below (a 5th, 3rds), else a 4th / 6ths
            let mut lo = 0;
            let mut hi = 127;
            self.swam_range_for(self.swam_inst, &mut lo, &mut hi, lib);
            let ch = self.chord_for_bar(prog_bar, cx);
            partner = -1;
            for d in [7, 4, 3, -5, -4, -3, 5, 8, 9, -7, -8, -9] {
                if partner < 0 && out_note + d <= hi && out_note + d >= lo && self.is_chord_tone(out_note + d, ch) {
                    partner = out_note + d;
                }
            }
        }

        // Re-bows: a note longer than the bow (4 s) is re-bowed on half-bar points at any tempo, each with the player's chance.
        let mut pts = [0.0f64; 8];
        let mut n_pts = 0;
        if !staccato && !tag_now && len > (4000.0 * ms_ppq) as f32 {
            let mut cand = [0.0f64; 8];
            let n = bowed::rebow_points(on_ppq, f64::from(len), bar_len, f64::from(self.host_tempo), 4.0f32, &mut cand, 8, true);
            for i in 0..n {
                let ci = at_or_default(&cand, i);
                if ci < off_ppq - 60.0 * ms_ppq && self.rand01(pass, step, 1918, out_note * 8 + i, cx) < vs.rebow {
                    put_at(&mut pts, n_pts, ci);
                    n_pts += 1;
                }
            }
        }

        // Connection. The note before's re-bows still to come are dropped; fast notes detach by 3 ms, connected notes
        // overlap 20 ms (the library's legato), anything else ends 5 ms before.
        if self.swam_note >= 0 {
            for i in 0..64 {
                let q = *self.q.queue.at(i);
                if q.used
                    && q.kind == 0
                    && (q.flags & 8192) == 0
                    && (q.d1 == self.swam_note || q.d1 == self.jb_partner)
                    && q.on_ppq > self.jb_prev_on + 1.0e-9
                    && q.on_ppq >= on_ppq - 60.0 * ms_ppq
                {
                    self.q.queue.at_mut(i).used = false;
                }
            }
        }
        let mut prev_end = self.swam_note_off;
        if self.swam_note >= 0 {
            if connected && out_note != self.swam_note && partner < 0 {
                if fast || self.jb_prev_fast {
                    if self.swam_note_off > on_ppq - 3.0 * ms_ppq {
                        prev_end = on_ppq - 3.0 * ms_ppq;
                        self.fd_move_off(self.swam_note, prev_end);
                    }
                } else {
                    prev_end = on_ppq + 20.0 * ms_ppq;
                    self.fd_move_off(self.swam_note, prev_end);
                }
            } else if self.swam_note_off > on_ppq - 5.0 * ms_ppq {
                prev_end = on_ppq - 5.0 * ms_ppq;
                self.fd_move_off(self.swam_note, prev_end);
            }
        }
        if self.jb_partner >= 0 {
            if self.swam_note_off > on_ppq - 5.0 * ms_ppq {
                self.fd_move_off(self.jb_partner, on_ppq - 5.0 * ms_ppq);
            }
            self.jb_partner = -1;
        }

        // CC64 over a fast run: on 5 ms before its first connected fast note, off 10 ms after its last note ends, sent once
        // the run is known to have ended: here when this note does not continue it or closes a phrase, else at a rest
        // (jbStepEnd). Never between two notes of a run.
        let run_note = connected && fast;
        if self.jb_sustain_on && !run_note {
            self.jb_release(prev_end + 10.0 * ms_ppq);
            self.jb_sustain_on = false;
        }
        if run_note {
            if !self.jb_sustain_on {
                self.schedule(1, 64, 127, on_ppq - 5.0 * ms_ppq, 0.0f32);
                self.jb_sustain_on = true;
                self.jb_sustain_sent = true;
            }
            self.jb_run_end = off_ppq;
            if phrase_end {
                self.jb_release(off_ppq + 10.0 * ms_ppq);
                self.jb_sustain_on = false;
            }
        }

        // The note: re-bowed segments end 15 ms before the next (8 ms and +10 velocity on an accent at Energy 3+).
        self.q.schedule_flags = flags | 2048 | (if t.swam_custom_arts && staccato { 1024 } else { 0 }); // 2048 = SWAM articulation hook at fire time
        if tag_now {
            // The loudness (CC11 where the target has it) tapers to 16 over the last 40 %; each dip is a portamento (velocity 12-16).
            let at = [on_ppq, tag_a1, tag_a2];
            let pitch = [out_note, out_note - 2, out_note];
            for k in 0..6 {
                let w = on_ppq + (off_ppq - on_ppq) * (0.6 + 0.4 * f64::from(k) / 6.0);
                let mut clear = true; // never inside an attack (5 ms before to 60 ms after)
                for a in at {
                    if w > a - 5.0 * ms_ppq && w < a + 60.0 * ms_ppq {
                        clear = false;
                    }
                }
                if clear {
                    self.swam_expr(clamp(v as i32, 20, 127) + (16 - clamp(v as i32, 20, 127)) * (k + 1) / 6, w, lib);
                }
            }
            for i in 0..3 {
                let ai = at_or_default(&at, i);
                let e = if i < 2 { at_or_default(&at, i + 1) + 20.0 * ms_ppq } else { off_ppq };
                let sv = if i == 0 { attack_vel } else { 12 + min(4, (5.0f32 * self.rand01(pass, step, 1967, out_note * 4 + i, cx)) as i32) };
                self.schedule(0, at_or_default(&pitch, i), sv, ai, (e - ai) as f32);
                self.jb_sub_on.set(i, ai);
                self.jb_subs = i + 1;
            }
            if partner >= 0 {
                self.schedule(0, partner, clamp(base_vel - 8, t.swam_vel_lo, t.swam_vel_hi), on_ppq, (off_ppq - on_ppq) as f32);
            }
            n_pts = 0;
        }
        let mut s0 = on_ppq;
        let mut sv = attack_vel;
        let mut i = 0;
        while !tag_now && i <= n_pts {
            let mut e = off_ppq;
            let mut next_vel = base_vel;
            if i < n_pts {
                let pi = at_or_default(&pts, i);
                let p_sib = self.positive_modulo(((pi - self.bar_floor(pi, bar_len)) / f64::from(self.tick_ppq(lib))).round() as i32, self.spb());
                let emo = self.energy >= 3 && self.accent_at(who, p_sib, lib) > 1.0f32;
                e = pi - (if emo { 8.0 } else { 15.0 }) * ms_ppq;
                next_vel = clamp(base_vel + (if emo { 10 } else { 0 }), t.swam_vel_lo, t.swam_vel_hi);
            }
            self.schedule(0, out_note, sv, s0, (e - s0) as f32);
            if partner >= 0 {
                self.schedule(0, partner, clamp(sv - 8, t.swam_vel_lo, t.swam_vel_hi), s0, (e - s0) as f32);
            }
            self.jb_sub_on.set(i, s0);
            self.jb_subs = i + 1;
            if i < n_pts {
                s0 = at_or_default(&pts, i);
                sv = next_vel;
            }
            i += 1;
        }
        self.q.schedule_flags = 0;

        if self.jb_lens_n == 4 {
            for i in 0..3 {
                self.jb_lens.set(i, self.jb_lens.val(i + 1));
            }
            self.jb_lens.set(3, len_beats);
        } else {
            self.jb_lens.set(self.jb_lens_n, len_beats);
            self.jb_lens_n += 1;
        }
        self.swam_note = out_note;
        self.swam_note_off = off_ppq;
        self.last_note_end = off_ppq;
        if self.sounding_since < 0.0 {
            self.sounding_since = on_ppq;
        }
        self.jb_prev_end = phrase_end;
        self.jb_prev_fast = fast;
        self.jb_partner = partner;
        self.jb_prev_on = on_ppq;
        let sn = JbSnap {
            on: on_ppq,
            note_off: self.swam_note_off,
            last_end: self.last_note_end,
            prev_on: self.jb_prev_on,
            run_end: self.jb_run_end,
            note: self.swam_note,
            partner: self.jb_partner,
            port_bar: self.jb_port_bar,
            lens_n: self.jb_lens_n,
            subs: self.jb_subs,
            prev_end: self.jb_prev_end,
            prev_fast: self.jb_prev_fast,
            sustain_on: self.jb_sustain_on,
            lens: self.jb_lens,
            sub_on: self.jb_sub_on,
        };
        if self.jb_hist_n == 8 {
            for i in 0..7 {
                self.jb_hist.set(i, self.jb_hist.val(i + 1));
            }
            self.jb_hist_n = 7;
        }
        self.jb_hist.set(self.jb_hist_n, sn);
        self.jb_hist_n += 1;
    }

    /// `void jbStepEnd (int step)`
    pub(crate) fn jb_step_end(&mut self, step: i32, lib: &LibraryData) {
        if !self.jb_sustain_on || !self.driver_is(Driver::Swam, lib) {
            return;
        }
        let ms_ppq = self.ppq_per_ms();
        let tick = f64::from(self.tick_ppq(lib));
        if f64::from(step + 1) * tick - 34.0 * ms_ppq - self.jb_run_end >= min(90.0 * ms_ppq, tick * 0.5) {
            self.jb_release(self.jb_run_end + 10.0 * ms_ppq);
            self.jb_sustain_on = false;
            if self.jb_hist_n > 0 {
                self.jb_hist.at_mut(self.jb_hist_n - 1).sustain_on = false; // the state after the last note: released at the rest
            }
        }
    }

    /// `bool jbInnerAttack (int slot, float64 cut) const`
    pub(crate) fn jb_inner_attack(&self, slot: i32, cut: f64) -> bool {
        if self.jb_subs < 2 || self.jb_sub_on.val(0) >= cut || self.q.queue.at(slot).kind != 0 {
            return false;
        }
        for k in 1..self.jb_subs {
            if (self.q.queue.at(slot).on_ppq - self.jb_sub_on.val(k)).abs() < 1.0e-9 {
                return true;
            }
        }
        false
    }

    /// `int jbRewind (float64 cut)`
    pub(crate) fn jb_rewind(&mut self, cut: f64, lib: &LibraryData) -> i32 {
        if !self.driver_is(Driver::Swam, lib) || self.jb_hist_n == 0 {
            return 0;
        }
        let mut dropped = 0;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 1 && q.d1 == 64 && q.on_ppq >= cut {
                dropped |= if q.d2 > 0 { 1 } else { 2 };
            }
        }
        let press_queued = (dropped & 1) != 0;
        let mut k = self.jb_hist_n - 1;
        while k >= 0 && self.jb_hist.val(k).on >= cut {
            k -= 1;
        }
        if k < 0 {
            if self.jb_sustain_on && !press_queued {
                self.jb_stray_down = true; // a pedal down from a note no longer known: lift it
            }
            self.swam_note = -1;
            self.jb_prev_end = false;
            self.jb_prev_fast = false;
            self.jb_partner = -1;
            self.jb_prev_on = -1.0;
            self.jb_lens_n = 0;
            self.jb_subs = 0;
            self.jb_hist_n = 0;
            self.jb_sustain_on = false;
            self.jb_run_end = -1.0;
            self.jb_port_bar = -1000;
        } else {
            let sn = self.jb_hist.val(k);
            self.swam_note = sn.note;
            self.swam_note_off = sn.note_off;
            self.last_note_end = sn.last_end;
            self.jb_prev_on = sn.prev_on;
            self.jb_run_end = sn.run_end;
            self.jb_partner = sn.partner;
            self.jb_port_bar = sn.port_bar;
            self.jb_lens_n = sn.lens_n;
            self.jb_subs = sn.subs;
            self.jb_prev_end = sn.prev_end;
            self.jb_prev_fast = sn.prev_fast;
            self.jb_sustain_on = sn.sustain_on;
            self.jb_lens = sn.lens;
            self.jb_sub_on = sn.sub_on;
            self.jb_hist_n = k + 1;
        }
        self.vib_level = -1;
        dropped
    }

    /// `void jbAfterRewind (float64 cut, int dropped)`
    pub(crate) fn jb_after_rewind(&mut self, cut: f64, dropped: i32, lib: &LibraryData) {
        if !self.driver_is(Driver::Swam, lib) {
            return;
        }
        self.jb_rel_floor = cut + self.ppq_per_ms();
        if self.jb_stray_down || (!self.jb_sustain_on && (dropped & 2) != 0) {
            self.schedule(1, 64, 0, self.jb_rel_floor, 0.0f32);
        }
        self.jb_stray_down = false;
        if self.jb_sustain_on && (dropped & 1) != 0 {
            self.schedule(1, 64, 127, cut, 0.0f32);
        }
    }

    /// `bool fdInnerAttack (int slot, float64 cut) const`
    pub(crate) fn fd_inner_attack(&self, slot: i32, cut: f64) -> bool {
        if self.fd_subs < 2 || self.fd_sub_on.val(0) >= cut || self.q.queue.at(slot).kind != 0 {
            return false;
        }
        for k in 1..self.fd_subs {
            if (self.q.queue.at(slot).on_ppq - self.fd_sub_on.val(k)).abs() < 1.0e-9 {
                return true;
            }
        }
        false
    }

    /// `void fdMoveOff (int note, float64 offPpq)`
    pub(crate) fn fd_move_off(&mut self, note: i32, off_ppq: f64) {
        // A queued copy is the latest one (a re-bow or shuffle attack still to come), so it is looked for first;
        // of several queued copies (a re-bow and a shuffle attack), the one starting last.
        let mut latest = -1;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 0 && q.ch == 0 && q.d1 == note && (latest < 0 || q.on_ppq > self.q.queue.at(latest).on_ppq) {
                latest = i;
            }
        }
        if latest >= 0 {
            let q = self.q.queue.at_mut(latest);
            q.dur_ppq = max(0.01, off_ppq - q.on_ppq) as f32;
            return;
        }
        for i in 0..64 {
            if self.active_slots.val(i)
                && self.active_channels.val(i) == self.midi_channel0
                && self.active_notes.val(i) == note
                && self.active_off_ppq.val(i) > self.current_ppq
            {
                self.active_off_ppq.set(i, max(off_ppq, self.current_ppq));
                return;
            }
        }
    }

    /// `void fdPress (int key, float64 at)`
    pub(crate) fn fd_press(&mut self, key: i32, at: f64) {
        let (now, ms) = (self.current_ppq, self.ppq_per_ms());
        let mut s = SchedAt { q: &mut self.q, now };
        let mut c = kit::DriverCtx { sink: &mut s, now, ms };
        kit::tap_key(&mut c, key, 100, at, 5.0);
    }

    /// `void fdReturn (float64 at)`
    pub(crate) fn fd_return(&mut self, at: f64, lib: &LibraryData) {
        let t = self.current_target(lib);
        let ret_key = self.ks(t.fd_return_key);
        let (now, ms) = (self.current_ppq, self.ppq_per_ms());
        let mut s = SchedAt { q: &mut self.q, now };
        let mut c = kit::DriverCtx { sink: &mut s, now, ms };
        self.fd_ret.fire(&mut c, at, t.fd_return_mode, ret_key);
    }

    /// `void fdDropQueuedKeys (float64 cut)`
    pub(crate) fn fd_drop_queued_keys(&mut self, cut: f64, lib: &LibraryData) {
        if !self.driver_is(Driver::Fiddle, lib) {
            return;
        }
        let t = self.current_target(lib);
        let ret_press = ReturnMode::return_for(t.fd_return_mode, self.fd_ret.ret_key, self.ks(t.fd_return_key));
        let mut press_queued = false;
        let mut ret_queued = false;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if !q.used || q.kind != 0 || (q.flags & 8192) == 0 || q.d1 >= t.fd_low || q.on_ppq < cut {
                continue;
            }
            if self.fd_ret.latched >= 0 && q.d1 == self.fd_ret.latched && q.on_ppq >= self.fd_ret.latch_at - 1.0e-9 {
                press_queued = true;
            } else if self.fd_ret.ret_key >= 0 && q.d1 == ret_press && (self.fd_ret.latched < 0 || q.on_ppq < self.fd_ret.latch_at - 1.0e-9) {
                ret_queued = true;
            }
            if q.d1 == self.fd_ret.held {
                self.fd_ret.held = -1; // its hold never started
            }
            self.q.queue.at_mut(i).used = false;
        }
        self.fd_ret.after_drop(press_queued, ret_queued, cut);
    }

    /// `void fdReturnNow ()`
    pub(crate) fn fd_return_now(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        self.fd_drop_queued_keys(-1.0e18, lib);
        if self.fd_ret.latched >= 0 && self.driver_is(Driver::Fiddle, lib) {
            let key =
                ReturnMode::return_for(self.current_target(lib).fd_return_mode, self.fd_ret.latched, self.ks(self.current_target(lib).fd_return_key));
            if key >= 0 {
                self.send_keyswitch(0x90 + self.midi_channel0, key, 100, cx, out);
                self.send_keyswitch(0x80 + self.midi_channel0, key, 0, cx, out);
            }
        }
        if self.fd_ret.held >= 0 {
            self.fd_move_off(self.fd_ret.held, self.current_ppq);
        }
        self.fd_ret.reset();
        self.fd_last_orn = false;
        self.fd_orn_beat = -1;
    }

    /// `void fiddlePerform (int who, int step, int pass, int progBar, int sib, float64 stepPpq, float64 swing, float dynMult)`
    /// (the sink is `sink` here: `out` is the C++ name of the note played)
    pub(crate) fn fiddle_perform(
        &mut self,
        who: i32,
        step: i32,
        pass: i32,
        prog_bar: i32,
        sib: i32,
        step_ppq: f64,
        swing: f64,
        dyn_mult: f32,
        cx: &mut Ctx<'_>,
        sink: &mut dyn MidiSink,
    ) {
        let lib = cx.lib;
        let chosen = self.swam_choose(who, lib);
        if chosen < 0 {
            return;
        }
        let t = self.current_target(lib);
        let st = &self.feel_for(who, lib).fiddle;
        let sp = self.fd_splits(lib);
        let ms_ppq = self.ppq_per_ms();
        let beat = 1.0f32; // ppq are quarter notes
        let flags = self.step_flags.val(chosen) & !(32 | 64); // The Fiddle has no pizz / tremolo
        let mut ms = self.feel_for(who, lib).timing_ms + self.step_shift_ms.val(chosen) + self.step_shift.pocket_ms; // + the song's Pocket
        ms = clamp(
            ms + self.rand_bell(step, pass, 1901, self.step_note.val(chosen), cx)
                * self.feel_for(who, lib).jitter_ms
                * (self.humanize / 50.0f32)
                * 0.5f32
                + self.groove_ms_for(who, step_ppq, cx),
            -34.0f32,
            90.0f32,
        ); // steps are made 40 ms ahead: room for CC11 5 ms before
        if self.lick_now.land {
            ms = 0.0f32; // a kick-off lands on the downbeat
        }
        let on_ppq = step_ppq + swing + f64::from(ms) * ms_ppq;
        // A note still playing its inner attacks (shuffle, re-bow) when this one comes: the attacks too close to this
        // note are dropped, and the line continues from the last one that sounds.
        if self.fd_subs > 1 {
            let mut keep = 0;
            while keep + 1 < self.fd_subs && self.fd_sub_on.val(keep + 1) <= on_ppq - 60.0 * ms_ppq {
                keep += 1;
            }
            if keep + 1 < self.fd_subs {
                for i in 0..64 {
                    let q = *self.q.queue.at(i);
                    if q.used && q.kind == 0 && (q.flags & 8192) == 0 && q.on_ppq > self.fd_sub_on.val(keep) + 1.0e-9 {
                        self.q.queue.at_mut(i).used = false;
                    }
                }
                self.fd_prev_note = self.fd_sub_pitch.val(keep);
            }
        }
        self.fd_subs = 0;
        // The ornamented note before has ended well before this one: its return goes right after it.
        if self.fd_ret.latched >= 0 && self.fd_ret.due >= 0.0 && on_ppq - 20.0 * ms_ppq > self.fd_ret.due {
            self.fd_return(self.fd_ret.due, lib);
        }
        let mut len = max(0.05f32, self.step_len.val(chosen)); // the step's length already carries the Length knob: no stretching
        let len_beats = len / beat;
        // The phrase's last note: a phrase-end bar, and the note's written end reaches the bar's end (within half a tick).
        let bar_len = f64::from(self.get_bar_length_ppq());
        let bar_end = self.bar_floor(step_ppq, bar_len) + bar_len;
        let phrase_end =
            self.is_phrase_end(prog_bar) && step_ppq + f64::from(len / max(0.1f32, self.length)) >= bar_end - f64::from(self.tick_ppq(lib)) * 0.5; // written length, before the Length knob
        if phrase_end {
            len = max(0.05f32, len - (30.0 * ms_ppq) as f32);
        }
        // A new phrase starts on a fresh bow, and a written rest of an 8th of a beat or more is never bridged.
        let connected =
            self.fd_prev_note >= 0 && !self.fd_prev_end && (on_ppq - self.fd_prev_off) < min(90.0 * ms_ppq, f64::from(self.tick_ppq(lib)) * 0.5);
        let fast = len < (140.0 * ms_ppq) as f32;
        let downbeat = sib == 0;
        let strong = crate::meter::strong_step(self.meter, sib);
        let accented = self.lick_now.accent || self.accent_at(who, self.positive_modulo(sib, self.spb()), lib) > 1.0f32;
        let out = if self.lick_now.on && self.lick_now.absolute {
            let mut a = self.step_note.val(chosen);
            let mut g = 0;
            while g < 10 && a < t.fd_low {
                a += 12;
                g += 1;
            }
            g = 0;
            while g < 10 && a > t.fd_high {
                a -= 12;
                g += 1;
            }
            clamp(a, t.fd_low, t.fd_high)
        } else {
            bowed::fold_to_line(self.step_note.val(chosen), self.fd_prev_note, t.fd_low, t.fd_high) // follows the line played
        };
        let interval = if self.fd_prev_note >= 0 { out - self.fd_prev_note } else { 0 };
        let ch = self.chord_for_bar(prog_bar, cx);
        let slide_in = if self.lick_now.on {
            self.lick_now.slide_in && connected && !self.fd_prev_fast // a lick slides where it says
        } else {
            connected
                && !fast
                && !self.fd_prev_fast
                && interval.abs() >= 2
                && interval.abs() <= 5
                && strong
                && self.is_chord_tone(out, ch)
                && self.rand01(pass, step, 1902, out, cx) < st.slides * (if self.energy >= 3 { 1.3f32 } else { 1.0f32 })
        };
        // Lead-in (Sing band): the next step is generated only when it is due, so whether it holds is not known here.
        // Approximation: a connected note under a beat on the 16th just before a half-bar point leads into it.
        let lead_in = connected && !fast && len_beats < 1.0f32 && (sib + 1) % half_step(self.meter) == 0;
        // Chop (Boyd): off-beat short notes become very short, Plain band, CC11 dip after.
        let chop = st.chop > 0.0f32 && sib % 2 == 1 && len_beats < 0.5f32 && self.rand01(pass, step, 1906, out, cx) < st.chop;
        if chop {
            len = min(len, (60.0 * ms_ppq) as f32);
        }
        let off_ppq = on_ppq + f64::from(len);
        // The tag (1.5): the Ending's held final note (a turnaround's or kick-off's landing on it too), with the player's chance: a double stop that slides down a whole step
        // (40 % of the hold) and back (70 %), the Bow band, CC11 tapering over the last 40 %.
        let tag_now = self.ample_final_hit
            && self.cur.is_some()
            && !chop
            && len_beats >= 2.0f32
            && st.tag > 0.0f32
            && self.rand01(pass, self.cur.map_or(0, |c| c.arr_pos), 1966, who, cx) < st.tag;
        let tag_a1 = on_ppq + (off_ppq - on_ppq) * 0.4;
        let tag_a2 = on_ppq + (off_ppq - on_ppq) * 0.7;
        // Sampled ornaments, only with the articulation return confirmed (nothing in the keyswitch range otherwise):
        // at most one a beat, never two notes in a row, none in a section's first bar but a downbeat scoop (FiddleVoice),
        // trills only on a phrase's last note of a beat or more, grace slides only into the root or the 3rd.
        let rel = self.positive_modulo(out - self.key_root - ch.root_pc, 12);
        let root_or_third = rel == 0 || rel == self.positive_modulo(ch.tones.val(1), 12);
        let mut orn = Ornament::None;
        if t.fd_return_mode != 0 && !self.lick_now.on && !tag_now {
            let beat_idx = (step_ppq + 1.0e-6).floor() as i32;
            let in_fill = sib >= fill_zone_start(self.meter) && {
                let nx_ch = self.chord_for_bar(prog_bar + 1, cx);
                let change_next = !self.same_chord(ch, nx_ch);
                self.fill_active(who, pass, prog_bar, change_next, true, cx, sink)
            };
            let first_bar = self.cur.is_some_and(|c| c.chord_index == 0) && prog_bar == self.cur_prog_bar;
            // (no lookahead: whether the next note is lower is not known here, so trills go up)
            let oc = bowed::OrnamentCtx {
                phrase_start: sib == 0 || !connected,
                downbeat,
                accented,
                in_fill,
                phrase_end: phrase_end && len_beats >= 1.0f32,
                first_bar_of_section: first_bar,
                line_falling: interval < 0,
                next_lower: false,
                staccato_flag: (flags & 1024) != 0 || chop,
                leap_semis: interval,
                energy: self.energy,
                last_had_ornament: self.fd_last_orn,
                beat_has_ornament: self.fd_orn_beat == beat_idx,
            };
            orn = bowed::choose_ornament(&oc, st, self.rand01(pass, step, 1911, out, cx));
            if (orn == Ornament::GraceSlideUp || orn == Ornament::GraceSlideDown) && !root_or_third {
                orn = Ornament::None;
            }
            if orn != Ornament::None {
                self.fd_orn_beat = beat_idx;
            }
        }
        self.fd_last_orn = orn != Ornament::None;
        let mut orn_key = match orn {
            Ornament::Scoop => t.fd_art[FD_SCOOP],
            Ornament::GraceUp => t.fd_art[FD_GRACE_UP],
            Ornament::GraceDown => t.fd_art[FD_GRACE_DOWN],
            Ornament::GraceSlideUp => t.fd_art[FD_GRACE_SLIDE_UP],
            Ornament::GraceSlideDown => t.fd_art[FD_GRACE_SLIDE_DOWN],
            Ornament::TrillUp => t.fd_art[FD_TRILL_UP],
            Ornament::TrillDown => t.fd_art[FD_TRILL_DOWN],
            Ornament::Staccato => t.fd_art[FD_STACCATO],
            Ornament::None => -1,
        };
        // A slide into the root or 3rd with the portamento key confirmed: the key (one-shot: it forces the slide on the next
        // connection, nothing to return) replaces the Slide band; the note keeps its own band (and vibrato).
        let porta_key = !tag_now && slide_in && root_or_third && orn == Ornament::None && t.fd_return_mode != 0 && t.fd_portamento_key >= 0;
        if porta_key && t.fd_return_mode == 3 {
            orn_key = t.fd_portamento_key; // hold: held for the note like the others
        }
        let band = if tag_now {
            Band::Bow
        } else if chop {
            Band::Plain
        } else if self.lick_now.band >= 0 {
            band_from(self.lick_now.band)
        } else {
            bowed::band_for(&bowed::NoteCtx {
                len_beats,
                connected,
                accented,
                downbeat,
                strong_beat: strong,
                slide_in: slide_in && !porta_key,
                lead_in,
                phrase_start: sib == 0,
                energy: self.energy,
            })
        };
        let weight = clamp((len_beats - 1.0f32) / 3.0f32, 0.0f32, 1.0f32) * 0.6f32
            + clamp(self.step_vel.val(chosen) * dyn_mult / 127.0f32, 0.0f32, 1.0f32) * 0.4f32;
        let vel = bowed::band_velocity(band, &sp, self.rand01(pass, step, 1903, out, cx), strong, weight);
        let band_lo = |b: Band| -> i32 {
            if b == Band::Slide {
                6
            } else if b == Band::Plain {
                sp.portamento + 4
            } else if b == Band::Sing {
                sp.vibrato + 4
            } else {
                sp.slur + 5
            }
        };

        // Double stop: a held or accented note, not in a run, with a partner 3-9 semitones away (an open string above, else below).
        let connected_run = connected && self.fd_prev_off - self.fd_prev_on < 1.0;
        let mut partner = -1;
        if !fast
            && !chop
            && !slide_in
            && orn_key < 0
            && !porta_key
            && len_beats >= 0.5f32
            && (len_beats >= 1.0f32 || accented)
            && !connected_run
            && self.rand01(pass, step, 1904, out, cx) < st.double_stops
        {
            // `[&] (int n) { return isChordTone (n, ch); }`, read for every pitch first.
            let mut tones = [false; 128];
            for (p, x) in (0i32..).zip(tones.iter_mut()) {
                *x = self.is_chord_tone(p, ch);
            }
            partner = bowed::double_stop_partner(out, |n| at_or_default(&tones, n), t.fd_low, t.fd_high);
        }
        if self.lick_now.on {
            partner = if porta_key { -1 } else { self.lick_partner_for(out, t.fd_low, t.fd_high) }; // a lick's own double stops
        }
        if tag_now {
            // a chord tone: the 5th above, else a 3rd above, else below (a 5th, 3rds), else a 4th / 6ths
            partner = -1;
            for d in [7, 4, 3, -5, -4, -3, 5, 8, 9, -7, -8, -9] {
                if partner < 0 && out + d <= t.fd_high && out + d >= t.fd_low && self.is_chord_tone(out + d, ch) {
                    partner = out + d;
                }
            }
        }
        // Shuffle: swung 8ths in one bow (slurred); never when its short notes would be fast notes (those detach instead).
        let mut sn = [bowed::ShuffleNote::default(); 16];
        let mut n_shuffle = 0;
        if !self.lick_now.on
            && !tag_now
            && !fast
            && !chop
            && orn_key < 0
            && !porta_key
            && len_beats >= 1.0f32
            && self.energy >= 3
            && self.rand01(pass, step, 1905, out, cx) < st.shuffle
        {
            // 6/8 (Task 17): a bow per eighth slurred 2 + 1 (shuffleSplit68); the longbow holds a bow per dotted beat.
            let jig = self.meter == Meter::M68;
            n_shuffle = if jig && st.shuffle_style != 2 {
                bowed::shuffle_split68(len_beats, (sib / 2) % (self.spb() / 2), &mut sn, 16)
            } else {
                bowed::shuffle_split(len_beats, self.swing_amt / 100.0f32, (sib / 2) % (self.spb() / 2), &mut sn, 16, st.shuffle_style)
            };
            let bow_beats = if jig { 1.5f32 } else { 1.0f32 };
            if st.shuffle_style == 2 {
                // longbow: one attack a beat (6/8: a dotted beat), no re-bowing
                let mut bb = 0.0f32;
                while bb + bow_beats <= len_beats + 1.0e-4f32 && n_shuffle < 15 {
                    put_at(
                        &mut sn,
                        n_shuffle,
                        bowed::ShuffleNote {
                            start_beats: bb,
                            len_beats: min(bow_beats, len_beats - bb),
                            accent: false,
                            partner: false,
                            new_bow: false,
                        },
                    );
                    n_shuffle += 1;
                    bb += bow_beats;
                }
            }
            if n_shuffle == 1 && st.shuffle_style == 2 {
                n_shuffle = 0;
            }
            let min_ms = if st.shuffle_style == 1 { 100.0f32 } else { 140.0f32 }; // georgia's sub-notes are slurred by design
            let mut i = 0;
            while i < n_shuffle {
                if at_or_default(&sn, i).len_beats * beat < (f64::from(min_ms) * ms_ppq) as f32 {
                    n_shuffle = 0;
                }
                i += 1;
            }
        }
        // Re-bowing: a plain note longer than the bow, cut on half-bar points (each bar below 90 BPM).
        let mut pts = [0.0f64; 8];
        let mut n_rebow = 0;
        if n_shuffle == 0 && !chop && !tag_now && orn_key < 0 && !porta_key {
            n_rebow = bowed::rebow_points(step_ppq, f64::from(len), bar_len, f64::from(self.host_tempo), t.fd_bow_seconds, &mut pts, 8, false);
        }
        while n_rebow > 0 && at_or_default(&pts, n_rebow - 1) >= off_ppq - 60.0 * ms_ppq {
            n_rebow -= 1; // no sliver segment at the end
        }

        // Every attack inside this note: CC11 never moves through one.
        let mut attacks = [0.0f64; 20];
        let mut n_attacks = 0;
        put_at(&mut attacks, n_attacks, on_ppq);
        n_attacks += 1;
        for i in 1..n_shuffle {
            put_at(&mut attacks, n_attacks, on_ppq + f64::from(at_or_default(&sn, i).start_beats * beat));
            n_attacks += 1;
        }
        for i in 0..n_rebow {
            put_at(&mut attacks, n_attacks, at_or_default(&pts, i));
            n_attacks += 1;
        }
        if tag_now {
            put_at(&mut attacks, n_attacks, tag_a1);
            n_attacks += 1;
            put_at(&mut attacks, n_attacks, tag_a2);
            n_attacks += 1;
        }
        let clear_of_attacks = |w: f64| -> bool {
            for i in 0..n_attacks {
                let a = at_or_default(&attacks, i);
                if w > a - 5.0 * ms_ppq && w < a + 60.0 * ms_ppq {
                    return false;
                }
            }
            true
        };

        // Loudness on CC11, placed before the note and left alone through the attack: a held note's own swell or
        // taper still queued past this point gives way to the new note.
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 1 && q.d1 == self.swam_expr_cc_sent && q.on_ppq > on_ppq - 5.0 * ms_ppq {
                self.q.queue.at_mut(i).used = false;
                self.expr_level = -1;
            }
        }
        let v = self.step_vel.val(chosen) * self.velocity as f32 / 96.0f32 * dyn_mult;
        let level = clamp((34.0f32 + v * 0.72f32) as i32, 20, 127);
        // A quick run keeps one level (CC11 moves slowly), and nothing lands inside the previous note's attack.
        if self.expr_level < 0 || self.fd_chop_dip || (!(fast && connected) && on_ppq - self.fd_prev_on >= 66.0 * ms_ppq) {
            self.swam_expr(level, on_ppq - 5.0 * ms_ppq, lib);
        }
        self.fd_chop_dip = false;
        let longbow = n_shuffle > 0 && st.shuffle_style == 2;
        if longbow && len_beats >= 3.0f32 && clear_of_attacks(on_ppq + f64::from(2.0f32 * beat) - 80.0 * ms_ppq) {
            self.swam_expr(min(127, level + 8), on_ppq + f64::from(2.0f32 * beat) - 80.0 * ms_ppq, lib); // rise into beat 3
        } else if len_beats >= 2.0f32 && clear_of_attacks(on_ppq + f64::from(len) * 0.55) {
            self.swam_expr(min(127, level + 8), on_ppq + f64::from(len) * 0.55, lib);
        }
        if tag_now {
            // the tag's taper: from the note's level to 16 in 6 steps over the last 40 % of the hold
            for k in 0..6 {
                let w = on_ppq + (off_ppq - on_ppq) * (0.6 + 0.4 * f64::from(k) / 6.0);
                if clear_of_attacks(w) {
                    self.swam_expr(level + (16 - level) * (k + 1) / 6, w, lib);
                }
            }
        }
        if phrase_end && !tag_now && clear_of_attacks(off_ppq - 40.0 * ms_ppq) {
            self.swam_expr(max(16, level - 6), off_ppq - 40.0 * ms_ppq, lib); // never inside an attack
        }

        // Connection: fast notes detach (3 ms gap), connected notes overlap 20 ms (the plugin's slur / slide).
        // A double stop is struck detached: the previous note ends 5 ms before the pair.
        if self.fd_ret.held >= 0 {
            // "hold": the key is held for its note only. That note ends 14 ms before this one (no legato overlap) and the
            // key 2 ms after it, so neither this note nor a fresh press of the same key overlaps it.
            let end = max(self.fd_prev_on + 5.0 * ms_ppq, min(self.fd_prev_off, on_ppq - 14.0 * ms_ppq));
            if self.fd_prev_note >= 0 {
                self.fd_move_off(self.fd_prev_note, end);
            }
            self.fd_move_off(self.fd_ret.held, end + 2.0 * ms_ppq);
            self.fd_ret.held = -1;
        } else if connected && self.fd_prev_note != out && partner < 0 {
            if fast || chop || self.fd_prev_fast {
                if self.fd_prev_off > on_ppq - 3.0 * ms_ppq {
                    self.fd_move_off(self.fd_prev_note, on_ppq - 3.0 * ms_ppq);
                }
            } else {
                self.fd_move_off(self.fd_prev_note, on_ppq + 20.0 * ms_ppq);
            }
        } else if self.fd_prev_note >= 0 && self.fd_prev_off > on_ppq - 5.0 * ms_ppq {
            self.fd_move_off(self.fd_prev_note, on_ppq - 5.0 * ms_ppq);
        }
        // The last pair's partner never rings into this note.
        if self.fd_partner >= 0 {
            if self.fd_prev_off > on_ppq - 5.0 * ms_ppq {
                self.fd_move_off(self.fd_partner, on_ppq - 5.0 * ms_ppq);
            }
            self.fd_partner = -1;
        }

        // The articulation key 10 ms before its note; a latched one is returned before the next plain note (or right
        // after its own note when the next is far), and before another press.
        if orn_key >= 0 {
            if t.fd_return_mode == 3 {
                let k = self.ks(orn_key);
                {
                    let now = self.current_ppq;
                    let mut s = SchedAt { q: &mut self.q, now };
                    let mut c = kit::DriverCtx { sink: &mut s, now, ms: ms_ppq };
                    kit::hold_key(
                        &mut c,
                        k,
                        100,
                        on_ppq - 10.0 * ms_ppq,
                        (off_ppq + 2.0 * ms_ppq - (on_ppq - 10.0 * ms_ppq)) as f32,
                        kit::K_BELOW_RANGE,
                    );
                }
                self.fd_ret.held = self.ks(orn_key);
            } else {
                self.fd_return(on_ppq - 20.0 * ms_ppq, lib);
                let k = self.ks(orn_key);
                let now = self.current_ppq;
                let mut s = SchedAt { q: &mut self.q, now };
                let mut c = kit::DriverCtx { sink: &mut s, now, ms: ms_ppq };
                self.fd_ret.latch(&mut c, k, on_ppq - 10.0 * ms_ppq, off_ppq + 5.0 * ms_ppq);
            }
        } else if self.fd_ret.latched >= 0 {
            self.fd_return(on_ppq - (if porta_key { 20.0 } else { 10.0 }) * ms_ppq, lib);
        }
        if porta_key && t.fd_return_mode != 3 {
            self.fd_press(self.ks(t.fd_portamento_key), on_ppq - 10.0 * ms_ppq);
        }
        self.q.schedule_flags = flags;
        let mut last_pitch = out;
        if tag_now {
            // Root, a whole step down, root: the dips overlap the note before by 20 ms (the plugin's slide). With a portamento
            // key it is pressed for each destination note (hold: released just before the next press), else the Slide band.
            let keyed = t.fd_return_mode != 0 && t.fd_portamento_key >= 0;
            let pk = if keyed { self.ks(t.fd_portamento_key) } else { -1 };
            let at = [on_ppq, tag_a1, tag_a2];
            let pitch = [out, out - 2, out];
            for i in 0..3 {
                let ai = at_or_default(&at, i);
                let e = if i < 2 { at_or_default(&at, i + 1) + 20.0 * ms_ppq } else { off_ppq };
                let sv = if i == 0 {
                    vel
                } else {
                    bowed::band_velocity(
                        if keyed { Band::Plain } else { Band::Slide },
                        &sp,
                        self.rand01(pass, step, 1967, out * 4 + i, cx),
                        false,
                        0.3f32,
                    )
                };
                if i > 0 && keyed {
                    if t.fd_return_mode == 3 {
                        let ke = if i == 1 { at_or_default(&at, 2) - 12.0 * ms_ppq } else { off_ppq + 2.0 * ms_ppq };
                        {
                            let now = self.current_ppq;
                            let mut s = SchedAt { q: &mut self.q, now };
                            let mut c = kit::DriverCtx { sink: &mut s, now, ms: ms_ppq };
                            kit::hold_key(&mut c, pk, 100, ai - 10.0 * ms_ppq, (ke - (ai - 10.0 * ms_ppq)) as f32, kit::K_BELOW_RANGE);
                        }
                        self.q.schedule_flags = flags;
                    } else {
                        self.fd_press(pk, ai - 10.0 * ms_ppq);
                        self.q.schedule_flags = flags;
                    }
                }
                self.schedule(0, at_or_default(&pitch, i), sv, ai, (e - ai) as f32);
                self.fd_sub_pitch.set(i, at_or_default(&pitch, i));
                self.fd_sub_on.set(i, ai);
                self.fd_subs = i + 1;
            }
            if partner >= 0 {
                self.schedule(0, partner, max(band_lo(band), vel - 8), on_ppq, (off_ppq - on_ppq) as f32);
            }
            if partner >= 0 {
                self.fd_partner = partner;
            }
        } else if n_shuffle > 0 {
            // One bow across the strings: notes overlap 10 ms (slur); with a partner the bow rocks between the two pitches,
            // without one a repeated pitch re-attacks after a 6 ms gap. Accents (3+3+2) bow-change, the rest slur.
            let pitch_of = |i: i32| if at_or_default(&sn, i).partner && partner >= 0 { partner } else { out };
            for i in 0..n_shuffle {
                let r = self.rand01(pass, step, 1907, out * 16 + i, cx);
                let sni = at_or_default(&sn, i);
                let sv = if i == 0 {
                    vel
                } else if sni.accent {
                    bowed::band_velocity(Band::Bow, &sp, r, strong, 0.2f32)
                } else {
                    bowed::band_velocity(Band::Plain, &sp, r, false, 0.0f32)
                };
                let s0 = on_ppq + f64::from(sni.start_beats * beat);
                let last = i == n_shuffle - 1;
                let repeat = !last && (pitch_of(i + 1) == pitch_of(i) || at_or_default(&sn, i + 1).new_bow); // 6/8: a bow change detaches too
                let sl = if last {
                    (off_ppq - s0) as f32
                } else {
                    sni.len_beats * beat + (if repeat { -((6.0 * ms_ppq) as f32) } else { (10.0 * ms_ppq) as f32 })
                };
                self.schedule(0, pitch_of(i), sv, s0, max(0.01f32, sl));
                last_pitch = pitch_of(i);
                if st.shuffle_style == 2 && i == 0 && partner >= 0 {
                    self.schedule(0, partner, max(band_lo(band), sv - 8), s0, (off_ppq - s0) as f32); // partner sustained under the beats
                }
                self.fd_sub_pitch.set(i, last_pitch);
                self.fd_sub_on.set(i, s0);
                self.fd_subs = i + 1;
            }
            if st.shuffle_style == 2 && partner >= 0 {
                self.fd_partner = partner;
            }
        } else {
            // Re-bowed segments: 10 ms gap, down / up bow alternating by 6 inside the Bow band. A partner sounds with each.
            let bow_lo = band_lo(Band::Bow);
            let mut s0 = on_ppq;
            for i in 0..=n_rebow {
                let e = if i < n_rebow { at_or_default(&pts, i) - 10.0 * ms_ppq } else { off_ppq };
                let sv = if i == 0 { vel } else { clamp(vel + (if i % 2 == 1 { -6 } else { 6 }), bow_lo, 118) };
                self.schedule(0, out, sv, s0, (e - s0) as f32);
                if partner >= 0 {
                    self.schedule(0, partner, max(if i == 0 { band_lo(band) } else { bow_lo }, sv - 8), s0, (e - s0) as f32);
                }
                self.fd_sub_pitch.set(i, out);
                self.fd_sub_on.set(i, s0);
                self.fd_subs = i + 1;
                if i < n_rebow {
                    s0 = at_or_default(&pts, i);
                }
            }
            if partner >= 0 {
                self.fd_partner = partner;
            }
        }
        self.q.schedule_flags = 0;
        if chop {
            self.swam_expr(max(16, level - 20), max(off_ppq + 5.0 * ms_ppq, on_ppq + 60.0 * ms_ppq), lib); // after the attack
            self.fd_chop_dip = true;
        }
        self.fd_prev_note = last_pitch;
        self.fd_prev_on = on_ppq;
        self.fd_prev_off = off_ppq;
        self.fd_prev_end = phrase_end;
        self.fd_prev_fast = fast || chop; // a chop is a fast note
    }
}

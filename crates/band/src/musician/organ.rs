//! `MusicianOrgan.cpp`: the organ (Hammond B-3X driver).
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.
//! `kitCtx()` is written in place (see the module doc of `musician/mod.rs`).

// Line-by-line translation: the C++ shapes (a default then field writes, a counter beside a loop, a late init) are kept.
#![allow(clippy::field_reassign_with_default, clippy::explicit_counter_loop, clippy::needless_late_init)]

use super::*;
use crate::song_feel::effective_energy;

impl Musician {
    /// `kit::sendOnChange (kitCtx(), sent, want, key, vel, at, lenMs)` with `sent` one of the musician's own fields:
    /// the field is read and written back around the call (the C++ passes a reference to the member).
    fn og_send_on_change(&mut self, which: OgSent, want: i32, key: i32, vel: i32, at: f64, len_ms: f64) {
        let ms = self.ppq_per_ms();
        let now = self.current_ppq;
        let mut sent = match which {
            OgSent::Reg => self.og_reg_sent,
            OgSent::LowerReg => self.og_lower_reg_sent,
        };
        {
            let mut s = SchedAt { q: &mut self.q, now };
            let mut c = kit::DriverCtx { sink: &mut s, now, ms };
            kit::send_on_change(&mut c, &mut sent, want, key, vel, at, len_ms);
        }
        match which {
            OgSent::Reg => self.og_reg_sent = sent,
            OgSent::LowerReg => self.og_lower_reg_sent = sent,
        }
    }

    /// `int organNextEnergy (int& arrPos)`
    pub(crate) fn organ_next_energy(&mut self, arr_pos: &mut i32, cx: &mut Ctx<'_>) -> i32 {
        let Some(c) = self.cur else { return -1 };
        let Some(song) = cx.song else { return -1 };
        if self.live_mode || song.entries.is_empty() {
            return -1;
        }
        let n = i32::try_from(song.entries.len()).unwrap_or(i32::MAX);
        for d in 1..=n {
            let e = self.entry_rel(d, cx);
            if e.arr_pos == c.arr_pos && self.jump_next_idx < 0 {
                continue;
            }
            let ns = i32::try_from(song.sections.len()).unwrap_or(i32::MAX);
            let Some(sec) = usize::try_from(clamp(e.section, 0, ns - 1)).ok().and_then(|k| song.sections.get(k)) else { return -1 };
            let sm = sec.musicians[usize::try_from(clamp(self.index, 0, 5)).unwrap_or(0)];
            if !sm.plays || self.kept_here(sec) {
                return -1;
            }
            let mut en = if sm.energy > 0 { clamp(sm.energy, 1, 4) } else { clamp(self.og_base_energy + sec.energy_shift + e.energy_add, 1, 4) };
            if sm.energy <= 0 {
                en = effective_energy(en, self.feel_shift_for(self.active_player(cx.lib), Some(sec), cx).energy_add); // the song's Density there
            }
            if self.og_nudge != 0 {
                en = clamp(en + self.og_nudge, 1, 4);
            }
            *arr_pos = e.arr_pos;
            return en;
        }
        -1
    }

    /// `int organRegKey (int who, int en) const`
    pub(crate) fn organ_reg_key(&self, who: i32, en: i32, lib: &LibraryData) -> i32 {
        let t = self.current_target(lib);
        let i = clamp(en, 1, 4) - 1;
        let row = usize::try_from(clamp(self.feel_for(who, lib).organ.registration, 0, 3)).unwrap_or(0);
        let r = t.og_registrations.get(row).map_or(-1, |rw| at_or_default(rw, i));
        if r >= 0 { r } else { at_or_default(&t.og_drawbar_keys, i) }
    }

    /// `void organLowerStep (int lowCh)`
    pub(crate) fn organ_lower_step(&mut self, low_ch: i32, lib: &LibraryData) {
        let e = clamp(self.energy, 1, 4);
        if e == self.og_lower_reg_sent {
            return;
        }
        let ms = self.ppq_per_ms();
        self.q.schedule_ch = low_ch;
        let key = self.organ_lower_reg_key(e, lib);
        let vel = self.current_target(lib).og_note_vel;
        let at = self.og_low_first_on - 30.0 * ms;
        self.og_send_on_change(OgSent::LowerReg, e, key, vel, at, 5.0);
        self.q.schedule_ch = 0;
    }

    /// `void organLowerFired (int d1, int flags)`
    pub(crate) fn organ_lower_fired(&mut self, d1: i32, flags: i32, lib: &LibraryData) {
        if self.driver_is(Driver::Organ, lib) && (flags & 8192) != 0 {
            let en = self.organ_lower_reg_energy(d1, lib);
            if en != 0 {
                self.og_lower_reg_fired = en;
            }
        }
    }

    /// `int organRegEnergy (int who, int key) const`
    pub(crate) fn organ_reg_energy(&self, who: i32, key: i32, lib: &LibraryData) -> i32 {
        for k in 0..4 {
            if self.organ_reg_key(who, k + 1, lib) == key {
                return k + 1;
            }
        }
        0
    }

    /// `void organStep (int who, int pass, int progBar, float64 stepPpq)`
    pub(crate) fn organ_step(&mut self, who: i32, pass: i32, prog_bar: i32, step_ppq: f64, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let o = &self.feel_for(who, lib).organ;
        let ms = self.ppq_per_ms();
        let e = clamp(self.energy, 1, 4);
        let pre = self.og_first_on - 30.0 * ms;
        let arr_pos = self.cur.map_or(-1, |c| c.arr_pos);
        let fast_at = |m: &Self, ps: i32, en: i32, ap: i32, cx: &Ctx<'_>| en >= 4 || (en == 3 && m.rand01(ps, ap, 1951, who, cx) < o.leslie_fast);
        // The chorus goes once per pass of the arrangement (the song path's pass counts its positions).
        let loop_ = match (self.cur, cx.song) {
            (Some(_), Some(song)) if !self.live_mode => self.floor_div(pass - arr_pos, max(1, song.arrangement_length)),
            _ => pass,
        };

        // A section's first step: registration, Leslie, percussion (each only on change), the chorus once per pass.
        let sec_key = self.organ_sec_key(pass);
        if sec_key != self.og_sec_key {
            self.og_sec_key_prev = self.og_sec_key;
            self.og_sec_key = sec_key;
            self.og_sec_at = step_ppq;
            if t.og_chorus_cc >= 0 && loop_ != self.og_chorus_pass {
                self.og_chorus_pass_prev = self.og_chorus_pass;
                self.og_chorus_pass = loop_;
                self.og_chorus_at = pre;
                self.schedule(1, t.og_chorus_cc, at_or_default(&t.og_chorus_values, clamp(o.chorus, 0, 6)), pre, 0.0f32);
            }
            let key = self.organ_reg_key(who, e, lib);
            self.og_send_on_change(OgSent::Reg, e, key, t.og_note_vel, pre, 5.0);
            let les = if fast_at(self, pass, e, arr_pos, cx) { 127 } else { 0 };
            if les == 127 {
                self.og_kick_back = -1.0; // fast here anyway: a kick stays
            }
            if t.og_leslie_cc >= 0 && les != self.og_leslie && !(les == 0 && self.og_kick_back >= 0.0) {
                // a kick's own return slows it
                self.schedule(1, t.og_leslie_cc, les, pre, 0.0f32);
                self.og_leslie = les;
            }
            let perc = if o.percussion > 0.0f32 && e >= 2 { 127 } else { 0 };
            if t.og_perc_cc >= 0 && perc != self.og_perc {
                self.schedule(1, t.og_perc_cc, perc, pre, 0.0f32);
                self.og_perc = perc;
            }
        }

        // Natural: the section's last bar, into a louder section: the fast Leslie a bar early.
        let song_opt = if self.cur.is_some() && !self.live_mode { cx.song } else { None };
        let nat = self.expressive() && song_opt.is_some();
        let bar_l = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
        let bar_start = self.bar_floor(step_ppq, bar_l);

        // Natural registration moves inside a section (vocabulary 2.2; the B-3X moves its drawbars under a sounding
        // chord): an Energy change mid-section is pressed at the next chord change; a build (a section >= 8 bars at
        // Energy >= 3, or a Build section, with the player's chance) one ladder step up at the half-way bar (the top
        // for a Build section's last quarter) on the held chord's second beat. The next section's start (or its
        // look-ahead) puts the section's own registration back. Not in the last bar once the look-ahead decided.
        if nat && sec_key != self.og_early_key && self.og_reg_sent >= 1 {
            let mut bars = 1;
            let bar = self.section_bar_of(step_ppq, &mut bars, cx);
            let song = song_opt.map_or(&[][..], |s| s.sections.as_slice());
            let ns = i32::try_from(song.len()).unwrap_or(i32::MAX);
            let shape = usize::try_from(clamp(self.entry_rel(0, cx).section, 0, ns - 1)).ok().and_then(|k| song.get(k)).map_or(0, |s| s.shape);
            // The move's point in the bar: 4/4 beat 2; 12-step meters the half-bar (halfStep: 6/8's 2nd dotted beat, 3/4's 1.5).
            let move_at = if self.meter == Meter::M44 { 1.0 } else { f64::from(half_step(self.meter)) * 0.25 };
            let mut want = e;
            let mut built = false;
            if bar * 2 >= bars
                && self.cur_end_ppq > bar_start + move_at + 1.0e-6
                && (shape == 1 || (bars >= 8 && e >= 3))
                && self.rand01(pass, arr_pos, 1968, who, cx) < o.build
            {
                let up = if shape == 1 && bar * 4 >= 3 * bars { 4 } else { min(4, e + 1) };
                if up > want {
                    want = up;
                    built = true;
                }
            }
            if want != self.og_reg_sent {
                let mut at = -1.0;
                if built {
                    at = max(bar_start + move_at, max(pre, self.current_ppq)); // the held chord's beat 2 (half-bar)
                } else if step_ppq < self.cur_start_ppq + 1.0 - 1.0e-9 {
                    at = max(pre, self.current_ppq); // a chord change
                }
                if at >= 0.0 {
                    let key = self.organ_reg_key(who, want, lib);
                    self.og_send_on_change(OgSent::Reg, want, key, t.og_note_vel, at, 5.0);
                }
            }
        }

        // Natural Leslie kicks (vocabulary 2.3), with the player's chance, at most one per 4 bars, below Energy 4: fast
        // at the start of a turnaround bar (the last bar before a change to the I: a section's end, or the V going to
        // the I), of the bar before a same-Energy section's downbeat (a louder one has the early ramp below), or at a
        // fill; slow again two bars later unless the section there plays it fast.
        if t.og_leslie_cc >= 0 {
            let bar_no = ((step_ppq - self.bar_origin) / bar_l + 1.0e-9).floor() as i64;
            // The return: 30 ms before the bar it is due on when that bar is in this section (else at the first step
            // from it on); also after a switch to Classic.
            let mut due_here = self.og_kick_back >= 0.0 && step_ppq >= self.og_kick_back - bar_l - 1.0e-9 && song_opt.is_some();
            if due_here && let Some(c) = self.cur {
                let mut sec_end = self.cur_end_ppq;
                for d in 1..c.section_chords - c.chord_index {
                    sec_end += f64::from(self.entry_rel(d, cx).len_half) * bar_l * 0.5;
                }
                due_here = self.og_kick_back < sec_end - 1.0e-6;
            }
            if self.og_kick_back >= 0.0 && (due_here || step_ppq >= self.og_kick_back - 1.0e-9) {
                self.og_kick_back_armed = self.og_kick_back;
                self.og_kick_back = -1.0;
                if e < 4 && !fast_at(self, pass, e, arr_pos, cx) && self.og_leslie != 0 {
                    let at = max(if due_here { self.og_kick_back_armed - 30.0 * ms } else { pre }, self.current_ppq);
                    self.schedule(1, t.og_leslie_cc, 0, at, 0.0f32);
                    self.og_leslie = 0;
                    self.og_kick_back_sent_at = at;
                }
            }
            if nat && self.og_leslie != 127 && e < 4 && bar_no - self.og_kick_bar >= 4 {
                let mut kick = false;
                if let Some(c) = self.cur
                    && (self.cur_end_ppq - (bar_start + bar_l)).abs() < 1.0e-6
                    && step_ppq < bar_start + 1.0 - 1.0e-9
                    && self.jump_next_idx < 0
                {
                    let sec_end = c.chord_index == c.section_chords - 1;
                    if self.chord_for_bar(prog_bar + 1, cx).degree == 0 && (sec_end || self.chord_for_bar(prog_bar, cx).degree == 4) {
                        kick = true;
                    }
                    let mut np = -1;
                    if sec_end && self.organ_next_energy(&mut np, cx) == e {
                        kick = true;
                    }
                }
                let sib = self.floor_to_int((step_ppq - bar_start) / max(1.0e-6, f64::from(self.tick_ppq(lib))) + 1.0e-6);
                if !kick && sib >= fill_zone_start(self.meter) {
                    let change = !self.same_chord(self.chord_for_bar(prog_bar, cx), self.chord_for_bar(prog_bar + 1, cx));
                    if self.fill_active(who, pass, prog_bar, change, false, cx, out) {
                        kick = true;
                    }
                }
                if kick && self.rand01(pass, bar_no as i32, 1969, who, cx) < o.leslie_kick {
                    let at = max(pre, self.current_ppq);
                    self.schedule(1, t.og_leslie_cc, 127, at, 0.0f32);
                    self.og_leslie = 127;
                    self.og_kick_bar_prev = self.og_kick_bar;
                    self.og_kick_bar = bar_no;
                    self.og_kick_at = at;
                    self.og_kick_back = bar_start + 2.0 * bar_l;
                }
            }
        }
        // The section's last bar (both feels): the next section's registration goes 30 ms before its downbeat, so its
        // notes keep their time (the delayed start above is only the fallback: song start, Live, a jump). Natural: into
        // a louder section, the fast Leslie a bar early.
        let ahead = song_opt.is_some() && self.jump_next_idx < 0;
        if ahead
            && sec_key != self.og_early_key
            && let Some(c) = self.cur
            && let Some(song) = song_opt
        {
            let mut bars = 1;
            let bar = self.section_bar_of(step_ppq, &mut bars, cx);
            if bar == bars - 1 {
                self.og_early_key_prev = self.og_early_key;
                self.og_early_key = sec_key;
                self.og_early_at = step_ppq;
                let mut next_pos = -1;
                let ne = self.organ_next_energy(&mut next_pos, cx);
                let mut sec_end = self.cur_end_ppq;
                let half = f64::from(self.get_bar_length_ppq()) * 0.5;
                for d in 1..c.section_chords - c.chord_index {
                    sec_end += f64::from(self.entry_rel(d, cx).len_half) * half;
                }
                if ne >= 1 && sec_end - 30.0 * ms > self.current_ppq {
                    let key = self.organ_reg_key(who, ne, lib);
                    self.og_send_on_change(OgSent::Reg, ne, key, t.og_note_vel, sec_end - 30.0 * ms, 5.0);
                }
                // The next section's own pass number (the song path's pass counts arrangement positions).
                let len = max(1, song.arrangement_length);
                let next_pass = pass - arr_pos + next_pos + (if next_pos <= arr_pos { len } else { 0 });
                if nat && ne > e && fast_at(self, next_pass, ne, next_pos, cx) && t.og_leslie_cc >= 0 && self.og_leslie != 127 {
                    self.schedule(1, t.og_leslie_cc, 127, pre, 0.0f32);
                    self.og_leslie = 127;
                }
            }
        }

        // Palm smear (vocabulary 2.4), Natural, with the player's chance: on a section's downbeat at Energy >= 3 (or a chord
        // change on beat 1 at Energy 4), 5 or more white keys from a 4th below the chord's top note up to it, 14 ms apart (18 at
        // Energy 3), 40 ms each, the chord on the beat; the swell +10 for the smear. Never in the first bar after a Break; the
        // chord's own keys are left out (a smear note's release would cut them); not when it would start before now.
        if self.perf.break_at >= 0.0 {
            self.og_break_end = max(self.og_break_end, self.perf.break_at + bar_l);
        }
        let mut smear_on = false;
        let mut smear_t0 = 0.0f64;
        let mut smear_skip = -1.0e18f64;
        if nat
            && e >= 3
            && o.smear > 0.0f32
            && self.og_folded_n > 0
            && let Some(c) = self.cur
        {
            let on_bar = (step_ppq - bar_start).abs() < 1.0e-6 && (self.cur_start_ppq - bar_start).abs() < 1.0e-6;
            let chord_down = e == 4 && c.chord_index > 0 && !self.same_chord(self.chord_for_bar(prog_bar - 1, cx), self.chord_for_bar(prog_bar, cx));
            let near_break =
                self.og_break_end > -1.0e17 && bar_start > self.og_break_end - bar_l - 1.0e-6 && bar_start < self.og_break_end + bar_l - 1.0e-6;
            let smear_key = i64::from(pass) * 65536 + i64::from(prog_bar);
            if on_bar
                && (c.chord_index == 0 || chord_down)
                && !near_break
                && self.og_smear_bar != smear_key
                && self.rand01(pass, prog_bar, 1970, who, cx) < o.smear
            {
                let mut top = 0;
                for j in 0..self.og_folded_n {
                    top = max(top, self.og_folded.val(j));
                }
                let mut keys: CArr<i32, 16> = CArr::default();
                let mut n = 0;
                let mut p2 = top - 1;
                while p2 >= 36 && p2 >= top - 16 && n < 7 {
                    let pc = self.positive_modulo(p2, 12);
                    if !(pc == 0 || pc == 2 || pc == 4 || pc == 5 || pc == 7 || pc == 9 || pc == 11) {
                        p2 -= 1;
                        continue;
                    }
                    let mut in_chord = false;
                    for j in 0..self.og_folded_n {
                        if self.og_folded.val(j) == p2 {
                            in_chord = true;
                        }
                    }
                    if in_chord {
                        p2 -= 1;
                        continue;
                    }
                    if p2 < top - 5 && n >= 5 {
                        break;
                    }
                    keys.set(n, p2);
                    n += 1;
                    p2 -= 1;
                }
                let gap = (if e == 4 { 14.0 } else { 18.0 }) * ms;
                if self.og_smear_skip_for == smear_key {
                    smear_skip = self.og_smear_skip_before;
                }
                let mut lost = n < 5;
                if smear_skip < -1.0e17 {
                    for i in 0..n {
                        if self.og_first_on - f64::from(n - i) * gap < self.current_ppq + 5.5 * ms {
                            lost = true;
                        }
                    }
                }
                if !lost {
                    smear_on = true;
                    smear_t0 = self.og_first_on - f64::from(n) * gap;
                    self.og_smear_bar = smear_key;
                    self.og_smear_end = self.og_first_on;
                    self.og_smear_skip_for = K_OG_NONE;
                    self.q.schedule_flags = 0;
                    for i in 0..n {
                        // keys[] runs high to low: the nearest key to the downbeat is the highest
                        let at2 = self.og_first_on - f64::from(i + 1) * gap;
                        if at2 >= smear_skip {
                            self.schedule(0, keys.val(i), t.og_note_vel, at2, (40.0 * ms) as f32);
                        }
                    }
                }
            }
        }

        // The swell.
        const BAND_LO: [i32; 4] = [40, 55, 75, 90];
        const BAND_HI: [i32; 4] = [70, 90, 110, 120];
        let mut level =
            clamp(40.0f32 + 80.0f32 * self.og_loud / 127.0f32, at_or_default(&BAND_LO, e - 1) as f32, at_or_default(&BAND_HI, e - 1) as f32);
        if nat && let Some(c) = self.cur {
            let tick = f64::from(self.tick_ppq(lib));
            let last_beat = step_ppq >= self.cur_end_ppq - 1.0 - 1.0e-6 && step_ppq < self.cur_end_ppq;
            let frac = clamp((step_ppq + tick - (self.cur_end_ppq - 1.0)) / 1.0, 0.0, 1.0) as f32;
            let section_end = c.chord_index == c.section_chords - 1;
            if last_beat && section_end && self.rand01(pass, c.arr_pos, 1952, who, cx) < o.swell {
                level += 15.0f32 * frac;
            } else if last_beat && self.is_phrase_end(prog_bar) {
                level -= 12.0f32 * frac;
            }
        }
        if t.og_expr_cc < 0 {
            return;
        }
        let mut want = clamp(level.round() as i32, 0, 127);
        let at = self.og_first_on - 5.0 * ms;
        if smear_on {
            // +10 from 5 ms before the smear's first key; the chord's own level 5 ms before the chord
            if smear_t0 - 5.0 * ms >= smear_skip {
                self.schedule(1, t.og_expr_cc, min(127, want + 10), smear_t0 - 5.0 * ms, 0.0f32);
                self.og_expr_level = min(127, want + 10);
                self.og_expr_at = smear_t0 - 5.0 * ms;
            }
            if at >= smear_skip && at > self.og_expr_at {
                self.schedule(1, t.og_expr_cc, want, at, 0.0f32);
                self.og_expr_level = want;
                self.og_expr_at = at;
            }
            return;
        }
        if self.og_expr_level >= 0 {
            if at <= self.og_expr_at {
                return;
            }
            let max_move = (15.0 * (at - self.og_expr_at) + 1.0e-9).floor() as i32;
            want = clamp(want, self.og_expr_level - max_move, self.og_expr_level + max_move);
            if (want - self.og_expr_level).abs() < 3 {
                return;
            }
        }
        self.schedule(1, t.og_expr_cc, want, at, 0.0f32);
        self.og_expr_level = want;
        self.og_expr_at = at;
    }

    /// `void organFired (int kind, int d1, int d2, int flags, float64 at)`
    pub(crate) fn organ_fired(&mut self, kind: i32, d1: i32, d2: i32, flags: i32, at: f64, lib: &LibraryData) {
        if !self.driver_is(Driver::Organ, lib) {
            return;
        }
        let t = self.current_target(lib);
        if kind == 1 {
            if d1 == t.og_expr_cc {
                self.og_expr_fired = d2;
                self.og_expr_fired_at = at;
            }
            if d1 == t.og_leslie_cc {
                self.og_leslie_fired = d2;
            }
            if d1 == t.og_perc_cc {
                self.og_perc_fired = d2;
            }
        } else if kind == 0 && (flags & 8192) != 0 {
            let en = self.organ_reg_energy(self.active_player(lib), d1, lib);
            if en != 0 {
                self.og_reg_fired = en;
            }
        }
    }

    /// `void organDropFrom (float64 cut)`
    pub(crate) fn organ_drop_from(&mut self, cut: f64, lib: &LibraryData) {
        if !self.driver_is(Driver::Organ, lib) {
            return;
        }
        let t = self.current_target(lib);
        self.og_expr_level = self.og_expr_fired;
        self.og_expr_at = self.og_expr_fired_at;
        self.og_leslie = self.og_leslie_fired;
        self.og_perc = self.og_perc_fired;
        self.og_reg_sent = self.og_reg_fired;
        self.og_lower_reg_sent = self.og_lower_reg_fired;
        let mut les_at = -1.0e18f64;
        let mut perc_at = -1.0e18f64;
        let mut reg_at = -1.0e18f64;
        let mut low_reg_at = -1.0e18f64;
        let mut chorus_queued = false;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if !q.used {
                continue;
            }
            if q.kind == 1 {
                if q.d1 == t.og_expr_cc && q.on_ppq >= self.og_expr_at {
                    self.og_expr_level = q.d2;
                    self.og_expr_at = q.on_ppq;
                }
                if q.d1 == t.og_leslie_cc && q.on_ppq >= les_at {
                    self.og_leslie = q.d2;
                    les_at = q.on_ppq;
                }
                if q.d1 == t.og_perc_cc && q.on_ppq >= perc_at {
                    self.og_perc = q.d2;
                    perc_at = q.on_ppq;
                }
                if q.d1 == t.og_chorus_cc {
                    chorus_queued = true;
                }
            } else if q.kind == 0 && (q.flags & 8192) != 0 && q.ch > 0 {
                if q.on_ppq >= low_reg_at {
                    let en = self.organ_lower_reg_energy(q.d1, lib);
                    if en != 0 {
                        self.og_lower_reg_sent = en;
                        low_reg_at = q.on_ppq;
                    }
                }
            } else if q.kind == 0 && (q.flags & 8192) != 0 && q.on_ppq >= reg_at {
                let en = self.organ_reg_energy(self.active_player(lib), q.d1, lib);
                if en != 0 {
                    self.og_reg_sent = en;
                    reg_at = q.on_ppq;
                }
            }
        }
        // A step made again whose events (30 / 5 ms ahead of it) already went out can send the same value once more
        // (registration, Leslie, chorus): harmless; the state above keeps it from sending a wrong one.
        if self.og_sec_at >= cut {
            self.og_sec_key = self.og_sec_key_prev;
            self.og_sec_at = -1.0e18;
        }
        if self.og_early_at >= cut {
            self.og_early_key = self.og_early_key_prev;
            self.og_early_at = -1.0e18;
        }
        if self.og_chorus_at >= cut && !chorus_queued {
            self.og_chorus_pass = self.og_chorus_pass_prev;
            self.og_chorus_at = -1.0e18;
        }
        // A Leslie kick that went is decided again (its return with it); a return that went is due again.
        if self.og_smear_bar != K_OG_NONE && self.og_smear_end >= cut - 1.0e-9 {
            self.og_smear_skip_for = self.og_smear_bar;
            self.og_smear_skip_before = cut;
            self.og_smear_bar = K_OG_NONE;
        }
        if self.og_kick_at >= cut {
            self.og_kick_bar = self.og_kick_bar_prev;
            self.og_kick_bar_prev = -1000000;
            self.og_kick_at = -1.0e18;
            self.og_kick_back = -1.0;
        } else if self.og_kick_back_sent_at >= cut {
            self.og_kick_back = self.og_kick_back_armed;
            self.og_kick_back_sent_at = -1.0e18;
        }
    }

    /// `void organStop ()`
    pub(crate) fn organ_stop(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        if self.driver_is(Driver::Organ, lib) && self.current_target(lib).og_expr_cc >= 0 && (self.og_expr_level >= 0 || self.og_expr_fired >= 0) {
            self.send_midi(0xB0 + self.midi_channel0, self.current_target(lib).og_expr_cc, 0, cx, out);
        }
        self.og_expr_level = -1;
        self.og_expr_fired = -1;
        self.og_expr_at = -1.0e18;
        self.og_expr_fired_at = -1.0e18;
        self.og_reg_sent = -1;
        self.og_reg_fired = -1;
        self.og_lower_reg_sent = -1;
        self.og_lower_reg_fired = -1;
        if self.og_lower_ch_used >= 0 && self.og_lower_ch_used != self.midi_channel0 {
            self.send_midi(0xB0 + self.og_lower_ch_used, 123, 0, cx, out); // the lower manual too
        }
        self.og_lower_ch_used = -1;
        self.og_leslie = self.og_leslie_fired;
        self.og_perc = self.og_perc_fired;
        self.og_sec_key = K_OG_NONE;
        self.og_sec_key_prev = K_OG_NONE;
        self.og_early_key = K_OG_NONE;
        self.og_early_key_prev = K_OG_NONE;
        self.og_sec_at = -1.0e18;
        self.og_early_at = -1.0e18;
        self.og_chorus_at = -1.0e18;
        self.og_chorus_pass = -1000000;
        self.og_chorus_pass_prev = -1000000;
        self.og_kick_bar = -1000000;
        self.og_kick_bar_prev = -1000000;
        self.og_kick_at = -1.0e18;
        self.og_kick_back_sent_at = -1.0e18;
        self.og_kick_back = -1.0;
        self.og_kick_back_armed = -1.0;
        self.og_smear_bar = K_OG_NONE;
        self.og_smear_skip_for = K_OG_NONE;
        self.og_smear_end = -1.0e18;
        self.og_break_end = -1.0e18;
    }

    /// `void organExpressionOff ()`
    pub(crate) fn organ_expression_off(&mut self, lib: &LibraryData) {
        if !self.driver_is(Driver::Organ, lib) {
            return;
        }
        let cc = self.current_target(lib).og_expr_cc;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if cc >= 0 && q.used && q.kind == 1 && q.d1 == cc {
                self.q.queue.at_mut(i).used = false;
            }
        }
        self.og_expr_level = self.og_expr_fired;
        self.og_expr_at = self.og_expr_fired_at;
    }
}

/// Which of the musician's registration fields `og_send_on_change` updates (`ogRegSent` / `ogLowerRegSent`).
#[derive(Clone, Copy)]
enum OgSent {
    Reg,
    LowerReg,
}

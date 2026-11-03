//! `MusicianGuitar.cpp`: Musician: guitar (Ample Sound guitars, guitar role), ported line by line.
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.

#![allow(clippy::needless_late_init)] // the C++ shape: declared, then set in an if / else

use super::*;
use crate::ample_slots::{ample_chord_key, ample_slot_for};
use crate::guitar_shapes as guitar;
use crate::library::ample_strums;

impl Musician {
    /// `void amplePushPrev (int note, float64 on)`
    pub(crate) fn ample_push_prev(&mut self, note: i32, on: f64) {
        if self.ample_hist_count == self.ample_hist.len() as i32 {
            for i in 1..self.ample_hist_count {
                let v = at_or_default(&self.ample_hist, i);
                put_at(&mut self.ample_hist, i - 1, v);
            }
            self.ample_hist_count -= 1;
        }
        put_at(&mut self.ample_hist, self.ample_hist_count, (note, on));
        self.ample_hist_count += 1;
        self.ample_prev_note = note;
        self.ample_prev_on = on;
    }

    /// `void ampleDropFrom (float64 cut)`
    pub(crate) fn ample_drop_from(&mut self, cut: f64) {
        while self.ample_hist_count > 0 && at_or_default(&self.ample_hist, self.ample_hist_count - 1).1 >= cut {
            self.ample_hist_count -= 1;
        }
        self.ample_prev_note = if self.ample_hist_count > 0 { at_or_default(&self.ample_hist, self.ample_hist_count - 1).0 } else { -1 };
        self.ample_prev_on = if self.ample_hist_count > 0 { at_or_default(&self.ample_hist, self.ample_hist_count - 1).1 } else { -1.0e9 };
        self.ample_art = -1;
        self.ample_chord_sent = -1;
        if self.ample_legato_pending {
            self.ample_release_legato(cut); // a pressed legato key must not ring on to its provisional end (verified, final review: released at the cut)
        }
        self.ample_legato_bar = -1000;
        self.ample_legato_pending = false;
        if self.ample_slide_out_at >= cut {
            self.ample_slide_out_bar = self.ample_slide_out_prev_bar;
            self.ample_slide_out_at = -1.0e9;
        }
        if self.ample_scratch_at >= cut {
            self.ample_scratch_bar = -1000;
            self.ample_scratch_at = -1.0e9;
        }
        if self.ample_vib_end >= cut {
            // Its CC1 0 was dropped: let the vibrato go at the cut (if it had started; else its CC1 went too).
            if self.ample_vib_start < cut {
                self.schedule(1, 1, 0, cut, 0.0f32);
            }
            self.ample_vib_end = -1.0e9;
        }
        if self.ample_last_stroke_at >= cut {
            self.ample_last_stroke_at = cut; // played just before: no slide-in out of a rest that wasn't one
        }
    }

    /// `void ampleVibratoOff ()`
    pub(crate) fn ample_vibrato_off(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if self.ample_vib_end < -1.0e8 {
            return;
        }
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 1 && q.d1 == 1 {
                self.q.queue.at_mut(i).used = false;
            }
        }
        self.send_midi(0xB0 + self.midi_channel0, 1, 0, cx, out);
        self.ample_vib_start = -1.0e9;
        self.ample_vib_end = -1.0e9;
    }

    /// `void ampleReleaseLegato (float64 off)`
    pub(crate) fn ample_release_legato(&mut self, off: f64) {
        let floor = 0.01; // the shortest note the queue sends (dispatch, fdMoveOff)
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 0 && q.d1 == self.ample_legato_key && (q.flags & 8192) != 0 {
                if off - q.on_ppq < floor {
                    self.q.queue.at_mut(i).used = false;
                } else {
                    self.q.queue.at_mut(i).dur_ppq = (off - q.on_ppq) as f32;
                }
                return;
            }
        }
        self.fd_move_off(self.ample_legato_key, off);
    }

    /// `float64 ampleAt (float64 onPpq)`
    pub(crate) fn ample_at(&mut self, on_ppq: f64, lib: &LibraryData) -> f64 {
        let lead = self.ample_lead(lib);
        if lead <= 0.0 {
            return on_ppq;
        }
        max(min(on_ppq, max(self.current_ppq, self.ample_step_at - 0.095)), on_ppq - lead)
    }

    /// `float64 ampleKeyAt (float64 keyPpq, float64 noteAt)`
    pub(crate) fn ample_key_at(&mut self, key_ppq: f64, note_at: f64, lib: &LibraryData) -> f64 {
        if self.ample_lead(lib) <= 0.0 {
            return key_ppq;
        }
        max(key_ppq, min(note_at - 0.5 * self.ppq_per_ms(), self.ample_step_at - 0.099))
    }

    /// `void ampleSchedule (int kind, int d1, int d2, float64 at, float durPpq, int extra)`
    pub(crate) fn ample_schedule(&mut self, kind: i32, d1: i32, d2: i32, at: f64, dur_ppq: f32, extra: i32, lib: &LibraryData) {
        let keep = self.q.schedule_flags;
        if self.ample_release(lib) && kind == 0 {
            let mut fl = extra;
            if (keep & 8192) == 0
                && self.ample_next_chord_at >= 0.0
                && (max(at, self.current_ppq) + f64::from(dur_ppq) - self.ample_next_chord_at).abs() < f64::from(self.tick_ppq(lib)) * 0.5
            {
                fl |= K_AMPLE_CHANGE_FLAG;
            }
            self.q.schedule_flags = keep | fl;
        }
        self.schedule(kind, d1, d2, at, dur_ppq);
        self.q.schedule_flags = keep;
    }

    /// `void ampleEnsureSetup (int who, float64 at, bool force)`
    pub(crate) fn ample_ensure_setup(&mut self, who: i32, at: f64, force: bool, lib: &LibraryData) {
        let t = self.current_target(lib);
        let a = &self.feel_for(who, lib).ample;
        let strums = ample_strums(self.feel_for(who, lib), self.ample_mode, self.energy);
        let sig = ((((self.target * 256 + who) * 2 + (if strums { 1 } else { 0 })) * 2 + a.position_mode) * 2
            + (if a.open_strings_first { 1 } else { 0 }))
            * 2
            + self.ample_chords;
        if sig == self.ample_sig && !force {
            return;
        }
        self.ample_sig = sig;
        self.ample_ready_at = -1.0e9;
        self.ample_forget();
        // Below-range taps: ampleSchedule adds no flag to a key outside the range, so these are plain kit taps.
        let ample_chords = self.ample_chords;
        let ms = self.ppq_per_ms();
        let now = self.current_ppq;
        let mut s = SchedAt { q: &mut self.q, now };
        let mut c = kit::DriverCtx { sink: &mut s, now, ms };
        kit::tap_key(&mut c, t.am_strum_mode_key, if strums { t.am_on_vel } else { t.am_off_vel }, at, 3.0);
        if strums {
            kit::tap_key(&mut c, t.am_chord_mode_key, if ample_chords == 1 { t.am_off_vel } else { t.am_on_vel }, at, 3.0); // Slots: Chord Detect off
        }
        kit::tap_key(&mut c, t.am_position_key, if a.position_mode == 0 { t.am_on_vel } else { t.am_off_vel }, at, 3.0);
        kit::tap_key(&mut c, t.am_open_first_key, if a.open_strings_first { t.am_on_vel } else { t.am_off_vel }, at, 3.0);
    }

    /// `void ampleBegin (int who, int progBar, float64 stepPpq)`
    pub(crate) fn ample_begin(&mut self, who: i32, prog_bar: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let ms = self.ppq_per_ms();
        self.ample_step_at = step_ppq;
        let at = self.ample_at(max(step_ppq - self.ample_lead_ppq(), self.current_ppq), lib);
        let section_start = self.cur.is_some_and(|c| c.chord_index == 0) && step_ppq - f64::from(self.tick_ppq(lib)) + 0.0001 < self.cur_start_ppq;
        self.ample_ensure_setup(who, at, section_start, lib);
        self.ample_count = 0;
        if section_start {
            self.ample_slide_in_armed = true;
        }
        let ch = self.chord_for_bar(prog_bar, cx);
        if self.ample_release(lib) {
            // Where this chord gives way to a different one (its section entry's end, or the bar's).
            let nx = self.chord_for_bar(prog_bar + 1, cx);
            let len = self.chord_len_ppq();
            let end = if self.cur.is_some() { self.cur_end_ppq } else { (((step_ppq + 0.0001) / len).floor() + 1.0) * len };
            self.ample_next_chord_at = if self.same_chord(ch, nx) && ch.bass_pc == nx.bass_pc { -1.0 } else { self.ample_at_for(end, lib) };
        }
        if !ample_strums(self.feel_for(who, lib), self.ample_mode, self.energy) {
            return;
        }
        let t = self.current_target(lib);
        let a = &self.feel_for(who, lib).ample;
        if self.ample_chords == 1 {
            // Slots: one key, C1-B1 = Ample's chord slot 1-12, set by the user from the Strummer chord sheet.
            let ss = self.key_root * 2 + self.scale_mode;
            if ss != self.ample_slots_sig {
                build_ample_slots(&self.ample_song_chords, self.key_root, self.scale_mode, &mut self.ample_slot_table);
                self.ample_slots_sig = ss;
            }
            let slot = ample_slot_for(&self.ample_slot_table, &ample_chord_key(ch.root_pc, &ch.tones.v, ch.count, ch.bass_pc, self.key_root));
            let content = -2 - slot;
            if content == self.ample_chord_sent {
                return;
            }
            self.ample_chord_sent = content;
            let press = (8.0 * ms) as f32;
            let chord_at = at + 1.0 * ms;
            self.ample_schedule(0, t.am_chord_low + slot - 1, 80, chord_at, press, K_AMPLE_CHORD_KEY_FLAG, lib);
            self.ample_ready_at = chord_at + f64::from(press) + 1.0 * ms;
            self.ample_begin_legato(who, prog_bar, chord_at, cx);
            return;
        }
        // Chord detect: the root in the lowest octave (C1-B1), the other tones above it; a slash chord's bass
        // then goes down alone, after the chord is up ("pressing C1-B1 solely makes a slash chord based on current").
        // Natural with strumInversion: that chord tone lowest instead, the rest folded above ("different inversions
        // map to different positions").
        let inv = if self.expressive() && a.strum_inversion < min(ch.count, 4) { a.strum_inversion } else { 0 };
        let root_key = t.am_root_high - self.positive_modulo(t.am_root_high - (self.key_root + ch.root_pc), 12); // the octave at or below rootHigh
        let mut mask = 0;
        for i in 1..min(ch.count, 4) {
            mask |= 1 << self.positive_modulo(ch.tones.val(i), 12);
        }
        let bass_key = if ch.bass_pc >= 0 { t.am_root_high - self.positive_modulo(t.am_root_high - (self.key_root + ch.bass_pc), 12) } else { -1 };
        let content = ((root_key * 4096 + mask) * 129 + (bass_key + 1)) * 3 + inv;
        if content == self.ample_chord_sent {
            return;
        }
        self.ample_chord_sent = content;
        let press = (8.0 * ms) as f32;
        let chord_at = at + 1.0 * ms;
        let ck = K_AMPLE_CHORD_KEY_FLAG; // chord detect is a keyswitch to the ear: no release velocity
        if inv == 0 {
            self.ample_schedule(0, root_key, 80, chord_at, press, ck, lib);
            for i in 1..min(ch.count, 4) {
                let mut n = root_key + self.positive_modulo(ch.tones.val(i), 12);
                if n <= root_key {
                    n += 12;
                }
                if n <= t.am_chord_high {
                    self.ample_schedule(0, n, 80, chord_at, press, ck, lib);
                }
            }
        } else {
            let low_key = t.am_root_high - self.positive_modulo(t.am_root_high - (self.key_root + ch.root_pc + ch.tones.val(inv)), 12);
            self.ample_schedule(0, low_key, 80, chord_at, press, ck, lib);
            for i in 0..min(ch.count, 4) {
                if i == inv {
                    continue;
                }
                let mut n = low_key + self.positive_modulo(ch.tones.val(i) - ch.tones.val(inv), 12);
                if n <= low_key {
                    n += 12;
                }
                if n <= t.am_chord_high {
                    self.ample_schedule(0, n, 80, chord_at, press, ck, lib);
                }
            }
        }
        self.ample_ready_at = chord_at + f64::from(press) + 1.0 * ms;
        if bass_key >= 0 {
            let bass_at = chord_at + f64::from(press) + 4.0 * ms;
            self.ample_schedule(0, bass_key, 80, bass_at, press, ck, lib);
            self.ample_ready_at = bass_at + f64::from(press) + 1.0 * ms;
        }
        self.ample_begin_legato(who, prog_bar, chord_at, cx);
    }

    /// `void ampleBeginLegato (int who, int progBar, float64 chordAt)`
    pub(crate) fn ample_begin_legato(&mut self, who: i32, prog_bar: i32, chord_at: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let ms = self.ppq_per_ms();
        let t = self.current_target(lib);
        let a = &self.feel_for(who, lib).ample;
        // Strum legato (Strummer 1.7.1-2): F0 held across the change hammers / pulls off the strings that change,
        // E0 slides them. Only out of a chord strummed within the last beat, never on a section's first bar, one per bar.
        // Ample only moves the strings while the key is held as the chord switches: pressed here, ahead of the detect
        // keys, for up to a bar; the change's first stroke (ampleStrum) then sets its release.
        self.ample_legato_pending = false;
        if self.expressive() && (a.hammer_on > 0.0f32 || a.slides > 0.0f32) {
            let recent = chord_at - self.ample_last_stroke_at <= 4.0 * f64::from(self.tick_ppq(lib)) + 1.0e-6; // both as played
            let first_bar = match self.cur {
                // a push: into the next section?
                Some(c) => {
                    if prog_bar == self.cur_prog_bar {
                        c.chord_index == 0
                    } else {
                        c.chord_index == c.section_chords - 1
                    }
                }
                None => false,
            };
            if recent && !first_bar && prog_bar != self.ample_legato_bar {
                let r = self.rand01(self.step_pass, prog_bar, 1921, who, cx);
                let hammer = a.hammer_on * (if self.energy >= 3 { 1.3f32 } else { 1.0f32 });
                let key = if r < hammer {
                    at_or_default(&t.am_art, 5)
                } else if r < hammer + a.slides {
                    at_or_default(&t.am_art, 4)
                } else {
                    -1
                };
                if key >= 0 {
                    let key_at = self.ample_key_at(chord_at - 12.0 * ms, chord_at, lib);
                    self.q.schedule_flags = 8192;
                    self.ample_schedule(0, key, 100, key_at, (chord_at + self.perf.bar_len - key_at) as f32, 0, lib); // provisional: the bar
                    self.q.schedule_flags = 0;
                    self.ample_legato_bar = prog_bar;
                    self.ample_legato_pending = true;
                    self.ample_legato_chord_at = chord_at;
                    self.ample_legato_key = key;
                }
            }
        }
    }

    /// `void ampleAdd (int note, float vel, float64 on, float len, int flags, int hand)`
    pub(crate) fn ample_add(&mut self, note: i32, vel: f32, on: f64, len: f32, flags: i32, hand: i32) {
        if self.ample_count >= self.ample_notes.len() as i32 {
            return;
        }
        put_at(&mut self.ample_notes, self.ample_count, AmpleNote { note, vel, len, on, flags, hand });
        self.ample_count += 1;
    }

    /// `void ampleStrum (int who, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn ample_strum(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        if self.ample_count == 0 {
            return;
        }
        let lib = cx.lib;
        let t = self.current_target(lib);
        let a = &self.feel_for(who, lib).ample;
        let mut on = at_or_default(&self.ample_notes, 0).on;
        let mut loud = 0.0f32;
        let mut len = 0.0f32;
        let mut flags = 0;
        let mut lo = 0;
        let mut hi = 0;
        for i in 0..self.ample_count {
            let n = at_or_default(&self.ample_notes, i);
            on = min(on, n.on);
            loud = max(loud, n.vel);
            len = max(len, n.len);
            flags |= n.flags;
            if n.note < at_or_default(&self.ample_notes, lo).note {
                lo = i;
            }
            if n.note > at_or_default(&self.ample_notes, hi).note {
                hi = i;
            }
        }
        let gstep = self.positive_modulo(prog_bar, 2) * self.spb() + sib;
        if self.ample_count == 1 {
            if !a.bass_strings && at_or_default(&self.ample_notes, 0).hand == 0 {
                return; // bassStrings false: lone bass notes only; picked notes still play
            }
            let p = at_or_default(&self.ample_notes, 0).note;
            let mut s = 0; // the highest string whose open note is at or below it
            for k in 0..6 {
                if at_or_default(&t.am_open_strings, k) <= p {
                    s = k;
                }
            }
            let at = self.ample_at(on, lib);
            self.ample_schedule(
                0,
                at_or_default(&t.am_strings, s),
                clamp(loud as i32, 1, 127),
                at,
                max(0.02f32, len),
                if (flags & 3) != 0 { K_AMPLE_MUTED_FLAG } else { 0 },
                lib,
            );
            self.ample_last_stroke_at = self.ample_at(on, lib);
            return;
        }
        // The hand: down on the beat, up on the 16ths between; on the 8th off-beat it follows the roll
        // (the grid position, whatever the musician's Speed makes a step). 3/4 keeps the quarter's hand. 6/8 counts the 16th
        // within the dotted beat: down on its first eighth (0 / 6) and its third (4 / 10), up on the second and the 16ths between.
        let down;
        if self.meter == Meter::M68 {
            let b = step_ppq / 1.5;
            let p = self.positive_modulo(self.floor_to_int((b - (b + 1.0e-6).floor()) * 6.0 + 0.5), 6);
            down = p == 0 || p == 4;
        } else {
            let q = self.positive_modulo(self.floor_to_int((step_ppq - (step_ppq + 1.0e-6).floor()) * 4.0 + 0.5), 4);
            down = if q == 0 {
                true
            } else if q % 2 == 1 {
                false
            } else {
                at_or_default(&self.ample_notes, lo).on < at_or_default(&self.ample_notes, hi).on
            };
        }
        let variant = if self.ample_count >= 5 { 0 } else { 1 }; // full, or the treble strings
        let chank = self.feel_for(who, lib).voicing == 8;
        let muted = (flags & 3) != 0 || (if chank { loud < 105.0f32 } else { false }) || self.rand01(pass, gstep, 1311, prog_bar, cx) < a.muted;
        on = max(self.ample_at(on, lib), self.ample_ready_at); // never ahead of the chord it strums (both onset-led)
        if self.ample_legato_pending && muted && on - self.ample_legato_chord_at < self.perf.bar_len {
            on = max(on, self.current_ppq + 2.0 * self.ppq_per_ms()); // room to let the legato key go first
        }
        let ring = f64::from(max(0.02f32, len));
        if self.expressive() {
            // Slide-in strum (D#0, 1.7.3): a section's first stroke, or one after half a bar of rest. Harmonic strum
            // (C#0, 1.7.4): the Ending's final hit and a Break's last stroke. Pressed just before the stroke.
            if on - self.ample_last_stroke_at >= self.perf.bar_len * 0.5 - 1.0e-6 {
                self.ample_slide_in_armed = true;
            }
            let ms = self.ppq_per_ms();
            let key_at = self.ample_key_at(on - 10.0 * ms, on, lib);
            let last_hit = self.ample_final_hit || (self.perf.break_at >= 0.0 && (step_ppq - self.perf.break_at).abs() < 1.0e-6);
            self.q.schedule_flags = 8192;
            if self.ample_slide_in_armed && a.slide_in > 0.0f32 && self.rand01(pass, gstep, 1922, prog_bar, cx) < a.slide_in {
                self.ample_schedule(0, at_or_default(&t.am_art, 3), 100, key_at, (8.0 * ms) as f32, 0, lib);
            }
            if last_hit && a.harmonics > 0.0f32 && self.rand01(pass, gstep, 1923, prog_bar, cx) < a.harmonics {
                self.ample_schedule(0, at_or_default(&t.am_art, 1), 100, key_at, (8.0 * ms) as f32, 0, lib);
            }
            self.q.schedule_flags = 0;
        }
        self.ample_slide_in_armed = false;
        self.ample_last_stroke_at = on;
        if self.ample_legato_pending {
            // The legato key's release: 20 ms after the change's first stroke, or just before it when it's muted (no
            // hammering into a mute). No stroke within the bar: its provisional end releases it.
            self.ample_legato_pending = false;
            if on - self.ample_legato_chord_at < self.perf.bar_len {
                self.ample_release_legato(on + (if muted { -2.0 } else { 20.0 }) * self.ppq_per_ms());
            }
        }
        if muted && loud < 60.0f32 {
            self.ample_schedule(0, at_or_default(&t.am_mute_voices, gstep % 2), clamp(loud as i32, 1, 127), on, ring as f32, K_AMPLE_MUTED_FLAG, lib);
            return;
        }
        // Velocity is the stroke's weight: Ample plays more strings the harder it's hit. The player's own
        // dynamics (accents, the phrase's arc, Energy, humanize) go straight into it.
        let norm = clamp(loud / 100.0f32, 0.0f32, 1.0f32);
        let vel = clamp(
            (t.am_stroke_vel_low as f32 + norm * (t.am_stroke_vel_high - t.am_stroke_vel_low) as f32 + 0.5f32) as i32,
            t.am_stroke_vel_low,
            t.am_stroke_vel_high,
        );
        if t.am_strum_time_key >= 0 {
            // Strum speed (higher velocity = quicker sweep): upstrokes and accents quick, soft strokes lazy, and as loose
            // as the Humanize knob says (0% = the same stroke always sweeps the same).
            let weight = (vel - t.am_stroke_vel_low) as f32 / max(1, t.am_stroke_vel_high - t.am_stroke_vel_low) as f32;
            let mut sweep = (if down { 62.0f32 } else { 86.0f32 })
                + (weight - 0.6f32) * 50.0f32
                + self.rand_bell(gstep, pass, 1315, prog_bar, cx) * 9.0f32 * (self.humanize / 50.0f32); // Classic: frozen
            if self.expressive() {
                // Natural: the same shape inside the target's high band (a real strum is 10-60 ms; on Ample's scale that is a high velocity).
                let x = (if down { 0.35f32 } else { 0.65f32 })
                    + (weight - 0.6f32) * 0.5f32
                    + self.rand_bell(gstep, pass, 1315, prog_bar, cx) * 0.12f32 * (self.humanize / 50.0f32);
                sweep = t.am_strum_time_lo as f32 + (t.am_strum_time_hi - t.am_strum_time_lo) as f32 * clamp(x, 0.0f32, 1.0f32);
            }
            self.q.schedule_flags = 8192;
            let key_at = self.ample_key_at(on - 3.0 * self.ppq_per_ms(), on, lib);
            self.ample_schedule(0, t.am_strum_time_key, clamp(sweep as i32, 1, 127), key_at, (2.0 * self.ppq_per_ms()) as f32, 0, lib);
            self.q.schedule_flags = 0;
        }
        let stroke = t.am_strokes.get((if down { 0 } else { 1 }) + (if muted { 2 } else { 0 })).copied().unwrap_or_default();
        self.ample_schedule(0, at_or_default(&stroke, variant), vel, on, ring as f32, if muted { K_AMPLE_MUTED_FLAG } else { 0 }, lib);
        if a.strum_noise > 0.0f32 && self.rand01(pass, gstep, 1313, prog_bar, cx) < a.strum_noise {
            self.ample_schedule(
                0,
                at_or_default(&t.am_noise, (if down { 0 } else { 2 }) + gstep % 2),
                clamp((loud * 0.6f32) as i32, 1, 127),
                on,
                self.tick_ppq(lib) * 0.5f32,
                0,
                lib,
            );
        }
    }

    /// `int ampleHarmonicFor (int note)`
    pub(crate) fn ample_harmonic_for(&mut self, note: i32, lib: &LibraryData) -> i32 {
        let t = self.current_target(lib);
        let mut best = -1;
        for &s in &t.am_open_strings {
            for d in [12, 19, 24] {
                let h = s + d;
                if h < t.am_note_low || h > t.am_note_high || self.positive_modulo(h - note, 12) != 0 {
                    continue;
                }
                if best < 0 || (h - note).abs() < (best - note).abs() {
                    best = h;
                }
            }
        }
        best
    }

    /// `bool ampleHoldUntil (int note, float64 until)`
    pub(crate) fn ample_hold_until(&mut self, note: i32, until: f64) -> bool {
        for i in 0..64 {
            if self.active_slots.val(i)
                && self.active_channels.val(i) == self.midi_channel0
                && self.active_notes.val(i) == note
                && self.active_off_ppq.val(i) > self.current_ppq
            {
                self.active_off_ppq.set(i, max(self.active_off_ppq.val(i), until));
                return true;
            }
        }
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 0 && q.d1 == note && (q.flags & 8192) == 0 {
                self.q.queue.at_mut(i).dur_ppq = max(q.dur_ppq, (until - q.on_ppq) as f32);
                return true;
            }
        }
        false
    }

    /// `void ampleFinger (int who, int pass, int progBar, int sib)`
    pub(crate) fn ample_finger(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, cx: &mut Ctx<'_>) {
        if self.ample_count < 3 {
            return;
        }
        let lib = cx.lib;
        let ch = self.chord_for_bar(prog_bar, cx);
        let root_pc = self.positive_modulo(self.key_root + ch.root_pc, 12);
        let mut pcs: u16 = 0;
        for i in 0..ch.count {
            pcs = (u32::from(pcs) | 1u32 << self.positive_modulo(root_pc + ch.tones.val(i), 12)) as u16;
        }
        let mut outside = 0;
        for i in 0..self.ample_count {
            if (u32::from(pcs) >> self.positive_modulo(at_or_default(&self.ample_notes, i).note, 12) & 1) == 0 {
                outside += 1;
            }
        }
        if outside * 2 > self.ample_count {
            return;
        }
        let required = (1u32 << root_pc | (if ch.count > 1 { 1u32 << self.positive_modulo(root_pc + ch.tones.val(1), 12) } else { 0u32 })) as u16;
        let bass_pc = if ch.bass_pc >= 0 { self.positive_modulo(self.key_root + ch.bass_pc, 12) } else { root_pc };
        let t = self.current_target(lib);
        let capo = clamp(self.feel_for(who, lib).ample.capo, 0, 7);
        let r = self.rand01(pass, self.positive_modulo(prog_bar, 2) * self.spb() + sib, 1901, prog_bar, cx);
        let prev = self.guitar_prev;
        let shape = guitar::best(pcs, required, bass_pc, capo, &t.am_open_strings, if prev.valid { Some(&prev) } else { None }, r);
        if !shape.valid {
            return;
        }
        self.guitar_prev = shape;
        let ns = guitar::notes(&shape, &t.am_open_strings, capo);
        // The step's notes in time order: the shape's strings take their places, low string first.
        let mut was = self.ample_notes;
        let was_count = self.ample_count;
        if let Some(w) = was.get_mut(..usize::try_from(was_count).unwrap_or(0)) {
            w.sort_by(|a, b| a.on.partial_cmp(&b.on).unwrap_or(std::cmp::Ordering::Equal)); // stable: at most 16 notes here, and below its 32-element threshold MSVC's std::sort is an insertion sort (stable)
        }
        let mut k = 0;
        for s in 0..6 {
            let nss = at_or_default(&ns, s);
            if nss < 0 {
                continue;
            }
            let mut n = at_or_default(&was, min(k, was_count - 1));
            if k >= was_count {
                n.on = at_or_default(&was, was_count - 1).on + f64::from(k - was_count + 1) * 6.0 * self.ppq_per_ms(); // more strings: the roll goes on
            }
            n.note = nss;
            put_at(&mut self.ample_notes, k, n);
            k += 1;
        }
        self.ample_count = k;
    }

    /// `int ampleStringFor (int note, const guitar::Shape* shape, int capo, int& fret)`
    pub(crate) fn ample_string_for(&mut self, note: i32, shape: Option<&GShape>, capo: i32, fret: &mut i32, lib: &LibraryData) -> i32 {
        guitar::string_for(note, &self.current_target(lib).am_open_strings, capo, shape, fret)
    }

    /// `void amplePick (int who, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn ample_pick(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        if self.ample_count == 0 {
            return;
        }
        self.ample_finger(who, pass, prog_bar, sib, cx);
        let lib = cx.lib;
        let t = self.current_target(lib);
        let a = &self.feel_for(who, lib).ample;
        let ms = self.ppq_per_ms();
        let tick = self.tick_ppq(lib);
        let gstep = self.positive_modulo(prog_bar, 2) * self.spb() + sib;
        let single = self.ample_count == 1;
        let accent = self.accent_at(who, self.positive_modulo(sib, self.spb()), lib) >= 1.03f32;
        // The hand: 4/4 and 3/4 count the 16th in the quarter (3/4's backbeat is beats 2 and 3); 6/8 counts it in the dotted
        // beat (the backbeat is the second dotted beat).
        let on_beat;
        let backbeat;
        if self.meter == Meter::M68 {
            let b = step_ppq / 1.5;
            on_beat = self.positive_modulo(self.floor_to_int((b - (b + 1.0e-6).floor()) * 6.0 + 0.5), 6) == 0;
            backbeat = on_beat && self.positive_modulo(self.floor_to_int(b + 0.5), 2) == 1;
        } else {
            let q = self.positive_modulo(self.floor_to_int((step_ppq - (step_ppq + 1.0e-6).floor()) * 4.0 + 0.5), 4);
            on_beat = q == 0;
            backbeat = on_beat
                && (if self.meter == Meter::M34 {
                    self.positive_modulo(self.floor_to_int(step_ppq + 0.5), 3) != 0
                } else {
                    self.positive_modulo(self.floor_to_int(step_ppq + 0.5), 2) == 1
                });
        }

        // Natural FX (spec §10.4), percussive players only (chank, or fx >= .5): Slap on a 2/4 chord step at Energy 3+,
        // Scratch once in the bar leading into a new section (its last beat).
        let percussive = self.expressive() && a.fx > 0.0f32 && (self.feel_for(who, lib).voicing == 8 || a.fx >= 0.5f32);
        let mut slapped = false;
        if percussive && backbeat && self.ample_count >= 3 && self.energy >= 3 && self.rand01(pass, gstep, 1925, prog_bar, cx) < a.fx {
            let at = self.ample_at(at_or_default(&self.ample_notes, 0).on, lib);
            self.ample_schedule(0, t.am_slap, 90, at, tick * 0.5f32, 0, lib);
            slapped = true;
        }
        if percussive
            && self.pickup_now
            && prog_bar == self.pickup_prog_bar
            && prog_bar != self.ample_scratch_bar
            && sib >= fill_zone_start(self.meter)
        {
            self.ample_scratch_bar = prog_bar; // decided once per pickup bar
            self.ample_scratch_at = self.ample_at(at_or_default(&self.ample_notes, 0).on, lib); // a regeneration from before here decides again
            if self.rand01(pass, prog_bar, 1926, who, cx) < a.fx {
                self.ample_schedule(0, t.am_scratch, 90, self.ample_scratch_at, tick * 0.5f32, 0, lib);
            }
        }
        // Body percussion on the backbeats of chord steps.
        if !slapped && a.fx > 0.0f32 && self.ample_count >= 3 && backbeat && self.rand01(pass, gstep, 1331, prog_bar, cx) < a.fx {
            let key = if self.rand01(pass, gstep, 1333, prog_bar, cx) < 0.5f32 { t.am_hit_top_muted } else { t.am_strum_mute };
            let at = self.ample_at(at_or_default(&self.ample_notes, 0).on, lib);
            self.ample_schedule(0, key, 90, at, tick * 0.5f32, 0, lib);
        }

        for i in 0..self.ample_count {
            let n = at_or_default(&self.ample_notes, i);
            let mut note = n.note;
            while note < t.am_note_low {
                note += 12;
            }
            while note > t.am_note_high {
                note -= 12;
            }
            // Into the target's range (127 on Sustain is a Pop: only when asked); quiet notes keep their body.
            let mut vel = clamp(
                (t.am_note_vel_low as f32 + clamp(n.vel / 127.0f32, 0.0f32, 1.0f32) * (t.am_note_vel_high - t.am_note_vel_low) as f32 + 0.5f32)
                    as i32,
                t.am_note_vel_low,
                t.am_note_vel_high,
            );
            let mut art = 0; // sustain, harmonic, palm mute, slide in, legato slide, hammer-on
            // An accent the player pops is decided first: it beats a chance palm mute (a Pop is an open string).
            let pop = accent && a.pops > 0.0f32 && (n.flags & 3) == 0 && self.rand01(pass, gstep, 1329, note, cx) < a.pops;
            if (n.flags & 3) != 0 || (!pop && a.palm_mute > 0.0f32 && note < 55 && on_beat && self.rand01(pass, gstep, 1321, note, cx) < a.palm_mute)
            {
                art = 2;
            } else if single && self.ample_prev_note >= 0 && n.on > self.ample_prev_on && n.on - self.ample_prev_on <= 1.0 + 1.0e-6 {
                let iv = (note - self.ample_prev_note).abs();
                let r = self.rand01(pass, gstep, 1323, note, cx);
                if (1..=2).contains(&iv) && r < a.hammer_on {
                    art = 5;
                } else if (3..=5).contains(&iv) && r < a.slides {
                    art = 4;
                }
                if art != 0 {
                    let until = self.ample_at(n.on, lib) + 0.05;
                    if !self.ample_hold_until(self.ample_prev_note, until) {
                        art = 0; // nothing left ringing to hammer from
                    }
                }
            }
            if art == 0 && !pop && a.harmonics > 0.0f32 && n.len >= 0.5f32 - 1.0e-4f32 && self.rand01(pass, gstep, 1325, note, cx) < a.harmonics {
                let h = self.ample_harmonic_for(note, lib);
                if h >= 0 {
                    note = h;
                    art = 1;
                }
            }
            // Ornaments: a grace note a scale step (hammer-on) or two (slide) below, then onto the note, at the
            // player's amount (fingerpicked notes rarely follow a close one, so this is where the technique is heard).
            let mut grace = -1;
            if art == 0 && !pop && single && (a.hammer_on > 0.0f32 || a.slides > 0.0f32) {
                let r = self.rand01(pass, gstep, 1335, note, cx);
                let hammer = r < a.hammer_on;
                let slide = !hammer && r < a.hammer_on + a.slides;
                if hammer || slide {
                    let g = if hammer { self.step_down(note, 0) } else { self.step_down(self.step_down(note, 0), 0) };
                    if g >= t.am_note_low && note - g >= 1 && note - g <= 5 {
                        grace = g;
                        art = if hammer { 5 } else { 4 };
                    }
                }
            }
            if art == 0
                && a.slide_in > 0.0f32
                && single
                && n.on - self.ample_prev_on >= 1.0
                && note >= at_or_default(&t.am_open_strings, 0) + 2
                && self.rand01(pass, gstep, 1327, note, cx) < a.slide_in
            {
                art = 3;
            }
            if art == 0 && pop {
                vel = 127;
            }
            // Palm-mute depth is velocity (2.2.4, lower = deeper): deep and soft at Energy 1, light and loud at 4.
            if art == 2 && self.expressive() {
                vel = clamp(
                    (t.am_note_vel_low as f32
                        + (t.am_note_vel_high - t.am_note_vel_low) as f32 * (0.25f32 + 0.15f32 * self.energy as f32)
                        + (self.rand01(pass, gstep, 1927, note, cx) - 0.5f32) * 8.0f32
                        + 0.5f32) as i32,
                    t.am_note_vel_low,
                    t.am_note_vel_high,
                );
            }

            let on = self.ample_at(n.on, lib);
            let key = at_or_default(&t.am_art, art);
            if key != self.ample_art {
                self.q.schedule_flags = 8192;
                let key_at = self.ample_key_at(on - 8.0 * ms, on, lib);
                self.ample_schedule(0, key, 100, key_at, (10.0 * ms) as f32, 0, lib);
                self.q.schedule_flags = 0;
                self.ample_art = key;
            }
            let mut len = n.len;
            if single && (a.hammer_on > 0.0f32 || a.slides > 0.0f32) {
                len = max(len, tick * 1.1f32); // still ringing when the next note comes
            }
            let mut note_on = on;
            let mut note_end = max(on, self.current_ppq) + f64::from(len);
            if grace >= 0 {
                // The grace note on the beat, held into the note it hammers / slides onto.
                let g = min(f64::from(tick) * 0.5, 70.0 * ms) as f32;
                self.ample_schedule(0, grace, vel, on, g + (25.0 * ms) as f32, 0, lib);
                self.ample_schedule(
                    0,
                    note,
                    max(t.am_note_vel_low, vel - 6),
                    on + f64::from(g),
                    max(0.05f32, len - g),
                    if art == 2 { K_AMPLE_MUTED_FLAG } else { 0 },
                    lib,
                );
                note_on = on + f64::from(g);
                note_end = max(note_on, self.current_ppq) + f64::from(max(0.05f32, len - g));
            } else {
                self.ample_schedule(0, note, vel, on, len, if art == 2 { K_AMPLE_MUTED_FLAG } else { 0 }, lib);
            }
            // A new note ends the last swell (CC1 is the whole channel's): back to 0 as it starts.
            if self.expressive() && self.ample_vib_end > on + 1.0e-9 {
                for q2 in 0..64 {
                    let q = *self.q.queue.at(q2);
                    if q.used && q.kind == 1 && q.d1 == 1 && q.on_ppq > on - 1.0e-9 {
                        self.q.queue.at_mut(q2).used = false;
                    }
                }
                if self.ample_vib_start < on {
                    self.ample_schedule(1, 1, 0, on, 0.0f32, 0, lib);
                }
                self.ample_vib_start = -1.0e9;
                self.ample_vib_end = -1.0e9;
            }
            if self.expressive() && single {
                // Slide-out (2.2.5: D#0 while a note above fret 3 rings): a phrase's last picked note, 60 ms before its end.
                let abs_bar = self.floor_to_int(step_ppq / max(1.0e-6, f64::from(self.get_bar_length_ppq())) + 1.0e-6); // progBar repeats each section
                if art != 1
                    && art != 2
                    && self.is_phrase_end(prog_bar)
                    && sib >= fill_zone_start(self.meter)
                    && n.len >= 0.5f32 * 0.9f32 // half a beat (humanize can trim 10%)
                    && self.slide_fret(note, clamp(a.capo, 0, 7), lib) >= 4
                    && abs_bar - self.ample_slide_out_bar >= 2
                    && a.slides > 0.0f32
                    && self.rand01(pass, gstep, 1924, prog_bar, cx) < a.slides * 0.5f32
                {
                    let at = max(note_on + 0.5 * ms, note_end - 60.0 * ms);
                    self.q.schedule_flags = 8192;
                    self.ample_schedule(0, at_or_default(&t.am_art, 3), 100, at, (8.0 * ms) as f32, 0, lib);
                    self.q.schedule_flags = 0;
                    self.ample_slide_out_prev_bar = self.ample_slide_out_bar;
                    self.ample_slide_out_bar = abs_bar;
                    self.ample_slide_out_at = at;
                    self.ample_art = -1; // the next note sends its own articulation again
                }
                // Vibrato on held lead notes: melodic-role players, notes of a beat or more (Ample's auto-vibrato
                // envelope leaves fast notes alone by itself).
                if self.feel_for(who, lib).role == 5 && n.len >= 1.0f32 * 0.9f32 && note_end - note_on > 150.0 * ms {
                    // a beat, as above
                    let depth = clamp(
                        28 + 10 * self.energy + self.floor_to_int(f64::from((self.rand01(pass, gstep, 1928, note, cx) - 0.5f32) * 8.0f32 + 0.5f32)),
                        1,
                        127,
                    );
                    let start = note_on + 150.0 * ms;
                    self.ample_vib_start = start;
                    self.ample_vib_end = note_end;
                    self.ample_schedule(1, 1, depth, self.ample_vib_start, 0.0f32, 0, lib);
                    self.ample_schedule(1, 1, 0, self.ample_vib_end, 0.0f32, 0, lib);
                }
            }
            if single {
                self.ample_push_prev(note, n.on);
            }
        }
        if !single {
            self.ample_push_prev(-1, at_or_default(&self.ample_notes, 0).on);
        }
    }

    /// amplePick's `slideFret` lambda: the fret `nn` plays on, from the last fingered shape (above fret 3).
    fn slide_fret(&mut self, nn: i32, capo: i32, lib: &LibraryData) -> i32 {
        let mut f = -1;
        let prev = self.guitar_prev;
        self.ample_string_for(nn, if prev.valid { Some(&prev) } else { None }, capo, &mut f, lib);
        f
    }

    /// `void strum (Voicing v, bool down, int maxStrings, float vel, float len, float spacingMs)`
    pub(crate) fn strum(&mut self, v: Voicing, down: bool, max_strings: i32, vel: f32, len: f32, spacing_ms: f32) {
        // Partial strums take the top (treble) strings in either direction.
        let shift = self.octave_shift * 12;
        let first = max(0, v.count - max_strings);
        let mut k = 0;
        if down {
            for i in first..v.count {
                self.add_step_note(v.notes.val(i) + shift, vel - k as f32 * 1.5f32, len, 1, spacing_ms * k as f32);
                k += 1;
            }
        } else {
            let mut i = v.count - 1;
            while i >= first {
                self.add_step_note(v.notes.val(i) + shift, vel - k as f32 * 2.0f32, len, 1, spacing_ms * k as f32);
                k += 1;
                i -= 1;
            }
        }
    }

    /// `void guitarStep (int who, int step, int pass, int progBar, int sib, float64 stepPpq, Chord ch, Chord nx, Chord pv, bool changeNext)`
    pub(crate) fn guitar_step(
        &mut self,
        who: i32,
        step: i32,
        pass: i32,
        prog_bar: i32,
        sib: i32,
        step_ppq: f64,
        ch: Chord,
        nx: Chord,
        pv: Chord,
        change_next: bool,
        cx: &mut Ctx<'_>,
        out: &mut dyn MidiSink,
    ) {
        let lib = cx.lib;
        let tick = self.tick_ppq(lib);
        // The lanes are 2 bars of 16 steps; 12-step meters read them through gridStep (comp), -1 = no lane hit here.
        let bar16 = self.positive_modulo(prog_bar, 2) * 16;
        // The `has` lambda reads gridAt, which takes &mut self here: its (pure) answers for the bar's 16 steps first.
        let mut has = [false; 16];
        for (s, h) in (0i32..).zip(has.iter_mut()) {
            *h = self.grid_at(who, 0, bar16 + s, 0, lib) > 0 || self.grid_at(who, 1, bar16 + s, 0, lib) > 0;
        }
        let src = self.grid_step_in(who, sib, GridRole::Comp, |s| usize::try_from(s).ok().and_then(|k| has.get(k)).copied().unwrap_or(false), lib);
        let gstep = bar16 + src;
        let shift = self.octave_shift * 12;
        let shape = self.voicing_for(who, ch, pv, lib);
        let spacing = self.feel_for(who, lib).roll_ms * (0.6f32 + 0.4f32 * self.humanize / 50.0f32);
        self.clear_step();

        let fill = sib >= fill_zone_start(self.meter) && self.fill_active(who, pass, prog_bar, change_next, true, cx, out);
        if fill && shape.count > 0 {
            // 16-only: the fill vocabulary is the 4/4 bar's last beat (12..15), end-aligned to the meter's bar.
            let fs = sib + 16 - self.spb();
            let variant = self.fill_variant(who, pass, prog_bar, cx);
            let top = shape.notes.val(shape.count - 1) + shift;
            if variant == 0 {
                // Hammer-on lick around the top string.
                let n = if fs == 12 {
                    top
                } else if fs == 13 {
                    self.step_up(top, 1)
                } else if fs == 14 {
                    top
                } else {
                    self.step_down(top, 1)
                };
                self.add_step_note(n, 76.0f32 + max(0, fs - 12) as f32 * 4.0f32, tick * 0.9f32, 1, 0.0f32);
            } else if variant == 1 {
                // Rake: quick alternating strums building up.
                self.strum(
                    shape,
                    fs % 2 == 0,
                    if fs % 2 == 0 { 6 } else { 4 },
                    66.0f32 + max(0, fs - 12) as f32 * 9.0f32,
                    tick * 0.8f32,
                    spacing * 0.6f32,
                );
            } else if fs == 14 {
                // Slide into the next chord from a whole step below.
                let next = self.voicing_for(who, nx, ch, lib);
                let from = max(0, next.count - 3);
                for i in from..next.count {
                    if !self.respect {
                        self.add_step_note(next.notes.val(i) + shift - 2, 50.0f32, tick * 0.5f32, 1, -35.0f32);
                    }
                    self.add_step_note(next.notes.val(i) + shift, 84.0f32, tick * 2.0f32, 1, 0.0f32);
                }
            }
        } else {
            if src >= 0 && self.lane_hit(who, 0, gstep, step, pass, cx) && shape.count > 0 {
                let type_ = self.lane_type(who, 0, gstep, lib);
                let mut n = if type_ == 2 {
                    shape.notes.val(1)
                } else if type_ == 3 {
                    shape.notes.val(0) + 12
                } else {
                    shape.notes.val(0)
                };
                if type_ != 2 && type_ != 3 && ch.bass_pc >= 0 {
                    n = self.slash_at_or_below(n, &ch); // G/B: the bass string plays B
                }
                let vel = self.lane_vel(who, 0, gstep, lib);
                let len = self.lane_len(who, 0, gstep, tick, lib);
                self.add_step_note(n + shift, vel, len, 0, 0.0f32);
            }
            if src >= 0 && self.lane_hit(who, 1, gstep, step, pass, cx) && shape.count > 0 {
                let type_ = self.lane_type(who, 1, gstep, lib);
                let vel = self.lane_vel(who, 1, gstep, lib);
                let len = self.lane_len(who, 1, gstep, tick, lib);
                if type_ == 1 {
                    self.strum(shape, true, 6, vel, len, spacing);
                } else if type_ == 2 {
                    self.strum(shape, false, 4, vel, len, spacing * 0.8f32);
                } else if type_ == 3 {
                    self.strum(shape, true, 3, vel, min(len, tick * 0.5f32), spacing);
                } else if (4..=8).contains(&type_) {
                    self.add_step_note(shape.notes.val(min(type_ - 4, shape.count - 1)) + shift, vel, len, 1, 0.0f32);
                } else if type_ == 9 {
                    self.add_step_note(shape.notes.val(shape.count - 1) + shift, vel, len, 1, 0.0f32);
                } else if type_ == 11 && shape.count >= 2 {
                    // Double stop on the top two strings, snapped short.
                    self.add_step_note(shape.notes.val(shape.count - 1) + shift, vel, min(len, tick * 0.7f32), 1, 0.0f32);
                    self.add_step_note(shape.notes.val(shape.count - 2) + shift, vel - 6.0f32, min(len, tick * 0.7f32), 1, 2.0f32);
                } else if type_ == 12 {
                    self.add_step_note(shape.notes.val(shape.count - 1) + shift, vel, tick * 0.3f32, 1, 0.0f32);
                }
            }
        }
        // A half-bar chord the lanes have no hit for: release the held chord and strum the new one (spec 2026-10-28 §2).
        if self.step_count == 0 && shape.count > 0 && self.revoice_due(ch, pv, step_ppq, cx) {
            let (vel, _) = self.revoice_vel(who, pv, step_ppq, lib);
            if vel > 0.0f32 {
                let len = self.revoice_len(step_ppq, lib);
                self.strum(shape, true, 6, vel, len, spacing);
            }
        }
        self.note_chord_strike(-1, step_ppq);
        self.perform_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
    }
}

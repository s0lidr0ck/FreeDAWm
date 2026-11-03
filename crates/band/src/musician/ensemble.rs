//! `MusicianEnsemble.cpp`: ensemble (Crow Hill / Miroslav / UJAM, brass, strings, melodic), ported line by line.
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.
//! Differences of form: the `barCap` lambdas capture copies (the bar length is read once: it depends only on the
//! meter, which a step never changes); `melodyStep`'s `gridStepIn` predicate reads a table of the 16 source steps made
//! first (`gridAt` takes `&mut self`); brassPerform's `add` lambda is [`br_add_out`]; fixed-table reads
//! (`t.ujPhraseKeys[(size_t) i]`, `brNotes[(size_t) i]`, …) go through `at_or_default` / `get`.

use super::*;

/// brassPerform's `add` lambda: a pitch in range, not yet in the voicing, while there is room.
fn br_add_out(outs: &mut [i32; 16], n: &mut i32, p: i32, lo: i32, hi: i32) {
    if p < lo || p > hi || *n >= 16 {
        return;
    }
    for j in 0..*n {
        if at_or_default(outs, j) == p {
            return;
        }
    }
    if let Some(d) = usize::try_from(*n).ok().and_then(|k| outs.get_mut(k)) {
        *d = p;
    }
    *n += 1;
}

impl Musician {
    // ---- Crow Hill, Miroslav, UJAM: UJAM phrases -------------------------------

    /// `void ujamRelease (int note)`
    pub(crate) fn ujam_release(&mut self, note: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        for i in 0..64 {
            if self.active_slots.val(i) && self.active_notes.val(i) == note {
                self.send_slot_note_off(i, cx, out);
            }
        }
    }

    /// `void ujamReset ()`
    pub(crate) fn ujam_reset(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if self.ujam_phrase >= 0 {
            self.ujam_release(self.ujam_phrase, cx, out);
            if self.ujam_stop_key >= 0 {
                // B1 = Stop (in Latch mode); none for a library that stops on release
                self.send_keyswitch(0x90 + self.midi_channel0, self.ujam_stop_key, 100, cx, out);
                self.send_keyswitch(0x80 + self.midi_channel0, self.ujam_stop_key, 0, cx, out);
            }
        }
        if self.ujam_cc1 > 0 && self.ujam_mod_cc >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.ujam_mod_cc, 0, cx, out);
        }
        self.ujam_phrase = -1;
        self.ujam_cc1 = -1;
        self.ujam_last_chord = -1;
    }

    /// `int ujamPhraseFor (int pass, int progBar)`
    pub(crate) fn ujam_phrase_for(&mut self, pass: i32, prog_bar: i32, cx: &mut Ctx<'_>) -> i32 {
        let t = self.current_target(cx.lib);
        let mut idx = at_or_default(&t.uj_energy_base, clamp(self.energy, 1, 4) - 1);
        let r = self.rand01(pass, prog_bar / 2, 977, 10, cx); // 10: the v0.8 UJAM target index, kept as a seed constant
        if self.energy >= 2 && self.energy <= 3 && r < 0.3f32 {
            idx -= 1;
        } else if self.energy == 3 && r > 0.85f32 {
            idx += 1;
        }
        at_or_default(&t.uj_phrase_keys, clamp(idx, 0, 5))
    }

    /// `void ujamStep (int who, int step, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn ujam_step(
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
        let _ = step;
        let lib = cx.lib;
        let ms = self.ppq_per_ms();
        let chord_len = self.chord_len_ppq();
        let tick = f64::from(self.tick_ppq(lib));
        let chord_idx = if self.cur.is_some() { self.cur_global } else { self.floor_to_int((step_ppq + 0.0001) / chord_len) };
        let chord_start = if self.cur.is_some() {
            step_ppq - tick + 0.0001 < self.cur_start_ppq
        } else {
            chord_idx != self.floor_to_int((step_ppq - tick + 0.0001) / chord_len)
        };
        let early = step_ppq - 30.0 * ms;
        let tg = self.current_target(lib);

        if chord_start && chord_idx != self.ujam_last_chord {
            self.ujam_last_chord = chord_idx;
            let ch = self.chord_for_bar(prog_bar, cx);
            let root = self.key_root + ch.root_pc;

            // Phrase key: held; re-sent only when the choice changes.
            let phrase = self.ujam_phrase_for(pass, prog_bar, cx);
            if phrase != self.ujam_phrase {
                if self.ujam_phrase >= 0 {
                    self.ujam_release(self.ujam_phrase, cx, out);
                }
                self.q.schedule_flags = 8192;
                self.schedule(0, phrase, 100, early - 5.0 * ms, 4000.0f32);
                self.q.schedule_flags = 0;
                self.ujam_phrase = phrase;
                self.ujam_stop_key = tg.uj_stop_key;
            }

            // Mod wheel = decrescendo: softer at low Energy and at the quiet end of the arc.
            let dyn_ = self.dynamics / 50.0f32;
            let t = prog_bar as f32 / max(1, self.num_bars()) as f32;
            let arc = self.feel_for(who, lib).arc * dyn_ * (t - 0.5f32) * 2.0f32;
            let cc1 = clamp(((4 - self.energy) as f32 * 14.0f32 - arc * 70.0f32) as i32, 0, 100);
            if tg.uj_mod_cc >= 0 && cc1 != self.ujam_cc1 {
                self.schedule(1, tg.uj_mod_cc, cc1, early - 2.0 * ms, 0.0f32);
                self.ujam_cc1 = cc1;
                self.ujam_mod_cc = tg.uj_mod_cc;
            }

            // Chord in the Play Range: root folded into C3..B3, triad / 7th above (UJAM recognises
            // maj, min, sus2, sus4, 7 and dim); bass note in C2..B2. Both held for the whole chord.
            let base = tg.uj_play_base + self.positive_modulo(root - tg.uj_play_base, 12);
            let hold = chord_len as f32 * 1.02f32;
            self.q.schedule_flags = 0;
            self.schedule(0, base, 96, early, hold);
            let tones = min(ch.count, 4);
            for i in 1..tones {
                let mut n = base + self.positive_modulo(ch.tones.val(i), 12);
                if n <= base {
                    n += 12;
                }
                if n <= tg.uj_play_high {
                    self.schedule(0, n, 92, early, hold);
                }
            }
            if tg.uj_bass_mode == 0 || (tg.uj_bass_mode == 1 && ch.bass_pc >= 0) {
                let b =
                    tg.uj_bass_base + self.positive_modulo((if ch.bass_pc >= 0 { self.key_root + ch.bass_pc } else { root }) - tg.uj_bass_base, 12);
                self.schedule(0, b, 100, early, hold);
            }
            // A song's Ending (or the Ending key): the library's own ending on this chord, picked by Energy.
            if self.hold_step && self.perf_crash && tg.uj_ending_keys[0] >= 0 {
                let mut idx = clamp(self.energy - 1 + (if self.rand01(pass, prog_bar, 981, 10, cx) < 0.5f32 { 0 } else { 1 }), 0, 4);
                while idx > 0 && at_or_default(&tg.uj_ending_keys, idx) < 0 {
                    idx -= 1;
                }
                self.q.schedule_flags = 8192;
                self.schedule(0, at_or_default(&tg.uj_ending_keys, idx), clamp(70 + self.energy * 12, 1, 127), early + 2.0 * ms, tick as f32);
                self.q.schedule_flags = 0;
            }
        }

        // Additions at phrase ends (Fills decides how often; the Take picks which one).
        let phrase_end = self.is_phrase_end(prog_bar);
        if phrase_end && sib == half_step(self.meter) && self.fill_active(who, pass, prog_bar, true, true, cx, out) {
            let variant = self.fill_variant(who, pass, prog_bar, cx) % 4;
            self.q.schedule_flags = 8192;
            if at_or_default(&tg.uj_additions, variant) >= 0 {
                self.schedule(0, at_or_default(&tg.uj_additions, variant), 110, early, (tick * 2.0) as f32);
            }
            self.q.schedule_flags = 0;
        }
    }

    // ---- articulationKeys driver (Crow Hill and Philharmonik 2) ----------------

    /// `int artKeyFor (int art)`
    pub(crate) fn art_key_for(&mut self, art: i32, lib: &LibraryData) -> i32 {
        let t = self.current_target(lib);
        t.art_key_base + (if t.art_custom_keys { self.mp2_slot(art) } else { max(0, at_or_default(&t.art_keys, art)) })
    }

    /// `void artArticulate (int flags)`
    pub(crate) fn art_articulate(&mut self, flags: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let art = if (flags & 32) != 0 {
            2
        } else if (flags & 64) != 0 {
            3
        } else if (flags & 1024) != 0 {
            1
        } else {
            0
        }; // 0 Long, 1 Short, 2 Pluck, 3 Tremolo
        if art == self.crow_art {
            return;
        }
        let k = self.art_key_for(art, cx.lib);
        let k = self.ks(k);
        self.send_keyswitch(0x90 + self.midi_channel0, k, 100, cx, out);
        let k = self.art_key_for(art, cx.lib);
        let k = self.ks(k);
        self.send_keyswitch(0x80 + self.midi_channel0, k, 0, cx, out);
        self.crow_art = art;
    }

    /// `void crowCC (int cc, int value, float64 when)`
    pub(crate) fn crow_cc(&mut self, cc: i32, value: i32, when: f64, lib: &LibraryData) {
        if self.ks_active(lib) {
            self.ks_cc(cc, value, when, lib);
            return;
        } // the keyswitch driver keeps its own controllers
        let v = clamp(value, 0, 127);
        if cc == 1 {
            if v == self.crow_dyn {
                return;
            }
            self.crow_dyn = v;
        } else {
            if v == self.crow_expr {
                return;
            }
            self.crow_expr = v;
        }
        let number = if cc == 1 { self.current_target(lib).art_dyn_cc } else { self.current_target(lib).art_expr_cc };
        if number < 0 {
            return;
        }
        if cc == 1 {
            self.art_dyn_cc_sent = number;
        } else {
            self.art_expr_cc_sent = number;
        }
        self.schedule(1, number, v, when, 0.0f32);
    }

    /// `void crowReset ()`
    pub(crate) fn crow_reset(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        // CC numbers are the ones in force when the controllers were sent (the target may have changed since).
        if self.crow_dyn >= 0 && self.art_dyn_cc_sent >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.art_dyn_cc_sent, 64, cx, out);
        }
        if self.crow_expr >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.art_expr_cc_sent, 127, cx, out);
        }
        self.crow_art = -1;
        self.crow_dyn = -1;
        self.crow_expr = -1;
    }

    // ---- Indiginus Blue Street Brass (brassSection) ----------------------------

    /// `void brassReset ()`
    pub(crate) fn brass_reset(&mut self) {
        self.br_n = 0;
        self.br_setup_pass = -1_000_000;
        self.br_setup_at = -1.0e18;
        self.br_ready_at = -1.0e18;
        self.br_leg = -1;
        self.br_leg_fired = -1;
        self.br_cc_level = -1;
        self.br_cc_fired = -1;
        self.br_cc_at = -1.0e18;
        self.br_cc_fired_at = -1.0e18;
        self.br_prev_note = -1;
        self.br_prev_out = -1;
        self.br_prev_on = -1.0e18;
        self.br_prev_end = -1.0e18;
        self.br_prev_leg = false;
        self.br_phrase_leg = false;
        self.br_phrase_sixth = false;
    }

    /// `void brAdd (int note, float vel, float64 on, float len, float written, int hand)`
    pub(crate) fn br_add(&mut self, note: i32, vel: f32, on: f64, len: f32, written: f32, hand: i32) {
        if self.br_n >= self.br_notes.len() as i32 {
            return;
        }
        if let Some(d) = usize::try_from(self.br_n).ok().and_then(|k| self.br_notes.get_mut(k)) {
            *d = BrNote { note, vel, len, written, on, hand };
        }
        self.br_n += 1;
    }

    /// `void brassSetup (int pass, int who)`
    pub(crate) fn brass_setup(&mut self, pass: i32, who: i32, lib: &LibraryData) {
        if self.live_mode || self.br_setup_pass == pass {
            return;
        }
        self.br_setup_pass = pass;
        let t = self.current_target(lib);
        let ms = self.ppq_per_ms();
        let at = self.current_ppq + 0.5 * ms;
        for &(num, v) in &t.br_setup_ccs {
            if num >= 0 {
                self.schedule(1, num, v, at, 0.0f32);
            }
        }
        if self.br_leg == 1 || self.br_leg_fired == 1 {
            self.q.schedule_flags = 8192;
            self.schedule(0, t.br_legato_off, 100, at, (5.0 * ms) as f32);
            self.q.schedule_flags = 0;
            self.br_leg = 0;
        }
        let role = self.feel_for(who, lib).role;
        let patch = if role == 5 {
            t.br_patch_melodic
        } else if role == 1 {
            t.br_patch_low
        } else {
            t.br_patch_ensemble
        };
        let value = if (1..=26).contains(&patch) { at_or_default(&t.br_instrument_value, patch) } else { -1 };
        if value >= 0 && t.br_instrument_cc >= 0 {
            self.schedule(1, t.br_instrument_cc, value, at, 0.0f32);
        }
        self.br_setup_at = at;
        self.br_ready_at = at + (if value >= 0 { 50.0 } else { 25.0 }) * ms;
    }

    /// `void brCcRebuild ()`
    pub(crate) fn br_cc_rebuild(&mut self, lib: &LibraryData) {
        self.br_cc_level = self.br_cc_fired;
        self.br_cc_at = self.br_cc_fired_at;
        let cc = self.current_target(lib).br_dyn_cc;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 1 && q.d1 == cc && q.on_ppq >= self.br_cc_at {
                self.br_cc_level = q.d2;
                self.br_cc_at = q.on_ppq;
            }
        }
    }

    /// `void brCutCC (float64 from)`
    pub(crate) fn br_cut_cc(&mut self, from: f64, lib: &LibraryData) {
        let cc = self.current_target(lib).br_dyn_cc;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 1 && q.d1 == cc && q.on_ppq >= from - 1.0e-9 {
                self.q.queue.at_mut(i).used = false;
            }
        }
        self.br_cc_rebuild(lib);
    }

    /// `void brCC (int v, float64 at)`
    pub(crate) fn br_cc(&mut self, v: i32, at: f64, lib: &LibraryData) {
        let v = clamp(v, 0, 127);
        self.schedule(1, self.current_target(lib).br_dyn_cc, v, at, 0.0f32);
        self.br_cc_level = v;
        self.br_cc_at = max(at, self.current_ppq);
    }

    /// `void brRamp (int from, int to, float64 a, float64 b)`
    pub(crate) fn br_ramp(&mut self, from: i32, to: i32, a: f64, b: f64, lib: &LibraryData) {
        let step_p = self.beat_ppq() / 16.0;
        let k = max(1, ((b - a) / step_p - 1.0e-9).ceil() as i32);
        let mut last = from;
        for i in 1..=k {
            let v = (f64::from(from) + f64::from(to - from) * f64::from(i) / f64::from(k)).round() as i32;
            if v != last {
                self.br_cc(v, min(b, a + step_p * f64::from(i)), lib);
                last = v;
            }
        }
    }

    /// `void brSetOff (int pitch, float64 on, float64 off)`
    pub(crate) fn br_set_off(&mut self, pitch: i32, on: f64, off: f64) {
        for i in 0..64 {
            let q = self.q.queue.at_mut(i);
            if q.used && q.kind == 0 && q.ch == 0 && q.d1 == pitch && (q.flags & 8192) == 0 && (q.on_ppq - on).abs() < 1.0e-6 {
                q.dur_ppq = max(0.01, off - q.on_ppq) as f32;
                return;
            }
        }
        for i in 0..64 {
            if self.active_slots.val(i)
                && self.active_channels.val(i) == self.midi_channel0
                && self.active_notes.val(i) == pitch
                && (self.active_flags.val(i) & 8192) == 0
            {
                self.active_off_ppq.set(i, max(off, self.current_ppq));
                return;
            }
        }
    }

    /// `int brLevel (float v) const`
    pub(crate) fn br_level(&self, v: f32) -> i32 {
        const LO: [i32; 4] = [40, 57, 73, 90];
        const HI: [i32; 4] = [70, 89, 108, 127];
        let e = clamp(self.energy, 1, 4) - 1;
        let f = clamp((v - 40.0f32) / 80.0f32, 0.0f32, 1.0f32);
        (at_or_default(&LO, e) as f32 + (at_or_default(&HI, e) - at_or_default(&LO, e)) as f32 * f).round() as i32
    }

    /// `void brassPerform (int who, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn brass_perform(&mut self, who: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>) {
        if self.br_n == 0 {
            return;
        }
        let lib = cx.lib;
        let n0 = self.br_n;
        self.br_n = 0;
        let t = self.current_target(lib);
        let b = &self.feel_for(who, lib).brass;
        let ms = self.ppq_per_ms();
        let beat = self.beat_ppq();
        let tick = f64::from(self.tick_ppq(lib));
        let nat = self.expressive() && !self.live_mode;
        let key = prog_bar * self.spb() + sib;

        // The melody: the step's top right-hand note (else its top note); the loudest note sets the level.
        let mut mi = -1;
        let mut loud = 0.0f32;
        let mut on = 1.0e18f64;
        for i in 0..n0 {
            let x = at_or_default(&self.br_notes, i);
            loud = max(loud, x.vel);
            on = min(on, x.on);
            if x.hand >= 1 && (mi < 0 || x.note > at_or_default(&self.br_notes, mi).note) {
                mi = i;
            }
        }
        if mi < 0 {
            for i in 0..n0 {
                if mi < 0 || at_or_default(&self.br_notes, i).note > at_or_default(&self.br_notes, mi).note {
                    mi = i;
                }
            }
        }
        let m = at_or_default(&self.br_notes, mi);
        let fold = |p: i32| {
            let mut p = p;
            while p < t.br_low {
                p += 12;
            }
            while p > t.br_high {
                p -= 12;
            }
            p
        };
        let top = fold(m.note);
        let written = m.written;
        let connected = self.br_prev_note >= 0 && on > self.br_prev_on + 1.0e-6 && on - self.br_prev_end <= tick + 1.0e-6;
        let repeated = connected && top == self.br_prev_note;
        if !connected {
            self.br_phrase_leg = self.expressive() && self.rand01(pass, key, 1989, who, cx) < b.legato;
            self.br_phrase_sixth = self.rand01(pass, key, 1993, who, cx) < 0.5f32;
        }

        // Section voicing (one channel).
        let mut outs = [0i32; 16];
        let mut n = 0;
        if self.live_mode {
            for i in 0..n0 {
                br_add_out(&mut outs, &mut n, fold(at_or_default(&self.br_notes, i).note), t.br_low, t.br_high);
            }
        } else {
            br_add_out(&mut outs, &mut n, top, t.br_low, t.br_high);
            let chord_mode = clamp(self.energy, 1, 4) >= 4 - (b.voiced * 2.0f32).round() as i32;
            if chord_mode {
                let ch = self.chord_for_bar(prog_bar, cx);
                let want = if self.energy >= 4 { 4 } else { 3 };
                let mut p = top - 1;
                while p >= top - 12 && n < want {
                    if self.is_chord_tone(p, ch) {
                        br_add_out(&mut outs, &mut n, p, t.br_low, t.br_high);
                    }
                    p -= 1;
                }
            } else if b.doubling == 1 {
                if top - 12 >= t.br_low {
                    br_add_out(&mut outs, &mut n, top - 12, t.br_low, t.br_high);
                } else {
                    br_add_out(&mut outs, &mut n, top + 12, t.br_low, t.br_high);
                }
            } else if b.doubling == 2 {
                let mut u = self.step_down(self.step_down(top, 0), 0); // a 3rd below
                if self.br_phrase_sixth {
                    u = self.step_down(self.step_down(self.step_down(u, 0), 0), 0); // ... or a 6th
                }
                br_add_out(&mut outs, &mut n, u, t.br_low, t.br_high);
            }
        }
        if n == 0 {
            return;
        }

        // Stab / swell / legato.
        let stab = nat && written <= (beat * 0.5) as f32 + 1.0e-4f32 && self.rand01(pass, key, 1990, top, cx) < b.stab;
        let swell = nat && !stab && written >= (beat * 2.0) as f32 - 1.0e-4f32 && self.rand01(pass, key, 1991, top, cx) < b.swell;
        let keys = self.expressive() && n == 1;
        let chord_off = self.expressive() && !self.live_mode && n > 1; // a chord (octaves, 3rds, a close voicing): legato off
        let want = if keys {
            if self.br_phrase_leg && !stab && !repeated { 1 } else { 0 }
        } else if chord_off {
            0
        } else {
            self.br_leg
        };
        let key_now = (keys || chord_off) && want != self.br_leg && (want == 1 || self.br_leg == 1 || keys);
        on = max(on, max(self.br_ready_at, self.current_ppq + (if key_now { 20.5 } else { 5.5 }) * ms));
        if key_now {
            self.q.schedule_flags = 8192;
            self.schedule(0, if want == 1 { t.br_legato_on } else { t.br_legato_off }, 100, on - 20.0 * ms, (5.0 * ms) as f32);
            self.q.schedule_flags = 0;
            self.br_leg = want;
        }

        let mut len = f64::from(m.len);
        if stab {
            len = beat * 0.25 + 30.0 * ms;
        }
        let leg_note = keys && want == 1;
        if leg_note {
            len = max(len, f64::from(written) + tick + 15.0 * ms); // held into the next note; trimmed to a 15 ms overlap when it comes
            if connected && self.br_prev_leg && self.br_prev_out >= 0 {
                self.br_set_off(self.br_prev_out, self.br_prev_on, on + 15.0 * ms);
            }
        }

        // Dynamics.
        let mut target = self.br_level(loud);
        let bar_l = max(1.0e-6, f64::from(self.get_bar_length_ppq()));
        let bar_end = self.bar_floor(step_ppq, bar_l) + bar_l;
        let phrase_bar = self.is_phrase_end(prog_bar);
        if nat && phrase_bar && step_ppq >= bar_end - beat - 1.0e-9 {
            // fall-away over a phrase's last beat
            target -= (25.0 * clamp((step_ppq + tick - (bar_end - beat)) / beat, 0.0, 1.0)).round() as i32;
        }
        target = clamp(target, 1, 127);
        let at = on - 5.0 * ms;
        let w_end = on + f64::from(written);
        self.br_cut_cc(at, lib);
        if stab {
            self.br_cc(100 + (self.rand01(pass, key, 1990, top + 128, cx) * 20.99f32) as i32, at, lib);
            self.br_cc(60, on + len + 1.0 * ms, lib);
        } else if swell && target > 45 {
            self.br_cc(45, at, lib);
            let mid = on + f64::from(written) * 0.5;
            self.br_ramp(45, target, at, mid, lib);
            if phrase_bar && w_end >= bar_end - tick * 0.5 {
                self.br_ramp(target, max(1, target - 25), max(mid, w_end - beat), w_end - 5.0 * ms, lib);
            }
        } else if nat && connected && self.br_cc_level >= 0 {
            // Inside a line: <= 15 a beat, the rest glided in 1/16-beat points over the note (at most a beat).
            let max_move = (15.0 * (at - self.br_cc_at) / beat + 1.0e-9).floor() as i32;
            let start = clamp(target, self.br_cc_level.wrapping_sub(max_move), self.br_cc_level.wrapping_add(max_move));
            self.br_cc(start, at, lib);
            if start != target {
                let end_at = min(at + beat, max(at, w_end - 5.0 * ms));
                let span = (15.0 * (end_at - at) / beat).floor() as i32;
                let reach = clamp(target, start.wrapping_sub(span), start.wrapping_add(span));
                if end_at > at {
                    self.br_ramp(start, reach, at, end_at, lib);
                }
            }
        } else {
            self.br_cc(target, at, lib);
        }

        // A fall at a phrase's end: CC1 -40 over the note's last 120 ms.
        if nat && !stab && phrase_bar && w_end >= bar_end - tick * 0.5 && self.rand01(pass, key, 1992, who, cx) < b.fall {
            let f_end = on + min(f64::from(written), len);
            let f_from = max(on, f_end - 120.0 * ms);
            let from = if self.br_cc_level >= 0 { self.br_cc_level } else { target };
            self.br_ramp(from, max(1, from - 40), f_from, f_end, lib);
        }

        for i in 0..n {
            self.schedule(0, at_or_default(&outs, i), t.br_note_vel, on, max(0.02, len) as f32);
        }
        self.br_prev_note = top;
        self.br_prev_on = on;
        self.br_prev_end = w_end;
        self.br_prev_out = top;
        self.br_prev_leg = leg_note;
    }

    /// `void brassFired (int kind, int d1, int d2, int flags, float64 at)`
    pub(crate) fn brass_fired(&mut self, kind: i32, d1: i32, d2: i32, flags: i32, at: f64, lib: &LibraryData) {
        if !self.brass_drv(lib) {
            return;
        }
        let t = self.current_target(lib);
        if kind == 1 && d1 == t.br_dyn_cc {
            self.br_cc_fired = d2;
            self.br_cc_fired_at = at;
        } else if kind == 0 && (flags & 8192) != 0 {
            if d1 == t.br_legato_on {
                self.br_leg_fired = 1;
            } else if d1 == t.br_legato_off {
                self.br_leg_fired = 0;
            }
        }
    }

    /// `void brassDropFrom (float64 cut)`
    pub(crate) fn brass_drop_from(&mut self, cut: f64, lib: &LibraryData) {
        if !self.brass_drv(lib) {
            return;
        }
        let t = self.current_target(lib);
        self.br_leg = self.br_leg_fired;
        let mut leg_at = -1.0e18f64;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 0 && (q.flags & 8192) != 0 && q.on_ppq >= leg_at && (q.d1 == t.br_legato_on || q.d1 == t.br_legato_off) {
                self.br_leg = if q.d1 == t.br_legato_on { 1 } else { 0 };
                leg_at = q.on_ppq;
            }
        }
        self.br_cc_rebuild(lib);
        if self.br_setup_at >= cut - 1.0e-9 {
            self.br_setup_pass = -1_000_000;
            self.br_setup_at = -1.0e18;
        }
        if self.br_prev_on >= cut - 1.0e-9 {
            self.br_prev_note = -1;
            self.br_prev_out = -1;
            self.br_prev_on = -1.0e18;
            self.br_prev_end = -1.0e18;
            self.br_prev_leg = false;
        }
        self.br_n = 0;
    }

    /// `void brassStop ()`
    pub(crate) fn brass_stop(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if self.brass_drv(cx.lib) {
            // (after a switch to another target: nothing of Blue Street Brass's is sent to it)
            let t = self.current_target(cx.lib);
            if (self.br_leg_fired == 1 || self.br_leg == 1) && t.br_legato_off >= 0 {
                self.send_midi(0x90 + self.midi_channel0, t.br_legato_off, 100, cx, out);
                self.send_midi(0x80 + self.midi_channel0, t.br_legato_off, 0, cx, out);
            }
            if (self.br_cc_fired >= 0 || self.br_cc_level >= 0) && t.br_dyn_cc >= 0 {
                self.send_midi(0xB0 + self.midi_channel0, t.br_dyn_cc, 0, cx, out); // CC1 back to 0
            }
        }
        self.brass_reset();
    }

    /// `void brassExpressionOff ()`
    pub(crate) fn brass_expression_off(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if !self.brass_drv(cx.lib) {
            return;
        }
        let t = self.current_target(cx.lib);
        for i in 0..64 {
            let q = self.q.queue.at_mut(i);
            if q.used
                && ((q.kind == 1 && q.d1 == t.br_dyn_cc)
                    || (q.kind == 0 && (q.flags & 8192) != 0 && (q.d1 == t.br_legato_on || q.d1 == t.br_legato_off)))
            {
                q.used = false;
            }
        }
        if self.br_leg_fired == 1 {
            self.send_midi(0x90 + self.midi_channel0, t.br_legato_off, 100, cx, out);
            self.send_midi(0x80 + self.midi_channel0, t.br_legato_off, 0, cx, out);
            self.br_leg_fired = 0;
        }
        if (self.br_cc_fired >= 0 || self.br_cc_level >= 0) && t.br_dyn_cc >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, t.br_dyn_cc, 0, cx, out); // CC1 back to 0
        }
        self.br_leg = self.br_leg_fired;
        self.br_cc_level = -1;
        self.br_cc_fired = -1;
        self.br_cc_at = -1.0e18;
        self.br_cc_fired_at = -1.0e18;
        self.br_prev_note = -1;
        self.br_prev_out = -1;
        self.br_prev_leg = false;
    }

    // ---- Ensemble and Melodic roles --------------------------------------------

    /// `int cellIndex (int type, int gstep, int count)`
    pub(crate) fn cell_index(&mut self, type_: i32, gstep: i32, count: i32) -> i32 {
        if count <= 0 {
            return 0;
        }
        if type_ == 11 {
            return gstep % 3;
        }
        if type_ == 12 {
            return (gstep / 2) % 3;
        }
        if type_ == 13 {
            let period = max(1, 2 * (count - 1));
            let pos = gstep % period;
            return if pos < count { pos } else { period - pos };
        }
        if type_ == 14 {
            return gstep % count;
        }
        if type_ == 15 {
            return (gstep / 2) % count;
        }
        if type_ == 17 {
            return min(gstep % 5, count - 1);
        }
        0
    }

    /// `void ensembleStep (int who, int step, int pass, int progBar, int sib, float64 stepPpq, Chord ch, Chord nx, Chord pv, bool changeNext)`
    pub(crate) fn ensemble_step(
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
        let gs = self.grid_step(who, sib, GridRole::Ensemble, lib); // the 16-step source index (-1: no hit here)
        let gstep = self.positive_modulo(prog_bar, 2) * 16 + max(0, gs);
        // 12-step meters: a note written to end in its bar keeps to it (written = 16ths before the speed multiplier); a written
        // tie survives. 6/8 pads / strings (spec 3.2): a note on a dotted beat written to end in the bar holds half a bar at most.
        let meter = self.meter;
        let bl = f64::from(self.get_bar_length_ppq());
        let bar_cap = move |len: f32| -> f32 {
            if !is12(meter) {
                return len;
            }
            let to_end = ((step_ppq / bl + 1.0e-9).floor() + 1.0) * bl - step_ppq;
            if f64::from(len) * 0.25 / f64::from(tick) > to_end + 1.0e-3 {
                return len; // a written tie
            }
            if meter == Meter::M68 && (sib == 0 || sib == half_step(meter)) {
                return min(len, min(to_end, bl * 0.5) as f32);
            }
            min(len, to_end as f32)
        };
        let shift = self.octave_shift * 12;
        let v = self.voicing_for(who, ch, pv, lib);
        self.clear_step();

        let fill = sib >= fill_zone_start(self.meter) && self.fill_active(who, pass, prog_bar, change_next, true, cx, out);
        let fs = sib + 16 - self.spb(); // 16-only: the fill vocabulary, end-aligned (4/4: sib)

        self.note_intent = self.feel_for(who, lib).artic; // e.g. Nadia's pizzicato applies to the low voice too
        if gs >= 0 && self.lane_hit(who, 0, gstep, step, pass, cx) {
            let type_ = self.lane_type(who, 0, gstep, lib);
            let vel = self.lane_vel(who, 0, gstep, lib);
            let len = bar_cap(self.lane_len(who, 0, gstep, tick, lib));
            self.add_left_hand(who, type_, ch, vel, len, lib);
        }
        self.note_intent = 0;

        // Fills for section players are additive: the pattern keeps playing underneath so pads never
        // drop out at phrase ends. Only the tremolo fill takes over the right hand (it is continuous sound).
        let mut grid_plays = true;
        if fill && v.count > 0 {
            let variant = self.fill_variant(who, pass, prog_bar, cx);
            let target = self.voicing_for(who, nx, ch, lib);
            let top = (if target.count > 0 { target.notes.val(target.count - 1) } else { v.notes.val(v.count - 1) }) + shift;
            if variant == 0 {
                // Scale run up the top voice into the next chord.
                let mut p = top;
                for _ in 0..16 - fs {
                    p = self.step_down(p, 0);
                }
                self.add_step_note(p, 66.0f32 + (fs - 12) as f32 * 6.0f32, tick, 1, 0.0f32);
            } else if variant == 1 {
                // Measured tremolo swelling into the change.
                self.note_intent = 64;
                self.add_right_hand(1, v, 58.0f32 + (fs - 12) as f32 * 10.0f32, tick * 0.9f32, 0.0f32);
                self.note_intent = 0;
                grid_plays = false;
            } else if fs == 12 {
                // Octave lift: the next chord's top an octave up, arriving early.
                self.add_step_note(top + 12, 74.0f32, bar_cap(tick * 4.0f32), 1, 0.0f32);
            }
        }
        if grid_plays {
            for lane in 1..3 {
                if gs < 0 || !self.lane_hit(who, lane, gstep, step, pass, cx) {
                    continue;
                }
                let type_ = self.lane_type(who, lane, gstep, lib);
                let vel = self.lane_vel(who, lane, gstep, lib);
                let len = bar_cap(self.lane_len(who, lane, gstep, tick, lib));
                let fw = self.feel_for(who, lib);
                self.note_intent = fw.artic
                    | (if self.pattern == 4 { 32 } else { 0 })
                    | (if fw.voicing == 6 && self.energy == 4 && self.pattern == 7 && fw.artic == 0 { 64 } else { 0 })
                    | (if len < tick * 1.2f32 { 1024 } else { 0 });
                if type_ <= 10 {
                    self.add_right_hand(type_, v, vel, len, 0.0f32);
                } else if v.count > 0 {
                    let ci = self.cell_index(type_, gstep, v.count);
                    self.add_step_note(v.notes.val(ci) + shift, vel, len, 1, 0.0f32);
                }
                self.note_intent = 0;
            }
        }

        if !self.dry_run {
            // a push's look-ahead sends no pedal
            self.pedal_events(who, prog_bar, sib, !self.same_chord(ch, pv), step_ppq, Self::bass_of(&ch) == Self::bass_of(&pv), lib);
        }
        self.perform_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
    }

    /// `int melodyPitch (int who, Chord ch, int off)`
    pub(crate) fn melody_pitch(&mut self, who: i32, ch: Chord, off: i32, lib: &LibraryData) -> i32 {
        let anchor = self.feel_for(who, lib).rh_anchor;
        let mode = if ch.flavor == 7 || ch.flavor == 8 { 1 - self.scale_mode } else { self.scale_mode };
        let root = self.key_root + ch.root_pc;
        let base = anchor - 12 + self.positive_modulo(root - (anchor - 12), 12);
        base + self.scale_offset_in(mode, ch.degree + off) - self.scale_offset_in(mode, ch.degree) + self.octave_shift * 12
    }

    /// `int snapToChord (int pitch, Chord ch)`
    pub(crate) fn snap_to_chord(&mut self, pitch: i32, ch: Chord) -> i32 {
        if self.is_chord_tone(pitch, ch) {
            return pitch;
        }
        for d in 1..=2 {
            if self.is_chord_tone(pitch - d, ch) {
                return pitch - d;
            }
            if self.is_chord_tone(pitch + d, ch) {
                return pitch + d;
            }
        }
        pitch
    }

    /// `int melodyLaneFor (int who, int pass, int pair)`
    pub(crate) fn melody_lane_for(&mut self, who: i32, pass: i32, pair: i32, cx: &mut Ctx<'_>) -> i32 {
        let pairs = max(1, self.num_bars() / 2);
        if pair == pairs - 1 && pairs > 1 {
            return if self.rand01(pass, pair, 951, who, cx) < 0.6f32 { 3 } else { 2 };
        }
        if pair > 0 {
            return if self.rand01(pass, pair, 953, who, cx) < 0.6f32 { 1 } else { 2 };
        }
        1
    }

    /// `void melodyStep (int who, int step, int pass, int progBar, int sib, float64 stepPpq, Chord ch, Chord nx, bool changeNext)`
    pub(crate) fn melody_step(
        &mut self,
        who: i32,
        step: i32,
        pass: i32,
        prog_bar: i32,
        sib: i32,
        step_ppq: f64,
        ch: Chord,
        nx: Chord,
        change_next: bool,
        cx: &mut Ctx<'_>,
        out: &mut dyn MidiSink,
    ) {
        let lib = cx.lib;
        let tick = self.tick_ppq(lib);
        let pair = prog_bar / 2;
        let lane = self.melody_lane_for(who, pass, pair, cx);
        let bar16 = self.positive_modulo(prog_bar, 2) * 16;
        // `[&] (int s) { return gridAt (who, lane, bar16 + s, 0) > 0; }`, read for the 16 source steps first.
        let mut has = [false; 16];
        for (s, h) in (0i32..).zip(has.iter_mut()) {
            *h = self.grid_at(who, lane, bar16 + s, 0, lib) > 0;
        }
        let gs = self.grid_step_in(who, sib, GridRole::Melodic, |s| at_or_default(&has, s), lib); // the 16-step source index (-1: no hit here)
        let gstep = bar16 + max(0, gs);
        // 12-step meters: a note written to end in its bar keeps to it (written = 16ths before the speed multiplier); a written tie survives.
        let meter = self.meter;
        let bl = f64::from(self.get_bar_length_ppq());
        let bar_cap = move |len: f32| -> f32 {
            if !is12(meter) {
                return len;
            }
            let to_end = ((step_ppq / bl + 1.0e-9).floor() + 1.0) * bl - step_ppq;
            if f64::from(len) * 0.25 / f64::from(tick) > to_end + 1.0e-3 { len } else { min(len, to_end as f32) }
        };
        self.clear_step();

        let phrase_end = self.is_phrase_end(prog_bar);
        let fill = sib >= fill_zone_start(self.meter) && phrase_end && self.fill_active(who, pass, prog_bar, change_next, true, cx, out);
        let fs = sib + 16 - self.spb(); // 16-only: the fill vocabulary, end-aligned (4/4: sib)
        let mut played = -1;

        if fill {
            let variant = self.fill_variant(who, pass, prog_bar, cx);
            let home = self.melody_pitch(who, ch, 4, lib);
            if variant == 0 {
                // Scale run up into the next chord's nearest tone.
                let mp = self.melody_pitch(who, nx, 2, lib);
                let target = self.snap_to_chord(mp, nx);
                let mut p = target;
                for _ in 0..16 - fs {
                    p = self.step_down(p, 0);
                }
                played = p;
                self.add_step_note(p, 72.0f32 + (fs - 12) as f32 * 5.0f32, tick * 0.9f32, 1, 0.0f32);
            } else if variant == 1 {
                // Trill on the fifth.
                played = if fs % 2 == 0 { home } else { self.step_up(home, 0) };
                self.add_step_note(played, 70.0f32 + (fs % 2) as f32 * -8.0f32, tick * 0.9f32, 1, 0.0f32);
            } else {
                // Turn around the fifth; the last note falls away (brass / winds).
                played = if fs == 12 {
                    home
                } else if fs == 13 {
                    self.step_up(home, 0)
                } else if fs == 14 {
                    home
                } else {
                    self.step_down(home, 0)
                };
                self.note_intent = if fs == 15 { 128 } else { 0 };
                self.add_step_note(played, 76.0f32, tick * 0.9f32, 1, 0.0f32);
                self.note_intent = 0;
            }
        } else if gs >= 0 && self.lane_hit(who, lane, gstep, step, pass, cx) {
            let type_ = self.lane_type(who, lane, gstep, lib);
            if type_ >= 100 {
                let len = bar_cap(self.lane_len(who, lane, gstep, tick, lib));
                let mut p = self.melody_pitch(who, ch, type_ - 107, lib);
                // Beats 1 and 3 land on chord tones; beats 2 and 4 keep their passing tones (3/4: the downbeat; 6/8: both dotted beats).
                if crate::meter::strong_step(self.meter, sib) && len >= tick * 2.0f32 * self.length {
                    p = self.snap_to_chord(p, ch);
                }
                played = p;
                let vel = self.lane_vel(who, lane, gstep, lib);
                self.add_step_note(p, vel, len, 1, 0.0f32);
            }
        }

        // Harmony voice (double stops): a diatonic third under the melody, or the fourth when the third is not a chord tone.
        if played >= 0 && gs >= 0 && self.lane_hit(who, 0, gstep, step, pass, cx) && self.lane_type(who, 0, gstep, lib) == 1 {
            let mut under = self.step_down(self.step_down(played, 0), 0);
            if !self.is_chord_tone(under, ch) {
                under = self.step_down(under, 0);
            }
            let vel = self.lane_vel(who, 0, gstep, lib);
            self.add_step_note(under, vel, bar_cap(max(self.step_len.val(0), tick)), 1, 0.0f32);
        }

        self.perform_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
    }

    /// `void personaStep (int who, int step, int pass, int progBar, int sib, float64 stepPpq)`
    /// (ported in Task 11: the keys persona path; the other roles' steps are their lanes' stubs)
    pub(crate) fn persona_step(
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
        let ch = self.chord_for_bar(prog_bar, cx);
        let nx = self.chord_for_bar(prog_bar + 1, cx);
        let pv = self.chord_for_bar(prog_bar - 1, cx);
        let change_next = !self.same_chord(ch, nx) && self.pattern_meets_bar;
        let liberties = !self.respect;
        let tick = self.tick_ppq(lib);

        let role = self.feel_for(who, lib).role;
        self.decide_phrasing(who, pass, prog_bar, step_ppq, cx, out);
        // Natural: a pushing player (keys, bass, guitar, ensemble) plays the downbeat of the next chord an 8th
        // early: the notes it would play there (same Take decisions), struck now, ringing through the downbeat.
        if !self.dry_run
            && self.perf.natural
            && change_next
            && (role == 0 || role == 1 || role == 3 || role == 4)
            && self.push_here(who, pass, step_ppq, cx)
        {
            let end = self.chord_end_for(step_ppq);
            let ahead = max(1, self.floor_to_int((end - step_ppq) / f64::from(tick) + 0.5));
            let keep_mode = self.phrase_mode;
            let keep_pickup = self.pickup_now;
            let keep_pickup_bar = self.pickup_prog_bar;
            self.dry_run = true;
            self.dry_count = 0;
            self.persona_step(who, step + ahead, pass, prog_bar + 1, self.positive_modulo(sib + ahead, self.spb()), end, cx, out);
            self.dry_run = false;
            self.phrase_mode = keep_mode;
            self.pickup_now = keep_pickup;
            self.pickup_prog_bar = keep_pickup_bar;
            if self.dry_count > 0 {
                self.clear_step();
                for i in 0..self.dry_count {
                    self.add_step_note_f(
                        self.dry_note.val(i),
                        self.dry_vel.val(i),
                        self.dry_len.val(i),
                        self.dry_hand.val(i),
                        self.dry_shift.val(i),
                        self.dry_flags.val(i),
                    );
                }
                self.note_chord_strike(if role == 3 { -1 } else { 1 }, step_ppq);
                self.push_now = true;
                self.push_now_chord = nx;
                self.annotate(4, step_ppq, cx, out);
                self.perform_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
                return;
            }
        }
        if role == 1 {
            self.bass_step(who, step, pass, prog_bar, sib, step_ppq, ch, nx, change_next, cx, out);
            return;
        }
        if role == 2 {
            self.drum_step(who, step, pass, prog_bar, sib, step_ppq, change_next, cx, out);
            return;
        }
        if role == 3 {
            self.guitar_step(who, step, pass, prog_bar, sib, step_ppq, ch, nx, pv, change_next, cx, out);
            return;
        }
        if role == 4 {
            self.ensemble_step(who, step, pass, prog_bar, sib, step_ppq, ch, nx, pv, change_next, cx, out);
            return;
        }
        if role == 5 {
            self.melody_step(who, step, pass, prog_bar, sib, step_ppq, ch, nx, change_next, cx, out);
            return;
        }

        let mut rh_chord = ch;
        let mut lh_chord = ch;
        let mut lead_from = pv;
        let mut walk_up = false;
        let style = self.feel_for(who, lib).voicing;

        if style == 1 {
            if change_next && sib >= self.spb() - 2 {
                rh_chord = nx;
                lead_from = ch;
            } else if liberties && self.energy <= 2 && ch.flavor == 0 && sib < 6 && self.rand01(pass, prog_bar, 501, 0, cx) < 0.5f32 {
                rh_chord = self.sus_of(ch);
            }
        } else if style == 3 && liberties {
            let rise = self.positive_modulo(nx.degree - ch.degree, 7);
            if sib >= fill_zone_start(self.meter) && change_next && rise == 1 {
                let dim = self.raw_chord(nx.root_pc - 1, 3, 6, 9, 4);
                rh_chord = dim;
                lh_chord = dim;
                lead_from = ch;
            } else if ch.degree == 0 && nx.degree == 3 && change_next && ch.flavor != 7 {
                walk_up = true;
            }

            if prog_bar == self.num_bars() - 1
                && self.energy <= 2
                && ch.degree == 0
                && nx.degree == 0
                && sib >= half_step(self.meter)
                && self.rand01(pass, prog_bar, 503, 0, cx) < 0.5f32
            {
                rh_chord = self.build_chord(3 * 16);
                lh_chord = rh_chord;
                lead_from = ch;
            }
        } else if style == 4 && liberties && change_next && sib >= self.spb() - 2 {
            let mut approach = nx;
            approach.root_pc = self.positive_modulo(nx.root_pc + 1, 12);
            rh_chord = approach;
            lead_from = ch;
        }

        let v = self.voicing_for(who, rh_chord, lead_from, lib);
        self.clear_step();

        // Right hand: grid, unless a fill owns this step (a hemiola bar, Task 17, keeps the whole bar regrouped: no fill).
        let hemiola = self.hemiola_bar(who, pass, prog_bar, cx);
        let fill = !hemiola && sib >= half_step(self.meter) && self.fill_active(who, pass, prog_bar, change_next, true, cx, out);
        let mut grid_rh = true;

        if fill {
            let variant = self.fill_variant(who, pass, prog_bar, cx);
            let target = self.voicing_for(who, nx, ch, lib);
            let current = self.voicing_for(who, ch, pv, lib);
            grid_rh = !self.play_fill(who, variant, sib, target, current, tick, lib);
        }

        if grid_rh {
            let rs = self.keys_hand_step(who, 1, prog_bar, sib, hemiola, lib); // the meter's oom-pah-pah (4/4: gstep)
            let prob = if rs < 0 { 0 } else { self.grid_at(who, 1, rs, 0, lib) };
            if prob > 0 && self.rand01(step, pass, 601, who, cx) * 100.0f32 < prob as f32 {
                let type_ = self.grid_at(who, 1, rs, 3, lib);
                let vel = self.grid_at(who, 1, rs, 1, lib) as f32;
                let len = self.grid_at(who, 1, rs, 2, lib) as f32 * tick * self.length;
                self.add_right_hand(type_, v, vel, len, 0.0f32);
                if hemiola {
                    HEMIOLA_NOTES.fetch_add(1, Ordering::Relaxed);
                    if self.meter == Meter::M44 || !self.is_phrase_end(prog_bar) {
                        HEMIOLA_OFF.fetch_add(1, Ordering::Relaxed);
                    }
                }

                // Nash's slip note: a grace a whole step under the top voice, sliding in just ahead.
                if style == 9 && v.count > 0 && self.rand01(step, pass, 641, who, cx) < (if self.energy >= 3 { 0.45f32 } else { 0.25f32 }) {
                    self.add_step_note(v.notes.val(v.count - 1) + self.octave_shift * 12 - 2, 50.0f32, tick * 0.4f32, 1, -40.0f32);
                }
            }
        }

        // Left hand: grid, with Deacon's walk-up into IV.
        if walk_up && (sib == self.spb() - 4 || sib == self.spb() - 2) {
            let r = self.lh_root(who, ch, lib);
            let mode = self.scale_mode;
            let step_up = if sib == self.spb() - 4 { 1 } else { 2 };
            self.add_step_note(
                r + self.scale_offset_in(mode, ch.degree + step_up) - self.scale_offset_in(mode, ch.degree),
                76.0f32,
                tick * 2.0f32 * self.length,
                0,
                0.0f32,
            );
        } else {
            let ls = self.keys_hand_step(who, 0, prog_bar, sib, hemiola, lib);
            let prob = if ls < 0 { 0 } else { self.grid_at(who, 0, ls, 0, lib) };
            if prob > 0 && self.rand01(step, pass, 607, who, cx) * 100.0f32 < prob as f32 {
                let type_ = self.grid_at(who, 0, ls, 3, lib);
                let vel = self.grid_at(who, 0, ls, 1, lib) as f32;
                let len = self.grid_at(who, 0, ls, 2, lib) as f32 * tick * self.length;
                self.add_left_hand(who, type_, lh_chord, vel, len, lib);
            }
        }

        // A half-bar chord the grid has no chord hit for: re-voice what the hands hold (spec 2026-10-28 §2).
        if self.revoice_due(ch, pv, step_ppq, cx) && !self.step_has_hand(1) {
            let (vel, lh) = self.revoice_vel(who, pv, step_ppq, lib);
            if vel > 0.0f32 {
                let len = self.revoice_len(step_ppq, lib);
                self.add_right_hand(1, v, vel, len, 0.0f32);
                if lh && !self.step_has_hand(0) {
                    self.add_left_hand(who, 1, lh_chord, vel, len, lib);
                }
            }
        }
        self.note_chord_strike(1, step_ppq);
        if !self.dry_run {
            // a push's look-ahead sends no pedal
            self.pedal_events(who, prog_bar, sib, !self.same_chord(ch, pv), step_ppq, Self::bass_of(&ch) == Self::bass_of(&pv), lib);
        }
        self.perform_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
    }
}

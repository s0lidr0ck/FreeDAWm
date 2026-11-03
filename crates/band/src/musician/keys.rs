//! `MusicianKeys.cpp`: keys (Session player, Persona player, pedal, harmonic lift).
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.

// Line-by-line translation: the C++ shapes (a default then field writes, a counter beside a loop, a late init) are kept.
#![allow(clippy::field_reassign_with_default, clippy::explicit_counter_loop, clippy::needless_late_init)]

use super::*;
use crate::library::{K_GRID_INTS, K_GRID12_INTS};
use crate::meter::{is_light_beat, strong_step};

/// The release-velocity hash: `(uint32) (int64 (offPpq * 960.0)) * 2654435761u ^ (uint32) note * 40503u`.
fn off_hash(off_ppq: f64, note: i32) -> u32 {
    ((off_ppq * 960.0) as i64 as u32).wrapping_mul(2654435761u32) ^ (note as u32).wrapping_mul(40503u32)
}

impl Musician {
    // Strong steps for the session player's energy thinning: 4/4 beats 1 and 3, 6/8 the two dotted beats, 3/4 the downbeat.

    // Reached only when activePlayer() is 0: MS has no Session player (setPlayer gives the instrument's first player), and
    // every factory instrument has players, so only a user Library leaving an instrument playerless gets here.
    /// `void sessionStep (int step, int pass, int progBar, int sib, float64 stepPpq)`
    pub(crate) fn session_step(&mut self, step: i32, pass: i32, prog_bar: i32, sib: i32, step_ppq: f64, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        // The session patterns are 4 bars of 16 steps; 12-step meters read them through gridStep (comp), bar by bar.
        let slot0 = (self.pattern * 4 + self.variation) * 64 + self.positive_modulo(self.floor_div(step, self.spb()), 4) * 16;
        let src = self.grid_step_in(0, sib, GridRole::Comp, |k| Self::offsets(slot0 + k + 1) > Self::offsets(slot0 + k), lib);
        let s = if self.meter == Meter::M44 {
            self.positive_modulo(step - self.origin_step(lib), 64)
        } else {
            self.positive_modulo(self.floor_div(step - self.origin_step(lib), self.spb()), 4) * 16 + max(0, src)
        };
        let idea_bar = s / 16;
        let slot = (self.pattern * 4 + self.variation) * 64 + s;
        let drums = self.is_drums();
        let authored = AUTHORED_ROOTS.val(self.variation * 4 + idea_bar);
        let ch = self.chord_for_bar(prog_bar, cx);
        let shift = if drums { 0 } else { self.wrap_shift(ch.degree - authored) };
        let borrowed = ch.flavor == 7 || ch.flavor == 8;
        let tick = self.tick_ppq(lib);
        let base = Self::bases(self.pattern) + self.key_root + self.octave_shift * 12;
        let mut loudest = 0;

        self.clear_step();

        let mut i = Self::offsets(slot);
        while src >= 0 && i < Self::offsets(slot + 1) {
            let d = Self::degrees(i);
            let vel = Self::velocities(i) as f32;

            // Energy: 1-2 thin the part, 3 plays it as authored, 4 thickens it.
            if self.energy == 1 && !(strong_step(self.meter, sib) || (is_beat(self.meter, sib) && Self::velocities(i) >= 85)) {
                i += 1;
                continue;
            }
            if self.energy == 2 && Self::velocities(i) < 78 && !is_beat(self.meter, sib) && self.rand01(step, pass, 41, i, cx) < 0.6f32 {
                i += 1;
                continue;
            }

            let mut note = d;
            if !drums {
                let new_deg = d + shift;
                note = base + self.scale_offset_in(if borrowed { 1 - self.scale_mode } else { self.scale_mode }, new_deg);
                if !borrowed {
                    note = base + self.scale_offset(new_deg);
                }

                if ch.flavor != 0 {
                    // Re-spell chord tones of the authored chord onto the target chord.
                    let rel = self.positive_modulo(d - authored, 7);
                    let root_deg = new_deg - rel;
                    let root_pitch = base + 12 * self.floor_div(root_deg, 7) + ch.root_pc;
                    if rel == 0 || rel == 2 || rel == 4 {
                        note = root_pitch + ch.tones.val(rel / 2);
                    } else if rel == 6 && ch.count >= 4 && ch.tones.val(3) < 12 {
                        note = root_pitch + ch.tones.val(3);
                    }
                }
            }

            self.add_step_note(note, vel, Self::durations(i) * tick * self.length, if self.pattern == 2 { 0 } else { 1 }, 0.0f32);
            if Self::velocities(i) > loudest {
                loudest = Self::velocities(i);
            }
            i += 1;
        }

        if self.step_count > 0 && !drums {
            let mut hi = 0;
            for i in 1..self.step_count {
                if self.step_note.val(i) > self.step_note.val(hi) {
                    hi = i;
                }
            }

            // Chord colour (7th / 9th / 6th) on strong-beat chordal hits.
            if (sib == 0 || (if self.meter == Meter::M34 { is_light_beat(self.meter, sib) } else { sib == half_step(self.meter) }))
                && self.is_chordal(self.pattern)
                && ch.count >= 4
                && self.step_count >= 2
            {
                let pc = self.positive_modulo(self.key_root + ch.root_pc + ch.tones.val(3), 12);
                let mut present = false;
                for i in 0..self.step_count {
                    if self.positive_modulo(self.step_note.val(i), 12) == pc {
                        present = true;
                    }
                }
                if !present {
                    let mut p = self.step_note.val(hi) - 1;
                    while self.positive_modulo(p, 12) != pc {
                        p -= 1;
                    }
                    self.add_step_note(p, self.step_vel.val(hi) - 12.0f32, self.step_len.val(hi), 1, 0.0f32);
                }
            }

            if self.energy == 4 && is_beat(self.meter, sib) && loudest >= 85 {
                self.add_step_note(
                    self.step_note.val(hi) + (if self.pattern == 2 { -12 } else { 12 }),
                    self.step_vel.val(hi) - 10.0f32,
                    self.step_len.val(hi),
                    1,
                    0.0f32,
                );
            }
        }

        // Slash bass: the part's lowest note, when it is the chord root, moves down to the bass note.
        if ch.bass_pc >= 0 && !drums && self.step_count > 0 && (self.is_chordal(self.pattern) || self.pattern == 2) {
            let mut lo = 0;
            for i in 1..self.step_count {
                if self.step_note.val(i) < self.step_note.val(lo) {
                    lo = i;
                }
            }
            if self.positive_modulo(self.step_note.val(lo) - (self.key_root + ch.root_pc), 12) == 0 {
                let n = self.slash_at_or_below(self.step_note.val(lo), &ch);
                self.step_note.set(lo, n);
            }
        }

        if drums && self.energy == 4 && sib % 2 == 1 && self.rand01(step, pass, 43, 0, cx) < 0.35f32 {
            let mut hat = false;
            for i in 0..self.step_count {
                if self.step_note.val(i) == 42 || self.step_note.val(i) == 46 {
                    hat = true;
                }
            }
            if !hat {
                self.add_step_note(42, 40.0f32, tick, 1, 0.0f32);
            }
        }

        self.perform_step(0, step, pass, prog_bar, sib, step_ppq, cx, out);
    }

    /// `Voicing buildVoicing (int who, Chord c, Voicing prev)`
    pub(crate) fn build_voicing(&mut self, who: i32, c: Chord, prev: Voicing, lib: &LibraryData) -> Voicing {
        let mut pcs: CArr<i32, 5> = CArr::default();
        let mut n = 0;
        let root = self.key_root + c.root_pc;
        let third = c.tones.val(1);
        let fifth = c.tones.val(2);
        let has_seventh = c.count >= 4 && (c.tones.val(3) == 10 || c.tones.val(3) == 11 || c.tones.val(3) == 9);
        let seventh = if has_seventh { c.tones.val(3) } else { c.seventh };
        let f = self.feel_for(who, lib);
        let low = f.rh_low;
        let mut high = f.rh_high;
        let anchor = f.rh_anchor;
        let style = f.voicing;

        if style == 7 {
            return self.guitar_shape(c);
        }
        if style == 6 {
            high -= 12; // the arp doubles the triad an octave up afterwards
        }

        if style == 1 {
            // Clay: the chord as written, close position.
            let mut i = 0;
            while i < c.count && i < 4 {
                pcs.set(n, root + c.tones.val(i));
                n += 1;
                i += 1;
            }
        } else if style == 9 {
            // Nash: close triad with the 6th on top (or the chord's own 7th / 9th if it has one).
            pcs.set(0, root);
            pcs.set(1, root + third);
            pcs.set(2, root + fifth);
            pcs.set(3, root + (if c.count >= 4 && c.tones.val(3) != 9 { c.tones.val(3) % 12 } else { 9 }));
            n = 4;
        } else if style == 5 {
            // Ensemble: four close parts, doubling the root on triads.
            let mut i = 0;
            while i < c.count && i < 4 {
                pcs.set(n, root + (c.tones.val(i) % 12));
                n += 1;
                i += 1;
            }
            if n == 3 {
                pcs.set(3, root);
                n = 4;
            }
        } else if style == 6 || style == 8 {
            // Arp source / funk chank: the triad (3rd-5th-7th on seventh chords for the chank).
            if style == 8 && c.count >= 4 && c.tones.val(3) < 12 {
                pcs.set(0, root + third);
                pcs.set(1, root + fifth);
                pcs.set(2, root + c.tones.val(3));
            } else {
                pcs.set(0, root);
                pcs.set(1, root + third);
                pcs.set(2, root + fifth);
            }
            n = 3;
        } else if style == 3 {
            // Deacon: rootless colours.
            if fifth == 6 && third == 3 {
                pcs.set(0, root + 3);
                pcs.set(1, root + 6);
                pcs.set(2, root + 9);
                n = 3;
            } else if third == 4 && ((has_seventh && c.tones.val(3) == 10) || (!has_seventh && c.seventh == 10 && c.flavor == 0)) {
                pcs.set(0, root + 4);
                pcs.set(1, root + 10);
                pcs.set(2, root + 2);
                pcs.set(3, root + 9);
                n = 4;
            } else if has_seventh {
                pcs.set(0, root + third);
                pcs.set(1, root + fifth);
                pcs.set(2, root + c.tones.val(3));
                pcs.set(3, root + 2);
                n = 4;
            } else if third == 4 {
                pcs.set(0, root + 4);
                pcs.set(1, root + 7);
                pcs.set(2, root + 9);
                pcs.set(3, root + 2);
                n = 4;
            } else {
                pcs.set(0, root + third);
                pcs.set(1, root + fifth);
                pcs.set(2, root + 10);
                pcs.set(3, root + 2);
                n = 4;
            }
        } else if style == 4 {
            // Mara: 3-5-7-9, dropped-2 below.
            pcs.set(0, root + third);
            pcs.set(1, root + (if third == 3 && self.energy >= 3 { 5 } else { fifth }));
            pcs.set(2, root + seventh);
            pcs.set(3, root + 2);
            n = 4;
        }

        let mut best = Voicing::default();
        best.count = 0;

        if style == 2 {
            // Selah: wide and open. Keep the previous top if it is still a chord colour.
            let mut set: CArr<i32, 5> = CArr::default();
            set.set(0, root);
            set.set(1, root + fifth);
            set.set(2, root + 2);
            let mut m = 3;
            if self.energy >= 3 && c.tones.val(1) != 2 && c.tones.val(1) != 5 {
                set.set(3, root + third);
                m = 4;
            }
            if c.tones.val(1) == 5 {
                set.set(m, root + 5);
                m += 1;
            }

            let mut top = -1;
            if prev.count > 0 {
                let pt = prev.notes.val(prev.count - 1);
                for k in 0..m {
                    if self.positive_modulo(pt - set.val(k), 12) == 0 {
                        top = pt;
                    }
                }
            }
            if top < 0 {
                let mut best_dist = 99;
                for p in (anchor - 6)..=(anchor + 6) {
                    for k in 0..m {
                        if self.positive_modulo(p - set.val(k), 12) == 0 && (p - anchor).abs() < best_dist {
                            best_dist = (p - anchor).abs();
                            top = p;
                        }
                    }
                }
            }

            let mut down: CArr<i32, 6> = CArr::default();
            let mut cnt = 1;
            down.set(0, top);
            let mut last = top;
            let mut p = top - 3;
            while p >= low - 5 && cnt < 4 {
                let mut ok = false;
                for k in 0..m {
                    if self.positive_modulo(p - set.val(k), 12) == 0 {
                        ok = true;
                    }
                }
                if ok && last - p >= 4 {
                    down.set(cnt, p);
                    cnt += 1;
                    last = p;
                }
                p -= 1;
            }
            best.count = cnt;
            for k in 0..cnt {
                best.notes.set(k, down.val(cnt - 1 - k));
            }
            return best;
        }

        let mut best_cost = 1000000;
        for inv in 0..n {
            for oct in 0..3 {
                let mut v = Voicing::default();
                v.count = n;
                let start = low + self.positive_modulo(pcs.val(inv) - low, 12) + 12 * oct;
                v.notes.set(0, start);
                let mut last = start;
                for j in 1..n {
                    let pc = pcs.val((inv + j) % n);
                    let p = last + 1 + self.positive_modulo(pc - last - 1, 12);
                    v.notes.set(j, p);
                    last = p;
                }
                if last > high {
                    continue;
                }

                let top = v.notes.val(n - 1);
                let mut cost = (top - anchor).abs();
                if prev.count > 0 {
                    cost = (top - prev.notes.val(prev.count - 1)).abs() * 2 + (top - anchor).abs() / 2;
                    for j in 0..n {
                        let mut nearest = 99;
                        for k in 0..prev.count {
                            nearest = min(nearest, (v.notes.val(j) - prev.notes.val(k)).abs());
                        }
                        cost += nearest;
                    }
                }
                if cost < best_cost {
                    best_cost = cost;
                    best = v;
                }
            }
        }

        if best.count == 0 {
            // Nothing fitted the register: stack from the bottom anyway.
            best.count = n;
            let mut last = low - 1;
            for j in 0..n {
                let p = last + 1 + self.positive_modulo(pcs.val(j) - last - 1, 12);
                best.notes.set(j, p);
                last = p;
            }
        }

        if style == 4 && best.count >= 4 {
            // Drop-2: second voice from the top goes down an octave, then re-sort.
            let dropped = best.notes.val(best.count - 2) - 12;
            let mut j = best.count - 2;
            while j > 0 {
                let v = best.notes.val(j - 1);
                best.notes.set(j, v);
                j -= 1;
            }
            best.notes.set(0, dropped);
        }

        if style == 6 && best.count == 3 {
            // Two octaves of the triad for arpeggios and cells.
            for j in 0..3 {
                let v = best.notes.val(j) + 12;
                best.notes.set(3 + j, v);
            }
            best.count = 6;
        }
        best
    }

    /// `Voicing guitarShape (Chord c)`
    pub(crate) fn guitar_shape(&mut self, c: Chord) -> Voicing {
        let mut v = Voicing::default();
        let root = self.key_root + c.root_pc;
        let r = 40 + self.positive_modulo(root - 40, 12);
        let third = c.tones.val(1);
        let fifth = c.tones.val(2);
        let has_seventh = c.count >= 4 && c.tones.val(3) < 12 && c.tones.val(3) != 9;
        let e_shape = r <= 45;

        v.notes.set(0, r);
        v.notes.set(1, r + fifth);
        v.notes.set(2, if has_seventh { r + c.tones.val(3) } else { r + 12 });
        v.notes.set(3, r + 12 + third);
        v.notes.set(4, r + 12 + fifth);
        v.count = 5;
        if e_shape {
            v.notes.set(5, r + 24);
            v.count = 6;
        }

        if c.count >= 4 && c.tones.val(3) == 14 {
            if e_shape {
                v.notes.set(5, r + 26);
            } else {
                v.notes.set(5, r + 26);
                v.count = 6;
            }
        }
        if c.count >= 4 && c.tones.val(3) == 9 {
            if e_shape {
                v.notes.set(5, r + 21);
            } else {
                v.notes.set(5, r + 21);
                v.count = 6;
            }
        }
        v
    }

    /// `Voicing voicingFor (int who, Chord c, Chord previous)`
    pub(crate) fn voicing_for(&mut self, who: i32, c: Chord, previous: Chord, lib: &LibraryData) -> Voicing {
        let mut none = Voicing::default();
        none.count = 0;
        let prev = self.build_voicing(who, previous, none, lib);
        self.build_voicing(who, c, prev, lib)
    }

    /// `int lhRoot (int who, Chord c)`
    pub(crate) fn lh_root(&mut self, who: i32, c: Chord, lib: &LibraryData) -> i32 {
        let low = self.feel_for(who, lib).lh_low;
        let root = self.key_root + c.root_pc;
        low + self.positive_modulo(root - low, 12) + self.octave_shift * 12
    }

    /// `int lhBass (int who, Chord c)`
    pub(crate) fn lh_bass(&mut self, who: i32, c: Chord, lib: &LibraryData) -> i32 {
        if c.bass_pc < 0 {
            return self.lh_root(who, c, lib);
        }
        let low = self.feel_for(who, lib).lh_low;
        low + self.positive_modulo(self.key_root + c.bass_pc - low, 12) + self.octave_shift * 12
    }

    /// `void addLeftHand (int who, int type, Chord c, float vel, float len)`
    pub(crate) fn add_left_hand(&mut self, who: i32, type_: i32, c: Chord, vel: f32, len: f32, lib: &LibraryData) {
        let r = self.lh_root(who, c, lib);
        let b = self.lh_bass(who, c, lib); // == r unless the chord has a slash bass
        let third = c.tones.val(1);
        let has_seventh = c.count >= 4 && (c.tones.val(3) == 10 || c.tones.val(3) == 11);
        let seventh = if has_seventh { c.tones.val(3) } else { c.seventh };

        if type_ == 1 {
            self.add_step_note(b, vel, len, 0, 0.0f32);
        } else if type_ == 2 {
            self.add_step_note(r + c.tones.val(2), vel, len, 0, 0.0f32);
        } else if type_ == 3 {
            self.add_step_note(b + 12, vel, len, 0, 0.0f32);
        } else if type_ == 4 {
            self.add_step_note(b, vel, len, 0, 0.0f32);
            self.add_step_note(b + 12, vel - 6.0f32, len, 0, 0.0f32);
        } else if type_ == 5 {
            self.add_step_note(b, vel, len, 0, 0.0f32);
            self.add_step_note(r + seventh, vel - 8.0f32, len, 0, 0.0f32);
        } else if type_ == 6 {
            self.add_step_note(b, vel, len, 0, 0.0f32);
            self.add_step_note(r + 12 + third, vel - 8.0f32, len, 0, 0.0f32);
        } else if type_ == 7 {
            self.add_step_note(b, vel, len, 0, 0.0f32);
        } else if type_ == 8 {
            self.add_step_note(r + third, vel, len, 0, 0.0f32);
        } else if type_ == 9 {
            self.add_step_note(r + 7, vel, len, 0, 0.0f32);
        } else if type_ == 10 {
            self.add_step_note(r + 9, vel, len, 0, 0.0f32);
        } else if type_ == 11 {
            self.add_step_note(r + 10, vel, len, 0, 0.0f32);
        }
    }

    /// `void addRightHand (int type, Voicing v, float vel, float len, float shiftMs)`
    pub(crate) fn add_right_hand(&mut self, type_: i32, v: Voicing, vel: f32, len: f32, shift_ms: f32) {
        if v.count == 0 {
            return;
        }
        let shift = self.octave_shift * 12;
        let top = v.notes.val(v.count - 1) + shift;

        if type_ == 1 {
            for k in 0..v.count {
                self.add_step_note(v.notes.val(k) + shift, vel, len, 1, shift_ms);
            }
        } else if type_ == 2 {
            self.add_step_note(top, vel, len, 1, shift_ms);
        } else if type_ == 3 {
            for k in 0..v.count - 1 {
                self.add_step_note(v.notes.val(k) + shift, vel, len, 1, shift_ms);
            }
        } else if (4..=8).contains(&type_) {
            self.add_step_note(v.notes.val(min(type_ - 4, v.count - 1)) + shift, vel, len, 1, shift_ms);
        } else if type_ == 9 {
            self.add_step_note(top, vel, len, 1, shift_ms);
            self.add_step_note(top + 12, vel - 4.0f32, len, 1, shift_ms);
        } else if type_ == 10 {
            self.add_step_note(top - 12, vel, len, 1, shift_ms);
        }
    }

    /// `int gridAt (int who, int hand, int gstep, int field)`
    pub(crate) fn grid_at(&mut self, who: i32, hand: i32, gstep: i32, field: i32, lib: &LibraryData) -> i32 {
        // The persona's own grid (was the flat personaGrid0..3 tables offset by (who - 1)).
        if who <= 0 || who > K_MAX_SEED as i32 {
            return 0;
        }
        let Some(p) = lib.persona(who) else { return 0 };
        let mut e = self.energy;
        let mut gs = gstep;
        if self.phrase_mode == 1 {
            e = if self.energy < 4 { self.energy + 1 } else { self.energy - 1 }; // Natural phrasing: the bar from the next Energy
        } else if self.phrase_mode == 2 {
            gs = self.positive_modulo(gstep + 16, 32); // ... or the other bar of the pair
        }
        let v;
        if self.meter != Meter::M44 && self.native_here(who, lib) {
            // native grids12: gs = bar * 16 + step, step 0..11
            let b = self.positive_modulo(gs, 32) / 16;
            let st = self.positive_modulo(gs, 16);
            if st >= 12 {
                return 0;
            }
            let idx = (((e - 1) * 4 + hand) * 24 + b * 12 + st) * 4 + field;
            v = at_or_default(&p.grids12, self.positive_modulo(idx, K_GRID12_INTS as i32));
        } else {
            let idx = (((e - 1) * 4 + hand) * 32 + gs) * 4 + field;
            v = at_or_default(&p.grid, self.positive_modulo(idx, K_GRID_INTS as i32));
        }
        if self.phrase_mode == 3 && field == 0 && v > 0 && self.positive_modulo(gs, 16) != 0 {
            // ... or thinned: off 16ths and the less certain hits go
            if self.positive_modulo(gs, 2) == 1 || v < 85 {
                return 0;
            }
        }
        v
    }

    // The keys' hand grid in the meter (spec 2.2 boom-chick / stride): the 32-step grid index a hand reads at sib, or -1.
    // 4/4: the step itself. 3/4 oom-pah-pah: the bass on 0, the chord on 4 and 8 (a source chord on 12 moves to an empty
    // 8). 6/8: the bass on 0 and 6 (source 0, 8), the chord on 3 and 9 (source 4, 12, else the half's downbeat 0, 8).
    /// `int keysHandStep (int who, int hand, int progBar, int sib, bool hemiola)`
    pub(crate) fn keys_hand_step(&mut self, who: i32, hand: i32, prog_bar: i32, sib: i32, hemiola: bool, lib: &LibraryData) -> i32 {
        let bar = self.positive_modulo(prog_bar, 2) * 16;
        if self.meter == Meter::M44 || self.native_here(who, lib) {
            return bar + sib;
        }
        let mut src = -1;
        // A hemiola bar (Task 17) swaps the rules: 6/8 as three quarters (3/4's oom-pah-pah), 3/4 as two dotted quarters
        // (6/8's bass on 0 and 6, chord on 3 and 9). A chord missing from the source's beat falls back a beat.
        let m = if hemiola { if self.meter == Meter::M34 { Meter::M68 } else { Meter::M34 } } else { self.meter };
        if m == Meter::M34 {
            if hand == 0 {
                src = if sib == 0 { 0 } else { -1 };
            } else if is_light_beat(m, sib) {
                src = sib;
            }
            if hand == 1 && sib == 8 && self.grid_at(who, 1, bar + 8, 0, lib) == 0 && self.grid_at(who, 1, bar + 12, 0, lib) > 0 {
                src = 12;
            }
            if hemiola && hand == 1 && src >= 4 && self.grid_at(who, 1, bar + src, 0, lib) == 0 {
                src -= 4;
            }
        } else {
            let h = half_step(m);
            if hand == 0 {
                src = if sib == 0 {
                    0
                } else if sib == h {
                    8
                } else {
                    -1
                };
            } else {
                src = if sib == h / 2 {
                    4
                } else if sib == h + h / 2 {
                    12
                } else {
                    -1
                };
            }
            if src >= 0 && hand == 1 && self.grid_at(who, 1, bar + src, 0, lib) == 0 {
                src -= 4; // no chord on the source's beat: the half's downbeat chord
            }
        }
        if src < 0 { -1 } else { bar + src }
    }

    /// `bool fillActive (int who, int pass, int progBar, bool changeNext, bool mark)`
    pub(crate) fn fill_active(
        &mut self,
        who: i32,
        pass: i32,
        prog_bar: i32,
        change_next: bool,
        mark: bool,
        cx: &mut Ctx<'_>,
        out: &mut dyn MidiSink,
    ) -> bool {
        if self.perf_fill && pass == self.step_pass && prog_bar == self.step_prog_bar {
            return true; // the Fill key
        }
        if self.pickup_now && self.perf.natural && prog_bar == self.pickup_prog_bar {
            if mark {
                self.annotate(1, self.step_ppq_now, cx, out);
            }
            return true;
        } // Natural: leading into the next section
        if self.perf.fills_yield
            && !self.is_drums()
            && self.step_ppq_now >= cx.shared.drum_fill_from - 1.0e-9
            && self.step_ppq_now < cx.shared.drum_fill_to
        {
            return false; // the drummer has it
        }
        let phrase_end = self.is_phrase_end(prog_bar);
        // 0-50%: up to the persona's own rate; 50-100%: from its rate up to every phrase.
        let base = self.feel_for(who, cx.lib).fill_rate;
        let mut rate = if self.fills <= 50.0f32 { base * self.fills / 50.0f32 } else { base + (1.0f32 - base) * (self.fills - 50.0f32) / 50.0f32 };
        if !phrase_end {
            rate = if self.feel_for(who, cx.lib).voicing == 3 && change_next { rate * 0.3f32 } else { 0.0f32 };
        }
        let fill = self.rand01(pass, prog_bar, 901, who, cx) < min(rate, 1.0f32);
        if mark && fill && self.rendering && prog_bar == self.step_prog_bar && pass == self.step_pass {
            self.annotate(1, self.step_ppq_now, cx, out); // the Band view's mark
        }
        fill
    }

    /// `int shuffledFill (int who, int block, int index)`
    pub(crate) fn shuffled_fill(&mut self, who: i32, block: i32, index: i32, cx: &mut Ctx<'_>) -> i32 {
        let n = self.fill_count(who, cx.lib);
        let mut order: CArr<i32, 5> = CArr::default();
        for k in 0..5 {
            order.set(k, k);
        }
        let mut k = n - 1;
        while k > 0 {
            let r = min(k, (self.rand01(block, k, 913, who, cx) * (k + 1) as f32) as i32);
            let tmp = order.val(k);
            order.set(k, order.val(r));
            order.set(r, tmp);
            k -= 1;
        }
        order.val(index)
    }

    /// `int fillVariant (int who, int pass, int progBar)`
    pub(crate) fn fill_variant(&mut self, who: i32, pass: i32, prog_bar: i32, cx: &mut Ctx<'_>) -> i32 {
        let bars = self.num_bars();
        let n = self.fill_count(who, cx.lib);
        let phrase_end = self.is_phrase_end(prog_bar);
        if !phrase_end {
            return min(n - 1, (self.rand01(pass, prog_bar, 917, who, cx) * n as f32) as i32);
        }

        // whole phrases of 4 (a last short one counts too)   // 8 chords: pass * 2 + (progBar == 7), as before
        let phrase = if bars >= 8 { pass * ((bars + 3) / 4) + prog_bar / 4 } else { pass };
        let block = self.floor_div(phrase, n);
        let index = phrase - block * n;
        let first = self.shuffled_fill(who, block, 0, cx);

        if first == self.shuffled_fill(who, block - 1, n - 1, cx) {
            if index == 0 {
                return self.shuffled_fill(who, block, 1, cx);
            }
            if index == 1 {
                return first;
            }
        }
        self.shuffled_fill(who, block, index, cx)
    }

    /// `bool playFill (int who, int variant, int sibInBar, Voicing target, Voicing current, float tick)`
    pub(crate) fn play_fill(
        &mut self,
        who: i32,
        variant: i32,
        sib_in_bar: i32,
        target: Voicing,
        current: Voicing,
        tick: f32,
        lib: &LibraryData,
    ) -> bool {
        // 16-only: the fill vocabulary below is written over the 4/4 second half (steps 8..15). In 12-step meters it is
        // end-aligned (sib = step + 4) so it still lands on the bar line; the fill zone itself is the meter's.
        let sib = sib_in_bar + (16 - self.spb());
        let zone = sib_in_bar >= fill_zone_start(self.meter);
        let style = self.feel_for(who, lib).voicing;
        let shift = self.octave_shift * 12;
        let top = (if target.count > 0 { target.notes.val(target.count - 1) } else { 72 }) + shift;
        let cur_top = (if current.count > 0 { current.notes.val(current.count - 1) } else { 72 }) + shift;
        let rise = (sib - 8) as f32;

        if style == 9 {
            if variant == 0 {
                // Slip-note lick: whole step below each chord tone, walking up to the next top.
                if zone && target.count > 0 {
                    let k = min(target.count - 1, sib - 12);
                    let p = target.notes.val(k) + shift;
                    self.add_step_note(p - 2, 54.0f32, tick * 0.4f32, 1, -40.0f32);
                    self.add_step_note(p, 72.0f32 + (sib - 12) as f32 * 4.0f32, tick, 1, 0.0f32);
                }
            } else if variant == 1 {
                // Sixths walking down from the top.
                if sib % 2 == 0 {
                    let mut p = top;
                    for _ in 0..(14 - sib) / 2 {
                        p = self.step_down(p, 0);
                    }
                    self.add_step_note(p, 74.0f32, tick * 1.8f32, 1, 0.0f32);
                    let q = self.step_down(self.step_down(self.step_down(self.step_down(p, 0), 0), 0), 0);
                    self.add_step_note(q, 64.0f32, tick * 1.8f32, 1, 0.0f32);
                }
            } else if sib == 12 || sib == 14 {
                // Two pickup chords into the change, the second a slip from below.
                if sib == 14 {
                    for k in 0..target.count {
                        self.add_step_note(target.notes.val(k) + shift - 2, 50.0f32, tick * 0.4f32, 1, -40.0f32);
                    }
                }
                self.add_right_hand(1, if sib == 12 { current } else { target }, if sib == 12 { 68.0f32 } else { 78.0f32 }, tick * 2.0f32, 0.0f32);
            }
            return true;
        }

        if style == 1 {
            if variant == 0 {
                // Scale run up into the next chord's top note, then push the chord.
                if sib == 8 {
                    self.add_right_hand(1, current, 70.0f32, tick * 2.0f32, 0.0f32);
                } else if (10..=13).contains(&sib) {
                    let mut p = top;
                    for _ in 0..14 - sib {
                        p = self.step_down(p, 0);
                    }
                    self.add_step_note(p, 60.0f32 + (rise - 2.0f32) * 4.0f32, tick, 1, 0.0f32);
                } else if sib == 14 {
                    self.add_right_hand(1, target, 74.0f32, tick * 2.0f32, 0.0f32);
                }
            } else if variant == 1 {
                // Falling run from above, landing on the next chord's top.
                if zone {
                    let mut p = top;
                    for _ in 0..16 - sib {
                        p = self.step_up(p, 0);
                    }
                    self.add_step_note(p, 72.0f32 - (rise - 4.0f32) * 3.0f32, tick, 1, 0.0f32);
                } else if sib == 8 {
                    self.add_right_hand(1, current, 68.0f32, tick * 3.0f32, 0.0f32);
                }
            } else if variant == 2 {
                // Rhythmic pickup stabs: current, current, next.
                if sib == 10 || sib == 12 {
                    self.add_right_hand(1, current, if sib == 10 { 64.0f32 } else { 70.0f32 }, tick, 0.0f32);
                } else if sib == 14 {
                    self.add_right_hand(1, target, 78.0f32, tick * 2.0f32, 0.0f32);
                }
            } else {
                // Broken arpeggio of the next chord climbing in 8ths.
                if sib % 2 == 0 && target.count > 0 {
                    let k = (sib - 8) / 2;
                    let note = if k < target.count { target.notes.val(k) + shift } else { top + 12 };
                    self.add_step_note(note, 58.0f32 + rise * 2.5f32, tick * 2.0f32, 1, 0.0f32);
                }
            }
            return true;
        }

        if style == 2 {
            if variant == 0 {
                // Octave-motif lift.
                if sib % 2 == 0 {
                    self.add_step_note(if (sib / 2) % 2 == 0 { top } else { top + 12 }, 56.0f32 + rise * 2.0f32, tick * 2.0f32, 1, 0.0f32);
                }
            } else if variant == 1 {
                // Slow rising arpeggio that blooms an octave up.
                if sib % 2 == 0 && target.count > 0 {
                    let k = (sib - 8) / 2;
                    let note = if k < target.count { target.notes.val(k) + shift } else { top + 12 };
                    self.add_step_note(note, 52.0f32 + rise * 2.0f32, tick * 4.0f32, 1, 0.0f32);
                }
            } else {
                // Held swell: the top an octave up, answered by the next top.
                if sib == 8 {
                    self.add_step_note(cur_top + 12, 54.0f32, tick * 6.0f32, 1, 0.0f32);
                } else if sib == 12 {
                    self.add_step_note(top, 62.0f32, tick * 4.0f32, 1, 0.0f32);
                }
            }
            return true;
        }

        if style == 3 {
            if variant == 0 {
                // Blues run climbing into the next chord.
                let mut p = top;
                for _ in 0..15 - sib {
                    p = self.step_down(p, 1);
                }
                self.add_step_note(p, 64.0f32 + rise * 4.5f32, tick, 1, 0.0f32);
                if self.energy == 4 {
                    self.add_step_note(p - 12, 58.0f32 + rise * 4.5f32, tick, 1, 0.0f32);
                }
            } else if variant == 1 {
                // Blues run cascading down from an octave above.
                let mut p = top + 12;
                for _ in 0..sib - 8 {
                    p = self.step_down(p, 1);
                }
                self.add_step_note(p, 96.0f32 - rise * 3.5f32, tick, 1, 0.0f32);
                if self.energy == 4 {
                    self.add_step_note(p - 12, 88.0f32 - rise * 3.5f32, tick, 1, 0.0f32);
                }
            } else if variant == 2 {
                // Turn around the top note on the last beat, after a held chord.
                if sib == 8 {
                    self.add_right_hand(1, current, 76.0f32, tick * 3.0f32, 0.0f32);
                } else if zone {
                    let note = if sib == 12 {
                        top
                    } else if sib == 13 {
                        self.step_up(top, 0)
                    } else if sib == 14 {
                        top
                    } else {
                        self.step_down(top, 0)
                    };
                    self.add_step_note(note, 70.0f32 + (sib - 12) as f32 * 5.0f32, tick, 1, 0.0f32);
                }
            } else if variant == 3 {
                // Chromatic chord walk-up: the next chord a whole step, then a half step below.
                if sib == 8 {
                    self.add_right_hand(1, current, 80.0f32, tick * 2.0f32, 0.0f32);
                } else if sib == 12 || sib == 14 {
                    let down = if self.respect {
                        0
                    } else if sib == 12 {
                        2
                    } else {
                        1
                    };
                    for k in 0..target.count {
                        self.add_step_note(target.notes.val(k) + shift - down, if sib == 12 { 78.0f32 } else { 86.0f32 }, tick * 2.0f32, 1, 0.0f32);
                    }
                }
            } else {
                // Octave-doubled pentatonic climb in 8ths.
                if sib % 2 == 0 {
                    let mut p = top;
                    for _ in 0..(14 - sib) / 2 + 1 {
                        p = self.step_down(p, 1);
                    }
                    self.add_step_note(p, 72.0f32 + rise * 3.0f32, tick * 2.0f32, 1, 0.0f32);
                    self.add_step_note(p - 12, 66.0f32 + rise * 3.0f32, tick * 2.0f32, 1, 0.0f32);
                }
            }
            return true;
        }

        if style == 4 {
            let t = cur_top;
            if variant == 0 && sib == 12 {
                // Grace-note cluster on beat 4.
                self.add_step_note(t - 1, 44.0f32, tick * 0.5f32, 1, -35.0f32);
                self.add_step_note(t, 58.0f32, tick * 2.0f32, 1, 0.0f32);
                self.add_step_note(t - 2, 52.0f32, tick * 2.0f32, 1, 0.0f32);
                if current.count >= 2 {
                    self.add_step_note(current.notes.val(current.count - 2) + shift, 50.0f32, tick * 2.0f32, 1, 0.0f32);
                }
                return true;
            }
            if variant == 1 && (sib == 10 || sib == 14) {
                // Two lighter clusters, the second slipping down from above.
                self.add_step_note(if sib == 10 { t - 1 } else { t + 1 }, 42.0f32, tick * 0.5f32, 1, -30.0f32);
                self.add_step_note(t, if sib == 10 { 50.0f32 } else { 56.0f32 }, tick * 1.5f32, 1, 0.0f32);
                if current.count >= 3 {
                    self.add_step_note(current.notes.val(current.count - 3) + shift, 46.0f32, tick * 1.5f32, 1, 0.0f32);
                }
                return true;
            }
            if variant == 2 && sib >= 13 {
                // Lazy three-note fall through the chord tones.
                let k = current.count - 1 - (sib - 13);
                if k >= 0 {
                    self.add_step_note(current.notes.val(k) + shift, 54.0f32 - (sib - 13) as f32 * 4.0f32, tick * 1.5f32, 1, 12.0f32);
                }
                return true;
            }
            if variant == 3 && sib == 14 {
                // Single slide into the next chord's top note.
                self.add_step_note(top - 1, 40.0f32, tick * 0.5f32, 1, -30.0f32);
                self.add_step_note(top, 60.0f32, tick * 2.0f32, 1, 0.0f32);
                return true;
            }
            return false;
        }
        false
    }

    /// `void naturalPedal (int mode, bool newChord, bool sameBass, float64 stepPpq, float64 lead, float64 ms)`
    pub(crate) fn natural_pedal(&mut self, mode: i32, new_chord: bool, same_bass: bool, step_ppq: f64, lead: f64, ms: f64, lib: &LibraryData) {
        let t = self.current_target(lib);
        let hold = if mode == 2 || self.energy <= 1 { (t.half_pedal[1] + t.half_pedal[2]) / 2 } else { 127 };
        let low = if new_chord {
            if same_bass { t.half_pedal[0] } else { 0 }
        } else if mode == 2 {
            t.half_pedal[0]
        } else {
            0
        };
        let from = if self.pedal_level >= 0 { self.pedal_level } else { hold };
        let attack = step_ppq + lead * ms;
        let lift_start = attack + 5.0 * ms;
        let lift_end = attack + 30.0 * ms;
        let press_end = attack + min(120.0 * ms, 0.5);
        let press_start = max(lift_end + 10.0 * ms, press_end - 60.0 * ms);
        for i in 0..4 {
            self.schedule(1, 64, from + (low - from) * (i + 1) / 4, lift_start + (lift_end - lift_start) * f64::from(i) / 3.0, 0.0f32);
        }
        for i in 0..5 {
            self.schedule(1, 64, low + (hold - low) * (i + 1) / 5, press_start + (press_end - press_start) * f64::from(i) / 4.0, 0.0f32);
        }
        self.pedal_level = hold;
        if self.pattern != 3 {
            let want_soft = if self.energy <= t.soft_pedal_max_energy { 127 } else { 0 };
            if want_soft != self.soft_pedal || t.soft_pedal_cc != self.soft_pedal_cc_sent {
                if self.soft_pedal > 0 && t.soft_pedal_cc != self.soft_pedal_cc_sent {
                    self.schedule(1, self.soft_pedal_cc_sent, 0, attack - 20.0 * ms, 0.0f32);
                }
                self.schedule(1, t.soft_pedal_cc, want_soft, attack - 20.0 * ms, 0.0f32);
                self.soft_pedal = want_soft;
                self.soft_pedal_cc_sent = t.soft_pedal_cc;
            }
        }
    }

    /// `void expressionOff ()`
    pub(crate) fn expression_off(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        if self.legato_on && self.legato_cc_sent >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.legato_cc_sent, 0, cx, out);
            self.legato_on = false;
        }
        self.drop_queued_wheel();
        self.ks_vibrato_off(cx, out); // keyswitch: its vibrato is Natural's
        self.bass_expression_lift(cx, out);
        for i in 0..64 {
            if self.pattern == 2
                && self.active_slots.val(i)
                && self.active_channels.val(i) == self.midi_channel0
                && (self.active_flags.val(i) & 8192) != 0
            {
                self.send_slot_note_off(i, cx, out);
            }
        }
        if self.continuous_pedal(lib) {
            self.send_midi(0xB0 + self.midi_channel0, 64, 0, cx, out);
            if self.soft_pedal > 0 {
                self.send_midi(0xB0 + self.midi_channel0, self.soft_pedal_cc_sent, 0, cx, out);
            }
            self.soft_pedal = -1;
        }
        self.harmonic_lift(cx, out);
        self.organ_expression_off(lib);
        self.brass_expression_off(cx, out);
        self.pedal_level = -1;
        if self.jb_sustain_sent && self.driver_is(Driver::Swam, lib) {
            self.send_midi(0xB0 + self.midi_channel0, 64, 0, cx, out); // Embertone: a run's sustain (its queued release goes below)
        }
        if self.jb_partner >= 0 {
            self.fd_move_off(self.jb_partner, self.current_ppq);
        }
        self.jb_reset(lib);
        if self.fd_partner >= 0 {
            self.fd_move_off(self.fd_partner, self.current_ppq);
        }
        self.fd_return_now(cx, out);
        self.fd_partner = -1;
        self.fd_chop_dip = false;
        self.fd_subs = 0;
        self.ample_vibrato_off(cx, out); // Ample: a held lead note's vibrato
        self.evolution_off(cx, out); // Evolution: held keys up now, nothing queued pressed later
        self.fd_prev_note = -1;
        self.fd_prev_on = -1.0;
        self.fd_prev_off = -1.0;
        self.fd_prev_end = false;
        self.fd_prev_fast = false;
    }

    /// `int releaseVelocity (int slot)`
    pub(crate) fn release_velocity(&mut self, slot: i32, lib: &LibraryData) -> i32 {
        if self.ample_release(lib)
            && (self.active_flags.val(slot) & (8192 | K_AMPLE_CHORD_KEY_FLAG)) == 0
            && self.active_notes.val(slot) >= self.current_target(lib).am_note_low
            && self.active_notes.val(slot) <= self.current_target(lib).am_note_high
        {
            // Ample (spec §10.1): Energy's weight, a lift into a chord change, a muted string's short release, +-6.
            let h = off_hash(self.active_off_ppq.val(slot), self.active_notes.val(slot));
            let mut v = 36 + 8 * self.energy + ((h >> 24) % 13u32) as i32 - 6;
            if (self.active_flags.val(slot) & K_AMPLE_CHANGE_FLAG) != 0 {
                v += 20;
            }
            if (self.active_flags.val(slot) & K_AMPLE_MUTED_FLAG) != 0 {
                v -= 12;
            }
            return clamp(v, 1, 127);
        }
        if !self.expressive() || (self.active_flags.val(slot) & 8192) != 0 || !self.continuous_pedal(lib) {
            return 0;
        }
        let h = off_hash(self.active_off_ppq.val(slot), self.active_notes.val(slot));
        (if self.energy <= 1 { 40 } else { 56 }) + ((h >> 24) % 17u32) as i32
    }

    /// `int pianoOffVel (float written, bool phraseEnd, bool ending, int note, float64 offPpq)`
    pub(crate) fn piano_off_vel(&mut self, written: f32, phrase_end: bool, ending: bool, note: i32, off_ppq: f64, lib: &LibraryData) -> i32 {
        let q = self.tick_ppq(lib) * 4.0f32 * 0.25f32;
        let beat = self.tick_ppq(lib) * 4.0f32;
        let mut lo = 36;
        let mut hi = 50;
        if ending {
            lo = 14;
            hi = 22;
        } else {
            if phrase_end {
                lo = 24;
                hi = 36;
            } else if written <= q + 1.0e-4f32 {
                lo = 84;
                hi = 100;
            } else if written < beat - 1.0e-4f32 {
                lo = 56;
                hi = 72;
            }
            if self.energy <= 1 {
                lo -= 10;
                hi -= 10;
            }
        }
        let h = off_hash(off_ppq, note);
        clamp((lo + hi) / 2 + ((h >> 24) % 13u32) as i32 - 6, lo, hi)
    }

    /// `void harmonicStep (float64 press, float64 release)`
    pub(crate) fn harmonic_step(&mut self, press: f64, release: f64, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let cc = self.current_target(cx.lib).harmonic_pedal_cc;
        let ms = self.ppq_per_ms();
        if self.harmonic_cc_sent == cc && self.harmonic_off_at >= 0.0 && press <= self.harmonic_off_at + 5.0 * ms {
            for i in 0..64 {
                let q = *self.q.queue.at(i);
                if q.used && q.kind == 1 && q.d1 == cc && q.d2 == 0 {
                    self.q.queue.at_mut(i).used = false;
                }
            }
        } else {
            if self.harmonic_cc_sent >= 0 && self.harmonic_cc_sent != cc {
                self.harmonic_lift(cx, out);
            }
            self.schedule(1, cc, 127, press, 0.0f32);
        }
        self.harmonic_cc_sent = cc;
        self.schedule(1, cc, 0, release, 0.0f32);
        self.harmonic_off_at = release;
    }

    /// `void harmonicLift ()`
    pub(crate) fn harmonic_lift(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if self.harmonic_cc_sent >= 0 {
            for i in 0..64 {
                let q = *self.q.queue.at(i);
                if q.used && q.kind == 1 && q.d1 == self.harmonic_cc_sent {
                    self.q.queue.at_mut(i).used = false;
                }
            }
        }
        if self.harmonic_held {
            self.send_midi(0xB0 + self.midi_channel0, self.harmonic_cc_sent, 0, cx, out);
        }
        self.harmonic_held = false;
        self.harmonic_off_at = -1.0;
    }

    /// `bool pianoLostFrom (float64 cut)`
    pub(crate) fn piano_lost_from(&mut self, cut: f64) -> bool {
        if self.harmonic_cc_sent < 0 {
            return false;
        }
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if q.used && q.kind == 1 && q.d1 == self.harmonic_cc_sent && q.d2 == 0 && q.on_ppq >= cut {
                return true;
            }
        }
        false
    }

    /// `void pianoDropFrom (float64 cut, bool lost)`
    pub(crate) fn piano_drop_from(&mut self, cut: f64, lost: bool) {
        if !lost {
            return;
        }
        self.schedule(1, self.harmonic_cc_sent, 0, cut, 0.0f32);
        self.harmonic_off_at = cut;
    }

    // ---- Half-bar chords (spec 2026-10-28 §2) --------------------------------------------------------------------
    // A persona's grid is read per bar and step: with chords every half bar, a chord that starts where the grid has
    // no chordal hit never sounds, and the hand keeps holding the chord before. The keys and the strummed players
    // then re-voice: the held tones are released at the step and the new chord struck at their velocity minus 10 (a
    // "push" re-strike), in the persona's own voicing. Natural feel and half-bar entries only: whole-bar songs, and
    // Classic (MIDI Studio's goldens, byte-identical), play exactly as before.

    /// Whether this step is the first of a half-bar chord that differs from the one before it. In a push's
    /// look-ahead (`dry_run`: the next chord's downbeat, struck an 8th early) the next entry is the one that counts;
    /// the re-voice then rides the push, since the downbeat itself is cleared after one.
    pub(crate) fn revoice_due(&self, ch: Chord, pv: Chord, step_ppq: f64, cx: &Ctx<'_>) -> bool {
        if !self.perf.natural || self.cur.is_none() || self.same_chord(ch, pv) {
            return false;
        }
        if self.dry_run {
            return (step_ppq - self.cur_end_ppq).abs() < 1.0e-6 && self.entry_rel(1, cx).len_half == 1;
        }
        self.cur.is_some_and(|c| c.len_half == 1) && step_ppq < self.cur_start_ppq + f64::from(self.tick_ppq(cx.lib)) - 1.0e-6
    }

    /// Whether the step buffer holds a note of `hand` (1 = the right hand's chord).
    pub(crate) fn step_has_hand(&self, hand: i32) -> bool {
        (0..self.step_count).any(|i| self.step_hand.val(i) == hand)
    }

    /// Releases, at `step_ppq`, every note of ours still sounding (or queued) past it that is a tone of `pv`: the
    /// loudest one's velocity (0 when nothing was held) and whether one lay below the right hand's range. A chord
    /// tone the hand let go within the half step before counts as held: it was carrying the chord to the change.
    pub(crate) fn release_held_chord(&mut self, who: i32, pv: Chord, step_ppq: f64, lib: &LibraryData) -> (f32, bool) {
        let rh_low = self.feel_for(who, lib).rh_low + self.octave_shift * 12;
        let since = step_ppq - 0.5 * f64::from(self.tick_ppq(lib));
        let bass = if pv.bass_pc >= 0 { self.positive_modulo(self.key_root + pv.bass_pc, 12) } else { -1 };
        let mut vel = 0;
        let mut lh = false;
        for i in 0..64 {
            if !self.active_slots.val(i) || self.active_channels.val(i) != self.midi_channel0 || (self.active_flags.val(i) & 8192) != 0 {
                continue;
            }
            let off = self.active_off_ppq.val(i);
            let n = self.active_notes.val(i);
            if off <= since || (!self.is_chord_tone(n, pv) && self.positive_modulo(n, 12) != bass) {
                continue;
            }
            vel = max(vel, self.active_vel.val(i));
            lh |= n < rh_low;
            if off > step_ppq {
                self.active_off_ppq.set(i, step_ppq);
            }
        }
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if !q.used || q.kind != 0 || q.ch != 0 || (q.flags & 8192) != 0 || q.on_ppq >= step_ppq - 1.0e-6 {
                continue;
            }
            let end = q.on_ppq + f64::from(q.dur_ppq);
            if end <= since || (!self.is_chord_tone(q.d1, pv) && self.positive_modulo(q.d1, 12) != bass) {
                continue;
            }
            vel = max(vel, q.d2);
            lh |= q.d1 < rh_low;
            if end > step_ppq {
                self.q.queue.at_mut(i).dur_ppq = max(0.01f32, (step_ppq - q.on_ppq) as f32);
            }
        }
        (vel as f32, lh)
    }

    /// Remembers this step's chordal strike (`hand` 1 = the right hand, -1 = any note): its loudest velocity and when.
    pub(crate) fn note_chord_strike(&mut self, hand: i32, step_ppq: f64) {
        if self.dry_run {
            return;
        }
        let mut vel = 0.0f32;
        for i in 0..self.step_count {
            if hand < 0 || self.step_hand.val(i) == hand {
                vel = max(vel, self.step_vel.val(i));
            }
        }
        if vel > 0.0f32 {
            self.strike_last_vel = vel;
            self.strike_last_ppq = step_ppq;
        }
    }

    /// The velocity a half-bar re-voice strikes at (0 = none due): the held chord's, released here; else, when the
    /// hand struck the chord before (within it) and let it go, that strike's. Also whether a held note lay in the left hand.
    pub(crate) fn revoice_vel(&mut self, who: i32, pv: Chord, step_ppq: f64, lib: &LibraryData) -> (f32, bool) {
        let (held, lh) = self.release_held_chord(who, pv, step_ppq, lib);
        if held > 0.0f32 {
            return (max(1.0f32, held - 10.0f32), lh);
        }
        // within the chord before this one (in a push's look-ahead that is the current chord)
        let from = if self.dry_run { self.cur_start_ppq } else { self.cur_start_ppq - self.chord_len_ppq() };
        let recent = self.strike_last_ppq >= from - 1.0e-6 && self.strike_last_ppq < step_ppq - 1.0e-6;
        if recent && self.strike_last_vel > 0.0f32 { (max(1.0f32, self.strike_last_vel - 10.0f32), false) } else { (0.0f32, false) }
    }

    /// The re-struck chord's length: to the end of the chord (a half bar in a push's look-ahead), a step at least.
    pub(crate) fn revoice_len(&self, step_ppq: f64, lib: &LibraryData) -> f32 {
        let tick = self.tick_ppq(lib);
        let to_end = if self.dry_run { self.len_ppq_for(0) } else { self.cur_end_ppq - step_ppq };
        max(tick, to_end as f32) * self.length
    }
}

//! `MusicianBass.cpp`: Musician: bass (shared lane helpers, Bass, MODO BASS 2 in Natural), ported line by line (Task 15).
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.

use super::*;
use crate::meter::{beat_of, is_last_beat};

impl Musician {
    /// `bool laneHit (int who, int lane, int gstep, int step, int pass)`
    pub(crate) fn lane_hit(&mut self, who: i32, lane: i32, gstep: i32, step: i32, pass: i32, cx: &mut Ctx<'_>) -> bool {
        let prob = self.grid_at(who, lane, gstep, 0, cx.lib);
        prob > 0 && self.rand01(step, pass, 611 + lane * 7, who, cx) * 100.0f32 < prob as f32
    }

    /// `bool isChordTone (int pitch, Chord c)`
    pub(crate) fn is_chord_tone(&mut self, pitch: i32, c: Chord) -> bool {
        let rel = self.positive_modulo(pitch - self.key_root - c.root_pc, 12);
        for i in 0..c.count {
            if self.positive_modulo(c.tones.val(i), 12) == rel {
                return true;
            }
        }
        false
    }

    /// `int approachTo (int target, int step, int pass, int who)`
    pub(crate) fn approach_to(&mut self, target: i32, step: i32, pass: i32, who: i32, cx: &mut Ctx<'_>) -> i32 {
        let below = self.rand01(step, pass, 627, who, cx) < 0.65f32;
        if self.respect {
            return if below { self.step_down(target, 0) } else { self.step_up(target, 0) };
        }
        if below { target - 1 } else { target + 1 }
    }

    /// `int nearestOctave (int note, int reference)`
    pub(crate) fn nearest_octave(&mut self, note: i32, reference: i32) -> i32 {
        let mut n = note;
        while n - reference > 6 {
            n -= 12;
        }
        while reference - n > 6 {
            n += 12;
        }
        n
    }

    /// `int bassPitch (int who, int type, Chord ch, Chord nx, bool changeNext, int sib, int step, int pass)`
    pub(crate) fn bass_pitch(
        &mut self,
        who: i32,
        type_: i32,
        ch: Chord,
        nx: Chord,
        change_next: bool,
        sib: i32,
        step: i32,
        pass: i32,
        cx: &mut Ctx<'_>,
    ) -> i32 {
        let lib = cx.lib;
        let r = self.lh_root(who, ch, lib);
        let bs = self.lh_bass(who, ch, lib); // the slash bass, or r
        let nx_bass = self.lh_bass(who, nx, lib);
        let nr = self.nearest_octave(nx_bass, r); // walks aim at the next chord's bass note
        let third = ch.tones.val(1);
        let fifth = ch.tones.val(2);
        let target = if change_next { nr } else { bs };

        if type_ == 2 {
            return r + fifth;
        }
        if type_ == 3 {
            return bs + 12;
        }
        if type_ == 14 {
            return r + third;
        }
        if type_ == 15 {
            return r + (if ch.count >= 4 && ch.tones.val(3) < 12 { ch.tones.val(3) } else { ch.seventh });
        }
        if type_ == 19 {
            return r + 9;
        }
        if type_ == 12 {
            return self.approach_to(target, step, pass, who, cx);
        }
        if type_ == 22 {
            return if target > r { self.step_down(target, 0) } else { self.step_up(target, 0) };
        }
        if type_ == 23 || type_ == 24 {
            // Diatonic walk from the root toward the next root (or up toward the octave).
            let goal = if change_next { nr } else { bs + 12 };
            let mut p = bs;
            for _k in 0..(if type_ == 23 { 1 } else { 2 }) {
                let next = if goal > p { self.step_up(p, 0) } else { self.step_down(p, 0) };
                if (goal > p && next >= goal) || (goal < p && next <= goal) {
                    break;
                }
                p = next;
            }
            return p;
        }
        if type_ == 13 {
            // Walking: root, colour, fifth-ish, then lean into the next root (on the meter's last beat).
            let beat = beat_of(self.meter, sib);
            if beat == 0 {
                return bs;
            }
            if is_last_beat(self.meter, sib) {
                return self.approach_to(target, step, pass, who, cx);
            }
            if beat == 1 {
                return if self.rand01(step, pass, 621, who, cx) < 0.5f32 { r + third } else { self.step_up(r, 0) };
            }
            if beat == 2 {
                return if self.rand01(step, pass, 623, who, cx) < 0.6f32 { r + fifth } else { r + third + (if third == 3 { 2 } else { 1 }) };
            }
            return self.approach_to(target, step, pass, who, cx);
        }
        bs
    }

    // 16-only: the fill vocabulary is 4/4's beat 4 (steps 12-15); 12-step meters pass the step end-aligned (sib + 16 - spb()).
    /// `void bassFill (int who, int variant, int sib, Chord ch, Chord nx, bool changeNext, float tick, int step, int pass)`
    pub(crate) fn bass_fill(
        &mut self,
        who: i32,
        variant: i32,
        sib: i32,
        ch: Chord,
        nx: Chord,
        change_next: bool,
        tick: f32,
        step: i32,
        pass: i32,
        cx: &mut Ctx<'_>,
    ) {
        let lib = cx.lib;
        let r = self.lh_root(who, ch, lib);
        let target = if change_next {
            let nb = self.lh_bass(who, nx, lib);
            self.nearest_octave(nb, r)
        } else {
            r + 12
        };
        let len = tick * 0.9f32;

        if self.feel_for(who, lib).heavy == 1 {
            let mute = tick * 0.5f32;
            if variant == 0 {
                // Tremolo-picked root, digging in harder each 16th.
                let n = self.bass_range(r);
                self.add_step_note(n, 96.0f32 + (sib - 12) as f32 * 6.0f32, mute, 0, 0.0f32);
            } else if variant == 1 {
                // Chromatic climb into the next root.
                let mut p = target;
                for _k in 0..16 - sib {
                    p = if self.respect { self.step_down(p, 0) } else { p - 1 };
                }
                let n = self.bass_range(p);
                self.add_step_note(n, 100.0f32 + (sib - 12) as f32 * 4.0f32, len, 0, 0.0f32);
            } else if variant == 2 {
                // Octave pedal: root and octave in 16ths.
                let n = self.bass_range(if sib % 2 == 0 { r } else { r + 12 });
                self.add_step_note(n, if sib % 2 == 0 { 108.0f32 } else { 96.0f32 }, mute, 0, 0.0f32);
            } else {
                // Gallop into the change: chug, rest, chug-chug.
                if sib != 13 {
                    let n = self.bass_range(if sib == 15 { target } else { r });
                    self.add_step_note(n, if sib == 12 { 112.0f32 } else { 100.0f32 }, mute, 0, 0.0f32);
                }
            }
            return;
        }

        if variant == 0 {
            // Chromatic (or diatonic, with Respect) climb into the next root.
            if sib == 12 {
                let n = self.bass_range(r);
                self.add_step_note(n, 92.0f32, len, 0, 0.0f32);
            } else {
                let mut p = target;
                for _k in 0..16 - sib {
                    p = if self.respect { self.step_down(p, 0) } else { p - 1 };
                }
                let n = self.bass_range(p);
                self.add_step_note(n, 80.0f32 + (sib - 12) as f32 * 5.0f32, len, 0, 0.0f32);
            }
        } else if variant == 1 {
            // Octave pops.
            let n = if sib == 12 {
                r
            } else if sib == 13 {
                r + 12
            } else if sib == 14 {
                r + 7
            } else {
                r + 12
            };
            let n = self.bass_range(n);
            self.add_step_note(n, if sib % 2 == 0 { 92.0f32 } else { 80.0f32 }, tick * 0.6f32, 0, 0.0f32);
        } else if variant == 2 {
            // Stepwise run down from the octave, landing next to the target.
            let mut p = r + 12;
            for _k in 0..sib - 12 {
                p = self.step_down(p, 0);
            }
            if sib == 15 {
                p = self.approach_to(target, step, pass, who, cx);
            }
            let n = self.bass_range(p);
            self.add_step_note(n, 88.0f32 - (sib - 12) as f32 * 3.0f32, len, 0, 0.0f32);
        } else {
            // Fifth, ghost, fifth below, approach.
            if sib == 12 {
                let n = self.bass_range(r + 7);
                self.add_step_note(n, 90.0f32, len, 0, 0.0f32);
            } else if sib == 13 {
                let n = self.bass_range(r);
                self.add_step_note(n, 40.0f32, tick * 0.4f32, 0, 0.0f32);
            } else if sib == 14 {
                let n = self.bass_range(if r - 5 >= 24 { r - 5 } else { r + 7 });
                self.add_step_note(n, 84.0f32, len, 0, 0.0f32);
            } else {
                let a = self.approach_to(target, step, pass, who, cx);
                let n = self.bass_range(a);
                self.add_step_note(n, 86.0f32, len, 0, 0.0f32);
            }
        }
    }

    // 12-step meters (Task 15): eighths on the even steps of the zone walking up into the next root (sib 0 of the next bar).
    /// `void bassFill12 (int who, int variant, int sib, Chord ch, Chord nx, bool changeNext, float tick)`
    pub(crate) fn bass_fill12(&mut self, who: i32, variant: i32, sib: i32, ch: Chord, nx: Chord, change_next: bool, tick: f32, lib: &LibraryData) {
        if sib % 2 == 1 {
            return;
        }
        let r = self.lh_root(who, ch, lib);
        let target = if change_next {
            let nb = self.lh_bass(who, nx, lib);
            self.nearest_octave(nb, r)
        } else {
            r + 12
        };
        let left = (self.spb() - sib) / 2; // eighths to the bar line (1 = the last)
        let len = tick * 1.8f32 * (if self.feel_for(who, lib).heavy == 1 { 0.6f32 } else { 0.9f32 });
        let mut p = target;
        if variant == 0 || left == 1 {
            for _k in 0..left {
                p = if self.respect { self.step_down(p, 0) } else { p - 1 }; // chromatic (diatonic with Respect) climb
            }
        } else {
            p = if left == 2 { r + 7 } else { r }; // root / fifth, then the approach
        }
        let n = self.bass_range(p);
        self.add_step_note(n, 84.0f32 + (3 - left) as f32 * 5.0f32, len, 0, 0.0f32);
    }

    /// `srcAt` (a lambda of `bassStep`): the grid index the bass reads `t` steps after this bar's start (t may run into
    /// the next bar), -1 = nothing there. `gridStepIn`'s `has` reads `gridAt` (pure), here taken for the 16 steps of the
    /// source bar first.
    fn bass_src_at(&mut self, who: i32, prog_bar: i32, t: i32, lib: &LibraryData) -> i32 {
        let s = t % self.spb();
        let b = self.positive_modulo(prog_bar + t / self.spb(), 2);
        if self.meter == Meter::M34 && self.energy <= 2 && s != 0 {
            return -1;
        }
        // `grid_step_in` reads `has` for the Bass role only in 3/4 at step 10 (the beat-4 approach), so the table is
        // filled only then; elsewhere every entry stays false and is never read.
        let mut hs = [false; 16];
        if self.meter == Meter::M34 && s == 10 {
            for (k, h) in (0i32..).zip(hs.iter_mut()) {
                *h = self.grid_at(who, 0, b * 16 + k, 0, lib) > 0 || self.grid_at(who, 1, b * 16 + k, 0, lib) > 0;
            }
        }
        let g = self.grid_step_in(who, s, GridRole::Bass, |k| usize::try_from(k).ok().and_then(|k| hs.get(k)).copied().unwrap_or(false), lib);
        if g < 0 { -1 } else { b * 16 + g }
    }

    /// `gridHas` (a lambda of `bassStep`).
    fn bass_grid_has(&mut self, who: i32, prog_bar: i32, lane: i32, t: i32, lib: &LibraryData) -> bool {
        let g = self.bass_src_at(who, prog_bar, t, lib);
        g >= 0 && self.grid_at(who, lane, g, 0, lib) != 0
    }

    /// `void bassStep (int who, int step, int pass, int progBar, int sib, float64 stepPpq, Chord ch, Chord nx, bool changeNext)`
    pub(crate) fn bass_step(
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
        // The lanes are 2 bars of 16 steps; 12-step meters read them through gridStep (bass). srcAt: the grid index the bass
        // reads `t` steps after this bar's start (t may run into the next bar), -1 = nothing there. 3/4 at Energy 1-2 is
        // oom-pah-pah: the bass only on the downbeat (spec 6).
        let src = self.bass_src_at(who, prog_bar, sib, lib);
        let gstep = if src >= 0 { src } else { self.positive_modulo(prog_bar, 2) * 16 };
        self.clear_step();
        self.bass_next_at = 1.0e18;

        // 12-step meters (Task 15): a walk-up by eighths into the change (6/8 from 9, from 6 at Energy 4; 3/4 beat 3), its
        // variant drawn with salt 1997 (only here, so the 4/4 stream is untouched).
        let bzone = self.fill_zone(); // the drums' zone (6/8: 9, from 6 at Energy 4)
        // Hemiola at a phrase end (Task 17, hemiolaBar): the bar regrouped, 3/4 as two dotted quarters (root on 0, fifth on 6),
        // 6/8 as three quarters (root, fifth, octave on 0, 4, 8), each held into the next; it takes the bar from the walk-up.
        let hemiola = self.hemiola_bar(who, pass, prog_bar, cx);
        let hg = if self.meter == Meter::M34 { 6 } else { 4 };
        if hemiola {
            if sib % hg == 0 {
                let k = sib / hg;
                let r = self.lh_root(who, ch, lib);
                let n = self.bass_range(if k == 1 {
                    r + 7
                } else if k == 2 {
                    r + 12
                } else {
                    r
                });
                self.add_step_note(n, if k == 0 { 100.0f32 } else { 90.0f32 }, hg as f32 * tick * 0.9f32 * max(0.5f32, self.length), 0, 0.0f32);
                HEMIOLA_NOTES.fetch_add(1, Ordering::Relaxed);
                if self.meter == Meter::M44 || !self.is_phrase_end(prog_bar) {
                    HEMIOLA_OFF.fetch_add(1, Ordering::Relaxed);
                }
            }
        } else if is12(self.meter) && sib >= bzone && self.fill_active(who, pass, prog_bar, change_next, true, cx, out) {
            self.step_extra_flags = if self.expressive() && self.bass_target(lib) { K_FILL_FLAG } else { 0 };
            let v = min(1, (self.rand01(pass, prog_bar, 1997, who, cx) * 2.0f32) as i32);
            self.bass_fill12(who, v, sib, ch, nx, change_next, tick, lib);
            self.step_extra_flags = 0;
        } else if !is12(self.meter) && sib >= self.fill_zone() && self.fill_active(who, pass, prog_bar, change_next, true, cx, out) {
            self.step_extra_flags = if self.expressive() && self.bass_target(lib) { K_FILL_FLAG } else { 0 };
            let v = self.fill_variant(who, pass, prog_bar, cx);
            self.bass_fill(who, v, sib + 16 - self.spb(), ch, nx, change_next, tick, step, pass, cx);
            self.step_extra_flags = 0;
        } else {
            let mut lane = 0;
            while lane < 2 && src >= 0 {
                if !self.lane_hit(who, lane, gstep, step, pass, cx) {
                    lane += 1;
                    continue;
                }
                let type_ = self.lane_type(who, lane, gstep, lib);
                let mut len = self.lane_len(who, lane, gstep, tick, lib);

                // Sustain up to the next hit in this lane (bass players hold notes between attacks).
                let mut sustain_to = 1;
                while sustain_to < self.spb() && !self.bass_grid_has(who, prog_bar, lane, sib + sustain_to, lib) {
                    sustain_to += 1;
                }
                if type_ != 16 && type_ != 20 {
                    len = max(len, min(sustain_to as f32 * tick - 0.02f32, tick * 8.0f32) * self.length);
                }

                if type_ == 16 {
                    len = min(len, tick * 0.4f32);
                }
                if type_ == 20 {
                    len = min(len, tick * 0.55f32);
                }
                let mut flags = if type_ == 16 {
                    1
                } else if type_ == 20 {
                    2
                } else {
                    0
                };
                if self.feel_for(who, lib).heavy == 1 {
                    flags |= 8;
                }
                if who == 6 && self.energy >= 3 {
                    flags |= 16; // Dex (persona 6) slaps at high energy
                }
                // Natural on MODO BASS: a note followed by a rest (a 16th note or more of silence before the next, at any step speed) gets stop-on-detach.
                if self.expressive() && self.bass_target(lib) && type_ != 16 && sustain_to as f32 * tick - len >= 0.24f32 {
                    flags |= K_STOP_FLAG;
                }
                if self.expressive() && self.bass_target(lib) && type_ != 16 {
                    // Spec 11.3: a staccato note (a quarter of its step or less) stops on detach; the last note before a
                    // rest of a beat or more, a Break or the section's end may fall (naturalBassChoices draws it).
                    if len <= 0.25f32 * sustain_to as f32 * tick {
                        flags |= K_STOP_FLAG;
                    }
                    // The next note in either lane (the bass is mono: a ghost in the other lane closes this one).
                    let mut next_hit = 1;
                    while next_hit < self.spb()
                        && !self.bass_grid_has(who, prog_bar, 0, sib + next_hit, lib)
                        && !self.bass_grid_has(who, prog_bar, 1, sib + next_hit, lib)
                    {
                        next_hit += 1;
                    }
                    let next_at = step_ppq + f64::from(next_hit) * f64::from(tick);
                    // A fall also ends before where a fill could start (the fill zone).
                    if self.step_count == 0 {
                        self.bass_next_at = if sib < self.fill_zone() {
                            min(next_at, step_ppq + f64::from(self.fill_zone() - sib) * f64::from(tick))
                        } else {
                            next_at
                        };
                    }
                    let brk = self.perf.break_at >= 0.0 && step_ppq < self.perf.break_at - 1.0e-6 && next_at >= self.perf.break_at - 1.0e-6;
                    let sec_end = self.cur.is_some_and(|c| c.chord_index == c.section_chords - 1) && next_at >= self.cur_end_ppq - 1.0e-6;
                    if (next_at - step_ppq) as f32 - len >= self.beat_ppq() as f32 || brk || sec_end {
                        flags |= K_FALL_FLAG;
                    }
                }
                // 6/8: the source's beats 2 and 4 are the "and-a" lead-ins into each dotted beat, lighter.
                let vel = self.lane_vel(who, lane, gstep, lib) * (if self.meter == Meter::M68 && (sib == 4 || sib == 10) { 0.85f32 } else { 1.0f32 });
                let p = self.bass_pitch(who, type_, ch, nx, change_next, sib, step, pass, cx);
                let n = self.bass_range(p);
                self.add_step_note_f(n, vel, len, 0, 0.0f32, flags);
                lane += 1;
            }
        }
        if self.ten_tail && self.step_count > 0 && self.rand01(pass, gstep, 2003, who, cx) < max(0.6f32, self.ten_now) {
            let target = self.positive_modulo(self.key_root + nx.root_pc, 12);
            let n = self.step_note.val(0);
            let mut best = n;
            for d in -6..=6 {
                if self.positive_modulo(n + d, 12) == target {
                    best = n + d;
                    break;
                }
            }
            self.step_note.set(0, best - 1); // tension: walk into the release
        }
        if self.expressive() && self.bass_target(lib) {
            self.natural_bass_section(who, pass, prog_bar, cx);
            self.natural_bass_choices(who, pass, gstep, sib, step_ppq, ch, cx);
        }
        self.perform_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
    }

    /// `void naturalBassSection (int who, int pass, int progBar)`
    pub(crate) fn natural_bass_section(&mut self, who: i32, pass: i32, prog_bar: i32, cx: &mut Ctx<'_>) {
        let Some(cur) = self.cur else { return };
        if cur.chord_index != 0 || prog_bar != self.cur_prog_bar {
            return;
        }
        let first = self.cur_index - cur.chord_index;
        if first == self.pos_section_at && pass == self.pos_section_pass {
            return;
        }
        self.pos_section_at = first;
        self.pos_section_pass = pass;
        let lib = cx.lib;
        let kb = self.current_target(lib);
        let bs = &self.feel_for(who, lib).bass;
        self.pos_key_pending = -1;
        self.pluck_pending = -1;
        self.pos_cc_pending = -1;
        if kb.kb_pos_lo >= 0 || kb.kb_pos_cc >= 0 {
            let mut f0 = 99;
            for i in 0..max(1, cur.section_chords) {
                let c = self.chord_for_bar(prog_bar + i, cx);
                f0 = min(f0, self.positive_modulo(self.key_root + (if c.bass_pc >= 0 { c.bass_pc } else { c.root_pc }) - 4, 12));
            }
            let lo = if bs.position == 0 {
                0
            } else if bs.position == 1 {
                5
            } else {
                9
            };
            let hi = if bs.position == 0 {
                5
            } else if bs.position == 1 {
                9
            } else {
                14
            };
            let dist = |f: i32| {
                if f < lo {
                    lo - f
                } else if f > hi {
                    f - hi
                } else {
                    0
                }
            };
            let f = clamp(if dist(f0 + 12) < dist(f0) { f0 + 12 } else { f0 }, lo, hi);
            if kb.kb_pos_lo >= 0 {
                self.pos_key_pending = min(kb.kb_pos_lo + f, kb.kb_pos_hi);
            }
            if kb.kb_pos_cc >= 0 {
                self.pos_cc_pending = clamp((f as f32 * 127.0f32 / 24.0f32).round() as i32, 0, 127); // frets 0-24 over 0-127
            }
        }
        if kb.kb_pluck_cc >= 0 {
            self.pluck_pending = clamp(
                (if bs.pluck == 0 {
                    20
                } else if bs.pluck == 1 {
                    64
                } else {
                    100
                }) + (if self.energy >= 4 {
                    12
                } else if self.energy <= 1 {
                    -12
                } else {
                    0
                }),
                0,
                127,
            );
        }
    }

    /// `void bassExpressionLift ()`
    pub(crate) fn bass_expression_lift(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if self.bass_vib_on && self.bass_vib_cc_sent >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.bass_vib_cc_sent, 0, cx, out);
        }
        if self.bass_ring_on && self.bass_ring_cc_sent >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.bass_ring_cc_sent, 0, cx, out);
        }
        if self.wheel_bent {
            self.send_midi(0xE0 + self.midi_channel0, 0, 64, cx, out);
        }
        self.bass_vib_on = false;
        self.bass_ring_on = false;
        self.wheel_bent = false;
        self.ring_bar = NO_STEP;
        self.ring_active_bar = NO_STEP;
        self.pos_section_at = -1;
        self.pos_section_pass = -1;
        self.pos_key_pending = -1;
        self.pluck_pending = -1;
        self.pos_cc_pending = -1;
    }

    /// `int bassLostFrom (float64 cut)`
    pub(crate) fn bass_lost_from(&mut self, cut: f64) -> i32 {
        let mut lost = 0;
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if !q.used || q.on_ppq < cut {
                continue;
            }
            if q.kind == 4 {
                lost |= 1;
            }
            if q.kind == 1 && self.bass_vib_cc_sent >= 0 && q.d1 == self.bass_vib_cc_sent {
                lost |= 2;
            }
            if q.kind == 1 && self.bass_ring_cc_sent >= 0 && q.d1 == self.bass_ring_cc_sent {
                lost |= 4;
            }
        }
        lost
    }

    /// `void bassDropFrom (float64 cut, int lost)`
    pub(crate) fn bass_drop_from(&mut self, cut: f64, lost: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if self.wheel_bent || (lost & 1) != 0 {
            self.schedule(4, 0, 64, cut, 0.0f32);
        }
        if (lost & 2) != 0 {
            self.schedule(1, self.bass_vib_cc_sent, 0, cut, 0.0f32);
        }
        // The ring lifts now (not queued: a regenerated bar-first note would read a queued 0 as a ring going on).
        if (lost & 4) != 0 && self.bass_ring_on {
            self.send_midi(0xB0 + self.midi_channel0, self.bass_ring_cc_sent, 0, cx, out);
            self.bass_ring_on = false;
        }
        self.ring_bar = NO_STEP;
        self.ring_active_bar = NO_STEP;
        if self.pos_sent_at >= cut {
            self.pos_section_at = -1;
            self.pos_section_pass = -1;
            self.pos_sent_at = -1.0e9;
        }
    }

    /// `void dropQueuedWheel ()`
    pub(crate) fn drop_queued_wheel(&mut self) {
        for i in 0..64 {
            if self.q.queue.at(i).used && self.q.queue.at(i).kind == 4 {
                self.q.queue.at_mut(i).used = false;
            }
        }
    }

    /// `void naturalBassChoices (int who, int pass, int gstep, int sib, float64 stepPpq, Chord ch)`
    pub(crate) fn natural_bass_choices(&mut self, who: i32, pass: i32, gstep: i32, sib: i32, step_ppq: f64, ch: Chord, cx: &mut Ctx<'_>) {
        if self.step_count == 0 {
            return;
        }
        let lib = cx.lib;
        let kb = self.current_target(lib);
        // `int& fl = stepFlags.at (0)`: read here, written back at the end.
        let mut fl = self.step_flags.val(0);
        let pitch = self.step_note.val(0);
        let iv = if self.last_bass_pitch >= 0 { (pitch - self.last_bass_pitch).abs() } else { 0 };
        let new_chord = self.last_bass_root >= 0 && ch.root_pc != self.last_bass_root;
        let close = step_ppq - self.last_bass_step_ppq <= 0.5 + 1.0e-6;
        // Articulation grows with Energy and Fills (Fills 0: only what a fill itself marks). Legato slides on a chord change
        // a step of 1-5 away, and between notes of one chord 2-5 apart within a beat; hammer-ons in fills, and on close
        // steps of 1-2 inside a chord.
        let fa = self.fills / 50.0f32;
        let within_beat = step_ppq - self.last_bass_step_ppq <= self.beat_ppq() + 1.0e-6;
        if kb.kb_legato_cc >= 0 && (1..=5).contains(&iv) && (fl & 1) == 0 {
            let chance = if new_chord {
                min(0.7f32, 0.3f32 * fa * (0.55f32 + 0.2f32 * self.energy as f32))
            } else if within_beat && iv >= 2 {
                min(0.4f32, 0.1f32 * fa * self.energy as f32)
            } else {
                0.0f32
            };
            if chance > 0.0f32 && self.rand01(pass, gstep, 1701, who, cx) < chance {
                fl |= K_SLIDE_FLAG;
            }
        }
        if kb.kb_hammer >= 0 && (fl & K_SLIDE_FLAG) == 0 && close && (1..=2).contains(&iv) && (fl & 1) == 0 {
            #[allow(clippy::if_same_then_else)] // the C++ shape: a fill always hammers, else a draw
            if (fl & K_FILL_FLAG) != 0 {
                fl |= K_HAMMER_FLAG;
            } else if !new_chord && self.rand01(pass, gstep, 1703, who, cx) < min(0.6f32, 0.08f32 * fa * (self.energy + 1) as f32) {
                fl |= K_HAMMER_FLAG;
            }
        }
        // Ghost notes in busy grooves (Energy 3-4): a short off-beat note inside a chord, played dead.
        if kb.kb_ghost >= 0
            && self.energy >= 3
            && !new_chord
            && (fl & (K_SLIDE_FLAG | K_HAMMER_FLAG | K_FILL_FLAG | 1)) == 0
            && self.step_len.val(0) <= 0.5f32 * self.beat_ppq() as f32 + 1.0e-4f32
        {
            let b = step_ppq / self.beat_ppq();
            if (b - b.round()).abs() > 0.05 && self.rand01(pass, gstep, 1705, who, cx) < min(0.5f32, 0.15f32 * fa * (self.energy - 2) as f32) {
                fl |= 1;
            }
        }
        let f = self.feel_for(who, lib);
        if kb.kb_down >= 0 && f.voicing == 0 && f.heavy == 1 && self.energy >= 3 && f.swing_base <= 52.0f32 && sib % 2 == 0 {
            fl |= K_DOWN_FLAG;
        }
        if kb.kb_stop >= 0
            && ((self.perf.break_at >= 0.0 && (step_ppq - self.perf.break_at).abs() < 0.01)
                || (self.perf.end_at >= 0.0 && (step_ppq - self.perf.end_at).abs() < 0.01))
        {
            fl |= K_STOP_FLAG;
        }
        if kb.kb_stop < 0 {
            fl &= !K_STOP_FLAG;
        }
        // Spec 11.3: falls, vibrato on long notes, let ring at low Energy (salts 1931-1934).
        let bs = &f.bass;
        if (fl & K_FALL_FLAG) != 0 && !((fl & (K_SLIDE_FLAG | 1)) == 0 && self.energy != 1 && self.rand01(pass, gstep, 1931, who, cx) < bs.falls) {
            fl &= !K_FALL_FLAG;
        }
        let len_beats = self.step_len.val(0) / self.beat_ppq() as f32;
        if kb.kb_vibrato_cc >= 0
            && len_beats >= 1.0f32
            && (fl & (1 | 16)) == 0
            && self.rand01(pass, gstep, 1932, who, cx) < bs.vibrato * (if self.energy <= 2 { 1.5f32 } else { 1.0f32 })
        {
            fl |= K_VIB_FLAG;
            self.bass_vib_depth = clamp(
                30 + min(30.0f32, 10.0f32 * (len_beats - 1.0f32)).round() as i32
                    + ((self.rand01(pass, gstep, 1934, who, cx) * 2.0f32 - 1.0f32) * 6.0f32).round() as i32,
                30,
                60,
            );
        }
        let bar = self.floor_to_int((step_ppq - self.bar_origin + 1.0e-6) / f64::from(self.get_bar_length_ppq()));
        if bar != self.ring_bar {
            self.ring_bar = bar;
            self.ring_active_bar = NO_STEP;
            if kb.kb_let_ring_cc >= 0 && self.energy <= 2 && (fl & K_FILL_FLAG) == 0 && self.rand01(pass, gstep, 1933, who, cx) < bs.let_ring {
                fl |= K_RING_FLAG;
                self.ring_active_bar = bar;
            }
        } else if self.ring_active_bar == bar {
            if (fl & K_FILL_FLAG) == 0 {
                fl |= K_RING_MORE_FLAG;
            } else {
                self.ring_active_bar = NO_STEP; // never in fills: the ring ends with the note before
            }
        }
        self.step_flags.set(0, fl);
        self.last_bass_pitch = pitch;
        self.last_bass_root = ch.root_pc;
        self.last_bass_step_ppq = step_ppq;
    }

    /// `void naturalBassNote (int& flags, float64 onPpq, float& len, int pitch)`
    pub(crate) fn natural_bass_note(&mut self, flags: &mut i32, on_ppq: f64, len: &mut f32, pitch: i32, lib: &LibraryData) {
        let kb = self.current_target(lib);
        let ms = self.ppq_per_ms();
        // `holdKey` (a lambda): kit::holdKey (kitCtx(), ks (key), 100, onPpq - 6.0 * ms, len + float (12.0 * ms)).
        let hold_key = |m: &mut Self, key: i32, len: f32| {
            let k = m.ks(key);
            let now = m.current_ppq;
            let mut s = SchedAt { q: &mut m.q, now };
            let mut c = kit::DriverCtx { sink: &mut s, now, ms };
            kit::hold_key(&mut c, k, 100, on_ppq - 6.0 * ms, len + (12.0 * ms) as f32, kit::K_BELOW_RANGE);
        };
        if (*flags & K_HAMMER_FLAG) != 0 {
            hold_key(self, kb.kb_hammer, *len);
        }
        if (*flags & K_STOP_FLAG) != 0 && (*flags & K_HAMMER_FLAG) == 0 {
            hold_key(self, kb.kb_stop, *len);
        }
        if (*flags & K_DOWN_FLAG) != 0 && (*flags & K_HAMMER_FLAG) == 0 {
            hold_key(self, kb.kb_down, *len); // with a stop too: down at the attack, stop at the off
        }
        if self.pos_key_pending >= 0 || self.pluck_pending >= 0 || self.pos_cc_pending >= 0 {
            // The section's position: one key (a 5 ms tap) or CC, and the pluck position, 20 ms before its first note.
            if self.pluck_pending >= 0 {
                self.schedule(1, kb.kb_pluck_cc, self.pluck_pending, on_ppq - 20.0 * ms, 0.0f32);
            }
            if self.pos_cc_pending >= 0 {
                self.schedule(1, kb.kb_pos_cc, self.pos_cc_pending, on_ppq - 20.0 * ms, 0.0f32);
            }
            if self.pos_key_pending >= 0 {
                let k = self.ks(self.pos_key_pending);
                let now = self.current_ppq;
                let mut s = SchedAt { q: &mut self.q, now };
                let mut c = kit::DriverCtx { sink: &mut s, now, ms };
                kit::tap_key(&mut c, k, 100, on_ppq - 20.0 * ms, 5.0);
            }
            self.pos_key_pending = -1;
            self.pluck_pending = -1;
            self.pos_cc_pending = -1;
            self.pos_sent_at = on_ppq - 20.0 * ms;
        }
        // A new note closes the one before (the bass is mono): its vibrato, and the bar's ring unless this note carries
        // it on, end here.
        for i in 0..64 {
            let q = *self.q.queue.at(i);
            if !q.used || q.kind != 1 || q.on_ppq <= on_ppq {
                continue;
            }
            let vib = self.bass_vib_cc_sent >= 0 && q.d1 == self.bass_vib_cc_sent;
            let ring = self.bass_ring_cc_sent >= 0 && q.d1 == self.bass_ring_cc_sent && (*flags & (K_RING_FLAG | K_RING_MORE_FLAG)) == 0;
            if vib || ring {
                if q.d2 == 0 {
                    self.q.queue.at_mut(i).on_ppq = on_ppq;
                    self.q.next_due_ppq = min(self.q.next_due_ppq, on_ppq);
                } else {
                    self.q.queue.at_mut(i).used = false;
                }
            }
        }
        let mut off = on_ppq + f64::from(*len);
        if (*flags & K_FALL_FLAG) != 0 {
            // A fall ends before the next note (45 ms early: that note may come early), so the wheel is
            // back at centre before it sounds.
            let cut = self.bass_next_at - 45.0 * ms;
            if cut < off && cut > on_ppq + 0.05 {
                off = cut;
                *len = (off - on_ppq) as f32;
            }
        }
        if (*flags & K_VIB_FLAG) != 0 && kb.kb_vibrato_cc >= 0 {
            // CC1 rises from 250 ms after the note-on to its depth in 4 steps (by 600 ms), back to 0 at the note-off.
            self.bass_vib_cc_sent = kb.kb_vibrato_cc;
            for k in 1..=4 {
                let at = on_ppq + (250.0 + 350.0 * f64::from(k - 1) / 3.0) * ms;
                if at < off {
                    self.schedule(1, kb.kb_vibrato_cc, self.bass_vib_depth * k / 4, at, 0.0f32);
                }
            }
            self.schedule(1, kb.kb_vibrato_cc, 0, off, 0.0f32);
        }
        if (*flags & K_FALL_FLAG) != 0 {
            // Pitch wheel from centre to the floor (-bendRange) over the note's last 120 ms; centre 10 ms after the off.
            let start = max(on_ppq, off - 120.0 * ms);
            for k in 0..5 {
                let v = 8192 - 8192 * (k + 1) / 5;
                self.schedule(4, v % 128, v / 128, start + (off - start) * f64::from(k) / 5.0, 0.0f32);
            }
            self.schedule(4, 0, 64, off + 10.0 * ms, 0.0f32);
        }
        if kb.kb_let_ring_cc >= 0 && (*flags & (K_RING_FLAG | K_RING_MORE_FLAG)) != 0 {
            // Let ring: 127 just before the bar's first note, 0 at the bar's last note-off (moved on by each note).
            if (*flags & K_RING_MORE_FLAG) != 0 {
                for i in 0..64 {
                    let q = *self.q.queue.at(i);
                    if q.used && q.kind == 1 && q.d1 == kb.kb_let_ring_cc && q.d2 == 0 {
                        self.q.queue.at_mut(i).on_ppq = max(q.on_ppq, off);
                    }
                }
            }
            if (*flags & K_RING_FLAG) != 0 {
                // The last bar's ring still waiting to lift: it goes on into this bar instead.
                let mut ringing = false;
                for i in 0..64 {
                    let q = *self.q.queue.at(i);
                    if q.used && q.kind == 1 && q.d1 == kb.kb_let_ring_cc && q.d2 == 0 {
                        self.q.queue.at_mut(i).used = false;
                        ringing = true;
                    }
                }
                self.bass_ring_cc_sent = kb.kb_let_ring_cc;
                if !ringing && !self.bass_ring_on {
                    self.schedule(1, kb.kb_let_ring_cc, 127, on_ppq - 5.0 * ms, 0.0f32);
                }
                self.schedule(1, kb.kb_let_ring_cc, 0, off, 0.0f32);
            }
        }
        if (*flags & K_SLIDE_FLAG) != 0 {
            // Only from a note 1-5 semitones away as played (a note folded up an octave above minNote isn't one): else no slide.
            let mut near = false;
            for i in 0..64 {
                if self.active_slots.val(i)
                    && self.active_channels.val(i) == self.midi_channel0
                    && (self.active_flags.val(i) & 8192) == 0
                    && self.active_off_ppq.val(i) > self.current_ppq
                {
                    let d = (self.active_notes.val(i) - pitch).abs();
                    near = near || (1..=5).contains(&d);
                }
            }
            if !near {
                *flags &= !K_SLIDE_FLAG;
            }
        }
        if (*flags & K_SLIDE_FLAG) != 0 {
            let mut source = false;
            for i in 0..64 {
                if self.active_slots.val(i)
                    && self.active_channels.val(i) == self.midi_channel0
                    && (self.active_flags.val(i) & 8192) == 0
                    && self.active_off_ppq.val(i) > self.current_ppq
                {
                    self.active_off_ppq.set(i, on_ppq + 40.0 * ms);
                    source = true;
                }
            }
            if source && (self.wheel_bent || (0..64).any(|i| self.q.queue.at(i).used && self.q.queue.at(i).kind == 4)) {
                self.drop_queued_wheel(); // never a fall on a note the next one slides from
                self.schedule(4, 0, 64, on_ppq - 3.0 * ms, 0.0f32);
            }
            if source {
                self.legato_cc_sent = kb.kb_legato_cc;
                self.schedule(1, kb.kb_legato_cc, 127, on_ppq - 3.0 * ms, 0.0f32);
                self.schedule(1, kb.kb_legato_cc, 0, on_ppq + 60.0 * ms, 0.0f32);
            }
        }
    }

    /// `int naturalMute (const TargetData& kb, bool palm) const`
    pub(crate) fn natural_mute(&self, kb: &TargetData, palm: bool) -> i32 {
        let span = (kb.kb_mute_hi - kb.kb_mute_lo) as f32;
        let x = if palm {
            1.0f32 - 0.1f32 * (self.energy - 1) as f32
        } else if self.energy <= 1 {
            0.25f32
        } else if self.energy == 2 {
            0.1f32
        } else {
            0.0f32
        };
        kb.kb_mute_lo + (span * clamp(x, 0.0f32, 1.0f32)).round() as i32
    }
}

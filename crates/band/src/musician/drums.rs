//! `MusicianDrums.cpp`: Musician: drums, ported line by line (Task 15).
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.

use super::*;
use crate::meter::{is_backbeat, is_light_beat};

impl Musician {
    // 16-only: the fill vocabulary is 4/4's (steps 8-15); 12-step meters pass the step end-aligned (sib + 16 - spb()).
    /// `void drumFill (int variant, int sib, float tick)`
    pub(crate) fn drum_fill(&mut self, variant: i32, sib: i32, tick: f32) {
        let len = tick;
        // 6/8 has no backbeat on 3 or 9: a fill snare landing there (sib is end-aligned to 16) goes to the low tom.
        let real = sib + self.spb() - 16;
        let off68 = self.meter == Meter::M68 && (real == 3 || real == 9);
        // `snareOr` (a lambda): addStepNote (off68 ? 45 : 38, v, l, lane, d).
        let snare_or = |m: &mut Self, v: f32, l: f32, lane: i32, d: f32| m.add_step_note(if off68 { 45 } else { 38 }, v, l, lane, d);
        if variant == 0 {
            // Tom run down on beat 4.
            const TOMS: CArr<i32, 4> = CArr::new([50, 48, 45, 43]);
            self.add_step_note(TOMS.val(sib - 12), 84.0f32 + (sib - 12) as f32 * 5.0f32, len, 1, 0.0f32);
        } else if variant == 1 {
            // Snare 16ths building through beats 3 and 4.
            snare_or(self, 50.0f32 + (sib - 8) as f32 * 8.0f32, len, 1, 0.0f32);
        } else if variant == 2 {
            // Snare and toms in 8ths, kick underneath.
            if sib == 8 {
                snare_or(self, 90.0f32, len, 1, 0.0f32);
                self.add_step_note(36, 92.0f32, len, 0, 0.0f32);
            } else if sib == 10 {
                self.add_step_note(48, 88.0f32, len, 1, 0.0f32);
            } else if sib == 12 {
                self.add_step_note(45, 94.0f32, len, 1, 0.0f32);
                self.add_step_note(36, 90.0f32, len, 0, 0.0f32);
            } else if sib == 14 {
                self.add_step_note(43, 100.0f32, len, 1, 0.0f32);
            }
        } else if variant == 3 {
            // Syncopated kick / snare break.
            if sib == 8 || sib == 11 || sib == 15 {
                self.add_step_note(36, if sib == 8 { 100.0f32 } else { 90.0f32 }, len, 0, 0.0f32);
            } else if sib == 10 || sib == 13 {
                snare_or(self, if sib == 10 { 96.0f32 } else { 100.0f32 }, len, 1, 0.0f32);
            } else if sib == 14 {
                snare_or(self, 104.0f32, len, 1, 0.0f32);
                self.add_step_note(41, 96.0f32, len, 1, 0.0f32);
            }
        } else {
            // Open hat lift into a snare and floor tom hit.
            if sib == 12 {
                self.add_step_note(46, 90.0f32, len * 2.0f32, 2, 0.0f32);
                self.add_step_note(36, 92.0f32, len, 0, 0.0f32);
            } else if sib == 14 {
                snare_or(self, 104.0f32, len, 1, 0.0f32);
                self.add_step_note(41, 96.0f32, len, 1, 0.0f32);
            } else if sib == 15 {
                snare_or(self, 50.0f32, len, 1, 0.0f32);
            }
        }
    }

    // 12-step meters (Task 15): step k of an n-step fill zone ending on the bar line at sib. 6/8: 16ths in groups of three
    // (snare / toms; a snare on 3 or 9 goes to the low tom: no backbeat there). 3/4: over beat 3. The crash is the next 1's.
    /// `void drumFill12 (int variant, int k, int n, int sib, float tick)`
    pub(crate) fn drum_fill12(&mut self, variant: i32, k: i32, n: i32, sib: i32, tick: f32) {
        let off68 = self.meter == Meter::M68 && (sib == 3 || sib == 9);
        // `snare` (a lambda): addStepNote (off68 ? 45 : 38, v, tick, 1, 0.0f).
        let snare = |m: &mut Self, v: f32| m.add_step_note(if off68 { 45 } else { 38 }, v, tick, 1, 0.0f32);
        const TOMS: CArr<i32, 6> = CArr::new([50, 48, 47, 45, 43, 41]);
        let down = clamp(k + 6 - n, 0, 5); // the run ends on the floor tom
        if variant == 0 {
            self.add_step_note(TOMS.val(down), 84.0f32 + k as f32 * 4.0f32, tick, 1, 0.0f32); // tom run down
        } else if variant == 1 {
            snare(self, 60.0f32 + k as f32 * (40.0f32 / max(1, n - 1) as f32)); // snare build
        } else if self.meter == Meter::M68 {
            // Snare-tom-tom triplets, the kick under each group's first.
            let j = k % 3;
            if j == 0 {
                snare(self, 96.0f32);
                self.add_step_note(36, 92.0f32, tick, 0, 0.0f32);
            } else {
                self.add_step_note(TOMS.val(if k >= 3 || n == 3 { 2 + j } else { j }), 88.0f32 + k as f32 * 2.0f32, tick, 1, 0.0f32);
            }
        } else {
            // 3/4 beat 3: snare and kick, then two toms.
            if k == 0 {
                snare(self, 94.0f32);
                self.add_step_note(36, 92.0f32, tick, 0, 0.0f32);
            } else if k == 2 {
                self.add_step_note(48, 92.0f32, tick, 1, 0.0f32);
            } else if k == 3 {
                self.add_step_note(43, 100.0f32, tick, 1, 0.0f32);
            }
        }
    }

    // 12-step meters (spec 3.1 / 3.2): the 32-step grid index a drum lane plays at sib, or -1; vmul scales its velocity.
    // 3/4: identity, beat 2's snare copied to an empty beat 3 (both get the light-beat x0.85 in drumStep). 6/8: kicks only on
    // the dotted beats (the half's first kick; on 6 beside the snare only at Energy 3+), the backbeat (source 4, else 8) once
    // on 6 and never on 3 / 9, source 12's snare a x0.5 ghost on 10 at Energy 3+, an eighths-only hat on 0, 2, 4 of each half.
    // At Half speed the pattern's halves are heard bars: the same rules per half (backbeat on its step 3).
    /// `int drumSrc12 (int who, int lane, int bar16, int sib, float& vmul)`
    pub(crate) fn drum_src12(&mut self, who: i32, lane: i32, bar16: i32, sib: i32, vmul: &mut f32, lib: &LibraryData) -> i32 {
        *vmul = 1.0f32;
        if self.native_here(who, lib) {
            return bar16 + sib; // native grids12: the lane as written
        }
        // The lambdas `has` and `ty` take the musician explicitly (gridAt / laneType take it mutably).
        let has = |m: &mut Self, s: i32| m.grid_at(who, lane, bar16 + s, 0, lib) > 0;
        let ty = |m: &mut Self, s: i32| m.lane_type(who, lane, bar16 + s, lib);
        let kick = |n: i32| n == 35 || n == 36;
        let snare = |n: i32| (37..=40).contains(&n); // snare, side stick, clap
        let hat = |n: i32| n == 42 || n == 44 || n == 46 || n == 51 || n == 53;
        if self.meter == Meter::M34 {
            return bar16 + (if sib == 8 && !has(self, 8) && has(self, 4) && snare(ty(self, 4)) { 4 } else { sib });
        }
        let h = sib / 6;
        let t = sib % 6;
        let base = h * 8;
        let c = map16(self.meter, sib, GridRole::Drums);
        let snare_at = |m: &mut Self, k: i32| {
            for l in 0..4 {
                if m.grid_at(who, l, bar16 + k, 0, lib) > 0 && snare(m.lane_type(who, l, bar16 + k, lib)) {
                    return true;
                }
            }
            false
        };
        if self.speed == 0 {
            // Half speed: each 6-step half of the pattern is a heard 6/8 bar, so its step 3 is the heard backbeat (6) and the
            // source half's beat 2 (base + 4) is that bar's backbeat; kicks on the heard dotted beats (t 0, 3).
            if t == 0 || t == 3 {
                let from = base + (if t == 0 { 0 } else { 4 });
                if t == 3 && has(self, base + 4) && snare(ty(self, base + 4)) {
                    return bar16 + base + 4;
                }
                if has(self, from) && !kick(ty(self, from)) {
                    return bar16 + from;
                }
                for s in from..from + 4 {
                    if has(self, s) && kick(ty(self, s)) {
                        return if t == 3 && self.energy < 3 && snare_at(self, base + 4) { -1 } else { bar16 + s };
                    }
                }
                return bar16 + from;
            }
            if has(self, c) && kick(ty(self, c)) {
                return -1;
            }
            return bar16 + c;
        }
        if t == 0 {
            if h == 1 {
                let s4 = has(self, 4) && snare(ty(self, 4));
                let s8 = has(self, 8) && snare(ty(self, 8));
                if s4 || s8 {
                    // One backbeat on 6 (first hit wins, max velocity): never a double snare when source 8 has one too.
                    if s4 && s8 && self.lane_vel(who, lane, bar16 + 4, lib) > 0.0f32 {
                        *vmul = max(1.0f32, self.lane_vel(who, lane, bar16 + 8, lib) / self.lane_vel(who, lane, bar16 + 4, lib));
                    }
                    if !s4 {
                        // nor across lanes: another lane's beat-2 snare has 6
                        for l in 0..4 {
                            if l != lane && self.grid_at(who, l, bar16 + 4, 0, lib) > 0 && snare(self.lane_type(who, l, bar16 + 4, lib)) {
                                return -1;
                            }
                        }
                    }
                    return bar16 + (if s4 { 4 } else { 8 });
                }
            }
            if has(self, base) && !kick(ty(self, base)) {
                return bar16 + base;
            }
            for s in base..base + 8 {
                if has(self, s) && kick(ty(self, s)) {
                    if h == 1 && self.energy < 3 && (snare_at(self, 4) || snare_at(self, 8)) {
                        return -1; // the snare has 6
                    }
                    return bar16 + s;
                }
            }
            return bar16 + base;
        }
        let mut hat_lane = false;
        let mut sixteenths = false;
        for s in 0..16 {
            if has(self, s) && hat(ty(self, s)) {
                hat_lane = true;
                sixteenths |= s % 2 == 1;
            }
        }
        if hat_lane && !sixteenths {
            if t % 2 == 1 {
                return -1; // t 3 too: an eighths hat plays 0, 2, 4 of the half
            }
            if t == 2 {
                return bar16 + (if !has(self, base + 2) && has(self, base + 4) { base + 4 } else { base + 2 });
            }
            return bar16 + base + 6;
        }
        if h == 1 && t == 4 && self.energy >= 3 && !has(self, 14) && has(self, 12) && snare(ty(self, 12)) {
            *vmul = 0.5f32;
            return bar16 + 12;
        }
        if has(self, c) && (kick(ty(self, c)) || (t == 3 && snare(ty(self, c)))) {
            return -1; // kicks on the dotted beats; no backbeat on 3 / 9
        }
        bar16 + c
    }

    /// `void drumStep (int who, int step, int pass, int progBar, int sib, float64 stepPpq, bool changeNext)`
    pub(crate) fn drum_step(
        &mut self,
        who: i32,
        step: i32,
        pass: i32,
        prog_bar: i32,
        sib: i32,
        step_ppq: f64,
        change_next: bool,
        cx: &mut Ctx<'_>,
        out: &mut dyn MidiSink,
    ) {
        let lib = cx.lib;
        let tick = self.tick_ppq(lib);
        // The lanes are 2 bars of 16 steps; 12-step meters read them through gridStep (drums).
        let bar16 = self.positive_modulo(prog_bar, 2) * 16;
        let gstep = bar16 + self.grid_step(who, sib, GridRole::Drums, lib);
        // A forced fill (the Fill key, a jump, an Ending) belongs to the bar it lands in, whatever the speed:
        // at Half speed the 16-step pattern spans two bars, so its own step counter would put the fill in the wrong one.
        let bar_l = f64::from(self.get_bar_length_ppq());
        // 12-step meters: the fill zone is the heard bar's (at Half speed the pattern's sib spans two bars).
        let fsib = if self.perf_fill || is12(self.meter) {
            clamp(self.floor_to_int((step_ppq - self.bar_origin) % bar_l / bar_l * f64::from(self.spb()) + 1.0e-6), 0, self.spb() - 1)
        } else {
            sib
        };
        // The zone: 4/4 from beat 3 (the tom run / hat lift from beat 4); 12-step meters from fillZone(), the vocabulary end-aligned.
        let zone = if self.meter == Meter::M44 { 8 } else { self.fill_zone() };
        let fs = fsib + 16 - self.spb();
        let fill = fsib >= zone && self.fill_active(who, pass, prog_bar, change_next, true, cx, out);
        // 12-step meters (Task 15): their own fill vocabulary over the whole zone, its variant drawn with salt 1996 (only here,
        // so the 4/4 stream is untouched). 6/8: 16ths grouped in threes over the last dotted beat (both at Energy 4); 3/4: beat 3.
        let variant = if !fill {
            -1
        } else if is12(self.meter) {
            min(2, (self.rand01(pass, prog_bar, 1996, who, cx) * 3.0f32) as i32)
        } else {
            self.fill_variant(who, pass, prog_bar, cx)
        };
        let fill_start = if variant == 0 || variant == 4 { 12 } else { 8 };
        let in_fill = fill && (is12(self.meter) || fs >= fill_start);
        let keep_kick =
            (if is12(self.meter) { variant != 2 } else { !(variant == 2 || variant == 3 || variant == 4) }) && self.feel_for(who, lib).heavy == 0;

        self.clear_step();
        // (the lambda `snare` (38 or 40) is unused here)
        for lane in 0..4 {
            if in_fill && !(lane == 0 && keep_kick) {
                continue;
            }
            let mut vmul = 1.0f32;
            let g = if is12(self.meter) { self.drum_src12(who, lane, bar16, sib, &mut vmul, lib) } else { gstep }; // 12-step: the lane's adapted source
            if g < 0 || !self.lane_hit(who, lane, g, step, pass, cx) {
                continue;
            }
            let type_ = self.lane_type(who, lane, g, lib);
            let mut vel = self.lane_vel(who, lane, g, lib) * vmul;
            if is_light_beat(self.meter, self.sixteenth_of(step_ppq)) && (35..=40).contains(&type_) {
                // kick, side stick, snare, clap
                vel *= 0.85f32; // 3/4: beats 2 and 3 lighter, no backbeat accent
            }
            let l = self.lane_len(who, lane, g, tick, lib);
            self.add_step_note(type_, vel, l, lane, 0.0f32);
        }
        if in_fill && is12(self.meter) {
            self.drum_fill12(variant, fsib - zone, self.spb() - zone, fsib, tick);
        } else if in_fill {
            self.drum_fill(variant, fs, tick);
        }
        if in_fill && self.perf.interplay() {
            let bar_start = self.bar_floor(step_ppq, bar_l);
            if !(step_ppq >= cx.shared.drum_fill_from - 1.0e-9 && step_ppq < cx.shared.drum_fill_to) {
                cx.shared.drum_fill_from = step_ppq;
            }
            cx.shared.drum_fill_to = bar_start + bar_l;
        }
        if in_fill && self.feel_for(who, lib).heavy == 1 {
            self.add_step_note(36, 100.0f32, tick, 0, 0.0f32);
        }
        if self.ten_now > 0.0f32
            && self.ten_ramp > 0.0f32
            && !in_fill
            && sib % 2 == 1
            && self.rand01(pass, gstep, 2001, who, cx) < self.ten_now * 0.7f32 * (0.3f32 + 0.7f32 * self.ten_ramp)
        {
            self.add_step_note(38, 40.0f32 + 40.0f32 * self.ten_ramp, tick, 1, 0.0f32); // tension: the snare builds
        }
        if self.rel_down {
            self.add_step_note(49, 104.0f32, tick * 4.0f32, 3, 0.0f32); // ... and the release lands
        }
        let modo_natural = self.expressive() && self.driver_is(Driver::DrumMap, lib) && self.target_plays(self.pattern, lib);
        if modo_natural {
            self.natural_drums(who, pass, prog_bar, gstep, sib, in_fill, if is12(self.meter) { -1 } else { variant }, step_ppq, cx); // 12-step: no 16-step flam / sticking map
        }

        // Crash on the downbeat after a fill, and at the top of each new pass (and wherever the band hits together).
        if (sib == 0 && self.energy >= 2) || self.perf_crash {
            let mut prev_bar = prog_bar - 1;
            let mut prev_pass = pass;
            if prev_bar < 0 {
                prev_bar = self.num_bars() - 1;
                prev_pass = pass - 1;
            }
            let after_fill = prev_pass >= 0 && self.fill_active(who, prev_pass, prev_bar, true, true, cx, out);
            if after_fill || (prog_bar == 0 && pass > 0) || self.perf_crash {
                let mut crash = 49;
                let mut china = false;
                if modo_natural {
                    // MODO DRUM in Natural: two crashes alternating by section, a splash at low Energy, a china on
                    // section downbeats for the players who have one.
                    let t = self.current_target(lib);
                    let ds = &self.feel_for(who, lib).drums;
                    let sec = self.drum_section(pass);
                    if t.dr_crashes[0] >= 0 && self.rand01(pass, sec, 1945, who, cx) < ds.crash_alternate {
                        let alt = at_or_default(&t.dr_crashes, self.positive_modulo(sec, 2));
                        crash = if alt >= 0 { alt } else { 49 };
                    }
                    if self.energy <= 2 && t.dr_splash >= 0 {
                        crash = t.dr_splash;
                    } else if prog_bar == 0 && self.energy >= 3 && t.dr_china >= 0 && self.rand01(pass, sec, 1946, who, cx) < ds.china {
                        crash = t.dr_china;
                        china = true;
                    }
                }
                let mut had = false;
                if modo_natural {
                    // the groove's own crash here is the chosen cymbal
                    for i in 0..self.step_count {
                        if self.step_note.val(i) == 49 || self.step_note.val(i) == crash {
                            self.step_note.set(i, crash);
                            self.step_vel.set(i, max(self.step_vel.val(i), 104.0f32));
                            had = true;
                        }
                    }
                }
                if !had {
                    self.add_step_note(crash, 104.0f32, tick * 4.0f32, 3, 0.0f32);
                }
                if self.feel_for(who, lib).heavy == 1 && !china {
                    self.add_step_note(52, 100.0f32, tick * 4.0f32, 3, 0.0f32);
                }
            }
        }
        // Natural on MODO DRUM: a Break's or the Ending's crash is choked a beat later.
        if modo_natural && self.stop_hit {
            for i in 0..self.step_count {
                for &(cym, choke) in &self.current_target(lib).dr_chokes {
                    if self.step_note.val(i) == cym {
                        self.schedule(0, choke, 96, step_ppq + 1.0, tick);
                    }
                }
            }
        }
        self.perform_step(who, step, pass, prog_bar, sib, step_ppq, cx, out);
    }

    /// `void naturalDrums (int who, int pass, int progBar, int gstep, int sib, bool inFill, int variant, float64 stepPpq)`
    pub(crate) fn natural_drums(
        &mut self,
        who: i32,
        pass: i32,
        prog_bar: i32,
        gstep: i32,
        sib: i32,
        in_fill: bool,
        variant: i32,
        step_ppq: f64,
        cx: &mut Ctx<'_>,
    ) {
        let lib = cx.lib;
        let t = self.current_target(lib);
        let ds = &self.feel_for(who, lib).drums;
        let tick = self.tick_ppq(lib);
        let sec = self.drum_section(pass);
        let cross_on =
            t.dr_cross_stick >= 0 && !in_fill && (self.energy == 1 || (self.energy == 2 && self.rand01(pass, sec, 1941, who, cx) < ds.cross_stick));
        let ride2 = t.dr_ride2 >= 0 && self.positive_modulo(sec, 2) == 1 && self.rand01(pass, sec, 1947, who, cx) < ds.ride_switch;
        // Flams: a fill's first snare, the (first) backbeat of a section's first bar. Fill steps are end-aligned (16-only vocabulary).
        let fs = sib + 16 - self.spb();
        const FIRST_SNARE: CArr<i32, 5> = CArr::new([-1, 8, 8, 10, 14]);
        let flam_step = (if in_fill {
            (0..5).contains(&variant) && fs == FIRST_SNARE.val(variant)
        } else {
            prog_bar == 0 && is_backbeat(self.meter, sib) && (self.meter != Meter::M44 || sib == 4)
        }) && self.rand01(pass, gstep, 1943, who, cx) < ds.flams * (if self.energy == 4 { 1.3f32 } else { 1.0f32 });
        // Sticking: a doubled right hand on the first two of three 16ths (R R L) in the tom run / snare build.
        let stick0 = if variant == 0 {
            12
        } else if variant == 1 {
            8
        } else {
            -1
        };
        let stick_now =
            in_fill && stick0 >= 0 && (fs == stick0 || fs == stick0 + 1) && self.rand01(pass, prog_bar, 1948, who, cx) < ds.flams * 0.5f32;
        // The bar as heard (at Half speed the pattern spans two bars): its "and of 4", its 2 and 4, its downbeat.
        let bar_l = f64::from(self.get_bar_length_ppq());
        let bsib = clamp(self.floor_to_int((step_ppq - self.bar_origin) % bar_l / bar_l * f64::from(self.spb()) + 1.0e-6), 0, self.spb() - 1);
        let bark_close = bsib == 0
            && self.dr_bark_from >= 0.0
            && (step_ppq - (self.dr_bark_from + bar_l * 2.0 / f64::from(self.spb()))).abs() < 0.25 * f64::from(tick);
        let bar_no = self.floor_to_int((step_ppq - self.bar_origin) / bar_l + 1.0e-9);
        let mut closed = false;
        let mut ride = self.dr_ride_bar == bar_no;
        let mut pedal = false;
        let mut flam_done = false;
        let count = self.step_count;
        for i in 0..count {
            // `int& n = stepNote.at (i)`: read here, written back after the body (every `continue` included).
            let mut n = self.step_note.val(i);
            'body: {
                let acc = self.accent_at(who, self.positive_modulo(sib, self.spb()), lib) >= 1.03f32 || self.step_vel.val(i) >= 96.0f32;
                let hat = n == 42 || n == 46;
                if bark_close && hat && t.dr_pedal_hat >= 0 {
                    n = t.dr_pedal_hat;
                    self.step_vel.set(i, 70.0f32);
                    closed = true;
                    break 'body;
                }
                if hat && bsib == self.spb() - 2 && self.energy >= 3 && t.dr_open_shank >= 0 && self.rand01(pass, gstep, 1944, who, cx) < ds.hat_bark
                {
                    n = t.dr_open_shank;
                    self.dr_bark_from = step_ppq;
                    break 'body;
                }
                if n == 42 && self.energy == 3 && sib % 2 == 1 && t.dr_aperture_cc >= 0 && t.dr_cc_tip >= 0 {
                    n = if acc && t.dr_cc_shank >= 0 { t.dr_cc_shank } else { t.dr_cc_tip };
                    let f = self.step_flags.val(i);
                    self.step_flags.set(i, f | K_APERTURE_FLAG);
                    break 'body;
                }
                if n == 51 {
                    ride = true;
                }
                if n == 53 || n == 44 {
                    ride |= n == 53;
                    pedal |= n == 44;
                }
                if n == 42 && acc && t.dr_shank_closed >= 0 {
                    n = t.dr_shank_closed;
                } else if n == 46 {
                    if self.energy >= 4 && t.dr_extra_open >= 0 && self.rand01(pass, gstep, 1801, who, cx) < 0.5f32 {
                        n = t.dr_extra_open;
                    } else if acc && t.dr_shank_open >= 0 {
                        n = t.dr_shank_open;
                    }
                } else if (n == 51 || n == 53)
                    && self.energy >= 4
                    && bsib == 0
                    && acc
                    && t.dr_bell_cymbal >= 0
                    && self.rand01(pass, gstep, 1950, who, cx) < ds.bell_cymbal
                {
                    n = t.dr_bell_cymbal;
                } else if n == 51
                    && self.energy >= 4
                    && is_beat(self.meter, sib)
                    && t.dr_ride_bell >= 0
                    && self.rand01(pass, gstep, 1803, who, cx) < 0.5f32
                {
                    n = t.dr_ride_bell;
                }
                if n == 51 && ride2 {
                    n = t.dr_ride2;
                }
                // At Energy 1 the cross-stick wins over a section's first-backbeat flam (the flam only takes a 38).
                if n == 38 && cross_on && (is_backbeat(self.meter, sib) || is_light_beat(self.meter, sib)) && self.step_vel.val(i) >= 60.0f32 {
                    n = t.dr_cross_stick;
                }
                if n == 38 && flam_step && !flam_done {
                    let f = self.step_flags.val(i);
                    self.step_flags.set(i, f | K_FLAM_FLAG | K_DRUM_RIGHT_FLAG);
                    flam_done = true;
                }
                if stick_now && (n == 38 || n == 41 || n == 43 || n == 45 || n == 47 || n == 48 || n == 50) {
                    let f = self.step_flags.val(i);
                    self.step_flags.set(i, f | K_DRUM_RIGHT_FLAG);
                }
            }
            self.step_note.set(i, n);
        }
        if ride {
            self.dr_ride_bar = bar_no;
        }
        if bark_close && !closed && t.dr_pedal_hat >= 0 {
            self.add_step_note(t.dr_pedal_hat, 70.0f32, tick, 2, 0.0f32);
            pedal = true;
        }
        // Pedal hat on the pattern's backbeats (3/4: beats 2 and 3) when the ride carries the groove, or in a fill (per bar).
        if (is_backbeat(self.meter, sib) || is_light_beat(self.meter, sib))
            && !pedal
            && t.dr_pedal_hat >= 0
            && (ride || in_fill)
            && self.rand01(pass, prog_bar, 1942, who, cx) < ds.pedal_hat
        {
            let v = 60.0f32 + 15.0f32 * self.rand01(pass, gstep, 1942, who, cx);
            self.add_step_note(t.dr_pedal_hat, v, tick, 2, 0.0f32);
        }
        if !in_fill && self.energy >= 3 && is_backbeat(self.meter, sib) && t.dr_foot_splash >= 0 && self.rand01(pass, gstep, 1805, who, cx) < 0.15f32
        {
            let tk = self.tick_ppq(lib);
            self.add_step_note(t.dr_foot_splash, 70.0f32, tk, 2, 0.0f32);
        }
    }
}

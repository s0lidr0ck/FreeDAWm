//! `MusicianQueue.cpp`: the note queue (held notes, clearing, regeneration, kept-take replay).
//!
//! Calling convention (`musician/mod.rs`): a C++ method that sends MIDI takes `cx: &mut Ctx<'_>` and
//! `out: &mut dyn MidiSink`; one that reads the song or the interplay record takes `cx`; one that reads only the
//! Library takes `lib: &LibraryData`. C++ default arguments are passed explicitly by the callers.

// Line-by-line translation: the C++ shapes (a default then field writes, a counter beside a loop, a late init) are kept.
#![allow(clippy::field_reassign_with_default, clippy::explicit_counter_loop, clippy::needless_late_init)]

use super::*;

impl Musician {
    /// `void regenerateFrom (double t)`
    pub(crate) fn regenerate_from(&mut self, t: f64, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        let tick = f64::from(self.tick_ppq(lib));
        let mut first = (t / tick - 1.0e-9).ceil() as i32;
        first = max(first, self.floor_to_int(self.current_ppq / tick) + 1); // a step already sounding stays as it was
        if first > self.last_emitted_step {
            return;
        }
        let cut = max(f64::from(first) * tick - 0.5 * tick, self.current_ppq + 1.0e-9); // humanize can pull a step a little early
        self.fd_drop_queued_keys(cut, lib); // The Fiddle: its latch follows what was really sent
        // Embertone: back to the last note before the cut first (its re-bows survive the drop); whether a run's press is dropped.
        let jb_dropped = self.jb_rewind(cut, lib);
        let bass_lost = self.bass_lost_from(cut); // MODO BASS: what of a fall / vibrato / ring goes
        let piano_lost = self.piano_lost_from(cut); // Pianoteq: a harmonic pedal's queued lift goes
        for i in 0..64 {
            if self.q.queue.at(i).used && self.q.queue.at(i).on_ppq >= cut && !self.fd_inner_attack(i, cut) && !self.jb_inner_attack(i, cut) {
                self.q.queue.at_mut(i).used = false;
            }
        }
        self.jb_after_rewind(cut, jb_dropped, lib);
        self.last_emitted_step = first - 1;
        self.ample_drop_from(cut); // its queued articulation / chord keys may be gone
        self.ks_drop_from(cut, lib); // keyswitch: its held / latched keys and vibrato as really sent
        self.evolution_drop_from(cut, lib); // Evolution: a held key whose owner went comes up at the cut
        self.bass_drop_from(cut, bass_lost, cx, out);
        self.piano_drop_from(cut, piano_lost);
        self.organ_drop_from(cut, lib); // Hammond: what of its registration / swell went
        self.brass_drop_from(cut, lib); // Blue Street Brass: its legato key / CC1 as still standing
        if self.pushed_from >= cut {
            self.pushed_at = -1.0;
            self.pushed_from = -1.0;
            self.push_verify = false;
        }
        // the push is made again (or not)
        else if self.pushed_at >= cut {
            self.push_verify = true; // its downbeat is: strike it if the chord changed
        }
        if self.dr_bark_from >= cut - 0.5 * tick {
            self.dr_bark_from = -1.0; // a hat bark made again decides again
        }
        if self.lick_land_set_at >= 0.0 && self.lick_land_set_at >= cut {
            self.lick_land_forget(); // the pickup step decides its kick-off again
        }
        if self.lick_fill_at >= 0.0 && self.lick_fill_at >= cut - 1.0e-9 {
            self.lick_fill_at = self.lick_fill_prev_at;
            self.lick_fill_prev_at = -1.0;
        } // that fill bar decides again
    }

    // ---- Note output ---------------------------------------------------------
    /// `void sendKeyswitch (int status, int note, int vel)`
    pub(crate) fn send_keyswitch(&mut self, status: i32, note: i32, vel: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.log_flag = 0x100;
        self.send_midi(status, note, vel, cx, out);
        self.log_flag = 0;
    }

    /// `void sendSlotNoteOff (int slot)`
    pub(crate) fn send_slot_note_off(&mut self, slot: i32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if !self.active_slots.val(slot) {
            return;
        }
        let lib = cx.lib;
        if (self.active_flags.val(slot) & (K_IN_LATCH_FLAG | K_IN_SAME_FLAG)) != 0 {
            // Indiginus latch / same: the key was tapped at its press; its "release" is the return tap (the target's
            // return key, or the same key again).
            let ch = self.active_channels.val(slot);
            if self.active_tap_off.val(slot) >= 0.0 {
                self.send_keyswitch(0x80 + ch, self.active_notes.val(slot), 0, cx, out);
                self.active_tap_off.set(slot, -1.0);
            } // the press tap still down
            let ret = if (self.active_flags.val(slot) & K_IN_SAME_FLAG) != 0 {
                self.active_notes.val(slot)
            } else if self.in_drv(lib) {
                self.current_target(lib).in_return_key
            } else {
                -1
            };
            self.active_slots.set(slot, false);
            if ret >= 0 && self.in_stopping {
                self.send_keyswitch(0x90 + ch, ret, 100, cx, out);
                self.send_keyswitch(0x80 + ch, ret, 0, cx, out);
            }
            // a stop: nothing queued is kept
            else if ret >= 0 {
                let keep = self.q.schedule_flags;
                let ms = self.ppq_per_ms();
                let now = self.current_ppq;
                let mut s = SchedAt { q: &mut self.q, now };
                let mut c = kit::DriverCtx { sink: &mut s, now, ms };
                kit::tap_key(&mut c, ret, 100, now, 5.0);
                self.q.schedule_flags = keep;
            }
        } else {
            if (self.active_flags.val(slot) & 8192) != 0 {
                self.log_flag = 0x100;
            }
            let off_vel = if self.active_off_vel.val(slot) >= 0 { self.active_off_vel.val(slot) } else { self.release_velocity(slot, lib) };
            self.send_midi(0x80 + self.active_channels.val(slot), self.active_notes.val(slot), off_vel, cx, out);
            self.log_flag = 0;
        }
        self.active_slots.set(slot, false);
        if (self.active_flags.val(slot) & 1) != 0 && self.ghost_held > 0 {
            self.ghost_held -= 1;
            if self.ghost_held == 0 {
                self.send_keyswitch(0x80 + self.midi_channel0, self.kb_ghost_note_sent, 0, cx, out);
            }
        }
        self.active_flags.set(slot, 0);
    }

    /// `void clearQueue ()`
    pub(crate) fn clear_queue(&mut self) {
        for i in 0..64 {
            self.q.queue.at_mut(i).used = false;
        }
        self.q.next_due_ppq = 1.0e18;
    }

    /// `void stopAllNotes ()`
    pub(crate) fn stop_all_notes(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        let lib = cx.lib;
        self.fd_return_now(cx, out); // The Fiddle: return a latched articulation before the queue goes
        self.in_stopping = true;
        self.evolution_off(cx, out); // Evolution: held keys up, the vibrato CC back to 0
        self.clear_queue();
        for i in 0..64 {
            self.send_slot_note_off(i, cx, out);
        }
        if !self.organ_active(lib) && !self.brass_active(lib) {
            self.send_midi(0xB0 + self.midi_channel0, 64, 0, cx, out); // an organ / Blue Street Brass has no sustain pedal
        }
        self.organ_stop(cx, out);
        self.brass_stop(cx, out);
        if self.expr_used {
            self.send_midi(0xB0 + self.midi_channel0, 11, 127, cx, out);
            self.expr_used = false;
        }
        if self.mute_cc > 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.kb_palm_cc_sent, 0, cx, out);
        }
        if self.soft_pedal > 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.soft_pedal_cc_sent, 0, cx, out);
        }
        if self.legato_on && self.legato_cc_sent >= 0 {
            self.send_midi(0xB0 + self.midi_channel0, self.legato_cc_sent, 0, cx, out);
        }
        self.bass_expression_lift(cx, out);
        self.harmonic_lift(cx, out);
        self.mute_cc = -1;
        self.soft_pedal = -1;
        self.style_ks = -1;
        self.ghost_held = 0;
        self.pedal_level = -1;
        self.legato_on = false;
        self.last_bass_pitch = -1;
        self.last_bass_root = -1;
        self.last_bass_step_ppq = -1.0e9;
        self.swam_reset(cx, out);
        self.crow_reset(cx, out);
        self.ks_reset(cx, out);
        self.ujam_reset(cx, out);
        self.ample_vibrato_off(cx, out);
        self.ample_reset();
        self.push_now = false;
        self.pushed_at = -1.0;
        self.pushed_from = -1.0;
        self.push_verify = false;
        self.dr_bark_from = -1.0;
        self.dr_ride_bar = -1;
        self.send_midi(0xB0 + self.midi_channel0, 123, 0, cx, out);
        self.in_stopping = false;
    }

    /// `void stopExpiredNotes (float64 position)`
    pub(crate) fn stop_expired_notes(&mut self, position: f64, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        for i in 0..64 {
            if self.active_slots.val(i) && self.active_tap_off.val(i) >= 0.0 && position >= self.active_tap_off.val(i) {
                self.log_flag = 0x100;
                self.send_midi(0x80 + self.active_channels.val(i), self.active_notes.val(i), 0, cx, out);
                self.log_flag = 0;
                self.active_tap_off.set(i, -1.0);
            }
            if self.active_slots.val(i) && position >= self.active_off_ppq.val(i) {
                self.send_slot_note_off(i, cx, out);
            }
        }
    }

    /// `void playFrozen (const FrozenPart& fp, float64 occStart, float64 occLen, float64 stepPpq)`
    pub(crate) fn play_frozen(&mut self, fp: &FrozenPart, occ_start: f64, occ_len: f64, step_ppq: f64, cx: &mut Ctx<'_>) {
        let lib = cx.lib;
        let tick = f64::from(self.tick_ppq(lib));
        let a = step_ppq - occ_start;
        let b = a + tick;
        let first_step = a < tick * 0.5; // and what was played a hair early, before the section
        self.style_ks = -1;
        self.mute_cc = -1; // the take sends its own; a live section after it sends again
        if self.ample_active(lib) {
            self.ample_sig = -1;
            self.ample_forget();
        } // the take carries its own Ample setup; a live section after it sends again
        let shortened = fp.length_ppq > occ_len + 1.0e-6; // its chords got shorter since it was kept
        for (i, &ev) in fp.events.iter().enumerate() {
            let rel = ev.ppq;
            if rel >= occ_len - 1.0e-9 {
                continue; // past the section's end (its chords got shorter)
            }
            if !((rel >= a - 1.0e-9 && rel < b - 1.0e-9) || (first_step && rel < a)) {
                continue;
            }
            let type_ = ev.status & 0xF0;
            if type_ == 0x90 && ev.d2 > 0 {
                let mut dur = 0.25f64;
                let mut off_vel = -1;
                for o in fp.events.get(i + 1..).unwrap_or_default() {
                    if o.d1 == ev.d1
                        && (o.status & 0x0F) == (ev.status & 0x0F)
                        && ((o.status & 0xF0) == 0x80 || ((o.status & 0xF0) == 0x90 && o.d2 == 0))
                    {
                        dur = o.ppq - rel;
                        off_vel = if (o.status & 0xF0) == 0x80 { i32::from(o.d2) } else { 0 };
                        break;
                    }
                }
                if shortened && rel + dur > occ_len {
                    dur = occ_len - rel; // nothing rings past the new end
                }
                // Kept on the Hammond's lower manual: back there when it is set (else onto the upper one).
                let low_ch = if self.organ_active(lib) && (ev.status & 0x0F) == 1 { self.organ_lower_channel(lib) } else { 0 };
                if !self.organ_active(lib) && ev.keyswitch && (ev.status & 0x0F) != 0 {
                    continue; // the Hammond's keys: not this target's
                }
                if self.organ_active(lib) && ev.keyswitch && (ev.status & 0x0F) == 1 {
                    let mut own = false;
                    for k in 0..4 {
                        if at_or_default(&self.current_target(lib).og_lower_keys, k) == i32::from(ev.d1) {
                            own = true;
                        }
                    }
                    if !own || low_ch == 0 {
                        continue;
                    }
                } else if self.organ_active(lib) && ev.keyswitch {
                    // A take kept on another target (The Fiddle, MODO, Ample): its keyswitches are not the organ's.
                    // Only the organ's own drawbar keys pass; anything else would hit a registration key or sound.
                    let mut own = false;
                    for k in 0..4 {
                        if at_or_default(&self.current_target(lib).og_drawbar_keys, k) == i32::from(ev.d1) {
                            own = true;
                        }
                    }
                    if self.organ_reg_energy(self.active_player(lib), i32::from(ev.d1), lib) != 0 {
                        own = true; // the player's own ladder
                    }
                    if !own {
                        continue;
                    }
                }
                self.q.schedule_flags = K_RAW_FLAG | (if ev.keyswitch { 8192 } else { 0 });
                let mut pitch = i32::from(ev.d1);
                let mut vel = i32::from(ev.d2);
                if self.organ_active(lib) && !ev.keyswitch {
                    // A take kept on another target: onto the organ's manual, at its fixed velocity; a pitch two voices
                    // fold onto at one moment is played once.
                    let lo = if low_ch > 0 { 48 } else { 36 }; // the lower manual's keys 36-47 are its presets
                    while pitch < lo {
                        pitch += 12;
                    }
                    while pitch > 96 {
                        pitch -= 12;
                    }
                    vel = self.current_target(lib).og_note_vel;
                    let mut dup = false;
                    for o in fp.events.get(..i).unwrap_or_default() {
                        if (o.status & 0xF0) != 0x90 || o.d2 == 0 || o.keyswitch || (o.ppq - rel).abs() > 1.0e-6 {
                            continue;
                        }
                        if (if (o.status & 0x0F) == 1 { self.organ_lower_channel(lib) } else { 0 }) != low_ch {
                            continue; // the other manual
                        }
                        let mut q = i32::from(o.d1);
                        while q < lo {
                            q += 12;
                        }
                        while q > 96 {
                            q -= 12;
                        }
                        if q == pitch {
                            dup = true;
                        }
                    }
                    if dup {
                        self.q.schedule_flags = 0;
                        continue;
                    }
                }
                // Half a sample early: the playhead's running sum can sit a hair under the recorded time, which
                // would put the note one sample late.
                let half_sample = 0.5 * f64::from(self.host_tempo) / 60.0 / max(1.0, self.sample_rate);
                self.q.schedule_off_vel = off_vel; // the note-off as recorded (release velocity included)
                self.q.schedule_ch = low_ch;
                if low_ch > 0 {
                    self.og_lower_ch_used = low_ch - 1;
                }
                self.schedule(0, pitch, vel, occ_start + rel - half_sample, (if dur > 0.0 { dur } else { 0.0 }) as f32);
                self.q.schedule_flags = 0;
                self.q.schedule_off_vel = -1;
                self.q.schedule_ch = 0;
                if self.is_drums() && (ev.d1 == 35 || ev.d1 == 36) {
                    cx.shared.add_kick(occ_start + rel); // bass lock still hears it
                }
            } else if type_ == 0xB0 && (ev.status & 0x0F) == 0 {
                self.schedule(1, i32::from(ev.d1), i32::from(ev.d2), occ_start + rel, 0.0);
            } else if type_ == 0xE0 {
                self.schedule(4, i32::from(ev.d1), i32::from(ev.d2), occ_start + rel, 0.0); // a kept MODO BASS fall
            }
        }
        if shortened && b >= occ_len - 1.0e-9 {
            if !self.organ_active(lib) {
                self.schedule(1, 64, 0, occ_start + occ_len - 0.01, 0.0); // the pedal comes up at the new end
            }
            // ... and a kept MODO BASS fall / vibrato cut off there lets go too.
            let vib_cc = if self.bass_target(lib) { self.current_target(lib).kb_vibrato_cc } else { -1 };
            let mut wheel = false;
            let mut vib = false;
            for ev in &fp.events {
                if (ev.status & 0xF0) == 0xE0 {
                    wheel = true;
                }
                if (ev.status & 0xF0) == 0xB0 && vib_cc >= 0 && i32::from(ev.d1) == vib_cc {
                    vib = true;
                }
            }
            if wheel {
                self.schedule(4, 0, 64, occ_start + occ_len - 0.01, 0.0);
            }
            if vib {
                self.schedule(1, vib_cc, 0, occ_start + occ_len - 0.01, 0.0);
            }
        }
        if self.perf.interplay() {
            let chordal = Self::chordal_instrument(self.pattern);
            let drums = self.is_drums();
            let me = &mut cx.shared.m[usize::try_from(clamp(self.index, 0, 5)).unwrap_or(0)];
            me.chordal = chordal;
            me.drums = drums;
            me.channel = self.midi_channel0 + 1;
            me.seen = step_ppq;
        }
    }
}

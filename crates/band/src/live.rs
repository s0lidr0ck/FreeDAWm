//! Port of `Live.h`.
//!
//! Live — playing the band from a MIDI keyboard (JUCE-free, allocation-free).
//! recogniseChord: the held notes -> one of MIDI Studio's chord slots (degree x 16 + flavor) plus a
//! slash bass. Performer: turns incoming notes (and section clicks) into a PerformanceState the
//! engine reads every step: the live chord now and next, a section jump, a forced fill, a break, a
//! band-wide energy nudge and an ending, each with the ppq where it lands. All-zero = not performing.

use crate::song::{SongTimeline, clamp_int, song_locate};

// ---- Chords -------------------------------------------------------------------------------------

/// The pitch classes (key-relative) a slot plays, exactly as the engine's buildChord builds them; returns the root pc.
pub fn slot_pitch_classes(slot: i32, scale_mode: i32, out: &mut [bool; 12]) -> i32 {
    const STEPS: [[i32; 7]; 2] = [[0, 2, 4, 5, 7, 9, 11], [0, 2, 3, 5, 7, 8, 10]];
    // steps[mode]: mode is 0 or 1 from every caller; anything else reads as major (C++ would read out of bounds).
    let off = |mode: i32, deg: i32| -> i32 {
        let row = if mode == 1 { &STEPS[1] } else { &STEPS[0] };
        12 * (if deg >= 0 { deg / 7 } else { -((-deg + 6) / 7) }) + row[((deg % 7) + 7).rem_euclid(7) as usize]
    };
    out.fill(false);
    let degree = clamp_int(slot / 16, 0, 6);
    let mut flavor = slot % 16;
    if flavor > 8 {
        flavor = 0;
    }
    let mode = if flavor == 7 || flavor == 8 { 1 - scale_mode } else { scale_mode };
    let r = off(mode, degree);
    let mut tones: [i32; 4] = [0, off(mode, degree + 2) - r, off(mode, degree + 4) - r, 0];
    let seventh = off(mode, degree + 6) - r;
    let mut count = 3;
    if flavor == 1 || flavor == 8 {
        tones[3] = seventh;
        count = 4;
    } else if flavor == 2 {
        tones[1] = 4;
        tones[2] = 7;
        tones[3] = 10;
        count = 4;
    } else if flavor == 3 {
        tones[1] = 2;
    } else if flavor == 4 {
        tones[1] = 5;
    } else if flavor == 5 {
        tones[3] = 14;
        count = 4;
    } else if flavor == 6 {
        tones[3] = 9;
        count = 4;
    }
    let root_pc = ((r % 12) + 12) % 12;
    for t in tones.iter().take(count) {
        out[(root_pc + t).rem_euclid(12) as usize] = true;
    }
    root_pc
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveChord {
    pub slot: i32,
    pub bass: i32,
}

impl Default for LiveChord {
    fn default() -> Self {
        Self { slot: 0, bass: -1 }
    }
}

/// Held pitch classes (key-relative) and the lowest note's pc -> the closest slot, plus a slash bass when
/// the lowest note isn't the root. Total: always returns a chord (nothing held -> I).
pub fn recognise_chord(held: &[bool; 12], lowest_pc: i32, scale_mode: i32) -> LiveChord {
    let held_count = held.iter().filter(|h| **h).count();
    if held_count == 0 {
        return LiveChord::default();
    }
    let mut best = -1.0e9_f64;
    let mut pick = LiveChord::default();
    for degree in 0..7 {
        for flavor in 0..=8 {
            let mut tones = [false; 12];
            let root = slot_pitch_classes(degree * 16 + flavor, scale_mode, &mut tones);
            let mut score = 0.0_f64;
            for pc in 0..12 {
                if held[pc] && tones[pc] {
                    score += 2.0;
                } else if held[pc] {
                    score -= 3.0;
                } else if tones[pc] {
                    score -= 1.0;
                }
            }
            if root == lowest_pc {
                score += 0.5; // less than one extra tone (-1): Am/C stays Am/C, not C6
            }
            if flavor == 7 || flavor == 8 {
                score -= 0.25;
            }
            score -= 0.1 * f64::from(flavor);
            if score > best {
                best = score;
                pick.slot = degree * 16 + flavor;
                pick.bass = root;
            }
        }
    }
    pick.bass = if pick.bass == lowest_pc { -1 } else { lowest_pc };
    pick
}

// ---- What the engine reads -----------------------------------------------------------------------

/// Published by the Performer once per block. ppq values are host positions; -1 = nothing pending.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerformanceState {
    pub live: bool,
    /// Live: bars per phrase
    pub fill_every: i32,
    /// Live: the bar line Live counts phrases from
    pub live_start: f64,
    /// Live: the chord playing, the pending one, and the one before now
    pub now: LiveChord,
    pub next: LiveChord,
    pub prev: LiveChord,
    /// Live: where `next` takes over
    pub next_land: f64,
    /// song position = host ppq + offset (after jumps)
    pub jump_offset: f64,
    /// a queued jump: from jumpLand the offset is jumpNewOffset
    pub jump_land: f64,
    pub jump_new_offset: f64,
    /// ... and the timeline entry it goes to
    pub jump_entry: i32,
    /// a forced fill in [fillFrom, fillTo); crash at fillTo
    pub fill_from: f64,
    pub fill_to: f64,
    /// everyone plays this downbeat, then rests one bar
    pub break_at: f64,
    /// band-wide energy nudge, and the one landing at energyLand
    pub energy_offset: i32,
    pub energy_next: i32,
    pub energy_land: f64,
    /// the mod wheel's nudge (when "Mod wheel rides Energy" is on)
    pub wheel_offset: i32,
    pub wheel_next: i32,
    pub wheel_land: f64,
    /// interplay (Band feel)
    pub bass_lock: bool,
    pub fills_yield: bool,
    pub leave_space: bool,
    pub dynamics_together: bool,
    /// Natural feel (groove, phrasing, pushes, band groove); false = Classic
    pub natural: bool,
    /// Tension and release (Band feel): the band reacts to harmonic tension
    pub tension: bool,
    /// a groove audition is playing (nothing it plays is kept)
    pub audition: bool,
    /// A/B Takes: the looped section alternates Take A and Take B per pass. abNowB: B is playing now; from abFlipAt
    /// (where the next loop lands, -1 = none queued) it is the other one.
    pub ab_on: bool,
    pub ab_now_b: bool,
    pub ab_section: char,
    pub ab_a: i32,
    pub ab_b: i32,
    pub ab_flip_at: f64,
    /// the ending: this downbeat, held a bar, then silence
    pub end_at: f64,
    pub bar_len: f64,
}

impl Default for PerformanceState {
    fn default() -> Self {
        Self {
            live: false,
            fill_every: 4,
            live_start: 0.0,
            now: LiveChord::default(),
            next: LiveChord::default(),
            prev: LiveChord::default(),
            next_land: -1.0,
            jump_offset: 0.0,
            jump_land: -1.0,
            jump_new_offset: 0.0,
            jump_entry: -1,
            fill_from: -1.0,
            fill_to: -1.0,
            break_at: -1.0,
            energy_offset: 0,
            energy_next: 0,
            energy_land: -1.0,
            wheel_offset: 0,
            wheel_next: 0,
            wheel_land: -1.0,
            bass_lock: false,
            fills_yield: false,
            leave_space: false,
            dynamics_together: false,
            natural: false,
            tension: false,
            audition: false,
            ab_on: false,
            ab_now_b: false,
            ab_section: '\0',
            ab_a: 1,
            ab_b: 1,
            ab_flip_at: -1.0,
            end_at: -1.0,
            bar_len: 4.0,
        }
    }
}

impl PerformanceState {
    pub fn any(&self) -> bool {
        self.live
            || self.jump_offset != 0.0
            || self.jump_land >= 0.0
            || self.fill_from >= 0.0
            || self.break_at >= 0.0
            || self.energy_offset != 0
            || self.energy_land >= 0.0
            || self.end_at >= 0.0
            || self.wheel_offset != 0
            || self.wheel_land >= 0.0
    }
    pub fn interplay(&self) -> bool {
        self.bass_lock || self.fills_yield || self.leave_space || self.dynamics_together
    }
}

/// One musician as the others hear it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandMember {
    pub chordal: bool,
    pub drums: bool,
    pub channel: i32,
    pub busy: i32,
    pub mean: f32,
    pub seen: f64,
}

impl Default for BandMember {
    fn default() -> Self {
        Self { chordal: false, drums: false, channel: 1, busy: 0, mean: 60.0, seen: -1.0e9 }
    }
}

/// What the musicians tell each other when interplay is on (owned by the Band; fixed size, audio thread only).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandShared {
    /// recent kick note-ons (ppq), a ring
    pub kicks: [f64; 32],
    pub kick_head: i32,
    pub kick_count: i32,
    /// the drummer's fill window
    pub drum_fill_from: f64,
    pub drum_fill_to: f64,
    pub m: [BandMember; 6],
    /// Natural feel: the band's lead groove per 16th, in ms - its drummer's, else its bass's (published by the Band
    /// from its musicians' settings before every block, so playback and exports lean the same way).
    pub drum_groove: [f32; 16],
    pub bass_groove: [f32; 16],
    pub drum_known: bool,
    pub bass_known: bool,
}

impl Default for BandShared {
    fn default() -> Self {
        Self {
            kicks: [0.0; 32],
            kick_head: 0,
            kick_count: 0,
            drum_fill_from: -1.0,
            drum_fill_to: -1.0,
            m: [BandMember::default(); 6],
            drum_groove: [0.0; 16],
            bass_groove: [0.0; 16],
            drum_known: false,
            bass_known: false,
        }
    }
}

impl BandShared {
    /// A stop or a jump: nothing heard yet.
    pub fn clear(&mut self) {
        self.kick_head = 0;
        self.kick_count = 0;
        self.drum_fill_from = -1.0;
        self.drum_fill_to = -1.0;
        self.m = [BandMember::default(); 6];
    }
    pub fn add_kick(&mut self, ppq: f64) {
        let head = self.kick_head.rem_euclid(32) as usize;
        self.kicks[head] = ppq;
        self.kick_head = (self.kick_head + 1).rem_euclid(32);
        self.kick_count = (self.kick_count + 1).min(32);
    }
    fn recent(&self) -> &[f64] {
        let n = self.kick_count.clamp(0, 32) as usize;
        &self.kicks[..n]
    }
    pub fn kick_near(&self, ppq: f64, window: f64) -> bool {
        self.recent().iter().any(|k| (k - ppq).abs() <= window)
    }
    pub fn drummer_around(&self, ppq: f64, bar_len: f64) -> bool {
        self.recent().iter().any(|k| ppq - k < bar_len * 2.0 && k - ppq < bar_len)
    }
}

/// What the UI shows as coming up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerformanceCue {
    pub section: char,
    pub fill: bool,
    pub brk: bool,
    pub ending: bool,
    pub energy: i32,
}

impl Default for PerformanceCue {
    fn default() -> Self {
        Self { section: '\0', fill: false, brk: false, ending: false, energy: 0 }
    }
}

// ---- The Performer ------------------------------------------------------------------------------

/// `Performer::Key::none`.
pub const KEY_NONE: i32 = -1;

#[derive(Clone, Debug)]
pub struct Performer {
    st: PerformanceState,
    held: [bool; 128],
    change_on_beat: bool,
    has_chord: bool,
    live_needs_start: bool,
    was_playing: bool,
    wheel_on: bool,
    loop_id: char,
    loop_queued: bool,
    ab_want: bool,
    ab_want_section: char,
    base: i32,
    key: i32,
    mode: i32,
    requested: char,
    jump_section_id: char,
    /// a jump to one pass of the section (-1: its first)
    requested_entry: i32,
    requested_at: f64,
    last_end: f64,
    last_ppq: f64,
    last_start: f64,
    beat: f64,
    /// a meter change: where bars count from, and where a pending one lands
    grid_origin: f64,
    bar_at: f64,
}

impl Default for Performer {
    fn default() -> Self {
        Self {
            st: PerformanceState::default(),
            held: [false; 128],
            change_on_beat: false,
            has_chord: false,
            live_needs_start: false,
            was_playing: false,
            wheel_on: false,
            loop_id: '\0',
            loop_queued: false,
            ab_want: false,
            ab_want_section: '\0',
            base: 24,
            key: 0,
            mode: 0,
            requested: '\0',
            jump_section_id: '\0',
            requested_entry: -1,
            requested_at: -1.0,
            last_end: 0.0,
            last_ppq: 0.0,
            last_start: -1.0e9,
            beat: 1.0,
            grid_origin: 0.0,
            bar_at: -1.0,
        }
    }
}

impl Performer {
    pub fn set_settings(&mut self, live: bool, on_beat: bool, fill_every: i32, key_base: i32) {
        if live != self.st.live {
            self.st.live = live;
            self.st.next_land = -1.0;
            self.live_needs_start = live;
            if live {
                self.st.jump_land = -1.0; // section jumps are a song-mode thing
            }
            self.st.end_at = -1.0; // switching Live (re)starts the band after an Ending
        }
        self.change_on_beat = on_beat;
        self.st.fill_every = if fill_every == 8 { 8 } else { 4 };
        self.base = key_base;
    }
    pub fn set_key(&mut self, key_root: i32, scale_mode: i32) {
        self.key = key_root.rem_euclid(12); // ((keyRoot % 12) + 12) % 12
        self.mode = if scale_mode != 0 { 1 } else { 0 };
    }
    /// "Mod wheel rides Energy": off = CC1 is ignored (and any wheel nudge goes away).
    pub fn set_mod_wheel(&mut self, on: bool) {
        if self.wheel_on != on {
            self.wheel_on = on;
            self.st.wheel_offset = 0;
            self.st.wheel_next = 0;
            self.st.wheel_land = -1.0;
        }
    }
    pub fn set_interplay(&mut self, bass_lock: bool, fills_yield: bool, leave_space: bool, dynamics: bool) {
        self.st.bass_lock = bass_lock;
        self.st.fills_yield = fills_yield;
        self.st.leave_space = leave_space;
        self.st.dynamics_together = dynamics;
    }
    pub fn set_natural(&mut self, on: bool) {
        self.st.natural = on;
    }
    pub fn set_tension(&mut self, on: bool) {
        self.st.tension = on;
    }
    /// A/B Takes (message thread -> processor -> here once per block).
    pub fn set_ab(&mut self, on: bool, section: char, a: i32, b: i32) {
        self.ab_want = on;
        self.ab_want_section = section;
        self.st.ab_a = a;
        self.st.ab_b = b;
    }
    /// A controller from the keyboard. CC1 (the mod wheel), when it rides Energy: bottom quarter = as
    /// written, then +1, +2, +3; the change lands on the next beat.
    pub fn cc(&mut self, controller: i32, value: i32, ppq: f64) {
        if !self.wheel_on || controller != 1 {
            return;
        }
        let target = if value >= 96 {
            3
        } else if value >= 64 {
            2
        } else if value >= 32 {
            1
        } else {
            0
        };
        let current = if self.st.wheel_land >= 0.0 { self.st.wheel_next } else { self.st.wheel_offset };
        if target == current {
            return;
        }
        self.st.wheel_next = target;
        self.st.wheel_land = ((ppq / self.beat + 1.0e-9).floor() + 1.0) * self.beat;
    }

    /// 0 = no action, 'A'..'H' a section, else one of the performance actions (by key offset).
    pub fn reserved_offset(&self, note: i32) -> i32 {
        if self.base >= 0 && note >= self.base && note <= self.base.saturating_add(12) { note - self.base } else { -1 }
    }

    pub fn note(&mut self, on: bool, note: i32, ppq: f64) {
        if !(0..=127).contains(&note) {
            return;
        }
        let off = self.reserved_offset(note);
        if off >= 0 {
            if on {
                self.perform(off, ppq);
            }
            return;
        }
        self.held[note as usize] = on;
        if !self.st.live {
            return;
        }
        if on {
            self.chord_changed(ppq);
        } else if self.st.next_land >= 0.0 && self.held_chord_keys() >= 3 {
            // legato: the old keys let go after the new ones
            if let Some(c) = self.recognise_held() {
                self.st.next = c;
            }
        }
        // (with fewer than 3 keys left it is a release, and letting go keeps the chord)
    }
    /// All notes off / all sound off from the host or controller: nothing held (the chord stays).
    pub fn release_all(&mut self) {
        self.held.fill(false);
    }

    /// A section click from the UI (message thread -> the processor passes it here on the audio thread).
    /// `entry` -1: the section's first pass.
    pub fn queue_jump(&mut self, section_id: char, entry: i32) {
        self.requested = section_id;
        self.requested_entry = entry;
        self.requested_at = -1.0;
    }
    /// Loop a section (5d): '\0' = off. Not part of the song; a performance state like a jump.
    pub fn set_loop(&mut self, section_id: char) {
        self.loop_id = section_id;
    }

    /// Once per block: lands what is due, resolves queued jumps against the song, resets on a transport jump.
    pub fn advance(&mut self, playing: bool, ppq_start: f64, ppq_end: f64, bar_len: f64, beat_len: f64, song: &SongTimeline) {
        // A seek or loop is a jump back past the last block's start, or forward past where it ended. (Not
        // "short of the estimated end": a tempo change makes every block land a hair short of it.)
        let discontinuity = playing && (!self.was_playing || ppq_start < self.last_start - 1.0e-6 || ppq_start > self.last_end + 0.25);
        // The bar (the meter): from scratch when stopped or after a seek; while playing, a new meter starts at the next
        // bar line, and not inside a Break - at the bar after it. Bars go on counting from there (Live's bar 0 moves).
        let want_bar = if bar_len > 0.0 { bar_len } else { 4.0 };
        if !playing || discontinuity {
            self.st.bar_len = want_bar;
            self.grid_origin = 0.0;
            self.bar_at = -1.0;
        } else if want_bar != self.st.bar_len {
            if self.bar_at < 0.0 {
                let b = self.bar_line_before(ppq_start);
                self.bar_at = if b < ppq_start - 1.0e-9 { b + self.st.bar_len } else { b };
            }
            if self.st.break_at >= 0.0 && self.bar_at < self.st.break_at + self.st.bar_len - 1.0e-9 {
                self.bar_at = self.st.break_at + self.st.bar_len;
            }
            if ppq_end > self.bar_at + 1.0e-9 {
                let bars = ((self.bar_at - self.st.live_start) / self.st.bar_len).round();
                self.st.live_start = self.bar_at - bars * want_bar;
                self.grid_origin = self.bar_at;
                self.st.bar_len = want_bar;
                self.bar_at = -1.0;
            }
        } else {
            self.bar_at = -1.0;
        }
        self.beat = if beat_len > 0.0 { beat_len } else { 1.0 };
        if !playing {
            self.reset_performance(true);
        } else if discontinuity {
            self.reset_performance(false); // a loop keeps the Energy nudge
        }
        self.last_ppq = ppq_start;
        self.last_start = ppq_start;
        if playing && (discontinuity || self.live_needs_start) {
            self.st.live_start = self.bar_line_before(ppq_start);
            if self.live_needs_start
                && !self.has_chord
                && let Some(e0) = song.entries.first()
            {
                self.st.now = LiveChord { slot: e0.slot, bass: e0.bass };
                self.has_chord = true;
            }
            self.live_needs_start = false;
        }
        self.was_playing = playing;
        self.last_end = ppq_end;
        if !playing {
            return;
        }

        // A queued section (click or key): lands on the next bar line after it was asked for. Not after an Ending.
        if self.requested != '\0'
            && !song.entries.is_empty()
            && song.has_ending
            && (ppq_start + self.st.jump_offset) / (self.st.bar_len * 0.5) >= f64::from(song.total_half) - 1.0e-9
        {
            self.requested = '\0'; // the song is over
        }
        if self.requested != '\0' && !self.st.live && self.st.end_at < 0.0 {
            let asked = if self.requested_at >= 0.0 { self.requested_at } else { ppq_start };
            // A pass asked for by name (the Band view) when it is still that section's; else the section's first pass.
            let exact = self.requested_entry >= 0
                && (self.requested_entry as usize) < song.entries.len()
                && song.section_id_of(self.requested_entry as usize) == self.requested;
            let from = if exact { self.requested_entry as usize } else { 0 };
            for i in from..song.entries.len() {
                if song.section_id_of(i) == self.requested {
                    self.st.jump_land = self.next_bar(asked);
                    self.jump_section_id = self.requested;
                    self.st.jump_entry = i as i32;
                    let start_half = song.entries.get(i).map_or(0, |e| e.start_half);
                    self.st.jump_new_offset = f64::from(start_half) * self.st.bar_len * 0.5 - self.st.jump_land;
                    self.loop_queued = false; // this jump is the user's, not the loop's
                    break;
                }
            }
        }
        self.requested = '\0';
        self.requested_entry = -1;

        // Looping a section: at the end of each pass of it, back to the start of that pass; from another section,
        // to the looped one when the current section ends (at once if the transport starts on a bar line). Off: the
        // arrangement simply carries on after the pass it was in.
        if self.loop_id == '\0' && self.loop_queued {
            if self.st.jump_land >= 0.0 {
                self.st.jump_land = -1.0;
                self.st.jump_entry = -1;
            }
            self.loop_queued = false; // off: no more repeats
        }
        if self.loop_id != '\0' && !self.st.live && self.st.end_at < 0.0 && self.st.jump_land < 0.0 && !song.entries.is_empty() {
            let half = self.st.bar_len * 0.5;
            let loc = song_locate(song, ppq_start + self.st.jump_offset, self.st.bar_len, false, 0.0);
            if let (false, Some(e)) = (loc.ended, song.entries.get(loc.idx.max(0) as usize)) {
                let first = (loc.idx - e.chord_index).max(0);
                let first_start = song.entries.get(first as usize).map_or(0, |f| f.start_half);
                let occ_start = loc.start - f64::from(e.start_half - first_start) * half;
                let mut occ_len = 0.0;
                for x in song.entries.iter().skip(first as usize) {
                    if x.arr_pos != e.arr_pos {
                        break;
                    }
                    occ_len += f64::from(x.len_half) * half;
                }
                let here = song.section_id_of(loc.idx.max(0) as usize) == self.loop_id;
                let mut target: i32 = -1;
                if here {
                    target = first;
                } else {
                    for i in 0..song.entries.len() {
                        if song.section_id_of(i) == self.loop_id {
                            target = i as i32;
                            break;
                        }
                    }
                }
                if target >= 0 {
                    let target_start = song.entries.get(target as usize).map_or(0, |t| t.start_half);
                    let on_bar = (ppq_start / self.st.bar_len - (ppq_start / self.st.bar_len).round()).abs() < 1.0e-6;
                    if !here && discontinuity && on_bar {
                        self.st.jump_offset = f64::from(target_start) * half - ppq_start; // starting in the loop
                    } else {
                        self.st.jump_land = occ_start + occ_len - self.st.jump_offset; // where this pass ends (host)
                        self.st.jump_entry = target;
                        self.st.jump_new_offset = f64::from(target_start) * half - self.st.jump_land;
                        self.jump_section_id = self.loop_id;
                        self.loop_queued = true;
                    }
                }
            }
        }

        // Land what is due.
        if self.st.next_land >= 0.0 && ppq_start >= self.st.next_land {
            self.st.prev = self.st.now;
            self.st.now = self.st.next;
            self.st.next_land = -1.0;
        }
        if self.st.jump_land >= 0.0 && ppq_start >= self.st.jump_land {
            if self.st.ab_on && self.jump_section_id == self.st.ab_section {
                self.st.ab_now_b = !self.st.ab_now_b; // A/B: the next pass is the other Take
            }
            self.st.jump_offset = self.st.jump_new_offset;
            self.st.jump_land = -1.0;
            self.st.jump_entry = -1;
            self.loop_queued = false;
        }
        // A/B: starting in the section plays A now and B from the next pass; starting elsewhere, A when it comes.
        if self.ab_want && !self.st.ab_on && !song.entries.is_empty() {
            let loc = song_locate(song, ppq_start + self.st.jump_offset, self.st.bar_len, false, 0.0);
            let i = loc.idx.clamp(0, song.entries.len() as i32 - 1);
            self.st.ab_section = self.ab_want_section;
            self.st.ab_on = true;
            self.st.ab_now_b = song.section_id_of(i as usize) != self.ab_want_section;
        }
        if !self.ab_want {
            self.st.ab_on = false;
        }
        self.st.ab_flip_at =
            if self.st.ab_on && self.st.jump_land >= 0.0 && self.jump_section_id == self.st.ab_section { self.st.jump_land } else { -1.0 };
        if self.st.energy_land >= 0.0 && ppq_start >= self.st.energy_land {
            self.st.energy_offset = self.st.energy_next;
            self.st.energy_land = -1.0;
        }
        if self.st.wheel_land >= 0.0 && ppq_start >= self.st.wheel_land {
            self.st.wheel_offset = self.st.wheel_next;
            self.st.wheel_land = -1.0;
        }
        if self.st.fill_from >= 0.0 && ppq_start >= self.st.fill_to + self.st.bar_len {
            self.st.fill_from = -1.0;
            self.st.fill_to = -1.0;
        }
        if self.st.break_at >= 0.0 && ppq_start >= self.st.break_at + self.st.bar_len {
            self.st.break_at = -1.0;
        }
    }

    pub fn state(&self) -> &PerformanceState {
        &self.st
    }
    pub fn cue(&self) -> PerformanceCue {
        let mut c = PerformanceCue::default();
        if self.st.jump_entry >= 0 {
            c.section = self.jump_section_id;
        }
        c.fill = self.st.fill_from >= 0.0 && self.last_ppq < self.st.fill_to;
        c.brk = self.st.break_at >= 0.0;
        c.ending = self.st.end_at >= 0.0;
        c.energy = if self.st.energy_land >= 0.0 { self.st.energy_next - self.st.energy_offset } else { 0 }; // shown until it lands
        c
    }
    pub fn pending_chord(&self) -> bool {
        self.st.next_land >= 0.0
    }

    fn next_bar(&self, ppq: f64) -> f64 {
        self.bar_line_before(ppq) + self.st.bar_len
    }
    /// The bar line at or before ppq (bars count from where the meter last changed; 0 until then).
    fn bar_line_before(&self, ppq: f64) -> f64 {
        self.grid_origin + ((ppq - self.grid_origin) / self.st.bar_len + 1.0e-9).floor() * self.st.bar_len
    }

    fn held_chord_keys(&self) -> i32 {
        let mut n = 0;
        for (k, h) in self.held.iter().enumerate() {
            if *h && self.reserved_offset(k as i32) < 0 {
                n += 1;
            }
        }
        n
    }

    /// The chord the keys held now make (None if no chord keys are down).
    fn recognise_held(&self) -> Option<LiveChord> {
        let mut pcs = [false; 12];
        let mut lowest = 128;
        for (n, h) in self.held.iter().enumerate() {
            let n = n as i32;
            if *h && self.reserved_offset(n) < 0 {
                pcs[((n - self.key) % 12 + 12).rem_euclid(12) as usize] = true;
                lowest = lowest.min(n);
            }
        }
        if lowest > 127 {
            return None;
        }
        Some(recognise_chord(&pcs, ((lowest - self.key) % 12 + 12) % 12, self.mode))
    }

    fn chord_changed(&mut self, ppq: f64) {
        let Some(c) = self.recognise_held() else { return };
        self.has_chord = true;
        if !self.was_playing {
            // stopped: it is simply the chord
            if c != self.st.now {
                self.st.prev = self.st.now;
                self.st.now = c;
            }
            self.st.next_land = -1.0;
            return;
        }
        // When it lands: the next bar (or beat); a change played just after the line lands at once.
        let unit = if self.change_on_beat { self.beat } else { self.st.bar_len };
        let line_before = self.grid_origin + ((ppq - self.grid_origin) / unit + 1.0e-9).floor() * unit;
        let grace = if self.change_on_beat { 0.25 } else { 0.5 }; // a sixteenth / an eighth note, in any metre
        // A change "at once" waits ~30 ms (at 120 BPM) so a rolled chord's later keys join it before it lands.
        const ROLL: f64 = 0.06;
        let mut land = if ppq - line_before <= grace { ppq + ROLL } else { line_before + unit };
        if self.st.next_land >= 0.0 && self.st.next_land > ppq - 1.0e-9 && self.st.next_land <= ppq + ROLL + 1.0e-9 {
            land = self.st.next_land; // a key of the same roll
        }
        if c == self.st.now && self.st.next_land < 0.0 {
            return; // nothing changes
        }
        self.st.next = c;
        self.st.next_land = land;
    }

    fn perform(&mut self, off: i32, ppq: f64) {
        const SECTION_OF: [i32; 13] = [0, -1, 1, -1, 2, 3, -1, 4, -1, 5, -1, 6, 7];
        let bar = self.bar_line_before(ppq);
        let section = usize::try_from(off).ok().and_then(|o| SECTION_OF.get(o)).copied().unwrap_or(-1);
        if section >= 0 {
            if self.st.live || self.st.end_at >= 0.0 {
                return; // Live replaces the song; nothing after an Ending
            }
            self.requested = char::from(b'A' + section as u8);
            self.requested_at = ppq;
            return;
        }
        if off == 1 {
            // Fill: the rest of this bar (the next bar if pressed in its last 16th)
            let sixteenth = 0.25;
            if ppq > bar + self.st.bar_len - sixteenth {
                self.st.fill_from = bar + self.st.bar_len;
                self.st.fill_to = bar + 2.0 * self.st.bar_len;
            } else {
                self.st.fill_from = ppq;
                self.st.fill_to = bar + self.st.bar_len;
            }
        }
        if off == 3 {
            self.st.break_at = bar + self.st.bar_len;
        }
        if off == 6 || off == 8 {
            let from = if self.st.energy_land >= 0.0 { self.st.energy_next } else { self.st.energy_offset };
            self.st.energy_next = clamp_int(from + if off == 8 { 1 } else { -1 }, -3, 3);
            self.st.energy_land = bar + self.st.bar_len;
        }
        if off == 10 {
            self.st.end_at = bar + self.st.bar_len;
            self.st.jump_land = -1.0;
            self.st.jump_entry = -1;
            self.st.break_at = -1.0;
            self.requested = '\0';
        }
    }

    fn reset_performance(&mut self, stopped: bool) {
        self.st.next_land = -1.0;
        self.st.jump_offset = 0.0;
        self.st.jump_land = -1.0;
        self.st.jump_entry = -1;
        self.st.fill_from = -1.0;
        self.st.fill_to = -1.0;
        self.st.break_at = -1.0;
        self.st.energy_land = -1.0;
        self.st.end_at = -1.0;
        if stopped {
            // Energy resets when the transport stops, not on a loop
            self.st.energy_offset = 0;
            self.st.energy_next = 0;
        }
        if self.st.wheel_land >= 0.0 {
            // the wheel is where the hand left it
            self.st.wheel_offset = self.st.wheel_next;
            self.st.wheel_land = -1.0;
        }
        self.st.energy_next = self.st.energy_offset;
        self.requested = '\0';
        // keys held through a restart are the chord
        if self.st.live
            && let Some(c) = self.recognise_held()
        {
            if c != self.st.now {
                self.st.prev = self.st.now;
                self.st.now = c;
            }
            self.has_chord = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::{LegacySong, build_timeline};

    fn pcs(notes: &[i32]) -> [bool; 12] {
        let mut h = [false; 12];
        for n in notes {
            h[n.rem_euclid(12) as usize] = true;
        }
        h
    }

    #[test]
    fn recognises_chords() {
        assert_eq!(recognise_chord(&pcs(&[0, 4, 7]), 0, 0), LiveChord { slot: 0, bass: -1 });
        assert_eq!(recognise_chord(&pcs(&[9, 0, 4]), 0, 0), LiveChord { slot: 80, bass: 0 }); // Am/C
        assert_eq!(recognise_chord(&pcs(&[7, 11, 2, 5]), 7, 0), LiveChord { slot: 65, bass: -1 }); // G7: the diatonic seventh (flavor 1) beats the dom7 flavor
        assert_eq!(recognise_chord(&[false; 12], 0, 0), LiveChord::default());
    }

    #[test]
    fn live_chord_lands_on_the_next_bar() {
        let t = build_timeline(&LegacySong::default().to_song());
        let mut p = Performer::default();
        p.set_settings(true, false, 4, -1);
        p.advance(true, 0.0, 0.1, 4.0, 1.0, &t);
        assert!(p.state().live && p.state().now == LiveChord { slot: 0, bass: -1 });
        for n in [57, 60, 64] {
            p.note(true, n, 1.2);
        }
        assert_eq!(p.state().next_land, 4.0);
        assert!(p.pending_chord());
        let mut b = 0.1;
        while b < 4.2 {
            p.advance(true, b, b + 0.1, 4.0, 1.0, &t);
            b += 0.1;
        }
        assert_eq!(p.state().now, LiveChord { slot: 80, bass: -1 });
        assert!(!p.pending_chord());
        p.advance(false, b, b, 4.0, 1.0, &t);
        assert!(p.state().live);
    }

    #[test]
    fn band_shared_ring() {
        let mut s = BandShared::default();
        for i in 0..40 {
            s.add_kick(f64::from(i));
        }
        assert_eq!((s.kick_count, s.kick_head), (32, 8));
        assert!(s.kick_near(39.05, 0.1) && !s.kick_near(100.0, 1.0));
        assert!(s.drummer_around(40.0, 4.0));
        s.clear();
        assert!(!s.drummer_around(40.0, 4.0));
    }
}

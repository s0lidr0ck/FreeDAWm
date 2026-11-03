//! Port of `DriverKit.h`: shared plain-function helpers for the instrument drivers.
//!
//! Audio-thread code: no allocation, no virtual dispatch, no boxed closures. A driver builds a [`DriverCtx`] on the
//! stack (Musician::kit_ctx()) and calls the helpers; each helper's body and order are copied from the one driver
//! implementation it was extracted from, so a port is byte-identical in the events it schedules.
//!   * tap_key / send_on_change — a below-range key tap (flag 8192), and "sent only on change" (the organ registrations).
//!   * send_init_keys           — a list of setup keys tapped in turn (Indiginus init keys, Ample setup keys).
//!   * never_send               — the never-send filter (Indiginus neverSend).
//!   * ExprCover / cc_before_notes — a CC a fixed lead before each new note onset, thinned, never re-covering (Indiginus CC11).
//!
//! The C++ `DriverCtx` carries `void* self` and a function pointer; here the scheduler is a [`Sched`] implementor
//! borrowed for the context's life (static dispatch, so no virtual call).

use crate::plucked_voice::StrumNote;

/// Flag: a key outside the playable range (setup / keyswitch); the range filter lets it through.
pub const K_BELOW_RANGE: i32 = 8192;
/// sched flags value meaning "leave Musician::scheduleFlags as it is".
pub const K_KEEP_FLAGS: i32 = -1;

/// The musician's scheduler (the C++ `SchedFn` called with `self`).
pub trait Sched {
    fn sched(&mut self, kind: i32, d1: i32, d2: i32, at: f64, dur_ppq: f32, flags: i32);
}

impl<F: FnMut(i32, i32, i32, f64, f32, i32)> Sched for F {
    fn sched(&mut self, kind: i32, d1: i32, d2: i32, at: f64, dur_ppq: f32, flags: i32) {
        self(kind, d1, d2, at, dur_ppq, flags)
    }
}

pub struct DriverCtx<'a, S: Sched + ?Sized> {
    /// The C++ `self` + `sched` pair.
    pub sink: &'a mut S,
    /// currentPpq
    pub now: f64,
    /// ppqPerMs()
    pub ms: f64,
}

impl<S: Sched + ?Sized> DriverCtx<'_, S> {
    /// flags != K_KEEP_FLAGS: scheduleFlags = flags for this event, then 0 (as the drivers wrote it); K_KEEP_FLAGS: untouched.
    /// (Any other value is a flag set, e.g. Indiginus kInSameFlag, bit 2.)
    pub fn send(&mut self, kind: i32, d1: i32, d2: i32, at: f64, dur_ppq: f32, flags: i32) {
        self.sink.sched(kind, d1, d2, at, dur_ppq, flags);
    }
}

/// A key tap outside the range: `len_ms` long at `at`.
pub fn tap_key<S: Sched + ?Sized>(c: &mut DriverCtx<'_, S>, key: i32, vel: i32, at: f64, len_ms: f64) {
    let ms = c.ms;
    c.send(0, key, vel, at, (len_ms * ms) as f32, K_BELOW_RANGE);
}

/// Sent only on change (organ registrations): taps `key` and records `want` in `sent` when it differs.
pub fn send_on_change<S: Sched + ?Sized>(c: &mut DriverCtx<'_, S>, sent: &mut i32, want: i32, key: i32, vel: i32, at: f64, len_ms: f64) -> bool {
    if want == *sent {
        return false;
    }
    tap_key(c, key, vel, at, len_ms);
    *sent = want;
    true
}

/// Setup keys (-1 skipped) tapped in turn from `at`, `gap_ms` apart, each `len_ms`; returns how many went.
/// `n` keys of `keys` are read (fewer when the slice is shorter).
pub fn send_init_keys<S: Sched + ?Sized>(c: &mut DriverCtx<'_, S>, keys: &[i32], n: i32, vel: i32, at: f64, gap_ms: f64, len_ms: f64) -> i32 {
    let mut k = 0;
    for &key in keys.iter().take(usize::try_from(n).unwrap_or(0)) {
        if key >= 0 {
            let ms = c.ms;
            c.send(0, key, vel, at + gap_ms * ms * f64::from(k), (len_ms * ms) as f32, K_BELOW_RANGE);
            k += 1;
        }
    }
    k
}

/// The never-send filter: is `note` one of `keys` (-1 padded)?
pub fn never_send(keys: &[i32], n: i32, note: i32) -> bool {
    for &key in keys.iter().take(usize::try_from(n).unwrap_or(0)) {
        if key >= 0 && key == note {
            return true;
        }
    }
    false
}

/// What a CC-before-note stream has decided: the last onset covered, the last CC sent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExprCover {
    pub covered: f64,
    pub last: f64,
}

impl Default for ExprCover {
    fn default() -> Self {
        Self { covered: -1.0e18, last: -1.0e18 }
    }
}

impl ExprCover {
    pub fn reset(&mut self) {
        self.covered = -1.0e18;
        self.last = -1.0e18;
    }
    /// A regeneration from `cut`: onsets after it decided again.
    pub fn drop_from(&mut self, cut: f64) {
        self.covered = if self.covered < cut { self.covered } else { cut };
        if self.last >= cut - 1.0e-9 {
            self.last = -1.0e18;
        }
    }
}

/// `cc` = `value`, `lead_ms` before each onset of `ons` (sorted, all past `cover.covered`), at most one per `min_gap`
/// and never before `now` or between a note and the one it follows. `n` onsets of `ons` are read.
pub fn cc_before_notes<S: Sched + ?Sized>(
    c: &mut DriverCtx<'_, S>,
    cover: &mut ExprCover,
    cc: i32,
    value: i32,
    ons: &[f64],
    n: i32,
    lead_ms: f64,
    min_gap: f64,
) {
    for &o in ons.iter().take(usize::try_from(n).unwrap_or(0)) {
        let at = o - lead_ms * c.ms;
        let room = at >= c.now - 1.0e-9 && at - cover.last >= min_gap - 1.0e-9 && cover.covered < at - 0.5 * c.ms;
        if room {
            c.send(1, cc, value, at, 0.0, K_KEEP_FLAGS);
            cover.last = at;
        }
        cover.covered = if cover.covered < o { o } else { cover.covered };
    }
}

// ---- Part 2 (Task 11): held keys, return modes, strums -----------------------------------------------------

/// A key held from `at` for `dur_ppq` (Fiddle "hold", Evolution held keys, MODO hammer / stop / down keys, the
/// Indiginus latch / same return tap). The caller computes `dur_ppq` exactly as its driver did.
/// (C++ default `flags = kBelowRange`.)
pub fn hold_key<S: Sched + ?Sized>(c: &mut DriverCtx<'_, S>, key: i32, vel: i32, at: f64, dur_ppq: f32, flags: i32) {
    c.send(0, key, vel, at, dur_ppq, flags);
}

/// A latching articulation key and its return (The Fiddle `fdReturn*`). Modes as Library.h `fdReturnMode`:
/// 0 unconfirmed (no return tap), 1 same (the latched key again), 2 key (the target's return key), 3 hold (held for
/// its note: `held`, no latch). Presses and returns are 5 ms taps at velocity 100.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReturnMode {
    /// the latched key, -1 none
    pub latched: i32,
    /// its press time
    pub latch_at: f64,
    /// when its return is due (after its note)
    pub due: f64,
    /// the key the last return gave back (a dropped return restores it)
    pub ret_key: i32,
    /// mode 3: the key held for the previous note
    pub held: i32,
}

impl Default for ReturnMode {
    fn default() -> Self {
        Self { latched: -1, latch_at: -1.0, due: -1.0, ret_key: -1, held: -1 }
    }
}

impl ReturnMode {
    pub const UNCONFIRMED: i32 = 0;
    pub const SAME: i32 = 1;
    pub const KEY: i32 = 2;
    pub const HOLD: i32 = 3;

    /// The key that returns `latched_key` in `mode` (`return_key` already octave-mapped), or -1.
    pub fn return_for(mode: i32, latched_key: i32, return_key: i32) -> i32 {
        if mode == Self::SAME {
            latched_key
        } else if mode == Self::KEY {
            return_key
        } else {
            -1
        }
    }

    /// The return tap at `at`; the latch is over.
    pub fn fire<S: Sched + ?Sized>(&mut self, c: &mut DriverCtx<'_, S>, at: f64, mode: i32, return_key: i32) {
        if self.latched < 0 {
            return;
        }
        let k = Self::return_for(mode, self.latched, return_key);
        if k >= 0 {
            tap_key(c, k, 100, at, 5.0);
        }
        self.ret_key = self.latched;
        self.latched = -1;
        self.due = -1.0;
    }

    /// Press `k` at `at` and latch it; its return is due at `due_at`.
    pub fn latch<S: Sched + ?Sized>(&mut self, c: &mut DriverCtx<'_, S>, k: i32, at: f64, due_at: f64) {
        tap_key(c, k, 100, at, 5.0);
        self.latched = k;
        self.latch_at = at;
        self.due = due_at;
    }

    /// Queued keys from `cut` were dropped: was the latch press among them, was the return before it?
    pub fn after_drop(&mut self, press_queued: bool, ret_queued: bool, cut: f64) {
        let mut l = if ret_queued { self.ret_key } else { -1 }; // the return before was not sent: that key is still in effect
        if self.latched >= 0 && !press_queued {
            l = self.latched;
        }
        if l != self.latched || ret_queued {
            self.latched = l;
            self.latch_at = -1.0;
            self.due = if l >= 0 { cut } else { -1.0 };
        }
        if ret_queued {
            self.ret_key = -1;
        }
    }

    pub fn reset(&mut self) {
        self.latched = -1;
        self.held = -1;
        self.ret_key = -1;
        self.latch_at = -1.0;
        self.due = -1.0;
    }
}

/// A strum / roll plan's notes (plucked::StrumNote: pitch, offsetMs, velocity) from `on`, each to `end`
/// (at least `min_dur`), velocity clamped to vel_lo..vel_hi. scheduleFlags left as they are. `n` notes of `sn` are read.
pub fn strum_notes<S: Sched + ?Sized>(c: &mut DriverCtx<'_, S>, sn: &[StrumNote], n: i32, on: f64, end: f64, vel_lo: i32, vel_hi: i32, min_dur: f64) {
    for note in sn.iter().take(usize::try_from(n).unwrap_or(0)) {
        let at = on + note.offset_ms * c.ms;
        let v = note.velocity;
        let d = end - at;
        let vel = if v < vel_lo {
            vel_lo
        } else if v > vel_hi {
            vel_hi
        } else {
            v
        };
        c.send(0, note.pitch, vel, at, (if d > min_dur { d } else { min_dur }) as f32, K_KEEP_FLAGS);
    }
}

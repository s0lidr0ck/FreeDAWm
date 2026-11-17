//! MIDI effects ahead of a track's instrument ([`soundcraft_model::Track::midi_fx`]): an
//! arpeggiator, a chord, a scale (force to scale) and note repeat.
//!
//! **Where they run.** [`sync`] wraps the strip's instrument, once the track has a MIDI effect, in
//! a [`MidiFxHost`]: a plugin that hands everything to the instrument it holds except the notes
//! and controllers, which it collects through the block and, before the instrument processes,
//! passes through the effects in list order. Every route into an instrument (clips, live input,
//! chased controllers, routed MIDI tracks, a module's MIDI, the Band, preview notes) goes through
//! the same plugin calls, so all of them are processed, and a MIDI to Track or External MIDI
//! "instrument" passes on what the effects made. A track that never had a MIDI effect is not
//! wrapped and plays exactly as before; one whose effects are all removed or bypassed passes its
//! events straight through.
//!
//! **Time.** The arpeggiator and note repeat step on the tempo: while the transport plays, on the
//! song's grid (a step on each multiple of the rate, from the transport's position in quarter
//! notes at each block's start); stopped, they run freely from the first key pressed.
//!
//! **Notes left on.** The host counts every note it gave the instrument. A change of the list or
//! of a setting lets go of all of them and plays the keys still held through the new list, so a
//! chord shape changed under a held key sounds at once and nothing hangs. All MIDI Notes Off,
//! a stop and a reset clear every effect.
//!
//! **Audio thread.** Nothing here allocates after [`sync`] has made the host (one boxed value of
//! fixed arrays) and nothing panics: buffers are fixed (full, an event is dropped and counted).

use std::sync::Arc;

use soundcraft_dsp::{LinkSlab, MidiOut, Plugin, PluginEditor, PluginInfo, PluginStateReader, SampleSource, Transport, analysis};
use soundcraft_model::{CHORD_SHAPES, MAX_MIDI_FX, MidiFx, MidiFxKind, SCALES};

/// Events one block holds at each stage.
const CAP: usize = 1024;
/// Keys an arpeggiator or note repeat holds at once.
const HELD: usize = 32;

/// One MIDI 1.0 channel message at a sample offset. `raw`: it came through `midi_raw` (and goes
/// on that way, its channel kept); otherwise through `note_on`/`note_off`/`midi`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ev {
    pub offset: u32,
    pub msg: [u8; 3],
    pub raw: bool,
}

/// A fixed buffer of events.
pub struct EvBuf {
    ev: [Ev; CAP],
    len: usize,
    /// Events that did not fit.
    pub dropped: u64,
}

impl Default for EvBuf {
    fn default() -> Self {
        EvBuf { ev: [Ev::default(); CAP], len: 0, dropped: 0 }
    }
}

impl EvBuf {
    pub fn push(&mut self, e: Ev) {
        match self.ev.get_mut(self.len) {
            Some(slot) => {
                *slot = e;
                self.len += 1;
            }
            None => self.dropped = self.dropped.saturating_add(1),
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn events(&self) -> &[Ev] {
        self.ev.get(..self.len).unwrap_or_default()
    }

    /// Orders by offset, events at one offset kept in the order they came (an insertion sort: the
    /// events mostly arrive in order).
    fn sort(&mut self) {
        let n = self.len.min(CAP);
        for i in 1..n {
            let mut j = i;
            while j > 0 && self.ev[j - 1].offset > self.ev[j].offset {
                self.ev.swap(j - 1, j);
                j -= 1;
            }
        }
    }
}

fn is_on(m: [u8; 3]) -> bool {
    m[0] & 0xF0 == 0x90 && m[2] > 0
}

fn is_off(m: [u8; 3]) -> bool {
    m[0] & 0xF0 == 0x80 || (m[0] & 0xF0 == 0x90 && m[2] == 0)
}

/// Index of a channel and pitch in the 16 × 128 note tables.
fn slot(m: [u8; 3]) -> usize {
    usize::from(m[0] & 0x0F) * 128 + usize::from(m[1] & 0x7F)
}

/// The notes an instant effect (scale, chord) makes of one key: at most this many.
const MAX_FAN: usize = 8;

/// One held key of an arpeggiator or note repeat.
#[derive(Debug, Clone, Copy, Default)]
struct Key {
    status: u8,
    pitch: u8,
    vel: u8,
    raw: bool,
    /// Note repeat: the latest pressure on it (0: none, the key's velocity is used).
    pressure: u8,
    /// Note repeat: it sounds now, until `off_at` (samples from the block's start).
    sounding: bool,
    off_at: f64,
}

/// One effect and its state.
struct Stage {
    fx: MidiFx,
    /// Scale, chord: how many keys hold each output note on (by channel × 128 + pitch).
    counts: [u8; 2048],
    /// Arpeggiator, note repeat: the keys held, in the order they were pressed.
    keys: [Key; HELD],
    n_keys: usize,
    /// Keys physically down (with Latch the list can outlive them).
    down: usize,
    /// Samples from the block's start to the next step, while stepping.
    next: f64,
    stepping: bool,
    /// While the transport plays: the grid index of the next step and of the last one played.
    next_k: i64,
    last_k: i64,
    /// Arpeggiator: the note it sounds (status, pitch, raw) and when it ends.
    cur: Option<(u8, u8, bool)>,
    cur_off: f64,
    /// Arpeggiator: steps since the keys were first pressed, and the random order's state.
    step: u32,
    rng: u32,
}

impl Stage {
    fn new(fx: MidiFx) -> Stage {
        Stage {
            fx: fx.sane(),
            counts: [0; 2048],
            keys: [Key::default(); HELD],
            n_keys: 0,
            down: 0,
            next: 0.0,
            stepping: false,
            next_k: 0,
            last_k: i64::MIN,
            cur: None,
            cur_off: 0.0,
            step: 0,
            rng: 0x9E37_79B9,
        }
    }

    fn clear(&mut self) {
        *self = Stage::new(self.fx);
    }

    /// The notes an instant effect makes of pitch `p` (output pitches, how many).
    fn fan(&self, p: u8) -> ([u8; MAX_FAN], usize) {
        let mut out = [0u8; MAX_FAN];
        let mut n = 0;
        match self.fx.kind {
            MidiFxKind::Scale => {
                let mask = SCALES.get(usize::from(self.fx.scale)).map_or(0xFFF, |s| s.1);
                if let Some(q) = snap(p, self.fx.root, mask, self.fx.drop) {
                    out[0] = q;
                    n = 1;
                }
            }
            MidiFxKind::Chord => {
                let shape = CHORD_SHAPES.get(usize::from(self.fx.chord)).map_or(&[0i8][..], |c| c.1);
                for iv in shape.iter().take(MAX_FAN) {
                    let q = i16::from(p) + i16::from(*iv);
                    if let (Ok(q), Some(o)) = (u8::try_from(q), out.get_mut(n))
                        && q <= 127
                    {
                        *o = q;
                        n += 1;
                    }
                }
            }
            _ => {
                out[0] = p;
                n = 1;
            }
        }
        (out, n)
    }

    /// Scale and chord: each event mapped as it comes.
    fn run_instant(&mut self, input: &[Ev], out: &mut EvBuf) {
        for e in input {
            let kind = e.msg[0] & 0xF0;
            if !matches!(kind, 0x80 | 0x90 | 0xA0) {
                out.push(*e);
                continue;
            }
            let (pitches, n) = self.fan(e.msg[1] & 0x7F);
            for q in pitches.iter().take(n) {
                let msg = [e.msg[0], *q, e.msg[2]];
                let Some(c) = self.counts.get_mut(slot(msg)) else { continue };
                if is_on(msg) {
                    // Two keys making one note: one note-on, its note-off when both are up.
                    if *c == 0 {
                        out.push(Ev { msg, ..*e });
                    }
                    *c = c.saturating_add(1);
                } else if is_off(msg) {
                    if *c > 0 {
                        *c -= 1;
                        if *c == 0 {
                            out.push(Ev { msg, ..*e });
                        }
                    }
                } else if *c > 0 {
                    // Poly pressure on a note that sounds.
                    out.push(Ev { msg, ..*e });
                }
            }
        }
    }

    fn add_key(&mut self, e: &Ev) -> bool {
        let was_empty = self.n_keys == 0;
        let (status, pitch) = (e.msg[0] & 0x0F | 0x90, e.msg[1] & 0x7F);
        if !self.keys.iter().take(self.n_keys).any(|k| k.status == status && k.pitch == pitch)
            && let Some(k) = self.keys.get_mut(self.n_keys)
        {
            *k = Key { status, pitch, vel: e.msg[2], raw: e.raw, ..Key::default() };
            self.n_keys += 1;
        }
        was_empty
    }

    /// Takes the key out of the list; the key as it was, if it was there.
    fn remove_key(&mut self, e: &Ev) -> Option<Key> {
        let (status, pitch) = (e.msg[0] & 0x0F | 0x90, e.msg[1] & 0x7F);
        let at = self.keys.iter().take(self.n_keys).position(|k| k.status == status && k.pitch == pitch)?;
        let k = self.keys.get(at).copied()?;
        for i in at..self.n_keys.saturating_sub(1) {
            self.keys[i] = self.keys[i + 1];
        }
        self.n_keys -= 1;
        Some(k)
    }

    /// The arpeggiator's next note: (status, pitch, velocity, raw).
    fn arp_note(&mut self) -> Option<(u8, u8, u8, bool)> {
        let n = self.n_keys;
        if n == 0 {
            return None;
        }
        // The order: by pitch, or as played.
        let mut order = [0usize; HELD];
        for (i, o) in order.iter_mut().enumerate().take(n) {
            *o = i;
        }
        if self.fx.pattern != 3 {
            let keys = &self.keys;
            order[..n].sort_unstable_by_key(|&i| keys.get(i).map_or((0, 0), |k| (k.pitch, k.status)));
        }
        let total = n * usize::from(self.fx.octaves.max(1));
        let s = self.step as usize;
        let i = match self.fx.pattern {
            1 => total - 1 - s % total,
            2 => {
                let period = (2 * total).saturating_sub(2).max(1);
                let j = s % period;
                if j < total { j } else { period - j }
            }
            4 => {
                // xorshift32
                let mut x = self.rng;
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                self.rng = x;
                x as usize % total
            }
            _ => s % total,
        };
        let k = self.keys.get(*order.get(i % n)?)?;
        let mut pitch = u16::from(k.pitch) + 12 * (i / n) as u16;
        while pitch > 127 {
            pitch -= 12;
        }
        Some((k.status, pitch as u8, k.vel, k.raw))
    }

    /// Arpeggiator and note repeat: the block walked in time, the keys' events and the steps in
    /// order (at one moment: a note ending, then the input, then the step).
    fn run_timed(&mut self, input: &[Ev], out: &mut EvBuf, frames: usize, t: &Transport, spq: f64) {
        let step_q = self.fx.step_quarters();
        let step_s = (step_q * spq).max(1.0);
        let gate_s = (step_s * f64::from(self.fx.gate) / 100.0).max(1.0);
        let end = frames as f64;
        if t.playing {
            // On the song's grid: the first step at or after this block's start (one played at
            // the end of the last block is not played again).
            let mut k = (t.quarters / step_q - 1e-6).ceil() as i64;
            if k == self.last_k {
                k += 1;
            }
            self.next_k = k;
            self.next = ((k as f64 * step_q - t.quarters) * spq).max(0.0);
        } else {
            self.last_k = i64::MIN;
        }
        let repeat = self.fx.kind == MidiFxKind::NoteRepeat;
        let at = |x: f64| x.min(end - 1.0).max(0.0) as u32;
        let mut i = 0;
        loop {
            let ev_at = input.get(i).map(|e| f64::from(e.offset));
            let step_at = (self.stepping && self.n_keys > 0 && self.next < end).then_some(self.next);
            // A note ending: the arpeggiator's, or the earliest of note repeat's.
            let off_at = if repeat {
                self.keys
                    .iter()
                    .take(self.n_keys)
                    .filter(|k| k.sounding)
                    .map(|k| k.off_at)
                    .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x))))
            } else {
                self.cur.map(|_| self.cur_off)
            }
            .filter(|x| *x < end);
            let first = [off_at, ev_at, step_at].into_iter().flatten().fold(f64::INFINITY, f64::min);
            if !first.is_finite() {
                break;
            }
            if off_at == Some(first) {
                if repeat {
                    for k in self.keys.iter_mut().take(self.n_keys) {
                        if k.sounding && k.off_at <= first {
                            k.sounding = false;
                            out.push(Ev { offset: at(first), msg: [k.status & 0x0F | 0x80, k.pitch, 0], raw: k.raw });
                        }
                    }
                } else if let Some((status, pitch, raw)) = self.cur.take() {
                    out.push(Ev { offset: at(first), msg: [status & 0x0F | 0x80, pitch, 0], raw });
                }
                continue;
            }
            if ev_at == Some(first) {
                let Some(e) = input.get(i).copied() else { break };
                i += 1;
                self.key_event(&e, out, t.playing, step_s, gate_s);
                continue;
            }
            // A step.
            let o = at(first);
            if repeat {
                for k in self.keys.iter_mut().take(self.n_keys) {
                    if k.sounding {
                        out.push(Ev { offset: o, msg: [k.status & 0x0F | 0x80, k.pitch, 0], raw: k.raw });
                    }
                    let vel = if k.pressure > 0 { k.pressure } else { k.vel };
                    out.push(Ev { offset: o, msg: [k.status, k.pitch, vel.max(1)], raw: k.raw });
                    k.sounding = true;
                    k.off_at = first + gate_s;
                }
            } else {
                if let Some((status, pitch, raw)) = self.cur.take() {
                    out.push(Ev { offset: o, msg: [status & 0x0F | 0x80, pitch, 0], raw });
                }
                if let Some((status, pitch, vel, raw)) = self.arp_note() {
                    out.push(Ev { offset: o, msg: [status, pitch, vel.max(1)], raw });
                    self.cur = Some((status, pitch, raw));
                    self.cur_off = first + gate_s;
                }
                self.step = self.step.wrapping_add(1);
            }
            if t.playing {
                self.last_k = self.next_k;
                self.next_k += 1;
            }
            self.next += step_s;
        }
        // Into the next block's time.
        self.next -= end;
        self.cur_off -= end;
        for k in self.keys.iter_mut().take(self.n_keys) {
            k.off_at -= end;
        }
        if self.n_keys == 0 {
            self.stepping = false;
        }
    }

    /// A key's event inside [`Stage::run_timed`].
    fn key_event(&mut self, e: &Ev, out: &mut EvBuf, playing: bool, step_s: f64, gate_s: f64) {
        let repeat = self.fx.kind == MidiFxKind::NoteRepeat;
        let t = f64::from(e.offset);
        if is_on(e.msg) {
            if !repeat && self.fx.latch && self.down == 0 {
                // New keys after all were let go replace the latched ones.
                self.n_keys = 0;
            }
            self.down = self.down.saturating_add(1);
            let was_empty = self.add_key(e);
            if was_empty {
                self.step = 0;
                self.stepping = true;
                if !playing {
                    // Stopped: the arpeggiator starts on the key, note repeat a step after it.
                    self.next = if repeat { t + step_s } else { t };
                }
            }
            if repeat {
                // The key sounds at once.
                if let Some(k) = self.keys.iter_mut().take(self.n_keys).find(|k| k.pitch == e.msg[1] & 0x7F && k.status == e.msg[0] & 0x0F | 0x90)
                    && !k.sounding
                {
                    k.sounding = true;
                    k.off_at = t + gate_s;
                    out.push(*e);
                }
            }
        } else if is_off(e.msg) {
            self.down = self.down.saturating_sub(1);
            if repeat {
                if let Some(k) = self.remove_key(e)
                    && k.sounding
                {
                    out.push(Ev { msg: [k.status & 0x0F | 0x80, k.pitch, e.msg[2]], ..*e });
                }
            } else if !self.fx.latch {
                self.remove_key(e);
            }
        } else {
            match e.msg[0] & 0xF0 {
                0xA0 if repeat => {
                    for k in self.keys.iter_mut().take(self.n_keys) {
                        if k.pitch == e.msg[1] & 0x7F && k.status & 0x0F == e.msg[0] & 0x0F {
                            k.pressure = e.msg[2] & 0x7F;
                        }
                    }
                }
                0xD0 if repeat => {
                    for k in self.keys.iter_mut().take(self.n_keys) {
                        if k.status & 0x0F == e.msg[0] & 0x0F {
                            k.pressure = e.msg[1] & 0x7F;
                        }
                    }
                }
                _ => {}
            }
            out.push(*e);
        }
    }

    fn run(&mut self, input: &[Ev], out: &mut EvBuf, frames: usize, t: &Transport, spq: f64) {
        out.clear();
        match self.fx.kind {
            MidiFxKind::Scale | MidiFxKind::Chord => self.run_instant(input, out),
            MidiFxKind::Arpeggiator | MidiFxKind::NoteRepeat => self.run_timed(input, out, frames, t, spq),
        }
    }
}

/// `p` moved into the scale with root `root` (bit per semitone in `mask`): itself when in it, else
/// the nearest note in it (the lower of two equally near), or `None` with `drop`.
pub fn snap(p: u8, root: u8, mask: u16, drop: bool) -> Option<u8> {
    let inside = |x: i16| mask & (1 << (x - i16::from(root)).rem_euclid(12)) != 0;
    let p = i16::from(p & 0x7F);
    if inside(p) {
        return u8::try_from(p).ok();
    }
    if drop {
        return None;
    }
    for d in 1..=11 {
        for q in [p - d, p + d] {
            if (0..=127).contains(&q) && inside(q) {
                return u8::try_from(q).ok();
            }
        }
    }
    None
}

/// The instrument with the MIDI effects in front of it (see the module docs).
pub struct MidiFxHost {
    inner: Box<dyn Plugin>,
    stages: [Stage; MAX_MIDI_FX],
    /// How many of `stages` are the track's list.
    n: usize,
    input: EvBuf,
    bufs: [EvBuf; 2],
    /// Keys held at the input (velocity, 0: up), and which came raw: played again through a
    /// changed list.
    keys: [u8; 2048],
    keys_raw: [u64; 32],
    /// Notes the host left on in the instrument (count per channel × 128 + pitch) and which went
    /// raw: let go when the list changes.
    out: [u8; 2048],
    out_raw: [u64; 32],
    /// Events given while [`MidiFxHost::set_direct`] is on (a clip that already holds the
    /// effects' output): they skip the effects and join their output by offset.
    passthru: EvBuf,
    direct: bool,
    transport: Transport,
    sr: f32,
}

/// A stand-in while the instrument is moved into a host (a zero-sized type: boxing it does not
/// allocate).
struct Hole;

impl Plugin for Hole {
    fn info(&self) -> &'static PluginInfo {
        &crate::track_midi::INFO
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {}
}

fn bit(bits: &[u64; 32], i: usize) -> bool {
    bits.get(i / 64).is_some_and(|w| w & (1u64 << (i % 64)) != 0)
}

fn set_bit(bits: &mut [u64; 32], i: usize, on: bool) {
    if let Some(w) = bits.get_mut(i / 64) {
        if on {
            *w |= 1u64 << (i % 64);
        } else {
            *w &= !(1u64 << (i % 64));
        }
    }
}

impl MidiFxHost {
    /// The host around `inner`, with no effects yet (everything passes straight through).
    pub fn new(inner: Box<dyn Plugin>, sr: f32) -> Box<MidiFxHost> {
        Box::new(MidiFxHost {
            inner,
            stages: std::array::from_fn(|_| Stage::new(MidiFx::new(MidiFxKind::Chord))),
            n: 0,
            input: EvBuf::default(),
            bufs: [EvBuf::default(), EvBuf::default()],
            keys: [0; 2048],
            keys_raw: [0; 32],
            out: [0; 2048],
            out_raw: [0; 32],
            passthru: EvBuf::default(),
            direct: false,
            transport: Transport::default(),
            sr,
        })
    }

    /// The instrument it holds.
    pub fn inner(&self) -> &dyn Plugin {
        &*self.inner
    }

    /// The effects it runs, as last configured.
    pub fn effects(&self) -> impl Iterator<Item = &MidiFx> {
        self.stages.iter().take(self.n).map(|s| &s.fx)
    }

    /// Events dropped because a buffer was full, since it was made.
    pub fn dropped(&self) -> u64 {
        self.input.dropped.saturating_add(self.bufs.iter().map(|b| b.dropped).sum::<u64>())
    }

    fn active(&self) -> bool {
        self.stages.iter().take(self.n).any(|s| !s.fx.bypass)
    }

    /// Takes the track's list. A change lets go of every note the host left on, starts each effect
    /// afresh and plays the keys still held through the new list. No allocation.
    pub fn configure(&mut self, fx: &[MidiFx]) {
        let n = fx.len().min(MAX_MIDI_FX);
        let same = n == self.n && self.stages.iter().zip(fx).all(|(s, f)| s.fx == f.sane());
        if same {
            return;
        }
        self.release_out();
        for (s, f) in self.stages.iter_mut().zip(fx) {
            s.fx = f.sane();
            s.clear();
        }
        self.n = n;
        self.input.clear();
        for i in 0..2048usize {
            let Some(&vel) = self.keys.get(i) else { continue };
            if vel == 0 {
                continue;
            }
            let msg = [0x90 | (i / 128) as u8, (i % 128) as u8, vel];
            self.feed(Ev { offset: 0, msg, raw: bit(&self.keys_raw, i) });
        }
    }

    /// Note-offs now for every note the host left on in the instrument.
    fn release_out(&mut self) {
        for i in 0..2048usize {
            if self.out.get(i).is_some_and(|c| *c > 0) {
                let msg = [0x80 | (i / 128) as u8, (i % 128) as u8, 0];
                let raw = bit(&self.out_raw, i);
                if let Some(c) = self.out.get_mut(i) {
                    *c = 1;
                }
                self.emit(Ev { offset: 0, msg, raw });
            }
        }
        self.out = [0; 2048];
    }

    /// While on, the events given go past the effects to the instrument (in offset order with
    /// what the effects make): the mix turns it on around a clip printed through them
    /// (`Clip::midi_fx_printed`). No allocation.
    pub fn set_direct(&mut self, on: bool) {
        self.direct = on;
    }

    /// An event at the input: kept for the block's effects, or straight to the instrument when no
    /// effect is on.
    fn take(&mut self, e: Ev) {
        if self.direct {
            if self.active() {
                self.passthru.push(e);
            } else {
                self.emit(e);
            }
            return;
        }
        let i = slot(e.msg);
        if is_on(e.msg) {
            if let Some(k) = self.keys.get_mut(i) {
                *k = e.msg[2];
            }
            set_bit(&mut self.keys_raw, i, e.raw);
        } else if is_off(e.msg)
            && let Some(k) = self.keys.get_mut(i)
        {
            *k = 0;
        }
        self.feed(e);
    }

    fn feed(&mut self, e: Ev) {
        if self.active() {
            self.input.push(e);
        } else {
            self.emit(e);
        }
    }

    /// An event to the instrument, counted.
    fn emit(&mut self, e: Ev) {
        let i = slot(e.msg);
        if is_on(e.msg) {
            if let Some(c) = self.out.get_mut(i) {
                *c = c.saturating_add(1);
            }
            set_bit(&mut self.out_raw, i, e.raw);
        } else if is_off(e.msg)
            && let Some(c) = self.out.get_mut(i)
        {
            *c = c.saturating_sub(1);
        }
        let off = e.offset as usize;
        if e.raw {
            self.inner.midi_raw(off, e.msg);
        } else if is_on(e.msg) {
            self.inner.note_on(off, e.msg[1] & 0x7F, e.msg[2]);
        } else if is_off(e.msg) {
            self.inner.note_off(off, e.msg[1] & 0x7F);
        } else {
            self.inner.midi(off, e.msg);
        }
    }

    /// Runs the block's events through the effects and hands the result to the instrument.
    fn run(&mut self, frames: usize) {
        if !self.active() {
            return;
        }
        self.input.sort();
        let tempo = if self.transport.tempo.is_finite() && self.transport.tempo > 1.0 { self.transport.tempo } else { 120.0 };
        let spq = f64::from(self.sr.max(1.0)) * 60.0 / tempo;
        let t = self.transport;
        // The input into the first buffer, then back and forth between the two.
        let [a, b] = &mut self.bufs;
        a.clear();
        for e in self.input.events() {
            a.push(*e);
        }
        self.input.clear();
        let mut in_a = true;
        for s in self.stages.iter_mut().take(self.n).filter(|s| !s.fx.bypass) {
            if in_a {
                s.run(a.events(), b, frames, &t, spq);
            } else {
                s.run(b.events(), a, frames, &t, spq);
            }
            in_a = !in_a;
        }
        let last = if in_a { 0 } else { 1 };
        let len = self.bufs.get(last).map_or(0, |b| b.len);
        let cap = u32::try_from(frames.saturating_sub(1)).unwrap_or(u32::MAX);
        // What went past the effects joins their output in offset order (each list is in order).
        self.passthru.sort();
        let (mut i, mut j) = (0, 0);
        loop {
            let a = self.bufs.get(last).filter(|_| i < len).and_then(|b| b.ev.get(i)).copied();
            let p = self.passthru.events().get(j).copied();
            let mut e = match (a, p) {
                (Some(a), Some(p)) if p.offset < a.offset.min(cap) => {
                    j += 1;
                    p
                }
                (Some(a), _) => {
                    i += 1;
                    a
                }
                (None, Some(p)) => {
                    j += 1;
                    p
                }
                (None, None) => break,
            };
            e.offset = e.offset.min(cap);
            self.emit(e);
        }
        self.passthru.clear();
    }

    /// Forgets every key and note (the instrument is told separately).
    fn clear(&mut self) {
        self.input.clear();
        self.passthru.clear();
        for s in self.stages.iter_mut() {
            s.clear();
        }
        self.keys = [0; 2048];
        self.out = [0; 2048];
    }
}

impl Plugin for MidiFxHost {
    fn info(&self) -> &'static PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.sr = sample_rate;
        self.inner.prepare(sample_rate, max_block, channels);
    }
    fn reset(&mut self) {
        self.clear();
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.inner.set_param(id, value)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.run(frames);
        self.inner.process(io, frames);
    }
    fn process_with_sidechain(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        self.run(frames);
        self.inner.process_with_sidechain(io, frames, key);
    }
    fn latency(&self) -> usize {
        self.inner.latency()
    }
    fn tail_samples(&self) -> usize {
        self.inner.tail_samples()
    }
    fn gain_reduction_db(&self) -> f32 {
        self.inner.gain_reduction_db()
    }
    fn readouts(&self, out: &mut [f32; 8]) {
        self.inner.readouts(out);
    }
    fn analysis(&self) -> Option<Arc<analysis::Analysis>> {
        self.inner.analysis()
    }
    fn set_transport(&mut self, transport: &Transport) {
        self.transport = *transport;
        self.inner.set_transport(transport);
    }
    fn set_offline(&mut self, offline: bool) {
        self.inner.set_offline(offline);
    }
    fn set_lfe_channels(&mut self, mask: u64) {
        self.inner.set_lfe_channels(mask);
    }
    fn set_param_automated(&mut self, id: &str, automated: bool) {
        self.inner.set_param_automated(id, automated);
    }
    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.take(Ev { offset: u32::try_from(offset).unwrap_or(u32::MAX), msg: [0x90, note & 0x7F, velocity.clamp(1, 127)], raw: false });
    }
    fn note_off(&mut self, offset: usize, note: u8) {
        self.take(Ev { offset: u32::try_from(offset).unwrap_or(u32::MAX), msg: [0x80, note & 0x7F, 0], raw: false });
    }
    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.take(Ev { offset: u32::try_from(offset).unwrap_or(u32::MAX), msg, raw: false });
    }
    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.take(Ev { offset: u32::try_from(offset).unwrap_or(u32::MAX), msg, raw: true });
    }
    fn all_notes_off(&mut self) {
        self.clear();
        self.inner.all_notes_off();
    }
    fn midi_out(&mut self, out: &mut MidiOut) {
        self.inner.midi_out(out);
    }
    fn extra_outputs(&self) -> &'static [&'static str] {
        self.inner.extra_outputs()
    }
    fn set_outputs_taken(&mut self, mask: u32) {
        self.inner.set_outputs_taken(mask);
    }
    fn read_output(&mut self, index: usize, out: &mut [Vec<f32>], frames: usize) -> bool {
        self.inner.read_output(index, out, frames)
    }
    fn set_sample(&mut self, sample: Option<Arc<dyn SampleSource>>) -> bool {
        self.inner.set_sample(sample)
    }
    fn set_asset(&mut self, key: &str, asset: Option<Arc<dyn SampleSource>>) -> bool {
        self.inner.set_asset(key, asset)
    }
    fn set_link(&mut self, link: Option<Arc<LinkSlab>>, member: usize) {
        self.inner.set_link(link, member);
    }
    fn action(&mut self, name: &str) -> bool {
        self.inner.action(name)
    }
    fn set_child(&mut self, slot: usize, child: Option<Box<dyn Plugin>>) -> Option<Box<dyn Plugin>> {
        self.inner.set_child(slot, child)
    }
    fn child_mut(&mut self, slot: usize) -> Option<&mut (dyn Plugin + 'static)> {
        self.inner.child_mut(slot)
    }
    fn take_released_asset(&mut self) -> Option<Arc<dyn SampleSource>> {
        self.inner.take_released_asset()
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.inner.save_state()
    }
    fn set_prepared_state(&mut self, data: &[f64]) -> bool {
        self.inner.set_prepared_state(data)
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        self.inner.load_state(data)
    }
    fn open_editor(&mut self) -> Result<(), String> {
        self.inner.open_editor()
    }
    fn close_editor(&mut self) {
        self.inner.close_editor();
    }
    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        self.inner.editor()
    }
    fn state_reader(&mut self) -> Option<Box<dyn PluginStateReader>> {
        self.inner.state_reader()
    }
    fn host_wrapper(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// Brings a strip's instrument in line with the track's MIDI effects: wrapped in a host the first
/// time the track has one (the only allocation, at that change), configured every time. An
/// instrument never wrapped stays as it is while the list is empty.
pub(crate) fn sync(plugin: &mut Box<dyn Plugin>, fx: &[MidiFx], sr: f32) {
    if plugin.host_wrapper().is_none() {
        if fx.is_empty() {
            return;
        }
        let inner = std::mem::replace(plugin, Box::new(Hole));
        *plugin = MidiFxHost::new(inner, sr);
    }
    if let Some(host) = plugin.host_wrapper().and_then(|a| a.downcast_mut::<MidiFxHost>()) {
        host.configure(fx);
    }
}

/// Turns [`MidiFxHost::set_direct`] on or off on `plugin` when it is one (an instrument with
/// MIDI effects); nothing otherwise. No allocation.
pub(crate) fn set_direct(plugin: &mut dyn Plugin, on: bool) {
    if let Some(host) = plugin.host_wrapper().and_then(|a| a.downcast_mut::<MidiFxHost>()) {
        host.set_direct(on);
    }
}

/// Collects what a [`MidiFxHost`] gives its instrument, for [`print`].
struct Sink {
    got: Vec<(u32, [u8; 3])>,
}

impl Plugin for Sink {
    fn info(&self) -> &'static PluginInfo {
        &crate::track_midi::INFO
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {}
    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.midi_raw(offset, [0x90, note & 0x7F, velocity]);
    }
    fn note_off(&mut self, offset: usize, note: u8) {
        self.midi_raw(offset, [0x80, note & 0x7F, 0]);
    }
    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.midi_raw(offset, msg);
    }
    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.got.push((u32::try_from(offset).unwrap_or(u32::MAX), msg));
    }
    fn host_wrapper(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// Offline block for [`print`].
const PRINT_BLOCK: usize = 256;

/// What `fx` make of `events` (timeline samples, MIDI 1.0 channel messages in time order) over
/// `range`, as the mix would give the instrument while the transport plays through it: the
/// arpeggiator and note repeat on the song's grid from the session's tempo map. Channels are
/// kept. Notes still on at the range's end are let go there. The Random pattern rolls afresh, so
/// it need not repeat what was heard. Allocates: off the audio thread (a take is placed with it).
pub fn print(s: &soundcraft_model::Session, fx: &[MidiFx], range: soundcraft_time::Range, events: &[(i64, [u8; 3])]) -> Vec<(i64, [u8; 3])> {
    let mut out = Vec::new();
    if range.is_empty() {
        return out;
    }
    let sr = s.sample_rate.as_f64() as f32;
    let mut host = MidiFxHost::new(Box::new(Sink { got: Vec::new() }), sr);
    host.configure(fx);
    let mut io: Vec<Vec<f32>> = Vec::new();
    let mut next = events.iter().filter(|(at, _)| *at >= range.start && *at < range.end).peekable();
    let mut pos = range.start;
    while pos < range.end {
        let n = usize::try_from(range.end.saturating_sub(pos)).unwrap_or(PRINT_BLOCK).min(PRINT_BLOCK);
        let end = pos.saturating_add(n as i64);
        host.set_transport(&crate::transport_at(s, pos, true, true));
        while let Some((at, msg)) = next.next_if(|(at, _)| *at < end) {
            host.midi_raw(usize::try_from(at.saturating_sub(pos)).unwrap_or(0), *msg);
        }
        host.process(&mut io, n);
        if let Some(sink) = host.inner.host_wrapper().and_then(|a| a.downcast_mut::<Sink>()) {
            for (o, m) in sink.got.drain(..) {
                out.push((pos.saturating_add(i64::from(o)), m));
            }
        }
        pos = end;
    }
    // Whatever is still on ends with the range.
    host.release_out();
    if let Some(sink) = host.inner.host_wrapper().and_then(|a| a.downcast_mut::<Sink>()) {
        out.extend(sink.got.drain(..).map(|(_, m)| (range.end, m)));
    }
    out
}

/// The events a MIDI clip's `seq` gives (notes and controllers on their channels) when it starts
/// at `start`, in time order, as `(sample, message)`: what [`print`] takes. Notes before tick 0
/// (hidden by a trim) and muted notes are left out; each note ends by `end`.
pub fn sequence_events(s: &soundcraft_model::Session, start: i64, end: i64, seq: &soundcraft_midi::Sequence) -> Vec<(i64, [u8; 3])> {
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(start, sr);
    let at = |tick: i64| s.tempo.tick_to_samples(base.saturating_add(tick), sr).max(start);
    let mut out: Vec<(i64, u8, [u8; 3])> = Vec::new();
    for c in seq.ctrls.iter().filter(|c| c.tick >= 0) {
        let mut m = crate::ctrl_message(c.kind, c.value, 0);
        m[0] |= c.channel & 0x0F;
        out.push((at(c.tick), 0, m));
    }
    for n in seq.notes.iter().filter(|n| n.start >= 0 && !n.muted) {
        let ch = n.channel & 0x0F;
        let on = at(n.start);
        if on >= end {
            continue;
        }
        let off = at(n.start.saturating_add(n.length)).min(end).max(on);
        out.push((on, 1, [0x90 | ch, n.pitch & 0x7F, n.velocity.clamp(1, 127)]));
        out.push((off, 0, [0x80 | ch, n.pitch & 0x7F, n.release_velocity & 0x7F]));
    }
    // At one time: note-offs and controllers before note-ons.
    out.sort_by_key(|e| (e.0, e.1));
    out.into_iter().map(|(t, _, m)| (t, m)).collect()
}

#[cfg(test)]
mod tests;

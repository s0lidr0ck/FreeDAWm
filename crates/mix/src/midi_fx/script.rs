//! The Script MIDI effect: a short Rhai script that sees the notes, controllers, pitch bend and
//! pressure going into an instrument and passes, changes, leaves out or adds events (now or later
//! on the tempo), reads the transport, and declares a few named parameters.
//!
//! **Where it runs.** A script engine allocates as it evaluates, so a script never runs on the
//! audio thread. It is compiled, and runs, on one worker thread ([`compile`] starts it). The
//! audio thread's side of a Script effect ([`ScriptStage`]) only copies events into a fixed
//! lock-free ring, wakes the worker, and reads what the worker made from a second ring into a
//! fixed table: no allocation, no lock, no wait (offline renders wait for the worker, bounded).
//!
//! **One block late.** While playing live, what a script makes of a block is heard one block
//! (the device buffer) later than the block it came in: each event comes out exactly the
//! largest block's length after it went in, so timing within the block and between blocks is
//! kept. A worker that falls behind makes its events late (sent as soon as they arrive, and
//! counted). An offline render (a bounce, Record Effects' Output) waits for the worker on each
//! block and is on time, so a bounce is the playback one block earlier.
//!
//! **Limits.** Each call of a script function is cut off after [`MAX_OPERATIONS`] operations and
//! each block after [`BLOCK_OPERATIONS`] or [`BLOCK_TIME`], calls nest at most
//! [`MAX_CALL_LEVELS`] deep, and strings, arrays and maps are bounded. A script that fails (an
//! error, or past a limit) is bypassed from then on: the events of that block and after pass
//! through unchanged, the notes it left on are let go, and the error (with its line) is kept for
//! the editor until the script is compiled again. Scripts get no file, network or process
//! access: no such function is registered, `eval` is turned off and modules do not exist (the
//! `no_module` feature).
//!
//! **What a script sees** (see the examples, [`EXAMPLES`]):
//! - `param(name, min, max, default)` at the top declares a parameter (at most
//!   [`MAX_SCRIPT_PARAMS`]); `param(name)` reads its value.
//! - `fn handle(e)`: each event, an object map: `kind` ("note_on", "note_off", "cc", "bend",
//!   "pressure", "poly_pressure", "program"), `channel` (1-16), `pitch` and `velocity` (notes),
//!   `number` and `value` (cc), `value` (bend -8192..8191, pressure, program), `pitch` and `value`
//!   (poly pressure), `beat` (the song position in quarter notes). Nothing is passed unless the
//!   script sends it: `send(e)` now, `send_after(e, beats)` later. Without `handle`, everything
//!   passes.
//! - `fn process(info)`: once a block, after its events: `info.playing`, `info.beat` (the
//!   block's start: the song position while playing, a free-running count while stopped),
//!   `info.beats` (the block's length) and `info.tempo`; `send_after` there counts from the
//!   block's start.
//! - `fn init()`: when the effect starts or is reset (a stop, a change of the list).
//! - `this`: an object map kept between calls, for the script's own state.
//! - `note_on(pitch, velocity)`, `note_off(pitch)`, `cc(number, value)` make events (channel 1);
//!   `tempo()`, `playing()`, `beat()`; `print(x)` writes a line to the editor's log.

#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use soundcraft_dsp::Transport;
use soundcraft_model::{MAX_SCRIPT_PARAMS, MidiScript, ScriptRuntime};

use super::{Ev, EvBuf, is_off, is_on, slot};

/// Operations one call of a script function may take.
pub const MAX_OPERATIONS: u64 = 100_000;
/// Operations every call in one block may take together.
pub const BLOCK_OPERATIONS: u64 = 400_000;
/// Time every call in one block may take together.
pub const BLOCK_TIME: std::time::Duration = std::time::Duration::from_millis(50);
/// How deep calls nest.
pub const MAX_CALL_LEVELS: usize = 32;
/// Events a script may send in one block (more are left out and counted).
pub const MAX_SENT: usize = 512;
/// Events a script may hold for later (more are left out and counted).
pub const MAX_LATER: usize = 4096;
/// Longest wait for something later (`send_after`), in quarter notes.
pub const MAX_LATER_BEATS: f64 = 64.0;
/// Users of one compiled script at once (live playback, a bounce, a print).
const POOL: usize = 4;
/// Records each ring holds.
const RING: usize = 2048;
/// Events waiting on the audio side.
const PENDING: usize = 512;
/// Longest an offline render waits for the worker on one block.
const OFFLINE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// The built-in examples: (name, script).
pub const EXAMPLES: [(&str, &str); 4] = [
    (
        "Transpose",
        r#"// Moves every note by a number of semitones. A note ends at the pitch it started on,
// even when the amount changes while it is held.
param("Semitones", -24, 24, 0);

fn init() {
    this.made = [];
    this.made.pad(128, -1);
}

fn handle(e) {
    if e.kind == "note_on" {
        let p = e.pitch + param("Semitones").to_int();
        if p < 0 || p > 127 { return; }
        this.made[e.pitch] = p;
        e.pitch = p;
    } else if e.kind == "note_off" || e.kind == "poly_pressure" {
        let p = this.made[e.pitch];
        if p < 0 { return; }
        if e.kind == "note_off" { this.made[e.pitch] = -1; }
        e.pitch = p;
    }
    send(e);
}
"#,
    ),
    (
        "Velocity Curve",
        r#"// Bends how hard notes play: above 0 louder, below 0 softer, then kept between a
// lowest and highest velocity.
param("Curve", -1.0, 1.0, 0.0);
param("Lowest", 1, 127, 1);
param("Highest", 1, 127, 127);

fn handle(e) {
    if e.kind == "note_on" {
        let x = e.velocity.to_float() / 127.0;
        let c = param("Curve");
        let y = if c >= 0.0 { x ** (1.0 - 0.75 * c) } else { x ** (1.0 - 3.0 * c) };
        let lo = param("Lowest");
        let hi = param("Highest");
        let v = (lo + y * (hi - lo)).round().to_int();
        e.velocity = if v < 1 { 1 } else if v > 127 { 127 } else { v };
    }
    send(e);
}
"#,
    ),
    (
        "Arpeggio",
        r#"// Plays the keys held one after another, lowest first, on the beat grid.
param("Steps per beat", 1, 8, 4);
param("Gate", 0.1, 1.0, 0.5);

fn init() {
    this.held = [];
    this.next = 0;
}

fn handle(e) {
    if e.kind == "note_on" {
        this.held.push(e.pitch);
        this.held.sort();
    } else if e.kind == "note_off" {
        let i = this.held.index_of(e.pitch);
        if i >= 0 { this.held.remove(i); }
    } else {
        send(e);
    }
}

fn process(info) {
    if this.held.is_empty() { return; }
    let step = 1.0 / param("Steps per beat");
    let k = (info.beat / step).ceiling();
    while k * step < info.beat + info.beats {
        let at = k * step - info.beat;
        let p = this.held[this.next % this.held.len()];
        send_after(note_on(p, 100), at);
        send_after(note_off(p), at + step * param("Gate"));
        this.next += 1;
        k += 1.0;
    }
}
"#,
    ),
    (
        "Note Echo",
        r#"// Each note is played again a few times, later and softer.
param("Echoes", 1, 8, 3);
param("Time (beats)", 0.125, 2.0, 0.5);
param("Decay", 0.1, 1.0, 0.6);

fn handle(e) {
    send(e);
    if e.kind != "note_on" && e.kind != "note_off" { return; }
    let t = param("Time (beats)");
    let v = if e.kind == "note_on" { e.velocity.to_float() } else { 0.0 };
    for i in 1..=param("Echoes").to_int() {
        let x = e;
        if x.kind == "note_on" {
            v *= param("Decay");
            if v < 1.0 { return; }
            x.velocity = v.round().to_int();
        }
        send_after(x, t * i.to_float());
    }
}
"#,
    ),
];

/// A parameter a script declares.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDecl {
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

/// What the editor shows of a running script.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// Why it was bypassed (its line in the message), once it failed.
    pub error: Option<String>,
    /// The last lines it printed.
    pub log: Vec<String>,
    /// Events that came out later than they should, or were left out (full buffers, limits).
    pub late: u64,
    pub dropped: u64,
}

/// A lock-free ring of fixed records between exactly one writer and one reader (each a
/// different thread): no allocation after it is made, no lock, no wait.
struct Ring {
    slots: Box<[[AtomicU64; 4]]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl Ring {
    fn new() -> Ring {
        Ring { slots: (0..RING).map(|_| std::array::from_fn(|_| AtomicU64::new(0))).collect(), head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    fn push(&self, rec: [u64; 4]) -> bool {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h.wrapping_sub(t) >= self.slots.len() {
            return false;
        }
        let Some(s) = self.slots.get(h % self.slots.len()) else { return false };
        for (w, v) in s.iter().zip(rec) {
            w.store(v, Ordering::Relaxed);
        }
        self.head.store(h.wrapping_add(1), Ordering::Release);
        true
    }

    fn pop(&self) -> Option<[u64; 4]> {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if t == h {
            return None;
        }
        let s = self.slots.get(t % self.slots.len())?;
        let rec = [0, 1, 2, 3].map(|i| s.get(i).map_or(0, |w| w.load(Ordering::Relaxed)));
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        Some(rec)
    }
}

// Record kinds (the top byte of a record's first word).
const K_EVENT: u64 = 1;
const K_BLOCK: u64 = 2;
const K_RESET: u64 = 3;
const K_DONE: u64 = 4;

/// A record's first word: its kind, a flag byte and the user's tag.
fn head(kind: u64, flags: u64, tag: u32) -> u64 {
    kind << 56 | (flags & 0xFF) << 48 | u64::from(tag)
}

fn unhead(w: u64) -> (u64, u64, u32) {
    (w >> 56, (w >> 48) & 0xFF, (w & 0xFFFF_FFFF) as u32)
}

fn pack(msg: [u8; 3]) -> u64 {
    u64::from(msg[0]) | u64::from(msg[1]) << 8 | u64::from(msg[2]) << 16
}

fn unpack(w: u64) -> [u8; 3] {
    [(w & 0xFF) as u8, ((w >> 8) & 0xFF) as u8, ((w >> 16) & 0xFF) as u8]
}

/// One user's lane to the worker.
struct Slot {
    claimed: AtomicBool,
    /// Bumped by each claim: a user's tag carries it, so the worker and the user each ignore what
    /// an earlier user left in the rings.
    generation: AtomicU32,
    to_worker: Ring,
    from_worker: Ring,
    /// The parameters' values (f64 bits), in declaration order.
    params: [AtomicU64; MAX_SCRIPT_PARAMS],
    sample_rate: AtomicU32,
}

/// A compiled script: what a [`MidiScript`]'s runtime holds, shared by the session, the mix's
/// users and the worker (which runs it and frees it once nothing else holds it).
pub struct Program {
    slots: [Slot; POOL],
    params: Vec<ParamDecl>,
    failed: AtomicBool,
    late: AtomicU64,
    dropped: AtomicU64,
    status: Mutex<(Option<String>, Vec<String>)>,
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Program").field("params", &self.params).finish()
    }
}

impl Program {
    fn new(params: Vec<ParamDecl>) -> Program {
        let params_now = |p: &[ParamDecl]| std::array::from_fn(|i| AtomicU64::new(p.get(i).map_or(0.0, |d| d.default).to_bits()));
        Program {
            slots: std::array::from_fn(|_| Slot {
                claimed: AtomicBool::new(false),
                generation: AtomicU32::new(0),
                to_worker: Ring::new(),
                from_worker: Ring::new(),
                params: params_now(&params),
                sample_rate: AtomicU32::new(48_000f32.to_bits()),
            }),
            params,
            failed: AtomicBool::new(false),
            late: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            status: Mutex::new((None, Vec::new())),
        }
    }

    /// The parameters it declares.
    pub fn params(&self) -> &[ParamDecl] {
        &self.params
    }

    /// Whether it failed and is bypassed.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Its error, log and counts, for the editor (not for the audio thread: it locks).
    pub fn status(&self) -> Status {
        let (error, log) = match self.status.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        };
        Status { error, log, late: self.late.load(Ordering::Relaxed), dropped: self.dropped.load(Ordering::Relaxed) }
    }

    fn with_status(&self, f: impl FnOnce(&mut (Option<String>, Vec<String>))) {
        match self.status.lock() {
            Ok(mut g) => f(&mut g),
            Err(p) => f(&mut p.into_inner()),
        }
    }

    fn fail(&self, why: String) {
        self.with_status(|s| {
            if s.0.is_none() {
                s.0 = Some(why);
            }
        });
        self.failed.store(true, Ordering::Release);
    }

    fn log(&self, line: &str) {
        self.with_status(|s| {
            s.1.push(line.chars().take(200).collect());
            if s.1.len() > 8 {
                s.1.remove(0);
            }
        });
    }

    /// The values of `script`'s parameters in declaration order (defaults where unset), bounded.
    pub fn values(&self, script: &MidiScript) -> [f64; MAX_SCRIPT_PARAMS] {
        std::array::from_fn(|i| match self.params.get(i) {
            Some(d) => script.value(&d.name).filter(|v| v.is_finite()).unwrap_or(d.default).clamp(d.min.min(d.max), d.max.max(d.min)),
            None => 0.0,
        })
    }
}

/// The compiled script a runtime holds (a cheap clone of the handle; no allocation).
pub fn program_of(rt: &ScriptRuntime) -> Option<Arc<Program>> {
    rt.program.clone().and_then(|p| p.downcast::<Program>().ok())
}

/// A user's claim on one of a program's slots; let go when dropped. Lock-free.
struct Claim {
    prog: Arc<Program>,
    slot: usize,
    generation: u32,
}

impl Claim {
    fn take(prog: Arc<Program>) -> Option<Claim> {
        let i = prog.slots.iter().position(|s| s.claimed.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok())?;
        let generation = prog.slots.get(i)?.generation.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        Some(Claim { prog, slot: i, generation })
    }

    fn slot(&self) -> Option<&Slot> {
        self.prog.slots.get(self.slot)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(s) = self.slot() {
            s.claimed.store(false, Ordering::Release);
        }
    }
}

/// Wakes the worker (no lock: an atomic read and, when it sleeps, a wake-up call).
fn wake() {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(Some(w)) = worker::WORKER.get() {
        w.thread.unpark();
    }
}

/// The audio thread's side of one Script effect (see the module docs). Fixed storage only.
pub(crate) struct ScriptStage {
    claim: Option<Claim>,
    /// The claim's generation in the high half, the reset count in the low.
    tag: u32,
    epoch: u16,
    /// This user's own clock: samples since it was made.
    clock: i64,
    /// Events from the worker, waiting for their time (due on `clock`), in order.
    pending: [(i64, Ev); PENDING],
    n_pending: usize,
    /// How many of each note (channel × 128 + pitch) the script left on.
    on: [u8; 2048],
    /// The worker has answered up to here (an offline render waits for it).
    done_to: i64,
    /// It saw its program fail and let go of the script's notes.
    gone: bool,
}

impl ScriptStage {
    pub(crate) fn new() -> ScriptStage {
        ScriptStage {
            claim: None,
            tag: 0,
            epoch: 0,
            clock: 0,
            pending: [(0, Ev::default()); PENDING],
            n_pending: 0,
            on: [0; 2048],
            done_to: 0,
            gone: false,
        }
    }

    /// The program it runs.
    pub(crate) fn program(&self) -> Option<&Arc<Program>> {
        self.claim.as_ref().map(|c| &c.prog)
    }

    /// Runs `prog` from now (`None`: passes everything). A new program claims a slot; the old
    /// claim goes (the worker still holds the old program, so nothing is freed here).
    pub(crate) fn set_program(&mut self, prog: Option<&Arc<Program>>, sample_rate: f32) {
        let same = match (self.program(), prog) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.claim = prog.and_then(|p| Claim::take(Arc::clone(p)));
            if prog.is_some()
                && self.claim.is_none()
                && let Some(p) = prog
            {
                p.dropped.fetch_add(1, Ordering::Relaxed);
            }
            self.gone = false;
        }
        if let Some(s) = self.claim.as_ref().and_then(Claim::slot) {
            s.sample_rate.store(sample_rate.to_bits(), Ordering::Relaxed);
        }
        self.reset();
    }

    /// The parameters' values now (no reset).
    pub(crate) fn set_params(&self, values: &[f64; MAX_SCRIPT_PARAMS]) {
        if let Some(s) = self.claim.as_ref().and_then(Claim::slot) {
            for (a, v) in s.params.iter().zip(values) {
                a.store(v.to_bits(), Ordering::Relaxed);
            }
        }
    }

    /// Starts afresh: what is waiting is forgotten and the worker resets the script (`init`).
    pub(crate) fn reset(&mut self) {
        self.n_pending = 0;
        self.on = [0; 2048];
        self.epoch = self.epoch.wrapping_add(1);
        let Some(c) = &self.claim else { return };
        self.tag = c.generation << 16 | u32::from(self.epoch);
        if let Some(s) = c.slot() {
            s.to_worker.push([head(K_RESET, 0, self.tag), self.clock as u64, 0, 0]);
        }
        self.done_to = self.clock;
        wake();
    }

    fn note(&mut self, msg: [u8; 3]) {
        if let Some(c) = self.on.get_mut(slot(msg)) {
            if is_on(msg) {
                *c = c.saturating_add(1);
            } else if is_off(msg) {
                *c = c.saturating_sub(1);
            }
        }
    }

    /// One block: `input` (in offset order) to the worker, and what is due in this block from it
    /// into `out` (in order). `lat`: how late it comes out live (the largest block).
    pub(crate) fn run(&mut self, input: &[Ev], out: &mut EvBuf, frames: usize, t: &Transport, sample_rate: f32, lat: usize, offline: bool) {
        out.clear();
        let n = i64::try_from(frames).unwrap_or(i64::MAX);
        // (A second handle on the program, so the claim is not borrowed: the claim holds it, so
        // dropping this one frees nothing.)
        let Some((prog, idx)) = self.claim.as_ref().map(|c| (Arc::clone(&c.prog), c.slot)) else {
            input.iter().for_each(|e| out.push(*e));
            self.clock = self.clock.saturating_add(n);
            return;
        };
        if prog.failed() {
            if !self.gone {
                // The notes it left on end now; then everything passes.
                self.gone = true;
                self.n_pending = 0;
                for i in 0..2048usize {
                    if self.on.get(i).is_some_and(|c| *c > 0) {
                        out.push(Ev { offset: 0, msg: [0x80 | (i / 128) as u8, (i % 128) as u8, 0], raw: true });
                    }
                }
                self.on = [0; 2048];
            }
            input.iter().for_each(|e| out.push(*e));
            self.clock = self.clock.saturating_add(n);
            return;
        }
        let Some(s) = prog.slots.get(idx) else { return };
        s.sample_rate.store(sample_rate.to_bits(), Ordering::Relaxed);
        let tag = self.tag;
        let mut sent = true;
        for e in input {
            let at = self.clock.saturating_add(i64::from(e.offset));
            if !s.to_worker.push([head(K_EVENT, u64::from(e.raw), tag), at as u64, pack(e.msg), 0]) {
                prog.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        let tempo = if t.tempo.is_finite() && t.tempo > 1.0 { t.tempo } else { 120.0 };
        let block = [
            head(K_BLOCK, u64::from(t.playing), tag),
            self.clock as u64,
            (frames as u64) << 32 | u64::from((tempo as f32).to_bits()),
            t.quarters.to_bits(),
        ];
        if !s.to_worker.push(block) {
            sent = false;
            prog.dropped.fetch_add(input.len() as u64, Ordering::Relaxed);
        }
        wake();
        let end = self.clock.saturating_add(n);
        if offline && sent {
            // An offline render waits for the worker's answer to this block.
            let start = std::time::Instant::now();
            while self.done_to < end && start.elapsed() < OFFLINE_WAIT {
                if !self.collect(s, tag, &prog.dropped) {
                    std::thread::sleep(std::time::Duration::from_micros(20));
                    wake();
                }
            }
        }
        self.collect(s, tag, &prog.dropped);
        // What is due in this block: live, `lat` after it went in.
        let delay = if offline { 0 } else { i64::try_from(lat).unwrap_or(0) };
        let mut k = 0;
        while let Some(&(due, e)) = self.pending.get(k).filter(|_| k < self.n_pending) {
            let at = due.saturating_add(delay);
            if at >= end {
                break;
            }
            if at < self.clock {
                prog.late.fetch_add(1, Ordering::Relaxed);
            }
            let ev = Ev { offset: u32::try_from(at.saturating_sub(self.clock).max(0)).unwrap_or(0), ..e };
            self.note(ev.msg);
            out.push(ev);
            k += 1;
        }
        if k > 0 {
            self.pending.copy_within(k..self.n_pending, 0);
            self.n_pending -= k;
        }
        self.clock = end;
    }

    /// Takes what the worker sent; whether there was anything.
    fn collect(&mut self, s: &Slot, tag: u32, dropped: &AtomicU64) -> bool {
        let mut any = false;
        while let Some(rec) = s.from_worker.pop() {
            any = true;
            let (kind, flags, t) = unhead(rec[0]);
            if t != tag {
                continue;
            }
            let at = rec[1] as i64;
            match kind {
                K_EVENT => {
                    let e = Ev { offset: 0, msg: unpack(rec[2]), raw: flags & 1 != 0 };
                    if self.n_pending >= PENDING {
                        dropped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    // In order of time (an insertion: they mostly come in order).
                    let mut i = self.n_pending;
                    while i > 0 && self.pending.get(i - 1).is_some_and(|p| p.0 > at) {
                        i -= 1;
                    }
                    self.pending.copy_within(i..self.n_pending, i + 1);
                    if let Some(p) = self.pending.get_mut(i) {
                        *p = (at, e);
                    }
                    self.n_pending += 1;
                }
                K_DONE => self.done_to = self.done_to.max(at),
                _ => {}
            }
        }
        any
    }
}

/// Compiles `source` on the worker (starting it the first time) and waits for it: the program,
/// or the error with its line. Control thread only.
pub fn compile(source: &str) -> Result<Arc<Program>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        worker::compile(source)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = source;
        Err("Scripts do not run in the browser build".into())
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod worker {
    //! The worker thread: compiles scripts and runs them (see the module docs).

    use super::*;
    use rhai::{AST, CallFnOptions, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Scope};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    pub(super) struct Worker {
        pub(super) thread: std::thread::Thread,
        tx: mpsc::Sender<Request>,
    }

    pub(super) static WORKER: std::sync::OnceLock<Option<Worker>> = std::sync::OnceLock::new();

    struct Request {
        source: String,
        reply: mpsc::Sender<Result<Arc<Program>, String>>,
    }

    pub(super) fn compile(source: &str) -> Result<Arc<Program>, String> {
        let w = WORKER.get_or_init(start).as_ref().ok_or("The script worker could not start")?;
        let (tx, rx) = mpsc::channel();
        w.tx.send(Request { source: source.to_string(), reply: tx }).map_err(|_| "The script worker has stopped".to_string())?;
        w.thread.unpark();
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "The script worker did not answer".to_string())?
    }

    fn start() -> Option<Worker> {
        let (tx, rx) = mpsc::channel::<Request>();
        let handle = std::thread::Builder::new().name("midi-script".into()).spawn(move || run(rx)).ok()?;
        Some(Worker { thread: handle.thread().clone(), tx })
    }

    /// What the script's functions read and write while it runs.
    #[derive(Default)]
    struct Ctx {
        compiling: bool,
        decls: Vec<ParamDecl>,
        values: [f64; MAX_SCRIPT_PARAMS],
        /// Events sent: (due on the user's clock, message, raw).
        out: Vec<(i64, [u8; 3], bool)>,
        /// What `send` counts from, and samples per quarter note.
        now: i64,
        spq: f64,
        tempo: f64,
        playing: bool,
        beat: f64,
        dropped: u64,
        /// Operations the block has taken and when it began.
        ops: u64,
        began: Option<Instant>,
        last_ops: u64,
    }

    type Shared = Rc<RefCell<Ctx>>;

    fn num(d: &Dynamic) -> Option<f64> {
        d.as_float().ok().or_else(|| d.as_int().ok().map(|i| i as f64))
    }

    /// An event as a script sees it.
    fn to_map(msg: [u8; 3], raw: bool, beat: f64) -> Option<Map> {
        let mut m = Map::new();
        let int = |v: u8| Dynamic::from_int(i64::from(v & 0x7F));
        let kind = match msg[0] & 0xF0 {
            0x90 if msg[2] > 0 => "note_on",
            0x80 | 0x90 => "note_off",
            0xA0 => "poly_pressure",
            0xB0 => "cc",
            0xC0 => "program",
            0xD0 => "pressure",
            0xE0 => "bend",
            _ => return None,
        };
        m.insert("kind".into(), kind.into());
        m.insert("channel".into(), Dynamic::from_int(i64::from(msg[0] & 0x0F) + 1));
        match kind {
            "note_on" | "note_off" => {
                m.insert("pitch".into(), int(msg[1]));
                m.insert("velocity".into(), int(msg[2]));
            }
            "poly_pressure" => {
                m.insert("pitch".into(), int(msg[1]));
                m.insert("value".into(), int(msg[2]));
            }
            "cc" => {
                m.insert("number".into(), int(msg[1]));
                m.insert("value".into(), int(msg[2]));
            }
            "program" | "pressure" => {
                m.insert("value".into(), int(msg[1]));
            }
            _ => {
                let v = (i64::from(msg[2] & 0x7F) << 7 | i64::from(msg[1] & 0x7F)) - 8192;
                m.insert("value".into(), Dynamic::from_int(v));
            }
        }
        m.insert("beat".into(), Dynamic::from_float(beat));
        m.insert("raw".into(), Dynamic::from_bool(raw));
        Some(m)
    }

    /// A script's event as MIDI (`None`: not a known kind, or a note out of range).
    fn from_map(m: &Map) -> Option<([u8; 3], bool)> {
        let get = |k: &str| m.get(k).and_then(num).map(|v| v.round());
        let byte = |k: &str| get(k).filter(|v| (0.0..=127.0).contains(v)).map(|v| v as u8);
        let clamp7 = |k: &str| get(k).unwrap_or(0.0).clamp(0.0, 127.0) as u8;
        let ch = (get("channel").unwrap_or(1.0).clamp(1.0, 16.0) as u8) - 1;
        let kind = m.get("kind")?.clone().into_immutable_string().ok()?;
        let msg = match kind.as_str() {
            "note_on" => [0x90 | ch, byte("pitch")?, clamp7("velocity").max(1)],
            "note_off" => [0x80 | ch, byte("pitch")?, clamp7("velocity")],
            "poly_pressure" => [0xA0 | ch, byte("pitch")?, clamp7("value")],
            "cc" => [0xB0 | ch, byte("number")?, clamp7("value")],
            "program" => [0xC0 | ch, clamp7("value"), 0],
            "pressure" => [0xD0 | ch, clamp7("value"), 0],
            "bend" => {
                let v = (get("value").unwrap_or(0.0).clamp(-8192.0, 8191.0) as i64 + 8192) as u16;
                [0xE0 | ch, (v & 0x7F) as u8, ((v >> 7) & 0x7F) as u8]
            }
            _ => return None,
        };
        let raw = m.get("raw").and_then(|r| r.as_bool().ok()).unwrap_or(false) || ch != 0;
        Some((msg, raw))
    }

    fn send(cx: &Shared, m: &Map, beats: f64) -> Result<(), Box<EvalAltResult>> {
        let mut c = cx.borrow_mut();
        if c.compiling {
            return Ok(());
        }
        let Some((msg, raw)) = from_map(m) else {
            c.dropped += 1;
            return Ok(());
        };
        if c.out.len() >= MAX_SENT {
            c.dropped += 1;
            return Ok(());
        }
        let later = if beats.is_finite() { beats.clamp(0.0, MAX_LATER_BEATS) } else { 0.0 };
        let due = c.now.saturating_add((later * c.spq).round() as i64);
        c.out.push((due, msg, raw));
        Ok(())
    }

    fn engine(cx: &Shared, prog_log: Rc<RefCell<Vec<String>>>) -> Engine {
        let mut e = Engine::new();
        e.set_max_operations(MAX_OPERATIONS);
        e.set_max_call_levels(MAX_CALL_LEVELS);
        e.set_max_expr_depths(64, 32);
        e.set_max_string_size(4096);
        e.set_max_array_size(1024);
        e.set_max_map_size(256);
        e.set_max_variables(256);
        e.set_max_functions(64);
        e.disable_symbol("eval");
        let log = Rc::clone(&prog_log);
        e.on_print(move |s| log.borrow_mut().push(s.to_string()));
        let log = prog_log;
        e.on_debug(move |s, _, _| log.borrow_mut().push(s.to_string()));
        let c = Rc::clone(cx);
        e.on_progress(move |ops| {
            let mut c = c.borrow_mut();
            let step = ops.saturating_sub(c.last_ops);
            c.last_ops = ops;
            c.ops = c.ops.saturating_add(step);
            if c.ops > BLOCK_OPERATIONS {
                return Some("the block's operations ran out".into());
            }
            if ops % 1024 == 0 && c.began.is_some_and(|b| b.elapsed() > BLOCK_TIME) {
                return Some("the block's time ran out".into());
            }
            None
        });
        let c = Rc::clone(cx);
        e.register_fn("param", move |name: ImmutableString, min: Dynamic, max: Dynamic, default: Dynamic| -> Result<(), Box<EvalAltResult>> {
            let mut c = c.borrow_mut();
            if !c.compiling {
                return Ok(());
            }
            let (Some(lo), Some(hi), Some(d)) = (num(&min), num(&max), num(&default)) else {
                return Err(format!("param(\"{name}\", min, max, default) takes numbers").into());
            };
            if !(lo.is_finite() && hi.is_finite() && d.is_finite()) || lo >= hi {
                return Err(format!("param(\"{name}\"): min must be below max").into());
            }
            if c.decls.iter().any(|p| p.name == name.as_str()) {
                return Err(format!("param(\"{name}\") is declared twice").into());
            }
            if c.decls.len() >= MAX_SCRIPT_PARAMS {
                return Err(format!("a script declares at most {MAX_SCRIPT_PARAMS} parameters").into());
            }
            c.decls.push(ParamDecl { name: name.chars().take(40).collect(), min: lo, max: hi, default: d.clamp(lo, hi) });
            Ok(())
        });
        let c = Rc::clone(cx);
        e.register_fn("param", move |name: ImmutableString| -> Result<f64, Box<EvalAltResult>> {
            let c = c.borrow();
            match c.decls.iter().position(|p| p.name == name.as_str()) {
                Some(i) if c.compiling => Ok(c.decls.get(i).map_or(0.0, |d| d.default)),
                Some(i) => Ok(c.values.get(i).copied().unwrap_or(0.0)),
                None => Err(format!("no parameter named \"{name}\"").into()),
            }
        });
        let c = Rc::clone(cx);
        e.register_fn("send", move |m: Map| send(&c, &m, 0.0));
        let c = Rc::clone(cx);
        e.register_fn("send_after", move |m: Map, beats: Dynamic| send(&c, &m, num(&beats).unwrap_or(0.0)));
        e.register_fn("note_on", |p: i64, v: i64| -> Map {
            let mut m = Map::new();
            m.insert("kind".into(), "note_on".into());
            m.insert("channel".into(), Dynamic::from_int(1));
            m.insert("pitch".into(), Dynamic::from_int(p));
            m.insert("velocity".into(), Dynamic::from_int(v));
            m
        });
        e.register_fn("note_off", |p: i64| -> Map {
            let mut m = Map::new();
            m.insert("kind".into(), "note_off".into());
            m.insert("channel".into(), Dynamic::from_int(1));
            m.insert("pitch".into(), Dynamic::from_int(p));
            m.insert("velocity".into(), Dynamic::from_int(0));
            m
        });
        e.register_fn("cc", |n: i64, v: i64| -> Map {
            let mut m = Map::new();
            m.insert("kind".into(), "cc".into());
            m.insert("channel".into(), Dynamic::from_int(1));
            m.insert("number".into(), Dynamic::from_int(n));
            m.insert("value".into(), Dynamic::from_int(v));
            m
        });
        let c = Rc::clone(cx);
        e.register_fn("tempo", move || c.borrow().tempo);
        let c = Rc::clone(cx);
        e.register_fn("playing", move || c.borrow().playing);
        let c = Rc::clone(cx);
        e.register_fn("beat", move || c.borrow().beat);
        e
    }

    /// One user's state on the worker.
    struct SlotRun {
        tag: Option<u32>,
        this: Dynamic,
        events: Vec<(i64, [u8; 3], bool)>,
        /// Events sent for later, in order of time.
        later: Vec<(i64, [u8; 3], bool)>,
        /// The free-running beat while stopped.
        free_beat: f64,
    }

    impl SlotRun {
        fn new() -> SlotRun {
            SlotRun { tag: None, this: Dynamic::from_map(Map::new()), events: Vec::new(), later: Vec::new(), free_beat: 0.0 }
        }
    }

    struct Run {
        prog: Arc<Program>,
        engine: Engine,
        ast: AST,
        cx: Shared,
        log: Rc<RefCell<Vec<String>>>,
        has: (bool, bool, bool),
        slots: Vec<SlotRun>,
    }

    fn compile_one(source: &str) -> Result<Run, String> {
        let cx: Shared = Rc::new(RefCell::new(Ctx { compiling: true, spq: 24_000.0, tempo: 120.0, ..Ctx::default() }));
        let log = Rc::new(RefCell::new(Vec::new()));
        let engine = engine(&cx, Rc::clone(&log));
        let ast = engine.compile(source).map_err(|e| e.to_string())?;
        let has = |name: &str, n: usize| ast.iter_functions().any(|f| f.name == name && f.params.len() == n);
        let has = (has("handle", 1), has("process", 1), has("init", 0));
        // The top runs once, here: it declares the parameters.
        cx.borrow_mut().began = Some(Instant::now());
        let mut scope = Scope::new();
        engine.run_ast_with_scope(&mut scope, &ast).map_err(|e| e.to_string())?;
        let decls = {
            let mut c = cx.borrow_mut();
            c.compiling = false;
            c.ops = 0;
            c.last_ops = 0;
            std::mem::take(&mut c.decls)
        };
        let prog = Arc::new(Program::new(decls.clone()));
        cx.borrow_mut().decls = decls;
        for line in log.borrow_mut().drain(..) {
            prog.log(&line);
        }
        Ok(Run { prog, engine, ast, cx, log, has, slots: (0..POOL).map(|_| SlotRun::new()).collect() })
    }

    impl Run {
        /// Calls `name` with `this` bound to the user's state; an error fails the program.
        fn call(&self, this: &mut Dynamic, name: &str, args: impl rhai::FuncArgs) -> bool {
            if self.prog.failed() {
                return false;
            }
            {
                let mut c = self.cx.borrow_mut();
                c.last_ops = 0;
            }
            let mut scope = Scope::new();
            let opts = CallFnOptions::new().eval_ast(false).rewind_scope(true).bind_this_ptr(this);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.engine.call_fn_with_options::<Dynamic>(opts, &mut scope, &self.ast, name, args)
            }));
            for line in self.log.borrow_mut().drain(..) {
                self.prog.log(&line);
            }
            match r {
                Ok(Ok(_)) => true,
                Ok(Err(e)) => {
                    self.prog.fail(format!("{name}: {e}"));
                    false
                }
                Err(_) => {
                    self.prog.fail(format!("{name}: the script engine failed"));
                    false
                }
            }
        }

        fn reset(&mut self, i: usize, tag: u32) {
            let mut sr = SlotRun::new();
            sr.tag = Some(tag);
            if self.has.2 {
                self.begin_block(i);
                let mut this = std::mem::take(&mut sr.this);
                self.call(&mut this, "init", ());
                sr.this = this;
            }
            if let Some(s) = self.slots.get_mut(i) {
                *s = sr;
            }
        }

        fn begin_block(&self, i: usize) {
            let mut c = self.cx.borrow_mut();
            c.ops = 0;
            c.began = Some(Instant::now());
            c.out.clear();
            if let Some(s) = self.prog.slots.get(i) {
                for (v, a) in c.values.iter_mut().zip(&s.params) {
                    *v = f64::from_bits(a.load(Ordering::Relaxed));
                }
            }
        }

        /// Takes what user `i` sent; whether there was anything.
        fn poll(&mut self, i: usize) -> bool {
            let prog = Arc::clone(&self.prog);
            let Some(slot) = prog.slots.get(i) else { return false };
            let mut any = false;
            while let Some(rec) = slot.to_worker.pop() {
                any = true;
                let (kind, flags, tag) = unhead(rec[0]);
                if kind == K_RESET || self.slots.get(i).is_some_and(|s| s.tag != Some(tag)) {
                    self.reset(i, tag);
                    if kind == K_RESET {
                        continue;
                    }
                }
                match kind {
                    K_EVENT => {
                        if let Some(s) = self.slots.get_mut(i) {
                            if s.events.len() < MAX_LATER {
                                s.events.push((rec[1] as i64, unpack(rec[2]), flags & 1 != 0));
                            } else {
                                prog.dropped.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    K_BLOCK => self.block(i, slot, tag, rec, flags & 1 != 0),
                    _ => {}
                }
            }
            any
        }

        fn block(&mut self, i: usize, slot: &Slot, tag: u32, rec: [u64; 4], playing: bool) {
            let Some(mut sr) = self.slots.get_mut(i).map(|s| std::mem::replace(s, SlotRun::new())) else { return };
            let clock = rec[1] as i64;
            let frames = (rec[2] >> 32) as i64;
            let tempo = f64::from(f32::from_bits((rec[2] & 0xFFFF_FFFF) as u32)).max(1.0);
            let rate = f64::from(f32::from_bits(slot.sample_rate.load(Ordering::Relaxed))).max(1.0);
            let spq = rate * 60.0 / tempo;
            let quarters = f64::from_bits(rec[3]);
            let beat0 = if playing { quarters } else { sr.free_beat };
            let beats = frames as f64 / spq;
            if !playing {
                sr.free_beat += beats;
            }
            self.begin_block(i);
            {
                let mut c = self.cx.borrow_mut();
                c.spq = spq;
                c.tempo = tempo;
                c.playing = playing;
            }
            let events = std::mem::take(&mut sr.events);
            let mut out: Vec<(i64, [u8; 3], bool)> = Vec::new();
            for (at, msg, raw) in &events {
                let beat = beat0 + (at - clock) as f64 / spq;
                let map = if self.has.0 && !self.prog.failed() { to_map(*msg, *raw, beat) } else { None };
                let Some(map) = map else {
                    out.push((*at, *msg, *raw));
                    continue;
                };
                {
                    let mut c = self.cx.borrow_mut();
                    c.now = *at;
                    c.beat = beat;
                }
                let mut this = std::mem::take(&mut sr.this);
                let ok = self.call(&mut this, "handle", (Dynamic::from_map(map),));
                sr.this = this;
                if !ok {
                    // Failed: this event passes unchanged (the rest of the block too).
                    out.push((*at, *msg, *raw));
                }
            }
            sr.events = events;
            sr.events.clear();
            if self.has.1 && !self.prog.failed() {
                {
                    let mut c = self.cx.borrow_mut();
                    c.now = clock;
                    c.beat = beat0;
                }
                let mut info = Map::new();
                info.insert("playing".into(), Dynamic::from_bool(playing));
                info.insert("beat".into(), Dynamic::from_float(beat0));
                info.insert("beats".into(), Dynamic::from_float(beats));
                info.insert("tempo".into(), Dynamic::from_float(tempo));
                let mut this = std::mem::take(&mut sr.this);
                self.call(&mut this, "process", (Dynamic::from_map(info),));
                sr.this = this;
            }
            {
                let mut c = self.cx.borrow_mut();
                out.append(&mut c.out);
                if c.dropped > 0 {
                    self.prog.dropped.fetch_add(std::mem::take(&mut c.dropped), Ordering::Relaxed);
                }
            }
            // Into the waiting list, in order of time; what falls in this block goes out.
            for e in out {
                if sr.later.len() >= MAX_LATER {
                    self.prog.dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                // At one time a note-off goes before a note-on (an echo of the same key).
                let key = |x: &(i64, [u8; 3], bool)| (x.0, is_on(x.1));
                let k = sr.later.partition_point(|x| key(x) <= key(&e));
                sr.later.insert(k, e);
            }
            let end = clock.saturating_add(frames);
            let n = sr.later.partition_point(|x| x.0 < end);
            for (at, msg, raw) in sr.later.drain(..n) {
                if !slot.from_worker.push([head(K_EVENT, u64::from(raw), tag), at as u64, pack(msg), 0]) {
                    self.prog.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            slot.from_worker.push([head(K_DONE, 0, tag), end as u64, 0, 0]);
            if let Some(s) = self.slots.get_mut(i) {
                *s = sr;
            }
        }
    }

    fn run(rx: mpsc::Receiver<Request>) {
        let mut runs: Vec<Run> = Vec::new();
        loop {
            let mut busy = false;
            loop {
                match rx.try_recv() {
                    Ok(req) => {
                        busy = true;
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile_one(&req.source)))
                            .unwrap_or_else(|_| Err("the script engine failed".into()));
                        let reply = r.map(|run| {
                            let p = Arc::clone(&run.prog);
                            runs.push(run);
                            p
                        });
                        let _ = req.reply.send(reply);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    // Nobody can ask any more (the process is ending).
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            for run in &mut runs {
                for i in 0..POOL {
                    busy |= run.poll(i);
                }
            }
            // A program nothing else holds (no session, no user) is freed here.
            runs.retain(|r| Arc::strong_count(&r.prog) > 1);
            if !busy {
                std::thread::park_timeout(Duration::from_millis(5));
            }
        }
    }
}

#[cfg(test)]
mod tests;

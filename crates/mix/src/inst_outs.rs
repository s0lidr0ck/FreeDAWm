//! An instrument's extra outputs on their own mixer channels ([`Track::instrument_outs`]).
//!
//! Each taken output plays into a bus, which an aux reads like any other bus: the aux brings its
//! own inserts, fader, sends and output, and the bus ordering already renders the instrument's
//! track before the auxes that read it (`sidechain`'s planner counts an instrument output as
//! feeding its bus). Per strip:
//!
//! - **Taking.** Before the instrument runs, it is told which outputs are taken
//!   (`Plugin::set_outputs_taken`); an output nobody takes stays on its main output.
//! - **Reading.** After it runs, each taken output's block is read into a buffer of its own
//!   (`Plugin::read_output`).
//! - **Latency.** The output leaves the instrument before the track's inserts, so it waits for
//!   them: it is delayed to the delay compensation the strip's main output is aligned to (less
//!   the instrument's own latency, which both share), and, on a bus an aux returns onto, by the
//!   aux returns' latency as the main output is. The lines are reserved in `sync`; rendering
//!   never allocates.
//! - **Mute and solo.** A silent strip (muted, or muted by a solo) plays nothing on its extra
//!   outputs either.
//!
//! Freezing and bouncing a track follow its main output only.

use crate::delay_line::DelayLine;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// The extra outputs of each instrument the mix has made, by plug-in id: what a hosted plug-in
/// declared when it was made (a built-in one's are known without one).
static KNOWN: Mutex<BTreeMap<String, Vec<String>>> = Mutex::new(BTreeMap::new());

/// Notes the extra outputs `p` (made for `id`) declares. Off the audio thread.
pub(crate) fn remember(id: &str, p: &dyn Plugin) {
    let names = p.extra_outputs();
    if let Ok(mut k) = KNOWN.lock() {
        if names.is_empty() {
            k.remove(id);
        } else {
            k.insert(id.to_string(), names);
        }
    }
}

/// The extra outputs of the instrument `id` as it declared them when the mix last made it
/// (empty: none, or never made).
pub fn known_outputs(id: &str) -> Vec<String> {
    KNOWN.lock().ok().and_then(|k| k.get(id).cloned()).unwrap_or_default()
}
use soundcraft_dsp::Plugin;
use soundcraft_model::{BusId, Track};

/// One taken output: where it goes, its block and its compensation line.
pub(crate) struct Out {
    pub output: u8,
    pub bus: BusId,
    pub buf: Vec<Vec<f32>>,
    line: DelayLine,
}

/// A strip's taken instrument outputs.
#[derive(Default)]
pub(crate) struct InstOuts {
    pub outs: Vec<Out>,
    /// The instrument filled the buffers this block.
    pub fresh: bool,
}

impl InstOuts {
    /// Brings the outputs in line with the track's (keeping the lines of those that stay).
    /// Allocates: `sync` only.
    pub fn sync(&mut self, t: &Track, max_block: usize) {
        let want: Vec<(u8, BusId)> = if t.kind.is_midi() { t.instrument_outs.iter().map(|o| (o.output, o.bus)).collect() } else { Vec::new() };
        if self.outs.len() == want.len() && self.outs.iter().zip(&want).all(|(o, w)| (o.output, o.bus) == *w) {
            for o in &mut self.outs {
                if o.buf.iter().any(|b| b.len() < max_block) {
                    o.buf = vec![vec![0.0; max_block]; 2];
                }
            }
            return;
        }
        let mut old = std::mem::take(&mut self.outs);
        for (output, bus) in want {
            let line = match old.iter().position(|o| o.output == output && o.bus == bus) {
                Some(i) => old.swap_remove(i).line,
                None => DelayLine::default(),
            };
            self.outs.push(Out { output, bus, buf: vec![vec![0.0; max_block]; 2], line });
        }
        self.fresh = false;
    }

    /// Room in each line for delays up to `cap` (`cap` gives it from the line's own need).
    /// Allocates: `sync` only.
    pub fn reserve(&mut self, cap: impl Fn(&DelayLine) -> usize) {
        for o in &mut self.outs {
            let c = cap(&o.line);
            o.line.reserve(c, 2);
        }
    }

    /// The taken outputs as `Plugin::set_outputs_taken` wants them.
    pub fn mask(&self) -> u32 {
        self.outs.iter().filter(|o| o.output < 32).fold(0, |m, o| m | (1u32 << o.output))
    }

    /// Before the instrument runs: tells it which outputs are taken (only when any is or was, so
    /// an instrument on a track with none is not called).
    pub fn before(&mut self, plugin: &mut dyn Plugin) {
        self.fresh = false;
        if !self.outs.is_empty() {
            plugin.set_outputs_taken(self.mask());
        }
    }

    /// After it ran: each taken output's block into its buffer. No allocation.
    pub fn after(&mut self, plugin: &mut dyn Plugin, frames: usize) {
        if self.outs.is_empty() {
            return;
        }
        for o in &mut self.outs {
            if !plugin.read_output(usize::from(o.output), &mut o.buf, frames) {
                for c in &mut o.buf {
                    c.iter_mut().take(frames).for_each(|x| *x = 0.0);
                }
            }
        }
        self.fresh = true;
    }

    /// Delays each output by `delay(bus)` samples (crossfading a change). `false` when a line
    /// needs room the next `sync` makes. No allocation.
    pub fn align(&mut self, delay: impl Fn(BusId) -> usize, fade: usize, frames: usize) -> bool {
        let mut fits = true;
        if !self.fresh {
            return fits;
        }
        for o in &mut self.outs {
            fits &= o.line.set(delay(o.bus), 2, fade);
            o.line.process(&mut o.buf, frames);
        }
        fits
    }
}

/// Adds a stereo output block into a bus's channels: a stereo bus left to left and right to
/// right, a mono one their average, a wider one its first two channels. No allocation.
pub(crate) fn add_into(src: &[Vec<f32>], dst: &mut [Vec<f32>], frames: usize) {
    let (Some(l), Some(r)) = (src.first(), src.get(1)) else { return };
    match dst {
        [mono] => {
            for ((d, a), b) in mono.iter_mut().zip(l).zip(r).take(frames) {
                *d += 0.5 * (a + b);
            }
        }
        [dl, dr, ..] => {
            for (d, a) in dl.iter_mut().zip(l).take(frames) {
                *d += a;
            }
            for (d, b) in dr.iter_mut().zip(r).take(frames) {
                *d += b;
            }
        }
        [] => {}
    }
}

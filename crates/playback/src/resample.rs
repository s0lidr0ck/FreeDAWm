//! Band-limited interpolation for the resampling path (device rate ≠ session rate, half speed,
//! any speed but 1): a 32-tap Blackman-windowed sinc whose cut-off follows the step, so reading
//! faster than the session rate filters what would fold back instead of aliasing it, and
//! reading slower does not image. The read point lags `HALF` frames behind the newest frame, so
//! the kernel never needs a frame not yet rendered; the frames before the current block come
//! from a short history. The kernel is tabulated per step (rebuilt only when the step changes)
//! and interpolated between phases. Never allocates.

use crate::outmap::MAX_DEVICE_CHANNELS;

/// Taps on each side of the read point.
pub(crate) const HALF: usize = 16;
/// Frames kept from the blocks before the current one.
pub(crate) const HIST: usize = 2 * HALF;
/// Kernel taps.
pub(crate) const TAPS: usize = 2 * HALF;
/// Phases tabulated between two frames.
const PHASES: usize = 256;

/// The last [`HIST`] frames rendered before the current block, per channel.
pub(crate) struct History {
    frames: [[f32; HIST]; MAX_DEVICE_CHANNELS],
    /// Where the next frame goes; the newest is just before it.
    next: usize,
}

impl Default for History {
    fn default() -> History {
        History { frames: [[0.0; HIST]; MAX_DEVICE_CHANNELS], next: 0 }
    }
}

impl History {
    /// Forget everything (a fresh start reads silence before its first block).
    pub(crate) fn clear(&mut self) {
        self.frames.iter_mut().for_each(|c| c.iter_mut().for_each(|x| *x = 0.0));
        self.next = 0;
    }

    /// Takes in the first `len` frames of `block` as the newest history (only the last
    /// [`HIST`] of them matter).
    pub(crate) fn push(&mut self, block: &[Vec<f32>], len: usize) {
        let from = len.saturating_sub(HIST);
        for i in from..len {
            for (c, h) in self.frames.iter_mut().enumerate() {
                if let Some(slot) = h.get_mut(self.next) {
                    *slot = block.get(c).and_then(|v| v.get(i)).copied().unwrap_or(0.0);
                }
            }
            self.next = (self.next + 1) % HIST;
        }
    }

    /// Channel `c`'s frame `back` frames before the block (1: the last one), 0 past [`HIST`].
    fn get(&self, c: usize, back: usize) -> f32 {
        if back == 0 || back > HIST {
            return 0.0;
        }
        let i = (self.next + HIST - back) % HIST;
        self.frames.get(c).and_then(|h| h.get(i)).copied().unwrap_or(0.0)
    }
}

/// The windowed-sinc kernel tabulated for one step.
pub(crate) struct Kernel {
    step: f64,
    table: [[f32; TAPS]; PHASES + 1],
}

impl Default for Kernel {
    fn default() -> Kernel {
        let mut k = Kernel { step: f64::NAN, table: [[0.0; TAPS]; PHASES + 1] };
        k.build(1.0);
        k
    }
}

impl Kernel {
    /// Tabulates the kernel for `step` session frames per output frame.
    fn build(&mut self, step: f64) {
        self.step = step;
        // Below the lower of the two Nyquists, with room for the transition band.
        let fc = if step.is_finite() && step > 1.0 { 0.94 / step } else { 0.94 };
        for (ph, row) in self.table.iter_mut().enumerate() {
            let frac = ph as f64 / PHASES as f64;
            let mut sum = 0.0f64;
            for (k, w) in row.iter_mut().enumerate() {
                // Taps at base - HALF + 1 ..= base + HALF.
                let t = (k as f64 - (HALF as f64 - 1.0)) - frac;
                let arg = std::f64::consts::PI * fc * t;
                let sinc = if arg.abs() < 1e-9 { 1.0 } else { arg.sin() / arg };
                let p = std::f64::consts::PI * t / HALF as f64;
                let window = if t.abs() >= HALF as f64 { 0.0 } else { 0.42 + 0.5 * p.cos() + 0.08 * (2.0 * p).cos() };
                let v = fc * sinc * window;
                sum += v;
                *w = v as f32;
            }
            // Unity gain at DC whatever the phase.
            if sum.abs() > 1e-9 {
                let g = (1.0 / sum) as f32;
                row.iter_mut().for_each(|x| *x *= g);
            }
        }
    }

    /// A kernel tabulated once for `step` (the input feed's fixed rate ratio), read with
    /// [`Kernel::weights`].
    pub(crate) fn for_step(step: f64) -> Box<Kernel> {
        let mut k = Box::new(Kernel { step: f64::NAN, table: [[0.0; TAPS]; PHASES + 1] });
        k.build(if step.is_finite() && step > 0.0 { step } else { 1.0 });
        k
    }

    /// The weights for a read `frac` (0..1) past tap `HALF - 1`, from the table as built (never
    /// rebuilt: no work beyond the interpolation).
    pub(crate) fn weights(&self, frac: f64) -> [f32; TAPS] {
        let pos = if frac.is_finite() { frac.clamp(0.0, 1.0) * PHASES as f64 } else { 0.0 };
        let i = (pos.floor() as usize).min(PHASES - 1);
        let f = (pos - i as f64) as f32;
        let mut w = [0.0f32; TAPS];
        if let (Some(a), Some(b)) = (self.table.get(i), self.table.get(i + 1)) {
            for ((o, x), y) in w.iter_mut().zip(a).zip(b) {
                *o = x + (y - x) * f;
            }
        }
        w
    }

    /// The kernel for a read at `read` (frames into the block, -1 or more) stepping `step`: the
    /// index of its first tap relative to the block, and the weights. The point actually read
    /// is `read - HALF`.
    pub(crate) fn at(&mut self, read: f64, step: f64) -> (isize, [f32; TAPS]) {
        let step = if step.is_finite() && step > 0.0 { step } else { 1.0 };
        if self.step != step {
            self.build(step);
        }
        let x = read - HALF as f64;
        let base = if x.is_finite() { x.floor() } else { 0.0 };
        let pos = if x.is_finite() { (x - base) * PHASES as f64 } else { 0.0 };
        let i = (pos.floor() as usize).min(PHASES - 1);
        let f = (pos - i as f64) as f32;
        let mut w = [0.0f32; TAPS];
        if let (Some(a), Some(b)) = (self.table.get(i), self.table.get(i + 1)) {
            for ((o, x), y) in w.iter_mut().zip(a).zip(b) {
                *o = x + (y - x) * f;
            }
        }
        let first = (base as isize).saturating_sub(HALF as isize - 1);
        (first, w)
    }
}

/// Channel `c` read with a [`Kernel::at`] result from `block` (its first `len` frames) and the
/// history before it.
pub(crate) fn sample(block: &[Vec<f32>], len: usize, hist: &History, c: usize, first: isize, w: &[f32; TAPS]) -> f32 {
    let ch = block.get(c);
    let mut acc = 0.0f32;
    for (k, wk) in w.iter().enumerate() {
        let j = first.saturating_add(k as isize);
        let v = if j >= 0 {
            let j = j as usize;
            if j < len { ch.and_then(|v| v.get(j)).copied().unwrap_or(0.0) } else { 0.0 }
        } else {
            hist.get(c, j.unsigned_abs())
        };
        acc += wk * v;
    }
    acc
}

/// What the player plays reading `input` (one channel, from a fresh start) at `step` per output
/// frame for `frames` frames: for tests that compare the player's resampled output.
#[cfg(test)]
pub(crate) fn reference(input: &[f32], from: f64, step: f64, frames: usize) -> Vec<f32> {
    let mut k = Kernel::default();
    let hist = History::default();
    let buf = [input.to_vec()];
    (0..frames)
        .map(|n| {
            let (first, w) = k.at(from + n as f64 * step, step);
            sample(&buf, input.len(), &hist, 0, first, &w)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads `input` at `step` through blocks of `block` frames, as the player does.
    fn run(input: &[f32], step: f64, block: usize) -> Vec<f32> {
        let mut hist = History::default();
        let mut k = Kernel::default();
        let mut out = Vec::new();
        let mut read = 0.0f64;
        for chunk in input.chunks(block) {
            let buf = vec![chunk.to_vec()];
            while read <= chunk.len() as f64 - 1.0 {
                let (first, w) = k.at(read, step);
                out.push(sample(&buf, chunk.len(), &hist, 0, first, &w));
                read += step;
            }
            hist.push(&buf, chunk.len());
            read -= chunk.len() as f64;
        }
        out
    }

    fn sine(freq: f64, rate: f64, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin() as f32).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn a_tone_in_band_passes_at_its_level_across_blocks() {
        // 1 kHz at 44.1 kHz read for a 48 kHz device, in blocks of odd sizes.
        let input = sine(1000.0, 44_100.0, 44_100);
        let out = run(&input, 44_100.0 / 48_000.0, 61);
        let tail = &out[1000..out.len() - 1000];
        assert!((rms(tail) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.005, "{}", rms(tail));
        // It is the same tone: compare with a direct evaluation (HALF session frames later).
        let lag = HALF as f64;
        let worst = (1000..out.len() - 1000)
            .map(|i| {
                let t = (i as f64 * 44_100.0 / 48_000.0 - lag) / 44_100.0;
                (out[i] - (2.0 * std::f64::consts::PI * 1000.0 * t).sin() as f32).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(worst < 0.005, "{worst}");
        // Blocks or no blocks, the same output.
        let whole = reference(&input, 0.0, 44_100.0 / 48_000.0, out.len());
        assert!(out.iter().zip(&whole).all(|(a, b)| (a - b).abs() < 1e-5));
    }

    #[test]
    fn reading_fast_filters_what_would_fold_back() {
        // At double speed a 15 kHz tone at 48 kHz would land at 30 kHz, past Nyquist: linear
        // interpolation folds it back to 18 kHz at nearly full level; the kernel removes it.
        let input = sine(15_000.0, 48_000.0, 48_000);
        let out = run(&input, 2.0, 128);
        let left = rms(&out[200..out.len() - 200]);
        assert!(left < 0.02, "{left}: at least 30 dB down");
        // And a low tone at double speed still passes.
        let input = sine(2_000.0, 48_000.0, 48_000);
        let out = run(&input, 2.0, 128);
        assert!((rms(&out[200..out.len() - 200]) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02);
    }

    #[test]
    fn silence_before_a_fresh_start_and_one_frame_blocks() {
        let mut hist = History::default();
        hist.push(&[vec![1.0; 40]], 40);
        hist.clear();
        let (first, w) = Kernel::default().at(0.0, 0.5);
        assert_eq!(sample(&[vec![0.0; 4]], 4, &hist, 0, first, &w), 0.0);
        // One-frame blocks of a constant read back as that constant once history is full.
        let out = run(&[0.5; 400], 0.5, 1);
        assert!(out[100..].iter().all(|v| (v - 0.5).abs() < 1e-4));
    }
}

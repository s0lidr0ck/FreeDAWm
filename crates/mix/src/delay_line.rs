//! Delay-compensation lines: a multichannel delay whose length can change while audio runs
//! through it, crossfading between the old and the new tap instead of clicking.

use crate::MAX_LATENCY;

/// A multichannel delay line (samples) over a ring reserved in `sync`.
///
/// Every processed sample is written into the ring, so the history needed by any delay up to the
/// capacity is always there. A delay change on a line that carries audio crossfades linearly from
/// the old tap to the new one (no click, no allocation). A line that carries no audio yet (fresh,
/// or cleared by a reset) jumps straight to the new delay, which keeps constant latency exact; so
/// does one on its second block: the latency a strip reports on its first block after a reset or
/// sync is provisional (a plugin that fails on that block was still counted, see
/// `note_insert_latency`), and the correction must stay sample-exact.
#[derive(Default)]
pub(crate) struct DelayLine {
    /// One ring per channel, `capacity + 1` samples (a delay of `capacity` reads the sample
    /// written `capacity` samples before the current one).
    bufs: Vec<Vec<f32>>,
    /// Next write position.
    w: usize,
    /// Current delay (the new tap while crossfading).
    len: usize,
    /// The old tap while crossfading.
    from: usize,
    /// Samples into the crossfade and its length; done when `fade >= fade_len`.
    fade: usize,
    fade_len: usize,
    /// Blocks processed since the line was created or cleared (counts up to 2).
    blocks: u8,
    /// Largest delay asked for since the last `reserve`, so `sync` can grow the line.
    need: usize,
}

impl DelayLine {
    /// Largest delay asked for since the last `reserve`.
    pub(crate) fn need(&self) -> usize {
        self.need
    }

    /// Address and capacity of each channel's ring (to check for reallocation).
    pub(crate) fn buffer_ids(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.bufs.iter().map(|b| (b.as_ptr() as usize, b.capacity()))
    }

    /// The current delay (the new tap while crossfading).
    pub(crate) fn delay(&self) -> usize {
        self.len
    }

    fn capacity(&self) -> usize {
        self.bufs.first().map_or(0, |b| b.len().saturating_sub(1))
    }

    fn fading(&self) -> bool {
        self.fade < self.fade_len
    }

    /// Something to process: the line holds a ring (it keeps history even at delay 0).
    pub(crate) fn active(&self) -> bool {
        !self.bufs.is_empty() && self.capacity() > 0
    }

    /// Makes room for delays up to `cap` on `ch` channels, keeping the most recent history.
    /// Allocates (only when it grows or the channel count changes): call it from `sync`.
    pub(crate) fn reserve(&mut self, cap: usize, ch: usize) {
        let cap = cap.min(MAX_LATENCY);
        self.need = self.len;
        if cap <= self.capacity() && self.bufs.len() == ch {
            return;
        }
        let cap = cap.max(self.capacity());
        let size = cap + 1;
        let old = std::mem::take(&mut self.bufs);
        let old_size = old.first().map_or(0, Vec::len);
        self.bufs = (0..ch)
            .map(|c| {
                let mut ring = vec![0.0; size];
                // Oldest first, ending just before the new write position `old_size`.
                if let Some(o) = old.get(c) {
                    for (i, r) in ring.iter_mut().take(old_size).enumerate() {
                        *r = o.get((self.w + i) % old_size.max(1)).copied().unwrap_or(0.0);
                    }
                }
                ring
            })
            .collect();
        self.w = old_size % size;
    }

    /// Sets the delay for the next block. Never allocates while the line carries audio: a delay
    /// beyond the capacity is clamped to it and `false` is returned (the next `sync` grows the
    /// line). A line that carries no audio yet is sized right away (`reserve`), as part of the
    /// first block after a sync or reset.
    pub(crate) fn set(&mut self, len: usize, ch: usize, fade_len: usize) -> bool {
        let len = len.min(MAX_LATENCY);
        self.need = self.need.max(len);
        // An unused line at delay 0 stays empty (no ring to allocate or run).
        if self.blocks == 0 && (len > self.capacity() || (!self.bufs.is_empty() && self.bufs.len() != ch)) {
            self.reserve(len, ch);
        }
        let fits = len <= self.capacity();
        let to = len.min(self.capacity());
        // A crossfade in progress finishes first; the new delay is taken up on a later block.
        if to == self.len || self.fading() {
            return fits;
        }
        if self.blocks >= 2 {
            self.from = self.len;
            self.fade = 0;
            self.fade_len = fade_len.max(1);
        }
        self.len = to;
        fits
    }

    pub(crate) fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let size = self.capacity() + 1;
        if !self.active() {
            return;
        }
        self.blocks = self.blocks.saturating_add(1).min(2);
        let (w0, len, from, fade, fade_len) = (self.w, self.len.min(size - 1), self.from.min(size - 1), self.fade, self.fade_len);
        for (c, ch) in io.iter_mut().enumerate() {
            let Some(ring) = self.bufs.get_mut(c) else { continue };
            let mut w = w0;
            let mut f = fade;
            for x in ch.iter_mut().take(frames) {
                if let Some(r) = ring.get_mut(w) {
                    *r = *x;
                }
                let new = ring.get((w + size - len) % size).copied().unwrap_or(0.0);
                *x = if f < fade_len {
                    let old = ring.get((w + size - from) % size).copied().unwrap_or(0.0);
                    f += 1;
                    let g = f as f32 / fade_len as f32;
                    old + (new - old) * g
                } else {
                    new
                };
                w += 1;
                if w >= size {
                    w = 0;
                }
            }
        }
        self.w = (w0 + frames) % size;
        self.fade = fade.saturating_add(frames).min(fade_len);
    }

    /// Silences the history (a reset): the next delay change jumps instead of crossfading.
    pub(crate) fn clear(&mut self) {
        self.bufs.iter_mut().for_each(|b| b.fill(0.0));
        self.fade = self.fade_len;
        self.blocks = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::DelayLine;

    const BLOCK: usize = 64;
    const FADE: usize = 32;

    /// Deterministic non-periodic signal.
    fn signal(n: usize) -> f32 {
        let mut x = (n as u32).wrapping_mul(2_654_435_761) ^ 0x9E37_79B9;
        x ^= x >> 15;
        x = x.wrapping_mul(0x85EB_CA6B);
        x ^= x >> 13;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Runs block `b` of the signal through `line` and returns the output.
    fn block(line: &mut DelayLine, b: usize) -> Vec<f32> {
        let mut io = vec![(0..BLOCK).map(|i| signal(b * BLOCK + i)).collect::<Vec<f32>>()];
        line.process(&mut io, BLOCK);
        io.remove(0)
    }

    fn delayed(n: usize, d: usize) -> f32 {
        n.checked_sub(d).map_or(0.0, signal)
    }

    fn warm_line(len: usize) -> DelayLine {
        let mut line = DelayLine::default();
        line.reserve(1000, 1);
        for b in 0..3 {
            assert!(line.set(len, 1, FADE));
            let out = block(&mut line, b);
            for (i, y) in out.iter().enumerate() {
                assert_eq!(*y, delayed(b * BLOCK + i, len), "constant delay is exact");
            }
        }
        line
    }

    #[test]
    fn a_delay_change_crossfades_linearly_between_the_taps() {
        for (from, to) in [(10, 50), (50, 10), (0, 37)] {
            let mut line = warm_line(from);
            assert!(line.set(to, 1, FADE));
            let mut out = block(&mut line, 3);
            out.extend(block(&mut line, 4));
            for (k, y) in out.iter().enumerate() {
                let n = 3 * BLOCK + k;
                let (old, new) = (delayed(n, from), delayed(n, to));
                let want = if k < FADE { old + (new - old) * ((k + 1) as f32 / FADE as f32) } else { new };
                assert!((y - want).abs() < 1e-6, "{from}->{to}, sample {k}: {y} vs {want}");
            }
            // Not vacuous: a hard switch (output = the new tap at once) differs inside the window.
            let off = (0..FADE - 1).map(|k| (out[k] - delayed(3 * BLOCK + k, to)).abs()).fold(0.0, f32::max);
            assert!(off > 0.1, "{from}->{to}: the window differs from a hard switch by {off}");
        }
    }

    #[test]
    fn a_change_during_a_crossfade_waits_for_it() {
        let mut line = warm_line(10);
        assert!(line.set(50, 1, 2 * BLOCK));
        block(&mut line, 3);
        assert!(line.set(80, 1, 2 * BLOCK), "ignored while fading");
        let out = block(&mut line, 4);
        for (k, y) in out.iter().enumerate() {
            let n = 4 * BLOCK + k;
            let g = (BLOCK + k + 1) as f32 / (2 * BLOCK) as f32;
            let want = delayed(n, 10) + (delayed(n, 50) - delayed(n, 10)) * g;
            assert!((y - want).abs() < 1e-6);
        }
        assert!(line.set(80, 1, FADE), "taken up once the crossfade ended");
        let out = block(&mut line, 5);
        let want = delayed(5 * BLOCK, 50) + (delayed(5 * BLOCK, 80) - delayed(5 * BLOCK, 50)) / FADE as f32;
        assert!((out[0] - want).abs() < 1e-6);
    }

    #[test]
    fn fresh_and_reset_lines_jump_exactly() {
        let mut line = DelayLine::default();
        assert!(!line.active(), "an unused line at delay 0 holds no ring");
        assert!(line.set(0, 2, FADE));
        assert!(!line.active());
        let mut line = warm_line(10);
        line.clear();
        assert!(line.set(40, 1, FADE));
        let out = block(&mut line, 3);
        // History was silenced: the new tap reads zeros, then the signal, with no crossfade.
        for (k, y) in out.iter().enumerate() {
            let want = if k < 40 { 0.0 } else { signal(3 * BLOCK + k - 40) };
            assert_eq!(*y, want);
        }
        // Its second block still jumps (the provisional first-block latency is corrected exactly).
        assert!(line.set(20, 1, FADE));
        let out = block(&mut line, 4);
        assert!(out.iter().enumerate().all(|(k, y)| *y == delayed(4 * BLOCK + k, 20)));
    }

    #[test]
    fn a_warm_line_never_grows_while_rendering_but_reserve_keeps_history() {
        let mut line = warm_line(10);
        let ids: Vec<_> = line.buffer_ids().collect();
        assert!(!line.set(5000, 1, FADE), "beyond the capacity");
        assert_eq!(line.need(), 5000);
        assert_eq!(line.buffer_ids().collect::<Vec<_>>(), ids, "no reallocation");
        let out = block(&mut line, 3);
        // Clamped to the capacity (1000) and crossfading towards it.
        let want = delayed(3 * BLOCK, 10) + (delayed(3 * BLOCK, 1000) - delayed(3 * BLOCK, 10)) / FADE as f32;
        assert!((out[0] - want).abs() < 1e-6);
        // Growing (sync's job) keeps the history in order.
        let mut line = warm_line(10);
        line.reserve(3000, 1);
        let out = block(&mut line, 3);
        assert!(out.iter().enumerate().all(|(k, y)| *y == delayed(3 * BLOCK + k, 10)));
        assert!(line.set(150, 1, FADE));
        let out = block(&mut line, 4);
        let n = 4 * BLOCK + FADE;
        assert!((out[FADE] - delayed(n, 150)).abs() < 1e-6, "history from before the grow is there");
    }
}

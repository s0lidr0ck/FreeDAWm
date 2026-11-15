//! Vault's convolution: a response of many seconds run with no latency and no spikes in load.
//!
//! The response is run in two stages that add up to the whole ([`super::ir`]):
//! - The **head** (the first [`HEAD_LEN`] samples) in partitions of [`HEAD_BLOCK`]. Each call
//!   takes the spectrum of the input block as far as it has been filled, so what goes in comes
//!   out in the same call: no latency. The older blocks' share is summed once per block.
//! - The **tail** (the rest) in partitions of [`TAIL_BLOCK`], eight times fewer of them for the
//!   same length. A block of input is only complete a block late, and its output is not due
//!   until [`HEAD_LEN`] after it began, so there is a whole block's time to do the block's work
//!   in. It is done a slice at a time as samples pass ([`Convolver::advance`]), so no single
//!   call pays for it all.
//!
//! Both keep the spectra of past input blocks (newest first in a ring) and multiply each by the
//! response partition that many blocks along. A partition's gain under Decay is a straight line
//! from its start to its end, made of the partition's two spectra ([`super::ir`]).
//!
//! Everything is allocated in [`Convolver::new`]; processing never allocates. A new response
//! starts from silence ([`Convolver::restart`]) without clearing the rings: a ring entry older
//! than the restart is never read.

// The loops run over channels by index: each indexes several per-channel buffers side by side.
#![allow(clippy::needless_range_loop)]

use super::ir::{HEAD_BINS, HEAD_BLOCK, HEAD_FFT, HEAD_LEN, HEAD_SEGS, MAX_IR_SECONDS, SpaceIr, TAIL_BINS, TAIL_BLOCK, TAIL_FFT};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

type C = Complex<f32>;
const ZERO: C = Complex::new(0.0, 0.0);
/// What a tail transform costs, in partition multiplies (for spreading a block's work evenly).
const FFT_WEIGHT: usize = 24;
/// A partition whose gain is under this at both ends is skipped, and so is everything after it.
const SILENT: f32 = 1.0e-5;

/// `acc += (h * a + r * b) * x`: a partition under the gain line `a + b * n / block`.
fn mac(acc: &mut [C], (h, r): (&[C], &[C]), x: &[C], (a, b): (f32, f32)) {
    if a == 1.0 && b == 0.0 {
        for ((acc, h), x) in acc.iter_mut().zip(h).zip(x) {
            *acc += h * x;
        }
    } else {
        for (((acc, h), r), x) in acc.iter_mut().zip(h).zip(r).zip(x) {
            *acc += (h * a + r * b) * x;
        }
    }
}

/// The real signal with the half spectrum `spec`, into `buf` (unscaled).
fn inverse(spec: &[C], buf: &mut [C], fft: &Arc<dyn Fft<f32>>, scratch: &mut [C]) {
    let n = buf.len();
    for (b, s) in buf.iter_mut().zip(spec) {
        *b = *s;
    }
    for k in 1..n / 2 {
        if let (Some(s), Some(b)) = (spec.get(k), buf.get_mut(n - k)) {
            *b = s.conj();
        }
    }
    fft.process_with_scratch(buf, scratch);
}

/// The spectrum of `block`, zero-padded to `buf`'s length, left in `buf`.
fn forward(block: &[f32], buf: &mut [C], fft: &Arc<dyn Fft<f32>>, scratch: &mut [C]) {
    buf.iter_mut().for_each(|b| *b = ZERO);
    for (b, x) in buf.iter_mut().zip(block) {
        b.re = *x;
    }
    fft.process_with_scratch(buf, scratch);
}

/// A tail block's work, part done.
#[derive(Debug, Clone, Copy)]
struct Job {
    /// The ring slot of the block's spectrum.
    slot: usize,
    /// Partitions to run (those with history behind them, not silenced by Decay).
    segs: usize,
    /// 0: the inputs' spectra, 1: the partitions, 2: the outputs, 3: done.
    stage: u8,
    i: usize,
    k: usize,
    done: usize,
    total: usize,
}

pub(crate) struct Convolver {
    head_fwd: Arc<dyn Fft<f32>>,
    head_inv: Arc<dyn Fft<f32>>,
    tail_fwd: Arc<dyn Fft<f32>>,
    tail_inv: Arc<dyn Fft<f32>>,
    scratch: Vec<C>,
    hbuf: Vec<C>,
    hspec: Vec<C>,
    tbuf: Vec<C>,
    // Routing: `(input, response channel, output)` per path.
    nin: usize,
    nout: usize,
    paths: [(usize, usize, usize); 4],
    npaths: usize,
    hsegs: usize,
    tsegs: usize,
    // The head: the block being filled, the input spectra (newest at `hcur`), the older blocks'
    // share of the block, the overlap.
    hin: [Vec<f32>; 2],
    hpos: usize,
    hfdl: [Vec<C>; 2],
    hcur: usize,
    hcount: usize,
    hpre: [Vec<C>; 2],
    hov: [Vec<f32>; 2],
    // The tail: the block being filled, the input spectra, the block being worked on, its sum,
    // the overlap, the output being played and the one being made.
    tcap: usize,
    tin: [Vec<f32>; 2],
    tpos: usize,
    tfdl: [Vec<C>; 2],
    tcur: usize,
    tcount: usize,
    tjob: [Vec<f32>; 2],
    tacc: [Vec<C>; 2],
    tov: [Vec<f32>; 2],
    tready: [Vec<f32>; 2],
    twork: [Vec<f32>; 2],
    job: Option<Job>,
    // Decay: each partition's gain line `(at its start, its rise)`, and how many are audible.
    hgain: Vec<(f32, f32)>,
    tgain: Vec<(f32, f32)>,
    hlive: usize,
    tlive: usize,
}

/// The tail partitions a response of [`MAX_IR_SECONDS`] has at `rate`.
fn tail_capacity(rate: f64) -> usize {
    let rate = if rate.is_finite() && rate > 0.0 { rate.min(768_000.0) } else { 48_000.0 };
    ((MAX_IR_SECONDS * rate) as usize).saturating_sub(HEAD_LEN).div_ceil(TAIL_BLOCK).max(1)
}

impl Convolver {
    /// Sized for responses up to [`MAX_IR_SECONDS`] at `rate`. Allocates.
    pub(crate) fn new(rate: f64) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let (head_fwd, head_inv) = (planner.plan_fft_forward(HEAD_FFT), planner.plan_fft_inverse(HEAD_FFT));
        let (tail_fwd, tail_inv) = (planner.plan_fft_forward(TAIL_FFT), planner.plan_fft_inverse(TAIL_FFT));
        let scratch = [&head_fwd, &head_inv, &tail_fwd, &tail_inv].iter().map(|f| f.get_inplace_scratch_len()).max().unwrap_or(0);
        let tcap = tail_capacity(rate);
        let two = |n: usize| [vec![0.0f32; n], vec![0.0f32; n]];
        let twoc = |n: usize| [vec![ZERO; n], vec![ZERO; n]];
        Convolver {
            head_fwd,
            head_inv,
            tail_fwd,
            tail_inv,
            scratch: vec![ZERO; scratch],
            hbuf: vec![ZERO; HEAD_FFT],
            hspec: vec![ZERO; HEAD_BINS],
            tbuf: vec![ZERO; TAIL_FFT],
            nin: 1,
            nout: 1,
            paths: [(0, 0, 0); 4],
            npaths: 0,
            hsegs: 0,
            tsegs: 0,
            hin: two(HEAD_BLOCK),
            hpos: 0,
            hfdl: twoc(HEAD_SEGS * HEAD_BINS),
            hcur: 0,
            hcount: 0,
            hpre: twoc(HEAD_BINS),
            hov: two(HEAD_BLOCK),
            tcap,
            tin: two(TAIL_BLOCK),
            tpos: 0,
            tfdl: twoc(tcap * TAIL_BINS),
            tcur: 0,
            tcount: 0,
            tjob: two(TAIL_BLOCK),
            tacc: twoc(TAIL_BINS),
            tov: two(TAIL_BLOCK),
            tready: two(TAIL_BLOCK),
            twork: two(TAIL_BLOCK),
            job: None,
            hgain: vec![(1.0, 0.0); HEAD_SEGS],
            tgain: vec![(1.0, 0.0); tcap],
            hlive: HEAD_SEGS,
            tlive: tcap,
        }
    }

    /// Input channels the response takes (1: the mono sum; 2: left and right, true stereo).
    pub(crate) fn inputs(&self) -> usize {
        self.nin
    }

    /// Output channels it makes (1: the same on every channel).
    pub(crate) fn outputs(&self) -> usize {
        self.nout
    }

    /// Takes `ir` for a track of `channels` channels, starting from silence.
    pub(crate) fn configure(&mut self, ir: &SpaceIr, channels: usize) {
        use crate::SampleSource;
        let stereo = channels >= 2;
        let (nin, nout, paths): (usize, usize, &[(usize, usize, usize)]) = match (ir.num_channels(), stereo) {
            (4, true) => (2, 2, &[(0, 0, 0), (0, 1, 1), (1, 2, 0), (1, 3, 1)]),
            (2, true) => (1, 2, &[(0, 0, 0), (0, 1, 1)]),
            _ => (1, 1, &[(0, 0, 0)]),
        };
        (self.nin, self.nout, self.npaths) = (nin, nout, paths.len());
        for (slot, p) in self.paths.iter_mut().zip(paths) {
            *slot = *p;
        }
        let (h, t) = ir.segs();
        (self.hsegs, self.tsegs) = (h.min(HEAD_SEGS), t.min(self.tcap));
        self.restart();
    }

    /// Back to silence (the rings are not cleared: nothing older than this is read again).
    pub(crate) fn restart(&mut self) {
        (self.hpos, self.hcur, self.hcount) = (0, 0, 0);
        (self.tpos, self.tcur, self.tcount) = (0, 0, 0);
        self.job = None;
        for v in self.hin.iter_mut().chain(&mut self.hov).chain(&mut self.tin).chain(&mut self.tov).chain(&mut self.tready).chain(&mut self.twork) {
            v.iter_mut().for_each(|x| *x = 0.0);
        }
        for v in self.hpre.iter_mut().chain(&mut self.tacc) {
            v.iter_mut().for_each(|x| *x = ZERO);
        }
    }

    /// Decay for a response of `len` frames: `decay` 1 leaves it as it is; below that it is
    /// turned down along its length, 60 dB more at its end for every time `decay` halves (at
    /// 0.5: 60 dB, at 0.25: 180 dB), in a straight line across each partition.
    pub(crate) fn set_decay(&mut self, len: usize, decay: f32) {
        let decay = if decay.is_finite() { decay.clamp(0.01, 1.0) } else { 1.0 };
        let slope = -3.0 * (1.0 / decay - 1.0) / len.max(1) as f32;
        let gain = |n: usize| if decay >= 1.0 { 1.0 } else { 10f32.powf(slope * n as f32) };
        let fill = |gains: &mut [(f32, f32)], from: usize, block: usize| {
            let count = gains.len();
            let mut live = count;
            for (k, g) in gains.iter_mut().enumerate() {
                let (a, b) = (gain(from + k * block), gain(from + (k + 1) * block));
                *g = (a, b - a);
                if a < SILENT && live == count {
                    live = k;
                }
            }
            live
        };
        self.hlive = fill(&mut self.hgain, 0, HEAD_BLOCK);
        self.tlive = fill(&mut self.tgain, HEAD_LEN, TAIL_BLOCK);
    }

    /// Runs one unit of the tail block's work; returns its weight.
    fn job_step(&mut self, ir: &SpaceIr) -> usize {
        let Some(job) = self.job.as_mut() else { return 0 };
        match job.stage {
            0 => {
                let c = job.i;
                forward(&self.tjob[c], &mut self.tbuf, &self.tail_fwd, &mut self.scratch);
                if let Some(slot) = self.tfdl[c].get_mut(job.slot * TAIL_BINS..(job.slot + 1) * TAIL_BINS) {
                    slot.copy_from_slice(&self.tbuf[..TAIL_BINS]);
                }
                job.i += 1;
                if job.i >= self.nin {
                    (job.stage, job.i, job.k) = (1, 0, 0);
                }
                FFT_WEIGHT
            }
            1 => {
                if job.segs == 0 || job.i >= self.npaths {
                    (job.stage, job.i) = (2, 0);
                    return 0;
                }
                let (c, j, o) = self.paths[job.i];
                let at = (job.slot + job.k) % self.tcap;
                if let (Some(stage), Some(x)) = (ir.tail(j), self.tfdl[c].get(at * TAIL_BINS..(at + 1) * TAIL_BINS)) {
                    mac(&mut self.tacc[o], stage.seg(job.k), x, self.tgain.get(job.k).copied().unwrap_or((1.0, 0.0)));
                }
                job.k += 1;
                if job.k >= job.segs {
                    (job.k, job.i) = (0, job.i + 1);
                }
                1
            }
            2 => {
                let o = job.i;
                inverse(&self.tacc[o], &mut self.tbuf, &self.tail_inv, &mut self.scratch);
                let scale = 1.0 / TAIL_FFT as f32;
                let (now, later) = self.tbuf.split_at(TAIL_BLOCK);
                for (((w, ov), a), b) in self.twork[o].iter_mut().zip(self.tov[o].iter_mut()).zip(now).zip(later) {
                    *w = a.re * scale + *ov;
                    *ov = b.re * scale;
                }
                self.tacc[o].iter_mut().for_each(|x| *x = ZERO);
                job.i += 1;
                if job.i >= self.nout {
                    job.stage = 3;
                }
                FFT_WEIGHT
            }
            _ => 0,
        }
    }

    /// Brings the tail block's work up to the share due after `tpos` of the block's samples.
    fn advance(&mut self, ir: &SpaceIr) {
        loop {
            let Some(job) = self.job else { return };
            let due = if self.tpos >= TAIL_BLOCK { usize::MAX } else { (job.total * self.tpos).div_ceil(TAIL_BLOCK) };
            if job.stage >= 3 || job.done >= due {
                return;
            }
            let w = self.job_step(ir);
            if let Some(j) = self.job.as_mut() {
                j.done += w;
            }
        }
    }

    /// Convolves the first `frames` of `input` (its first [`Convolver::inputs`] channels) with
    /// `ir` into `out` (its first [`Convolver::outputs`]). `ir` is the response last given to
    /// [`Convolver::configure`].
    pub(crate) fn process(&mut self, ir: &SpaceIr, input: &[Vec<f32>; 2], out: &mut [Vec<f32>; 2], frames: usize) {
        let frames = input.iter().take(self.nin).chain(out.iter().take(self.nout)).map(Vec::len).fold(frames, usize::min);
        let hscale = 1.0 / HEAD_FFT as f32;
        let mut done = 0;
        while done < frames {
            let mut n = (frames - done).min(HEAD_BLOCK - self.hpos);
            if self.tsegs > 0 {
                n = n.min(TAIL_BLOCK - self.tpos);
            }
            let (hpos, hcur) = (self.hpos, self.hcur);
            // The head: the block so far, its spectrum, the output it gives.
            for c in 0..self.nin {
                self.hin[c][hpos..hpos + n].copy_from_slice(&input[c][done..done + n]);
                forward(&self.hin[c], &mut self.hbuf, &self.head_fwd, &mut self.scratch);
                self.hfdl[c][hcur * HEAD_BINS..(hcur + 1) * HEAD_BINS].copy_from_slice(&self.hbuf[..HEAD_BINS]);
            }
            let older = self.hsegs.min(self.hcount + 1).min(self.hlive);
            if hpos == 0 {
                for o in 0..self.nout {
                    self.hpre[o].iter_mut().for_each(|x| *x = ZERO);
                }
                for &(c, j, o) in self.paths.iter().take(self.npaths) {
                    let Some(stage) = ir.head(j) else { continue };
                    for k in 1..older {
                        let at = (hcur + k) % HEAD_SEGS;
                        mac(&mut self.hpre[o], stage.seg(k), &self.hfdl[c][at * HEAD_BINS..(at + 1) * HEAD_BINS], self.hgain[k]);
                    }
                }
            }
            for o in 0..self.nout {
                self.hspec.copy_from_slice(&self.hpre[o]);
                if older > 0 {
                    for &(c, j, po) in self.paths.iter().take(self.npaths) {
                        if po == o
                            && let Some(stage) = ir.head(j)
                        {
                            mac(&mut self.hspec, stage.seg(0), &self.hfdl[c][hcur * HEAD_BINS..(hcur + 1) * HEAD_BINS], self.hgain[0]);
                        }
                    }
                }
                inverse(&self.hspec, &mut self.hbuf, &self.head_inv, &mut self.scratch);
                for i in 0..n {
                    out[o][done + i] = self.hbuf[hpos + i].re * hscale + self.hov[o][hpos + i];
                }
                if hpos + n == HEAD_BLOCK {
                    for (ov, b) in self.hov[o].iter_mut().zip(&self.hbuf[HEAD_BLOCK..]) {
                        *ov = b.re * hscale;
                    }
                }
            }
            self.hpos += n;
            if self.hpos == HEAD_BLOCK {
                self.hin.iter_mut().flatten().for_each(|x| *x = 0.0);
                self.hpos = 0;
                self.hcur = (hcur + HEAD_SEGS - 1) % HEAD_SEGS;
                self.hcount = (self.hcount + 1).min(HEAD_SEGS);
            }
            // The tail: play what the last finished block gave, work on the one before this.
            if self.tsegs > 0 {
                let tpos = self.tpos;
                for c in 0..self.nin {
                    self.tin[c][tpos..tpos + n].copy_from_slice(&input[c][done..done + n]);
                }
                for o in 0..self.nout {
                    for i in 0..n {
                        out[o][done + i] += self.tready[o][tpos + i];
                    }
                }
                self.tpos += n;
                self.advance(ir);
                if self.tpos == TAIL_BLOCK {
                    for o in 0..self.nout {
                        std::mem::swap(&mut self.tready[o], &mut self.twork[o]);
                    }
                    for c in 0..self.nin {
                        self.tjob[c].copy_from_slice(&self.tin[c]);
                    }
                    self.tcur = (self.tcur + self.tcap - 1) % self.tcap;
                    let segs = self.tsegs.min(self.tcount + 1).min(self.tlive).min(self.tcap);
                    let total = (self.nin + self.nout) * FFT_WEIGHT + self.npaths * segs;
                    self.job = Some(Job { slot: self.tcur, segs, stage: 0, i: 0, k: 0, done: 0, total });
                    self.tcount = (self.tcount + 1).min(self.tcap);
                    self.tpos = 0;
                }
            }
            done += n;
        }
    }

    /// The addresses of the big buffers (allocation tests).
    #[cfg(test)]
    fn buffer_ptrs(&self) -> Vec<usize> {
        let mut v = vec![self.scratch.as_ptr() as usize, self.tbuf.as_ptr() as usize, self.hbuf.as_ptr() as usize];
        v.extend(self.tfdl.iter().chain(&self.hfdl).map(|b| b.as_ptr() as usize));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SampleSource;

    const RATE: f64 = 48_000.0;

    fn noise(len: usize, mut seed: u32) -> Vec<f32> {
        (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
            })
            .collect()
    }

    /// A decaying noise response of `len` samples.
    fn response(len: usize, seed: u32) -> Vec<f32> {
        noise(len, seed).iter().enumerate().map(|(i, x)| x * (-(i as f32) * 5.0 / len as f32).exp()).collect()
    }

    /// A sparse input: clicks and short bursts at uneven places (direct convolution stays cheap).
    fn sparse(len: usize) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        let mut at = 3;
        let mut v = 0.9f32;
        while at + 40 < len {
            for i in 0..(at % 7 + 1) {
                x[at + i] = v * (1.0 - i as f32 * 0.1);
            }
            v = -v * 0.93;
            at += 977 + (at % 1531);
        }
        x
    }

    fn direct(x: &[f32], h: &[f32]) -> Vec<f32> {
        let mut y = vec![0.0f64; x.len()];
        for (n, xv) in x.iter().enumerate().filter(|(_, v)| **v != 0.0) {
            for (yv, hv) in y[n..].iter_mut().zip(h) {
                *yv += f64::from(*xv) * f64::from(*hv);
            }
        }
        y.iter().map(|v| *v as f32).collect()
    }

    fn run(c: &mut Convolver, ir: &SpaceIr, input: &[Vec<f32>], block: usize) -> [Vec<f32>; 2] {
        let len = input[0].len();
        let mut out = [Vec::with_capacity(len), Vec::with_capacity(len)];
        let mut at = 0;
        while at < len {
            let n = block.min(len - at);
            let chunk = |c: usize| input.get(c).map_or(vec![0.0; n], |x| x[at..at + n].to_vec());
            let inp = [chunk(0), chunk(1)];
            let mut o = [vec![0.0f32; n], vec![0.0f32; n]];
            c.process(ir, &inp, &mut o, n);
            out[0].extend_from_slice(&o[0]);
            out[1].extend_from_slice(&o[1]);
            at += n;
        }
        out
    }

    fn worst(got: &[f32], want: &[f32]) -> f32 {
        got.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn a_long_response_matches_direct_convolution_in_any_block_size() {
        // Head and five tail partitions and a bit.
        let ir = SpaceIr::new(&[response(30_000, 7)], RATE, RATE).unwrap();
        let h = ir.channel(0).unwrap().to_vec();
        let x = sparse(70_000);
        let want = direct(&x, &h);
        let scale = want.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        for block in [1usize, 37, 512, 1024, 4096, 5000] {
            let mut c = Convolver::new(RATE);
            c.configure(&ir, 1);
            c.set_decay(ir.len(), 1.0);
            let before = c.buffer_ptrs();
            let got = run(&mut c, &ir, std::slice::from_ref(&x), block);
            assert!(worst(&got[0], &want) < 2.0e-5 * scale.max(1.0), "block {block}: {} of {scale}", worst(&got[0], &want));
            assert_eq!(c.buffer_ptrs(), before, "block {block}: nothing was allocated again");
        }
    }

    #[test]
    fn short_responses_run_in_the_head_alone() {
        for len in [1usize, 300, 512, 513, 8192] {
            let ir = SpaceIr::new(&[response(len, 3)], RATE, RATE).unwrap();
            let h = ir.channel(0).unwrap().to_vec();
            let x = sparse(20_000);
            let mut c = Convolver::new(RATE);
            c.configure(&ir, 1);
            let got = run(&mut c, &ir, std::slice::from_ref(&x), 100);
            assert!(worst(&got[0], &direct(&x, &h)) < 2.0e-5, "{len} samples");
            assert_eq!(ir.segs().1, 0);
        }
    }

    #[test]
    fn a_stereo_pair_takes_the_mono_sum_and_true_stereo_takes_both_sides() {
        let chans: Vec<Vec<f32>> = (0..4).map(|i| response(12_000, 11 + i)).collect();
        let x = [sparse(30_000), sparse(30_000).iter().rev().copied().collect::<Vec<f32>>()];
        // A pair: one input, each side its own response.
        let ir = SpaceIr::new(&chans[..2], RATE, RATE).unwrap();
        let mut c = Convolver::new(RATE);
        c.configure(&ir, 2);
        assert_eq!((c.inputs(), c.outputs()), (1, 2));
        let got = run(&mut c, &ir, &x[..1], 480);
        for o in 0..2 {
            assert!(worst(&got[o], &direct(&x[0], ir.channel(o).unwrap())) < 2.0e-5, "side {o}");
        }
        // True stereo: left in to both outs, right in to both outs.
        let ir = SpaceIr::new(&chans, RATE, RATE).unwrap();
        c.configure(&ir, 2);
        assert_eq!((c.inputs(), c.outputs()), (2, 2));
        let got = run(&mut c, &ir, &x, 480);
        for o in 0..2 {
            let (a, b) = (direct(&x[0], ir.channel(o).unwrap()), direct(&x[1], ir.channel(2 + o).unwrap()));
            let want: Vec<f32> = a.iter().zip(&b).map(|(p, q)| p + q).collect();
            assert!(worst(&got[o], &want) < 4.0e-5, "out {o}: {}", worst(&got[o], &want));
        }
        // On a mono track any response is its first channel.
        c.configure(&ir, 1);
        assert_eq!((c.inputs(), c.outputs()), (1, 1));
    }

    #[test]
    fn decay_turns_the_response_down_along_its_length_in_straight_lines() {
        let ir = SpaceIr::new(&[response(30_000, 5)], RATE, RATE).unwrap();
        let h = ir.channel(0).unwrap().to_vec();
        let x = sparse(70_000);
        let mut c = Convolver::new(RATE);
        c.configure(&ir, 1);
        c.set_decay(ir.len(), 0.5);
        let got = run(&mut c, &ir, std::slice::from_ref(&x), 333);
        // The same lines, drawn on the response itself.
        let gain = |n: usize| 10f32.powf(-3.0 * n as f32 / h.len() as f32);
        let line = |n: usize| {
            let (from, block) = if n < HEAD_LEN {
                (n / HEAD_BLOCK * HEAD_BLOCK, HEAD_BLOCK)
            } else {
                (HEAD_LEN + (n - HEAD_LEN) / TAIL_BLOCK * TAIL_BLOCK, TAIL_BLOCK)
            };
            gain(from) + (gain(from + block) - gain(from)) * (n - from) as f32 / block as f32
        };
        let shaped: Vec<f32> = h.iter().enumerate().map(|(n, v)| v * line(n)).collect();
        assert!(worst(&got[0], &direct(&x, &shaped)) < 3.0e-5);
        // And the lines stay within a dB of the curve they follow, even here: 60 dB over little
        // more than half a second, 8 dB to a tail partition (a hall's is under 2).
        let off = (0..h.len()).map(|n| (20.0 * (line(n) / gain(n)).log10()).abs()).fold(0.0f32, f32::max);
        assert!(off < 1.0, "{off} dB");
        // A strong Decay silences the far partitions, which are then skipped.
        c.set_decay(ir.len(), 0.05);
        assert!(c.tlive == 0 && (1..HEAD_SEGS).contains(&c.hlive), "{} {}", c.tlive, c.hlive);
    }

    #[test]
    fn a_restart_leaves_nothing_of_what_came_before() {
        let ir = SpaceIr::new(&[response(30_000, 9)], RATE, RATE).unwrap();
        let mut c = Convolver::new(RATE);
        c.configure(&ir, 1);
        let loud = vec![noise(20_000, 1)];
        run(&mut c, &ir, &loud, 512);
        c.restart();
        let x = sparse(40_000);
        let got = run(&mut c, &ir, std::slice::from_ref(&x), 512);
        assert!(worst(&got[0], &direct(&x, ir.channel(0).unwrap())) < 2.0e-5);
    }

    #[test]
    fn the_tails_work_is_spread_over_its_block() {
        // Ten seconds of response: 115 tail partitions. No call does more than a fair share.
        let ir = SpaceIr::new(&[response(480_000, 2)], RATE, RATE).unwrap();
        let mut c = Convolver::new(RATE);
        c.configure(&ir, 1);
        let x = [noise(64, 4), vec![]];
        let mut out = [vec![0.0f32; 64], vec![0.0f32; 64]];
        let mut most = 0;
        for _ in 0..(TAIL_BLOCK / 64) * 130 {
            let before = c.job.map_or(0, |j| j.done);
            c.process(&ir, &x, &mut out, 64);
            let after = c.job.map_or(0, |j| j.done);
            most = most.max(after.saturating_sub(before));
        }
        let total = c.job.map_or(0, |j| j.total);
        assert!(total > 150, "{total}");
        assert!(most <= total / 64 + 2 * FFT_WEIGHT, "one call did {most} of {total}");
    }
}

//! What every Auto Pitch engine shares: the input history (each channel and their mono sum), the
//! detector and corrector run on the mono sum every detection hop, and the log of their decisions
//! by input time. An engine (Studio's phase vocoder; a low-delay one later) reads the input and
//! the log and writes the output; all channels move by the same amount, so a stereo image holds.
//!
//! Sizes are fixed at construction (off the audio thread); nothing here allocates afterwards.

use super::correct::{Corrector, Settings};
use super::detect::Detector;

/// Decisions kept (a few times what any engine looks back).
const LOG: usize = 64;

/// One detection's decision, at input sample `tc` (the middle of the detector's window).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Est {
    pub tc: f64,
    /// Sung pitch (MIDI cents), NaN when unvoiced.
    pub cents: f64,
    /// The note aimed at (MIDI cents), NaN when letting the voice through.
    pub target: f64,
    /// The correction applied (cents).
    pub corr: f64,
}

pub(super) struct Front {
    pub sr: f64,
    pub channels: usize,
    /// Detection hop (samples).
    pub hop: usize,
    /// Input history per channel and the mono sum (power-of-two rings).
    x: Vec<Vec<f32>>,
    mono: Vec<f32>,
    xmask: usize,
    mmask: usize,
    /// Input samples taken since the last reset.
    pub n_in: u64,
    pub det: Detector,
    pub cor: Corrector,
    log: [Est; LOG],
    /// Decisions made since the last reset.
    log_n: u64,
}

impl Front {
    /// `history`: input samples each engine may look back (the ring holds at least that plus a
    /// hop). Allocates.
    pub(super) fn new(sr: f64, channels: usize, hop: usize, history: usize, floor_hz: f64) -> Front {
        let det = Detector::new(sr, floor_hz);
        let hop = hop.max(1);
        let xlen = (history + 2 * hop).next_power_of_two();
        let mlen = (det.frame_len() + 2 * hop).next_power_of_two();
        Front {
            sr,
            channels,
            hop,
            x: vec![vec![0.0; xlen]; channels],
            mono: vec![0.0; mlen],
            xmask: xlen - 1,
            mmask: mlen - 1,
            n_in: 0,
            det,
            cor: Corrector::new(hop as f64 / sr * 1e3),
            log: [Est::default(); LOG],
            log_n: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        self.x.iter_mut().for_each(|c| c.iter_mut().for_each(|v| *v = 0.0));
        self.mono.iter_mut().for_each(|v| *v = 0.0);
        self.n_in = 0;
        self.det.reset();
        self.cor.reset();
        self.log_n = 0;
    }

    /// Input sample `i` (absolute) of channel `c`.
    #[inline]
    pub(super) fn x(&self, c: usize, i: u64) -> f32 {
        self.x.get(c).and_then(|b| b.get(i as usize & self.xmask)).copied().unwrap_or(0.0)
    }

    /// Takes `take` frames of `io` from `pos` (channels past `io`'s get silence).
    pub(super) fn push(&mut self, io: &[Vec<f32>], pos: usize, take: usize) {
        let n = self.channels.max(1) as f32;
        for i in 0..take {
            let at = (self.n_in + i as u64) as usize;
            let mut sum = 0.0f32;
            for (c, ring) in self.x.iter_mut().enumerate() {
                let v = io.get(c).and_then(|b| b.get(pos + i)).copied().unwrap_or(0.0);
                let v = if v.is_finite() { v.clamp(-64.0, 64.0) } else { 0.0 };
                if let Some(slot) = ring.get_mut(at & self.xmask) {
                    *slot = v;
                }
                sum += v;
            }
            if let Some(slot) = self.mono.get_mut(at & self.mmask) {
                *slot = sum / n;
            }
        }
        self.n_in += take as u64;
    }

    /// Whether a detection is due now (the input has just reached a hop boundary).
    pub(super) fn detection_due(&self) -> bool {
        self.n_in.is_multiple_of(self.hop as u64) && self.n_in >= self.det.frame_len() as u64
    }

    /// Runs the detector and corrector on the latest window; returns the decision logged.
    pub(super) fn detect(&mut self, s: &Settings, mask: u16) -> Est {
        let fl = self.det.frame_len();
        let start = self.n_in - fl as u64;
        let mm = self.mmask;
        for (k, v) in self.det.frame_mut().iter_mut().enumerate() {
            *v = f64::from(self.mono.get((start as usize + k) & mm).copied().unwrap_or(0.0));
        }
        let (cents, conf) = self.det.analyse();
        let (target, corr) = self.cor.step(s, mask, cents, conf);
        let e = Est {
            tc: self.n_in as f64 - fl as f64 / 2.0,
            cents: cents.filter(|_| target.is_some()).unwrap_or(f64::NAN),
            target: target.unwrap_or(f64::NAN),
            corr,
        };
        if let Some(slot) = self.log.get_mut((self.log_n % LOG as u64) as usize) {
            *slot = e;
        }
        self.log_n += 1;
        e
    }

    /// Input time (samples) of the decision about to be made at the next detection: where the
    /// key is looked up.
    pub(super) fn next_tc_offset(&self) -> f64 {
        self.det.frame_len() as f64 / 2.0
    }

    fn entry(&self, k: u64) -> Est {
        self.log.get((k % LOG as u64) as usize).copied().unwrap_or_default()
    }

    /// Voiced (aimed at a note), sung cents and correction at input time `t`, from the decisions
    /// made so far (linear between two; the nearest's voicing).
    pub(super) fn at(&self, t: f64) -> (bool, f64, f64) {
        if self.log_n == 0 {
            return (false, 0.0, 0.0);
        }
        let oldest = self.log_n.saturating_sub(LOG as u64);
        let first = self.entry(oldest);
        let k = (t - first.tc) / self.hop as f64;
        if k.is_nan() || k < 0.0 {
            return (false, 0.0, 0.0);
        }
        let i = (k.floor() as u64).min(self.log_n - 1 - oldest);
        let e0 = self.entry(oldest + i);
        let e1 = if oldest + i + 1 < self.log_n { self.entry(oldest + i + 1) } else { e0 };
        let u = (k - i as f64).clamp(0.0, 1.0);
        let corr = e0.corr + (e1.corr - e0.corr) * u;
        let near = if u < 0.5 { e0 } else { e1 };
        let cents = match (e0.cents.is_finite(), e1.cents.is_finite()) {
            (true, true) => e0.cents + (e1.cents - e0.cents) * u,
            _ => [near.cents, e0.cents, e1.cents].into_iter().find(|c| c.is_finite()).unwrap_or(0.0),
        };
        (near.target.is_finite(), cents, if corr.is_finite() { corr } else { 0.0 })
    }

    /// Addresses and capacities of the heap buffers (tests: nothing reallocates).
    #[cfg(test)]
    pub(super) fn buffers(&self) -> Vec<(usize, usize)> {
        fn of<T>(v: &Vec<T>) -> (usize, usize) {
            (v.as_ptr() as usize, v.capacity())
        }
        let mut out = vec![of(&self.mono)];
        out.extend(self.x.iter().map(of));
        out
    }
}

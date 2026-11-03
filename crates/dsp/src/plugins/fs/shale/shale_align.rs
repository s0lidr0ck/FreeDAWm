//! A faithful translation of `Shale/Source/ShaleAlign.h`: drum mic phase alignment.
//!
//! One mic in a Kit Link group is the phase reference; every other mic measures itself against
//! it hit by hit ([`measure`], normalised cross-correlation over +/-10 ms with a parabolic
//! peak), keeps the last 24 good measurements ([`Estimator`], their median, spread and majority
//! polarity), and corrects itself with [`AlignDelay`]: a fractional delay of `base + offset`
//! (base reported as latency, so the offset can be negative) with a polarity flip on top.
//!
//! [`measure`] and [`Estimator`] run on the C++ message thread (`runAlignment`, 4 Hz) against the
//! reference mic's audio through Kit Link's shared audio (`ShaleAudioShare`, here the link group's
//! slab). A module has no message thread, so the port measures on the audio thread a few lags per
//! block ([`Measurer`], the same arithmetic); [`AlignDelay`] runs on the audio thread and works on
//! its own (Align, Align Offset and Align Polarity set by hand, by a preset or by Line Up).

// Index loops kept as the C++ writes them (several arrays per index).
#![allow(clippy::needless_range_loop)]

/// `maxLagMs`.
pub const MAX_LAG_MS: f64 = 10.0;
/// The window starts well before the reported hit: `preMs`, `postMs`.
pub const PRE_MS: f64 = 12.0;
pub const POST_MS: f64 = 20.0;

/// The Align Offset parameter: 0..1, 0.5 = none, the ends +/-10 ms.
pub fn knob_to_offset_ms(v: f64) -> f64 {
    (v - 0.5) * 2.0 * MAX_LAG_MS
}

pub fn offset_ms_to_knob(ms: f64) -> f64 {
    (0.5 + ms / (2.0 * MAX_LAG_MS)).clamp(0.0, 1.0)
}

/// One measurement.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Lag {
    pub ok: bool,
    /// + : the reference hears it this much LATER than this mic.
    pub samples: f64,
    /// Signed peak correlation, -1 .. 1.
    pub corr: f64,
}

/// `measure (x, n, y, maxLag, minCorr = 0.3)`. `x`: this mic, `n` samples. `y`: the reference
/// over the same stretch plus `max_lag` either side (`n + 2 * max_lag` samples; `y[max_lag]`
/// lines up with `x[0]`). Too-short slices measure nothing (`ok` false), as a bad `n` does.
pub fn measure(x: &[f32], n: usize, y: &[f32], max_lag: usize, min_corr: f64) -> Lag {
    let mut res = Lag::default();
    if n < 16 || max_lag < 1 || x.len() < n || y.len() < n + 2 * max_lag {
        return res;
    }
    let x = &x[..n];
    let y = &y[..n + 2 * max_lag];

    let mut mx = 0.0f64;
    let mut my = 0.0f64;
    for &v in x {
        mx += f64::from(v);
    }
    for &v in y {
        my += f64::from(v);
    }
    mx /= n as f64;
    my /= (n + 2 * max_lag) as f64;

    let mut ex = 0.0f64;
    for &v in x {
        ex += (f64::from(v) - mx) * (f64::from(v) - mx);
    }
    if ex < 1.0e-12 {
        return res;
    }

    // energy of y under the window at lag 0 (offset 0 into y), slid along
    let mut ey = 0.0f64;
    for &v in &y[..n] {
        ey += (f64::from(v) - my) * (f64::from(v) - my);
    }

    let lags = 2 * max_lag + 1;
    let mut r = vec![0.0f64; lags];
    for k in 0..lags {
        if k > 0 {
            let out = f64::from(y[k - 1]) - my;
            let inp = f64::from(y[k + n - 1]) - my;
            ey += inp * inp - out * out;
        }
        let mut s = 0.0f64;
        for i in 0..n {
            s += (f64::from(x[i]) - mx) * (f64::from(y[i + k]) - my);
        }
        r[k] = if ey > 1.0e-12 { s / (ex * ey).sqrt() } else { 0.0 };
    }

    let mut best = 0usize;
    for k in 1..lags {
        if r[k].abs() > r[best].abs() {
            best = k;
        }
    }

    let mut frac = 0.0f64;
    if best > 0 && best < lags - 1 {
        let (a, b, c) = (r[best - 1].abs(), r[best].abs(), r[best + 1].abs());
        let den = a - 2.0 * b + c;
        if den.abs() > 1.0e-12 {
            frac = (0.5 * (a - c) / den).clamp(-0.5, 0.5);
        }
    }

    res.corr = r[best];
    res.samples = (best as f64 - max_lag as f64) + frac;
    // a peak at the edge is out of range
    res.ok = res.corr.abs() >= min_corr && best > 0 && best < lags - 1;
    res
}

/// [`measure`] spread over several calls, for the audio thread: the C++ measures on its message
/// thread, which a module does not have, so the plugin runs it a few lags per block instead
/// (`step`), never allocating (the windows and the correlations are sized by [`Measurer::reserve`]
/// in `prepare`). The arithmetic is [`measure`]'s, in the same order, so the result is the same
/// to the bit.
#[derive(Debug, Clone, Default)]
pub struct Measurer {
    x: Vec<f32>,
    y: Vec<f32>,
    r: Vec<f64>,
    n: usize,
    max_lag: usize,
    min_corr: f64,
    mx: f64,
    my: f64,
    ex: f64,
    ey: f64,
    /// The next lag to correlate; `None`: idle.
    k: Option<usize>,
}

impl Measurer {
    /// Room for windows of `n` samples and `max_lag` either side. Allocates (call in `prepare`);
    /// keeps what it has when that is enough.
    pub fn reserve(&mut self, n: usize, max_lag: usize) {
        let lags = 2 * max_lag + 1;
        for (v, len) in [(&mut self.x, n), (&mut self.y, n + 2 * max_lag)] {
            if v.len() < len {
                v.resize(len, 0.0);
            }
        }
        if self.r.len() < lags {
            self.r.resize(lags, 0.0);
        }
    }

    /// Whether `n` and `max_lag` fit what [`Measurer::reserve`] made.
    pub fn fits(&self, n: usize, max_lag: usize) -> bool {
        self.x.len() >= n && self.y.len() >= n + 2 * max_lag && self.r.len() > 2 * max_lag
    }

    /// The windows to fill before [`Measurer::start`]: `x` (this mic, `n`) and `y` (the reference,
    /// `n + 2 * max_lag`). `None` when they do not fit.
    pub fn windows(&mut self, n: usize, max_lag: usize) -> Option<(&mut [f32], &mut [f32])> {
        if !self.fits(n, max_lag) {
            return None;
        }
        Some((&mut self.x[..n], &mut self.y[..n + 2 * max_lag]))
    }

    /// Its heap blocks as (pointer, capacity), for the no-allocation tests.
    #[cfg(test)]
    pub(crate) fn heap(&self) -> [(usize, usize); 3] {
        [(self.x.as_ptr() as usize, self.x.capacity()), (self.y.as_ptr() as usize, self.y.capacity()), (self.r.as_ptr() as usize, self.r.capacity())]
    }

    /// Whether a measurement is under way.
    pub fn busy(&self) -> bool {
        self.k.is_some()
    }

    /// Drops a measurement under way.
    pub fn cancel(&mut self) {
        self.k = None;
    }

    /// Starts measuring the filled windows (`measure`'s set-up: the means and the energies). A
    /// measurement [`measure`] would refuse (too short, silent `x`, or windows that do not fit)
    /// finishes at once: [`Measurer::step`] then returns its `Lag` (`ok` false).
    pub fn start(&mut self, n: usize, max_lag: usize, min_corr: f64) {
        self.n = n;
        self.max_lag = max_lag;
        self.min_corr = min_corr;
        self.mx = 0.0;
        self.my = 0.0;
        self.ex = 0.0;
        self.ey = 0.0;
        if n < 16 || max_lag < 1 || !self.fits(n, max_lag) {
            self.k = Some(usize::MAX);
            return;
        }
        let x = &self.x[..n];
        let y = &self.y[..n + 2 * max_lag];
        for &v in x {
            self.mx += f64::from(v);
        }
        for &v in y {
            self.my += f64::from(v);
        }
        self.mx /= n as f64;
        self.my /= (n + 2 * max_lag) as f64;
        for &v in x {
            self.ex += (f64::from(v) - self.mx) * (f64::from(v) - self.mx);
        }
        if self.ex < 1.0e-12 {
            self.k = Some(usize::MAX);
            return;
        }
        for &v in &y[..n] {
            self.ey += (f64::from(v) - self.my) * (f64::from(v) - self.my);
        }
        self.k = Some(0);
    }

    /// Correlates the next lags, about `budget` multiply-adds' worth (at least one lag); the
    /// measurement once it is finished (then idle again), `None` while it is not.
    pub fn step(&mut self, budget: usize) -> Option<Lag> {
        let k0 = self.k?;
        let (n, max_lag) = (self.n, self.max_lag);
        if k0 == usize::MAX {
            self.k = None;
            return Some(Lag::default());
        }
        let lags = 2 * max_lag + 1;
        let end = (k0 + (budget / n.max(1)).max(1)).min(lags);
        let (mx, my, ex) = (self.mx, self.my, self.ex);
        let (x, y) = (&self.x[..n], &self.y[..n + 2 * max_lag]);
        for k in k0..end {
            if k > 0 {
                let out = f64::from(y[k - 1]) - my;
                let inp = f64::from(y[k + n - 1]) - my;
                self.ey += inp * inp - out * out;
            }
            let mut s = 0.0f64;
            for i in 0..n {
                s += (f64::from(x[i]) - mx) * (f64::from(y[i + k]) - my);
            }
            self.r[k] = if self.ey > 1.0e-12 { s / (ex * self.ey).sqrt() } else { 0.0 };
        }
        if end < lags {
            self.k = Some(end);
            return None;
        }
        self.k = None;
        let r = &self.r[..lags];
        let mut best = 0usize;
        for k in 1..lags {
            if r[k].abs() > r[best].abs() {
                best = k;
            }
        }
        let mut frac = 0.0f64;
        if best > 0 && best < lags - 1 {
            let (a, b, c) = (r[best - 1].abs(), r[best].abs(), r[best + 1].abs());
            let den = a - 2.0 * b + c;
            if den.abs() > 1.0e-12 {
                frac = (0.5 * (a - c) / den).clamp(-0.5, 0.5);
            }
        }
        let corr = r[best];
        Some(Lag { ok: corr.abs() >= self.min_corr && best > 0 && best < lags - 1, samples: (best as f64 - max_lag as f64) + frac, corr })
    }
}

/// `Estimator`: the last few good measurements, and what they agree on.
#[derive(Debug, Clone, Default)]
pub struct Estimator {
    lags: [f64; Estimator::KEEP],
    corrs: [f64; Estimator::KEEP],
    signs: [i32; Estimator::KEEP],
    count: usize,
    next: usize,
    rejected: i32,
}

impl Estimator {
    pub const KEEP: usize = 24;

    pub fn reset(&mut self) {
        self.count = 0;
        self.next = 0;
    }

    pub fn add(&mut self, l: &Lag) {
        if !l.ok {
            self.rejected = self.rejected.wrapping_add(1);
            return;
        }
        self.lags[self.next] = l.samples;
        self.signs[self.next] = if l.corr >= 0.0 { 1 } else { -1 };
        self.corrs[self.next] = l.corr.abs();
        self.next = (self.next + 1) % Self::KEEP;
        self.count = (self.count + 1).min(Self::KEEP);
    }

    pub fn size(&self) -> usize {
        self.count
    }

    pub fn rejected_count(&self) -> i32 {
        self.rejected
    }

    pub fn median(&self) -> f64 {
        self.median_of(&self.lags)
    }

    /// Median absolute deviation from the median, in samples.
    pub fn spread(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let m = self.median();
        let mut d = [0.0f64; Self::KEEP];
        for i in 0..self.count {
            d[i] = (self.lags[i] - m).abs();
        }
        self.median_of(&d)
    }

    /// +1 or -1: which polarity most of the hits agree on.
    pub fn polarity(&self) -> i32 {
        let s: i32 = self.signs[..self.count].iter().sum();
        if s >= 0 { 1 } else { -1 }
    }

    pub fn mean_corr(&self) -> f64 {
        let s: f64 = self.corrs[..self.count].iter().sum();
        if self.count > 0 { s / self.count as f64 } else { 0.0 }
    }

    /// `std::nth_element (v.begin(), v.begin() + count / 2, v.begin() + count)`, then that
    /// element: the `count / 2`-th smallest of the first `count`.
    fn median_of(&self, a: &[f64; Self::KEEP]) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let mut v = *a;
        let head = &mut v[..self.count];
        head.sort_by(f64::total_cmp);
        head[self.count / 2]
    }
}

/// `AlignDelay`: a fractional delay of `base + offset` (Catmull-Rom), with a polarity flip.
#[derive(Debug, Clone)]
pub struct AlignDelay {
    sr: f64,
    current: f64,
    target: f64,
    gain_now: f64,
    gain_want: f64,
    base: i32,
    cap: i32,
    write: i32,
    ring: [Vec<f32>; 2],
}

impl Default for AlignDelay {
    fn default() -> Self {
        AlignDelay {
            sr: 48000.0,
            current: 0.0,
            target: 0.0,
            gain_now: 1.0,
            gain_want: 1.0,
            base: 484,
            cap: 2048,
            write: 0,
            ring: [Vec::new(), Vec::new()],
        }
    }
}

impl AlignDelay {
    /// `prepare`. Allocates only when the ring has to grow (a live re-prepare at the same rate
    /// and block size does not).
    pub fn prepare(&mut self, sample_rate: f64, max_block: i32) {
        self.sr = if sample_rate > 0.0 { sample_rate } else { 48000.0 };
        self.base = (MAX_LAG_MS * 0.001 * self.sr).ceil() as i32 + 4;
        self.cap = 2 * self.base + max_block.max(1) + 16;
        for b in &mut self.ring {
            b.clear();
            b.resize(self.cap.max(1) as usize, 0.0);
        }
        self.write = 0;
        self.current = f64::from(self.base);
        self.target = f64::from(self.base);
        self.gain_now = 1.0;
    }

    /// Latency to report while on.
    pub fn latency(&self) -> i32 {
        self.base
    }

    /// `offset_samples`: + delays this mic (it was early), - brings it forward.
    pub fn set_target(&mut self, offset_samples: f64, flip: bool) {
        self.target = (f64::from(self.base) + offset_samples).clamp(2.0, f64::from(2 * self.base));
        self.gain_want = if flip { -1.0 } else { 1.0 };
    }

    /// Rust-only (live reset): start the delay and the polarity at their targets, so a transport
    /// reset neither glides the offset in from zero nor crossfades the polarity again.
    pub fn seat_at_target(&mut self) {
        self.current = self.target;
        self.gain_now = self.gain_want;
    }

    pub fn process(&mut self, ch: &mut [Vec<f32>], num_channels: usize, n: usize) {
        let num_channels = num_channels.min(2).min(ch.len());
        let n = ch.iter().take(num_channels).map(Vec::len).fold(n, usize::min);
        let cap = self.cap.max(1);
        for i in 0..n {
            // move the delay no faster than 1/8 sample per sample
            let step = (self.target - self.current).clamp(-0.125, 0.125);
            self.current += step;
            self.gain_now += (self.gain_want - self.gain_now) * 0.002; // ~10 ms polarity crossfade

            let read_pos = f64::from(self.write) - self.current;
            let ip = read_pos.floor() as i32;
            let f = read_pos - f64::from(ip);

            for c in 0..num_channels {
                let r = &mut self.ring[c];
                if let Some(s) = r.get_mut(self.write.rem_euclid(cap) as usize) {
                    *s = ch[c][i];
                }
                let at = |k: i32| f64::from(r.get(k.rem_euclid(cap) as usize).copied().unwrap_or(0.0));
                // 4-point cubic (Catmull-Rom)
                let (xm1, x0, x1, x2) = (at(ip - 1), at(ip), at(ip + 1), at(ip + 2));
                let y = x0 + f * ((x1 - xm1) * 0.5 + f * ((xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2) + f * (1.5 * (x0 - x1) + 0.5 * (x2 - xm1))));
                ch[c][i] = (y * self.gain_now) as f32;
            }
            self.write = (self.write + 1) % cap;
        }
    }

    pub fn current(&self) -> f64 {
        self.current
    }
    pub fn gain_now(&self) -> f64 {
        self.gain_now
    }
    pub fn target(&self) -> f64 {
        self.target
    }
    pub fn gain_want(&self) -> f64 {
        self.gain_want
    }

    /// The rings as (pointer, capacity) (allocation tests).
    #[cfg(test)]
    pub(crate) fn heap(&self) -> [(*const f32, usize); 2] {
        [(self.ring[0].as_ptr(), self.ring[0].capacity()), (self.ring[1].as_ptr(), self.ring[1].capacity())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_knob_maps_round_trip() {
        assert_eq!(knob_to_offset_ms(0.5), 0.0);
        assert_eq!(knob_to_offset_ms(0.0), -10.0);
        assert_eq!(knob_to_offset_ms(1.0), 10.0);
        assert!((offset_ms_to_knob(2.5) - 0.625).abs() < 1e-12);
        assert_eq!(offset_ms_to_knob(50.0), 1.0);
    }

    fn click(n: usize, at: usize, sign: f32) -> Vec<f32> {
        // A decaying burst: correlation peaks where it lands.
        (0..n).map(|i| if i >= at { sign * (-((i - at) as f32) / 20.0).exp() * ((i - at) as f32 * 0.7).sin() } else { 0.0 }).collect()
    }

    #[test]
    fn measure_finds_the_lag_and_the_polarity() {
        let (n, max_lag) = (1536, 480);
        let x = click(n, 576, 1.0);
        // The reference hears it 37 samples later, upside down.
        let y = click(n + 2 * max_lag, max_lag + 576 + 37, -1.0);
        let l = measure(&x, n, &y, max_lag, 0.3);
        assert!(l.ok);
        assert!((l.samples - 37.0).abs() < 0.5, "{l:?}");
        assert!(l.corr < -0.9, "{l:?}");
        // Too short, flat or out-of-range input measures nothing.
        assert!(!measure(&x, 8, &y, max_lag, 0.3).ok);
        assert!(!measure(&vec![0.5; n], n, &y, max_lag, 0.3).ok);
        assert!(!measure(&x, n, &y[..10], max_lag, 0.3).ok);
    }

    #[test]
    fn the_measurer_in_steps_is_measure_to_the_bit() {
        let (n, max_lag) = (1536, 480);
        let noise = |i: usize| ((i.wrapping_mul(2_654_435_761) % 1000) as f32 / 1000.0 - 0.5) * 0.05;
        let x: Vec<f32> = click(n, 576, 1.0).iter().enumerate().map(|(i, v)| v + noise(i)).collect();
        let y: Vec<f32> = click(n + 2 * max_lag, max_lag + 576 - 23, -0.7).iter().enumerate().map(|(i, v)| v + noise(i + 7)).collect();
        let want = measure(&x, n, &y, max_lag, 0.3);
        assert!(want.ok, "{want:?}");
        let mut m = Measurer::default();
        m.reserve(n, max_lag);
        for budget in [0, 1, 1536 * 7, 65_536, usize::MAX / 4] {
            let (wx, wy) = m.windows(n, max_lag).unwrap();
            wx.copy_from_slice(&x);
            wy.copy_from_slice(&y);
            m.start(n, max_lag, 0.3);
            assert!(m.busy());
            let mut steps = 0;
            let got = loop {
                steps += 1;
                if let Some(l) = m.step(budget) {
                    break l;
                }
            };
            assert_eq!(got, want, "budget {budget}");
            assert!(!m.busy());
            if budget <= 1536 {
                assert_eq!(steps, 2 * max_lag + 1, "one lag per step at the least");
            }
        }
        // What `measure` refuses finishes at once, not ok; windows that do not fit are refused.
        m.start(8, max_lag, 0.3);
        assert_eq!(m.step(1), Some(Lag::default()));
        assert!(m.windows(n + 1, max_lag).is_none());
        m.start(n + 1, max_lag, 0.3);
        assert_eq!(m.step(1), Some(Lag::default()));
        let (wx, _) = m.windows(n, max_lag).unwrap();
        wx.fill(0.25);
        m.start(n, max_lag, 0.3);
        assert_eq!(m.step(1), Some(Lag::default()));
        assert_eq!(m.step(1), None, "idle");
        m.start(n, max_lag, 0.3);
        m.cancel();
        assert!(!m.busy());
    }

    #[test]
    fn estimator_takes_the_median_and_the_majority_polarity() {
        let mut e = Estimator::default();
        assert_eq!(e.median(), 0.0);
        for (s, c) in [(10.0, 0.9), (11.0, -0.8), (50.0, 0.7), (12.0, 0.95), (9.0, 0.6)] {
            e.add(&Lag { ok: true, samples: s, corr: c });
        }
        e.add(&Lag { ok: false, samples: 999.0, corr: 1.0 });
        assert_eq!(e.size(), 5);
        assert_eq!(e.rejected_count(), 1);
        assert_eq!(e.median(), 11.0);
        assert_eq!(e.spread(), 1.0);
        assert_eq!(e.polarity(), 1);
        assert!((e.mean_corr() - 0.79).abs() < 1e-12);
        for _ in 0..40 {
            e.add(&Lag { ok: true, samples: 3.0, corr: -0.5 });
        }
        assert_eq!(e.size(), Estimator::KEEP);
        assert_eq!(e.median(), 3.0);
        assert_eq!(e.polarity(), -1);
        e.reset();
        assert_eq!(e.size(), 0);
    }

    #[test]
    fn align_delay_delays_by_base_plus_offset_and_flips() {
        let mut d = AlignDelay::default();
        d.prepare(48000.0, 512);
        assert_eq!(d.latency(), 484);
        d.set_target(10.0, true);
        d.seat_at_target();
        let mut io = vec![vec![0.0f32; 1024]];
        io[0][0] = 1.0;
        d.process(&mut io, 1, 1024);
        let peak = io[0].iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i).unwrap();
        assert_eq!(peak, 494);
        assert!(io[0][494] < -0.99);
    }
}

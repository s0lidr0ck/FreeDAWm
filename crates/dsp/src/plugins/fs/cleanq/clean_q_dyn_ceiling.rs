//! CleanQDynCeiling -- the ceiling, applied in real time.
//! Line-by-line translation of `CleanQ/Source/CleanQDynCeiling.h`.
//!
//! Conform turns the ceiling into a static EQ. This is the other half, soothe-style: it watches the
//! spectrum moment by moment and cuts only where, and only while, a moment pushes up through the
//! ceiling. Per frame (an STFT: 2048 points at 44.1/48 kHz, a new frame every N/8 samples):
//!
//! 1. The frame's power spectrum, smoothed onto the ceiling's grid (third-octave), in dB.
//! 2. Minus the ceiling.
//! 3. Minus its own MEDIAN (shape, not level).
//! 4. Wherever that is more than Threshold dB above zero, the excess is a cut request, scaled by
//!    Amount and limited to Max cut.
//! 5. Each grid point's cut follows its request with Attack and Release, and is spread back onto
//!    the FFT bins by interpolation on log frequency.
//!
//! It runs AFTER the static EQ. Stereo is linked: both channels get the same cut, and one complex
//! FFT carries both channels (left real, right imaginary).
//!
//! Latency: the frame length N, reported to the host only while it is switched on.
//!
//! Translation notes: `std::complex<double>` is the small [`Cx`] below, with the MSVC operator
//! formulas (`(a*c - b*d, a*d + b*c)`, `norm = re² + im²`, `polar(1, t) = (cos t, sin t)`).
//! `std::nth_element` is `select_nth_unstable_by` (same n-th value). `prepare` allocates every
//! buffer for the sample rate; `process` and `reset` never allocate. The display atomics
//! (`cutView`, `active`) are plain fields here.

use super::clean_q_ceiling::{self as ceiling, GRID_N};

/// `3.14159265358979323846` in the C++: the same double as `std::f64::consts::PI`.
const PI_DC: f64 = std::f64::consts::PI;

/// `std::complex<double>`, with the operators the C++ uses.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Cx {
    pub re: f64,
    pub im: f64,
}

impl Cx {
    #[inline]
    const fn new(re: f64, im: f64) -> Self {
        Cx { re, im }
    }
    #[inline]
    fn add(self, o: Cx) -> Cx {
        Cx::new(self.re + o.re, self.im + o.im)
    }
    #[inline]
    fn sub(self, o: Cx) -> Cx {
        Cx::new(self.re - o.re, self.im - o.im)
    }
    #[inline]
    fn mul(self, o: Cx) -> Cx {
        Cx::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
    #[inline]
    fn scale(self, s: f64) -> Cx {
        Cx::new(self.re * s, self.im * s)
    }
    #[inline]
    fn conj(self) -> Cx {
        Cx::new(self.re, -self.im)
    }
    #[inline]
    fn norm(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub on: bool,
    /// How far above the ceiling a moment may go.
    pub threshold_db: f64,
    /// 0..1 of the excess removed.
    pub amount: f64,
    pub max_cut_db: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub delta: bool,
    /// Pass through (at the same latency).
    pub bypass: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { on: false, threshold_db: 3.0, amount: 0.7, max_cut_db: 9.0, attack_ms: 5.0, release_ms: 120.0, delta: false, bypass: false }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct BinWeight {
    k: usize,
    w: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct GridPos {
    i0: usize,
    i1: usize,
    t: f64,
}

pub fn frame_size_for(sr: f64) -> usize {
    if sr <= 50000.0 {
        2048
    } else if sr <= 100000.0 {
        4096
    } else {
        8192
    }
}

pub struct DynCeiling {
    set: Settings,
    sr: f64,
    ola_scale: f64,
    power_scale: f64,
    n: usize,
    h: usize,
    log2n: i32,
    write_pos: usize,
    hop_count: usize,
    out_pos: usize,
    grid_lo: usize,
    grid_hi: usize,

    window: Vec<f64>,
    in_l: Vec<f64>,
    in_r: Vec<f64>,
    out_l: Vec<f64>,
    out_r: Vec<f64>,
    bin_power: Vec<f64>,
    bin_gain: Vec<f64>,
    buf: Vec<Cx>,
    twiddle: Vec<Cx>,
    grid_bins: Vec<Vec<BinWeight>>,
    bin_to_grid: Vec<GridPos>,

    has_ceiling: bool,
    ceil: [f64; GRID_N],
    spec: [f64; GRID_N],
    spec_pow: [f64; GRID_N],
    diffs: [f64; GRID_N],
    cut: [f64; GRID_N],
    cut_view: [f32; GRID_N],
    active: bool,
}

impl Default for DynCeiling {
    fn default() -> Self {
        Self::new()
    }
}

impl DynCeiling {
    /// Unprepared, as the C++ member is until `prepareToPlay`: call `prepare` before `process`
    /// (an unprepared `process` returns without touching the buffers).
    pub fn new() -> Self {
        DynCeiling {
            set: Settings::default(),
            sr: 48000.0,
            ola_scale: 0.25,
            power_scale: 1.0,
            n: 2048,
            h: 256,
            log2n: 11,
            write_pos: 0,
            hop_count: 0,
            out_pos: 0,
            grid_lo: 0,
            grid_hi: GRID_N - 1,
            window: Vec::new(),
            in_l: Vec::new(),
            in_r: Vec::new(),
            out_l: Vec::new(),
            out_r: Vec::new(),
            bin_power: Vec::new(),
            bin_gain: Vec::new(),
            buf: Vec::new(),
            twiddle: Vec::new(),
            grid_bins: (0..GRID_N).map(|_| Vec::new()).collect(),
            bin_to_grid: Vec::new(),
            has_ceiling: false,
            ceil: [0.0; GRID_N],
            spec: [0.0; GRID_N],
            spec_pow: [0.0; GRID_N],
            diffs: [0.0; GRID_N],
            cut: [0.0; GRID_N],
            cut_view: [0.0; GRID_N],
            active: false,
        }
    }

    pub fn prepare(&mut self, sample_rate: f64) {
        self.sr = if sample_rate > 0.0 { sample_rate } else { 48000.0 };
        self.n = frame_size_for(self.sr);
        self.h = self.n / 8;
        self.log2n = (self.n as f64).log2().round() as i32;
        let n = self.n;

        self.window.clear();
        self.window.extend((0..n).map(|i| (0.5 - 0.5 * (2.0 * PI_DC * i as f64 / n as f64).cos()).sqrt())); // sqrt-Hann, periodic

        // sqrt-Hann in and out is Hann overall; at 8x overlap Hann sums to 4
        self.ola_scale = 1.0 / 4.0;

        let mut wsum = 0.0;
        for &w in &self.window {
            wsum += w;
        }
        self.power_scale = 2.0 / wsum; // a full-scale sine reads 0 dB

        self.twiddle.clear();
        self.twiddle.extend((0..n / 2).map(|k| {
            let t = -2.0 * PI_DC * k as f64 / n as f64;
            Cx::new(t.cos(), t.sin())
        }));

        for v in [&mut self.in_l, &mut self.in_r, &mut self.out_l, &mut self.out_r] {
            v.clear();
            v.resize(n, 0.0);
        }
        self.buf.clear();
        self.buf.resize(n, Cx::default());
        self.bin_power.clear();
        self.bin_power.resize(n / 2 + 1, 0.0);
        self.bin_gain.clear();
        self.bin_gain.resize(n / 2 + 1, 1.0);
        self.write_pos = 0;
        self.hop_count = 0;
        self.out_pos = 0;

        self.build_grid_maps();
        self.cut = [0.0; GRID_N];
        self.spec_pow = [0.0; GRID_N];
        self.cut_view = [0.0; GRID_N];
        self.active = false;
    }

    pub fn latency_samples(&self) -> usize {
        self.n
    }

    /// Forget everything in flight, without allocating: for switching it on.
    pub fn reset(&mut self) {
        for v in [&mut self.in_l, &mut self.in_r, &mut self.out_l, &mut self.out_r] {
            v.iter_mut().for_each(|x| *x = 0.0);
        }
        self.write_pos = 0;
        self.hop_count = 0;
        self.out_pos = 0;
        self.cut = [0.0; GRID_N];
        self.spec_pow = [0.0; GRID_N];
        self.cut_view = [0.0; GRID_N];
        self.active = false;
    }

    /// The ceiling on the grid, in dBFS. Empty or wrong-sized: nothing to hold to. No allocation.
    pub fn set_ceiling(&mut self, grid_db: &[f64]) {
        self.has_ceiling = grid_db.len() == GRID_N;
        if self.has_ceiling {
            self.ceil.copy_from_slice(grid_db);
        }
    }

    /// Whether a ceiling is set (`set_ceiling` with a full grid).
    pub fn has_ceiling(&self) -> bool {
        self.has_ceiling
    }

    pub fn set_settings(&mut self, s: &Settings) {
        self.set = *s;
    }

    /// Stereo in place. `r` may be `None` for mono.
    pub fn process(&mut self, l: &mut [f32], mut r: Option<&mut [f32]>, num_samples: usize) {
        let n = self.n;
        if self.in_l.len() != n || self.out_l.len() != n || self.buf.len() != n || self.h == 0 {
            return; // not prepared
        }
        let len = num_samples.min(l.len()).min(r.as_ref().map_or(usize::MAX, |r| r.len()));

        for i in 0..len {
            let xl = f64::from(l[i]);
            let xr = match &r {
                Some(r) => f64::from(r[i]),
                None => xl,
            };

            // output: what the overlap-add finished for this sample
            let yl = self.out_l[self.out_pos];
            let yr = self.out_r[self.out_pos];
            self.out_l[self.out_pos] = 0.0;
            self.out_r[self.out_pos] = 0.0;
            self.out_pos = (self.out_pos + 1) % n;

            l[i] = yl as f32;
            if let Some(rr) = r.as_deref_mut() {
                rr[i] = yr as f32;
            }

            self.in_l[self.write_pos] = xl;
            self.in_r[self.write_pos] = xr;
            self.write_pos = (self.write_pos + 1) % n;

            self.hop_count += 1;
            if self.hop_count == self.h {
                self.hop_count = 0;
                self.frame();
            }
        }
    }

    /// The cut at each grid point right now, dB (0 or negative), for the display.
    pub fn cut_at(&self, grid_index: usize) -> f32 {
        self.cut_view.get(grid_index).copied().unwrap_or(0.0)
    }
    pub fn is_cutting(&self) -> bool {
        self.active
    }

    /// Addresses and capacities of every heap buffer, for the no-allocation test.
    #[cfg(test)]
    pub(crate) fn buffer_fingerprint(&self) -> Vec<(usize, usize)> {
        let mut v = vec![
            (self.window.as_ptr() as usize, self.window.capacity()),
            (self.in_l.as_ptr() as usize, self.in_l.capacity()),
            (self.in_r.as_ptr() as usize, self.in_r.capacity()),
            (self.out_l.as_ptr() as usize, self.out_l.capacity()),
            (self.out_r.as_ptr() as usize, self.out_r.capacity()),
            (self.bin_power.as_ptr() as usize, self.bin_power.capacity()),
            (self.bin_gain.as_ptr() as usize, self.bin_gain.capacity()),
            (self.buf.as_ptr() as usize, self.buf.capacity()),
            (self.twiddle.as_ptr() as usize, self.twiddle.capacity()),
            (self.bin_to_grid.as_ptr() as usize, self.bin_to_grid.capacity()),
        ];
        v.extend(self.grid_bins.iter().map(|b| (b.as_ptr() as usize, b.capacity())));
        v
    }

    // ------------------------------------------------------------------ the frame
    fn frame(&mut self) {
        let n = self.n;

        // oldest .. newest, windowed; left in the real part, right in the imaginary
        for i in 0..n {
            let k = (self.write_pos + i) % n;
            self.buf[i] = Cx::new(self.in_l[k] * self.window[i], self.in_r[k] * self.window[i]);
        }
        self.fft(false);

        // |L|^2 + |R|^2 per bin, from the packed transform
        for k in 0..=n / 2 {
            let a = self.buf[k];
            let b = self.buf[(n - k) % n].conj();
            let l = a.add(b).scale(0.5); // `0.5 * (a + b)`
            let r = Cx::new(0.0, -0.5).mul(a.sub(b));
            self.bin_power[k] = (l.norm() + r.norm()) * self.power_scale * self.power_scale * 0.5;
        }

        self.compute_cut();

        // the gain on every bin: interpolated from the grid on log frequency
        for k in 0..=n / 2 {
            let m = self.bin_to_grid[k];
            let db = self.cut[m.i0] * (1.0 - m.t) + self.cut[m.i1] * m.t;
            let mut g = (db * 0.11512925464970229).exp();
            if self.set.bypass || !self.set.on {
                g = 1.0;
            } else if self.set.delta {
                g = 1.0 - g;
            }
            self.bin_gain[k] = g;
        }

        for k in 0..n {
            let g = self.bin_gain[if k <= n / 2 { k } else { n - k }];
            self.buf[k] = self.buf[k].scale(g);
        }

        self.fft(true);

        for i in 0..n {
            let w = self.window[i] * self.ola_scale / n as f64;
            let o = (self.out_pos + i) % n;
            self.out_l[o] += self.buf[i].re * w;
            self.out_r[o] += self.buf[i].im * w;
        }
    }

    fn compute_cut(&mut self) {
        let frame_sec = self.h as f64 / self.sr;
        let att = (-frame_sec / 0.0005f64.max(self.set.attack_ms * 0.001)).exp();
        let rel = (-frame_sec / 0.001f64.max(self.set.release_ms * 0.001)).exp();

        // the frame on the grid, lightly averaged over time (about 15 ms)
        let det = (-frame_sec / 0.015).exp();
        let mut total = 0.0;
        for i in 0..GRID_N {
            let mut p = 0.0;
            for bw in &self.grid_bins[i] {
                p += bw.w * self.bin_power[bw.k];
            }
            let sm = &mut self.spec_pow[i];
            *sm = p + (*sm - p) * det;
            total += *sm;
            self.spec[i] = if *sm > 1.0e-12 { 10.0 * sm.log10() } else { -120.0 };
        }

        // against the ceiling, level taken out by the median, over the range that is there
        let mut n = 0usize;
        let live = self.set.on && self.has_ceiling && total > 1.0e-9; // about -90 dBFS: silence asks for nothing
        if live {
            for i in self.grid_lo..=self.grid_hi {
                if self.spec[i] > -100.0 {
                    self.diffs[n] = self.spec[i] - self.ceil[i];
                    n += 1;
                }
            }
        }

        let mut med = 0.0;
        if n > 8 {
            let (_, m, _) = self.diffs[..n].select_nth_unstable_by(n / 2, f64::total_cmp);
            med = *m;
        }

        let mut any = false;
        for i in 0..GRID_N {
            let mut want = 0.0;
            if n > 8 && i >= self.grid_lo && i <= self.grid_hi && self.spec[i] > -100.0 {
                let excess = self.spec[i] - self.ceil[i] - med - self.set.threshold_db;
                if excess > 0.0 {
                    want = -self.set.max_cut_db.min(excess * self.set.amount);
                }
            }

            let c = &mut self.cut[i];
            *c = if want < *c { want + (*c - want) * att } else { want + (*c - want) * rel };
            if *c > -0.01 {
                *c = 0.0;
            }
            if *c < -0.1 {
                any = true;
            }
            self.cut_view[i] = *c as f32;
        }
        self.active = any;
    }

    // ------------------------------------------------------------------ maps
    fn build_grid_maps(&mut self) {
        let bin_hz = self.sr / self.n as f64;
        let nb = self.n / 2 + 1;

        // each grid point: the mean power over a third of an octave, weighted by overlap
        let half = 2.0f64.powf(0.5 / 3.0);
        for (i, list) in self.grid_bins.iter_mut().enumerate() {
            list.clear();
            // At least 4 bins wide: one frame's estimate from one or two bins swings by 5 dB.
            let f = ceiling::grid_hz(i);
            let c = f / bin_hz;
            let w = 4.0f64.max(f * (half - 1.0 / half) / bin_hz);
            let lo = 0.5f64.max(c - 0.5 * w);
            let hi = c + 0.5 * w;
            {
                let mut wsum = 0.0;
                let k0 = 1i64.max((lo + 0.5).floor() as i64);
                let k1 = (nb as i64 - 1).min((hi - 0.5).ceil() as i64);
                for k in k0..=k1 {
                    let w = 0.0f64.max(hi.min(k as f64 + 0.5) - lo.max(k as f64 - 0.5));
                    if w > 0.0 {
                        list.push(BinWeight { k: k as usize, w });
                        wsum += w;
                    }
                }
                for bw in list.iter_mut() {
                    bw.w /= 1.0e-12f64.max(wsum);
                }
            }
        }

        // where each bin falls on the grid
        self.bin_to_grid.clear();
        self.bin_to_grid.resize(nb, GridPos::default());
        let span = (ceiling::GRID_HI / ceiling::GRID_LO).ln();
        for (k, g) in self.bin_to_grid.iter_mut().enumerate() {
            let f = ceiling::GRID_LO.max(k as f64 * bin_hz);
            let pos = ((f / ceiling::GRID_LO).ln() / span * (GRID_N - 1) as f64).clamp(0.0, (GRID_N - 1) as f64);
            g.i0 = pos.floor() as usize;
            g.i1 = (g.i0 + 1).min(GRID_N - 1);
            g.t = pos - g.i0 as f64;
        }

        // the range it works over: 30 Hz up to 16 kHz or just under Nyquist
        self.grid_lo = 0;
        self.grid_hi = GRID_N - 1;
        while self.grid_lo < GRID_N - 1 && ceiling::grid_hz(self.grid_lo) < 30.0 {
            self.grid_lo += 1;
        }
        while self.grid_hi > 0 && ceiling::grid_hz(self.grid_hi) > 16000.0f64.min(self.sr * 0.45) {
            self.grid_hi -= 1;
        }
    }

    // ------------------------------------------------------------------ FFT (radix 2, in place)
    fn fft(&mut self, inverse: bool) {
        let n = self.n;
        let a = &mut self.buf;
        let mut j = 0usize;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j ^= bit;
            if i < j {
                a.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            let mut i = 0;
            while i < n {
                for k in 0..len / 2 {
                    let mut w = self.twiddle[k * step];
                    if inverse {
                        w = w.conj();
                    }
                    let u = a[i + k];
                    let v = a[i + k + len / 2].mul(w);
                    a[i + k] = u.add(v);
                    a[i + k + len / 2] = u.sub(v);
                }
                i += len;
            }
            len <<= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(len: usize, f: f64, sr: f64, amp: f32) -> Vec<f32> {
        (0..len).map(|i| amp * (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin() as f32).collect()
    }

    #[test]
    fn without_a_ceiling_it_is_a_pure_delay_of_n() {
        for sr in [44100.0, 48000.0, 96000.0, 192000.0] {
            let mut d = DynCeiling::new();
            d.prepare(sr);
            d.set_settings(&Settings { on: true, ..Settings::default() });
            let n = d.latency_samples();
            assert_eq!(n, frame_size_for(sr));
            let src = sine(4 * n, 997.0, sr, 0.5);
            let mut l = src.clone();
            let mut r = src.clone();
            d.process(&mut l, Some(&mut r), 4 * n);
            for i in 2 * n..4 * n {
                assert!((l[i] - src[i - n]).abs() < 1e-5, "sr {sr} i {i}: {} vs {}", l[i], src[i - n]);
            }
        }
    }

    #[test]
    fn a_ceiling_below_a_tone_cuts_it() {
        let mut d = DynCeiling::new();
        d.prepare(48000.0);
        d.set_settings(&Settings { on: true, ..Settings::default() });
        // A flat ceiling, with pink noise underneath and a loud 2 kHz tone poking up through it.
        d.set_ceiling(&super::super::clean_q_ceiling::flat_ceiling(-60.0).to_grid());
        let mut seed = 1u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let tone = sine(48000, 2000.0, 48000.0, 0.5);
        let mut l: Vec<f32> = tone.iter().map(|t| t + 0.05 * noise()).collect();
        let mut r = l.clone();
        d.process(&mut l, Some(&mut r), 48000);
        assert!(d.is_cutting());
        let idx = (0..GRID_N).min_by(|&a, &b| (ceiling::grid_hz(a) - 2000.0).abs().total_cmp(&(ceiling::grid_hz(b) - 2000.0).abs())).unwrap_or(0);
        assert!(d.cut_at(idx) < -3.0, "{}", d.cut_at(idx));
        assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()));
    }

    #[test]
    fn unprepared_process_is_a_no_op() {
        let mut d = DynCeiling::new();
        let mut l = vec![0.5f32; 64];
        d.process(&mut l, None, 64);
        assert_eq!(l, vec![0.5f32; 64]);
    }
}

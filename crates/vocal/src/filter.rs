//! Second-order Butterworth low-pass and zero-phase `filtfilt` (scipy's `butter(2, …)` and
//! `filtfilt(…, padlen=…)` with odd extension and `lfilter_zi` initial conditions).

/// Biquad coefficients `(b, a)` with `a[0] == 1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    pub b: [f64; 3],
    pub a: [f64; 3],
}

/// Bilinear second-order Butterworth low-pass at `fc` Hz for sample rate `fs` Hz.
pub fn butter2_lowpass(fc: f64, fs: f64) -> Biquad {
    let k = (std::f64::consts::PI * fc / fs).tan();
    let k2 = k * k;
    let s2 = std::f64::consts::SQRT_2;
    let norm = 1.0 / (1.0 + s2 * k + k2);
    Biquad { b: [k2 * norm, 2.0 * k2 * norm, k2 * norm], a: [1.0, 2.0 * (k2 - 1.0) * norm, (1.0 - s2 * k + k2) * norm] }
}

impl Biquad {
    /// scipy `lfilter_zi`: the transposed-direct-form-II state for a unit-step steady state.
    fn zi(&self) -> [f64; 2] {
        let [b0, b1, b2] = self.b;
        let [_, a1, a2] = self.a;
        let (c0, c1) = (b1 - a1 * b0, b2 - a2 * b0);
        let den = 1.0 + a1 + a2;
        let z0 = if den != 0.0 { (c0 + c1) / den } else { 0.0 };
        [z0, c1 - a2 * z0]
    }

    /// scipy `lfilter` with initial state `z`, in place.
    fn run(&self, x: &mut [f64], mut z: [f64; 2]) {
        let [b0, b1, b2] = self.b;
        let [_, a1, a2] = self.a;
        for v in x.iter_mut() {
            let xi = *v;
            let y = b0 * xi + z[0];
            z[0] = b1 * xi - a1 * y + z[1];
            z[1] = b2 * xi - a2 * y;
            *v = y;
        }
    }
}

/// Zero-phase forward-backward filtering of `x` with odd extension of `padlen` samples
/// (clamped to `len − 1`), as scipy's `filtfilt(b, a, x, padlen=padlen)`.
pub fn filtfilt(f: &Biquad, x: &[f64], padlen: usize) -> Vec<f64> {
    let n = x.len();
    let (Some(&first), Some(&last)) = (x.first(), x.last()) else { return Vec::new() };
    let pad = padlen.min(n - 1);
    let mut ext = Vec::with_capacity(n + 2 * pad);
    ext.extend((1..=pad).rev().map(|i| 2.0 * first - x.get(i).copied().unwrap_or(first)));
    ext.extend_from_slice(x);
    ext.extend((1..=pad).map(|i| 2.0 * last - x.get(n - 1 - i).copied().unwrap_or(last)));

    let zi = f.zi();
    let x0 = ext.first().copied().unwrap_or(0.0);
    f.run(&mut ext, [zi[0] * x0, zi[1] * x0]);
    ext.reverse();
    let y0 = ext.first().copied().unwrap_or(0.0);
    f.run(&mut ext, [zi[0] * y0, zi[1] * y0]);
    ext.reverse();
    ext.get(pad..pad + n).map(<[f64]>::to_vec).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn butter_matches_scipy() {
        // scipy.signal.butter(2, 2.5 / 100).
        let f = butter2_lowpass(2.5, 200.0);
        let want_b = [0.001_460_316_3, 0.002_920_632_6, 0.001_460_316_3];
        let want_a = [1.0, -1.889_033_079_4, 0.894_874_344_6];
        for i in 0..3 {
            assert!((f.b[i] - want_b[i]).abs() < 1e-7, "b{i} {}", f.b[i]);
            assert!((f.a[i] - want_a[i]).abs() < 1e-7, "a{i} {}", f.a[i]);
        }
    }

    #[test]
    fn filtfilt_matches_scipy() {
        let f = butter2_lowpass(2.5, 200.0);
        let c = filtfilt(&f, &[3.0; 40], 12);
        assert!(c.iter().all(|v| (v - 3.0).abs() < 1e-9));
        // scipy.signal.filtfilt(b, a, 10·sin(0.7·k) for k in 0..20, padlen=12).
        let want = [
            -0.978_851_748_7,
            -0.716_280_377_1,
            -0.458_635_670_2,
            -0.206_330_403_8,
            0.040_532_810_9,
            0.282_115_616_6,
            0.518_677_667_6,
            0.750_363_811_3,
            0.977_040_444_6,
            1.198_253_763_8,
            1.413_325_395_9,
            1.621_536_389_3,
            1.822_309_091_0,
            2.015_297_545_6,
            2.200_340_190_4,
            2.377_293_512_4,
            2.545_821_450_6,
            2.705_236_284_1,
            2.854_462_683_4,
            2.992_138_843_9,
        ];
        let x: Vec<f64> = (0..20).map(|k| 10.0 * (0.7 * f64::from(k)).sin()).collect();
        let y = filtfilt(&f, &x, 12);
        assert_eq!(y.len(), 20);
        for (k, (g, w)) in y.iter().zip(want).enumerate() {
            assert!((g - w).abs() < 1e-6, "k {k}: {g} want {w}");
        }
        assert!(filtfilt(&f, &[], 12).is_empty());
        let one = filtfilt(&f, &[5.0], 12);
        assert!(one.len() == 1 && (one[0] - 5.0).abs() < 1e-9, "{one:?}");
    }
}

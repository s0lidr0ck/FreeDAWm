//! Heft: a bass enhancer. FreeDAWm's own design: low notes are heard on small speakers that cannot
//! play them, because the ear hears a fundamental from its harmonics.
//!
//! Per channel (every channel of the track, each on its own):
//!
//! 1. **The band below Frequency** (40..250 Hz) is taken out with a fourth-order low-pass
//!    (Linkwitz-Riley) and only it makes harmonics, so nothing above it (a kick's click, a voice)
//!    is distorted.
//! 2. **Harmonics** are made level-independently: the band is divided by its own envelope (fast
//!    up, slower down) so it swings about ±1, run through the second and third Chebyshev
//!    polynomials (`2x² - 1` makes exactly the second harmonic of a full-scale sine, `4x³ - 3x`
//!    exactly the third, neither the fundamental), blended by **Character** (0 % even, the
//!    second: warm; 100 % odd, the third: firmer), and multiplied by the envelope again, so they
//!    follow the bass's level. The polynomials are at most cubic and the band is below 250 Hz, so
//!    nothing reaches the Nyquist at any rate: no oversampling, no aliasing, no latency.
//! 3. The harmonics are high-passed at Frequency (eight poles: a harmonic that would land below
//!    it is left out) and added in, scaled by **Harmonics** (0..100 %: up to the band's own level).
//! 4. **Original Bass** keeps (100 %), reduces or removes (0 %) the band below Frequency: the dry
//!    signal crossfaded with its fourth-order high-pass, so at 100 % with Harmonics at 0 the output
//!    is the input, sample for sample.
//! 5. **Output** trim.
//!
//! Latency 0. Every buffer is made in `prepare`; `process` and `set_param` never allocate.
//! `readouts`: `[0]` input peak dBFS, `[1]` output peak dBFS, `[2]` the added harmonics' peak
//! dBFS.

pub mod presets;

use crate::biquad::{Biquad, FilterType};
use crate::params::{Params, lin, log, param_plumbing};
use crate::util::{Smoother, sane_channels, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain, gain_to_db};
use std::f32::consts::FRAC_1_SQRT_2;

static HEFT_PARAMS: [ParamInfo; 5] = [
    log("freq", "Frequency", 40.0, 250.0, 100.0, Unit::Hz),
    lin("harmonics", "Harmonics", 0.0, 100.0, 50.0, Unit::Percent),
    lin("character", "Character", 0.0, 100.0, 40.0, Unit::Percent),
    lin("original", "Original Bass", 0.0, 100.0, 100.0, Unit::Percent),
    lin("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

const P_FREQ: usize = 0;
const P_HARMONICS: usize = 1;
const P_CHARACTER: usize = 2;
const P_ORIGINAL: usize = 3;
const P_OUTPUT: usize = 4;

pub static HEFT_INFO: PluginInfo = PluginInfo {
    id: "heft",
    name: "Heft",
    short_name: "Heft",
    category: Category::Other,
    params: &HEFT_PARAMS,
    is_instrument: false,
    audiosuite: true,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// The envelope's floor (below it the band is treated as silence, so noise is not magnified).
const FLOOR: f32 = 1.0e-5;

/// One channel's filters and envelope.
#[derive(Debug, Clone, Copy, Default)]
struct Chan {
    /// The band below Frequency (two Butterworth sections: Linkwitz-Riley).
    lp: [Biquad; 2],
    /// The dry signal's high-pass at Frequency (for Original Bass below 100 %).
    hp: [Biquad; 2],
    /// The harmonics' high-pass at Frequency (eight poles).
    hh: [Biquad; 4],
    env: f32,
}

impl Chan {
    fn design(&mut self, hz: f32, sr: f32) {
        for b in &mut self.lp {
            b.set(FilterType::LowPass, hz, FRAC_1_SQRT_2, 0.0, sr);
        }
        for b in self.hp.iter_mut().chain(&mut self.hh) {
            b.set(FilterType::HighPass, hz, FRAC_1_SQRT_2, 0.0, sr);
        }
    }

    fn reset(&mut self) {
        self.lp.iter_mut().chain(&mut self.hp).chain(&mut self.hh).for_each(Biquad::reset);
        self.env = 0.0;
    }
}

pub struct Heft {
    p: Params,
    sr: f32,
    chans: Vec<Chan>,
    /// The Frequency the filters are designed for.
    designed: f32,
    harmonics: Smoother,
    character: Smoother,
    original: Smoother,
    output: Smoother,
    attack: f32,
    release: f32,
    in_peak: f32,
    out_peak: f32,
    add_peak: f32,
}

impl Default for Heft {
    fn default() -> Self {
        Self::new()
    }
}

impl Heft {
    pub fn new() -> Self {
        let mut h = Heft {
            p: Params::new(&HEFT_PARAMS),
            sr: 48_000.0,
            chans: Vec::new(),
            designed: f32::NAN,
            harmonics: Smoother::new(0.5),
            character: Smoother::new(0.4),
            original: Smoother::new(1.0),
            output: Smoother::new(1.0),
            attack: 0.0,
            release: 0.0,
            in_peak: 0.0,
            out_peak: 0.0,
            add_peak: 0.0,
        };
        h.update();
        h
    }

    fn update(&mut self) {
        self.harmonics.set(self.p.v(P_HARMONICS) / 100.0);
        self.character.set(self.p.v(P_CHARACTER) / 100.0);
        self.original.set(self.p.v(P_ORIGINAL) / 100.0);
        self.output.set(db_to_gain(self.p.v(P_OUTPUT)));
    }

    fn design(&mut self) {
        let hz = self.p.v(P_FREQ).clamp(40.0, 250.0).min(self.sr * 0.4);
        if hz != self.designed {
            self.designed = hz;
            let sr = self.sr;
            for c in &mut self.chans {
                c.design(hz, sr);
            }
        }
    }
}

/// The harmonics of `x` (a band swinging about ±1): `character` 0 the second, 1 the third.
#[inline]
pub fn harmonics(x: f32, character: f32) -> f32 {
    let x = x.clamp(-1.0, 1.0);
    let even = 2.0 * x * x - 1.0;
    let odd = (4.0 * x * x - 3.0) * x;
    even + (odd - even) * character
}

impl Plugin for Heft {
    param_plumbing!(HEFT_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.chans = vec![Chan::default(); sane_channels(channels).max(1)];
        self.designed = f32::NAN;
        self.attack = time_coef(1.0, self.sr);
        self.release = time_coef(60.0, self.sr);
        for s in [&mut self.harmonics, &mut self.character, &mut self.original, &mut self.output] {
            s.set_time(20.0, self.sr);
            s.snap();
        }
        self.design();
        self.reset();
    }

    fn reset(&mut self) {
        self.chans.iter_mut().for_each(Chan::reset);
        self.in_peak = 0.0;
        self.out_peak = 0.0;
        self.add_peak = 0.0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.design();
        let n = io.iter().map(Vec::len).fold(frames, usize::min);
        let ch = io.len().min(self.chans.len());
        self.in_peak = 0.0;
        self.out_peak = 0.0;
        self.add_peak = 0.0;
        let (attack, release) = (self.attack, self.release);
        for i in 0..n {
            let amount = self.harmonics.next();
            let character = self.character.next();
            let original = self.original.next();
            let gain = self.output.next();
            for (buf, c) in io.iter_mut().zip(self.chans.iter_mut()).take(ch) {
                let Some(x) = buf.get_mut(i) else { continue };
                let dry = if x.is_finite() { *x } else { 0.0 };
                self.in_peak = self.in_peak.max(dry.abs());
                let low = c.lp.iter_mut().fold(dry, |v, b| b.process_sample(v));
                let a = low.abs();
                c.env = a + (c.env - a) * if a > c.env { attack } else { release };
                let made = if c.env > FLOOR { harmonics(low / c.env, character) * c.env } else { 0.0 };
                let added = c.hh.iter_mut().fold(made, |v, b| b.process_sample(v)) * amount;
                let high = c.hp.iter_mut().fold(dry, |v, b| b.process_sample(v));
                // Original Bass: the dry signal, crossfaded with its high-pass as it goes down
                let base = if original >= 1.0 { dry } else { dry * original + high * (1.0 - original) };
                let y = (base + added) * gain;
                self.add_peak = self.add_peak.max(added.abs());
                let y = if y.is_finite() { y } else { 0.0 };
                self.out_peak = self.out_peak.max(y.abs());
                *x = y;
            }
        }
        if self.chans.iter().any(|c| !c.env.is_finite()) {
            self.reset();
        }
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = gain_to_db(self.in_peak);
        out[1] = gain_to_db(self.out_peak);
        out[2] = gain_to_db(self.add_peak);
    }
}

#[cfg(test)]
mod tests;

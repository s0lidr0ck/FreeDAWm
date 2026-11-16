//! Keel: a bass amplifier. FreeDAWm's own design, built from the Amp engine's parts.
//!
//! The signal path, for one mono signal (the input channels averaged; the result is written to
//! every channel, as Amp does):
//!
//! 1. **Input** trim, then a 25 Hz high-pass that keeps DC and rumble out of the preamp.
//! 2. **Preamp** at four times the session rate (the half-band stages Sat, Dist and Filter use:
//!    flat to 0.45 fs, images and aliases below 0.45 fs rejected by more than 100 dB). Three
//!    voicings, all running all the time so a switch crossfades over 10 ms instead of clicking:
//!    - *Tube*: one 12AX7 common-cathode stage, solved every sample by the Amp engine's
//!      [`TriodeStage`] (Koren's triode law with grid current: the top of the wave rounds off
//!      first, the way a valve preamp does).
//!    - *Solid State*: a biased hyperbolic tangent (smooth, slightly asymmetric).
//!    - *Grind*: a firm symmetric clipper after a mid push at 800 Hz, for an overdriven bass.
//!
//!    **Drive** sets how hard the preamp is hit (0..10; 5 is unity gain into it). The level
//!    after it rises 6 dB from 5 to 10 and falls 6 dB from 5 to 0, so a cleaner setting is not a
//!    louder one.
//! 3. **Tone stack**: the Amp engine's bass-treble-middle network ([`ToneStack`]), solved by
//!    nodal analysis each sample, with part values of Keel's own per voicing (a big bass pot for
//!    the Tube, a wide mid for the Solid State, a tight low end for Grind), made up to the level
//!    it has with every knob at 5. **Bright** adds a shelf above 2.5 kHz.
//! 4. **Cabinet**: a speaker cabinet as filters (its resonance, its body and its top end): 4x10,
//!    1x15, 8x10, or Off. **Blend** mixes the amp (through the cabinet) with the DI, the clean
//!    input delayed by the preamp's latency so the two stay in time: 0 % DI only, 100 % amp only.
//! 5. **Squeeze**: a compressor-style limiter on the blended sound (soft knee, 3 ms attack, 150
//!    ms release, half its reduction made up again), from off at 0 to -24 dB at 20:1 at 10.
//! 6. **Output** trim.
//!
//! Latency: the preamp's half-bands, rounded to whole samples (5 at every rate), reported and
//! matched by the DI. Every buffer is made in `prepare`; `process`, `set_param` and `reset` never
//! allocate. `readouts`: `[0]` input peak dBFS, `[1]` output peak dBFS, `[2]` Squeeze's gain
//! reduction dB.

pub mod presets;

use super::fs::amp::amp_network::{Network, ToneStack, ToneStackParts};
use super::fs::amp::amp_tube::{K12AX7, StageParts, TriodeStage};
use super::fs::sat::sat_halfband::{self as hb, Stage};
use crate::biquad::{Biquad, FilterType};
use crate::params::{Params, choice, lin, param_plumbing, toggle};
use crate::util::{Smoother, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain, gain_to_db};

/// The preamp voicings.
pub const VOICINGS: [&str; 3] = ["Tube", "Solid State", "Grind"];
/// The cabinets.
pub const CABS: [&str; 4] = ["4x10", "1x15", "8x10", "Off"];
const CAB_OFF: usize = 3;

static KEEL_PARAMS: [ParamInfo; 11] = [
    choice("voicing", "Voicing", &VOICINGS, 0),
    lin("input", "Input", -12.0, 12.0, 0.0, Unit::Db),
    lin("drive", "Drive", 0.0, 10.0, 4.0, Unit::None),
    lin("bass", "Bass", 0.0, 10.0, 5.0, Unit::None),
    lin("mid", "Mid", 0.0, 10.0, 5.0, Unit::None),
    lin("treble", "Treble", 0.0, 10.0, 5.0, Unit::None),
    toggle("bright", "Bright", false),
    choice("cab", "Cabinet", &CABS, 0),
    lin("blend", "Blend", 0.0, 100.0, 70.0, Unit::Percent),
    lin("squeeze", "Squeeze", 0.0, 10.0, 0.0, Unit::None),
    lin("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

const P_VOICING: usize = 0;
const P_INPUT: usize = 1;
const P_DRIVE: usize = 2;
const P_BASS: usize = 3;
const P_MID: usize = 4;
const P_TREBLE: usize = 5;
const P_BRIGHT: usize = 6;
const P_CAB: usize = 7;
const P_BLEND: usize = 8;
const P_SQUEEZE: usize = 9;
const P_OUTPUT: usize = 10;

pub static KEEL_INFO: PluginInfo = PluginInfo {
    id: "keel",
    name: "Keel",
    short_name: "Keel",
    category: Category::Other,
    params: &KEEL_PARAMS,
    is_instrument: false,
    audiosuite: true,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// The tone stack's parts per voicing (Keel's own values).
pub const STACKS: [ToneStackParts; 3] = [
    // Tube: a big bass pot and a wide middle, the treble on an audio taper
    ToneStackParts {
        c_treble: 470e-12,
        c_bass: 0.1e-6,
        c_mid: 0.047e-6,
        r_slope: 100e3,
        pot_treble: 250e3,
        pot_bass: 1e6,
        pot_mid: 25e3,
        treble_audio: true,
    },
    // Solid State: a lower slope resistor for a tighter, more even response
    ToneStackParts {
        c_treble: 1000e-12,
        c_bass: 0.047e-6,
        c_mid: 0.022e-6,
        r_slope: 47e3,
        pot_treble: 250e3,
        pot_bass: 250e3,
        pot_mid: 50e3,
        treble_audio: false,
    },
    // Grind: a small bass cap, so the drive does not turn to mud
    ToneStackParts {
        c_treble: 330e-12,
        c_bass: 0.033e-6,
        c_mid: 0.022e-6,
        r_slope: 33e3,
        pot_treble: 250e3,
        pot_bass: 1e6,
        pot_mid: 25e3,
        treble_audio: false,
    },
];

/// Volts at the tube's grid for a full-scale input at Drive 5.
const TUBE_VOLTS: f64 = 0.5;
/// The tube stage's parts.
const TUBE: StageParts = StageParts { tube: K12AX7, supply: 300.0, rp: 100e3, rk: 1500.0, ck: 25e-6, rg_stop: 68e3, cathode_share: 1.0 };
/// The Solid State voicing's bias (its asymmetry).
const SS_BIAS: f64 = 0.2;

/// How hard Drive hits the preamp: 0.1 at 0, 1 at 5, 10 at 10.
pub fn drive_gain(drive: f32) -> f64 {
    let d = if drive.is_finite() { f64::from(drive.clamp(0.0, 10.0)) } else { 4.0 };
    0.1 * 10f64.powf(d * 0.2)
}

/// The Grind voicing's clipper: unity slope at 0, a firm knee, a ceiling of 1.
#[inline]
fn grind(v: f64) -> f64 {
    v / (1.0 + v.powi(4)).sqrt().sqrt()
}

/// The Solid State voicing's curve: unity slope at 0, rounder on one side.
#[inline]
fn solid(v: f64) -> f64 {
    let t = SS_BIAS.tanh();
    ((v + SS_BIAS).tanh() - t) / (1.0 - t * t)
}

/// A speaker cabinet as filters: resonance (a peaking high-pass), body, presence, top roll-off.
#[derive(Debug, Clone, Copy, Default)]
struct Cab {
    f: [Biquad; 5],
    trim: f32,
}

impl Cab {
    /// (high-pass Hz, its Q, body Hz, body dB, presence Hz, presence dB, low-pass Hz)
    const SHAPES: [(f32, f32, f32, f32, f32, f32, f32); 3] =
        [(55.0, 1.1, 120.0, 2.0, 2200.0, 3.0, 4500.0), (42.0, 1.3, 90.0, 3.0, 600.0, -3.0, 2600.0), (60.0, 0.9, 150.0, 3.0, 1200.0, 2.0, 3600.0)];

    fn design(&mut self, which: usize, sr: f32) {
        let Some(&(hp, hq, body, body_db, pres, pres_db, lp)) = Self::SHAPES.get(which) else { return };
        let lp = lp.min(sr * 0.45);
        self.f[0].set(FilterType::HighPass, hp, hq, 0.0, sr);
        self.f[1].set(FilterType::Peak, body, 0.8, body_db, sr);
        self.f[2].set(FilterType::Peak, pres, 1.2, pres_db, sr);
        self.f[3].set(FilterType::LowPass, lp, 0.7, 0.0, sr);
        self.f[4].set(FilterType::LowPass, lp, 0.7, 0.0, sr);
        // made up to unity at 400 Hz, the middle of a bass's body
        let db: f32 = self.f.iter().map(|b| b.magnitude_db(400.0, sr)).sum();
        self.trim = db_to_gain(-db);
    }

    fn reset(&mut self) {
        self.f.iter_mut().for_each(Biquad::reset);
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        self.f.iter_mut().fold(x, |v, b| b.process_sample(v)) * self.trim
    }
}

/// A tone stack in its network, driven from a low impedance, loaded by 1 MΩ.
#[derive(Debug, Clone, Default)]
struct Stack {
    net: Network,
    stack: ToneStack,
    src: usize,
    /// Makes the stack's level with every knob at 5 up to unity.
    makeup: f64,
}

impl Stack {
    fn build(parts: &ToneStackParts, sr: f64) -> Stack {
        let mut net = Network::new(1);
        let n_in = net.add_node();
        let src = net.add_source(n_in, 1_000.0);
        let mut stack = ToneStack::default();
        stack.build(&mut net, n_in, parts);
        net.add_resistor(stack.n_wiper, Network::GROUND, 1e6);
        net.prepare(sr);
        stack.set_knobs(&mut net, 0.5, 0.5, 0.5);
        // the geometric mean over a bass's range, at the knobs' middle
        let hz = [80.0, 250.0, 800.0, 2500.0];
        let mean = hz.iter().map(|h| net.ac_response(src, stack.n_wiper, *h).norm().max(1e-6).ln()).sum::<f64>() / hz.len() as f64;
        Stack { net, stack, src, makeup: (-mean).exp() }
    }

    fn set_knobs(&mut self, bass: f32, mid: f32, treble: f32) {
        let k = |v: f32| f64::from(if v.is_finite() { v.clamp(0.0, 10.0) } else { 5.0 }) / 10.0;
        self.stack.set_knobs(&mut self.net, k(treble), k(bass), k(mid));
    }

    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        self.net.set_input(self.src, x);
        self.net.step();
        self.net.voltage(self.stack.n_wiper) * self.makeup
    }
}

/// The amp path's tone (dB at each of `freqs`, Hz) for the editor's curve: the voicing's tone
/// stack at the knobs, Bright and the cabinet, as `prepare` at `sr` makes them. Leaves out the
/// preamp (it is level-dependent). Allocates: UI thread.
#[allow(clippy::too_many_arguments)]
pub fn tone_curve(voicing: usize, bass: f32, mid: f32, treble: f32, bright: bool, cab: usize, sr: f32, freqs: &[f32]) -> Vec<f32> {
    let sr = sane_sr(sr);
    let Some(parts) = STACKS.get(voicing) else { return vec![0.0; freqs.len()] };
    let mut stack = Stack::build(parts, f64::from(sr));
    stack.set_knobs(bass, mid, treble);
    let mut sh = Biquad::default();
    sh.set(FilterType::HighShelf, 2500.0, 0.7, 5.0, sr);
    let mut c = Cab::default();
    if cab != CAB_OFF {
        c.design(cab, sr);
    }
    freqs
        .iter()
        .map(|hz| {
            let hz = hz.clamp(1.0, sr * 0.49);
            let mut db = gain_to_db((stack.net.ac_response(stack.src, stack.stack.n_wiper, f64::from(hz)).norm() * stack.makeup) as f32);
            if bright {
                db += sh.magnitude_db(hz, sr);
            }
            if cab != CAB_OFF {
                db += c.f.iter().map(|b| b.magnitude_db(hz, sr)).sum::<f32>() + gain_to_db(c.trim);
            }
            db
        })
        .collect()
}

/// The DI's delay: the preamp's latency, at most [`DI_LEN`] - 1 samples.
const DI_LEN: usize = 16;

pub struct Keel {
    p: Params,
    sr: f32,
    latency: usize,
    // smoothed controls
    input: Smoother,
    drive: Smoother,
    blend: Smoother,
    output: Smoother,
    /// How much of each voicing is heard (crossfaded on a switch).
    mix: [Smoother; 3],
    // the preamp
    hp_in: Biquad,
    grind_push: Biquad,
    up: [Stage; 2],
    /// The Grind's own interpolator (its input has the mid push).
    up_grind: [Stage; 2],
    down: [Stage; 2],
    tube: TriodeStage,
    tube_gain: f64,
    dc: Biquad,
    // the tone stacks, one per voicing, all running
    stacks: [Stack; 3],
    knobs: [f32; 3],
    bright: Biquad,
    cab: [Cab; 3],
    di: [f32; DI_LEN],
    di_at: usize,
    // Squeeze
    env_db: f32,
    attack: f32,
    release: f32,
    gr_db: f32,
    // meters
    in_peak: f32,
    out_peak: f32,
}

impl Default for Keel {
    fn default() -> Self {
        Self::new()
    }
}

impl Keel {
    pub fn new() -> Self {
        let mut k = Keel {
            p: Params::new(&KEEL_PARAMS),
            sr: 48_000.0,
            latency: 0,
            input: Smoother::new(1.0),
            drive: Smoother::new(drive_gain(4.0) as f32),
            blend: Smoother::new(0.7),
            output: Smoother::new(1.0),
            mix: [Smoother::new(1.0), Smoother::new(0.0), Smoother::new(0.0)],
            hp_in: Biquad::default(),
            grind_push: Biquad::default(),
            up: [Stage::default(); 2],
            up_grind: [Stage::default(); 2],
            down: [Stage::default(); 2],
            tube: TriodeStage::default(),
            tube_gain: 60.0,
            dc: Biquad::default(),
            stacks: Default::default(),
            knobs: [f32::NAN; 3],
            bright: Biquad::default(),
            cab: [Cab::default(); 3],
            di: [0.0; DI_LEN],
            di_at: 0,
            env_db: -120.0,
            attack: 0.0,
            release: 0.0,
            gr_db: 0.0,
            in_peak: 0.0,
            out_peak: 0.0,
        };
        k.update();
        k
    }

    fn update(&mut self) {
        self.input.set(db_to_gain(self.p.v(P_INPUT)));
        self.drive.set(drive_gain(self.p.v(P_DRIVE)) as f32);
        self.blend.set(self.p.v(P_BLEND) / 100.0);
        self.output.set(db_to_gain(self.p.v(P_OUTPUT)));
        let v = self.p.choice(P_VOICING).min(VOICINGS.len() - 1);
        for (i, m) in self.mix.iter_mut().enumerate() {
            m.set(if i == v { 1.0 } else { 0.0 });
        }
    }

    /// The tube stage's small-signal gain, measured: a quiet sine through it, at its rate.
    fn measure_tube(&mut self, rate: f64) -> f64 {
        let n = (rate * 0.03) as usize;
        let w = std::f64::consts::TAU * 1000.0 / rate;
        let dc = self.tube.dc_plate();
        let mut peak = 0.0f64;
        for i in 0..n {
            let y = self.tube.process(0.01 * (w * i as f64).sin()) - dc;
            if i > n / 2 {
                peak = peak.max(y.abs());
            }
        }
        self.tube.reset();
        if peak.is_finite() && peak > 1e-6 { peak / 0.01 } else { 60.0 }
    }

    fn snap(&mut self) {
        for s in [&mut self.input, &mut self.drive, &mut self.blend, &mut self.output] {
            s.snap();
        }
        self.mix.iter_mut().for_each(Smoother::snap);
    }

    /// Squeeze: the gain (linear) for a sample of level `x`, with the detector's envelope.
    #[inline]
    fn squeeze(&mut self, x: f32, amount: f32) -> f32 {
        if amount <= 0.01 {
            self.gr_db = 0.0;
            self.env_db = -120.0;
            return 1.0;
        }
        let threshold = -2.4 * amount;
        let ratio = 4.0 + 1.6 * amount;
        let knee = 6.0;
        let lvl = gain_to_db(x).max(-120.0);
        let coef = if lvl > self.env_db { self.attack } else { self.release };
        self.env_db = lvl + (self.env_db - lvl) * coef;
        let over = self.env_db - threshold;
        let slope = 1.0 - 1.0 / ratio;
        let gr = if over <= -knee * 0.5 {
            0.0
        } else if over >= knee * 0.5 {
            over * slope
        } else {
            let o = over + knee * 0.5;
            slope * o * o / (2.0 * knee)
        };
        self.gr_db = gr;
        // half of the reduction at the threshold made up again
        let makeup = -threshold * slope * 0.5;
        db_to_gain(makeup - gr)
    }

    /// One mono sample through the amp.
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let x = x * self.input.next();
        self.in_peak = self.in_peak.max(x.abs());
        // the DI, delayed by the preamp's latency
        self.di[self.di_at] = x;
        let di = self.di[(self.di_at + DI_LEN - self.latency.min(DI_LEN - 1)) % DI_LEN];
        self.di_at = (self.di_at + 1) % DI_LEN;

        let k = f64::from(self.drive.next());
        let w = [self.mix[0].next(), self.mix[1].next(), self.mix[2].next()].map(f64::from);
        let pre = f64::from(self.hp_in.process_sample(x));
        let pushed = f64::from(self.grind_push.process_sample(pre as f32));
        // the voicings, oversampled (the Grind hears its mid push through its own interpolator)
        let up = hb::up4(&mut self.up, pre);
        let up_g = hb::up4(&mut self.up_grind, pushed);
        let mut out4 = [0.0f64; 4];
        let dc = self.tube.dc_plate();
        let tube_norm = 1.0 / (self.tube_gain * TUBE_VOLTS);
        for ((o, u), g) in out4.iter_mut().zip(up).zip(up_g) {
            let v = u * k;
            let mut y = 0.0;
            // the tube always runs, so it is warm when it is chosen
            let t = -(self.tube.process(v * TUBE_VOLTS) - dc) * tube_norm;
            y += w[0] * t;
            if w[1] > 0.0 {
                y += w[1] * solid(v);
            }
            if w[2] > 0.0 {
                y += w[2] * grind(g * k * 1.5) / 1.5;
            }
            *o = y;
        }
        let amp = hb::down4(&mut self.down, out4);
        // a cleaner Drive is not a louder one: 6 dB per 5 steps either way
        let amp = amp / k.powf(0.7);
        let amp = self.dc.process_sample(amp as f32);
        // the tone stacks (all running, so a switch does not thump)
        let mut toned = 0.0f64;
        for (i, s) in self.stacks.iter_mut().enumerate() {
            let y = s.run(f64::from(amp));
            toned += w[i] * y;
        }
        let mut amp = toned as f32;
        if self.p.on(P_BRIGHT) {
            amp = self.bright.process_sample(amp);
        }
        let cab = self.p.choice(P_CAB);
        if let Some(c) = self.cab.get_mut(cab).filter(|_| cab != CAB_OFF) {
            amp = c.run(amp);
        }
        let b = self.blend.next();
        let y = amp * b + di * (1.0 - b);
        let g = self.squeeze(y, self.p.v(P_SQUEEZE));
        let y = y * g * self.output.next();
        let y = if y.is_finite() { y } else { 0.0 };
        self.out_peak = self.out_peak.max(y.abs());
        y
    }
}

impl Plugin for Keel {
    param_plumbing!(KEEL_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        let sr = sane_sr(sample_rate);
        self.sr = sr;
        let rate = f64::from(sr);
        let coefs = [hb::coefs_for_stage(0), hb::coefs_for_stage(1)];
        for (st, c) in self.up.iter_mut().zip(&coefs).chain(self.up_grind.iter_mut().zip(&coefs)) {
            st.set(c);
        }
        for (st, c) in self.down.iter_mut().zip(&coefs) {
            st.set(c);
        }
        self.latency = hb::round_trip_latency(&coefs, 2).min(DI_LEN - 1);
        self.tube.prepare(TUBE, rate * 4.0);
        self.tube_gain = self.measure_tube(rate * 4.0);
        self.hp_in.set(FilterType::HighPass1, 25.0, 0.7, 0.0, sr);
        self.grind_push.set(FilterType::Peak, 800.0, 0.9, 6.0, sr);
        self.dc.set(FilterType::HighPass1, 12.0, 0.7, 0.0, sr);
        self.bright.set(FilterType::HighShelf, 2500.0, 0.7, 5.0, sr);
        for (s, parts) in self.stacks.iter_mut().zip(&STACKS) {
            *s = Stack::build(parts, rate);
        }
        self.knobs = [f32::NAN; 3];
        for (i, c) in self.cab.iter_mut().enumerate() {
            c.design(i, sr);
        }
        self.attack = time_coef(3.0, sr);
        self.release = time_coef(150.0, sr);
        for s in [&mut self.input, &mut self.drive, &mut self.blend, &mut self.output] {
            s.set_time(20.0, sr);
        }
        for m in &mut self.mix {
            m.set_time(10.0, sr);
        }
        self.reset();
        self.snap();
    }

    fn reset(&mut self) {
        self.up.iter_mut().chain(&mut self.up_grind).chain(&mut self.down).for_each(Stage::reset);
        self.tube.reset();
        for b in [&mut self.hp_in, &mut self.grind_push, &mut self.dc, &mut self.bright] {
            b.reset();
        }
        for s in &mut self.stacks {
            s.net.reset();
        }
        self.cab.iter_mut().for_each(Cab::reset);
        self.di = [0.0; DI_LEN];
        self.di_at = 0;
        self.env_db = -120.0;
        self.gr_db = 0.0;
        self.in_peak = 0.0;
        self.out_peak = 0.0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let knobs = [self.p.v(P_BASS), self.p.v(P_MID), self.p.v(P_TREBLE)];
        if knobs != self.knobs {
            self.knobs = knobs;
            for s in &mut self.stacks {
                s.set_knobs(knobs[0], knobs[1], knobs[2]);
            }
        }
        let n = io.iter().map(Vec::len).fold(frames, usize::min);
        let ch = io.len();
        if ch == 0 {
            return;
        }
        self.in_peak = 0.0;
        self.out_peak = 0.0;
        let scale = 1.0 / ch as f32;
        for i in 0..n {
            let x = io.iter().map(|c| c[i]).sum::<f32>() * scale;
            let y = self.tick(x);
            for c in io.iter_mut() {
                c[i] = y;
            }
        }
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr_db.max(0.0)
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = gain_to_db(self.in_peak);
        out[1] = gain_to_db(self.out_peak);
        out[2] = self.gr_db.max(0.0);
    }
}

#[cfg(test)]
mod tests;

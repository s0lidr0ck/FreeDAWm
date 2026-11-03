//! `AmpModels.h`: the amps, wired end to end from their schematics, each running at 2x or 4x the
//! host's rate behind a linear-phase Kaiser-windowed-sinc oversampler. The four blackface-circuit
//! Fenders (Twin, Deluxe, Super, Princeton) share one circuit driven by each amp's parts list;
//! the AC30, the Orange OR120 and the Marshall JTM45 have their own.
//!
//! Translated line by line. The C++'s `probe[]` (the tests' look at three nodes) is left out: it
//! never reaches the sound. Rust addition: `reset_fresh` on every amp puts its dynamic state back
//! where `prepare` leaves it, in place (no allocation), for a live `prepareToPlay` and for the
//! wrapper's oversampling re-prepare.

use super::amp_extras::{BiasTremolo, CathodeFollower, FollowerParts, OpticalVibrato, SpringTank};
use super::amp_network::{K_DELUXE_STACK, K_MARSHALL_STACK, K_TWIN_STACK, Network, ToneStack, ToneStackParts};
use super::amp_power::{
    K_AC30_POWER, K_DELUXE_POWER, K_MARSHALL_POWER, K_OR120_POWER, K_PRINCETON_POWER, K_SUPER_POWER, K_TWIN_POWER, PowerAmp, PowerParts,
};
use super::amp_tube::{K12AT7, StageParts, TriodeStage, cclamp, cmax};
use rustfft::num_complex::Complex;

/// 2x or 4x up and down, linear phase: a Kaiser windowed sinc (65 taps at 2x, 129 at 4x),
/// polyphase on the way up.
#[derive(Debug, Clone)]
pub struct Oversampler {
    pub factor: usize,
    h: Vec<f64>,
    up_hist: Vec<f64>,
    down_hist: Vec<f64>,
    branch: Vec<Vec<f64>>,
    up_taps: usize,
    up_pos: usize,
    down_pos: usize,
}

impl Default for Oversampler {
    fn default() -> Self {
        Oversampler { factor: 4, h: Vec::new(), up_hist: Vec::new(), down_hist: Vec::new(), branch: Vec::new(), up_taps: 33, up_pos: 0, down_pos: 0 }
    }
}

impl Oversampler {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self, out: &mut Vec<usize>) {
        super::amp_network::heap_vec(out, &self.h);
        super::amp_network::heap_vec(out, &self.up_hist);
        super::amp_network::heap_vec(out, &self.down_hist);
        super::amp_network::heap_vec(out, &self.branch);
        for b in &self.branch {
            super::amp_network::heap_vec(out, b);
        }
    }

    pub fn prepare(&mut self, new_factor: usize) {
        self.factor = if new_factor == 2 { 2 } else { 4 };
        let factor = self.factor;
        let taps = 32 * factor + 1;
        self.h = vec![0.0; taps];
        let mid = taps / 2;
        let (cut, beta, pi) = (0.46 / factor as f64, 8.0, std::f64::consts::PI);
        let i0 = |x: f64| {
            let (mut s, mut t) = (1.0, 1.0);
            for k in 1..30 {
                let q = x / (2 * k) as f64;
                t *= q * q;
                s += t;
            }
            s
        };
        for i in 0..taps {
            let t = i as f64 - mid as f64;
            let r = t / mid as f64;
            let sinc = if t == 0.0 { 2.0 * cut } else { (2.0 * pi * cut * t).sin() / (pi * t) };
            self.h[i] = sinc * i0(beta * cmax(0.0, 1.0 - r * r).sqrt()) / i0(beta);
        }
        self.up_taps = taps.div_ceil(factor);
        self.branch = vec![vec![0.0; self.up_taps]; factor];
        for i in 0..taps {
            self.branch[i % factor][i / factor] = self.h[i] * factor as f64;
        }
        self.up_hist = vec![0.0; self.up_taps];
        self.down_hist = vec![0.0; taps];
        self.up_pos = 0;
        self.down_pos = 0;
    }

    pub fn reset_fresh(&mut self) {
        self.up_hist.iter_mut().for_each(|x| *x = 0.0);
        self.down_hist.iter_mut().for_each(|x| *x = 0.0);
        self.up_pos = 0;
        self.down_pos = 0;
    }

    /// Latency, at the host's rate: half the filter each way.
    pub fn latency(&self) -> usize {
        2 * ((self.h.len().max(1) - 1) / 2) / self.factor
    }

    pub fn up(&mut self, x: f64, out: &mut [f64; 4]) {
        let n = self.up_taps;
        if self.up_hist.len() != n || n == 0 {
            return;
        }
        self.up_hist[self.up_pos] = x;
        for (ph, o) in out.iter_mut().enumerate().take(self.factor) {
            let b = &self.branch[ph];
            let mut s = 0.0;
            let mut k = self.up_pos;
            for bi in b.iter().take(n) {
                s += bi * self.up_hist[k];
                k = if k == 0 { n - 1 } else { k - 1 };
            }
            *o = s;
        }
        self.up_pos = (self.up_pos + 1) % n;
    }

    pub fn down(&mut self, input: &[f64; 4]) -> f64 {
        let size = self.down_hist.len();
        if size == 0 {
            return 0.0;
        }
        for &x in input.iter().take(self.factor) {
            self.down_hist[self.down_pos] = x;
            self.down_pos = (self.down_pos + 1) % size;
        }
        let mut s = 0.0;
        let mut k = (self.down_pos + size - self.factor) % size;
        for hi in &self.h {
            s += hi * self.down_hist[k];
            k = if k == 0 { size - 1 } else { k - 1 };
        }
        s
    }
}

/// A DC blocker: the coupling cap into a grid leak.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Coupling {
    a: f64,
    x_prev: f64,
    y: f64,
}

impl Coupling {
    pub fn prepare(&mut self, hz: f64, fs: f64) {
        self.a = (-2.0 * 3.14159265358979 * hz / fs).exp();
        self.x_prev = 0.0;
        self.y = 0.0;
    }
    pub fn reset_fresh(&mut self) {
        self.x_prev = 0.0;
        self.y = 0.0;
    }
    #[inline]
    pub fn run(&mut self, x: f64) -> f64 {
        self.y = self.a * (self.y + x - self.x_prev);
        self.x_prev = x;
        self.y
    }
}

// ======================================================================== the knobs
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Controls {
    // shared
    pub volume: f64,
    pub treble: f64,
    pub middle: f64,
    pub bass: f64,
    pub speed: f64,
    pub intensity: f64,
    // Twin
    pub bright: bool,
    /// Twin: the Normal channel instead of Vibrato.
    pub normal_channel: bool,
    pub reverb: f64,
    pub master: f64,
    pub pull_boost: bool,
    // Marshall (volume2 is also the AC30's Normal volume)
    pub volume2: f64,
    pub presence: f64,
    // AC30: the Cut, and the input (0 Top Boost / Brilliant, 1 Normal, 2 both)
    pub cut: f64,
    pub ac_input: i32,
    // Orange: the FAC position (0..5) and HF Drive
    pub fac: i32,
    pub hf_drive: f64,
}

impl Default for Controls {
    fn default() -> Self {
        Controls {
            volume: 0.4,
            treble: 0.6,
            middle: 0.5,
            bass: 0.4,
            speed: 0.4,
            intensity: 0.0,
            bright: false,
            normal_channel: false,
            reverb: 0.25,
            master: 0.7,
            pull_boost: false,
            volume2: 0.0,
            presence: 0.4,
            cut: 0.0,
            ac_input: 0,
            fac: 0,
            hf_drive: 0.0,
        }
    }
}

/// The Miller capacitance a 12AX7/7025 grid shows its source (bypassed / unbypassed cathode).
pub const K_MILLER_BYPASSED: f64 = 105e-12;
pub const K_MILLER_UNBYPASSED: f64 = 60e-12;
pub const K_REVERB_TURNS: f64 = 53.0;
/// Calibrated: see `FenderReverb::tick`.
pub const K_TANK_GAIN: f64 = 0.028;
/// The Twin's two channels meet at the inverter through 220k each: about 0.48.
pub const K_CHANNEL_MIX: f64 = 0.48;

// ======================================================================== the Fenders
/// What differs between two blackface-circuit Fender reverb amps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FenderConfig {
    /// The preamp plates' supply, behind their 100k.
    pub stage_supply: f64,
    pub stack: ToneStackParts,
    /// Across the Volume pot's top.
    pub bright_cap: f64,
    /// Twin: a switch; Deluxe: always in.
    pub bright_switched: bool,
    /// Deluxe: no Middle pot.
    pub fixed_middle: bool,
    pub power: PowerParts,
    /// Per tube, what a tech biases it to (A).
    pub idle_current: f64,
    pub has_master: bool,
    pub has_pull_boost: bool,
    /// Princeton: one channel, no channel mixer.
    pub single_channel: bool,
    /// Princeton: the tremolo moves the power tubes' bias.
    pub bias_vibrato: bool,
}

pub const K_TWIN_CONFIG: FenderConfig = FenderConfig {
    stage_supply: 370.0,
    stack: K_TWIN_STACK,
    bright_cap: 120e-12,
    bright_switched: true,
    fixed_middle: false,
    power: K_TWIN_POWER,
    idle_current: 0.035,
    has_master: true,
    has_pull_boost: true,
    single_channel: false,
    bias_vibrato: false,
};
pub const K_DELUXE_CONFIG: FenderConfig = FenderConfig {
    stage_supply: 285.0,
    stack: K_DELUXE_STACK,
    bright_cap: 47e-12,
    bright_switched: false,
    fixed_middle: true,
    power: K_DELUXE_POWER,
    idle_current: 0.022,
    has_master: false,
    has_pull_boost: false,
    single_channel: false,
    bias_vibrato: false,
};
pub const K_SUPER_CONFIG: FenderConfig = FenderConfig {
    stage_supply: 350.0,
    stack: K_TWIN_STACK,
    bright_cap: 120e-12,
    bright_switched: true,
    fixed_middle: false,
    power: K_SUPER_POWER,
    idle_current: 0.038,
    has_master: false,
    has_pull_boost: false,
    single_channel: false,
    bias_vibrato: false,
};
pub const K_PRINCETON_CONFIG: FenderConfig = FenderConfig {
    stage_supply: 270.0,
    stack: K_DELUXE_STACK,
    bright_cap: 1e-15,
    bright_switched: false,
    fixed_middle: true,
    power: K_PRINCETON_POWER,
    idle_current: 0.020,
    has_master: false,
    has_pull_boost: false,
    single_channel: true,
    bias_vibrato: true,
};

/// The Princeton's tremolo: the oscillator into the power tubes' bias line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiasVibrato {
    fs: f64,
    phase: f64,
}

impl Default for BiasVibrato {
    fn default() -> Self {
        BiasVibrato { fs: 48_000.0, phase: 0.0 }
    }
}

impl BiasVibrato {
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs = sample_rate;
        self.phase = 0.0;
    }
    pub fn reset_fresh(&mut self) {
        self.phase = 0.0;
    }
    pub fn offset(&mut self, speed: f64, intensity: f64) -> f64 {
        let hz = 2.0 * 4f64.powf(cclamp(speed, 0.0, 1.0));
        self.phase += hz / self.fs;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let osc = (1.3 * (2.0 * 3.14159265358979 * self.phase).sin()).tanh() / 1.3f64.tanh();
        45.0 * cclamp(intensity, 0.0, 1.0) * osc
    }
    pub fn rate_hz(&self, speed: f64) -> f64 {
        2.0 * 4f64.powf(cclamp(speed, 0.0, 1.0))
    }
}

/// A Fender channel's front half: the first stage's plate -> tone stack -> Volume, with the
/// Bright cap across its top -> the second stage's grid (and its Miller capacitance).
#[derive(Debug, Clone, Default)]
pub struct FrontHalf {
    pub net: Network,
    pub stack: ToneStack,
    src: usize,
    pub vol_top: usize,
    r_up: usize,
    r_dn: usize,
    r_bright: usize,
    bright_switched: bool,
    fixed_middle: bool,
}

impl FrontHalf {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self, out: &mut Vec<usize>) {
        self.net.heap_blocks(out);
    }

    pub fn build(&mut self, fs: f64, cfg: &FenderConfig) {
        self.bright_switched = cfg.bright_switched;
        self.fixed_middle = cfg.fixed_middle;
        self.net = Network::new(1);
        let p1 = self.net.add_node();
        self.src = self.net.add_source(p1, 38e3); // the plate, through rp || 100k
        self.stack.build(&mut self.net, p1, &cfg.stack);
        self.vol_top = self.net.add_node();
        self.r_up = self.net.add_resistor(self.stack.n_wiper, self.vol_top, 500e3);
        self.r_dn = self.net.add_resistor(self.vol_top, Network::GROUND, 500e3);
        let br = self.net.add_node();
        self.net.add_capacitor(self.stack.n_wiper, br, cfg.bright_cap); // the Bright cap...
        self.r_bright = self.net.add_resistor(br, self.vol_top, 1e12); // ...switched in series
        self.net.add_capacitor(self.vol_top, Network::GROUND, K_MILLER_BYPASSED); // the next grid
        self.net.prepare(fs);
    }

    pub fn set_knobs(&mut self, c: &Controls) {
        self.stack.set_knobs(&mut self.net, c.treble, c.bass, if self.fixed_middle { 1.0 } else { c.middle });
        let v = Network::audio_taper(c.volume);
        self.net.set_resistor(self.r_up, cmax(100.0, 1e6 * (1.0 - v)));
        self.net.set_resistor(self.r_dn, cmax(100.0, 1e6 * v));
        self.net.set_resistor(self.r_bright, if c.bright || !self.bright_switched { 1.0 } else { 1e12 });
    }

    #[inline]
    pub fn run(&mut self, plate: f64) -> f64 {
        self.net.set_input(self.src, plate);
        self.net.step();
        self.net.voltage(self.vol_top)
    }

    /// The source index (the tone-stack curve drives it).
    pub fn source(&self) -> usize {
        self.src
    }
}

/// The four blackface-circuit Fender reverb amps.
#[derive(Debug, Clone)]
pub struct FenderReverb {
    cfg: FenderConfig,
    fs: f64,
    ctl: Controls,
    os: Oversampler,
    v2a: TriodeStage,
    v2b: TriodeStage,
    v4a: TriodeStage,
    v4b: TriodeStage,
    driver: TriodeStage,
    v1a: TriodeStage,
    v1b: TriodeStage,
    chan_a: FrontHalf,
    chan_n: FrontHalf,
    net_b: Network,
    src_b: usize,
    src_ret: usize,
    r_rev_up: usize,
    r_rev_dn: usize,
    node_mix: usize,
    src_drv: usize,
    r_boost: usize,
    tank: SpringTank,
    vib: OpticalVibrato,
    bias_vib: BiasVibrato,
    power: PowerAmp,
    c_v4b: Coupling,
    dc_ret: Coupling,
    c_send: Coupling,
    c_v1b: Coupling,
}

impl FenderReverb {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self, out: &mut Vec<usize>) {
        self.os.heap_blocks(out);
        self.chan_a.heap_blocks(out);
        self.chan_n.heap_blocks(out);
        self.net_b.heap_blocks(out);
        self.tank.heap_blocks(out);
        self.power.heap_blocks(out);
    }

    pub fn new(cfg: FenderConfig) -> Self {
        FenderReverb {
            cfg,
            fs: 192_000.0,
            ctl: Controls::default(),
            os: Oversampler::default(),
            v2a: TriodeStage::default(),
            v2b: TriodeStage::default(),
            v4a: TriodeStage::default(),
            v4b: TriodeStage::default(),
            driver: TriodeStage::default(),
            v1a: TriodeStage::default(),
            v1b: TriodeStage::default(),
            chan_a: FrontHalf::default(),
            chan_n: FrontHalf::default(),
            net_b: Network::new(1),
            src_b: 0,
            src_ret: 0,
            r_rev_up: 0,
            r_rev_dn: 0,
            node_mix: 0,
            src_drv: 0,
            r_boost: 0,
            tank: SpringTank::default(),
            vib: OpticalVibrato::default(),
            bias_vib: BiasVibrato::default(),
            power: PowerAmp::default(),
            c_v4b: Coupling::default(),
            dc_ret: Coupling::default(),
            c_send: Coupling::default(),
            c_v1b: Coupling::default(),
        }
    }

    pub fn prepare(&mut self, host_rate: f64, oversampling: usize) {
        self.os.prepare(oversampling);
        self.fs = host_rate * self.os.factor as f64;
        let fs = self.fs;

        // The 7025 stages, on their 100k plate resistors from the preamp supply.
        let mut s = StageParts { supply: self.cfg.stage_supply, rp: 100e3, ..StageParts::default() };
        s.rk = 1500.0;
        s.ck = 25e-6;
        self.v2a.prepare(s, fs);
        self.v1a.prepare(s, fs);
        s.rk = 820.0;
        s.ck = 25e-6;
        self.v2b.prepare(s, fs);
        self.v1b.prepare(s, fs);
        self.v4b.prepare(s, fs);
        s.rk = 820.0;
        s.ck = 25e-6;
        s.rg_stop = 1e3;
        self.v4a.prepare(s, fs); // recovery: the tank's coil feeds it
        // reverb driver: both halves of a 12AT7 in parallel into the tank's transformer
        let d = StageParts { tube: K12AT7, supply: 450.0, rp: 12e3, rk: 340.0, ck: 25e-6, rg_stop: 1e3, ..StageParts::default() };
        self.driver.prepare(d, fs);

        // --- network A (and the Normal channel's front half, the same circuit)
        let cfg = self.cfg;
        self.chan_a.build(fs, &cfg);
        self.chan_n.build(fs, &cfg);

        // --- network B: V2B's plate and the reverb return -> the mix at V4B's grid
        let mut net = Network::new(1);
        let p2 = net.add_node();
        let dry = net.add_node();
        let g = net.add_node();
        let ret = net.add_node();
        let pot_top = net.add_node();
        let wiper = net.add_node();
        self.src_b = net.add_source(p2, 38e3);
        self.src_ret = net.add_source(ret, 38e3);
        net.add_capacitor(p2, dry, 0.022e-6);
        net.add_resistor(dry, g, 3.3e6);
        net.add_capacitor(dry, g, 10e-12);
        net.add_capacitor(ret, pot_top, 0.003e-6);
        self.r_rev_up = net.add_resistor(pot_top, wiper, 50e3);
        self.r_rev_dn = net.add_resistor(wiper, Network::GROUND, 50e3);
        net.add_resistor(wiper, g, 470e3);
        net.add_resistor(g, Network::GROUND, 220e3);
        // the Pull Boost: the reverb driver's plate, through .002 uF and 220k, into the mix
        let drv_plate = net.add_node();
        let boost_cap = net.add_node();
        self.src_drv = net.add_source(drv_plate, 5e3);
        net.add_capacitor(drv_plate, boost_cap, 0.002e-6);
        self.r_boost = net.add_resistor(boost_cap, g, 1e12);
        net.add_capacitor(g, Network::GROUND, K_MILLER_BYPASSED); // V4B's grid
        self.node_mix = g;
        net.prepare(fs);
        self.net_b = net;

        self.tank.prepare(fs, 2.8);
        self.vib.prepare(fs);
        self.bias_vib.prepare(fs);
        self.power.idle_target = self.cfg.idle_current;
        self.power.prepare(&cfg.power, fs);
        self.c_v4b.prepare(8.0, fs);
        self.c_v1b.prepare(8.0, fs);
        self.dc_ret.prepare(20.0, fs);
        self.c_send.prepare(1.0 / (2.0 * 3.14159265358979 * 500e-12 * 1e6), fs); // 318 Hz
        self.set_controls(&Controls::default());
    }

    /// (Rust) As `prepare` leaves it, in place.
    pub fn reset_fresh(&mut self) {
        self.os.reset_fresh();
        for st in [&mut self.v2a, &mut self.v1a, &mut self.v2b, &mut self.v1b, &mut self.v4b, &mut self.v4a, &mut self.driver] {
            st.reset();
        }
        self.chan_a.net.reset();
        self.chan_n.net.reset();
        self.net_b.reset();
        // (the network's sources hold last sample's inputs; `prepare` makes them anew, at 0)
        for (net, srcs) in [(&mut self.net_b, [self.src_b, self.src_ret, self.src_drv])] {
            for s in srcs {
                net.set_input(s, 0.0);
            }
        }
        let (sa, sn) = (self.chan_a.source(), self.chan_n.source());
        self.chan_a.net.set_input(sa, 0.0);
        self.chan_n.net.set_input(sn, 0.0);
        self.tank.reset_fresh();
        self.vib.reset_fresh();
        self.bias_vib.reset_fresh();
        self.power.reset_fresh();
        for c in [&mut self.c_v4b, &mut self.c_v1b, &mut self.dc_ret, &mut self.c_send] {
            c.reset_fresh();
        }
        self.set_controls(&Controls::default());
    }

    pub fn set_controls(&mut self, c: &Controls) {
        self.ctl = *c;
        self.chan_a.set_knobs(c);
        self.chan_n.set_knobs(c);
        let r = Network::audio_taper(c.reverb);
        self.net_b.set_resistor(self.r_rev_up, cmax(50.0, 100e3 * (1.0 - r)));
        self.net_b.set_resistor(self.r_rev_dn, cmax(50.0, 100e3 * r));
        self.net_b.set_resistor(self.r_boost, if c.pull_boost && self.cfg.has_pull_boost { 220e3 } else { 1e12 });
    }

    pub fn latency(&self) -> usize {
        self.os.latency()
    }

    /// One host-rate sample: guitar volts in, speaker volts out.
    pub fn process(&mut self, x: f64) -> f64 {
        let mut up = [0.0; 4];
        let mut dn = [0.0; 4];
        self.os.up(x, &mut up);
        for i in 0..self.os.factor {
            dn[i] = self.tick(up[i]);
        }
        self.os.down(&dn)
    }

    fn tick(&mut self, x: f64) -> f64 {
        // the Princeton's tremolo: the power tubes' bias, every sample
        if self.cfg.bias_vibrato {
            let off = if self.ctl.intensity > 0.001 { self.bias_vib.offset(self.ctl.speed, self.ctl.intensity) } else { 0.0 };
            self.power.set_bias_offset(off);
        }

        if self.ctl.normal_channel && !self.cfg.single_channel {
            // the Normal channel: V1A, the stack, Volume/Bright, V1B -- and straight to the mixer
            let n1 = self.v1a.process(x) - self.v1a.dc_plate();
            let front = self.chan_n.run(n1);
            let n2 = self.c_v1b.run(self.v1b.process(front) - self.v1b.dc_plate());
            return self.power.process(n2 * self.mix() * self.master_gain());
        }

        // V2A and the tone stack, Volume, Bright
        let p1 = self.v2a.process(x) - self.v2a.dc_plate();
        // V2B
        let front = self.chan_a.run(p1);
        let p2 = self.v2b.process(front) - self.v2b.dc_plate();

        // the reverb: driver off V2B's plate, the tank, the recovery stage
        self.net_b.set_input(self.src_b, p2);
        self.net_b.step(); // (the return below is last sample's)
        let drv = self.driver.process(self.c_send.run(p2)) - self.driver.dc_plate();
        let tank_out = self.tank.process(drv / K_REVERB_TURNS) * K_TANK_GAIN;
        let ret = self.dc_ret.run(self.v4a.process(tank_out) - self.v4a.dc_plate());
        self.net_b.set_input(self.src_ret, ret);
        self.net_b.set_input(self.src_drv, drv); // (both reach the mix next sample)

        // V4B, then the vibrato's LDR shunt, then the Master into the inverter
        let mixed = self.net_b.voltage(self.node_mix);
        let mut v = self.c_v4b.run(self.v4b.process(mixed) - self.v4b.dc_plate());
        if self.ctl.intensity > 0.001 && !self.cfg.bias_vibrato {
            v *= self.vib.gain(self.ctl.speed, self.ctl.intensity, 100e3);
        }
        v *= self.mix() * self.master_gain();
        self.power.process(v)
    }

    pub fn power(&self) -> &PowerAmp {
        &self.power
    }

    pub fn vibrato_rate(&self) -> f64 {
        self.vib.rate_hz(self.ctl.speed)
    }

    pub fn config(&self) -> &FenderConfig {
        &self.cfg
    }

    fn master_gain(&self) -> f64 {
        if self.cfg.has_master { Network::audio_taper(self.ctl.master) } else { 1.0 }
    }

    /// Two channels meet at the inverter through the 220k/220k mixer; one channel does not.
    fn mix(&self) -> f64 {
        if self.cfg.single_channel { 1.0 } else { K_CHANNEL_MIX }
    }
}

// ======================================================================== the AC30
/// 1964 Vox AC30/6 Top Boost.
#[derive(Debug, Clone, Default)]
pub struct Ac30 {
    fs: f64,
    cf_idle: f64,
    ctl: Controls,
    os: Oversampler,
    v1n: TriodeStage,
    v1b: TriodeStage,
    tb: TriodeStage,
    follower: CathodeFollower,
    net_a: Network,
    net_b: Network,
    src_b: usize,
    r_br_up: usize,
    r_br_dn: usize,
    node_br_w: usize,
    src_cf: usize,
    src_n: usize,
    r_tr_up: usize,
    r_tr_dn: usize,
    r_ba_up: usize,
    r_ba_dn: usize,
    r_no_up: usize,
    r_no_dn: usize,
    node_pi: usize,
    pub(crate) node_tone_out: usize,
    power: PowerAmp,
}

fn pot(n: &mut Network, up: usize, dn: usize, total: f64, frac: f64) {
    n.set_resistor(up, cmax(50.0, total * (1.0 - frac)));
    n.set_resistor(dn, cmax(50.0, total * frac));
}

impl Ac30 {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self, out: &mut Vec<usize>) {
        self.os.heap_blocks(out);
        self.net_a.heap_blocks(out);
        self.net_b.heap_blocks(out);
        self.power.heap_blocks(out);
    }

    pub fn prepare(&mut self, host_rate: f64, oversampling: usize) {
        self.os.prepare(oversampling);
        self.fs = host_rate * self.os.factor as f64;
        let fs = self.fs;

        // V1: 220k plates from ~285 V, both halves on one bypassed 1.5k
        let s = StageParts { supply: 285.0, rp: 220e3, rk: 1500.0, ck: 25e-6, cathode_share: 2.0, ..StageParts::default() };
        self.v1n.prepare(s, fs);
        self.v1b.prepare(s, fs);
        // the Top Boost gain stage: 100k from ~300 V, 1.5k unbypassed
        let t = StageParts { supply: 300.0, rp: 100e3, rk: 1500.0, ck: 0.0, rg_stop: 10e3, ..StageParts::default() };
        self.tb.prepare(t, fs);
        // its cathode follower: 56k, plate at the supply, fed from the gain stage's plate
        self.follower.prepare(FollowerParts { supply: 300.0, rk: 56e3, r_source: 100e3, ..FollowerParts::default() });
        self.cf_idle = self.follower.process(self.tb.dc_plate());
        self.build_networks(fs);
        self.power.prepare(&K_AC30_POWER, fs);
        self.set_controls(&Controls::default());
    }

    /// (Rust) Only the passive networks and their knobs: the tone-stack curve's netlist.
    pub(crate) fn prepare_tone(&mut self, fs: f64, c: &Controls) {
        self.fs = fs;
        self.build_networks(fs);
        self.set_controls(c);
    }

    fn build_networks(&mut self, fs: f64) {
        // --- network A: the Brilliant half's plate -> 500 pF -> Brilliant Volume -> the Top Boost grid
        let mut a = Network::new(1);
        {
            let pb = a.add_node();
            let top = a.add_node();
            let w = a.add_node();
            self.src_b = a.add_source(pb, 48e3); // 62k rp || 220k
            a.add_capacitor(pb, top, 500e-12);
            self.r_br_up = a.add_resistor(top, w, 250e3);
            self.r_br_dn = a.add_resistor(w, Network::GROUND, 250e3);
            a.add_capacitor(top, w, 100e-12);
            a.add_capacitor(w, Network::GROUND, K_MILLER_UNBYPASSED);
            self.node_br_w = w;
        }
        a.prepare(fs);
        self.net_a = a;

        // --- network B: the Top Boost stack from the follower, the Normal channel, the mix
        let mut b = Network::new(1);
        {
            let a_ = b.add_node();
            let t1 = b.add_node();
            let tw = b.add_node();
            let t2 = b.add_node();
            let n1 = b.add_node();
            let n2 = b.add_node();
            self.src_cf = b.add_source(a_, self.follower.output_resistance());
            b.add_capacitor(a_, t1, 50e-12); // C43
            self.r_tr_up = b.add_resistor(t1, tw, 500e3); // the Treble pot, wiper = output
            self.r_tr_dn = b.add_resistor(tw, t2, 500e3);
            b.add_resistor(a_, n1, 100e3); // R79
            b.add_capacitor(n1, t2, 0.022e-6); // C44
            b.add_capacitor(n1, n2, 0.022e-6); // C45
            b.add_resistor(n2, Network::GROUND, 10e3); // R80
            self.r_ba_up = b.add_resistor(t2, n2, 500e3); // the Bass pot: t2 -> wiper (n2)
            self.r_ba_dn = b.add_resistor(n2, Network::GROUND, 500e3); //  wiper -> ground
            self.node_tone_out = tw;

            let pn = b.add_node();
            let ntop = b.add_node();
            let nw = b.add_node();
            self.src_n = b.add_source(pn, 48e3);
            b.add_capacitor(pn, ntop, 0.047e-6); // C2
            self.r_no_up = b.add_resistor(ntop, nw, 250e3); // Normal Volume
            self.r_no_dn = b.add_resistor(nw, Network::GROUND, 250e3);

            let m = b.add_node();
            let g = b.add_node();
            b.add_resistor(tw, m, 220e3); // R9
            b.add_resistor(nw, m, 220e3); // R7
            b.add_capacitor(m, g, 0.047e-6); // C6
            b.add_resistor(g, Network::GROUND, 1e6); // R8 (to the tail tap: AC ground)
            self.node_pi = g;
        }
        b.prepare(fs);
        self.net_b = b;
    }

    pub fn reset_fresh(&mut self) {
        self.os.reset_fresh();
        for st in [&mut self.v1n, &mut self.v1b, &mut self.tb] {
            st.reset();
        }
        self.net_a.reset();
        self.net_b.reset();
        self.net_a.set_input(self.src_b, 0.0);
        self.net_b.set_input(self.src_cf, 0.0);
        self.net_b.set_input(self.src_n, 0.0);
        self.power.reset_fresh();
        self.set_controls(&Controls::default());
    }

    pub fn set_controls(&mut self, c: &Controls) {
        self.ctl = *c;
        pot(&mut self.net_a, self.r_br_up, self.r_br_dn, 500e3, Network::audio_taper(c.volume)); // Brilliant
        pot(&mut self.net_b, self.r_no_up, self.r_no_dn, 500e3, Network::audio_taper(c.volume2)); // Normal
        // Treble up: the wiper toward the 50 pF end. Bass up: the wiper toward ground.
        pot(&mut self.net_b, self.r_tr_up, self.r_tr_dn, 1e6, Network::audio_taper(c.treble));
        self.net_b.set_resistor(self.r_ba_up, cmax(50.0, 1e6 * Network::audio_taper(c.bass)));
        self.net_b.set_resistor(self.r_ba_dn, cmax(50.0, 1e6 * (1.0 - Network::audio_taper(c.bass))));
        self.power.set_cut(c.cut);
    }

    pub fn latency(&self) -> usize {
        self.os.latency()
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let mut up = [0.0; 4];
        let mut dn = [0.0; 4];
        self.os.up(x, &mut up);
        for i in 0..self.os.factor {
            dn[i] = self.tick(up[i]);
        }
        self.os.down(&dn)
    }

    fn tick(&mut self, x: f64) -> f64 {
        let brilliant = self.ctl.ac_input != 1;
        let normal = self.ctl.ac_input != 0;

        // the Brilliant / Top Boost side
        let mut cf_out = 0.0;
        if brilliant {
            let pb = self.v1b.process(x) - self.v1b.dc_plate();
            self.net_a.set_input(self.src_b, pb);
            self.net_a.step();
            let plate = self.tb.process(self.net_a.voltage(self.node_br_w)); // (DC and all, into the follower)
            cf_out = self.follower.process(plate) - self.cf_idle;
        }
        // the Normal side
        let pn = if normal { self.v1n.process(x) - self.v1n.dc_plate() } else { 0.0 };

        self.net_b.set_input(self.src_cf, cf_out);
        self.net_b.set_input(self.src_n, pn);
        self.net_b.step();
        let v = self.net_b.voltage(self.node_pi);
        self.power.process(v)
    }

    pub fn power(&self) -> &PowerAmp {
        &self.power
    }

    /// The Top Boost tone stack's network and its source (the tile's curve).
    pub(crate) fn tone_network(&self) -> (&Network, usize) {
        (&self.net_b, self.src_cf)
    }

    /// (Rust) The mixer network, its Top Boost (follower) and Normal sources, and the node the
    /// two meet at (the phase inverter's grid): the curve for the Normal and Both inputs.
    pub(crate) fn mix_network(&self) -> (&Network, usize, usize, usize) {
        (&self.net_b, self.src_cf, self.src_n, self.node_pi)
    }
}

// ======================================================================== the Orange
#[derive(Debug, Clone, Copy, PartialEq)]
struct Peaking {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Default for Peaking {
    fn default() -> Self {
        Peaking { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }
}

impl Peaking {
    fn design(&mut self, hz: f64, q: f64, db: f64, fs: f64) {
        let ag = 10f64.powf(db / 40.0);
        let w = 2.0 * 3.14159265358979 * hz / fs;
        let (c, a) = (w.cos(), w.sin() / (2.0 * q));
        let n = 1.0 + a / ag;
        self.b0 = (1.0 + a * ag) / n;
        self.b1 = -2.0 * c / n;
        self.b2 = (1.0 - a * ag) / n;
        self.a1 = -2.0 * c / n;
        self.a2 = (1.0 - a / ag) / n;
    }
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Orange OR120 Graphic, early 1970s.
#[derive(Debug, Clone)]
pub struct Orange {
    fs: f64,
    plate_lp: f64,
    lp: f64,
    last_speaker: f64,
    /// What HF Drive is doing at 5.2 kHz (the C++ tests read them).
    pub hf_peak_db: f64,
    pub fb_notch_db: f64,
    /// Tests only: 0 opens the loop.
    pub feedback_scale: f64,
    ctl: Controls,
    os: Oversampler,
    v1a: TriodeStage,
    v1b: TriodeStage,
    v2a: TriodeStage,
    net_a: Network,
    net_b: Network,
    src_a: usize,
    src_b: usize,
    r_bass_up: usize,
    r_bass_dn: usize,
    r_treb_up: usize,
    r_treb_dn: usize,
    r_gain_up: usize,
    r_gain_dn: usize,
    node_gain: usize,
    node_v2a: usize,
    pub(crate) node_tone_out: usize,
    r_fac: [usize; 6],
    hf: Peaking,
    fb_shape: Peaking,
    power: PowerAmp,
}

impl Default for Orange {
    fn default() -> Self {
        Orange {
            fs: 192_000.0,
            plate_lp: 0.0,
            lp: 0.0,
            last_speaker: 0.0,
            hf_peak_db: 0.0,
            fb_notch_db: 0.0,
            feedback_scale: 1.0,
            ctl: Controls::default(),
            os: Oversampler::default(),
            v1a: TriodeStage::default(),
            v1b: TriodeStage::default(),
            v2a: TriodeStage::default(),
            net_a: Network::new(1),
            net_b: Network::new(1),
            src_a: 0,
            src_b: 0,
            r_bass_up: 0,
            r_bass_dn: 0,
            r_treb_up: 0,
            r_treb_dn: 0,
            r_gain_up: 0,
            r_gain_dn: 0,
            node_gain: 0,
            node_v2a: 0,
            node_tone_out: 0,
            r_fac: [0; 6],
            hf: Peaking::default(),
            fb_shape: Peaking::default(),
            power: PowerAmp::default(),
        }
    }
}

impl Orange {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self, out: &mut Vec<usize>) {
        self.os.heap_blocks(out);
        self.net_a.heap_blocks(out);
        self.net_b.heap_blocks(out);
        self.power.heap_blocks(out);
    }

    /// The feedback's share at V2a's cathode: 15k into the cathode's ~300 ohm.
    pub const K_FEEDBACK: f64 = 300.0 / (15e3 + 300.0);

    pub fn prepare(&mut self, host_rate: f64, oversampling: usize) {
        self.os.prepare(oversampling);
        self.fs = host_rate * self.os.factor as f64;
        let fs = self.fs;

        let s = StageParts { supply: 400.0, rp: 220e3, rk: 2200.0, ck: 50e-6, ..StageParts::default() };
        self.v1a.prepare(s, fs);
        self.v1b.prepare(s, fs);
        // V2a: 100k from ~450 V; the 1.5k and the 1k pot (600 ohm), unbypassed
        let d = StageParts { supply: 450.0, rp: 100e3, rk: 600.0, ck: 0.0, rg_stop: 10e3, ..StageParts::default() };
        self.v2a.prepare(d, fs);
        self.build_networks(fs);

        self.power.idle_target = 0.028;
        self.power.prepare(&K_OR120_POWER, fs);
        // the 1 nF from V2a's plate to the supply, against ~38k (100k || rp)
        self.plate_lp = (-2.0 * 3.14159265358979 * (1.0 / (2.0 * 3.14159265358979 * 38e3 * 1e-9)) / fs).exp();
        self.set_controls(&Controls::default());
    }

    /// (Rust) Only the passive networks and their knobs: the tone-stack curve's netlist.
    pub(crate) fn prepare_tone(&mut self, fs: f64, c: &Controls) {
        self.fs = fs;
        self.build_networks(fs);
        self.set_controls(c);
    }

    fn build_networks(&mut self, fs: f64) {
        // --- network A: V1a's plate -> the Baxandall -> Gain (330 pF) -> V1b's grid
        let mut a = Network::new(1);
        {
            let pl = a.add_node();
            let p = a.add_node();
            self.src_a = a.add_source(pl, 48e3); // 62k rp || 220k
            a.add_capacitor(pl, p, 0.068e-6);
            let bt = a.add_node();
            let bw = a.add_node();
            let bb = a.add_node();
            a.add_resistor(p, bt, 100e3);
            self.r_bass_up = a.add_resistor(bt, bw, 500e3);
            self.r_bass_dn = a.add_resistor(bw, bb, 500e3);
            a.add_capacitor(bt, bw, 2.2e-9);
            a.add_capacitor(bw, bb, 22e-9);
            a.add_resistor(bb, Network::GROUND, 22e3);
            let tt = a.add_node();
            let o = a.add_node();
            let tb = a.add_node();
            a.add_capacitor(p, tt, 1.5e-9);
            self.r_treb_up = a.add_resistor(tt, o, 500e3);
            self.r_treb_dn = a.add_resistor(o, tb, 500e3);
            a.add_capacitor(tb, Network::GROUND, 10e-9);
            a.add_resistor(bw, o, 100e3);
            let gw = a.add_node();
            self.r_gain_up = a.add_resistor(o, gw, 500e3);
            self.r_gain_dn = a.add_resistor(gw, Network::GROUND, 500e3);
            a.add_capacitor(o, gw, 330e-12);
            a.add_capacitor(gw, Network::GROUND, K_MILLER_BYPASSED); // V1b's grid
            self.node_gain = gw;
            self.node_tone_out = o;
        }
        a.prepare(fs);
        self.net_a = a;

        // --- network B: V1b's plate -> 68 nF -> the FAC's six paths -> 100k / 69k -> V2a
        let mut b = Network::new(1);
        {
            let pl = b.add_node();
            let f = b.add_node();
            let r = b.add_node();
            self.src_b = b.add_source(pl, 48e3);
            // the series capacitance at each position, the 68 nF coupling included
            let chain = [4.7e-9, 4.7e-9, 2.2e-9, 1e-9, 330e-12];
            let mut inv = 1.0 / 0.068e-6;
            for k in 0..6 {
                if k > 0 {
                    inv += 1.0 / chain[k - 1];
                }
                let m = b.add_node();
                b.add_capacitor(pl, m, 1.0 / inv);
                self.r_fac[k] = b.add_resistor(m, f, 1e12);
            }
            b.add_resistor(f, r, 100e3);
            b.add_resistor(r, Network::GROUND, 1.0 / (1.0 / 100e3 + 1.0 / 220e3));
            b.add_capacitor(r, Network::GROUND, 76e-12); // V2a's Miller (unbypassed)
            self.node_v2a = r;
        }
        b.prepare(fs);
        self.net_b = b;
    }

    /// (Rust) As `prepare` leaves it, in place. The C++ `prepare` leaves `lp`, `lastSpeaker` and
    /// the two peaking filters' states as they were (a fresh instance has them at 0); a live reset
    /// clears them too, so a reset renders the same whatever played before it (the feedback
    /// loop's one-sample memory and the HF Drive filter's history).
    pub fn reset_fresh(&mut self) {
        self.os.reset_fresh();
        for st in [&mut self.v1a, &mut self.v1b, &mut self.v2a] {
            st.reset();
        }
        self.net_a.reset();
        self.net_b.reset();
        self.net_a.set_input(self.src_a, 0.0);
        self.net_b.set_input(self.src_b, 0.0);
        self.power.reset_fresh();
        self.lp = 0.0;
        self.last_speaker = 0.0;
        self.hf.z1 = 0.0;
        self.hf.z2 = 0.0;
        self.fb_shape.z1 = 0.0;
        self.fb_shape.z2 = 0.0;
        self.set_controls(&Controls::default());
    }

    pub fn set_controls(&mut self, c: &Controls) {
        self.ctl = *c;
        pot(&mut self.net_a, self.r_bass_up, self.r_bass_dn, 1e6, Network::audio_taper(c.bass)); // up: wiper toward the top
        pot(&mut self.net_a, self.r_treb_up, self.r_treb_dn, 1e6, Network::audio_taper(c.treble));
        pot(&mut self.net_a, self.r_gain_up, self.r_gain_dn, 1e6, Network::audio_taper(c.volume));
        let fac = c.fac.clamp(0, 5) as usize;
        for k in 0..6 {
            self.net_b.set_resistor(self.r_fac[k], if k == fac { 1.0 } else { 1e12 });
        }
        self.design_hf_drive(c.hf_drive);
    }

    pub fn latency(&self) -> usize {
        self.os.latency()
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let mut up = [0.0; 4];
        let mut dn = [0.0; 4];
        self.os.up(x, &mut up);
        for i in 0..self.os.factor {
            dn[i] = self.tick(up[i]);
        }
        self.os.down(&dn)
    }

    fn tick(&mut self, x: f64) -> f64 {
        let p1 = self.v1a.process(x) - self.v1a.dc_plate();
        self.net_a.set_input(self.src_a, p1);
        self.net_a.step();
        let p2 = self.v1b.process(self.net_a.voltage(self.node_gain)) - self.v1b.dc_plate();
        self.net_b.set_input(self.src_b, p2);
        self.net_b.step();

        // V2a, with the feedback from the 16 ohm tap arriving at its cathode (last sample's),
        // ADDED as seen from the grid (the power section does not invert)
        let fbs = self.fb_shape.run(self.last_speaker);
        let drive = self.net_b.voltage(self.node_v2a) + self.feedback_scale * Self::K_FEEDBACK * fbs;
        let mut v = self.v2a.process(drive) - self.v2a.dc_plate();
        v = self.hf.run(v); // HF Drive
        self.lp += (v - self.lp) * (1.0 - self.plate_lp); // the 1 nF
        self.last_speaker = self.power.process(self.lp);
        self.last_speaker
    }

    pub fn power(&self) -> &PowerAmp {
        &self.power
    }

    /// The Baxandall's network and its source (the tile's curve).
    pub(crate) fn tone_network(&self) -> (&Network, usize) {
        (&self.net_a, self.src_a)
    }

    /// HF Drive: V2a's gain from its cathode network at this knob setting, matched by a peaking
    /// filter; the feedback's share is that network against the 15k, matched by a second one.
    fn design_hf_drive(&mut self, knob: f64) {
        let knob = cclamp(knob, 0.0, 1.0);
        let (a_, k_, rk, l, c, rl) = (153e3, 101.0, 1500.0, 2e-3, 0.47e-6, 20.0);
        let r_top = 1000.0 * (1.0 - knob);
        let r_bot = 1000.0 * knob;
        let zk = |hz: f64| -> Complex<f64> {
            let s = Complex::new(0.0, 2.0 * 3.14159265358979 * hz);
            let zlc = Complex::new(rl, 0.0) + s * l + Complex::new(1.0, 0.0) / (s * c);
            let branch = Complex::new(r_top, 0.0) + if r_bot > 0.0 { (zlc * r_bot) / (zlc + r_bot) } else { Complex::new(0.0, 0.0) };
            (branch * rk) / (branch + rk)
        };
        let z_low = zk(20.0).norm();
        let gain_db = |hz: f64| -> f64 { 20.0 * ((Complex::new(a_ + k_ * z_low, 0.0)) / (zk(hz) * k_ + a_)).norm().log10() };
        let f0 = 1.0 / (2.0 * 3.14159265358979 * (l * c).sqrt());
        let fs = self.fs;
        let fit = |db_at: &dyn Fn(f64) -> f64, f: &mut Peaking| -> f64 {
            // centre, height, and the width where it is half its height (in dB)
            let h = db_at(f0);
            let (mut lo, mut hi) = (f0, f0);
            while lo > 100.0 && db_at(lo).abs() > 0.5 * h.abs() {
                lo /= 1.02;
            }
            while hi < 20000.0 && db_at(hi).abs() > 0.5 * h.abs() {
                hi *= 1.02;
            }
            f.design(f0, cclamp(f0 / cmax(1.0, hi - lo), 0.2, 8.0), h, fs);
            h
        };
        let mut hf = self.hf;
        self.hf_peak_db = fit(&gain_db, &mut hf);
        self.hf = hf;

        // the feedback's share: that network (with the tube's ~1/gm) against the 15k
        let beta = |hz: f64| -> Complex<f64> {
            let z = zk(hz);
            let par = (z * 625.0) / (z + 625.0);
            par / (par + 15e3)
        };
        let beta_low = beta(20.0).norm();
        let mut fb_shape = self.fb_shape;
        self.fb_notch_db = fit(&|hz: f64| 20.0 * (beta(hz).norm() / beta_low).log10(), &mut fb_shape);
        self.fb_shape = fb_shape;
    }
}

// ======================================================================== the Marshall
/// 1966 Marshall model 1962 "Bluesbreaker" (JTM45 trem circuit), both channels jumpered.
#[derive(Debug, Clone, Default)]
pub struct Marshall {
    fs: f64,
    cf_idle: f64,
    ctl: Controls,
    os: Oversampler,
    v1: TriodeStage,
    v2a: TriodeStage,
    follower: CathodeFollower,
    net_a: Network,
    net_b: Network,
    stack: ToneStack,
    src_bright: usize,
    src_normal: usize,
    src_cf: usize,
    node_mix: usize,
    node_pi: usize,
    r_v1_up: usize,
    r_v1_dn: usize,
    r_v2_up: usize,
    r_v2_dn: usize,
    trem: BiasTremolo,
    power: PowerAmp,
}

impl Marshall {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self, out: &mut Vec<usize>) {
        self.os.heap_blocks(out);
        self.net_a.heap_blocks(out);
        self.net_b.heap_blocks(out);
        self.power.heap_blocks(out);
    }

    pub fn prepare(&mut self, host_rate: f64, oversampling: usize) {
        self.os.prepare(oversampling);
        self.fs = host_rate * self.os.factor as f64;
        let fs = self.fs;

        // V1: both halves, one shared 820 ohm / 250 uF, plates ~220 V from ~318 V
        let s = StageParts { supply: 318.0, rp: 100e3, rk: 820.0, ck: 250e-6, cathode_share: 2.0, ..StageParts::default() };
        self.v1.prepare(s, fs);
        // V2A: 100k, 820 ohm unbypassed
        let s2 = StageParts { supply: 310.0, rp: 100e3, rk: 820.0, ck: 0.0, rg_stop: 270e3, ..StageParts::default() };
        self.v2a.prepare(s2, fs);
        // the cathode follower, fed from V2A's plate resistor
        self.follower.prepare(FollowerParts { supply: 310.0, rk: 100e3, r_source: 100e3, ..FollowerParts::default() });
        self.build_networks(fs);

        self.trem.prepare(fs);
        self.power.prepare(&K_MARSHALL_POWER, fs);
        self.cf_idle = self.follower.process(self.v2a.dc_plate());
        self.set_controls(&Controls::default());
    }

    /// (Rust) Only the passive networks and their knobs: the tone-stack curve's netlist.
    pub(crate) fn prepare_tone(&mut self, fs: f64, c: &Controls) {
        self.fs = fs;
        self.build_networks(fs);
        self.set_controls(c);
    }

    fn build_networks(&mut self, fs: f64) {
        // --- network A: V1's plates -> .02 -> Volume I (100 pF bright) and Volume II -> 270k mix
        let mut a = Network::new(1);
        let p_bright = a.add_node();
        let p_normal = a.add_node();
        self.src_bright = a.add_source(p_bright, 38e3);
        self.src_normal = a.add_source(p_normal, 38e3);
        let t1 = a.add_node();
        let w1 = a.add_node();
        let t2 = a.add_node();
        let w2 = a.add_node();
        let mix = a.add_node();
        a.add_capacitor(p_bright, t1, 0.02e-6);
        self.r_v1_up = a.add_resistor(t1, w1, 500e3);
        self.r_v1_dn = a.add_resistor(w1, Network::GROUND, 500e3);
        a.add_capacitor(t1, w1, 100e-12); // the bright channel's cap
        a.add_capacitor(p_normal, t2, 0.02e-6);
        self.r_v2_up = a.add_resistor(t2, w2, 500e3);
        self.r_v2_dn = a.add_resistor(w2, Network::GROUND, 500e3);
        a.add_resistor(w1, mix, 270e3);
        a.add_resistor(w2, mix, 270e3);
        a.add_capacitor(mix, Network::GROUND, K_MILLER_UNBYPASSED); // V2A's grid
        self.node_mix = mix;
        a.prepare(fs);
        self.net_a = a;

        // --- network B: the cathode follower -> tone stack -> .02 -> the inverter's 1M
        let mut b = Network::new(1);
        let cf_node = b.add_node();
        self.src_cf = b.add_source(cf_node, self.follower.output_resistance());
        self.stack.build(&mut b, cf_node, &K_MARSHALL_STACK);
        let pi_in = b.add_node();
        b.add_capacitor(self.stack.n_wiper, pi_in, 0.02e-6);
        b.add_resistor(pi_in, Network::GROUND, 1e6);
        self.node_pi = pi_in;
        b.prepare(fs);
        self.net_b = b;
    }

    pub fn reset_fresh(&mut self) {
        self.os.reset_fresh();
        self.v1.reset();
        self.v2a.reset();
        self.net_a.reset();
        self.net_b.reset();
        self.net_a.set_input(self.src_bright, 0.0);
        self.net_a.set_input(self.src_normal, 0.0);
        self.net_b.set_input(self.src_cf, 0.0);
        self.trem.reset_fresh();
        self.power.reset_fresh();
        self.set_controls(&Controls::default());
    }

    pub fn set_controls(&mut self, c: &Controls) {
        self.ctl = *c;
        self.stack.set_knobs(&mut self.net_b, c.treble, c.bass, c.middle);
        let a = Network::audio_taper(c.volume);
        let b = Network::audio_taper(c.volume2);
        self.net_a.set_resistor(self.r_v1_up, cmax(100.0, 1e6 * (1.0 - a)));
        self.net_a.set_resistor(self.r_v1_dn, cmax(100.0, 1e6 * a));
        self.net_a.set_resistor(self.r_v2_up, cmax(100.0, 1e6 * (1.0 - b)));
        self.net_a.set_resistor(self.r_v2_dn, cmax(100.0, 1e6 * b));
        self.power.set_presence(c.presence);
    }

    pub fn latency(&self) -> usize {
        self.os.latency()
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let mut up = [0.0; 4];
        let mut dn = [0.0; 4];
        self.os.up(x, &mut up);
        for i in 0..self.os.factor {
            dn[i] = self.tick(up[i]);
        }
        self.os.down(&dn)
    }

    fn tick(&mut self, x: f64) -> f64 {
        let p1 = self.v1.process(x) - self.v1.dc_plate();
        self.net_a.set_input(self.src_bright, p1);
        self.net_a.set_input(self.src_normal, p1);
        self.net_a.step();
        let mut m = self.net_a.voltage(self.node_mix);
        if self.ctl.intensity > 0.001 {
            m *= self.trem.gain(self.ctl.speed, self.ctl.intensity);
        }

        // V2A into the direct-coupled cathode follower (DC and all)
        let p2 = self.v2a.process(m);
        let k = self.follower.process(p2) - self.cf_idle;
        self.net_b.set_input(self.src_cf, k);
        self.net_b.step();
        let v = self.net_b.voltage(self.node_pi);
        self.power.process(v)
    }

    pub fn power(&self) -> &PowerAmp {
        &self.power
    }

    /// The tone stack's network, its source and its output (the tile's curve).
    pub(crate) fn tone_network(&self) -> (&Network, usize, usize) {
        (&self.net_b, self.src_cf, self.stack.n_wiper)
    }
}

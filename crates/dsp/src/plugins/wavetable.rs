//! Wavetable Synth: 16 voices, each a band-limited wavetable oscillator (thirteen banks of 32
//! frames, every one made in code: see [`tables`]) with a sub oscillator and noise, unison of up
//! to seven detuned copies spread across the stereo field, the Subtractive Synth's filter (12 or
//! 24 dB low-pass, high-pass, band-pass, notch) with key tracking, an amp envelope, a second
//! envelope to the table position and the cutoff, and an LFO to pitch, cutoff, position and
//! level. The table position also follows velocity and the mod wheel. Pitch bend, the mod wheel's
//! vibrato, the sustain pedal, glide and voice stealing work as on the Subtractive Synth, whose
//! envelopes, filter, event queue, unison spread and voice choice it shares.

use super::synth::{
    Adsr, AdsrRates, EventKind, EventQueue, FILTERS, LFO_WAVES, MAX_EVENTS, MAX_UNISON, SILENT, Stage, Svf, UNISON, VIBRATO_HZ, event_of, lfo_shape,
    pick_voice, unison_copies,
};
use super::synth_mod::{self as sm, ModMatrix, VoiceMod};
use crate::osc::{Osc, Waveform};
use crate::params::{Params, choice, lin, log, param_plumbing};
use crate::util::{Smoother, frames_in, ms_to_samples, note_hz, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

pub mod presets;
#[cfg(test)]
mod reference;
pub mod tables;

static WT_PARAMS: [ParamInfo; 42] = [
    choice("table", "Table", tables::BANKS, 0),
    lin("position", "Position", 0.0, 100.0, 0.0, Unit::Percent),
    // What moves the position, each as a share of the whole table.
    lin("env2_position", "Env 2 to Position", -100.0, 100.0, 0.0, Unit::Percent),
    lin("lfo_position", "LFO to Position", 0.0, 100.0, 0.0, Unit::Percent),
    lin("velocity_position", "Velocity to Position", -100.0, 100.0, 0.0, Unit::Percent),
    lin("wheel_position", "Mod Wheel to Position", -100.0, 100.0, 0.0, Unit::Percent),
    lin("osc_pitch", "Pitch", -24.0, 24.0, 0.0, Unit::Semitones),
    lin("osc_fine", "Fine", -100.0, 100.0, 0.0, Unit::Cents),
    lin("sub_level", "Sub Osc", 0.0, 100.0, 0.0, Unit::Percent),
    lin("noise_level", "Noise", 0.0, 100.0, 0.0, Unit::Percent),
    choice("unison", "Unison", UNISON, 0),
    lin("unison_detune", "Unison Detune", 0.0, 100.0, 20.0, Unit::Cents),
    lin("unison_spread", "Unison Spread", 0.0, 100.0, 50.0, Unit::Percent),
    choice("filter_type", "Filter Type", FILTERS, 0),
    log("cutoff", "Cutoff", 20.0, 20000.0, 12000.0, Unit::Hz),
    lin("resonance", "Resonance", 0.0, 100.0, 10.0, Unit::Percent),
    lin("env2_cutoff", "Env 2 to Cutoff", -100.0, 100.0, 0.0, Unit::Percent),
    lin("key_track", "Key Tracking", 0.0, 100.0, 0.0, Unit::Percent),
    log("amp_attack", "Amp Attack", 0.5, 5000.0, 5.0, Unit::Ms),
    log("amp_decay", "Amp Decay", 1.0, 5000.0, 300.0, Unit::Ms),
    lin("amp_sustain", "Amp Sustain", 0.0, 100.0, 80.0, Unit::Percent),
    log("amp_release", "Amp Release", 1.0, 10000.0, 300.0, Unit::Ms),
    log("env2_attack", "Env 2 Attack", 0.5, 5000.0, 5.0, Unit::Ms),
    log("env2_decay", "Env 2 Decay", 1.0, 5000.0, 600.0, Unit::Ms),
    lin("env2_sustain", "Env 2 Sustain", 0.0, 100.0, 0.0, Unit::Percent),
    log("env2_release", "Env 2 Release", 1.0, 10000.0, 300.0, Unit::Ms),
    choice("lfo_wave", "LFO Wave", LFO_WAVES, 0),
    log("lfo_rate", "LFO Rate", 0.05, 30.0, 2.0, Unit::Hz),
    lin("lfo_pitch", "LFO to Pitch", 0.0, 1200.0, 0.0, Unit::Cents),
    lin("lfo_cutoff", "LFO to Cutoff", 0.0, 100.0, 0.0, Unit::Percent),
    lin("lfo_amp", "LFO to Level", 0.0, 100.0, 0.0, Unit::Percent),
    lin("glide", "Glide", 0.0, 1000.0, 0.0, Unit::Ms),
    lin("velocity", "Velocity Sens", 0.0, 100.0, 70.0, Unit::Percent),
    lin("level", "Level", -48.0, 6.0, -6.0, Unit::Db),
    lin("bend_range", "Bend Range", 0.0, 24.0, 2.0, Unit::Semitones),
    lin("mod_wheel", "Mod Wheel Vibrato", 0.0, 100.0, 0.0, Unit::Cents),
    // The macros: each a knob the modulation matrix's macro targets follow (nothing at 0).
    lin("macro1", "Macro 1", 0.0, 100.0, 0.0, Unit::Percent),
    lin("macro2", "Macro 2", 0.0, 100.0, 0.0, Unit::Percent),
    lin("macro3", "Macro 3", 0.0, 100.0, 0.0, Unit::Percent),
    lin("macro4", "Macro 4", 0.0, 100.0, 0.0, Unit::Percent),
    // Warp: what bends the table oscillator (Off: as before).
    choice("warp_mode", "Warp", WARP_MODES, 0),
    lin("warp_amount", "Warp Amount", 0.0, 100.0, 0.0, Unit::Percent),
];
const WARP_MODE: usize = 40;
const WARP_AMOUNT: usize = 41;
/// Warp's modes: a sine at the note's pitch phase-modulating the table, folding its output
/// back on itself, bending its phase, or a slave phase restarted by the note's (sync).
pub const WARP_MODES: &[&str] = &["Off", "FM", "Fold", "Bend", "Sync"];
/// FM's phase deviation at full amount, cycles (an index of about 2.2).
const WARP_FM_DEPTH: f32 = 0.35;
const MACRO1: usize = 36;

/// What the modulation matrix can move (parameter ids), by index.
pub(crate) const MOD_DESTS: [&str; 19] = [
    "osc_pitch",
    "osc_fine",
    "position",
    "sub_level",
    "noise_level",
    "unison_detune",
    "unison_spread",
    "cutoff",
    "resonance",
    "env2_cutoff",
    "env2_position",
    "key_track",
    sm::SHARED_DEST,
    "lfo_pitch",
    "lfo_cutoff",
    "lfo_position",
    "lfo_amp",
    "level",
    "warp_amount",
];
const D_PITCH: usize = 0;
const D_FINE: usize = 1;
const D_POS: usize = 2;
const D_SUB: usize = 3;
const D_NOISE: usize = 4;
const D_UNI_DETUNE: usize = 5;
const D_UNI_SPREAD: usize = 6;
const D_CUTOFF: usize = 7;
const D_RES: usize = 8;
const D_ENV2_CUT: usize = 9;
const D_ENV2_POS: usize = 10;
const D_KEY: usize = 11;
const D_LFO_RATE: usize = 12;
const D_LFO_PITCH: usize = 13;
const D_LFO_CUTOFF: usize = 14;
const D_LFO_POS: usize = 15;
const D_LFO_AMP: usize = 16;
const D_LEVEL: usize = 17;
const D_WARP: usize = 18;

const TABLE: usize = 0;
const POSITION: usize = 1;
const ENV2_POS: usize = 2;
const LFO_POS: usize = 3;
const VEL_POS: usize = 4;
const WHEEL_POS: usize = 5;
const PITCH: usize = 6;
const FINE: usize = 7;
const SUB_LEVEL: usize = 8;
const NOISE_LEVEL: usize = 9;
const UNISON_P: usize = 10;
const UNISON_DETUNE: usize = 11;
const UNISON_SPREAD: usize = 12;
const FILTER_TYPE: usize = 13;
const CUTOFF: usize = 14;
const RESONANCE: usize = 15;
const ENV2_CUTOFF: usize = 16;
const KEY_TRACK: usize = 17;
const AMP_A: usize = 18;
const ENV2_A: usize = 22;
const LFO_WAVE: usize = 26;
const LFO_RATE: usize = 27;
const LFO_PITCH: usize = 28;
const LFO_CUTOFF: usize = 29;
const LFO_AMP: usize = 30;
const GLIDE: usize = 31;
const VEL_SENS: usize = 32;
const LEVEL: usize = 33;
const BEND_RANGE: usize = 34;
const MOD_WHEEL: usize = 35;

pub static WAVETABLE_INFO: PluginInfo = PluginInfo {
    id: "wavetable_synth",
    name: "Cicada",
    short_name: "Cicada",
    category: Category::Instrument,
    params: &WT_PARAMS,
    is_instrument: true,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

const VOICES: usize = 16;

#[derive(Debug, Clone, Copy)]
struct Voice {
    note: u8,
    /// The level velocity gives (after Velocity Sens) and the raw velocity (0..1).
    vel: f32,
    vel_raw: f32,
    freq: f32,
    target: f32,
    /// Each unison copy's phase (0..1).
    phases: [f32; MAX_UNISON],
    amp: Adsr,
    env2: Adsr,
    svf: Svf,
    svf2: Svf,
    svf_r: Svf,
    svf2_r: Svf,
    sub: Osc,
    noise: u32,
    age: u64,
    held: bool,
    sustained: bool,
    /// Warp's Sync: each copy's slave phase and the rest of its last restart's correction.
    sync_ph: [f32; MAX_UNISON],
    sync_fix: [f32; MAX_UNISON],
    /// The matrix's values for this voice, and its unison copies when the matrix moves their
    /// detune or spread.
    m: VoiceMod,
    copies: [(f32, f32, f32); MAX_UNISON],
}

impl Voice {
    fn new(seed: u32) -> Self {
        Self {
            m: VoiceMod::new(),
            copies: [(1.0, 1.0, 1.0); MAX_UNISON],
            note: 0,
            vel: 0.0,
            vel_raw: 0.0,
            freq: 440.0,
            target: 440.0,
            phases: [0.0; MAX_UNISON],
            amp: Adsr::new(),
            env2: Adsr::new(),
            svf: Svf::default(),
            svf2: Svf::default(),
            svf_r: Svf::default(),
            svf2_r: Svf::default(),
            sub: Osc::new(seed.wrapping_mul(104_729)),
            noise: seed.wrapping_mul(2_654_435_761).max(1),
            age: 0,
            held: false,
            sustained: false,
            sync_ph: [0.0; MAX_UNISON],
            sync_fix: [0.0; MAX_UNISON],
        }
    }

    fn active(&self) -> bool {
        self.amp.stage != Stage::Idle
    }

    /// A fresh start: copy 0 from phase 0, the others spread (so a stack does not start as one
    /// wave), the filters emptied.
    fn restart(&mut self) {
        for (i, p) in self.phases.iter_mut().enumerate() {
            *p = (i as f32 * 0.618_034).fract();
        }
        self.sub.reset();
        self.sync_ph = self.phases;
        self.sync_fix = [0.0; MAX_UNISON];
        for f in [&mut self.svf, &mut self.svf2, &mut self.svf_r, &mut self.svf2_r] {
            *f = Svf::default();
        }
    }
}

/// Polyphonic wavetable synthesizer.
pub struct WavetableSynth {
    p: Params,
    t: &'static tables::Tables,
    sr: f32,
    ch: usize,
    voices: [Voice; VOICES],
    events: EventQueue,
    amp_rates: AdsrRates,
    env2_rates: AdsrRates,
    glide_coef: f32,
    last_freq: Option<f32>,
    counter: u64,
    level: Smoother,
    cutoff: Smoother,
    position: Smoother,
    damping: Smoother,
    sustain: bool,
    bend: f32,
    wheel: f32,
    vib_phase: f32,
    lfo_phase: f32,
    /// The modulation matrix and its evaluation (as the Subtractive Synth's).
    mm: ModMatrix,
    infos: [&'static ParamInfo; sm::MAX_DESTS],
    bases: [f32; sm::MAX_DESTS],
    macro_off: [f32; sm::MAX_DESTS],
    tick_left: usize,
    lfo_rate_mod: f32,
    rng: u32,
}

/// The values one sample shares across voices, which the matrix may change per voice.
#[derive(Debug, Clone, Copy)]
struct Shared {
    /// Bend and vibrato in cents (before the LFO, pitch and fine), the LFO now, the wheel.
    cents: f32,
    lfo: f32,
    wheel: f32,
    lfo_pitch: f32,
    pitch: f32,
    fine: f32,
    /// The pitch ratio from all of them.
    ratio: f32,
    base_pos: f32,
    lfo_pos: f32,
    wheel_pos: f32,
    shared_pos: f32,
    env2_pos: f32,
    k: f32,
    cutoff: f32,
    env2_oct: f32,
    lfo_oct: f32,
    lfo_amp: f32,
    key_track: f32,
    sub_level: f32,
    noise_level: f32,
    gain: f32,
    /// Warp's amount, 0..1.
    warp: f32,
}

impl Shared {
    /// `self` with what the matrix sets for the voice whose values are `m`.
    #[inline]
    fn modded(mut self, mm: &ModMatrix, m: &VoiceMod) -> Shared {
        let on = |d| mm.on(d);
        let pick = |d, v: f32| if on(d) { m.get(d) } else { v };
        if on(D_PITCH) || on(D_FINE) || on(D_LFO_PITCH) {
            let cents = self.cents + pick(D_LFO_PITCH, self.lfo_pitch) * self.lfo + pick(D_PITCH, self.pitch) * 100.0 + pick(D_FINE, self.fine);
            self.ratio = 2f32.powf(cents / 1200.0);
        }
        if on(D_POS) || on(D_LFO_POS) {
            self.shared_pos = pick(D_POS, self.base_pos) + pick(D_LFO_POS, self.lfo_pos) * 0.5 * self.lfo + self.wheel_pos * self.wheel;
        }
        self.env2_pos = pick(D_ENV2_POS, self.env2_pos);
        self.k = pick(D_RES, self.k);
        self.cutoff = pick(D_CUTOFF, self.cutoff);
        self.env2_oct = pick(D_ENV2_CUT, self.env2_oct);
        if on(D_LFO_CUTOFF) {
            self.lfo_oct = m.get(D_LFO_CUTOFF) * self.lfo;
        }
        if on(D_LFO_AMP) {
            self.lfo_amp = 1.0 - m.get(D_LFO_AMP) * 0.5 * (1.0 - self.lfo);
        }
        self.key_track = pick(D_KEY, self.key_track);
        self.sub_level = pick(D_SUB, self.sub_level);
        self.noise_level = pick(D_NOISE, self.noise_level);
        self.gain = pick(D_LEVEL, self.gain);
        self.warp = pick(D_WARP, self.warp);
        self
    }
}

/// Warp's settings for one voice and sample: the mode, the amount (0..1), the band limit read
/// (darker by as much as the warp brightens the wave, so it does not fold back), and each mode's
/// depth.
#[derive(Debug, Clone, Copy)]
struct Warp {
    mode: usize,
    amount: f32,
    level: usize,
    /// Fold's drive, Bend's exponent, Sync's slave ratio.
    depth: f32,
}

/// `x` folded back on itself by `amount` (0..1) with drive `drive`: a smooth (sine) fold, so
/// the harmonics it adds fall away quickly, blended in by the amount (none at 0).
#[inline]
fn fold(x: f32, amount: f32, drive: f32) -> f32 {
    let y = (1.0 - amount) * x + amount * (std::f32::consts::FRAC_PI_2 * drive * x).sin();
    if y.is_finite() { y } else { 0.0 }
}

impl Warp {
    /// `inc`: the highest unison copy's step (cycles per sample).
    fn new(mode: usize, amount: f32, inc: f32) -> Warp {
        let (depth, brighter) = match mode {
            1 => (0.0, 1.0 + std::f32::consts::TAU * WARP_FM_DEPTH * amount),
            2 => (1.0 + 3.0 * amount, 1.0 + 2.0 * (1.0 + 3.0 * amount) * amount),
            3 => ((2.0 * amount).exp2(), (2.0 * amount).exp2()),
            4 => ((4.0 * amount).exp2(), (4.0 * amount).exp2()),
            _ => (1.0, 1.0),
        };
        Warp { mode, amount, level: tables::level_for(inc * brighter), depth }
    }

    /// One sample of the table at phase `ph` stepping `dt`; `sp`/`fix`: the copy's Sync state.
    #[inline]
    fn read(&self, t: &tables::Tables, bank: usize, pos: f32, ph: f32, dt: f32, sp: &mut f32, fix: &mut f32) -> f32 {
        let at = |p: f32| t.read(bank, pos, self.level, p);
        match self.mode {
            1 => at((ph + WARP_FM_DEPTH * self.amount * (std::f32::consts::TAU * ph).sin()).rem_euclid(1.0)),
            2 => fold(at(ph), self.amount, self.depth),
            3 => at(ph.clamp(0.0, 1.0).powf(self.depth)),
            4 => {
                // A slave phase up to four octaves above the note's, restarted where the note's
                // wraps; the jump spread over the two samples around it (a PolyBLEP step, as the
                // Subtractive Synth's hard sync).
                let dts = (dt * self.depth).clamp(0.0, 0.49);
                let p0 = if sp.is_finite() { sp.clamp(0.0, 0.999_999) } else { 0.0 };
                let mut s = at(p0) + std::mem::take(fix);
                let mut np = p0 + dts;
                if np >= 1.0 {
                    np -= 1.0;
                }
                if dt > 0.0 && ph + dt >= 1.0 {
                    let d = ((ph + dt - 1.0) / dt).clamp(0.0, 1.0);
                    let jump_from = (p0 + dts * (1.0 - d)).fract();
                    let h = at(0.0) - at(jump_from);
                    s += 0.5 * h * d * d;
                    np = (dts * d).min(0.999_999);
                    let e = 1.0 - d;
                    *fix = -0.5 * h * e * e;
                    if !fix.is_finite() {
                        *fix = 0.0;
                    }
                }
                *sp = if np.is_finite() { np } else { 0.0 };
                s
            }
            _ => at(ph),
        }
    }
}

impl Default for WavetableSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl WavetableSynth {
    /// A new synth (builds the shared tables the first time one is made).
    pub fn new() -> Self {
        let mut voices = [Voice::new(1); VOICES];
        for (i, v) in voices.iter_mut().enumerate() {
            *v = Voice::new(0x2000 + i as u32 * 37);
        }
        let mut s = Self {
            p: Params::new(&WT_PARAMS),
            t: tables::tables(),
            sr: 48_000.0,
            ch: 0,
            voices,
            events: EventQueue::new(),
            amp_rates: AdsrRates::default(),
            env2_rates: AdsrRates::default(),
            glide_coef: 0.0,
            last_freq: None,
            counter: 0,
            level: Smoother::new(0.5),
            cutoff: Smoother::new(12_000.0),
            position: Smoother::new(0.0),
            damping: Smoother::new(2.0),
            sustain: false,
            bend: 0.0,
            wheel: 0.0,
            vib_phase: 0.0,
            lfo_phase: 0.0,
            mm: ModMatrix::new(),
            infos: sm::infos(WAVETABLE_INFO.id),
            bases: [0.0; sm::MAX_DESTS],
            macro_off: [0.0; sm::MAX_DESTS],
            tick_left: 0,
            lfo_rate_mod: 2.0,
            rng: sm::RANDOM_SEED,
        };
        s.update();
        s
    }

    /// The LFO's rate now: the knob's, or what the matrix makes of it.
    #[inline]
    fn lfo_rate(&self) -> f32 {
        if self.mm.on(D_LFO_RATE) { self.lfo_rate_mod } else { self.p.v(LFO_RATE) }
    }

    /// A note starts on voice `i`: its random value, and the matrix evaluated at once.
    fn mod_note_on(&mut self, i: usize) {
        let r = sm::next_random(&mut self.rng);
        if let Some(v) = self.voices.get_mut(i) {
            v.m.rnd = r;
            v.m.fresh = true;
        }
        self.tick_left = 0;
    }

    /// See the Subtractive Synth's.
    fn mod_step(&mut self) {
        if self.tick_left == 0 {
            self.tick_left = sm::TICK;
            self.mod_tick();
        }
        self.tick_left -= 1;
        let used = self.mm.used();
        for v in self.voices.iter_mut().filter(|v| v.active()) {
            v.m.advance(used);
        }
    }

    fn mod_tick(&mut self) {
        let p = &self.p;
        let macros = [p.v(MACRO1) / 100.0, p.v(MACRO1 + 1) / 100.0, p.v(MACRO1 + 2) / 100.0, p.v(MACRO1 + 3) / 100.0];
        self.mm.macro_offsets(macros, &mut self.macro_off);
        let b = &mut self.bases;
        b[D_PITCH] = p.v(PITCH);
        b[D_FINE] = p.v(FINE);
        b[D_POS] = self.position.value() * 100.0;
        b[D_SUB] = p.v(SUB_LEVEL);
        b[D_NOISE] = p.v(NOISE_LEVEL);
        b[D_UNI_DETUNE] = p.v(UNISON_DETUNE);
        b[D_UNI_SPREAD] = p.v(UNISON_SPREAD);
        b[D_CUTOFF] = self.cutoff.value();
        b[D_RES] = (2.0 - self.damping.value()) / 1.95 * 100.0;
        b[D_ENV2_CUT] = p.v(ENV2_CUTOFF);
        b[D_ENV2_POS] = p.v(ENV2_POS);
        b[D_KEY] = p.v(KEY_TRACK);
        b[D_LFO_RATE] = p.v(LFO_RATE);
        b[D_LFO_PITCH] = p.v(LFO_PITCH);
        b[D_LFO_CUTOFF] = p.v(LFO_CUTOFF);
        b[D_LFO_POS] = p.v(LFO_POS);
        b[D_LFO_AMP] = p.v(LFO_AMP);
        b[D_LEVEL] = p.v(LEVEL);
        b[D_WARP] = p.v(WARP_AMOUNT);
        let lfo = lfo_shape(p.choice(LFO_WAVE), self.lfo_phase);
        if self.mm.on(D_LFO_RATE) {
            self.lfo_rate_mod = sm::shared_value(&self.mm, D_LFO_RATE, lfo, self.wheel, &self.macro_off, self.infos[D_LFO_RATE], b[D_LFO_RATE]);
        }
        let unison = p.choice(UNISON_P).min(MAX_UNISON - 1) + 1;
        let copies_moved = self.mm.on(D_UNI_DETUNE) || self.mm.on(D_UNI_SPREAD);
        let mut src = [0.0f32; sm::N_SOURCES];
        src[sm::SRC_LFO] = lfo;
        src[sm::SRC_WHEEL] = self.wheel;
        for v in self.voices.iter_mut().filter(|v| v.active()) {
            src[sm::SRC_AMP] = v.amp.level;
            src[sm::SRC_ENV2] = v.env2.level;
            src[sm::SRC_VEL] = v.vel_raw;
            src[sm::SRC_KEY] = sm::key_value(v.note);
            src[sm::SRC_RND] = v.m.rnd;
            sm::aim_voice(&self.mm, &mut v.m, &src, &self.macro_off, &self.bases, &self.infos, D_LFO_RATE, sm::TICK, |d, x, base| match d {
                D_POS | D_SUB | D_NOISE | D_UNI_SPREAD | D_ENV2_POS | D_KEY | D_LFO_POS | D_LFO_AMP => x / 100.0,
                D_RES => 2.0 - 1.95 * (x / 100.0),
                D_ENV2_CUT => x / 100.0 * 6.0,
                D_LFO_CUTOFF => x / 100.0 * 4.0,
                D_WARP => x / 100.0,
                D_LEVEL => sm::level_gain(x, base),
                _ => x,
            });
            if copies_moved {
                let detune = if self.mm.on(D_UNI_DETUNE) { v.m.get(D_UNI_DETUNE) } else { self.bases[D_UNI_DETUNE] };
                let spread = if self.mm.on(D_UNI_SPREAD) { v.m.get(D_UNI_SPREAD) } else { self.bases[D_UNI_SPREAD] / 100.0 };
                v.copies = unison_copies(unison, detune, spread);
            }
        }
    }

    fn update(&mut self) {
        let sr = self.sr;
        let v = |i| self.p.v(i);
        self.amp_rates = AdsrRates::new(v(AMP_A), v(AMP_A + 1), v(AMP_A + 2) / 100.0, v(AMP_A + 3), sr);
        self.env2_rates = AdsrRates::new(v(ENV2_A), v(ENV2_A + 1), v(ENV2_A + 2) / 100.0, v(ENV2_A + 3), sr);
        let glide = v(GLIDE);
        self.glide_coef = if glide < 0.5 { 0.0 } else { crate::util::time_coef(glide / 3.0, sr) };
        self.level.set(db_to_gain(v(LEVEL)));
        self.cutoff.set(v(CUTOFF));
        self.position.set(v(POSITION) / 100.0);
        self.damping.set(2.0 - 1.95 * (v(RESONANCE) / 100.0));
    }

    fn start_note(&mut self, note: u8, vel: u8) {
        if vel == 0 {
            self.stop_note(note);
            return;
        }
        self.counter += 1;
        let freq = note_hz(f32::from(note.min(127)));
        let idx = pick_voice(self.voices.iter().map(|v| (v.active(), v.note, v.held, v.amp.level)), note);
        let sens = self.p.v(VEL_SENS) / 100.0;
        let glide = self.glide_coef > 0.0;
        let start = if glide { self.last_freq.unwrap_or(freq) } else { freq };
        let counter = self.counter;
        if let Some(v) = self.voices.get_mut(idx) {
            let was_active = v.active();
            let raw = f32::from(vel.min(127)) / 127.0;
            v.note = note;
            v.vel = (1.0 - sens) + sens * raw;
            v.vel_raw = raw;
            v.target = freq;
            v.freq = if was_active && glide { v.freq } else { start };
            if !was_active {
                v.restart();
            }
            v.amp.gate_on();
            v.env2.gate_on();
            v.age = counter;
            v.held = true;
            v.sustained = false;
        }
        self.mod_note_on(idx);
        self.last_freq = Some(freq);
    }

    fn stop_note(&mut self, note: u8) {
        let sustain = self.sustain;
        for v in self.voices.iter_mut().filter(|v| v.held && v.note == note) {
            v.held = false;
            if sustain {
                v.sustained = true;
            } else {
                v.amp.gate_off();
                v.env2.gate_off();
            }
        }
    }

    fn set_sustain(&mut self, on: bool) {
        self.sustain = on;
        if !on {
            for v in self.voices.iter_mut().filter(|v| v.sustained) {
                v.sustained = false;
                v.amp.gate_off();
                v.env2.gate_off();
            }
        }
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(n, v) => self.start_note(n, v),
            EventKind::Off(n) => self.stop_note(n),
            EventKind::AllOff => {
                for v in &mut self.voices {
                    v.held = false;
                    v.sustained = false;
                    v.amp.gate_off();
                    v.env2.gate_off();
                }
            }
            EventKind::Sustain(on) => self.set_sustain(on),
            EventKind::Bend(b) => self.bend = if b.is_finite() { b.clamp(-1.0, 1.0) } else { 0.0 },
            EventKind::Mod(m) => self.wheel = if m.is_finite() { m.clamp(0.0, 1.0) } else { 0.0 },
            EventKind::ResetControllers => {
                self.set_sustain(false);
                self.bend = 0.0;
                self.wheel = 0.0;
            }
        }
    }

    /// The bend and the mod wheel's vibrato, cents.
    #[inline]
    fn pitch_cents(&self) -> f32 {
        let mut cents = self.bend * self.p.v(BEND_RANGE) * 100.0;
        if self.wheel > 0.0 {
            cents += self.p.v(MOD_WHEEL) * self.wheel * (std::f32::consts::TAU * self.vib_phase).sin();
        }
        cents
    }

    /// One sample, left and right (the same unless unison spreads the copies).
    #[inline]
    fn render_sample(&mut self) -> (f32, f32) {
        if self.mm.active() {
            self.mod_step();
        }
        let sr = self.sr;
        let bank = self.p.choice(TABLE).min(tables::BANKS.len() - 1);
        let unison = self.p.choice(UNISON_P).min(MAX_UNISON - 1) + 1;
        let warp_mode = self.p.choice(WARP_MODE).min(WARP_MODES.len() - 1);
        let k = self.damping.next();
        let cutoff = self.cutoff.next();
        let base_pos = self.position.next();
        let nyq = sr * 0.45;
        let cents = self.pitch_cents();
        let cents_base = cents;
        self.vib_phase = (self.vib_phase + VIBRATO_HZ / sr).fract();
        let lfo = lfo_shape(self.p.choice(LFO_WAVE), self.lfo_phase);
        self.lfo_phase = (self.lfo_phase + self.lfo_rate() / sr).fract();
        let cents = cents + self.p.v(LFO_PITCH) * lfo + self.p.v(PITCH) * 100.0 + self.p.v(FINE);
        let lfo_oct = self.p.v(LFO_CUTOFF) / 100.0 * 4.0 * lfo;
        let lfo_amp = 1.0 - self.p.v(LFO_AMP) / 100.0 * 0.5 * (1.0 - lfo);
        // Shared position offsets: the LFO (±half the table at full) and the mod wheel.
        let shared_pos = base_pos + self.p.v(LFO_POS) / 100.0 * 0.5 * lfo + self.p.v(WHEEL_POS) / 100.0 * self.wheel;
        let env2_pos = self.p.v(ENV2_POS) / 100.0;
        let vel_pos = self.p.v(VEL_POS) / 100.0;
        let env2_oct = self.p.v(ENV2_CUTOFF) / 100.0 * 6.0;
        let kind = self.p.choice(FILTER_TYPE);
        let key_track = self.p.v(KEY_TRACK) / 100.0;
        let sub_level = self.p.v(SUB_LEVEL) / 100.0;
        let noise_level = self.p.v(NOISE_LEVEL) / 100.0;
        let ratio = if cents == 0.0 { 1.0 } else { 2f32.powf(cents / 1200.0) };
        let copies = unison_copies(unison, self.p.v(UNISON_DETUNE), self.p.v(UNISON_SPREAD) / 100.0);
        // The highest copy's ratio picks the band limit for the whole stack.
        let top_ratio = copies.iter().take(unison).fold(1.0f32, |m, c| m.max(c.0));
        let modded = self.mm.active();
        let own_copies = self.mm.on(D_UNI_DETUNE) || self.mm.on(D_UNI_SPREAD);
        let sh = Shared {
            cents: cents_base,
            lfo,
            wheel: self.wheel,
            lfo_pitch: self.p.v(LFO_PITCH),
            pitch: self.p.v(PITCH),
            fine: self.p.v(FINE),
            ratio,
            base_pos,
            lfo_pos: self.p.v(LFO_POS) / 100.0,
            wheel_pos: self.p.v(WHEEL_POS) / 100.0,
            shared_pos,
            env2_pos,
            k,
            cutoff,
            env2_oct,
            lfo_oct,
            lfo_amp,
            key_track,
            sub_level,
            noise_level,
            gain: 1.0,
            warp: self.p.v(WARP_AMOUNT) / 100.0,
        };
        let (mut out_l, mut out_r) = (0.0, 0.0);
        for v in &mut self.voices {
            if !v.active() {
                continue;
            }
            let Shared { ratio, shared_pos, env2_pos, k, cutoff, env2_oct, lfo_oct, lfo_amp, key_track, sub_level, noise_level, gain, warp, .. } =
                if modded { sh.modded(&self.mm, &v.m) } else { sh };
            let (copies, top_ratio) =
                if own_copies { (v.copies, v.copies.iter().take(unison).fold(1.0f32, |m, c| m.max(c.0))) } else { (copies, top_ratio) };
            if self.glide_coef > 0.0 {
                v.freq = v.target + (v.freq - v.target) * self.glide_coef;
            } else {
                v.freq = v.target;
            }
            let a = v.amp.next(&self.amp_rates);
            let e2 = v.env2.next(&self.env2_rates);
            let pos = (shared_pos + env2_pos * e2 + vel_pos * v.vel_raw).clamp(0.0, 1.0);
            let inc = (v.freq * ratio / sr).clamp(0.0, 0.49);
            let level = tables::level_for(inc * top_ratio);
            let (mut l, mut r) = (0.0, 0.0);
            // Warp off (or at 0): the plain read, as before.
            let warped = (warp_mode != 0 && warp > 0.0).then(|| Warp::new(warp_mode, warp.min(1.0), inc * top_ratio));
            let sync = v.sync_ph.iter_mut().zip(v.sync_fix.iter_mut());
            for ((ph, (sp, fix)), &(cr, gl, gr)) in v.phases.iter_mut().zip(sync).zip(copies.iter()).take(unison) {
                let s = match &warped {
                    Some(w) => w.read(self.t, bank, pos, *ph, inc * cr, sp, fix),
                    None => self.t.read(bank, pos, level, *ph),
                };
                *ph += inc * cr;
                if *ph >= 1.0 {
                    *ph -= 1.0;
                }
                if !ph.is_finite() || *ph >= 1.0 {
                    *ph = 0.0;
                }
                l += s * gl;
                r += s * gr;
            }
            let mut centre = 0.0;
            if sub_level > 0.0 {
                centre += sub_level * v.sub.next(Waveform::Square, inc * 0.5);
            }
            if noise_level > 0.0 {
                v.noise ^= v.noise << 13;
                v.noise ^= v.noise >> 17;
                v.noise ^= v.noise << 5;
                centre += noise_level * (v.noise as f32 / u32::MAX as f32 * 2.0 - 1.0);
            }
            let track = if key_track > 0.0 { key_track * (f32::from(v.note) - 60.0) / 12.0 } else { 0.0 };
            let fc = (cutoff * 2f32.powf(env2_oct * e2 + track + lfo_oct)).clamp(20.0, nyq);
            let g = (std::f32::consts::PI * fc / sr).tan();
            let gain = a * v.vel * lfo_amp * gain;
            if unison == 1 {
                let mut y = v.svf.run(l + centre, g, k, kind);
                if kind == 1 {
                    y = v.svf2.run(y, g, k, 1);
                }
                out_l += y * gain;
                out_r += y * gain;
                continue;
            }
            let mut yl = v.svf.run(l + centre, g, k, kind);
            let mut yr = v.svf_r.run(r + centre, g, k, kind);
            if kind == 1 {
                yl = v.svf2.run(yl, g, k, 1);
                yr = v.svf2_r.run(yr, g, k, 1);
            }
            out_l += yl * gain;
            out_r += yr * gain;
        }
        (out_l * 0.3, out_r * 0.3)
    }
}

impl Plugin for WavetableSynth {
    param_plumbing!(WAVETABLE_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        for s in [&mut self.level, &mut self.cutoff, &mut self.position, &mut self.damping] {
            s.set_time(20.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn set_prepared_state(&mut self, data: &[f64]) -> bool {
        self.mm.load(data, MOD_DESTS.len());
        for v in &mut self.voices {
            v.m.fresh = true;
        }
        self.tick_left = 0;
        true
    }

    fn reset(&mut self) {
        for (i, v) in self.voices.iter_mut().enumerate() {
            *v = Voice::new(0x2000 + i as u32 * 37);
        }
        self.rng = sm::RANDOM_SEED;
        self.tick_left = 0;
        self.events.clear();
        self.last_freq = None;
        for s in [&mut self.level, &mut self.cutoff, &mut self.position, &mut self.damping] {
            s.snap();
        }
        self.sustain = false;
        self.bend = 0.0;
        self.wheel = 0.0;
        self.vib_phase = 0.0;
        self.lfo_phase = 0.0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        let mut evs = [super::synth::Event { offset: 0, kind: EventKind::AllOff }; MAX_EVENTS];
        let count = {
            let sorted = self.events.sorted();
            for (d, s) in evs.iter_mut().zip(sorted) {
                *d = *s;
            }
            sorted.len().min(MAX_EVENTS)
        };
        self.events.clear();
        let mut next = 0;
        for n in 0..frames {
            while let Some(e) = evs.get(next).filter(|_| next < count) {
                if e.offset > n && n + 1 < frames {
                    break;
                }
                let kind = e.kind;
                self.apply(kind);
                next += 1;
            }
            let (l, r) = self.render_sample();
            let g = self.level.next();
            let (l, r) = (l * g, r * g);
            if ch == 1 {
                if let Some(x) = io.first_mut().and_then(|b| b.get_mut(n)) {
                    *x = 0.5 * (l + r);
                }
                continue;
            }
            for (c, b) in io.iter_mut().take(ch).enumerate() {
                if let Some(x) = b.get_mut(n) {
                    *x = if c % 2 == 0 { l } else { r };
                }
            }
        }
        for e in evs.iter().take(count).skip(next) {
            self.apply(e.kind);
        }
    }

    fn tail_samples(&self) -> usize {
        ms_to_samples(self.p.v(AMP_A + 3) * (SILENT.ln() / 0.001f32.ln()), self.sr) as usize
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.events.push(offset, EventKind::On(note, velocity));
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.events.push(offset, EventKind::Off(note));
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        if let Some(kind) = event_of(msg) {
            self.events.push(offset, kind);
        }
    }

    fn all_notes_off(&mut self) {
        self.events.push(0, EventKind::AllOff);
    }
}

#[cfg(test)]
mod tests;

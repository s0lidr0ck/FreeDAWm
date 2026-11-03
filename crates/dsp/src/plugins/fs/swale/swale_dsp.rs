//! A faithful translation of `Swale/Source/SwaleDSP.h`: automatic volume swells that land in
//! time (Pick / One-Shot / Rhythmic), the Clean or Slow Gear (JFET shunt) VCA, Bloom, Alternate.
//!
//! Same state, order of operations, constants and arithmetic types as the C++ (`float` stays
//! `f32`, `double` stays `f64`). C++ behaviour Rust does not share is reproduced deliberately:
//! - `(int) (v + 0.5f)` for the choice indexes is `as i32` (Rust saturates where C++ is undefined;
//!   the parameter layer clamps first, so only in-range values reach it);
//! - `++trigCount` wraps (`wrapping_add`) where the C++ `int` would overflow;
//! - the UI outputs (`envOut`, `detOut`, `trigSeq`, `bpmOut`) are plain fields read after
//!   `process` (the C++ atomics exist only for the editor's thread);
//! - `std::max`/`std::min` on doubles never see a NaN here, so `f64::max`/`min` match them.
//!
//! `ScopedNoDenormals` (the wrapper's flush-to-zero) has no safe Rust equivalent. No decision in
//! this engine can see a denormal: every comparison is against a threshold of at least ~3e-5
//! (`thresh * rearmRatio`, 10^(-90/20)) or against `slowEnv * onsetRatio` only once the level is
//! over `thresh`, and the stage position `lin` is a double clamped at 0; so no DAZ emulation is
//! needed (see `fs_swale`'s tests: `denormal_levels_never_trigger`).

/// Parameter indexes (`SwaleDSP::ParamIndex`), param1..param27.
pub const P_ATTACK: usize = 0;
pub const P_HOLD: usize = 1;
pub const P_RELEASE: usize = 2;
pub const P_SENS: usize = 3;
pub const P_MIX: usize = 4;
pub const P_LEVEL: usize = 5;
pub const P_MODE: usize = 6;
pub const P_BYPASS: usize = 7;
pub const P_TRIM: usize = 8;
pub const P_CURVE: usize = 9;
pub const P_RETRIG: usize = 10;
pub const P_DET_HP: usize = 11;
pub const P_FLOOR: usize = 12;
pub const P_BLOOM: usize = 13;
pub const P_VCA: usize = 14;
pub const P_ALTERNATE: usize = 15;
pub const P_ATK_SYNC: usize = 16;
pub const P_ATK_NOTE: usize = 17;
pub const P_HOLD_SYNC: usize = 18;
pub const P_HOLD_NOTE: usize = 19;
pub const P_REL_SYNC: usize = 20;
pub const P_REL_NOTE: usize = 21;
pub const P_RATE: usize = 22;
pub const P_PEAK_BEAT: usize = 23;
pub const P_TEMPO_SRC: usize = 24;
pub const P_BPM: usize = 25;
pub const P_PHRASE: usize = 26;
pub const NUM_PARAMS: usize = 27;

pub const PI: f64 = std::f64::consts::PI;
pub const TWO_PI: f64 = std::f64::consts::TAU;

// Slow Gear VCA: series resistor into a 2SK30A-class JFET shunting the signal to ground, gate
// driven by the swell ramp.
pub const SG_RS: f64 = 47000.0;
pub const SG_RL: f64 = 1.0e6;
pub const SG_IDSS: f64 = 3.0e-3;
pub const SG_VP: f64 = -1.6;
/// Volts at the FET for 0 dBFS.
pub const SG_VIN: f64 = 0.03;
pub const SG_BETA: f64 = 2.0 * SG_IDSS / (SG_VP * SG_VP);

// ---------------------------------------------------------------- knob -> unit maps
// The UI prints these same numbers (`swale_config`). Each keeps the C++ arithmetic type.

/// Attack / Release: 5 ms .. 4 s.
pub fn knob_to_stage_sec(v: f32) -> f64 {
    0.005 * 800.0f64.powf(f64::from(v))
}
/// 0 .. 4 s.
pub fn knob_to_hold_sec(v: f32) -> f64 {
    4.0 * f64::from(v) * f64::from(v)
}
pub fn knob_to_sens_db(v: f32) -> f32 {
    -20.0 - 46.0 * v
}
pub fn knob_to_level_db(v: f32) -> f32 {
    -24.0 + 36.0 * v
}
pub fn knob_to_trim_db(v: f32) -> f32 {
    -12.0 + 36.0 * v
}
/// Below 0.005 the floor is silent.
pub fn knob_to_floor_db(v: f32) -> f32 {
    -60.0 + 48.0 * v
}
pub fn knob_to_det_hp_hz(v: f32) -> f32 {
    20.0 * 25.0f32.powf(v)
}
/// Below the trigger line.
pub fn knob_to_rearm_db(v: f32) -> f32 {
    -24.0 + 17.0 * v
}
pub fn knob_to_bpm(v: f32) -> f32 {
    40.0 + 200.0 * v
}

/// Note values, long -> short; index 18 is "None" (Hold only).
const BEATS: [f32; 19] =
    [16.0, 8.0, 6.0, 4.0, 3.0, 8.0 / 3.0, 2.0, 1.5, 4.0 / 3.0, 1.0, 0.75, 2.0 / 3.0, 0.5, 0.375, 1.0 / 3.0, 0.25, 1.0 / 6.0, 0.125, 0.0];

/// `noteBeats`: the beats of a note index, clamped to 0..18.
pub fn note_beats(i: i32) -> f32 {
    BEATS[i.clamp(0, 18) as usize]
}

/// `clampT`: `v < lo ? lo : (v > hi ? hi : v)` (a NaN passes through, as in the C++).
fn clamp_t<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Stage position -> VCA opening (`shape`); the display draws through it too.
pub fn shape(curve: i32, x: f64) -> f64 {
    let t = clamp_t(x, 0.0, 1.0);
    if curve == 0 {
        return t;
    }
    if curve == 1 {
        return ((4.6 * t).exp() - 1.0) / 98.484; // slow start, arrives late: even in dB
    }
    if curve == 2 {
        return 1.0 - ((4.6 * (1.0 - t)).exp() - 1.0) / 98.484; // fast start, eases in to the top
    }
    t * t * t * (t * (6.0 * t - 15.0) + 10.0) // S-curve
}

/// `isSane`: finite (tested on the bits, as the C++ does under /fp:fast) and inside ±100.
fn is_sane(v: f32) -> bool {
    let bits = v.to_bits();
    if (bits & 0x7f80_0000) == 0x7f80_0000 {
        return false; // NaN or infinity
    }
    v > -100.0 && v < 100.0
}

/// JFET shunt attenuator with the usual gate linearisation (`fetVca`).
fn fet_vca(vin: f64, a: f64) -> f64 {
    let g = 1.0 / SG_RS + 1.0 / SG_RL;
    let drive = vin.abs() / SG_RS;
    // region test: on (clean resistor) while v <= 2a; fully off while the gate stays below pinch-off
    let mut v = if a > 0.0 { drive / (SG_BETA * a + g) } else { drive / g };
    if v > (2.0 * a).abs() {
        let a2 = SG_BETA / 8.0;
        let b2 = 0.5 * SG_BETA * a + g;
        let c2 = 0.5 * SG_BETA * a * a - drive;
        v = (-b2 + (b2 * b2 - 4.0 * a2 * c2).max(0.0).sqrt()) / (2.0 * a2);
    }
    if vin < 0.0 { -v } else { v }
}

/// `std::max (a, b)` on floats: `(a < b) ? b : a`.
fn max_f(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}

/// The Swale engine (`SwaleDSP`). Plain data: no heap fields.
#[derive(Debug, Clone)]
pub struct SwaleDsp {
    // ---- outputs back to the UI
    env_out: f32,
    det_out: f32,
    trig_seq: i32,
    bpm_out: f32,

    // ---- parameter state
    attack_knob: f32,
    hold_knob: f32,
    release_knob: f32,
    thresh: f32,
    mix_knob: f32,
    level_lin: f32,
    trim_lin: f32,
    mode_idx: i32,
    curve_idx: i32,
    vca_idx: i32,
    phrase_on: bool,
    bypassed: bool,
    atk_synced: bool,
    hold_synced: bool,
    rel_synced: bool,
    peak_on_beat: bool,
    atk_note_idx: i32,
    hold_note_idx: i32,
    rel_note_idx: i32,
    rate_idx: i32,
    tempo_src_idx: i32,
    retrig_knob: f32,
    rearm_ratio: f32,
    onset_ratio: f32,
    det_hp_coef: f32,
    floor_lin: f32,
    bloom_knob: f32,
    alt_knob: f32,
    manual_bpm: f32,

    // ---- smoothed
    trim_sm: f32,
    level_sm: f32,
    mix_sm: f32,
    floor_sm: f32,

    // ---- host transport
    host_bpm: f32,
    host_playing: bool,

    // ---- engine state
    fs: f64,
    hp_x: f32,
    hp_y: f32,
    fast_env: f32,
    slow_env: f32,
    fast_atk: f32,
    fast_rel: f32,
    slow_atk: f32,
    slow_rel: f32,
    armed: bool,
    /// 0 idle, 1 close, 2 attack, 3 hold, 4 release.
    stage: i32,
    /// Linear stage position 0..1.
    lin: f64,
    atk_step: f64,
    rel_step: f64,
    close_step: f64,
    hold_samples: f64,
    hold_count: f64,
    below_count: f64,
    since_trig: f64,
    atk_sec: f64,
    hold_sec: f64,
    rel_sec: f64,
    cycle_sec: f64,
    /// Rhythmic: 0..1 across one Rate.
    phase: f64,
    last_pos: f64,
    gain_sm: f32,
    gain_coef: f32,
    side: i32,
    pan_l: f32,
    pan_r: f32,
    pan_coef: f32,
    trig_count: i32,
    bpm_now: f32,
    ctl_counter: i32,
    ui_counter: i32,
    bloom_g: f32,
    /// Bloom filter states.
    bl1: [f32; 2],
    bl2: [f32; 2],
}

impl Default for SwaleDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl SwaleDsp {
    /// `SwaleDSP()`: the member defaults, then `prepare (48000.0)`.
    pub fn new() -> Self {
        let mut s = SwaleDsp {
            env_out: 0.0,
            det_out: 0.0,
            trig_seq: 0,
            bpm_out: 120.0,
            attack_knob: 0.55,
            hold_knob: 0.0,
            release_knob: 0.5,
            thresh: 0.0071,
            mix_knob: 1.0,
            level_lin: 1.0,
            trim_lin: 1.0,
            mode_idx: 0,
            curve_idx: 1,
            vca_idx: 0,
            phrase_on: true,
            bypassed: false,
            atk_synced: false,
            hold_synced: false,
            rel_synced: false,
            peak_on_beat: true,
            atk_note_idx: 9,
            hold_note_idx: 18,
            rel_note_idx: 12,
            rate_idx: 6,
            tempo_src_idx: 0,
            retrig_knob: 0.7,
            rearm_ratio: 0.25,
            onset_ratio: 2.41,
            det_hp_coef: 0.99,
            floor_lin: 0.0,
            bloom_knob: 0.0,
            alt_knob: 0.0,
            manual_bpm: 120.0,
            trim_sm: 1.0,
            level_sm: 1.0,
            mix_sm: 1.0,
            floor_sm: 0.0,
            host_bpm: 0.0,
            host_playing: false,
            fs: 48000.0,
            hp_x: 0.0,
            hp_y: 0.0,
            fast_env: 0.0,
            slow_env: 0.0,
            fast_atk: 0.1,
            fast_rel: 0.002,
            slow_atk: 0.001,
            slow_rel: 0.0002,
            armed: true,
            stage: 0,
            lin: 0.0,
            atk_step: 0.0,
            rel_step: 0.0,
            close_step: 0.0,
            hold_samples: 0.0,
            hold_count: 0.0,
            below_count: 0.0,
            since_trig: 0.0,
            atk_sec: 0.2,
            hold_sec: 0.0,
            rel_sec: 0.2,
            cycle_sec: 1.0,
            phase: 0.0,
            last_pos: 0.0,
            gain_sm: 0.0,
            gain_coef: 0.05,
            side: 0,
            pan_l: 1.0,
            pan_r: 1.0,
            pan_coef: 0.004,
            trig_count: 0,
            bpm_now: 120.0,
            ctl_counter: 0,
            ui_counter: 0,
            bloom_g: 1.0,
            bl1: [0.0; 2],
            bl2: [0.0; 2],
        };
        s.prepare(48000.0);
        s
    }

    // ------------------------------------------------------------------ host transport

    /// `setHostTransport (playing, bpm, ppq, ppqValid)`.
    pub fn set_host_transport(&mut self, playing: bool, bpm: f32, ppq: f64, ppq_valid: bool) {
        self.host_playing = playing;
        self.host_bpm = if (20.0..=999.0).contains(&bpm) { bpm } else { 0.0 };
        if ppq_valid {
            self.lock_to_host(ppq);
        }
    }

    pub fn host_bpm(&self) -> f32 {
        self.host_bpm
    }

    // ------------------------------------------------------------------ events

    /// `setParameter (index, v)`.
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        let unit = |v: f32| clamp_t(v, 0.0f32, 1.0);
        match index {
            P_ATTACK => self.attack_knob = unit(v),
            P_HOLD => self.hold_knob = unit(v),
            P_RELEASE => self.release_knob = unit(v),
            P_SENS => self.thresh = 10.0f32.powf(knob_to_sens_db(unit(v)) / 20.0),
            P_MIX => self.mix_knob = unit(v),
            P_LEVEL => self.level_lin = 10.0f32.powf(knob_to_level_db(unit(v)) / 20.0),
            P_MODE => {
                let m = clamp_t((v + 0.5) as i32, 0, 2);
                if m != self.mode_idx {
                    self.mode_idx = m;
                    self.stage = if self.lin > 0.0 { 4 } else { 0 };
                    self.armed = true;
                }
            }
            P_BYPASS => self.bypassed = v > 0.5,
            P_TRIM => self.trim_lin = 10.0f32.powf(knob_to_trim_db(unit(v)) / 20.0),
            P_CURVE => self.curve_idx = clamp_t((v + 0.5) as i32, 0, 3),
            P_RETRIG => {
                self.retrig_knob = unit(v);
                // how far the note must fall before the next one swells
                self.rearm_ratio = 10.0f32.powf(knob_to_rearm_db(self.retrig_knob) / 20.0);
                // ...or how hard a fresh pick must jump over a note that is still ringing
                self.onset_ratio = 3.6 - 1.7 * self.retrig_knob;
            }
            P_DET_HP => self.det_hp_coef = (-TWO_PI * 20.0 * 25.0f64.powf(f64::from(unit(v))) / self.fs).exp() as f32,
            P_FLOOR => {
                let k = unit(v);
                self.floor_lin = if k < 0.005 { 0.0 } else { 10.0f32.powf(knob_to_floor_db(k) / 20.0) };
            }
            P_BLOOM => self.bloom_knob = unit(v),
            P_VCA => self.vca_idx = clamp_t((v + 0.5) as i32, 0, 1),
            P_ALTERNATE => self.alt_knob = unit(v),
            P_ATK_SYNC => self.atk_synced = v > 0.5,
            P_ATK_NOTE => self.atk_note_idx = clamp_t((v + 0.5) as i32, 0, 17),
            P_HOLD_SYNC => self.hold_synced = v > 0.5,
            P_HOLD_NOTE => self.hold_note_idx = clamp_t((v + 0.5) as i32, 0, 18),
            P_REL_SYNC => self.rel_synced = v > 0.5,
            P_REL_NOTE => self.rel_note_idx = clamp_t((v + 0.5) as i32, 0, 17),
            P_RATE => self.rate_idx = clamp_t((v + 0.5) as i32, 0, 17),
            P_PEAK_BEAT => self.peak_on_beat = v > 0.5,
            P_TEMPO_SRC => self.tempo_src_idx = clamp_t((v + 0.5) as i32, 0, 1),
            P_BPM => self.manual_bpm = knob_to_bpm(unit(v)),
            P_PHRASE => self.phrase_on = v > 0.5,
            _ => {}
        }
    }

    // ------------------------------------------------------------------ init

    /// `prepare (sampleRate)`.
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs = if sample_rate > 0.0 { sample_rate } else { 48000.0 };

        self.fast_atk = self.time_coef(0.0003);
        self.fast_rel = self.time_coef(0.012);
        self.slow_atk = self.time_coef(0.025);
        self.slow_rel = self.time_coef(0.120);
        self.gain_coef = self.time_coef(0.0007);
        self.pan_coef = self.time_coef(0.004);
        self.close_step = 1.0 / (0.002 * self.fs);
        self.det_hp_coef = (-TWO_PI * 20.0 * 25.0f64.powf(0.3) / self.fs).exp() as f32;
        self.rearm_ratio = 10.0f32.powf(knob_to_rearm_db(0.7) / 20.0);
        self.level_lin = 10.0f32.powf(knob_to_level_db(0.6667) / 20.0);
        self.level_sm = self.level_lin;
        self.thresh = 10.0f32.powf(knob_to_sens_db(0.5) / 20.0);

        // engine state, so a re-prepare never leaves a half-open swell behind
        self.hp_x = 0.0;
        self.hp_y = 0.0;
        self.fast_env = 0.0;
        self.slow_env = 0.0;
        self.armed = true;
        self.stage = 0;
        self.lin = 0.0;
        self.hold_count = 0.0;
        self.below_count = 0.0;
        self.since_trig = 0.0;
        self.phase = 0.0;
        self.last_pos = 0.0;
        self.gain_sm = 0.0;
        self.side = 0;
        self.pan_l = 1.0;
        self.pan_r = 1.0;
        self.bl1 = [0.0; 2];
        self.bl2 = [0.0; 2];
        self.ctl_counter = 0;
        self.ui_counter = 0;

        self.update_times();
    }

    /// The sample rate `prepare` settled on.
    pub fn sample_rate(&self) -> f64 {
        self.fs
    }

    /// Starts every parameter glide at its target (`trimSm`, `levelSm`, `mixSm`, `floorSm`).
    /// Not in the C++ (whose `prepare` restarts Level from its default): the DAW's reset uses it so
    /// a Play / Stop / Locate never re-glides from defaults.
    pub fn seed_glides(&mut self) {
        self.trim_sm = self.trim_lin;
        self.level_sm = self.level_lin;
        self.mix_sm = self.mix_knob;
        self.floor_sm = self.floor_lin;
    }

    /// Puts Bloom's coefficient back to its constructed value (`bloomG = 1`). The C++ `prepare`
    /// leaves the last one (recomputed at the first control tick before Bloom ever uses it); the
    /// DAW's reset clears it too, so nothing of the module's past survives a reset.
    pub fn clear_bloom_coef(&mut self) {
        self.bloom_g = 1.0;
    }

    // ------------------------------------------------------------------ main

    /// `process (channels, numChannels, numSamples)`: the first one or two of `io` (`nc`), at
    /// most `frames` samples (fewer if a buffer is shorter).
    pub fn process(&mut self, io: &mut [Vec<f32>], nc: usize, frames: usize) {
        if nc < 1 {
            return;
        }
        let Some((first, rest)) = io.split_first_mut() else { return };
        let mut ch_r = if nc > 1 { rest.first_mut() } else { None };
        let mut n = frames.min(first.len());
        if let Some(r) = ch_r.as_deref() {
            n = n.min(r.len());
        }
        let stereo = ch_r.is_some();

        for i in 0..n {
            let x0 = first[i];
            let x1 = match ch_r.as_deref() {
                Some(r) => r[i],
                None => x0,
            };

            self.trim_sm += (self.trim_lin - self.trim_sm) * 0.005;
            self.level_sm += (self.level_lin - self.level_sm) * 0.005;
            self.mix_sm += (self.mix_knob - self.mix_sm) * 0.004;
            self.floor_sm += (self.floor_lin - self.floor_sm) * 0.004;

            if self.ctl_counter == 0 {
                self.update_times();
            }
            self.ctl_counter = (self.ctl_counter + 1) % 16;

            // detector: mono sum, low cut, fast and slow followers
            let mono = 0.5 * (x0 + x1) * self.trim_sm;
            let hpv = self.det_hp_coef * (self.hp_y + mono - self.hp_x);
            self.hp_x = mono;
            self.hp_y = hpv;
            let rect = hpv.abs();
            self.fast_env += (rect - self.fast_env) * if rect > self.fast_env { self.fast_atk } else { self.fast_rel };
            self.slow_env += (rect - self.slow_env) * if rect > self.slow_env { self.slow_atk } else { self.slow_rel };

            // the Rhythmic clock always runs, so switching modes mid-song stays on the grid
            self.phase += 1.0 / (self.cycle_sec.max(0.02) * self.fs);
            if self.phase >= 1.0 {
                self.phase -= self.phase.floor();
            }

            if self.mode_idx == 2 {
                self.lin = self.rhythmic_lin();
            } else {
                self.run_triggered();
            }

            let open = shape(self.curve_idx, self.lin) as f32;
            self.gain_sm += (open - self.gain_sm) * self.gain_coef;

            // alternate sides: the pan only moves while the VCA is shut
            let w = self.alt_knob;
            self.pan_l += ((if self.side == 0 { 1.0 } else { 1.0 - w }) - self.pan_l) * self.pan_coef;
            self.pan_r += ((if self.side == 1 { 1.0 } else { 1.0 - w }) - self.pan_r) * self.pan_coef;

            let (mut y0, mut y1);

            if self.bypassed {
                y0 = x0;
                y1 = x1;
            } else {
                let (mut wet0, mut wet1);

                if self.vca_idx == 0 {
                    let g = self.floor_sm + (1.0 - self.floor_sm) * self.gain_sm;
                    wet0 = x0 * (self.trim_sm * g);
                    wet1 = x1 * (self.trim_sm * g);
                } else {
                    // Gate drive: the small-signal gain the swell asks for (from Vgs = 0, about
                    // -44 dB), pushed 0.6 V past pinch-off at the top so a fully open VCA is clean.
                    let gt = clamp_t(f64::from(self.gain_sm), 1.0e-4, 1.0);
                    let k = clamp_t((gt - 0.85) / 0.15, 0.0, 1.0);
                    let a = (-SG_VP).min((1.0 / SG_RS + 1.0 / SG_RL) * (1.0 / gt - 1.0) / SG_BETA) - 0.6 * k * k;
                    let makeup = (SG_RS + SG_RL) / (SG_RL * SG_VIN);
                    let bleed = self.floor_sm * (1.0 - self.gain_sm) * self.trim_sm;
                    wet0 = (fet_vca(f64::from(x0 * self.trim_sm) * SG_VIN, a) * makeup) as f32 + x0 * bleed;
                    wet1 = (fet_vca(f64::from(x1 * self.trim_sm) * SG_VIN, a) * makeup) as f32 + x1 * bleed;
                }

                if self.bloom_knob > 0.001 {
                    let mut wet = [wet0, wet1];
                    #[allow(clippy::needless_range_loop)] // the C++ loop, indexing three arrays by channel
                    for c in 0..2 {
                        let v1 = (wet[c] - self.bl1[c]) * self.bloom_g;
                        let ya = v1 + self.bl1[c];
                        self.bl1[c] = ya + v1;
                        let v2 = (ya - self.bl2[c]) * self.bloom_g;
                        let yb = v2 + self.bl2[c];
                        self.bl2[c] = yb + v2;
                        wet[c] = yb;
                    }
                    wet0 = wet[0];
                    wet1 = wet[1];
                }

                // Alternate is a stereo move; mono ignores it.
                if stereo {
                    wet0 *= self.pan_l;
                    wet1 *= self.pan_r;
                }

                y0 = (x0 * (self.trim_sm * (1.0 - self.mix_sm)) + wet0 * self.mix_sm) * self.level_sm;
                y1 = (x1 * (self.trim_sm * (1.0 - self.mix_sm)) + wet1 * self.mix_sm) * self.level_sm;

                // NaN / blow-up guard.
                if !is_sane(y0) || !is_sane(y1) {
                    y0 = 0.0;
                    y1 = 0.0;
                    self.bl1 = [0.0; 2];
                    self.bl2 = [0.0; 2];
                    self.gain_sm = 0.0;
                    self.lin = 0.0;
                    self.stage = 0;
                }
            }

            first[i] = y0;
            if let Some(r) = ch_r.as_deref_mut() {
                r[i] = y1;
            }

            // ---- UI feedback, about every 800 samples
            self.ui_counter += 1;
            if self.ui_counter >= 800 {
                self.ui_counter = 0;
                self.env_out = self.gain_sm;
                self.det_out = self.fast_env;
                self.bpm_out = self.bpm_now;
            }
        }
    }

    // ------------------------------------------------------------------ outputs

    /// `envOut`: the VCA opening 0..1 (after Curve), as last published.
    pub fn env_out(&self) -> f32 {
        self.env_out
    }
    /// `detOut`: the detector level, linear, as last published.
    pub fn det_out(&self) -> f32 {
        self.det_out
    }
    /// `trigSeq`: bumped every time a swell starts.
    pub fn trig_seq(&self) -> i32 {
        self.trig_seq
    }
    /// `bpmOut`: the tempo actually in use, as last published.
    pub fn bpm_out(&self) -> f32 {
        self.bpm_out
    }

    /// The Rhythmic clock (tests).
    pub fn phase(&self) -> f64 {
        self.phase
    }
    /// The stage lengths in seconds as last derived: attack, hold, release, cycle (tests).
    pub fn stage_secs(&self) -> (f64, f64, f64, f64) {
        (self.atk_sec, self.hold_sec, self.rel_sec, self.cycle_sec)
    }
    /// The glides (trim, level, mix, floor) and their targets (tests).
    pub fn glides(&self) -> ([f32; 4], [f32; 4]) {
        ([self.trim_sm, self.level_sm, self.mix_sm, self.floor_sm], [self.trim_lin, self.level_lin, self.mix_knob, self.floor_lin])
    }

    // ------------------------------------------------------------------ helpers

    fn time_coef(&self, seconds: f32) -> f32 {
        1.0 - (-1.0f32 / (max_f(seconds, 0.00005) * self.fs as f32)).exp()
    }

    /// Keep the Rhythmic cycle on the host's grid while it plays (`lockToHost`).
    fn lock_to_host(&mut self, ppq: f64) {
        if !self.host_playing || self.tempo_src_idx != 0 || self.host_bpm < 20.0 || ppq < 0.0 {
            return;
        }
        let beats = f64::from(note_beats(self.rate_idx));
        if beats <= 0.0 {
            return;
        }
        let target = ppq / beats - (ppq / beats).floor();
        let mut diff = target - self.phase;
        diff -= (diff + 0.5).floor();
        if diff.abs() > 0.01 {
            self.phase = target;
        }
    }

    /// Stage lengths from the knobs / note values (control rate, `updateTimes`).
    fn update_times(&mut self) {
        self.bpm_now = if self.tempo_src_idx == 0 && self.host_bpm >= 20.0 { self.host_bpm } else { self.manual_bpm };
        let beat_sec = 60.0 / f64::from(self.bpm_now);

        self.atk_sec = if self.atk_synced { f64::from(note_beats(self.atk_note_idx)) * beat_sec } else { knob_to_stage_sec(self.attack_knob) };
        self.rel_sec = if self.rel_synced { f64::from(note_beats(self.rel_note_idx)) * beat_sec } else { knob_to_stage_sec(self.release_knob) };
        self.hold_sec = if self.hold_synced { f64::from(note_beats(self.hold_note_idx)) * beat_sec } else { knob_to_hold_sec(self.hold_knob) };
        self.cycle_sec = f64::from(note_beats(self.rate_idx)) * beat_sec;

        self.atk_step = 1.0 / (self.atk_sec.max(0.001) * self.fs);
        self.rel_step = 1.0 / (self.rel_sec.max(0.001) * self.fs);
        self.hold_samples = self.hold_sec * self.fs;

        // Bloom: lowpass opens with the VCA, 300 Hz (at full Bloom) up to just under Nyquist
        if self.bloom_knob > 0.001 {
            let f_top = 0.49 * self.fs;
            let f_min = f_top * (300.0 / f_top).powf(f64::from(self.bloom_knob));
            let fc = f_min * (f_top / f_min).powf(f64::from(self.gain_sm));
            let g = (PI * fc.min(f_top) / self.fs).tan();
            self.bloom_g = (g / (1.0 + g)) as f32;
        }
    }

    /// Rhythmic: the whole shape as a function of where we are in the cycle (`rhythmicLin`).
    fn rhythmic_lin(&mut self) -> f64 {
        let (mut a, mut h, mut r) = (self.atk_sec, self.hold_sec, self.rel_sec);
        let total = a + h + r;
        if total > self.cycle_sec {
            // too long for the Rate: keep the shape, fit the cycle
            let k = self.cycle_sec / total;
            a *= k;
            h *= k;
            r *= k;
        }

        let mut pos = self.phase * self.cycle_sec + if self.peak_on_beat { a } else { 0.0 };
        if pos >= self.cycle_sec {
            pos -= self.cycle_sec;
        }
        if pos < self.last_pos - 0.5 * self.cycle_sec {
            // new swell starts
            self.side = 1 - self.side;
            self.bump_trigger();
        }
        self.last_pos = pos;

        if pos < a {
            return pos / a.max(1.0e-6);
        }
        if pos < a + h {
            return 1.0;
        }
        if pos < a + h + r {
            return 1.0 - (pos - a - h) / r.max(1.0e-6);
        }
        0.0
    }

    /// `++trigCount; trigSeq.store (trigCount);`
    fn bump_trigger(&mut self) {
        self.trig_count = self.trig_count.wrapping_add(1);
        self.trig_seq = self.trig_count;
    }

    fn start_swell(&mut self) {
        self.armed = false;
        self.since_trig = 0.0;
        self.below_count = 0.0;
        self.hold_count = 0.0;
        self.bump_trigger();
        if self.lin > 0.02 {
            self.stage = 1; // something is still open: shut it quickly first
        } else {
            self.stage = 2;
            self.lin = 0.0;
            self.side = 1 - self.side;
        }
    }

    /// Pick / One-Shot state machine, one step per sample (`runTriggered`).
    fn run_triggered(&mut self) {
        let level = self.fast_env;
        self.since_trig += 1.0;

        if !self.armed && level < self.thresh * self.rearm_ratio {
            self.armed = true;
        }

        if self.phrase_on && self.mode_idx == 0 {
            // Phrase: only ever open from closed or from a fade. Picks that land while it is open
            // change nothing.
            if (self.stage == 0 || self.stage == 4) && level > self.thresh {
                self.armed = false;
                self.since_trig = 0.0;
                self.below_count = 0.0;
                self.hold_count = 0.0;
                self.bump_trigger();
                if self.lin < 0.02 {
                    self.side = 1 - self.side; // only swap sides while it is inaudible
                }
                self.stage = 2; // rise from wherever it is: no shutting first
            }
        } else {
            let mut fire = self.armed && level > self.thresh;
            if !fire && !self.armed && level > self.thresh && self.since_trig > 0.06 * self.fs && level > self.slow_env * self.onset_ratio {
                fire = true;
            }
            if fire {
                self.start_swell();
            }
        }

        let below = level < self.thresh * self.rearm_ratio;
        if below {
            self.below_count += 1.0;
        } else {
            self.below_count = 0.0;
        }

        if self.stage == 1 {
            self.lin -= self.close_step;
            if self.lin <= 0.0 {
                self.lin = 0.0;
                self.stage = 2;
                self.side = 1 - self.side;
            }
        } else if self.stage == 2 {
            self.lin += self.atk_step;
            if self.lin >= 1.0 {
                self.lin = 1.0;
                self.stage = 3;
                self.hold_count = 0.0;
            } else if self.mode_idx == 0 && below && self.below_count > self.hold_samples + 0.03 * self.fs {
                self.stage = 4; // note died before the swell finished
            }
        } else if self.stage == 3 {
            self.hold_count += 1.0;
            if self.mode_idx == 0 {
                // Pick: open while it sustains, Hold = hang time
                if below && self.below_count > self.hold_samples {
                    self.stage = 4;
                }
            } else if self.hold_count >= self.hold_samples {
                // One-Shot: fixed
                self.stage = 4;
            }
        } else if self.stage == 4 {
            self.lin -= self.rel_step;
            if self.lin <= 0.0 {
                self.lin = 0.0;
                self.stage = 0;
            }
        }
    }
}

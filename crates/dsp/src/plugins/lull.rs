//! Lull: a gate and expander for getting one source out of a microphone that hears several.
//!
//! Two ways of working, one set of controls:
//!
//! - **Expand**: a downward expander. Below the threshold the sound is turned down by Ratio, by
//!   at most Range, through a soft Knee: the gentle way to push bleed back between phrases
//!   without the source ever sounding switched.
//! - **Gate**: below the threshold the sound drops by Range, as fast as Attack, Hold and Release
//!   say: the firm way, for drums and anything that should be simply on or off.
//!
//! Either way the level is read from a detector with its own high-pass and low-pass (so a kick's
//! rumble or a hi-hat's hiss cannot open a vocal microphone), with Hysteresis so a level that
//! hovers at the threshold does not chatter, and optional Lookahead so the first transient is not
//! clipped.
//!
//! A sidechain key can be used two ways:
//!
//! - **Guard**: the key is the thing you do NOT want (the guitar amp next to the singer). While it
//!   is loud, the threshold is raised by up to Guard dB, so its bleed cannot open the microphone.
//!   Guard Delay lines the key up with the bleed's later arrival through the air.
//! - **Trigger**: the classic external key: the key's level opens and closes the gate.
//!
//! Latency is the Lookahead, and nothing when it is zero. Every buffer is allocated in `prepare`;
//! `process` and `set_param` never allocate.
//!
//! For its editor Lull publishes live values ([`Analysis::live`]): [`LIVE_LEVEL`], [`LIVE_GR`],
//! [`LIVE_OUT`], [`LIVE_OPEN`] and [`LIVE_THRESHOLD`].

pub mod presets;

use crate::analysis::Analysis;
use crate::biquad::{Biquad, FilterType};
use crate::params::{Params, choice, lin, log, toggle};
use crate::util::{frames_in, sane_channels, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain, gain_to_db};
use std::sync::Arc;

/// The longest Lookahead and Guard Delay, in ms (the delay lines are sized for them).
pub const MAX_LOOKAHEAD_MS: f32 = 10.0;
pub const MAX_GUARD_DELAY_MS: f32 = 50.0;
/// The detector's level falls back this fast between peaks, in ms.
const DETECTOR_FALL_MS: f32 = 5.0;
/// A key this many dB over the threshold raises it by the whole Guard amount.
const GUARD_SPAN_DB: f32 = 12.0;
/// Levels below this read as silence.
const FLOOR_DB: f32 = -120.0;
/// How quickly Mix and Trim glide to a new value, in ms.
const GLIDE_MS: f32 = 20.0;
/// The sidechain filters are out of the way at these settings.
const HP_OFF_HZ: f32 = 20.5;
const LP_OFF_HZ: f32 = 19_900.0;

/// [`Analysis::live`]: the detector's peak level over the last block in dBFS, the deepest gain
/// reduction in dB, the output's peak in dBFS, whether the gate is open (1 or 0), and the
/// threshold in force in dBFS (the Threshold, raised by the Guard while the key is loud).
pub const LIVE_LEVEL: usize = 0;
pub const LIVE_GR: usize = 1;
pub const LIVE_OUT: usize = 2;
pub const LIVE_OPEN: usize = 3;
pub const LIVE_THRESHOLD: usize = 4;
pub const LIVE_LEN: usize = 5;

const P_MODE: usize = 0;
const P_THRESHOLD: usize = 1;
const P_RANGE: usize = 2;
const P_RATIO: usize = 3;
const P_KNEE: usize = 4;
const P_HYSTERESIS: usize = 5;
const P_ATTACK: usize = 6;
const P_HOLD: usize = 7;
const P_RELEASE: usize = 8;
const P_LOOKAHEAD: usize = 9;
const P_SC_HP: usize = 10;
const P_SC_LP: usize = 11;
const P_LISTEN: usize = 12;
const P_KEY: usize = 13;
const P_GUARD: usize = 14;
const P_GUARD_DELAY: usize = 15;
const P_LINK: usize = 16;
const P_MIX: usize = 17;
const P_TRIM: usize = 18;

/// The Mode and Key choices.
pub const MODE_NAMES: [&str; 2] = ["Expand", "Gate"];
pub const KEY_NAMES: [&str; 2] = ["Guard", "Trigger"];

static LULL_PARAMS: [ParamInfo; 19] = [
    choice("mode", "Mode", &MODE_NAMES, 0),
    lin("threshold", "Threshold", -80.0, 0.0, -40.0, Unit::Db),
    lin("range", "Range", 0.0, 80.0, 20.0, Unit::Db),
    log("ratio", "Ratio", 1.1, 10.0, 2.0, Unit::Ratio),
    lin("knee", "Knee", 0.0, 24.0, 6.0, Unit::Db),
    lin("hysteresis", "Hysteresis", 0.0, 12.0, 3.0, Unit::Db),
    log("attack", "Attack", 0.05, 100.0, 1.0, Unit::Ms),
    lin("hold", "Hold", 0.0, 500.0, 20.0, Unit::Ms),
    log("release", "Release", 5.0, 2000.0, 120.0, Unit::Ms),
    lin("lookahead", "Lookahead", 0.0, MAX_LOOKAHEAD_MS, 0.0, Unit::Ms),
    log("sc_hp", "Detector High-Pass", 20.0, 2000.0, 20.0, Unit::Hz),
    log("sc_lp", "Detector Low-Pass", 200.0, 20000.0, 20000.0, Unit::Hz),
    toggle("listen", "Listen", false),
    choice("key", "Key", &KEY_NAMES, 0),
    lin("guard", "Guard", 0.0, 24.0, 9.0, Unit::Db),
    lin("guard_delay", "Guard Delay", 0.0, MAX_GUARD_DELAY_MS, 0.0, Unit::Ms),
    lin("link", "Link", 0.0, 100.0, 100.0, Unit::Percent),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
    lin("trim", "Trim", -12.0, 12.0, 0.0, Unit::Db),
];

pub static LULL_INFO: PluginInfo = PluginInfo {
    id: "lull",
    name: "Lull",
    short_name: "Lull",
    category: Category::Dynamics,
    params: &LULL_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: true,
    link: None,
    references: 0,
    midi_out: false,
};

/// The static curve: how much a level is turned down once the gain has settled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    pub gate: bool,
    pub threshold: f32,
    pub range: f32,
    pub ratio: f32,
    pub knee: f32,
}

impl Curve {
    /// The gain reduction in dB (0..=range) for a detector level of `level_db`, with the
    /// threshold at `threshold_db` (the curve's own, or one moved by hysteresis or the guard).
    pub fn reduction_at(&self, level_db: f32, threshold_db: f32) -> f32 {
        // How far below the threshold (negative: above it).
        let under = threshold_db - level_db;
        let knee = self.knee.max(0.0);
        let gr = if self.gate {
            // The whole range, eased in across the knee.
            let t = if knee > 1.0e-3 {
                ((under + knee * 0.5) / knee).clamp(0.0, 1.0)
            } else if under > 0.0 {
                1.0
            } else {
                0.0
            };
            self.range * t * t * (3.0 - 2.0 * t)
        } else {
            let eased = if knee > 1.0e-3 && under.abs() * 2.0 <= knee {
                let t = under + knee * 0.5;
                t * t / (2.0 * knee)
            } else {
                under.max(0.0)
            };
            eased * (self.ratio.max(1.0) - 1.0)
        };
        if gr.is_finite() { gr.clamp(0.0, self.range.max(0.0)) } else { 0.0 }
    }

    /// [`Curve::reduction_at`] at the curve's own threshold.
    pub fn reduction(&self, level_db: f32) -> f32 {
        self.reduction_at(level_db, self.threshold)
    }
}

/// How much a key at `key_db` raises a threshold of `threshold_db`, for a Guard of `guard_db`:
/// nothing while the key is under the threshold, all of it once it is [`GUARD_SPAN_DB`] over.
pub fn guard_raise(guard_db: f32, key_db: f32, threshold_db: f32) -> f32 {
    let t = ((key_db - threshold_db) / GUARD_SPAN_DB).clamp(0.0, 1.0);
    guard_db.max(0.0) * t * t * (3.0 - 2.0 * t)
}

/// A ring that hands back what was written `delay` samples ago.
#[derive(Default)]
struct Ring {
    buf: Vec<f32>,
    pos: usize,
}

impl Ring {
    fn with_len(len: usize) -> Self {
        Ring { buf: vec![0.0; len.max(1)], pos: 0 }
    }

    /// Writes `x` and returns the sample written `delay` writes ago (`x` itself for 0).
    #[inline]
    fn push(&mut self, x: f32, delay: usize) -> f32 {
        let len = self.buf.len();
        if let Some(slot) = self.buf.get_mut(self.pos) {
            *slot = x;
        }
        let delay = delay.min(len.saturating_sub(1));
        let at = (self.pos + len - delay) % len;
        self.pos = (self.pos + 1) % len;
        self.buf.get(at).copied().unwrap_or(x)
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

/// One channel's detector and gain.
struct Channel {
    hp: Biquad,
    lp: Biquad,
    key_hp: Biquad,
    key_lp: Biquad,
    /// The audio, delayed by the Lookahead; the key's level, delayed by the Guard Delay.
    audio: Ring,
    key_level: Ring,
    /// The detector's signal this sample (what Listen plays).
    heard: f32,
    /// The detector's and the key's level (linear peaks that fall back between peaks).
    level: f32,
    key: f32,
    /// The gain reduction in force, in dB (>= 0), and the samples of Hold left.
    gr: f32,
    hold_left: usize,
    open: bool,
}

impl Channel {
    fn new(sr: f32) -> Self {
        Channel {
            hp: Biquad::default(),
            lp: Biquad::default(),
            key_hp: Biquad::default(),
            key_lp: Biquad::default(),
            audio: Ring::with_len((MAX_LOOKAHEAD_MS * 0.001 * sr).ceil() as usize + 2),
            key_level: Ring::with_len((MAX_GUARD_DELAY_MS * 0.001 * sr).ceil() as usize + 2),
            heard: 0.0,
            level: 0.0,
            key: 0.0,
            gr: 0.0,
            hold_left: 0,
            open: false,
        }
    }

    fn reset(&mut self, range: f32) {
        for f in [&mut self.hp, &mut self.lp, &mut self.key_hp, &mut self.key_lp] {
            f.reset();
        }
        self.audio.clear();
        self.key_level.clear();
        self.heard = 0.0;
        self.level = 0.0;
        self.key = 0.0;
        // Closed until something opens it: no burst of bleed at the start.
        self.gr = range;
        self.hold_left = 0;
        self.open = false;
    }
}

/// What the parameters come to.
#[derive(Debug, Clone, Copy)]
struct Settings {
    curve: Curve,
    hysteresis: f32,
    /// One-pole coefficients per sample.
    attack: f32,
    release: f32,
    fall: f32,
    hold: usize,
    lookahead: usize,
    hp_on: bool,
    lp_on: bool,
    listen: bool,
    trigger: bool,
    guard: f32,
    guard_delay: usize,
    link: f32,
    mix: f32,
    trim: f32,
}

/// The gate and expander.
///
/// `gain_reduction_db` is the deepest reduction during the last block.
pub struct Lull {
    p: Params,
    sr: f32,
    channels: Vec<Channel>,
    /// Each channel's own detector level this sample (so the link can read them all).
    levels: Vec<f32>,
    s: Settings,
    mix_now: f32,
    trim_now: f32,
    glide: f32,
    gr_meter: f32,
    analysis: Arc<Analysis>,
}

impl Default for Lull {
    fn default() -> Self {
        Self::new()
    }
}

impl Lull {
    pub fn new() -> Self {
        let mut l = Lull {
            p: Params::new(&LULL_PARAMS),
            sr: 48_000.0,
            channels: Vec::new(),
            levels: Vec::new(),
            s: Settings {
                curve: Curve { gate: false, threshold: -40.0, range: 20.0, ratio: 2.0, knee: 6.0 },
                hysteresis: 0.0,
                attack: 0.0,
                release: 0.0,
                fall: 0.0,
                hold: 0,
                lookahead: 0,
                hp_on: false,
                lp_on: false,
                listen: false,
                trigger: false,
                guard: 0.0,
                guard_delay: 0,
                link: 1.0,
                mix: 1.0,
                trim: 1.0,
            },
            mix_now: 1.0,
            trim_now: 1.0,
            glide: 0.0,
            gr_meter: 0.0,
            analysis: Arc::new(Analysis::with_live(LIVE_LEN)),
        };
        l.update();
        l.mix_now = l.s.mix;
        l.trim_now = l.s.trim;
        l
    }

    /// The static curve as the parameters stand.
    pub fn curve(&self) -> Curve {
        self.s.curve
    }

    /// Works the parameters out into [`Settings`] and the detector filters. No allocation.
    fn update(&mut self) {
        let sr = self.sr;
        let samples = |ms: f32| (ms * 0.001 * sr).round().max(0.0) as usize;
        let (hp, lp) = (self.p.v(P_SC_HP), self.p.v(P_SC_LP));
        let lookahead = samples(self.p.v(P_LOOKAHEAD));
        self.s = Settings {
            curve: Curve {
                gate: self.p.choice(P_MODE) == 1,
                threshold: self.p.v(P_THRESHOLD),
                range: self.p.v(P_RANGE),
                ratio: self.p.v(P_RATIO),
                knee: self.p.v(P_KNEE),
            },
            hysteresis: self.p.v(P_HYSTERESIS),
            attack: time_coef(self.p.v(P_ATTACK), sr),
            release: time_coef(self.p.v(P_RELEASE), sr),
            fall: time_coef(DETECTOR_FALL_MS, sr),
            // With lookahead the detector runs ahead of the audio: the hold covers the gap, so
            // the tail of a note is not closed on early.
            hold: samples(self.p.v(P_HOLD)) + lookahead,
            lookahead,
            hp_on: hp > HP_OFF_HZ,
            lp_on: lp < LP_OFF_HZ,
            listen: self.p.on(P_LISTEN),
            trigger: self.p.choice(P_KEY) == 1,
            guard: self.p.v(P_GUARD),
            guard_delay: samples(self.p.v(P_GUARD_DELAY)),
            link: self.p.v(P_LINK) / 100.0,
            mix: self.p.v(P_MIX) / 100.0,
            trim: db_to_gain(self.p.v(P_TRIM)),
        };
        let q = std::f32::consts::FRAC_1_SQRT_2;
        for c in &mut self.channels {
            for f in [&mut c.hp, &mut c.key_hp] {
                f.set(FilterType::HighPass, hp, q, 0.0, sr);
            }
            for f in [&mut c.lp, &mut c.key_lp] {
                f.set(FilterType::LowPass, lp.min(sr * 0.45), q, 0.0, sr);
            }
        }
    }

    fn run(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        let s = self.s;
        let ch = self.channels.len().min(io.len());
        if ch == 0 {
            return;
        }
        let frames = frames_in(io, frames, ch);
        // A key that does not cover the block is no key.
        let key = key.filter(|k| !k.is_empty() && k.iter().take(ch).all(|b| b.len() >= frames));
        let (mut peak_level, mut peak_out, mut deepest, mut raise_max) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut any_open = false;
        for i in 0..frames {
            self.mix_now = s.mix + (self.mix_now - s.mix) * self.glide;
            self.trim_now = s.trim + (self.trim_now - s.trim) * self.glide;
            // 1. Each channel's detector and key levels.
            let mut loudest = 0.0f32;
            for (c, (state, buf)) in self.channels.iter_mut().zip(io.iter()).take(ch).enumerate() {
                let x = buf.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
                let k = key.and_then(|k| k.get(c)).and_then(|b| b.get(i)).copied().filter(|v| v.is_finite());
                let filter = |hp: &mut Biquad, lp: &mut Biquad, v: f32| {
                    let v = if s.hp_on { hp.process_sample(v) } else { v };
                    if s.lp_on { lp.process_sample(v) } else { v }
                };
                let own = filter(&mut state.hp, &mut state.lp, x);
                let keyed = k.map(|k| filter(&mut state.key_hp, &mut state.key_lp, k));
                // What opens the gate: the key in Trigger, else the sound itself.
                let heard = match keyed {
                    Some(k) if s.trigger => k,
                    _ => own,
                };
                state.heard = heard;
                state.level = heard.abs().max(state.level * s.fall);
                if !state.level.is_finite() || state.level < 1.0e-9 {
                    state.level = 0.0;
                }
                // What guards it: the key's level, as it was a Guard Delay ago.
                let guard_in = match keyed {
                    Some(k) if !s.trigger => k.abs(),
                    _ => 0.0,
                };
                state.key = guard_in.max(state.key * s.fall);
                if !state.key.is_finite() || state.key < 1.0e-9 {
                    state.key = 0.0;
                }
                if let Some(slot) = self.levels.get_mut(c) {
                    *slot = state.level;
                }
                loudest = loudest.max(state.level);
            }
            // 2. Each channel's gain.
            for (c, (state, buf)) in self.channels.iter_mut().zip(io.iter_mut()).take(ch).enumerate() {
                let Some(x) = buf.get_mut(i) else { continue };
                let input = if x.is_finite() { *x } else { 0.0 };
                let own = self.levels.get(c).copied().unwrap_or(0.0);
                let level = own + (loudest - own) * s.link;
                let level_db = gain_to_db(level).max(FLOOR_DB);
                let key_then = state.key_level.push(state.key, s.guard_delay);
                let raise = guard_raise(s.guard, gain_to_db(key_then).max(FLOOR_DB), s.curve.threshold);
                let threshold = s.curve.threshold + raise;
                // Hysteresis: once open, it takes a lower level to close.
                if state.open {
                    state.open = level_db > threshold - s.hysteresis;
                } else {
                    state.open = level_db > threshold;
                }
                let want = s.curve.reduction_at(level_db, if state.open { threshold - s.hysteresis } else { threshold });
                if want < state.gr {
                    // Opening: Attack, and the Hold starts over.
                    state.gr = want + (state.gr - want) * s.attack;
                    state.hold_left = s.hold;
                } else if state.hold_left > 0 {
                    state.hold_left -= 1;
                } else {
                    state.gr = want + (state.gr - want) * s.release;
                }
                if !state.gr.is_finite() {
                    state.gr = 0.0;
                }
                let dry = state.audio.push(input, s.lookahead);
                let wet = dry * db_to_gain(-state.gr);
                let out = if s.listen {
                    // The detector's own signal (the key in Trigger).
                    state.heard
                } else {
                    (dry + (wet - dry) * self.mix_now) * self.trim_now
                };
                *x = out;
                peak_level = peak_level.max(level);
                peak_out = peak_out.max(out.abs());
                deepest = deepest.max(state.gr);
                raise_max = raise_max.max(raise);
                any_open |= state.open;
            }
        }
        if frames > 0 {
            self.gr_meter = deepest;
            let live = &self.analysis.live;
            live.set(LIVE_LEVEL, gain_to_db(peak_level).max(FLOOR_DB));
            live.set(LIVE_GR, deepest);
            live.set(LIVE_OUT, gain_to_db(peak_out).max(FLOOR_DB));
            live.set(LIVE_OPEN, if any_open { 1.0 } else { 0.0 });
            live.set(LIVE_THRESHOLD, s.curve.threshold + raise_max);
        }
    }
}

impl Plugin for Lull {
    crate::params::param_plumbing!(LULL_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        let ch = sane_channels(channels);
        self.channels = (0..ch).map(|_| Channel::new(self.sr)).collect();
        self.levels = vec![0.0; ch];
        self.glide = time_coef(GLIDE_MS, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        let range = self.s.curve.range;
        for c in &mut self.channels {
            c.reset(range);
        }
        self.mix_now = self.s.mix;
        self.trim_now = self.s.trim;
        self.gr_meter = 0.0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.run(io, frames, None);
    }

    fn process_with_sidechain(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        self.run(io, frames, key);
    }

    fn latency(&self) -> usize {
        if self.channels.is_empty() { 0 } else { self.s.lookahead }
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr_meter
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let live = &self.analysis.live;
        out[0] = live.get(LIVE_LEVEL);
        out[1] = live.get(LIVE_OUT);
        out[2] = live.get(LIVE_GR);
        out[3] = live.get(LIVE_OPEN);
        out[4] = live.get(LIVE_THRESHOLD);
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(hz: f32, amp: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / SR).sin()).collect()
    }

    fn prepared(set: &[(&str, f32)]) -> Lull {
        let mut l = Lull::new();
        l.prepare(SR, 512, 2);
        for (id, v) in set {
            assert!(l.set_param(id, *v), "{id}");
        }
        l.reset();
        l
    }

    fn run_keyed(l: &mut Lull, x: &[f32], key: Option<&[f32]>) -> Vec<f32> {
        let mut out = Vec::with_capacity(x.len());
        for (n, block) in x.chunks(512).enumerate() {
            let mut io = vec![block.to_vec(), block.to_vec()];
            match key {
                Some(k) => {
                    let kb = k[n * 512..n * 512 + block.len()].to_vec();
                    l.process_with_sidechain(&mut io, block.len(), Some(&[kb.clone(), kb]));
                }
                None => l.process(&mut io, block.len()),
            }
            out.extend_from_slice(&io[0]);
        }
        out
    }

    fn run(l: &mut Lull, x: &[f32]) -> Vec<f32> {
        run_keyed(l, x, None)
    }

    fn peak_db(x: &[f32]) -> f32 {
        gain_to_db(x.iter().fold(0.0f32, |m, v| m.max(v.abs())))
    }

    #[test]
    fn the_curves_are_as_described() {
        let expand = Curve { gate: false, threshold: -40.0, range: 20.0, ratio: 2.0, knee: 0.0 };
        assert_eq!(expand.reduction(-30.0), 0.0);
        assert_eq!(expand.reduction(-40.0), 0.0);
        assert!((expand.reduction(-50.0) - 10.0).abs() < 1e-4, "2:1 turns 10 dB under into 10 dB of reduction");
        assert_eq!(expand.reduction(-90.0), 20.0, "never more than the range");
        let soft = Curve { knee: 12.0, ..expand };
        assert!(soft.reduction(-40.0) > 0.0 && soft.reduction(-40.0) < 3.0, "the knee starts early and gently");
        assert!((soft.reduction(-50.0) - 10.0).abs() < 1e-4, "and meets the hard curve past it");
        let gate = Curve { gate: true, ..expand };
        assert_eq!(gate.reduction(-39.9), 0.0);
        assert_eq!(gate.reduction(-40.1), 20.0);
        let soft_gate = Curve { gate: true, knee: 6.0, ..expand };
        assert!((soft_gate.reduction(-40.0) - 10.0).abs() < 1e-3, "half way at the threshold");
        // The curve never rises as the level does.
        for c in [expand, soft, gate, soft_gate] {
            let mut last = f32::MAX;
            for i in 0..=160 {
                let gr = c.reduction(-100.0 + i as f32 * 0.5);
                assert!(gr <= last + 1e-4 && (0.0..=20.0).contains(&gr));
                last = gr;
            }
        }
    }

    #[test]
    fn loud_passes_and_quiet_is_turned_down() {
        // Expand, 2:1: a tone 10 dB under the threshold comes out 10 dB lower still.
        let mut l = prepared(&[("threshold", -30.0), ("knee", 0.0), ("hysteresis", 0.0), ("range", 40.0)]);
        let loud = sine(1000.0, db_to_gain(-12.0), 24_000);
        let y = run(&mut l, &loud);
        assert!((peak_db(&y[12_000..]) + 12.0).abs() < 0.2, "above the threshold: {}", peak_db(&y[12_000..]));
        let quiet = sine(1000.0, db_to_gain(-40.0), 48_000);
        let y = run(&mut l, &quiet);
        assert!((peak_db(&y[40_000..]) + 50.0).abs() < 1.0, "10 dB under at 2:1: {}", peak_db(&y[40_000..]));
        assert!((l.gain_reduction_db() - 10.0).abs() < 1.0);
        // Gate: the same quiet tone drops by the whole range.
        let mut g = prepared(&[("mode", 1.0), ("threshold", -30.0), ("range", 40.0), ("knee", 0.0)]);
        let y = run(&mut g, &quiet);
        assert!(peak_db(&y[40_000..]) < -78.0, "gated: {}", peak_db(&y[40_000..]));
    }

    #[test]
    fn it_starts_closed_and_reset_closes_it_again() {
        let mut g = prepared(&[("mode", 1.0), ("threshold", -30.0), ("range", 60.0), ("release", 500.0)]);
        let quiet = sine(500.0, db_to_gain(-50.0), 4800);
        assert!(peak_db(&run(&mut g, &quiet)) < -100.0, "no burst of bleed at the start");
        let _ = run(&mut g, &sine(500.0, 0.5, 4800));
        g.reset();
        assert!(peak_db(&run(&mut g, &quiet)) < -100.0);
    }

    #[test]
    fn attack_hold_and_release_shape_the_gain() {
        // A burst, then silence-level tone: the gate holds, then closes over the release.
        let burst = |hold: f32, release: f32| {
            let mut g = prepared(&[
                ("mode", 1.0),
                ("threshold", -30.0),
                ("range", 60.0),
                ("knee", 0.0),
                ("hysteresis", 0.0),
                ("attack", 0.05),
                ("hold", hold),
                ("release", release),
            ]);
            let mut x = sine(1000.0, 0.5, 9600);
            x.extend(sine(1000.0, db_to_gain(-50.0), 38_400));
            let y = run(&mut g, &x);
            // The gain on the quiet tail, in dB, 50 ms and 400 ms after the burst ends.
            let at = |ms: usize| peak_db(&y[9600 + ms * 48..9600 + ms * 48 + 96]) + 50.0;
            (at(50), at(400))
        };
        let (held, later) = burst(200.0, 20.0);
        assert!(held > -3.0, "still held open 50 ms on: {held} dB");
        assert!(later < -50.0, "closed 400 ms on: {later} dB");
        let (unheld, _) = burst(0.0, 20.0);
        assert!(unheld < -40.0, "no hold, fast release: {unheld} dB at 50 ms");
        let (slow, _) = burst(0.0, 1000.0);
        assert!(slow > -6.0 && slow < -0.5, "a slow release is only part closed at 50 ms: {slow} dB");
    }

    #[test]
    fn hysteresis_keeps_a_hovering_level_from_chattering() {
        // A tone wobbling 2 dB either side of the threshold.
        let x: Vec<f32> = (0..96_000)
            .map(|i| {
                let wobble = 2.0 * (std::f32::consts::TAU * 6.0 * i as f32 / SR).sin();
                db_to_gain(-30.0 + wobble) * (std::f32::consts::TAU * 1000.0 * i as f32 / SR).sin()
            })
            .collect();
        let swings = |hysteresis: f32| {
            let mut g = prepared(&[
                ("mode", 1.0),
                ("threshold", -30.0),
                ("range", 40.0),
                ("knee", 0.0),
                ("hysteresis", hysteresis),
                ("hold", 0.0),
                ("release", 5.0),
            ]);
            let y = run(&mut g, &x);
            // Count how often the output crosses from closed to open after the first second.
            let mut count = 0;
            let mut was_open = true;
            for block in y[48_000..].chunks(96) {
                let open = peak_db(block) > -50.0;
                if open && !was_open {
                    count += 1;
                }
                was_open = open;
            }
            count
        };
        assert!(swings(0.0) >= 4, "without hysteresis it chatters: {}", swings(0.0));
        assert_eq!(swings(6.0), 0, "with 6 dB of hysteresis it stays open");
    }

    #[test]
    fn lookahead_opens_before_the_transient_and_is_the_latency() {
        let mut x = vec![0.0f32; 4800];
        x.extend(sine(1000.0, 0.5, 4800));
        let first_cycle_peak = |lookahead: f32| {
            let mut g = prepared(&[("mode", 1.0), ("threshold", -30.0), ("range", 60.0), ("attack", 0.5), ("lookahead", lookahead)]);
            let lat = g.latency();
            assert_eq!(lat, (lookahead * 48.0).round() as usize);
            let y = run(&mut g, &x);
            // The first half-millisecond of the tone, where it lands in the output.
            y[4800 + lat..4800 + lat + 24].iter().fold(0.0f32, |m, v| m.max(v.abs())) / x[4800..4824].iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        assert!(first_cycle_peak(0.0) < 0.5, "without lookahead the front is clipped: {}", first_cycle_peak(0.0));
        assert!(first_cycle_peak(5.0) > 0.9, "with it the front is whole: {}", first_cycle_peak(5.0));
    }

    #[test]
    fn the_detector_filters_decide_what_opens_it() {
        // A loud 60 Hz rumble and nothing else: open with the filter out, closed with it in.
        let rumble = sine(60.0, 0.5, 48_000);
        let out = |hp: f32| {
            let mut g = prepared(&[("mode", 1.0), ("threshold", -30.0), ("range", 60.0), ("sc_hp", hp)]);
            peak_db(&run(&mut g, &rumble)[24_000..])
        };
        assert!(out(20.0) > -7.0, "unfiltered, the rumble opens it: {}", out(20.0));
        assert!(out(1000.0) < -50.0, "high-passed, it does not: {}", out(1000.0));
        // Listen plays the detector's signal: with the high-pass in, the rumble is nearly gone.
        let mut g = prepared(&[("sc_hp", 1000.0), ("listen", 1.0)]);
        assert!(peak_db(&run(&mut g, &rumble)[24_000..]) < -40.0);
    }

    #[test]
    fn a_guarding_key_raises_the_threshold_and_a_trigger_key_opens_it() {
        // The microphone hears bleed at -34 dB: over the -40 dB threshold, so it opens.
        let bleed = sine(700.0, db_to_gain(-34.0), 48_000);
        let key = sine(700.0, db_to_gain(-10.0), 48_000);
        let set = [("mode", 1.0), ("threshold", -40.0), ("range", 50.0), ("knee", 0.0), ("hysteresis", 0.0), ("guard", 12.0)];
        let mut g = prepared(&set);
        assert!(peak_db(&run(&mut g, &bleed)[24_000..]) > -36.0, "no key: the bleed opens it");
        let mut g = prepared(&set);
        let y = run_keyed(&mut g, &bleed, Some(&key));
        assert!(peak_db(&y[24_000..]) < -70.0, "guarded: the bleed stays out: {}", peak_db(&y[24_000..]));
        assert!((g.analysis().unwrap().live.get(LIVE_THRESHOLD) + 28.0).abs() < 0.5, "the threshold in force is 12 dB up");
        // The singer, well over even the raised threshold, still gets through.
        let voice = sine(300.0, db_to_gain(-12.0), 48_000);
        let mut g = prepared(&set);
        assert!(peak_db(&run_keyed(&mut g, &voice, Some(&key))[24_000..]) > -13.0);
        // Trigger: the key alone opens a gate that the quiet input would not.
        let quiet = sine(300.0, db_to_gain(-60.0), 48_000);
        let mut g = prepared(&[("mode", 1.0), ("threshold", -40.0), ("range", 50.0), ("key", 1.0)]);
        assert!(peak_db(&run_keyed(&mut g, &quiet, Some(&key))[24_000..]) > -61.0, "triggered open");
        let mut g = prepared(&[("mode", 1.0), ("threshold", -40.0), ("range", 50.0), ("key", 1.0)]);
        assert!(peak_db(&run(&mut g, &quiet)[24_000..]) < -100.0, "no key, closed");
        assert_eq!(guard_raise(12.0, -50.0, -40.0), 0.0);
        assert_eq!(guard_raise(12.0, -20.0, -40.0), 12.0);
    }

    #[test]
    fn the_link_makes_both_sides_follow_the_louder() {
        let loud = sine(1000.0, 0.5, 24_000);
        let quiet = sine(1000.0, db_to_gain(-60.0), 24_000);
        let right_out = |link: f32| {
            let mut g = prepared(&[("mode", 1.0), ("threshold", -30.0), ("range", 40.0), ("link", link)]);
            let mut out = Vec::new();
            for (a, b) in loud.chunks(512).zip(quiet.chunks(512)) {
                let mut io = vec![a.to_vec(), b.to_vec()];
                g.process(&mut io, a.len());
                out.extend_from_slice(&io[1]);
            }
            peak_db(&out[12_000..])
        };
        assert!(right_out(100.0) > -61.0, "linked: the quiet side stays open with the loud one");
        assert!(right_out(0.0) < -95.0, "unlinked: it closes on its own");
    }

    #[test]
    fn mix_trim_and_the_live_values() {
        let quiet = sine(1000.0, db_to_gain(-50.0), 48_000);
        let mut g = prepared(&[("mode", 1.0), ("threshold", -30.0), ("range", 80.0), ("mix", 50.0)]);
        let y = run(&mut g, &quiet);
        assert!((peak_db(&y[40_000..]) + 56.0).abs() < 0.5, "half of the dry signal is left: {}", peak_db(&y[40_000..]));
        let mut g = prepared(&[("threshold", -80.0), ("trim", 6.0)]);
        let y = run(&mut g, &sine(1000.0, 0.25, 24_000));
        assert!((peak_db(&y[12_000..]) + 6.0).abs() < 0.2, "+6 dB of trim: {}", peak_db(&y[12_000..]));
        let live = &g.analysis().unwrap().live;
        assert!((live.get(LIVE_LEVEL) + 12.0).abs() < 0.5 && live.get(LIVE_OPEN) == 1.0 && live.get(LIVE_GR) < 0.1);
    }

    #[test]
    fn unprepared_and_odd_input_are_safe() {
        let mut l = Lull::new();
        let mut io = vec![vec![0.5f32; 64]; 2];
        l.process(&mut io, 64);
        assert_eq!(io[0][0], 0.5);
        assert_eq!(l.latency(), 0);
        l.prepare(SR, 64, 2);
        let mut io = vec![vec![f32::NAN; 64], vec![f32::INFINITY; 64]];
        l.process(&mut io, 64);
        assert!(io.iter().flatten().all(|v| v.is_finite()));
        // A key shorter than the block is ignored.
        let mut io = vec![vec![0.1f32; 64]; 2];
        l.process_with_sidechain(&mut io, 64, Some(&[vec![0.0; 8]]));
        assert!(io.iter().flatten().all(|v| v.is_finite()));
    }
}

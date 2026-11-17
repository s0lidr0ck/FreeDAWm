//! Modulators: an LFO or an envelope follower on a track that moves any of its plug-ins'
//! parameters ([`crate::Track::modulators`]).
//!
//! Each modulator puts out a value every block (an LFO -1 to 1, an envelope follower 0 to 1,
//! following the track's signal where it enters the chain); each of its targets moves one
//! parameter of one chain module by `depth` times that value, as a share of the parameter's
//! whole range, around the value the parameter has (its stored value, or its lane's when
//! automation plays). The mix does the work on the audio thread; nothing here allocates there.
//! Values out of range (a hand-edited session) are clamped where they are read
//! ([`Modulator::sane`]).

use crate::ModuleId;
use serde::{Deserialize, Serialize};

/// Most modulators on one track.
pub const MAX_MODULATORS: usize = 8;
/// Most targets one modulator drives.
pub const MAX_MOD_TARGETS: usize = 8;

/// What a modulator follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModulatorKind {
    /// A low-frequency oscillator, free or locked to the tempo.
    Lfo,
    /// The level of the track's signal where it enters the chain.
    Envelope,
}

impl ModulatorKind {
    pub const ALL: [ModulatorKind; 2] = [ModulatorKind::Lfo, ModulatorKind::Envelope];

    pub fn name(self) -> &'static str {
        match self {
            ModulatorKind::Lfo => "LFO",
            ModulatorKind::Envelope => "Envelope",
        }
    }

    /// The command and session spelling (`lfo`, `envelope`).
    pub fn key(self) -> &'static str {
        match self {
            ModulatorKind::Lfo => "lfo",
            ModulatorKind::Envelope => "envelope",
        }
    }

    pub fn from_key(k: &str) -> Option<ModulatorKind> {
        ModulatorKind::ALL.into_iter().find(|x| x.key() == k)
    }
}

/// An LFO's shapes, by [`Modulator::shape`].
pub const LFO_SHAPES: [&str; 5] = ["Sine", "Triangle", "Saw Up", "Square", "Random"];

/// The tempo-locked LFO cycles: (name, quarter notes per cycle), by [`Modulator::sync`] - 1.
pub const LFO_SYNC: [(&str, f64); 9] = [
    ("4 bars", 16.0),
    ("2 bars", 8.0),
    ("1 bar", 4.0),
    ("1/2", 2.0),
    ("1/4", 1.0),
    ("1/8", 0.5),
    ("1/8T", 1.0 / 3.0),
    ("1/16", 0.25),
    ("1/32", 0.125),
];

/// One parameter a modulator moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModTarget {
    /// The chain module.
    pub module: ModuleId,
    /// The plug-in's parameter id.
    pub param: String,
    /// How far, -1 to 1: the share of the parameter's whole range the modulator's full swing
    /// moves it (negative: the other way).
    pub depth: f32,
}

/// One modulator (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Modulator {
    /// Stable within the track (given by the command that adds it).
    pub id: u32,
    pub kind: ModulatorKind,
    /// Off: its targets sit at their own values.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypass: bool,
    /// LFO: an index into [`LFO_SHAPES`].
    #[serde(default)]
    pub shape: u8,
    /// LFO: cycles per second when not locked to the tempo (0.01 to 40).
    #[serde(default = "default_rate")]
    pub rate_hz: f32,
    /// LFO: 0 free; else locked to the tempo, `LFO_SYNC[sync - 1]` per cycle, its phase from the
    /// song position.
    #[serde(default)]
    pub sync: u8,
    /// Envelope: how fast it rises and falls, in milliseconds (0.1 to 2000).
    #[serde(default = "default_attack")]
    pub attack_ms: f32,
    #[serde(default = "default_release")]
    pub release_ms: f32,
    /// What it moves (at most [`MAX_MOD_TARGETS`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<ModTarget>,
}

fn default_rate() -> f32 {
    1.0
}
fn default_attack() -> f32 {
    10.0
}
fn default_release() -> f32 {
    200.0
}

impl Modulator {
    /// A new modulator of `kind` with the defaults (a 1 Hz sine; a 10 ms / 200 ms follower).
    pub fn new(id: u32, kind: ModulatorKind) -> Self {
        Modulator {
            id,
            kind,
            bypass: false,
            shape: 0,
            rate_hz: default_rate(),
            sync: 0,
            attack_ms: default_attack(),
            release_ms: default_release(),
            targets: Vec::new(),
        }
    }

    /// The values clamped into range (NaN to the defaults), the targets cut to
    /// [`MAX_MOD_TARGETS`] and their depths clamped.
    pub fn sane(&mut self) {
        let fix = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        self.shape = self.shape.min((LFO_SHAPES.len() - 1) as u8);
        self.sync = self.sync.min(LFO_SYNC.len() as u8);
        self.rate_hz = fix(self.rate_hz, 0.01, 40.0, default_rate());
        self.attack_ms = fix(self.attack_ms, 0.1, 2000.0, default_attack());
        self.release_ms = fix(self.release_ms, 0.1, 2000.0, default_release());
        self.targets.truncate(MAX_MOD_TARGETS);
        for t in &mut self.targets {
            t.depth = fix(t.depth, -1.0, 1.0, 0.0);
        }
    }

    /// Quarter notes per cycle when locked to the tempo.
    pub fn sync_quarters(&self) -> Option<f64> {
        usize::from(self.sync).checked_sub(1).and_then(|i| LFO_SYNC.get(i)).map(|(_, q)| *q)
    }
}

/// The LFO's value, -1 to 1, at `phase` (cycles; the fraction is used) for shape `shape`
/// ([`LFO_SHAPES`]). Random holds one value per cycle, `seed` being the cycle's.
pub fn lfo_value(shape: u8, phase: f64, seed: u64) -> f32 {
    let p = phase.rem_euclid(1.0);
    let v = match shape {
        0 => (p * std::f64::consts::TAU).sin(),
        1 => 1.0 - 4.0 * (p - 0.5).abs(),
        2 => 2.0 * p - 1.0,
        3 => {
            if p < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        _ => {
            // one value per cycle from the cycle number (a hash, so it repeats on a replay)
            let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
            x ^= x >> 31;
            x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x ^= x >> 29;
            (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    };
    v as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_swing_between_minus_one_and_one() {
        for shape in 0..LFO_SHAPES.len() as u8 {
            let vals: Vec<f32> = (0..1000).map(|i| lfo_value(shape, f64::from(i) / 100.0, (i / 100) as u64)).collect();
            assert!(vals.iter().all(|v| (-1.0..=1.0).contains(v)), "{shape}");
            let (lo, hi) = vals.iter().fold((1.0f32, -1.0f32), |(a, b), v| (a.min(*v), b.max(*v)));
            assert!(lo < -0.5 && hi > 0.5, "{shape}: {lo}..{hi}");
        }
        assert!((lfo_value(0, 0.25, 0) - 1.0).abs() < 1e-6);
        assert_eq!(lfo_value(3, 0.1, 0), 1.0);
        assert_eq!(lfo_value(4, 0.1, 7), lfo_value(4, 0.9, 7), "random holds through a cycle");
    }

    #[test]
    fn hostile_values_are_made_sane() {
        let mut m = Modulator::new(1, ModulatorKind::Lfo);
        m.shape = 200;
        m.sync = 99;
        m.rate_hz = f32::NAN;
        m.attack_ms = -5.0;
        m.release_ms = f32::INFINITY;
        m.targets = (0..20).map(|i| ModTarget { module: ModuleId(i), param: "x".into(), depth: 7.0 }).collect();
        m.sane();
        assert_eq!((m.shape, m.sync), (4, 9));
        assert_eq!((m.rate_hz, m.attack_ms, m.release_ms), (1.0, 0.1, 200.0));
        assert_eq!(m.targets.len(), MAX_MOD_TARGETS);
        assert!(m.targets.iter().all(|t| t.depth == 1.0));
        assert_eq!(m.sync_quarters(), Some(0.125));
        let json = serde_json::to_string(&Modulator::new(3, ModulatorKind::Envelope)).unwrap();
        let back: Modulator = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Modulator::new(3, ModulatorKind::Envelope));
    }
}

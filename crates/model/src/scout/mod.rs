//! Scout: finished captures, the conversation with Claude and the goal, kept in the session.
//!
//! Changed only through `scout.*` commands. [`ScoutMetrics`] mirrors `soundcraft_dsp::scout::Metrics`
//! field for field (the model cannot depend on dsp; `soundcraft-mix` converts).

mod profile;
pub use profile::{MAX_PROFILE_BYTES, PROFILE_ROLES, ProfileKind, ProfileSource, ScoutProfile, profile_file_stem};

use crate::{Mixer, ModuleId, TrackId, TrackKind};
use serde::{Deserialize, Serialize};
use soundcraft_time::Range;

/// Most captures, turns or strips a loaded session may carry (a hostile file cannot balloon memory).
pub const MAX_SCOUT_ITEMS: usize = 10_000;

// ---- lenient loading: a malformed `scout` block never makes the session unloadable ----------

/// Drops `null`, non-finite and non-`f32` numbers from an object (so the field keeps its default)
/// and zeroes such numbers inside arrays. JSON writes NaN and infinity as `null`.
fn clean(v: &mut serde_json::Value) {
    use serde_json::Value;
    let bad = |n: &Value| match n {
        Value::Null => true,
        Value::Number(n) => !n.as_f64().is_some_and(|f| (f as f32).is_finite()),
        _ => false,
    };
    if let Value::Object(m) = v {
        m.retain(|_, x| !bad(x));
        for x in m.values_mut() {
            if let Value::Array(a) = x {
                for e in a.iter_mut() {
                    if bad(e) {
                        *e = Value::from(0.0);
                    }
                }
            }
        }
    }
}

fn lenient_f64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let v: Option<f64> = Option::deserialize(d).unwrap_or(None);
    Ok(v.filter(|v| v.is_finite()).unwrap_or(0.0))
}

fn lenient_f32<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    Ok(lenient_f64(d)? as f32).map(|v| if v.is_finite() { v } else { 0.0 })
}

fn lenient_metrics<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ScoutMetrics, D::Error> {
    let mut v = serde_json::Value::deserialize(d)?;
    clean(&mut v);
    Ok(serde_json::from_value(v).unwrap_or_default())
}

/// Entries with an unknown `kind` or a bad shape are skipped.
fn lenient_guides<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Guide>, D::Error> {
    let items: Vec<serde_json::Value> = Vec::deserialize(d)?;
    Ok(items
        .into_iter()
        .take(MAX_SCOUT_ITEMS)
        .filter_map(|mut v| {
            clean(&mut v);
            serde_json::from_value(v).ok()
        })
        .collect())
}

/// Last resort: any remaining error gives an empty Scout; sizes are capped.
pub(crate) fn lenient_state<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ScoutState, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let mut st: ScoutState = serde_json::from_value(v).unwrap_or_default();
    st.captures.truncate(MAX_SCOUT_ITEMS);
    st.turns.truncate(MAX_SCOUT_ITEMS);
    for c in &mut st.captures {
        c.strips.truncate(MAX_SCOUT_ITEMS);
    }
    Ok(st)
}

/// Spectrum bands per metrics record (`ScoutDSP::numBands`).
pub const SCOUT_BANDS: usize = 31;

/// Everything Scout keeps in a session.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ScoutState {
    /// What the user is trying to achieve.
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub captures: Vec<ScoutCapture>,
    #[serde(default)]
    pub turns: Vec<ScoutTurn>,
    /// The turn whose guides are drawn, if any.
    #[serde(default)]
    pub guides_shown: Option<u32>,
    /// Next capture / turn id (shared counter, always above every id in use).
    #[serde(default)]
    pub next_id: u32,
}

impl ScoutState {
    /// True when nothing is recorded (then the session file omits `scout`).
    pub fn is_empty(&self) -> bool {
        *self == ScoutState::default()
    }
}

/// One PRE/POST capture of a set of strips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoutCapture {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub name: String,
    /// RFC 3339.
    #[serde(default)]
    pub created: String,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub sample_rate: f64,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub tempo: f64,
    #[serde(default)]
    pub played: Vec<Range>,
    #[serde(default)]
    pub loop_range: Option<Range>,
    /// More ranges were played than the capture could list: the last entry of `played` was
    /// widened to span every later one, so `played` is no longer exact.
    #[serde(default)]
    pub played_truncated: bool,
    #[serde(default)]
    pub strips: Vec<ScoutStrip>,
    /// The band design the strips' `spectrum31` was measured with
    /// (`soundcraft_dsp::scout::BANDS_VERSION`): 1 = the original 2nd-order bands, 2 = the
    /// 6th-order Butterworth bands of 2026-10-21. Captures stored before the field existed
    /// load as 1; spectra of different versions do not compare directly.
    #[serde(default = "old_bands_version", deserialize_with = "lenient_bands_version")]
    pub bands_version: u32,
}

/// `bands_version` of a capture stored before the field existed.
fn old_bands_version() -> u32 {
    1
}

/// A `bands_version` that is not a u32 reads as the old bands rather than dropping the block.
fn lenient_bands_version<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v: Option<u32> = Option::deserialize(d).unwrap_or(None);
    Ok(v.unwrap_or_else(old_bands_version))
}

fn default_mixer() -> Mixer {
    Mixer::new(crate::ChannelFormat::Stereo)
}

/// One strip's result inside a capture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoutStrip {
    #[serde(default)]
    pub track: TrackId,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: TrackKind,
    /// "channel" | "fx_bus" | "mix_bus" | "master".
    #[serde(default)]
    pub channel_type: String,
    /// "Channel" | "Bus" | "Master".
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub channels: u8,
    /// Snapshot at capture start.
    #[serde(default = "default_mixer")]
    pub mixer: Mixer,
    #[serde(default)]
    pub automated: Vec<AutomatedParam>,
    /// PRE (before the first insert).
    #[serde(default, deserialize_with = "lenient_metrics")]
    pub base: ScoutMetrics,
    /// POST (after the last insert).
    #[serde(default, deserialize_with = "lenient_metrics")]
    pub post: ScoutMetrics,
    #[serde(default)]
    pub note: Option<String>,
    /// The sound profile this strip is moving toward (a copy of the library's, so the session
    /// keeps it). A target on the master is a mastering target.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "lenient_target")]
    pub target: Option<Box<ScoutProfile>>,
}

/// A target that doesn't read as a profile is dropped, never the strip.
fn lenient_target<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Box<ScoutProfile>>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    if v.is_null() {
        return Ok(None);
    }
    Ok(ScoutProfile::from_json(&v.to_string()).ok().map(Box::new))
}

/// A module parameter that moved during the capture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomatedParam {
    #[serde(default)]
    pub module: ModuleId,
    #[serde(default)]
    pub param: String,
    #[serde(default, deserialize_with = "lenient_f32")]
    pub start: f32,
    #[serde(default, deserialize_with = "lenient_f32")]
    pub end: f32,
}

/// `soundcraft_dsp::scout::Metrics`, field for field (same names, types and defaults).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
// Struct-level default: a missing field takes the C++ initialiser (-100 dB, correlation 1), not zero.
#[serde(default)]
pub struct ScoutMetrics {
    pub valid: bool,
    pub duration_sec: f32,
    pub sample_rate: f32,
    pub activity_ratio: f32,
    pub clipped_samples: i32,
    pub peak_hold_dbfs: f32,
    pub noise_floor_dbfs: f32,
    pub peak_dbfs: f32,
    pub true_peak_dbtp: f32,
    pub rms_dbfs: f32,
    pub lufs_integrated: f32,
    pub lra: f32,
    pub crest_factor_db: f32,
    pub dynamic_range_db: f32,
    pub peak_to_lufs_db: f32,
    pub transient_density: f32,
    pub spectrum31: [f32; SCOUT_BANDS],
    pub zone_sub: f32,
    pub zone_low: f32,
    pub zone_low_mid: f32,
    pub zone_mid: f32,
    pub zone_high_mid: f32,
    pub zone_high: f32,
    pub zone_air: f32,
    pub stereo_correlation: f32,
    pub stereo_width: f32,
    pub low_freq_correlation: f32,
    pub lr_balance_db: f32,
    pub dc_offset: f32,
    pub spectral_centroid_hz: f32,
    pub mid_side_ratio_db: f32,
    pub side_low_ratio_db: f32,
    pub noise_floor_p10_dbfs: f32,
    pub lufs_p10: f32,
    pub lufs_p50: f32,
    pub lufs_p90: f32,
    pub lufs_momentary_max: f32,
    pub lufs_short_term_max: f32,
}

/// The C++ member initialisers (`ScoutDSP::Metrics`).
impl Default for ScoutMetrics {
    fn default() -> Self {
        Self {
            valid: false,
            duration_sec: 0.0,
            sample_rate: 0.0,
            activity_ratio: 0.0,
            clipped_samples: 0,
            peak_hold_dbfs: -100.0,
            noise_floor_dbfs: -100.0,
            peak_dbfs: -100.0,
            true_peak_dbtp: -100.0,
            rms_dbfs: -100.0,
            lufs_integrated: -100.0,
            lra: 0.0,
            crest_factor_db: 0.0,
            dynamic_range_db: 0.0,
            peak_to_lufs_db: 0.0,
            transient_density: 0.0,
            spectrum31: [0.0; SCOUT_BANDS],
            zone_sub: 0.0,
            zone_low: 0.0,
            zone_low_mid: 0.0,
            zone_mid: 0.0,
            zone_high_mid: 0.0,
            zone_high: 0.0,
            zone_air: 0.0,
            stereo_correlation: 1.0,
            stereo_width: 0.0,
            low_freq_correlation: 1.0,
            lr_balance_db: 0.0,
            dc_offset: 0.0,
            spectral_centroid_hz: 0.0,
            mid_side_ratio_db: -100.0,
            side_low_ratio_db: -100.0,
            noise_floor_p10_dbfs: -100.0,
            lufs_p10: -100.0,
            lufs_p50: -100.0,
            lufs_p90: -100.0,
            lufs_momentary_max: -100.0,
            lufs_short_term_max: -100.0,
        }
    }
}

/// One question and Claude's answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScoutTurn {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub question: String,
    /// Capture ids sent with the question.
    #[serde(default)]
    pub captures: Vec<u32>,
    #[serde(default)]
    pub answer: String,
    #[serde(default, deserialize_with = "lenient_guides")]
    pub guides: Vec<Guide>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub model: String,
    /// Why guides in the answer were left out (unknown track, nonsense numbers…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A visual suggestion from Claude. Guides never change the mix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Guide {
    Eq {
        #[serde(default)]
        track: String,
        #[serde(default)]
        lo_hz: f32,
        #[serde(default)]
        hi_hz: f32,
        #[serde(default)]
        gain_lo_db: f32,
        #[serde(default)]
        gain_hi_db: f32,
        #[serde(default)]
        dynamic: bool,
        #[serde(default)]
        q: Option<f32>,
        #[serde(default)]
        label: String,
    },
    Module {
        #[serde(default)]
        track: String,
        #[serde(default)]
        module: String,
        #[serde(default)]
        param: String,
        #[serde(default)]
        current: String,
        #[serde(default)]
        range: String,
        #[serde(default)]
        reason: String,
    },
}

#[cfg(test)]
mod tests;

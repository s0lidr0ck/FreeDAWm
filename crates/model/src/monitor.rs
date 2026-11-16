//! Monitor profiles: what the monitor stage (`soundcraft_dsp::monitor`) is set up from. They
//! belong to the listener and the computer, never to a session: one JSON file each in the
//! settings folder (`soundcraft_engine::monitor_profiles`).

use serde::{Deserialize, Serialize};

/// Most bytes of a monitor profile file that are read.
pub const MAX_MONITOR_PROFILE_BYTES: usize = 1 << 20;
/// Most points of a profile's curve that are kept.
pub const MAX_MONITOR_POINTS: usize = 256;
const MAX_TEXT: usize = 200;

fn cap(s: &mut String) {
    *s = s.trim().chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect();
}

fn clean(points: &mut Vec<[f32; 2]>) {
    points.retain(|p| p[0].is_finite() && p[1].is_finite() && p[0] >= 10.0 && p[0] <= 24_000.0);
    points.truncate(MAX_MONITOR_POINTS);
    points.iter_mut().for_each(|p| p[1] = p[1].clamp(-40.0, 40.0));
    points.sort_by(|a, b| a[0].total_cmp(&b[0]));
}

fn one() -> f32 {
    1.0
}

/// A hearing correction for one listener on one pair of headphones: the gain each ear needs at
/// each frequency, as `[Hz, dB]` points (a positive gain where that ear hears less). The stage
/// applies the shape and lowers the whole so nothing is boosted above the mix.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HearingProfile {
    #[serde(default)]
    pub name: String,
    /// The headphones it was made on (free text).
    #[serde(default)]
    pub headphones: String,
    #[serde(default)]
    pub left: Vec<[f32; 2]>,
    #[serde(default)]
    pub right: Vec<[f32; 2]>,
}

impl HearingProfile {
    pub fn sanitize(&mut self) {
        cap(&mut self.name);
        cap(&mut self.headphones);
        clean(&mut self.left);
        clean(&mut self.right);
    }

    /// Reads a profile file's text (lenient: missing fields are empty, hostile numbers dropped).
    pub fn from_json(text: &str) -> Result<HearingProfile, String> {
        let mut p: HearingProfile = serde_json::from_str(text).map_err(|e| e.to_string())?;
        p.sanitize();
        Ok(p)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// A simulated output (a phone, a car…): the response the mix is heard through.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceProfile {
    #[serde(default)]
    pub name: String,
    /// The response as `[Hz, dB]` points, the same on both sides.
    #[serde(default)]
    pub curve: Vec<[f32; 2]>,
    /// A 24 dB/oct high-pass, Hz (what a small speaker cannot play).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highpass_hz: Option<f32>,
    /// A 24 dB/oct low-pass, Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lowpass_hz: Option<f32>,
    /// Stereo width: 1 as mixed, 0 mono.
    #[serde(default = "one")]
    pub width: f32,
    /// Level of the whole, dB.
    #[serde(default)]
    pub trim_db: f32,
    #[serde(default)]
    pub notes: String,
}

impl Default for DeviceProfile {
    fn default() -> Self {
        DeviceProfile { name: String::new(), curve: Vec::new(), highpass_hz: None, lowpass_hz: None, width: 1.0, trim_db: 0.0, notes: String::new() }
    }
}

impl DeviceProfile {
    pub fn sanitize(&mut self) {
        cap(&mut self.name);
        cap(&mut self.notes);
        clean(&mut self.curve);
        let hz = |v: Option<f32>| v.filter(|f| f.is_finite() && *f >= 10.0 && *f <= 24_000.0);
        self.highpass_hz = hz(self.highpass_hz);
        self.lowpass_hz = hz(self.lowpass_hz);
        self.width = if self.width.is_finite() { self.width.clamp(0.0, 1.0) } else { 1.0 };
        self.trim_db = if self.trim_db.is_finite() { self.trim_db.clamp(-40.0, 12.0) } else { 0.0 };
    }

    pub fn from_json(text: &str) -> Result<DeviceProfile, String> {
        let mut p: DeviceProfile = serde_json::from_str(text).map_err(|e| e.to_string())?;
        p.sanitize();
        Ok(p)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_round_trip_and_read_leniently() {
        let h = HearingProfile { name: "Me".into(), headphones: "HD 600".into(), left: vec![[1000.0, 3.0]], right: vec![[4000.0, 9.5]] };
        assert_eq!(HearingProfile::from_json(&h.to_json()).unwrap(), h);
        let d = DeviceProfile { name: "Phone".into(), curve: vec![[500.0, -3.0]], highpass_hz: Some(300.0), width: 0.0, ..DeviceProfile::default() };
        assert_eq!(DeviceProfile::from_json(&d.to_json()).unwrap(), d);
        assert_eq!(DeviceProfile::from_json("{}").unwrap(), DeviceProfile::default());
    }

    #[test]
    fn hostile_values_are_dropped_or_clamped() {
        let p = DeviceProfile::from_json(
            r#"{"name":"  x\u0007 ","curve":[[5000,1e30],[100,-2],[1e9,0],[-1,0]],"highpass_hz":-4,"lowpass_hz":1e12,"width":9,"trim_db":1e9}"#,
        )
        .unwrap();
        assert_eq!(p.name, "x");
        assert_eq!(p.curve, vec![[100.0, -2.0], [5000.0, 40.0]]);
        assert_eq!((p.highpass_hz, p.lowpass_hz, p.width, p.trim_db), (None, None, 1.0, 12.0));
        assert!(HearingProfile::from_json("[").is_err());
    }
}

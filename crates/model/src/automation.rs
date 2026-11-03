//! Automation lanes and modes.

use crate::ModuleId;
use soundcraft_time::Samples;

/// Automation modes (per track).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum AutomationMode {
    Off,
    #[default]
    Read,
    Touch,
    Latch,
    TouchLatch,
    Write,
    Trim,
}

impl AutomationMode {
    pub const ALL: [AutomationMode; 7] = [
        AutomationMode::Off,
        AutomationMode::Read,
        AutomationMode::Touch,
        AutomationMode::Latch,
        AutomationMode::TouchLatch,
        AutomationMode::Write,
        AutomationMode::Trim,
    ];
    pub fn label(self) -> &'static str {
        match self {
            AutomationMode::Off => "off",
            AutomationMode::Read => "read",
            AutomationMode::Touch => "touch",
            AutomationMode::Latch => "latch",
            AutomationMode::TouchLatch => "touch/latch",
            AutomationMode::Write => "write",
            AutomationMode::Trim => "trim",
        }
    }
    pub fn from_id(s: &str) -> Option<AutomationMode> {
        let s = s.to_ascii_lowercase();
        AutomationMode::ALL.into_iter().find(|m| m.label() == s || m.label().replace('/', "_") == s || format!("{m:?}").to_ascii_lowercase() == s)
    }
    pub fn reads(self) -> bool {
        !matches!(self, AutomationMode::Off)
    }
}

/// Which parameter an automation lane controls.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum AutoParam {
    /// Fader level, dB.
    Volume,
    /// Pan, -1..1 (index = which panner of a stereo track: 0 left, 1 right).
    Pan(u8),
    /// Mute (0/1).
    Mute,
    /// Send level, dB.
    SendLevel(u8),
    SendPan(u8),
    SendMute(u8),
    /// Trim offset on top of the volume curve, dB.
    Trim,
    /// A chain module (by stable id) + parameter id.
    Plugin {
        module: ModuleId,
        param: String,
    },
}

impl AutoParam {
    pub fn label(&self) -> String {
        match self {
            AutoParam::Volume => "volume".into(),
            AutoParam::Pan(0) => "pan".into(),
            AutoParam::Pan(i) => format!("pan {}", i + 1),
            AutoParam::Mute => "mute".into(),
            AutoParam::SendLevel(i) => format!("send {} level", slot_letter(*i)),
            AutoParam::SendPan(i) => format!("send {} pan", slot_letter(*i)),
            AutoParam::SendMute(i) => format!("send {} mute", slot_letter(*i)),
            AutoParam::Trim => "trim".into(),
            AutoParam::Plugin { module, param } => format!("module {} {param}", module.0),
        }
    }
    /// The text id [`AutoParam::parse`] reads back.
    pub fn id(&self) -> String {
        match self {
            AutoParam::Volume => "volume".into(),
            AutoParam::Pan(0) => "pan".into(),
            AutoParam::Pan(_) => "pan2".into(),
            AutoParam::Mute => "mute".into(),
            AutoParam::SendLevel(i) => format!("send_{}_level", slot_letter(*i).to_ascii_lowercase()),
            AutoParam::SendPan(i) => format!("send_{}_pan", slot_letter(*i).to_ascii_lowercase()),
            AutoParam::SendMute(i) => format!("send_{}_mute", slot_letter(*i).to_ascii_lowercase()),
            AutoParam::Trim => "trim".into(),
            AutoParam::Plugin { module, param } => format!("plugin:m{}:{param}", module.0),
        }
    }
    /// Parse ids like `volume`, `pan`, `pan2`, `mute`, `send_a_level`, `trim`, `plugin:m3:threshold`.
    /// Chain positions (`plugin:a:..`) need a track and are resolved by the engine.
    /// The words are read in any case; a plugin's parameter id keeps its own (it is the module's
    /// key, matched exactly when the lane plays: FireStation's ids are camelCase).
    pub fn parse(s: &str) -> Option<AutoParam> {
        let raw = s.trim();
        let s = raw.to_ascii_lowercase();
        match s.as_str() {
            "volume" | "vol" => return Some(AutoParam::Volume),
            "pan" | "pan1" => return Some(AutoParam::Pan(0)),
            "pan2" => return Some(AutoParam::Pan(1)),
            "mute" => return Some(AutoParam::Mute),
            "trim" => return Some(AutoParam::Trim),
            _ => {}
        }
        if let Some(rest) = s.strip_prefix("send_") {
            let mut it = rest.splitn(2, '_');
            let slot = letter_slot(it.next()?)?;
            return match it.next()? {
                "level" => Some(AutoParam::SendLevel(slot)),
                "pan" => Some(AutoParam::SendPan(slot)),
                "mute" => Some(AutoParam::SendMute(slot)),
                _ => None,
            };
        }
        if s.starts_with("plugin:") {
            // ASCII lowercasing keeps byte offsets, so the original text slices the same way.
            let (module, param) = raw.get("plugin:".len()..)?.split_once(':')?;
            let id: u32 = module.to_ascii_lowercase().strip_prefix('m')?.parse().ok()?;
            return Some(AutoParam::Plugin { module: ModuleId(id), param: param.to_string() });
        }
        None
    }
    /// Value range and default.
    pub fn range(&self) -> (f32, f32, f32) {
        match self {
            AutoParam::Volume | AutoParam::SendLevel(_) => (-144.0, 12.0, 0.0),
            AutoParam::Pan(_) | AutoParam::SendPan(_) => (-1.0, 1.0, 0.0),
            AutoParam::Mute | AutoParam::SendMute(_) => (0.0, 1.0, 0.0),
            AutoParam::Trim => (-144.0, 24.0, 0.0),
            AutoParam::Plugin { .. } => (f32::MIN, f32::MAX, 0.0),
        }
    }
}

pub fn slot_letter(i: u8) -> char {
    char::from(b'A'.saturating_add(i.min(25)))
}

/// Parses a legacy chain-position token: a letter (`a`) or an index (`0`).
pub fn letter_slot(s: &str) -> Option<u8> {
    let c = s.chars().next()?.to_ascii_uppercase();
    if s.len() == 1 && c.is_ascii_uppercase() { Some(c as u8 - b'A') } else { s.parse().ok() }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AutomationPoint {
    pub at: Samples,
    pub value: f32,
    /// Shape of the segment from this point to the next: 0 linear, -1..1 bowed one way or the
    /// other ([`curve_shape`]). Sessions written before curves load as straight lines.
    #[serde(default, skip_serializing_if = "is_linear")]
    pub curve: f32,
}

fn is_linear(c: &f32) -> bool {
    *c == 0.0
}

impl AutomationPoint {
    pub fn new(at: Samples, value: f32) -> Self {
        AutomationPoint { at, value, curve: 0.0 }
    }
}

/// A segment with |curve| above this is kept by Thin and drawn bowed.
pub const CURVE_EPSILON: f32 = 0.01;

/// The eased position `t` (0..1) along a segment of curvature `k` (-1..1): `t ^ (2 ^ (2k))`, so
/// 0 is linear and ±1 strongly bowed (`t^4` / `t^0.25`). Arithmetic only (read by the audio thread).
pub fn curve_shape(k: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if k.abs() < CURVE_EPSILON || k.is_nan() {
        return t;
    }
    t.powf(2f32.powf(2.0 * k.clamp(-1.0, 1.0)))
}

/// A breakpoint automation lane. Points are kept sorted by time.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AutomationLane {
    pub param: AutoParam,
    pub points: Vec<AutomationPoint>,
    #[serde(default)]
    pub enabled: bool,
}

impl AutomationLane {
    pub fn new(param: AutoParam) -> Self {
        AutomationLane { param, points: Vec::new(), enabled: true }
    }

    /// Value at `at` (linear interpolation; `default` when empty).
    pub fn value_at(&self, at: Samples, default: f32) -> f32 {
        let pts = &self.points;
        let Some(first) = pts.first() else { return default };
        if at <= first.at {
            return first.value;
        }
        let idx = pts.partition_point(|p| p.at <= at);
        match (pts.get(idx.wrapping_sub(1)), pts.get(idx)) {
            (Some(a), Some(b)) if b.at > a.at => {
                // Mute-like lanes step instead of ramping.
                if matches!(self.param, AutoParam::Mute | AutoParam::SendMute(_)) {
                    return a.value;
                }
                let t = (at - a.at) as f32 / (b.at - a.at) as f32;
                a.value + (b.value - a.value) * curve_shape(a.curve, t)
            }
            (Some(a), _) => a.value,
            _ => default,
        }
    }

    /// Insert or replace a breakpoint (a replaced one keeps its curve).
    pub fn set_point(&mut self, at: Samples, value: f32) {
        self.set_point_curved(at, value, None);
    }

    /// Insert or replace a breakpoint, setting its segment's curve when `curve` is given.
    pub fn set_point_curved(&mut self, at: Samples, value: f32, curve: Option<f32>) {
        let (lo, hi, _) = self.param.range();
        let value = if value.is_nan() { 0.0 } else { value.clamp(lo, hi) };
        let curve = curve.map(|c| if c.is_nan() { 0.0 } else { c.clamp(-1.0, 1.0) });
        match self.points.binary_search_by(|p| p.at.cmp(&at)) {
            Ok(i) => {
                if let Some(p) = self.points.get_mut(i) {
                    p.value = value;
                    if let Some(c) = curve {
                        p.curve = c;
                    }
                }
            }
            Err(i) => self.points.insert(i, AutomationPoint { at, value, curve: curve.unwrap_or(0.0) }),
        }
    }

    /// Set the curve of the segment starting at the point at `at`. False when there is none.
    pub fn set_curve(&mut self, at: Samples, curve: f32) -> bool {
        let curve = if curve.is_nan() { 0.0 } else { curve.clamp(-1.0, 1.0) };
        match self.points.binary_search_by(|p| p.at.cmp(&at)) {
            Ok(i) => {
                if let Some(p) = self.points.get_mut(i) {
                    p.curve = curve;
                }
                true
            }
            Err(_) => false,
        }
    }

    /// The times of the points inside `[start, end]`.
    pub fn points_in(&self, start: Samples, end: Samples) -> Vec<Samples> {
        self.points.iter().filter(|p| p.at >= start && p.at <= end).map(|p| p.at).collect()
    }

    /// Remove the points at the given times. Returns how many went.
    pub fn remove_points(&mut self, ats: &[Samples]) -> usize {
        let before = self.points.len();
        self.points.retain(|p| !ats.contains(&p.at));
        before - self.points.len()
    }

    /// Move the points at `ats` by `dt` samples and `dv` in value, together: the group keeps its
    /// shape, stopping at time 0 and at the value range; a moved point landing on another point
    /// replaces it. Returns the moved points' new times (in the order of `ats`).
    pub fn move_points(&mut self, ats: &[Samples], dt: Samples, dv: f32) -> Vec<Samples> {
        let (lo, hi, _) = self.param.range();
        let dv = if dv.is_nan() { 0.0 } else { dv };
        let moving: Vec<AutomationPoint> = self.points.iter().filter(|p| ats.contains(&p.at)).copied().collect();
        if moving.is_empty() {
            return Vec::new();
        }
        // The group stays inside the lane as a whole.
        let first = moving.iter().map(|p| p.at).min().unwrap_or(0);
        let dt = dt.max(-first);
        let (vmin, vmax) = moving.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.value), b.max(p.value)));
        let dv = if lo.is_finite() && hi.is_finite() { dv.min(hi - vmax).max(lo - vmin) } else { dv };
        let dv = if dv.is_finite() { dv } else { 0.0 };
        let moved: Vec<AutomationPoint> =
            moving.iter().map(|p| AutomationPoint { at: p.at.saturating_add(dt).max(0), value: p.value + dv, curve: p.curve }).collect();
        self.points.retain(|p| !ats.contains(&p.at));
        self.points.retain(|p| !moved.iter().any(|m| m.at == p.at));
        self.points.extend(moved.iter().copied());
        self.points.sort_by_key(|p| p.at);
        self.points.dedup_by_key(|p| p.at);
        ats.iter().filter_map(|a| moving.iter().position(|p| p.at == *a)).filter_map(|i| moved.get(i).map(|p| p.at)).collect()
    }

    /// Write a drawn series (a Pencil pass), replacing what was under it: the points inside the
    /// series' span go, anchors keep the curve outside it unchanged, then the series is thinned
    /// with `tolerance` so a freehand line yields a reasonable number of points.
    pub fn draw(&mut self, series: &[(Samples, f32)], default: f32, tolerance: f32) {
        let mut pts: Vec<(Samples, f32)> = series.iter().filter(|(_, v)| v.is_finite()).map(|(a, v)| ((*a).max(0), *v)).collect();
        pts.sort_by_key(|p| p.0);
        pts.dedup_by_key(|p| p.0);
        let (Some(&(first, _)), Some(&(last, _))) = (pts.first(), pts.last()) else { return };
        let before = self.value_at(first.saturating_sub(1), default);
        let after = self.value_at(last.saturating_add(1), default);
        let had_points = !self.points.is_empty();
        self.clear_range(first, last.saturating_add(1));
        if had_points {
            if first > 0 {
                self.set_point_curved(first - 1, before, Some(0.0));
            }
            self.set_point_curved(last.saturating_add(1), after, Some(0.0));
        }
        if pts.len() > 2 && tolerance > 0.0 {
            let mut tmp = AutomationLane::new(self.param.clone());
            for (a, v) in &pts {
                tmp.set_point(*a, *v);
            }
            tmp.thin(tolerance);
            pts = tmp.points.iter().map(|p| (p.at, p.value)).collect();
        }
        for (a, v) in pts {
            self.set_point_curved(a, v, Some(0.0));
        }
    }

    /// Remove points in `[start, end)`.
    pub fn clear_range(&mut self, start: Samples, end: Samples) -> usize {
        let before = self.points.len();
        self.points.retain(|p| p.at < start || p.at >= end);
        before - self.points.len()
    }

    /// Write a constant value over a range, keeping the surrounding curve intact with anchor points.
    pub fn write_range(&mut self, start: Samples, end: Samples, value: f32, default: f32) {
        if end <= start {
            return;
        }
        let before = self.value_at(start.saturating_sub(1), default);
        let after = self.value_at(end, default);
        self.clear_range(start, end.saturating_add(1));
        if start > 0 {
            self.set_point(start - 1, before);
        }
        self.set_point(start, value);
        self.set_point(end, value);
        self.set_point(end.saturating_add(1), after);
    }

    /// Thin: remove points whose removal changes the curve by less than `tolerance`.
    pub fn thin(&mut self, tolerance: f32) -> usize {
        // Linear thinning would move the transitions of step lanes.
        if matches!(self.param, AutoParam::Mute | AutoParam::SendMute(_)) || self.points.len() < 3 || tolerance.is_nan() {
            return 0;
        }
        let tolerance = f64::from(tolerance);
        let mut keep = vec![true; self.points.len()];
        let mut last = 0usize;
        // Slopes from the last kept point that keep every point removed since then within `tolerance`.
        let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
        for i in 1..self.points.len() - 1 {
            let (Some(a), Some(b), Some(c)) = (self.points.get(last), self.points.get(i), self.points.get(i + 1)) else { continue };
            // A curved segment is kept whole: neither its start nor its end goes.
            let curved = b.curve.abs() > CURVE_EPSILON || self.points.get(i - 1).is_some_and(|p| p.curve.abs() > CURVE_EPSILON);
            if curved {
                last = i;
                (lo, hi) = (f64::NEG_INFINITY, f64::INFINITY);
                continue;
            }
            let dt = b.at.saturating_sub(a.at) as f64;
            let rise = f64::from(b.value) - f64::from(a.value);
            lo = lo.max((rise - tolerance) / dt);
            hi = hi.min((rise + tolerance) / dt);
            let slope = (f64::from(c.value) - f64::from(a.value)) / c.at.saturating_sub(a.at) as f64;
            if (lo..=hi).contains(&slope) {
                if let Some(k) = keep.get_mut(i) {
                    *k = false;
                }
            } else {
                last = i;
                (lo, hi) = (f64::NEG_INFINITY, f64::INFINITY);
            }
        }
        let before = self.points.len();
        let mut it = keep.iter();
        self.points.retain(|_| it.next().copied().unwrap_or(true));
        before - self.points.len()
    }

    /// Shift points at or after `from` by `delta` (Shuffle edits, Insert Time).
    pub fn shift_from(&mut self, from: Samples, delta: Samples) {
        for p in &mut self.points {
            if p.at >= from {
                p.at = p.at.saturating_add(delta).max(0);
            }
        }
        self.points.sort_by_key(|p| p.at);
        self.points.dedup_by_key(|p| p.at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_and_steps() {
        let mut l = AutomationLane::new(AutoParam::Volume);
        assert_eq!(l.value_at(10, -3.0), -3.0);
        l.set_point(0, 0.0);
        l.set_point(100, -10.0);
        assert!((l.value_at(50, 0.0) + 5.0).abs() < 1e-6);
        assert_eq!(l.value_at(1000, 0.0), -10.0);
        let mut m = AutomationLane::new(AutoParam::Mute);
        m.set_point(0, 0.0);
        m.set_point(100, 1.0);
        assert_eq!(m.value_at(99, 0.0), 0.0);
        assert_eq!(m.value_at(100, 0.0), 1.0);
    }

    #[test]
    fn write_range_preserves_outside() {
        let mut l = AutomationLane::new(AutoParam::Volume);
        l.set_point(0, 0.0);
        l.set_point(1000, 0.0);
        l.write_range(200, 400, -20.0, 0.0);
        assert_eq!(l.value_at(100, 0.0), 0.0);
        assert_eq!(l.value_at(300, 0.0), -20.0);
        assert_eq!(l.value_at(600, 0.0), 0.0);
    }

    #[test]
    fn thin_removes_collinear() {
        let mut l = AutomationLane::new(AutoParam::Volume);
        for i in 0..=10 {
            l.set_point(i * 10, -(i as f32));
        }
        assert_eq!(l.thin(0.01), 9);
        assert_eq!(l.points.len(), 2);
    }

    #[test]
    fn thin_keeps_removed_points_within_tolerance() {
        for direction in [1.0, -1.0] {
            let mut lane = AutomationLane::new(AutoParam::Volume);
            for (at, value) in [(0, 0.0), (100, 0.0), (200, 0.0), (300, 0.12), (400, 0.26)] {
                lane.set_point(at, value * direction);
            }
            let original = lane.points.clone();
            let tolerance = 0.1;
            assert!(lane.thin(tolerance) > 0);
            for point in original {
                let error = (lane.value_at(point.at, 0.0) - point.value).abs();
                assert!(error <= tolerance, "at {}: error {error} exceeds {tolerance}", point.at);
            }
        }
    }

    #[test]
    fn thin_keeps_mute_changes_in_place() {
        for param in [AutoParam::Mute, AutoParam::SendMute(0)] {
            let mut lane = AutomationLane::new(param);
            for (at, value) in [(0, 0.0), (9000, 1.0), (10000, 1.0)] {
                lane.set_point(at, value);
            }
            lane.thin(0.1);
            assert_eq!(lane.value_at(8999, 0.0), 0.0);
            assert_eq!(lane.value_at(9000, 0.0), 1.0);
        }
    }

    #[test]
    fn plugin_params_use_module_ids() {
        assert_eq!(AutoParam::parse("plugin:m3:threshold"), Some(AutoParam::Plugin { module: ModuleId(3), param: "threshold".into() }));
        assert_eq!(AutoParam::parse("trim"), Some(AutoParam::Trim));
        assert_eq!(AutoParam::parse("plugin:a:threshold"), None); // positions need a track: engine resolver
        // A parameter id keeps its case (FireStation's are camelCase): the lane must match the
        // module's own key, or it never plays.
        let p = AutoParam::Plugin { module: ModuleId(3), param: "ceilOn".into() };
        assert_eq!(AutoParam::parse(&p.id()), Some(p.clone()));
        assert_eq!(AutoParam::parse(" PLUGIN:M3:ceilOn "), Some(p));
        assert_eq!(AutoParam::Trim.range(), (-144.0, 24.0, 0.0));
    }

    #[test]
    fn param_ids() {
        assert_eq!(AutoParam::parse("send_b_level"), Some(AutoParam::SendLevel(1)));
        assert_eq!(AutoParam::parse("plugin:m3:threshold"), Some(AutoParam::Plugin { module: ModuleId(3), param: "threshold".into() }));
        assert_eq!(AutoParam::parse("nope"), None);
        for p in [
            AutoParam::Volume,
            AutoParam::Pan(1),
            AutoParam::SendMute(2),
            AutoParam::Trim,
            AutoParam::Plugin { module: ModuleId(7), param: "x".into() },
        ] {
            assert_eq!(AutoParam::parse(&p.id()), Some(p));
        }
        assert_eq!(AutomationMode::from_id("touch/latch"), Some(AutomationMode::TouchLatch));
    }

    #[test]
    fn curve_shapes_value_at() {
        let mut l = AutomationLane::new(AutoParam::Volume);
        l.set_point(0, 0.0);
        l.set_point(100, -10.0);
        assert!((l.value_at(50, 0.0) + 5.0).abs() < 1e-6, "k=0 is linear");
        assert!(l.set_curve(0, 1.0));
        // t^4 at the midpoint: 1/16 of the way.
        assert!((l.value_at(50, 0.0) + 10.0 / 16.0).abs() < 1e-4, "{}", l.value_at(50, 0.0));
        assert!(l.set_curve(0, -1.0));
        assert!((l.value_at(50, 0.0) + 10.0 * 0.5f32.powf(0.25)).abs() < 1e-4);
        assert_eq!(l.value_at(0, 0.0), 0.0);
        assert_eq!(l.value_at(100, 0.0), -10.0);
        assert!(!l.set_curve(7, 0.5), "no point there");
        // A replaced point keeps its curve.
        l.set_point(0, -1.0);
        assert_eq!(l.points[0].curve, -1.0);
        assert_eq!(curve_shape(0.0, 0.3), 0.3);
        assert_eq!(curve_shape(f32::NAN, 0.3), 0.3);
    }

    #[test]
    fn curve_serde_defaults_to_linear() {
        let old = r#"{"param":"Volume","points":[{"at":0,"value":0.0},{"at":10,"value":1.0}]}"#;
        let l: AutomationLane = serde_json::from_str(old).unwrap();
        assert!(l.points.iter().all(|p| p.curve == 0.0));
        let mut l2 = l.clone();
        l2.set_curve(0, 0.5);
        let text = serde_json::to_string(&l2).unwrap();
        assert!(text.contains("\"curve\":0.5"), "{text}");
        let back: AutomationLane = serde_json::from_str(&text).unwrap();
        assert_eq!(back, l2);
        assert!(!serde_json::to_string(&l).unwrap().contains("curve"), "linear points write no curve");
    }

    #[test]
    fn thin_keeps_curved_segments() {
        let mut l = AutomationLane::new(AutoParam::Volume);
        for i in 0..=10 {
            l.set_point(i * 10, -(i as f32));
        }
        l.set_curve(40, 0.8);
        l.thin(0.01);
        let ats: Vec<Samples> = l.points.iter().map(|p| p.at).collect();
        assert!(ats.contains(&40) && ats.contains(&50), "{ats:?}");
        assert_eq!(l.points.iter().find(|p| p.at == 40).map(|p| p.curve), Some(0.8));
        assert!(l.points.len() <= 4, "{ats:?}");
    }

    #[test]
    fn draw_replaces_the_span_and_keeps_anchors() {
        let mut l = AutomationLane::new(AutoParam::Volume);
        l.set_point(0, 0.0);
        l.set_point(1000, -20.0);
        // Old points under the drawn span: the sawtooth the owner saw.
        for at in [300, 400, 500] {
            l.set_point(at, 5.0);
        }
        let series: Vec<(Samples, f32)> = (0..=20).map(|i| (200 + i * 20, -10.0 - i as f32 * 0.5)).collect();
        l.draw(&series, 0.0, 0.01);
        let ats: Vec<Samples> = l.points.iter().map(|p| p.at).collect();
        assert!(!ats.contains(&300) && !ats.contains(&400) && !ats.contains(&500), "{ats:?}");
        // Anchors: the curve outside the span is what it was (0 -> 5 at 300; 5 at 500 -> -20 at 1000).
        assert!((l.value_at(100, 0.0) - 5.0 / 3.0).abs() < 1e-4, "{}", l.value_at(100, 0.0));
        assert!((l.value_at(800, 0.0) + 10.0).abs() < 1e-4, "{}", l.value_at(800, 0.0));
        // Inside: the drawn (straight) line, thinned to its ends.
        assert!((l.value_at(400, 0.0) + 15.0).abs() < 0.02, "{}", l.value_at(400, 0.0));
        assert!(ats.contains(&200) && ats.contains(&600));
        assert!(l.points.len() <= 6, "{ats:?}");
        // On an empty lane a drawn line needs no anchors.
        let mut e = AutomationLane::new(AutoParam::Pan(0));
        e.draw(&[(100, 0.5), (200, -0.5)], 0.0, 0.01);
        assert_eq!(e.points.len(), 2);
    }

    #[test]
    fn move_and_remove_points() {
        let mut l = AutomationLane::new(AutoParam::Volume);
        for (at, v) in [(0, 0.0), (100, -6.0), (200, -12.0), (300, -3.0)] {
            l.set_point(at, v);
        }
        let out = l.move_points(&[100, 200], 50, 2.0);
        assert_eq!(out, vec![150, 250]);
        let pts: Vec<(Samples, f32)> = l.points.iter().map(|p| (p.at, p.value)).collect();
        assert_eq!(pts, vec![(0, 0.0), (150, -4.0), (250, -10.0), (300, -3.0)]);
        // The group stops at time 0 and inside the range, keeping its shape.
        let out = l.move_points(&[150, 250], -500, 100.0);
        assert_eq!(out, vec![0, 100]);
        let pts: Vec<(Samples, f32)> = l.points.iter().map(|p| (p.at, p.value)).collect();
        assert_eq!(pts, vec![(0, 12.0), (100, 6.0), (300, -3.0)], "landing on 0 replaced that point");
        assert_eq!(l.points_in(0, 100), vec![0, 100]);
        assert_eq!(l.remove_points(&[100, 999]), 1);
        assert_eq!(l.points.len(), 2);
        assert!(l.move_points(&[], 1, 1.0).is_empty());
    }

    #[test]
    fn set_point_clamps_nan() {
        let mut l = AutomationLane::new(AutoParam::Pan(0));
        l.set_point(0, f32::NAN);
        l.set_point(1, 5.0);
        assert_eq!(l.points[0].value, 0.0);
        assert_eq!(l.points[1].value, 1.0);
    }
}

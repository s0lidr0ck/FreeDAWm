//! Automation inside a clip (Logic's region automation, Studio Pro's part envelopes): lanes
//! stored on the clip ([`crate::Clip::automation`], points in samples from the clip's start) that
//! play while the clip plays.
//!
//! The mixer is not taught about clips: the track's *played* lanes ([`crate::Track::played`],
//! not saved) are its own lanes with each clip's lanes laid over the clip's range, made by
//! [`crate::Track::refresh_played`] after each edit (the engine does it after every command that
//! changed the document). Inside a clip with a lane for a parameter, a dB lane (volume, trim, a
//! send's level) adds to the track's lane there and any other lane replaces it; outside, the
//! track's lane plays as before. At the clip's edges the value moves over [`EDGE_RAMP`] samples
//! (no click). A parameter only the clip automates holds the clip's first value before it and its
//! last value after it, as any lane holds its end values. Playback, a bounce and a freeze read the
//! same flattened lanes.

use crate::{AutoParam, AutomationLane, AutomationPoint, Clip};
use soundcraft_time::Samples;

/// The ramp at a clip's edges where its automation takes over from the track's or hands back.
pub const EDGE_RAMP: Samples = 64;

/// Whether a clip's lane for `p` adds to the track's (a level in dB) rather than replacing it.
pub fn adds(p: &AutoParam) -> bool {
    matches!(p, AutoParam::Volume | AutoParam::Trim | AutoParam::SendLevel(_))
}

/// The value `p` plays at `at` given the track's lane (`base`) and one clip's lane over
/// `[start, end)` (points from `start`).
fn inside(p: &AutoParam, base: Option<&AutomationLane>, clip: &AutomationLane, start: Samples, at: Samples) -> f32 {
    let def = p.range().2;
    let c = clip.value_at(at.saturating_sub(start), def);
    if adds(p) { base.map_or(def, |b| b.value_at(at, def)) + c } else { c }
}

/// The played lane for `p`: the track's lane `base` (if any) with each of `clips`' lanes for
/// `p` laid over the clip's range. `None` when no clip has a lane for `p` (the track's lane plays
/// as it is).
pub fn flatten(p: &AutoParam, base: Option<&AutomationLane>, clips: &[Clip]) -> Option<AutomationLane> {
    let mut over: Vec<(Samples, Samples, &AutomationLane)> = clips
        .iter()
        .filter(|c| !c.muted)
        .filter_map(|c| c.automation.iter().find(|l| &l.param == p && !l.points.is_empty()).map(|l| (c.start, c.end(), l)))
        .collect();
    if over.is_empty() {
        return None;
    }
    over.sort_by_key(|(s, ..)| *s);
    // A later clip cuts an earlier one short where they overlap.
    for i in 1..over.len() {
        let next = over[i].0;
        if let Some(prev) = over.get_mut(i - 1) {
            prev.1 = prev.1.min(next).max(prev.0);
        }
    }
    let def = p.range().2;
    let base_at = |at: Samples, first: f32| base.map_or(first, |b| b.value_at(at, def));
    let mut out = AutomationLane::new(p.clone());
    out.enabled = base.is_none_or(|b| b.enabled);
    out.suspended = base.is_some_and(|b| b.suspended);
    let mut pts: Vec<AutomationPoint> = Vec::new();
    let mut prev_end = Samples::MIN;
    for (k, (start, end, lane)) in over.iter().enumerate() {
        let (start, end) = (*start, *end);
        if end <= start {
            continue;
        }
        let first = inside(p, base, lane, start, start);
        // The track's own points between the previous clip and this one.
        if let Some(b) = base {
            pts.extend(b.points.iter().filter(|q| q.at > prev_end && q.at < start - EDGE_RAMP).copied());
        }
        // Into the clip.
        let ramp = EDGE_RAMP.min((end - start) / 2).max(1);
        if k == 0 || start - EDGE_RAMP > prev_end {
            pts.push(AutomationPoint::new(start - ramp, base_at(start - ramp, first)));
        }
        pts.push(AutomationPoint::new(start, first));
        // Its points (and, for a dB lane, the track's under it), as heard.
        let mut times: Vec<Samples> = lane.points.iter().map(|q| q.at.saturating_add(start)).filter(|t| *t > start && *t < end - ramp).collect();
        if adds(p)
            && let Some(b) = base
        {
            times.extend(b.points.iter().map(|q| q.at).filter(|t| *t > start && *t < end - ramp));
            times.sort_unstable();
            times.dedup();
            pts.extend(times.into_iter().map(|t| AutomationPoint::new(t, inside(p, base, lane, start, t))));
        } else {
            pts.extend(lane.points.iter().filter(|q| q.at > 0 && q.at.saturating_add(start) < end - ramp).map(|q| AutomationPoint {
                at: q.at.saturating_add(start),
                value: q.value,
                curve: q.curve,
            }));
        }
        // Out of it.
        pts.push(AutomationPoint::new(end - ramp, inside(p, base, lane, start, end - ramp)));
        let after = over.get(k + 1).map(|n| n.0);
        if after.is_none_or(|n| n - EDGE_RAMP > end) {
            pts.push(AutomationPoint::new(end, base_at(end, inside(p, base, lane, start, end - 1))));
        }
        prev_end = end;
    }
    if let Some(b) = base {
        pts.extend(b.points.iter().filter(|q| q.at > prev_end).copied());
    }
    pts.sort_by_key(|q| q.at);
    pts.dedup_by_key(|q| q.at);
    out.points = pts;
    Some(out)
}

/// The played lanes of a track with `lanes` and `clips` (see the module docs): empty when no clip
/// has automation, so playback reads the track's own lanes.
pub fn played(lanes: &[AutomationLane], clips: &[Clip]) -> Vec<AutomationLane> {
    if clips.iter().all(|c| c.automation.is_empty()) {
        return Vec::new();
    }
    let mut params: Vec<AutoParam> = Vec::new();
    for c in clips {
        for l in &c.automation {
            if !params.contains(&l.param) {
                params.push(l.param.clone());
            }
        }
    }
    let mut out: Vec<AutomationLane> = lanes.iter().filter(|l| !params.contains(&l.param)).cloned().collect();
    for p in &params {
        let base = lanes.iter().find(|l| &l.param == p);
        match flatten(p, base, clips) {
            Some(l) => out.push(l),
            None => out.extend(base.cloned()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClipId, SourceId};

    fn clip(start: Samples, len: Samples, lane: AutomationLane) -> Clip {
        let mut c = Clip::audio(ClipId(1), "c", SourceId(1), 0, start, len);
        c.automation = vec![lane];
        c
    }

    fn lane(p: AutoParam, pts: &[(Samples, f32)]) -> AutomationLane {
        let mut l = AutomationLane::new(p);
        for (t, v) in pts {
            l.set_point(*t, *v);
        }
        l
    }

    #[test]
    fn a_clip_lane_replaces_the_tracks_pan_over_its_range_only() {
        let track = lane(AutoParam::Pan(0), &[(0, -1.0), (100_000, -1.0)]);
        let c = clip(10_000, 10_000, lane(AutoParam::Pan(0), &[(0, 0.5), (9_999, 0.5)]));
        let l = flatten(&AutoParam::Pan(0), Some(&track), &[c]).unwrap();
        assert_eq!(l.value_at(5_000, 0.0), -1.0);
        assert_eq!(l.value_at(10_000, 0.0), 0.5);
        assert_eq!(l.value_at(15_000, 0.0), 0.5);
        assert_eq!(l.value_at(25_000, 0.0), -1.0);
        // The edges ramp over EDGE_RAMP samples.
        let mid = l.value_at(10_000 - EDGE_RAMP / 2, 0.0);
        assert!(mid > -1.0 && mid < 0.5, "{mid}");
    }

    #[test]
    fn a_db_lane_adds_to_the_tracks() {
        let track = lane(AutoParam::Volume, &[(0, -6.0), (40_000, -12.0)]);
        let c = clip(10_000, 20_000, lane(AutoParam::Volume, &[(0, 3.0), (19_999, 3.0)]));
        let l = flatten(&AutoParam::Volume, Some(&track), &[c]).unwrap();
        for at in [12_000, 20_000, 29_000] {
            let want = track.value_at(at, 0.0) + 3.0;
            assert!((l.value_at(at, 0.0) - want).abs() < 0.01, "{at}: {} vs {want}", l.value_at(at, 0.0));
        }
        assert_eq!(l.value_at(5_000, 0.0), track.value_at(5_000, 0.0));
        assert!((l.value_at(35_000, 0.0) - track.value_at(35_000, 0.0)).abs() < 1e-4);
    }

    #[test]
    fn a_parameter_only_the_clip_automates_holds_its_ends_and_no_clip_lane_leaves_the_track_alone() {
        let c = clip(1_000, 1_000, lane(AutoParam::Width, &[(0, 0.2), (999, 0.8)]));
        let l = flatten(&AutoParam::Width, None, std::slice::from_ref(&c)).unwrap();
        assert_eq!(l.value_at(0, 1.0), 0.2);
        assert!((l.value_at(5_000, 1.0) - 0.8).abs() < 0.01, "{}", l.value_at(5_000, 1.0));
        assert!(flatten(&AutoParam::Pan(0), None, std::slice::from_ref(&c)).is_none());
        let lanes = vec![lane(AutoParam::Pan(0), &[(0, 0.3)])];
        let p = played(&lanes, &[c]);
        assert_eq!(p.len(), 2, "the track's pan as it is, and the flattened width");
        assert!(played(&lanes, &[]).is_empty());
    }
}

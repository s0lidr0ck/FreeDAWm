//! Clock alignment between a remote recorder and the booth.
//!
//! Each PC's transport counts the frames its audio device rolled since Play (the *take clock*,
//! continuous across loop wraps), read on a network thread with the time since the last block
//! added (the player's `TransportProbe`). Two audio devices never run at exactly the
//! same rate (100 ppm is 0.36 s an hour), so during a take the remote PC pings the booth every
//! few seconds, NTP-style: it notes its own clock when it sends (`t0`) and when the answer comes
//! back (`t3`); the booth answers with its clock when the ping arrived and when it answered
//! (`b1`, `b2`). Assuming the network delay is the same both ways, the booth's clock read
//! `(b1 + b2) / 2` while the remote's read `(t0 + t3) / 2`: one [`SyncPoint`]. The take carries
//! its points and the booth maps the remote's times onto its own clock through them, piecewise
//! linearly ([`ClockMap`]): between two points by the line through them (a gap in the pings,
//! say a dropped connection, is filled from its neighbours), beyond the ends by the overall rate.
//!
//! Both clocks are the *audible* position (what is leaving the speakers now): each side takes its
//! output latency and mix delay off, as local MIDI recording does, so a note lands where the
//! booth heard it.

use serde::{Deserialize, Serialize};

/// The remote's clock (`local`) and the booth's (`booth`) at the same moment, in seconds on each
/// take clock, with the round trip it was measured over.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SyncPoint {
    pub local: f64,
    pub booth: f64,
    pub rtt: f64,
}

/// A point from one ping: sent at `t0` and answered at `t3` (remote clock), received at `b1`
/// and answered at `b2` (booth clock). `None` when the readings make no sense (the answer
/// before the question, a booth that took longer than the round trip, or non-finite numbers).
pub fn sync_point(t0: f64, b1: f64, b2: f64, t3: f64) -> Option<SyncPoint> {
    if ![t0, b1, b2, t3].iter().all(|v| v.is_finite()) || t3 < t0 || b2 < b1 {
        return None;
    }
    let rtt = (t3 - t0) - (b2 - b1);
    (rtt >= 0.0).then_some(SyncPoint { local: (t0 + t3) * 0.5, booth: (b1 + b2) * 0.5, rtt })
}

/// Points measured over a round trip this much longer than the best are dropped (a delay that
/// long is rarely symmetric): the best plus 2 ms, or twice the best, whichever is larger.
fn rtt_limit(best: f64) -> f64 {
    (best + 0.002).max(best * 2.0)
}

/// The most points a take keeps (a ping every 2 s for 24 h is 43 200). Beyond it they are thinned.
pub const MAX_POINTS: usize = 50_000;

/// Remote take-clock seconds to booth take-clock seconds (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct ClockMap {
    /// Filtered points, sorted by `local`, `local` strictly increasing.
    points: Vec<SyncPoint>,
    /// The overall rate (booth seconds per remote second), first point to last.
    slope: f64,
}

impl ClockMap {
    /// The map through `points` (any order; bad and slow ones dropped). `None` without any
    /// usable point.
    pub fn new(points: &[SyncPoint]) -> Option<ClockMap> {
        let usable: Vec<SyncPoint> =
            points.iter().copied().filter(|p| p.local.is_finite() && p.booth.is_finite() && p.rtt.is_finite() && p.rtt >= 0.0).collect();
        let best = usable.iter().map(|p| p.rtt).fold(f64::INFINITY, f64::min);
        if !best.is_finite() {
            return None;
        }
        let limit = rtt_limit(best);
        let mut kept: Vec<SyncPoint> = usable.into_iter().filter(|p| p.rtt <= limit).collect();
        kept.sort_by(|a, b| a.local.total_cmp(&b.local));
        // Points closer than 50 ms apart (a burst at the start) are averaged into one.
        let mut points: Vec<SyncPoint> = Vec::with_capacity(kept.len());
        let mut group: Vec<SyncPoint> = Vec::new();
        let flush = |group: &mut Vec<SyncPoint>, points: &mut Vec<SyncPoint>| {
            if group.is_empty() {
                return;
            }
            let n = group.len() as f64;
            let local = group.iter().map(|p| p.local).sum::<f64>() / n;
            let booth = group.iter().map(|p| p.booth).sum::<f64>() / n;
            let rtt = group.iter().map(|p| p.rtt).fold(f64::INFINITY, f64::min);
            points.push(SyncPoint { local, booth, rtt });
            group.clear();
        };
        for p in kept {
            if group.first().is_some_and(|g| p.local - g.local > 0.05) {
                flush(&mut group, &mut points);
            }
            group.push(p);
        }
        flush(&mut group, &mut points);
        let slope = match (points.first(), points.last()) {
            (Some(a), Some(b)) if b.local - a.local > 1.0 => {
                let s = (b.booth - a.booth) / (b.local - a.local);
                // A real device is within 1 % of its nominal rate; anything else is a bad point.
                if (0.99..=1.01).contains(&s) { s } else { 1.0 }
            }
            _ => 1.0,
        };
        Some(ClockMap { points, slope })
    }

    /// The points the map goes through.
    pub fn points(&self) -> &[SyncPoint] {
        &self.points
    }

    /// The booth's clock when the remote's read `local`.
    pub fn map(&self, local: f64) -> f64 {
        let pts = &self.points;
        let Some(first) = pts.first() else { return local };
        let last = pts.last().unwrap_or(first);
        if local <= first.local {
            return first.booth + (local - first.local) * self.slope;
        }
        if local >= last.local {
            return last.booth + (local - last.local) * self.slope;
        }
        // The segment holding `local`.
        let i = pts.partition_point(|p| p.local <= local);
        match (pts.get(i.saturating_sub(1)), pts.get(i)) {
            (Some(a), Some(b)) if b.local > a.local => a.booth + (local - a.local) * (b.booth - a.booth) / (b.local - a.local),
            (Some(a), _) => a.booth + (local - a.local) * self.slope,
            _ => local,
        }
    }
}

impl ClockMap {
    /// The remote's clock when the booth's read `booth`: the inverse of [`ClockMap::map`] (the
    /// map rises everywhere: points with `booth` not rising are passed over by the line through
    /// their neighbours).
    pub fn unmap(&self, booth: f64) -> f64 {
        let pts = &self.points;
        let slope = if self.slope > 0.0 { self.slope } else { 1.0 };
        let Some(first) = pts.first() else { return booth };
        let last = pts.last().unwrap_or(first);
        if booth <= first.booth {
            return first.local + (booth - first.booth) / slope;
        }
        if booth >= last.booth {
            return last.local + (booth - last.booth) / slope;
        }
        let i = pts.partition_point(|p| p.booth <= booth);
        match (pts.get(i.saturating_sub(1)), pts.get(i)) {
            (Some(a), Some(b)) if b.booth > a.booth => a.local + (booth - a.booth) * (b.local - a.local) / (b.booth - a.booth),
            (Some(a), _) => a.local + (booth - a.booth) / slope,
            _ => booth,
        }
    }

    /// The overall rate, booth seconds per remote second.
    pub fn slope(&self) -> f64 {
        self.slope
    }

    /// A map through `points` taken as they are (already measured exactly: no filtering): the
    /// mix stream's points, (remote clock, booth timeline) at the sample the performer heard.
    pub fn exact(points: &[SyncPoint]) -> Option<ClockMap> {
        let mut points: Vec<SyncPoint> = points.iter().copied().filter(|p| p.local.is_finite() && p.booth.is_finite()).collect();
        points.sort_by(|a, b| a.local.total_cmp(&b.local));
        points.dedup_by(|b, a| b.local - a.local < 1e-6);
        let slope = match (points.first(), points.last()) {
            (Some(a), Some(b)) if b.local - a.local > 1.0 => {
                let s = (b.booth - a.booth) / (b.local - a.local);
                if (0.99..=1.01).contains(&s) { s } else { 1.0 }
            }
            _ => 1.0,
        };
        (!points.is_empty()).then_some(ClockMap { points, slope })
    }
}

/// Keeps a take's points under [`MAX_POINTS`]: every other one goes when it is full.
pub fn push_point(points: &mut Vec<SyncPoint>, p: SyncPoint) {
    if points.len() >= MAX_POINTS {
        let mut k = 0;
        points.retain(|_| {
            k += 1;
            k % 2 == 0
        });
    }
    points.push(p);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ping_gives_the_midpoints_and_the_round_trip() {
        let p = sync_point(10.0, 20.001, 20.0015, 10.004).unwrap();
        assert!((p.local - 10.002).abs() < 1e-12);
        assert!((p.booth - 20.00125).abs() < 1e-12);
        assert!((p.rtt - 0.0035).abs() < 1e-12);
        assert!(sync_point(10.0, 1.0, 1.0, 9.0).is_none(), "answer before the question");
        assert!(sync_point(10.0, 1.0, 2.0, 10.5).is_none(), "booth slower than the round trip");
        assert!(sync_point(f64::NAN, 1.0, 1.0, 2.0).is_none());
    }

    #[test]
    fn the_map_follows_drift_and_fills_gaps() {
        // Booth = 5 + local * (1 + 100 ppm).
        let rate = 1.0001;
        let pts: Vec<SyncPoint> = (0..20)
            .filter(|i| !(8..14).contains(i))
            .map(|i| f64::from(i) * 2.0)
            .map(|l| SyncPoint { local: l, booth: 5.0 + l * rate, rtt: 0.001 })
            .collect();
        let m = ClockMap::new(&pts).unwrap();
        for l in [0.0, 1.0, 17.3, 25.0, 38.0, 100.0, -3.0] {
            assert!((m.map(l) - (5.0 + l * rate)).abs() < 1e-9, "{l}: {} vs {}", m.map(l), 5.0 + l * rate);
        }
    }

    #[test]
    fn slow_round_trips_are_left_out_and_bursts_averaged() {
        let mut pts = vec![SyncPoint { local: 1.0, booth: 2.0, rtt: 0.001 }, SyncPoint { local: 1.01, booth: 2.012, rtt: 0.001 }];
        pts.push(SyncPoint { local: 5.0, booth: 7.0, rtt: 0.2 });
        pts.push(SyncPoint { local: 9.0, booth: 10.0, rtt: 0.002 });
        let m = ClockMap::new(&pts).unwrap();
        assert_eq!(m.points().len(), 2);
        assert!((m.points()[0].local - 1.005).abs() < 1e-12);
        assert!((m.points()[0].booth - 2.006).abs() < 1e-12);
        assert!(ClockMap::new(&[]).is_none());
        let one = ClockMap::new(&[SyncPoint { local: 3.0, booth: 4.5, rtt: 0.0 }]).unwrap();
        assert!((one.map(10.0) - 11.5).abs() < 1e-12, "one point: an offset");
    }

    #[test]
    fn unmap_inverts_map() {
        let rate = 1.0002;
        let pts: Vec<SyncPoint> = (0..10).map(|i| f64::from(i) * 3.0).map(|l| SyncPoint { local: l, booth: 2.0 + l * rate, rtt: 0.001 }).collect();
        let m = ClockMap::new(&pts).unwrap();
        for l in [-5.0, 0.0, 1.3, 14.9, 27.0, 60.0] {
            assert!((m.unmap(m.map(l)) - l).abs() < 1e-9, "{l}");
        }
        let e = ClockMap::exact(&[SyncPoint { local: 1.0, booth: 11.0, rtt: 0.0 }]).unwrap();
        assert!((e.unmap(12.5) - 2.5).abs() < 1e-12);
    }

    #[test]
    fn the_point_list_is_thinned_when_full() {
        let mut v: Vec<SyncPoint> = (0..MAX_POINTS).map(|i| SyncPoint { local: i as f64, booth: i as f64, rtt: 0.0 }).collect();
        push_point(&mut v, SyncPoint { local: 1e9, booth: 1e9, rtt: 0.0 });
        assert_eq!(v.len(), MAX_POINTS / 2 + 1);
    }
}

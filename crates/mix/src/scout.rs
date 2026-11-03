//! Scout taps: a PRE and a POST measurement point on every strip and the master.
//!
//! A [`ScoutRig`] is built off the audio thread with everything a capture needs (one
//! [`StripTaps`] per strip, each with two preallocated [`ScoutMeter`]s), armed with
//! [`MixEngine::arm_scout`](crate::MixEngine::arm_scout), and handed back whole with
//! [`MixEngine::scout_done`](crate::MixEngine::scout_done) once the capture has ended. Nothing in
//! between allocates, locks or panics, and nothing of the rig is dropped on the audio thread: a
//! strip deleted mid-capture parks its taps in the rig's preallocated `orphans`.
//!
//! - PRE sits after the strip's source (clips, clip effects, input monitoring, the instrument, an
//!   aux's bus input), trim and phase invert, immediately before the first insert.
//! - POST sits after the last insert, before the pre-fader send copy, the fader and the pan.
//! - Both taps are before the strip's delay-compensation line (`Strip::align`), so they are not
//!   PDC-aligned across strips: POST runs late by the strip's own insert latency, as the inserts
//!   put it out. PRE and POST of one strip cover the same timeline frames.
//! - Strips are measured on L/R; a mono strip feeds L = R and a strip of more than two channels
//!   is folded with ITU-R BS.775 gains ([`itu_stereo_fold`]).
//! - Each strip owns its taps (`Strip::scout`), so strips rendering in parallel never share
//!   mutable state. The master's taps stay in the rig (the master renders inline).

use crate::{MAX_CHANNELS, itu_stereo_fold, main_channels, strip_channels, strip_format};
use soundcraft_dsp::scout::{MAX_CAPTURE_SECONDS, Metrics, ScoutMeter};
use soundcraft_model::{AutomatedParam, Route, ScoutMetrics, ScoutStrip, Session, Track, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Played ranges a capture records before merging further jumps into the last one.
const PLAYED_CAP: usize = 64;

/// Frames folded to stereo at a time (the fold scratch length).
const FOLD_CHUNK: usize = 256;

/// The note on a strip whose track left the session while it was being captured.
pub const REMOVED_NOTE: &str = "track removed during capture";

/// The note on a strip whose track was inactive (or a VCA, which never renders) for some of the
/// capture: its taps were not fed then.
pub const INACTIVE_NOTE: &str = "track inactive during capture";

/// The note on a strip whose track was not in the session when the capture was armed.
pub const MISSING_NOTE: &str = "track not found";

/// One strip's PRE and POST taps, with what it needs to feed them without allocating.
pub struct StripTaps {
    pub(crate) track: TrackId,
    pre: ScoutMeter,
    post: ScoutMeter,
    /// Fold-down gains (to L, to R) per channel for strips of more than two channels.
    fold: [(f32, f32); MAX_CHANNELS],
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
    /// Its track left the session during the capture.
    removed: bool,
    /// Its track was inactive (or a VCA) during a captured block.
    inactive: bool,
    /// The strip as it stood at capture start (name, kind, roles, mixer); metrics filled at finish.
    info: ScoutStrip,
}

impl StripTaps {
    /// Feeds PRE with `win` = (offset, frames) of `buf`. Audio-thread safe.
    pub(crate) fn feed_pre(&mut self, buf: &[Vec<f32>], win: (usize, usize)) {
        feed(&mut self.pre, &self.fold, &mut self.scratch_l, &mut self.scratch_r, buf, win);
    }

    /// Feeds POST with `win` = (offset, frames) of `buf`. Audio-thread safe.
    pub(crate) fn feed_post(&mut self, buf: &[Vec<f32>], win: (usize, usize)) {
        feed(&mut self.post, &self.fold, &mut self.scratch_l, &mut self.scratch_r, buf, win);
    }

    /// Notes that its track did not render (inactive or a VCA) during a captured block.
    pub(crate) fn mark_inactive(&mut self) {
        self.inactive = true;
    }

    pub(crate) fn heap_blocks(&self, out: &mut Vec<(usize, usize)>) {
        out.push((self as *const StripTaps as usize, 0));
        out.push((self.scratch_l.as_ptr() as usize, self.scratch_l.capacity()));
        out.push((self.scratch_r.as_ptr() as usize, self.scratch_r.capacity()));
        out.extend(self.pre.heap_blocks());
        out.extend(self.post.heap_blocks());
    }
}

/// Feeds `m` with frames `off..off + n` of `buf` as L/R (mono: L = R; wider: folded).
fn feed(m: &mut ScoutMeter, fold: &[(f32, f32); MAX_CHANNELS], sl: &mut [f32], sr: &mut [f32], buf: &[Vec<f32>], (off, n): (usize, usize)) {
    let end = off.saturating_add(n);
    match buf {
        [] => {}
        [mono] => {
            if let Some(x) = mono.get(off..end) {
                m.feed(x, x);
            }
        }
        [l, r] => {
            if let (Some(a), Some(b)) = (l.get(off..end), r.get(off..end)) {
                m.feed(a, b);
            }
        }
        _ => {
            let chunk = sl.len().min(sr.len());
            if chunk == 0 {
                return;
            }
            let mut at = off;
            while at < end {
                let k = chunk.min(end - at);
                let (Some(dl), Some(dr)) = (sl.get_mut(..k), sr.get_mut(..k)) else { return };
                dl.fill(0.0);
                dr.fill(0.0);
                for (ch, &(gl, gr)) in buf.iter().zip(fold.iter()) {
                    let Some(x) = ch.get(at..at + k) else { continue };
                    for ((l, r), &v) in dl.iter_mut().zip(dr.iter_mut()).zip(x) {
                        *l += gl * v;
                        *r += gr * v;
                    }
                }
                m.feed(dl, dr);
                at += k;
            }
        }
    }
}

/// A capture: the taps of every strip it measures, its bounds and its progress.
pub struct ScoutRig {
    /// Taps not placed in a strip (the master's, and any whose strip does not exist), and the
    /// slots placed taps come back to. Never grows after `new`.
    taps: Vec<Option<Box<StripTaps>>>,
    /// Taps of strips whose track left the session mid-capture (capacity: the strip count).
    orphans: Vec<Box<StripTaps>>,
    /// The strips in the order they were asked for.
    order: Vec<TrackId>,
    start: Option<Samples>,
    end: Option<Samples>,
    /// `MAX_CAPTURE_SECONDS * sample_rate` frames.
    budget: u64,
    captured: u64,
    progress: Arc<AtomicU64>,
    played: Vec<Range>,
    /// `played` ran out of room and its last entry was widened to cover later ranges.
    played_truncated: bool,
    /// The taps were handed to their strips.
    pub(crate) placed: bool,
    stopped: bool,
    ended: bool,
    /// The host's capture id (`scout.capture`), so a rig coming back is stored under it.
    capture: u32,
}

impl ScoutRig {
    /// Builds a rig for `strips` (tracks of `s`, the master included), capturing from `start` (or
    /// wherever the transport is) to `end` (or for [`MAX_CAPTURE_SECONDS`]). Allocates: call off
    /// the audio thread. Each strip's mixer is snapshotted now.
    pub fn new(s: &Session, strips: &[TrackId], start: Option<Samples>, end: Option<Samples>) -> ScoutRig {
        let sr = s.sample_rate.as_f64();
        let main_ch = main_channels(s);
        let mut order: Vec<TrackId> = Vec::with_capacity(strips.len());
        for t in strips {
            if !order.contains(t) {
                order.push(*t);
            }
        }
        let taps = order
            .iter()
            .map(|&id| {
                let mut fold = [(0.0, 0.0); MAX_CHANNELS];
                let info = match s.track(id) {
                    Some(t) => {
                        let ch = strip_channels(t, main_ch);
                        for (g, sp) in fold.iter_mut().zip(strip_format(t, ch).speakers().iter()) {
                            *g = itu_stereo_fold(*sp);
                        }
                        let (channel_type, role) = channel_type(s, t);
                        ScoutStrip {
                            track: id,
                            name: t.name.clone(),
                            kind: t.kind,
                            channel_type: channel_type.into(),
                            role: role.into(),
                            channels: u8::try_from(ch).unwrap_or(u8::MAX),
                            mixer: t.mixer.clone(),
                            automated: Vec::new(),
                            base: ScoutMetrics::default(),
                            post: ScoutMetrics::default(),
                            note: None,
                            target: None,
                        }
                    }
                    None => ScoutStrip {
                        track: id,
                        name: String::new(),
                        kind: TrackKind::default(),
                        channel_type: String::new(),
                        role: String::new(),
                        channels: 0,
                        mixer: soundcraft_model::Mixer::new(soundcraft_model::ChannelFormat::Stereo),
                        automated: Vec::new(),
                        base: ScoutMetrics::default(),
                        post: ScoutMetrics::default(),
                        note: Some(MISSING_NOTE.into()),
                        target: None,
                    },
                };
                Some(Box::new(StripTaps {
                    track: id,
                    pre: ScoutMeter::new(sr),
                    post: ScoutMeter::new(sr),
                    fold,
                    scratch_l: vec![0.0; FOLD_CHUNK],
                    scratch_r: vec![0.0; FOLD_CHUNK],
                    removed: false,
                    inactive: false,
                    info,
                }))
            })
            .collect();
        let budget = (f64::from(MAX_CAPTURE_SECONDS) * sr).round().max(0.0) as u64;
        ScoutRig {
            taps,
            orphans: Vec::with_capacity(order.len()),
            order,
            start,
            end,
            budget,
            captured: 0,
            progress: Arc::new(AtomicU64::new(0)),
            played: Vec::with_capacity(PLAYED_CAP),
            played_truncated: false,
            placed: false,
            stopped: false,
            ended: false,
            capture: 0,
        }
    }

    /// Tags the rig with the host's capture id (see [`ScoutRig::capture`]).
    pub fn with_capture(mut self, id: u32) -> ScoutRig {
        self.capture = id;
        self
    }

    /// The capture id given with [`ScoutRig::with_capture`] (0 if none).
    pub fn capture(&self) -> u32 {
        self.capture
    }

    /// The capture's bounds (a loop capture's start and end), as built.
    pub fn bounds(&self) -> (Option<Samples>, Option<Samples>) {
        (self.start, self.end)
    }

    /// Frames captured so far, updated every block (for a progress display).
    pub fn progress(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.progress)
    }

    /// Frames captured.
    pub fn frames(&self) -> u64 {
        self.captured
    }

    /// The timeline ranges played during the capture, in order (empty while stopped).
    pub fn played(&self) -> &[Range] {
        &self.played
    }

    /// More ranges were played than `played` holds (64): its last entry was widened to span
    /// every later one, so the list is no longer exact.
    pub fn played_truncated(&self) -> bool {
        self.played_truncated
    }

    /// The capture has ended: its budget is full, `end` was reached, or it was stopped.
    pub fn finished(&self) -> bool {
        self.stopped || self.ended || self.captured >= self.budget
    }

    /// Ends the capture. An armed rig is ended through `MixEngine::stop_scout`; this is for one
    /// never armed (a host without an audio engine ends its capture here).
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    /// The part of the block `pos..pos + frames` to capture, as (offset, frames): inside
    /// `[start, end)` and the remaining budget. `None`: nothing this block. A bounded capture
    /// (`start` or `end` set: a loop) feeds only while `playing`, and begins only in the block
    /// that contains `start`, so it is exactly that range however the transport got there.
    pub(crate) fn window(&self, pos: Samples, frames: usize, playing: bool) -> Option<(usize, usize)> {
        if self.finished() {
            return None;
        }
        if self.start.is_some() || self.end.is_some() {
            if !playing {
                return None;
            }
            if let Some(s) = self.start
                && self.captured == 0
                && !(pos <= s && s < pos.saturating_add(frames as i64))
            {
                return None;
            }
        }
        let lo = self.start.map_or(pos, |s| s.max(pos));
        let mut hi = pos.saturating_add(frames as i64);
        if let Some(e) = self.end {
            hi = hi.min(e);
        }
        let left = i64::try_from(self.budget.saturating_sub(self.captured)).unwrap_or(i64::MAX);
        hi = hi.min(lo.saturating_add(left));
        if hi <= lo { None } else { Some((usize::try_from(lo - pos).ok()?, usize::try_from(hi - lo).ok()?)) }
    }

    /// Books a rendered block: the frames captured, the played range (unless the transport is
    /// stopped: nothing on the timeline played), and whether `end` was reached. No allocation.
    pub(crate) fn advance(&mut self, pos: Samples, frames: usize, win: Option<(usize, usize)>, playing: bool) {
        if let Some((off, n)) = win {
            self.captured = self.captured.saturating_add(n as u64);
            self.progress.store(self.captured, Ordering::Relaxed);
            let at = pos.saturating_add(off as i64);
            let r = Range { start: at, end: at.saturating_add(n as i64) };
            if playing {
                let full = self.played.len() >= self.played.capacity();
                match self.played.last_mut() {
                    Some(last) if last.end == r.start => last.end = r.end,
                    // Out of room (a capture that jumped 64 times): widen the last entry to span
                    // this range too (rather than allocate on the audio thread), and say so.
                    Some(last) if full => {
                        last.start = last.start.min(r.start);
                        last.end = last.end.max(r.end);
                        self.played_truncated = true;
                    }
                    _ => self.played.push(r),
                }
            }
        }
        if let Some(e) = self.end {
            let block_end = pos.saturating_add(frames as i64);
            if self.captured > 0 && block_end >= e {
                self.ended = true;
            }
        }
    }

    /// The master's taps (they live in the rig).
    pub(crate) fn taps_for(&mut self, t: TrackId) -> Option<&mut StripTaps> {
        self.taps.iter_mut().flatten().find(|x| x.track == t).map(|b| &mut **b)
    }

    /// Takes the taps of `t` out of the rig (to place them in its strip).
    pub(crate) fn take_taps(&mut self, t: TrackId) -> Option<Box<StripTaps>> {
        self.taps.iter_mut().find(|x| x.as_ref().is_some_and(|x| x.track == t)).and_then(Option::take)
    }

    /// Takes back taps from a strip, into a free slot (there is always one). No allocation.
    pub(crate) fn put_back(&mut self, taps: Box<StripTaps>) {
        let mut taps = Some(taps);
        if let Some(slot) = self.taps.iter_mut().find(|x| x.is_none()) {
            *slot = taps.take();
        }
        // No free slot cannot happen (each placed tap left one); park it rather than drop it.
        if let Some(t) = taps
            && self.orphans.len() < self.orphans.capacity()
        {
            self.orphans.push(t);
        }
    }

    /// Takes in the taps of a strip whose track left the session. A capture that had already
    /// ended keeps its data valid. No allocation.
    pub(crate) fn orphan(&mut self, mut taps: Box<StripTaps>) {
        taps.removed |= !self.finished();
        if self.orphans.len() < self.orphans.capacity() {
            self.orphans.push(taps);
        } else {
            self.put_back(taps);
        }
    }

    /// Marks the taps of `t` still held in the rig (the master's) as removed, unless the capture
    /// had already ended. No allocation.
    pub(crate) fn mark_removed(&mut self, t: TrackId) {
        if self.finished() {
            return;
        }
        for x in self.taps.iter_mut().flatten().filter(|x| x.track == t) {
            x.removed = true;
        }
    }

    /// Marks every tap still held in the rig whose track is not in `s` as removed (a track
    /// deleted between arming and the first render). Taps of a track that was already missing
    /// when the rig was built keep their own note. No allocation.
    pub(crate) fn mark_absent(&mut self, s: &Session) {
        for x in self.taps.iter_mut().flatten() {
            if x.info.note.is_none() && s.track(x.track).is_none() {
                x.removed = true;
            }
        }
    }

    /// Address and capacity of every heap block the rig and its taps own (wherever they sit).
    pub(crate) fn heap_blocks(&self, out: &mut Vec<(usize, usize)>) {
        out.push((self.taps.as_ptr() as usize, self.taps.capacity()));
        out.push((self.orphans.as_ptr() as usize, self.orphans.capacity()));
        out.push((self.played.as_ptr() as usize, self.played.capacity()));
        for t in self.taps.iter().flatten().chain(self.orphans.iter()) {
            t.heap_blocks(out);
        }
    }

    /// The finished strips, in the order they were asked for. Runs off the audio thread: computes
    /// the metrics, and lists the insert params automation moved over the played range. A strip
    /// whose track left the session mid-capture is reported invalid, with a note.
    pub fn finish(self, s: &Session) -> Vec<ScoutStrip> {
        let span = match (self.played.first(), self.played.last()) {
            (Some(a), Some(b)) => Some(Range { start: a.start, end: b.end }),
            _ => None,
        };
        let all: Vec<Box<StripTaps>> = self.taps.into_iter().flatten().chain(self.orphans).collect();
        self.order
            .iter()
            .filter_map(|id| all.iter().find(|t| t.track == *id))
            .map(|t| {
                let mut out = t.info.clone();
                out.base = to_model(&t.pre.finish());
                out.post = to_model(&t.post.finish());
                if t.removed {
                    out.base.valid = false;
                    out.post.valid = false;
                    out.note = Some(REMOVED_NOTE.into());
                } else {
                    if t.inactive && out.note.is_none() {
                        out.note = Some(INACTIVE_NOTE.into());
                    }
                    if let Some(r) = span.filter(|_| s.track(t.track).is_some()) {
                        out.automated = automated_params(s, t.track, r);
                    }
                }
                out
            })
            .collect()
    }
}

/// Scout2's channel type and role of a track: an Aux fed only by sends is an "fx_bus"; an Aux fed
/// by any track's output route, or a Folder, is a "mix_bus"; the master is "master"; everything
/// else is a "channel".
pub fn channel_type(s: &Session, t: &Track) -> (&'static str, &'static str) {
    match t.kind {
        TrackKind::Master => ("master", "Master"),
        TrackKind::Folder => ("mix_bus", "Bus"),
        TrackKind::Aux => {
            let fed_by_output = match &t.mixer.input {
                Route::Bus(b) => s.tracks.iter().any(|o| o.id != t.id && o.mixer.output == Route::Bus(*b)),
                _ => false,
            };
            if fed_by_output { ("mix_bus", "Bus") } else { ("fx_bus", "Bus") }
        }
        _ => ("channel", "Channel"),
    }
}

/// Every laned insert param of `track`, evaluated at `range.start` and `range.end` the way the
/// render does (`crate::plugin_lane`), listed when the two values differ.
pub fn automated_params(s: &Session, track: TrackId, range: Range) -> Vec<AutomatedParam> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let mut out = Vec::new();
    for ins in &t.mixer.inserts {
        for (k, &v) in &ins.params {
            let Some(l) = crate::plugin_lane(t, ins.id, k) else { continue };
            let (a, b) = (l.value_at(range.start, v), l.value_at(range.end, v));
            if a.to_bits() != b.to_bits() {
                out.push(AutomatedParam { module: ins.id, param: k.clone(), start: a, end: b });
            }
        }
    }
    out
}

/// `soundcraft_dsp::scout::Metrics` as the session stores it.
pub fn to_model(m: &Metrics) -> ScoutMetrics {
    ScoutMetrics {
        valid: m.valid,
        duration_sec: m.duration_sec,
        sample_rate: m.sample_rate,
        activity_ratio: m.activity_ratio,
        clipped_samples: m.clipped_samples,
        peak_hold_dbfs: m.peak_hold_dbfs,
        noise_floor_dbfs: m.noise_floor_dbfs,
        peak_dbfs: m.peak_dbfs,
        true_peak_dbtp: m.true_peak_dbtp,
        rms_dbfs: m.rms_dbfs,
        lufs_integrated: m.lufs_integrated,
        lra: m.lra,
        crest_factor_db: m.crest_factor_db,
        dynamic_range_db: m.dynamic_range_db,
        peak_to_lufs_db: m.peak_to_lufs_db,
        transient_density: m.transient_density,
        spectrum31: m.spectrum31,
        zone_sub: m.zone_sub,
        zone_low: m.zone_low,
        zone_low_mid: m.zone_low_mid,
        zone_mid: m.zone_mid,
        zone_high_mid: m.zone_high_mid,
        zone_high: m.zone_high,
        zone_air: m.zone_air,
        stereo_correlation: m.stereo_correlation,
        stereo_width: m.stereo_width,
        low_freq_correlation: m.low_freq_correlation,
        lr_balance_db: m.lr_balance_db,
        dc_offset: m.dc_offset,
        spectral_centroid_hz: m.spectral_centroid_hz,
        mid_side_ratio_db: m.mid_side_ratio_db,
        side_low_ratio_db: m.side_low_ratio_db,
        noise_floor_p10_dbfs: m.noise_floor_p10_dbfs,
        lufs_p10: m.lufs_p10,
        lufs_p50: m.lufs_p50,
        lufs_p90: m.lufs_p90,
        lufs_momentary_max: m.lufs_momentary_max,
        lufs_short_term_max: m.lufs_short_term_max,
    }
}

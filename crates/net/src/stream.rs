//! The remote mix stream (spec `docs/superpowers/specs/2026-11-02-remote-audio-recording-design.md`,
//! sections 2 and 3): the host's main mix, live, to a remote performer, so he hears what the host
//! hears; and the map from what he heard to the host's timeline, so his take lands where he
//! played it relative to what he heard, however late the network delivered it.
//!
//! - [`MixEncoder`] (host, a network thread): reads the player's tap
//!   (`soundcraft_playback::stream::MixTap`), converts to 48 kHz, cuts 20 ms frames and encodes
//!   them with Opus (stereo, 256 kb/s). Each [`StreamPacket`] carries the host's take clock
//!   (frames rolled since Play, in seconds) and timeline position of its first sample, and an
//!   epoch that changes whenever the host's take clock jumps (Stop, Play, a locate), so the
//!   performer drops what he has queued and starts again at once.
//! - [`MixDecoder`] (performer, a network thread): a jitter buffer (starts at 150 ms, grows by
//!   30 ms when it runs dry, up to 500 ms; packets put back in order, a lost one concealed by
//!   Opus), the decoded audio converted to the performer's rate and queued for his audio thread
//!   (`soundcraft_playback::stream::StreamIn`), which plays it in order and marks each block
//!   with the stream frames played before it and his own take clock. [`MixDecoder::heard`]
//!   turns a mark into a point (his take clock, the host's take clock of the sample he heard
//!   then): the take's `heard` map ([`crate::take::AudioTake::heard`]).
//!
//! Opus delays what it decodes by [`CODEC_DELAY`] samples (its lookahead): taken off. Clock
//! drift between the PCs moves the queue: frames are dropped when it runs over the target by
//! 100 ms (and the map starts a new segment there), silence plays when it runs dry (not counted:
//! the map is by frames played, so it stays exact).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

use soundcraft_playback::stream::{HeardMark, StreamIn, TapBlock};

use crate::sync::SyncPoint;

/// The stream's rate, frame and bitrate.
pub const RATE: u32 = 48_000;
pub const FRAME: usize = 960;
pub const BITRATE: i32 = 256_000;
/// Samples (at 48 kHz) Opus delays its output by (2.5 ms; measured by a test).
pub const CODEC_DELAY: i64 = 120;
/// The jitter buffer: where it starts, its steps and its limits (ms).
pub const START_MS: f64 = 150.0;
pub const GROW_MS: f64 = 30.0;
pub const MAX_MS: f64 = 500.0;
pub const MIN_MS: f64 = 80.0;

/// One 20 ms Opus frame of the host's mix (`{"event": "stream", "params": …}` on a follower's
/// link).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamPacket {
    pub seq: u64,
    pub epoch: u64,
    /// The host's take clock (s) and timeline position (s) at the packet's first input sample.
    pub rolled: f64,
    pub pos: f64,
    /// Opus, base64.
    pub data: String,
}

/// A streaming rate converter for monitoring (cubic Hermite): good enough to listen to, steady
/// across calls.
#[derive(Debug, Clone)]
struct Resampler {
    step: f64,
    phase: f64,
    hist: VecDeque<[f32; 2]>,
    /// The first input frame went in twice (as the one before it), so output frame `m` sits
    /// exactly at input frame `m * step`.
    primed: bool,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Resampler {
        Resampler { step: f64::from(from.max(1)) / f64::from(to.max(1)), phase: 0.0, hist: VecDeque::new(), primed: false }
    }

    fn passthrough(&self) -> bool {
        (self.step - 1.0).abs() < 1e-12
    }

    /// Interleaved stereo in, interleaved stereo out.
    fn run(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough() {
            out.extend_from_slice(input);
            return;
        }
        for f in input.chunks_exact(2) {
            if !self.primed {
                self.primed = true;
                self.hist.push_back([f[0], f[1]]);
            }
            self.hist.push_back([f[0], f[1]]);
        }
        // Output sample at input position `phase` (relative to hist[1]) needs hist[0..4].
        while self.hist.len() >= 4 {
            while self.phase < 1.0 {
                let t = self.phase as f32;
                for c in 0..2 {
                    let p = |i: usize| self.hist.get(i).map_or(0.0, |s| s[c]);
                    let (y0, y1, y2, y3) = (p(0), p(1), p(2), p(3));
                    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
                    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
                    let cc = -0.5 * y0 + 0.5 * y2;
                    out.push(((a * t + b) * t + cc) * t + y1);
                }
                self.phase += self.step;
            }
            self.phase -= 1.0;
            self.hist.pop_front();
        }
    }
}

/// The host's side (see the module docs).
pub struct MixEncoder {
    rate: u32,
    enc: Option<rusty_opus::OpusEncoder>,
    rs: Resampler,
    epoch: u64,
    seq: u64,
    /// The next take-clock frame expected (a jump starts a new epoch), the epoch's first one and
    /// its timeline position.
    next: Option<i64>,
    start_rolled: i64,
    start_pos: i64,
    /// 48 kHz frames out so far this epoch, and waiting to be encoded.
    out_frames: u64,
    pending: Vec<f32>,
}

impl MixEncoder {
    /// An encoder for a session at `rate`.
    pub fn new(rate: u32) -> MixEncoder {
        MixEncoder {
            rate: rate.max(1),
            enc: None,
            rs: Resampler::new(rate, RATE),
            epoch: 0,
            seq: 0,
            next: None,
            start_rolled: 0,
            start_pos: 0,
            out_frames: 0,
            pending: Vec::new(),
        }
    }

    fn encoder(&mut self) -> Option<&mut rusty_opus::OpusEncoder> {
        if self.enc.is_none() {
            match rusty_opus::OpusEncoder::new(RATE as i32, 2, rusty_opus::Application::Audio) {
                Ok(mut e) => {
                    e.bitrate_bps = BITRATE;
                    self.enc = Some(e);
                }
                Err(e) => log::warn!("mix stream: no encoder: {e:?}"),
            }
        }
        self.enc.as_mut()
    }

    /// The tap's blocks in, the packets out.
    pub fn feed(&mut self, blocks: &[TapBlock]) -> Vec<StreamPacket> {
        let mut out = Vec::new();
        for b in blocks {
            if !b.rolling {
                // Stopped: the next roll is a new epoch.
                self.next = None;
                continue;
            }
            if self.next != Some(b.rolled) {
                self.epoch += 1;
                self.start_rolled = b.rolled;
                self.start_pos = b.position;
                self.out_frames = 0;
                self.pending.clear();
                self.rs = Resampler::new(self.rate, RATE);
                // A fresh encoder: its delay restarts with the epoch.
                self.enc = None;
            }
            self.next = Some(b.rolled.saturating_add((b.samples.len() / 2) as i64));
            let mut conv = Vec::with_capacity(b.samples.len() + 8);
            self.rs.run(&b.samples, &mut conv);
            self.pending.extend_from_slice(&conv);
            while self.pending.len() >= FRAME * 2 {
                let frame: Vec<f32> = self.pending.drain(..FRAME * 2).collect();
                let first = self.out_frames;
                self.out_frames += FRAME as u64;
                let mut buf = [0u8; 4000];
                let encoded = self.encoder().map(|e| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| e.encode(&frame, FRAME, &mut buf))));
                let len = match encoded {
                    Some(Ok(Ok(n))) => n,
                    _ => {
                        self.enc = None;
                        continue;
                    }
                };
                let at = first as f64 / f64::from(RATE);
                let r = f64::from(self.rate);
                self.seq += 1;
                out.push(StreamPacket {
                    seq: self.seq,
                    epoch: self.epoch,
                    rolled: self.start_rolled as f64 / r + at,
                    pos: self.start_pos as f64 / r + at,
                    data: crate::audio::b64_encode(buf.get(..len).unwrap_or(&[])),
                });
            }
        }
        out
    }
}

/// A run of stream frames queued for the performer: from frame `index` on (his frames played),
/// at the host's take clock `rolled` (s), one frame each `1 / rate`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    index: u64,
    rolled: f64,
    pos: f64,
}

/// The performer's side (see the module docs).
pub struct MixDecoder {
    rate: u32,
    dec: Option<rusty_opus::OpusDecoder>,
    rs: Resampler,
    epoch: Option<u64>,
    /// Packets waiting, by sequence; the next one to decode.
    held: BTreeMap<u64, StreamPacket>,
    next_seq: Option<u64>,
    /// Frames of this epoch decoded (48 kHz) and out of the rate converter (the performer's
    /// rate), and the epoch's first packet's place.
    decoded: u64,
    converted: u64,
    epoch_rolled: f64,
    epoch_pos: f64,
    started: bool,
    /// Frames pushed to the performer's queue so far, the map of what they are, and the target.
    pushed: u64,
    segments: VecDeque<Segment>,
    pub target_ms: f64,
    starved_seen: u64,
    calm_since: u64,
    /// Packets lost (concealed) and late (dropped).
    pub lost: u64,
    pub late: u64,
    pub packets: u64,
    /// The epoch's next decoded frame's index, accounting for frames dropped (drift).
    skip_next: bool,
}

impl MixDecoder {
    /// A decoder for a performer whose session runs at `rate`.
    pub fn new(rate: u32) -> MixDecoder {
        MixDecoder {
            rate: rate.max(1),
            dec: None,
            rs: Resampler::new(RATE, rate),
            epoch: None,
            held: BTreeMap::new(),
            next_seq: None,
            decoded: 0,
            converted: 0,
            epoch_rolled: 0.0,
            epoch_pos: 0.0,
            started: false,
            pushed: 0,
            segments: VecDeque::new(),
            target_ms: START_MS,
            starved_seen: 0,
            calm_since: 0,
            lost: 0,
            late: 0,
            packets: 0,
            skip_next: false,
        }
    }

    /// A packet arrived. A new epoch drops what was queued (`queue.flush()`).
    pub fn on_packet(&mut self, p: StreamPacket, queue: &StreamIn) {
        self.packets += 1;
        if self.epoch.is_none_or(|e| p.epoch > e) {
            self.epoch = Some(p.epoch);
            self.held.clear();
            self.next_seq = Some(p.seq);
            self.decoded = 0;
            self.converted = 0;
            self.epoch_rolled = p.rolled;
            self.epoch_pos = p.pos;
            self.started = false;
            self.dec = None;
            self.rs = Resampler::new(RATE, self.rate);
            // Everything queued is from before: dropped (the audio thread counts it as played, so
            // frame `pushed` is still the next one it plays).
            queue.flush();
        }
        if Some(p.epoch) != self.epoch {
            return;
        }
        if self.next_seq.is_some_and(|n| p.seq < n) {
            self.late += 1;
            return;
        }
        if self.held.len() < 500 {
            self.held.insert(p.seq, p);
        }
    }

    fn decoder(&mut self) -> Option<&mut rusty_opus::OpusDecoder> {
        if self.dec.is_none() {
            self.dec = rusty_opus::OpusDecoder::new(RATE as i32, 2).ok();
        }
        self.dec.as_mut()
    }

    /// Decodes one frame (`None`: lost, concealed) into interleaved 48 kHz stereo.
    fn decode(&mut self, data: Option<&[u8]>) -> Vec<f32> {
        let mut pcm = vec![0f32; FRAME * 2];
        let input = data.unwrap_or(&[]);
        let ok = self.decoder().map(|d| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.decode(input, FRAME, &mut pcm))));
        match ok {
            Some(Ok(Ok(n))) => pcm.truncate(n.min(FRAME) * 2),
            _ => {
                self.dec = None;
                pcm.iter_mut().for_each(|x| *x = 0.0);
            }
        }
        pcm
    }

    fn queued_ms(&self, queue: &StreamIn) -> f64 {
        queue.queued() as f64 * 1000.0 / f64::from(self.rate)
    }

    /// Moves packets from the jitter buffer into the performer's queue (call every ~10-20 ms).
    pub fn pump(&mut self, queue: &StreamIn) {
        // It ran dry: wait longer from now on.
        let starved = queue.starved();
        if self.started && starved > self.starved_seen {
            self.target_ms = (self.target_ms + GROW_MS).min(MAX_MS);
            self.calm_since = queue.played();
        } else if queue.played().saturating_sub(self.calm_since) > u64::from(self.rate) * 20 {
            // 20 s without running dry: a little less delay.
            self.target_ms = (self.target_ms - 10.0).max(MIN_MS);
            self.calm_since = queue.played();
        }
        self.starved_seen = starved;
        let Some(mut next) = self.next_seq else { return };
        let held_ms = self.held.len() as f64 * 20.0;
        if !self.started {
            // Start once the target is buffered.
            if held_ms < self.target_ms {
                return;
            }
            self.started = true;
        }
        // Drift: the queue ran over by 100 ms: drop a frame's worth (the map starts again there).
        if self.queued_ms(queue) > self.target_ms + 100.0 {
            self.skip_next = true;
        }
        while self.queued_ms(queue) < self.target_ms + 20.0 {
            let have = self.held.remove(&next);
            if have.is_none() {
                // A gap: conceal it only if later packets are here and the queue runs low.
                let later = self.held.keys().next().copied();
                if later.is_none() || self.queued_ms(queue) > self.target_ms / 2.0 {
                    break;
                }
                self.lost += 1;
            }
            let bytes = have.as_ref().and_then(|p| crate::audio::b64_decode(&p.data));
            let pcm = self.decode(bytes.as_deref());
            self.decoded += (pcm.len() / 2) as u64;
            next += 1;
            self.next_seq = Some(next);
            // Converted even when dropped, so output frame `m` of the epoch is always decoded
            // frame `m * step` (the map stays exact).
            let mut conv = Vec::with_capacity(pcm.len() + 8);
            self.rs.run(&pcm, &mut conv);
            let first_out = self.converted;
            self.converted += (conv.len() / 2) as u64;
            if self.skip_next {
                // Dropped: the next frame pushed starts a new segment.
                self.skip_next = false;
                continue;
            }
            if conv.is_empty() {
                continue;
            }
            // This frame's first sample (decoded frame `first_out * step`): Opus's delay off.
            let at = (first_out as f64 * self.rs.step - CODEC_DELAY as f64) / f64::from(RATE);
            let seg = Segment { index: self.pushed, rolled: self.epoch_rolled + at, pos: self.epoch_pos + at };
            let continuous = self.segments.back().is_some_and(|s| {
                let expect = s.rolled + (seg.index - s.index) as f64 / f64::from(self.rate);
                (expect - seg.rolled).abs() < 1.0 / f64::from(self.rate)
            });
            if !continuous {
                self.segments.push_back(seg);
                if self.segments.len() > 10_000 {
                    self.segments.pop_front();
                }
            }
            let n = queue.push(&conv);
            self.pushed = self.pushed.saturating_add(n as u64);
            if n * 2 < conv.len() {
                break;
            }
        }
    }

    /// The host's take clock (s) and timeline (s) of stream frame `index` (frames played).
    pub fn at_index(&self, index: u64) -> Option<(f64, f64)> {
        let i = self.segments.partition_point(|s| s.index <= index);
        let s = self.segments.get(i.checked_sub(1)?)?;
        let d = (index - s.index) as f64 / f64::from(self.rate);
        Some((s.rolled + d, s.pos + d))
    }

    /// A heard point from the performer's audio thread's mark: his take clock (s) at that block,
    /// and the host's take clock of the stream sample he heard then. `None` while it isn't
    /// playing stream all through, or before the stream's first frame.
    pub fn heard(&self, mark: HeardMark) -> Option<SyncPoint> {
        if !mark.rolling || !mark.full {
            return None;
        }
        let (booth, _) = self.at_index(mark.played)?;
        Some(SyncPoint { local: mark.rolled as f64 / f64::from(self.rate), booth, rtt: 0.0 })
    }

    /// The host's timeline position (s) the performer hears now (by the last mark).
    pub fn heard_pos(&self, mark: HeardMark) -> Option<f64> {
        self.at_index(mark.played).map(|(_, p)| p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse_stream(len: usize, at: &[usize]) -> Vec<f32> {
        let mut v = vec![0f32; len * 2];
        for a in at {
            if let Some(x) = v.get_mut(a * 2) {
                *x = 0.9;
            }
            if let Some(x) = v.get_mut(a * 2 + 1) {
                *x = 0.9;
            }
        }
        v
    }

    fn peaks(pcm: &[f32]) -> Vec<usize> {
        let l: Vec<f32> = pcm.iter().step_by(2).map(|x| x.abs()).collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < l.len() {
            if l[i] > 0.3 {
                let end = (i + 40).min(l.len());
                let best = (i..end).max_by(|a, b| l[*a].total_cmp(&l[*b])).unwrap_or(i);
                out.push(best);
                i = best + 200;
            } else {
                i += 1;
            }
        }
        out
    }

    #[test]
    fn opus_delays_by_its_lookahead() {
        let mut e = rusty_opus::OpusEncoder::new(48_000, 2, rusty_opus::Application::Audio).unwrap();
        e.bitrate_bps = BITRATE;
        let mut d = rusty_opus::OpusDecoder::new(48_000, 2).unwrap();
        let input = impulse_stream(FRAME * 10, &[2_000, 5_000]);
        let mut out = Vec::new();
        for f in input.chunks(FRAME * 2) {
            let mut buf = [0u8; 4000];
            let n = e.encode(f, FRAME, &mut buf).unwrap();
            let mut pcm = vec![0f32; FRAME * 2];
            d.decode(&buf[..n], FRAME, &mut pcm).unwrap();
            out.extend(pcm);
        }
        let p = peaks(&out);
        assert_eq!(p.len(), 2, "{p:?}");
        for (got, want) in p.iter().zip([2_000i64, 5_000]) {
            assert!((*got as i64 - want - CODEC_DELAY).abs() <= 2, "decoded peak {got}, sent {want}");
        }
    }

    #[test]
    fn the_resampler_keeps_time() {
        let mut r = Resampler::new(44_100, 48_000);
        let mut out = Vec::new();
        let input = impulse_stream(44_100, &[22_050]);
        for c in input.chunks(512) {
            r.run(c, &mut out);
        }
        let n = out.len() / 2;
        assert!((n as i64 - 48_000).abs() < 8, "{n}");
        let p = peaks(&out);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0], 24_000, "output frame m is input frame m * step, exactly");
    }

    /// A simulated link: each packet arrives `base` + up to `jitter` ms later (so out of order),
    /// `loss` of them never.
    struct Net {
        rng: u64,
        inflight: Vec<(f64, StreamPacket)>,
    }

    impl Net {
        fn rand(&mut self) -> f64 {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            (self.rng % 1_000_000) as f64 / 1_000_000.0
        }
    }

    /// The host plays clicks; the performer hears them through the stream (over a slow, jittery,
    /// lossy link, his clock off by `ppm`) and plays a pluck the instant he hears each; his
    /// heard map puts each pluck at the host take-clock time of its click.
    fn performer_hears(host_rate: u32, perf_rate: u32, ppm: f64, base_ms: f64, jitter_ms: f64, loss: f64) -> Vec<f64> {
        let mut enc = MixEncoder::new(host_rate);
        let mut dec = MixDecoder::new(perf_rate);
        let queue = StreamIn::default();
        queue.set_on(true);
        let mut net = Net { rng: 0x9E37_79B9_7F4A_7C15, inflight: Vec::new() };
        let block = 256usize;
        let clicks: Vec<i64> = (1..12).map(|k| i64::from(host_rate) * k * 7 / 10 + 33 * k).collect();
        let mut host_rolled = 0i64;
        let mut perf_rolled = 0i64;
        let mut perf_frac = 0.0f64;
        let mut points: Vec<SyncPoint> = Vec::new();
        let mut plucks: Vec<i64> = Vec::new();
        let step_s = block as f64 / f64::from(host_rate);
        let mut t = 0.0;
        while host_rolled < i64::from(host_rate) * 9 {
            // The host renders a block.
            let mut s = vec![0f32; block * 2];
            for c in &clicks {
                let k = c - host_rolled;
                if (0..block as i64).contains(&k) {
                    s[k as usize * 2] = 0.9;
                    s[k as usize * 2 + 1] = 0.9;
                }
            }
            let b = TapBlock { rolled: host_rolled, position: 100_000 + host_rolled, rolling: true, samples: s };
            host_rolled += block as i64;
            for p in enc.feed(&[b]) {
                let r = net.rand();
                if r < loss {
                    continue;
                }
                let at = t + (base_ms + net.rand() * jitter_ms) / 1000.0;
                net.inflight.push((at, p));
            }
            t += step_s;
            // Arrivals.
            let (arrived, waiting): (Vec<_>, Vec<_>) = net.inflight.drain(..).partition(|(at, _)| *at <= t);
            net.inflight = waiting;
            for (_, p) in arrived {
                dec.on_packet(p, &queue);
            }
            dec.pump(&queue);
            // The performer's audio thread: the same wall time at his (drifting) rate.
            perf_frac += step_s * f64::from(perf_rate) * (1.0 + ppm * 1e-6);
            let n = perf_frac.floor() as usize;
            perf_frac -= n as f64;
            let (mut l, mut r) = (vec![0f32; n], vec![0f32; n]);
            queue.mix_into(&mut l, &mut r, n, perf_rolled, true);
            if let Some(p) = dec.heard(queue.mark()) {
                points.push(p);
            }
            for (i, x) in l.iter().enumerate() {
                // He plays the instant he hears it (the click's peak).
                if *x > 0.3 && plucks.last().is_none_or(|p| perf_rolled + i as i64 - p > 1_000) {
                    let mut best = i;
                    for j in i..(i + 40).min(l.len()) {
                        if l[j] > l[best] {
                            best = j;
                        }
                    }
                    plucks.push(perf_rolled + best as i64);
                }
            }
            perf_rolled += n as i64;
        }
        let map = crate::sync::ClockMap::exact(&points).unwrap();
        // Each pluck on the host's take clock, against its click (in host samples).
        let mut offs = Vec::new();
        for p in plucks {
            let booth = map.map(p as f64 / f64::from(perf_rate)) * f64::from(host_rate);
            let near = clicks.iter().min_by(|a, b| (booth - **a as f64).abs().total_cmp(&(booth - **b as f64).abs())).unwrap();
            offs.push(booth - *near as f64);
        }
        assert!(offs.len() >= 8, "only {} clicks heard (lost {}, late {})", offs.len(), dec.lost, dec.late);
        offs
    }

    #[test]
    fn a_performer_over_a_slow_jittery_link_lands_where_he_heard_it() {
        let offs = performer_hears(48_000, 48_000, 0.0, 120.0, 80.0, 0.02);
        eprintln!("48k, 120 ms + 80 ms jitter, 2 % loss: {offs:?}");
        // The spec asks for 2 ms (96 samples); it is exact here.
        assert!(offs.iter().all(|o| o.abs() <= 1.0), "{offs:?}");
    }

    #[test]
    fn different_rates_and_drift_still_land_within_a_few_samples() {
        let offs = performer_hears(44_100, 48_000, 120.0, 250.0, 150.0, 0.03);
        eprintln!("44.1k host, 48k performer at +120 ppm, 250 ms + 150 ms jitter: {offs:?}");
        // Within a sample after two rate conversions (2 ms would be 88).
        assert!(offs.iter().all(|o| o.abs() <= 1.0), "{offs:?}");
    }

    #[test]
    fn a_locate_starts_a_new_epoch_and_flushes() {
        let mut enc = MixEncoder::new(48_000);
        let blk = |rolled: i64, pos: i64| TapBlock { rolled, position: pos, rolling: true, samples: vec![0.0; 1920] };
        let a = enc.feed(&[blk(0, 0), blk(960, 960)]);
        assert_eq!(a.len(), 2);
        // Stop, then Play somewhere else: the take clock restarts at 0, a new epoch.
        let stop = TapBlock { rolling: false, ..blk(0, 0) };
        let b = enc.feed(&[stop, blk(0, 480_000)]);
        assert_eq!(b.len(), 1);
        assert_eq!((a[0].epoch, b[0].epoch), (1, 2));
        assert!((b[0].pos - 10.0).abs() < 1e-9);
        let mut dec = MixDecoder::new(48_000);
        let q = StreamIn::default();
        q.set_on(true);
        dec.on_packet(a[0].clone(), &q);
        dec.on_packet(a[1].clone(), &q);
        dec.target_ms = 20.0;
        dec.pump(&q);
        assert!(q.queued() > 0);
        dec.on_packet(b[0].clone(), &q);
        let (mut l, mut r) = (vec![0f32; 10], vec![0f32; 10]);
        q.mix_into(&mut l, &mut r, 10, 0, true);
        assert_eq!(q.queued(), 0, "the old epoch's audio is gone at once");
        dec.pump(&q);
        let (_, pos) = dec.at_index(q.played()).unwrap();
        assert!((pos - (10.0 - CODEC_DELAY as f64 / 48_000.0)).abs() < 1e-6, "{pos}");
    }
}

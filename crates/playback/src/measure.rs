//! Reported latency and the loopback round-trip measurement.
//!
//! - [`Latency`] is what the devices and the feed report, in frames at the session rate:
//!   - `output`: from the output callback's timestamps (playback − callback);
//!   - `input`: from the input callback's timestamps (callback − capture), stored in the feed;
//!   - on ASIO (with the `asio` feature), the driver's own `ASIOGetLatencies` figures, read at
//!     open, replace both `output` and `input`;
//!   - `feed`: the input feed's own latency ([`crate::feed::InputFeed::extra_latency`]), read live;
//!   - `mix`: the mix's compensation delay after the last render.
//! - [`estimate`] is the round trip when nothing is measured.
//! - A measurement ([`crate::Player::measure_latency`]) writes a click into one device output
//!   channel and records one input channel exactly as the mix sees it, in the same callbacks, so
//!   both share one frame counter. [`detect`] then finds the click's onset on the UI thread: the
//!   round trip is the onset frame less the click frame.

use std::time::Duration;

/// `d` in frames at `rate`, rounded; saturating, and 0 for a rate that isn't a positive number.
pub(crate) fn frames(d: Duration, rate: f64) -> u32 {
    let f = (d.as_secs_f64() * rate).round();
    // NaN (a NaN rate, or 0 × ∞) and negatives are 0; the float cast saturates the rest.
    if f > 0.0 { f.min(f64::from(u32::MAX)) as u32 } else { 0 }
}

/// The latencies the devices and the input feed report, in frames at the session rate (see the
/// module docs).
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
pub struct Latency {
    pub input: u32,
    pub output: u32,
    pub feed: u32,
    pub mix: u32,
}

/// The round trip when nothing is measured: input + output + feed. (The record offset adds the
/// mix's own delay, `mix`, on top.)
pub fn estimate(l: Latency) -> i64 {
    i64::from(l.input) + i64::from(l.output) + i64::from(l.feed)
}

/// A take's record offset: take frame `k` belongs at timeline `start + k − offset`.
///
/// - `measured` is a loopback round trip with the feed latency it went through, when there is
///   one for this device, buffer and rate. It replaces the estimate ([`estimate`]): the true
///   input + output + feed. Its feed part is swapped for this take's (`l.feed`), because a drift
///   feed's latency moves between the measurement and the take (a fixed feed's never does).
/// - `manual` is the user's offset, added either way.
/// - `l.mix` is the mix's own delay (PDC) while the take records: the monitored playback comes
///   out that much late, so what was played along to sits that much earlier.
///
/// Read `l.feed` and `l.mix` at the take's first frame ([`crate::record::Recorder::start_feed_latency`],
/// [`crate::record::Recorder::start_mix_latency`]). Saturating.
pub fn record_latency(measured: Option<(i64, u32)>, manual: i64, l: Latency) -> i64 {
    let round_trip = match measured {
        Some((m, feed)) => m.saturating_sub(i64::from(feed)).saturating_add(i64::from(l.feed)),
        None => estimate(l),
    };
    round_trip.saturating_add(manual).saturating_add(i64::from(l.mix))
}

/// How long a measurement records.
const MEASURE_SECONDS: f64 = 2.0;
/// How long a measurement may be out before it counts as lost (the output stopped): its two
/// seconds plus a generous margin. (On the web there is no clock to time it out with.)
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);
/// Frames of the pre-click window (the noise floor), in seconds.
const LEAD_SECONDS: f64 = 0.1;
/// The click: one sample up, one down (no DC), well below full scale.
pub(crate) const CLICK: f32 = 0.5;
/// The onset must stand this far above the pre-click noise floor (20 dB).
const RISE: f32 = 10.0;
/// The quietest floor assumed (−100 dBFS): a silent input is not a 20 dB rise away from a click.
const FLOOR: f32 = 1e-5;
/// The quietest click that counts (−60 dBFS; it goes out at −6 dBFS): after a digitally silent
/// pre-window, faint noise would otherwise stand 20 dB above the floor.
const ONSET: f32 = 1e-3;

/// One measurement's recording, preallocated on the UI thread and handed to the audio thread and
/// back (never resized there).
pub struct MeasureBuf {
    /// Device output channel the click goes to, and the input channel recorded (0-based).
    pub(crate) output: usize,
    pub(crate) input: usize,
    /// The input as the mix sees it, from the callback the measurement starts in.
    pub(crate) samples: Vec<f32>,
    /// Frames recorded so far.
    pub(crate) len: usize,
    /// The click's frame (the pre-click window is everything before it).
    pub(crate) lead: usize,
    /// The measurement could not finish (the transport started, the input went, the rates).
    pub(crate) interrupted: bool,
    /// Which measurement this is: one cancelled or timed out is dropped when it comes back.
    pub(crate) id: u64,
    /// The input feed's latency when the click went out (frames at the session rate): the
    /// round trip went through that much feed (a drift feed's moves).
    pub(crate) feed: u32,
}

impl MeasureBuf {
    /// A measurement of two seconds at `rate` (off the audio thread: it allocates).
    pub(crate) fn new(output: usize, input: usize, rate: u32) -> Box<MeasureBuf> {
        let rate = f64::from(rate.max(1));
        let lead = ((rate * LEAD_SECONDS).round() as usize).max(1);
        let len = ((rate * MEASURE_SECONDS).round() as usize).max(lead * 4);
        Box::new(MeasureBuf { output, input, samples: vec![0.0; len], len: 0, lead, interrupted: false, id: 0, feed: 0 })
    }

    /// The output sample at the measurement's next frame (call before [`MeasureBuf::record`]).
    pub(crate) fn click(&self) -> f32 {
        if self.full() {
            0.0
        } else if self.len == self.lead {
            CLICK
        } else if self.len == self.lead + 1 {
            -CLICK
        } else {
            0.0
        }
    }

    /// Record the input at the measurement's next frame.
    pub(crate) fn record(&mut self, x: f32) {
        if let Some(s) = self.samples.get_mut(self.len) {
            *s = x;
        }
        self.len = (self.len + 1).min(self.samples.len());
    }

    /// Every frame is recorded.
    pub(crate) fn full(&self) -> bool {
        self.len >= self.samples.len()
    }

    /// The round trip in frames, or why there is none.
    pub(crate) fn result(&self) -> Result<i64, String> {
        if self.interrupted {
            return Err("Measure was interrupted: keep the transport stopped while it runs".into());
        }
        detect(self.samples.get(..self.len).unwrap_or(&[]), self.lead, self.input, self.output)
    }
}

/// The click's onset in `samples` (recorded from the frame the click counter starts at, with the
/// click written at `lead`) less `lead`: the round trip. The onset is the first frame of the
/// largest magnitude after `lead`, either polarity (a click smeared by the converters lines up
/// where it peaks, as a recorded take does; a clipped return at its first full-scale frame). The
/// peak must reach [`ONSET`] and stand [`RISE`] above the pre-click floor (the peak before
/// `lead`), else the click did not come back on `input`.
pub(crate) fn detect(samples: &[f32], lead: usize, input: usize, output: usize) -> Result<i64, String> {
    let none = || Err(format!("No click came back on input {}: patch output {} to it", input.saturating_add(1), output.saturating_add(1)));
    let (Some(before), Some(after)) = (samples.get(..lead), samples.get(lead..)) else { return none() };
    let peak = |x: &[f32]| x.iter().map(|v| if v.is_finite() { v.abs() } else { 0.0 }).fold(0.0f32, f32::max);
    let (floor, top) = (peak(before).max(FLOOR), peak(after));
    if top < ONSET || top < RISE * floor {
        return none();
    }
    // The first frame at the peak (ties: the earliest).
    match after.iter().position(|v| v.is_finite() && v.abs() >= top) {
        Some(g) => Ok(i64::try_from(g).unwrap_or(i64::MAX)),
        None => none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_rounds_durations() {
        assert_eq!(frames(Duration::from_micros(2667), 48_000.0), 128);
        assert_eq!(frames(Duration::from_nanos(31_250), 48_000.0), 2, "1.5 frames round up");
        assert_eq!(frames(Duration::from_nanos(31_000), 48_000.0), 1);
        assert_eq!(frames(Duration::ZERO, 48_000.0), 0);
        assert_eq!(frames(Duration::MAX, 1e9), u32::MAX, "saturates");
        assert_eq!(frames(Duration::from_secs(1), f64::NAN), 0);
        assert_eq!(frames(Duration::from_secs(1), -48_000.0), 0);
        assert_eq!(frames(Duration::from_secs(1), f64::INFINITY), u32::MAX);
    }

    #[test]
    fn the_estimate_is_input_output_and_feed() {
        assert_eq!(estimate(Latency { input: 100, output: 120, feed: 128, mix: 64 }), 348);
        assert_eq!(estimate(Latency { input: u32::MAX, output: u32::MAX, feed: u32::MAX, mix: 0 }), 3 * i64::from(u32::MAX));
    }

    #[test]
    fn the_record_latency_takes_measured_feed_manual_and_mix() {
        let l = Latency { input: 100, output: 100, feed: 0, mix: 64 };
        assert_eq!(record_latency(None, 5, l), 269, "unmeasured: the estimate");
        assert_eq!(record_latency(Some((300, 0)), 5, l), 369, "measured replaces the estimate only");
        // A measurement through a drift feed holding 40 then: this take's feed holds 70.
        assert_eq!(record_latency(Some((340, 40)), 0, Latency { feed: 70, ..l }), 300 + 70 + 64);
        assert_eq!(record_latency(Some((i64::MAX, 0)), i64::MAX, l), i64::MAX, "saturates");
    }

    #[test]
    fn detect_finds_the_onset_after_the_click() {
        let mut x = vec![0.0f32; 1000];
        x[100 + 37] = 0.4;
        x[100 + 38] = -0.4;
        assert_eq!(detect(&x, 100, 0, 0), Ok(37));
        // A click smeared by the converters: the round trip is where it peaks (as a recorded
        // take lines up), not where it first rises (half the peak would be one sample early).
        let mut y = vec![0.0f32; 1000];
        y[400..404].copy_from_slice(&[0.2, 0.3, 0.1, -0.1]);
        assert_eq!(detect(&y, 100, 0, 0), Ok(301));
        // Inverted polarity (an input wired the other way round): down first, then up.
        let mut z = vec![0.0f32; 1000];
        z[100 + 37] = -0.4;
        z[100 + 38] = 0.4;
        assert_eq!(detect(&z, 100, 0, 0), Ok(37));
        // A return clipped flat: the first frame at the peak.
        let mut c = vec![0.0f32; 1000];
        c[200..204].copy_from_slice(&[1.0, 1.0, 1.0, -1.0]);
        assert_eq!(detect(&c, 100, 0, 0), Ok(100));
    }

    #[test]
    fn detect_needs_an_absolute_onset_level() {
        // A digitally silent pre-window, then −70 dBFS noise: 20 dB over nothing is no click.
        let mut rng = 0x9e37_79b9u32;
        let x: Vec<f32> = (0..1000)
            .map(|i| {
                rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let white = ((rng >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 3f32.sqrt();
                if i < 100 { 0.0 } else { 3.16e-4 * white }
            })
            .collect();
        assert_eq!(detect(&x, 100, 0, 1), Err("No click came back on input 1: patch output 2 to it".into()));
    }

    #[test]
    fn detect_needs_a_click_20_db_above_the_floor() {
        let mut x = vec![0.01f32; 1000];
        x[500] = 0.05;
        let err = detect(&x, 100, 2, 0).unwrap_err();
        assert_eq!(err, "No click came back on input 3: patch output 1 to it");
        x[500] = 0.2;
        assert_eq!(detect(&x, 100, 2, 0), Ok(400));
        assert!(detect(&[0.0; 1000], 100, 0, 0).is_err(), "silence");
        assert!(detect(&[], 100, 0, 0).is_err() && detect(&[0.0; 10], 100, 0, 0).is_err(), "too short");
    }

    #[test]
    fn the_click_is_one_sample_up_and_one_down_at_the_lead() {
        let mut m = MeasureBuf::new(1, 0, 48_000);
        assert_eq!((m.lead, m.samples.len()), (4800, 96_000));
        let mut out = Vec::new();
        while !m.full() {
            out.push(m.click());
            m.record(0.0);
        }
        assert_eq!(out.len(), 96_000);
        assert_eq!((out[4800], out[4801]), (CLICK, -CLICK));
        assert_eq!(out.iter().filter(|x| **x != 0.0).count(), 2);
        assert_eq!(m.click(), 0.0, "nothing after the end");
    }
}

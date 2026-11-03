//! The record offset: how much earlier takes are placed than they were captured, kept per
//! machine (in [`crate::UiState`], the app preferences) for each audio system + device + buffer +
//! sample rate ([`soundcraft_playback::devices::latency_key`]), because interface drivers add
//! hidden latency that changes with each.
//!
//! - Without a measurement the total is the round trip the devices report
//!   ([`soundcraft_playback::estimate`]) plus the manual offset plus the mix's own delay; the
//!   Playback Engine window and the record button say "not measured at this buffer".
//! - A loopback measurement (`setup.measure_latency`) gives the true round trip for its key and
//!   replaces the reported estimate there. A failed one (no click, interrupted, timed out) shows
//!   its error and keeps the value there was.
//! - Each take reads the feed and mix parts at its first frame
//!   ([`soundcraft_playback::record::Recorder::start_feed_latency`] and `start_mix_latency`), so a
//!   drift feed's moving latency and low-latency monitoring's smaller mix delay don't move takes.

use crate::SoundApp;
use serde_json::{Value, json};
use soundcraft_playback::Latency;
use soundcraft_time::Samples;

/// The manual offset's range, in samples (two seconds at 48 kHz).
pub const MAX_MANUAL: i64 = 96_000;
/// What the window and the record button say while the current key has no measurement.
pub const NOT_MEASURED: &str = "not measured at this buffer";

/// One key's record offset.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RecordOffset {
    /// Added to the round trip, in samples (±[`MAX_MANUAL`]).
    pub manual: i64,
    /// The loopback round trip, when one was measured with this key.
    pub measured: Option<i64>,
    /// The input feed's latency when that measurement came back: a take swaps it for its own.
    pub measured_feed: u32,
}

/// The total offset for `o` with the latencies `reported` (the measured round trip replaces only
/// the reported estimate; the manual offset and the mix's own delay add either way).
pub fn record_latency(o: &RecordOffset, reported: Latency) -> i64 {
    soundcraft_playback::measure::record_latency(o.measured.map(|m| (m, o.measured_feed)), o.manual, reported)
}

/// A take's offset: [`record_latency`] with the feed and mix latencies its first frame saw, when
/// it has one (else those reported now).
pub fn take_latency(o: &RecordOffset, reported: Latency, start_feed: Option<u32>, start_mix: Option<u32>) -> i64 {
    let l = Latency { feed: start_feed.unwrap_or(reported.feed), mix: start_mix.unwrap_or(reported.mix), ..reported };
    record_latency(o, l)
}

/// `hz` in kHz, with one decimal only when it isn't whole (`44.1`, `48`).
fn khz(hz: u32) -> String {
    if hz.is_multiple_of(1000) { (hz / 1000).to_string() } else { format!("{:.1}", f64::from(hz) / 1000.0) }
}

/// The warning shown when recording at a device rate other than the session's (recording goes
/// ahead).
pub fn rate_warning(device: u32, session: u32) -> Option<String> {
    (device != session).then(|| format!("Device runs at {} kHz, session is {} kHz: takes are resampled and may drift", khz(device), khz(session)))
}

impl SoundApp {
    /// The record offset key of the open devices: system, device (output, and input when it is
    /// another device), buffer and rate. The input is the one the player resolved at open
    /// ([`soundcraft_playback::Player::input_name`]), so the key is the same before and after the
    /// input opens.
    pub fn record_offset_key(&self) -> String {
        let Some(p) = &self.player else {
            return soundcraft_playback::devices::latency_key("None", "No audio device", None, self.engine.session().sample_rate.hz());
        };
        let device = match p.input_name.as_deref() {
            Some(i) if i != p.device_name => format!("{} + {i}", p.device_name),
            _ => p.device_name.clone(),
        };
        soundcraft_playback::devices::latency_key(&p.system, &device, p.device_buffer, p.device_rate)
    }

    /// The current key's record offset (the default when there is none).
    pub fn record_offset(&self) -> RecordOffset {
        self.ui.record_offsets.get(&self.record_offset_key()).cloned().unwrap_or_default()
    }

    /// What the open devices report now (nothing without a player).
    pub fn reported_latency(&self) -> Latency {
        self.player.as_ref().map(soundcraft_playback::Player::reported_latency).unwrap_or_default()
    }

    /// The total record offset now, in samples.
    pub fn record_offset_total(&self) -> i64 {
        record_latency(&self.record_offset(), self.reported_latency())
    }

    /// The offset of the take recording now. Once it has its first frame, the latencies are
    /// snapshotted once (the devices' as reported then, the feed's and the mix's at that frame),
    /// so the offset stays put for the rest of the take; before, those reported now.
    pub(crate) fn take_latency(&mut self) -> Samples {
        let o = self.record_offset();
        if let Some(l) = self.take_reported {
            return record_latency(&o, l);
        }
        let r = self.recorder.as_ref();
        let (feed, mix) = (
            r.and_then(soundcraft_playback::record::Recorder::start_feed_latency),
            r.and_then(soundcraft_playback::record::Recorder::start_mix_latency),
        );
        let reported = self.reported_latency();
        if r.and_then(soundcraft_playback::record::Recorder::start_pos).is_some() {
            self.take_reported = Some(Latency { feed: feed.unwrap_or(reported.feed), mix: mix.unwrap_or(reported.mix), ..reported });
        }
        take_latency(&o, reported, feed, mix)
    }

    /// Forgets a measurement out on a player being replaced: it never comes back from the new one.
    pub(crate) fn forget_measure(&mut self) {
        self.measuring = None;
    }

    /// "offset N samples (measured)" or "offset N samples (not measured at this buffer)".
    pub fn record_offset_summary(&self) -> String {
        let how = if self.record_offset().measured.is_some() { "measured" } else { NOT_MEASURED };
        format!("offset {} samples ({how})", self.record_offset_total())
    }

    /// The rate warning for the open device, when it runs at another rate than the session.
    pub fn rate_warning(&self) -> Option<String> {
        let p = self.player.as_ref().filter(|p| !p.silent)?;
        rate_warning(p.device_rate, self.engine.session().sample_rate.hz())
    }

    /// True while a loopback measurement is out.
    pub fn measuring(&self) -> bool {
        self.measuring.is_some()
    }

    /// Abandons the measurement out there (Cancel, or its window closing); the value there was
    /// stays.
    pub fn cancel_measure(&mut self) {
        if self.measuring.take().is_some() {
            if let Some(p) = &self.player {
                p.cancel_measure();
            }
            self.ui.status = "Measure cancelled".into();
        }
    }

    /// Polls the measurement out there (each frame).
    pub(crate) fn poll_measure(&mut self) {
        if self.measuring.is_none() {
            return;
        }
        let Some(r) = self.player.as_ref().and_then(soundcraft_playback::Player::measure_result) else { return };
        self.finish_measure(r);
    }

    /// A measurement's outcome: `Ok(round trip)` is stored for the key it was started with, and
    /// the status says it; an error (no click came back, interrupted, timed out) goes to the
    /// status and the value there was stays.
    pub fn finish_measure(&mut self, r: Result<i64, String>) {
        let key = self.measuring.take().unwrap_or_else(|| self.record_offset_key());
        match r {
            Ok(n) => {
                let feed = self.player.as_ref().map_or(0, soundcraft_playback::Player::measure_feed);
                let o = self.ui.record_offsets.entry(key).or_default();
                o.measured = Some(n);
                o.measured_feed = feed;
                let (buffer, rate) = self.player.as_ref().map_or((None, 48_000), |p| (p.device_buffer, p.device_rate));
                let buffer = buffer.map_or("the default buffer".to_string(), |b| b.to_string());
                let ms = n as f64 * 1000.0 / f64::from(rate.max(1));
                self.ui.status = format!("Round trip at {buffer}: {n} samples ({ms:.1} ms)");
            }
            Err(e) => self.ui.status = e,
        }
    }
}

/// `setup.record_offset {samples?}`: sets the current key's manual offset (clamped to
/// ±[`MAX_MANUAL`]); without `samples` it only reads. Returns `{manual, measured, total,
/// measured_here, key}`.
pub(crate) fn record_offset_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let key = app.record_offset_key();
    if let Some(v) = p.get("samples") {
        let n = v.as_f64().filter(|n| !n.is_nan()).ok_or("`samples` must be a number")?;
        let manual = n.round().clamp(-(MAX_MANUAL as f64), MAX_MANUAL as f64) as i64;
        app.ui.record_offsets.entry(key.clone()).or_default().manual = manual;
    }
    let o = app.record_offset();
    Ok(json!({"manual": o.manual, "measured": o.measured, "total": app.record_offset_total(), "measured_here": o.measured.is_some(), "key": key}))
}

/// `setup.measure_latency {output?, input?}`: starts a loopback measurement out of the named
/// output's first channel (default: the main's) into the named input's first channel (default:
/// channel 1). The result arrives later (see [`SoundApp::finish_measure`]).
pub(crate) fn measure_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let s = app.engine.session();
    let output = match p.get("output").and_then(Value::as_str) {
        Some(name) => {
            let i = s.output_index(name).ok_or_else(|| format!("no output named “{name}”"))?;
            s.outputs.get(i).map_or(0, |o| usize::from(o.first_channel))
        }
        None => s.outputs.first().map_or(0, |o| usize::from(o.first_channel)),
    };
    let input = match p.get("input").and_then(Value::as_str) {
        Some(name) => s.input_channels(name).ok_or_else(|| format!("no input named “{name}”"))?.0,
        None => 0,
    };
    // Everything that refuses a measure before opening the input.
    let rate = s.sample_rate.hz();
    let Some(player) = app.player.as_ref().filter(|p| !p.silent) else { return Err("no audio device".into()) };
    if app.measuring.is_some() {
        return Err("a measurement is already running".into());
    }
    if player.is_playing() || app.engine.transport.recording {
        return Err("stop the transport first".into());
    }
    if player.device_rate != rate {
        return Err("Measure needs the session and the device at the same rate".into());
    }
    // The input must be open for the click to come back.
    app.ensure_input();
    let key = app.record_offset_key();
    let Some(player) = &app.player else { return Err("no audio device".into()) };
    player.measure_latency(output, input)?;
    app.measuring = Some(key);
    app.ui.status = "Measuring the round trip…".into();
    Ok(json!({"started": true}))
}

/// `setup.measure_cancel`: abandons the measurement out there (the previous value stays).
/// Returns `{cancelled: bool}`: false when none was running.
pub(crate) fn measure_cancel_command(app: &mut SoundApp) -> Value {
    let running = app.measuring();
    app.cancel_measure();
    json!({"cancelled": running})
}

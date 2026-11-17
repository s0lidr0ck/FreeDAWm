//! Reconnecting by itself when the audio output device that went away comes back: after the
//! player reports the device gone, the output system is asked every [`PROBE_EVERY`] (on a worker
//! thread, as listing devices can take a moment) whether the device is listed again; when it is,
//! the app reconnects as the Reconnect button does. ASIO is never probed (a driver may still be
//! loaded), so it keeps the manual button.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

/// How often a lost device is looked for.
pub const PROBE_EVERY: Duration = Duration::from_secs(2);

/// What [`Watch::step`] asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing yet.
    Wait,
    /// Look for the device now (and pass the answer to [`Watch::answer`]).
    Probe,
    /// The device is back: reconnect.
    Reconnect,
}

/// A device that went away, being watched for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    pub system: String,
    pub device: String,
    /// When to probe next (`None` while a probe is out).
    next: Option<Instant>,
    /// The last answer, not yet acted on.
    back: bool,
}

impl Watch {
    /// Watches for `device` of `system`, from `now` (the first probe after [`PROBE_EVERY`]).
    /// `None` for ASIO, which is never probed.
    pub fn new(system: &str, device: &str, now: Instant) -> Option<Watch> {
        (!system.eq_ignore_ascii_case("ASIO") && !device.is_empty()).then(|| Watch {
            system: system.to_string(),
            device: device.to_string(),
            next: Some(now + PROBE_EVERY),
            back: false,
        })
    }

    /// What to do at `now`.
    pub fn step(&mut self, now: Instant) -> Step {
        if self.back {
            return Step::Reconnect;
        }
        match self.next {
            Some(t) if now >= t => {
                self.next = None;
                Step::Probe
            }
            _ => Step::Wait,
        }
    }

    /// A probe's answer at `now`: listed again or not (the next probe after [`PROBE_EVERY`]).
    pub fn answer(&mut self, present: bool, now: Instant) {
        self.back = present;
        self.next = Some(now + PROBE_EVERY);
    }
}

/// The watch with its probe in flight, as the app holds it.
pub struct Returning {
    pub watch: Watch,
    probe: Option<Receiver<bool>>,
}

impl Returning {
    pub fn new(watch: Watch) -> Returning {
        Returning { watch, probe: None }
    }

    /// Runs the watch one frame: starts a probe on a worker thread when due, takes its answer
    /// when in. True when the device is back.
    pub fn poll(&mut self, now: Instant) -> bool {
        if let Some(rx) = &self.probe {
            match rx.try_recv() {
                Ok(present) => {
                    self.probe = None;
                    self.watch.answer(present, now);
                }
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    self.probe = None;
                    self.watch.answer(false, now);
                }
            }
        }
        match self.watch.step(now) {
            Step::Reconnect => true,
            Step::Wait => false,
            Step::Probe => {
                let (tx, rx) = channel();
                let (system, device) = (self.watch.system.clone(), self.watch.device.clone());
                let spawned = std::thread::Builder::new().name("device-return".into()).spawn(move || {
                    let _ = tx.send(soundcraft_playback::devices::output_present(&system, &device));
                });
                match spawned {
                    Ok(_) => self.probe = Some(rx),
                    Err(_) => self.watch.answer(false, now),
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lost_device_is_probed_every_two_seconds_and_reconnected_when_back() {
        let t0 = Instant::now();
        assert!(Watch::new("ASIO", "Card", t0).is_none(), "ASIO keeps the manual button");
        let mut w = Watch::new("ALSA", "Card", t0).unwrap();
        assert_eq!(w.step(t0), Step::Wait);
        assert_eq!(w.step(t0 + PROBE_EVERY), Step::Probe);
        // One probe at a time: nothing more until it answers.
        assert_eq!(w.step(t0 + PROBE_EVERY * 3), Step::Wait);
        w.answer(false, t0 + PROBE_EVERY * 3);
        assert_eq!(w.step(t0 + PROBE_EVERY * 4 - Duration::from_millis(1)), Step::Wait);
        assert_eq!(w.step(t0 + PROBE_EVERY * 4), Step::Probe);
        w.answer(true, t0 + PROBE_EVERY * 4);
        assert_eq!(w.step(t0 + PROBE_EVERY * 4), Step::Reconnect);
    }
}

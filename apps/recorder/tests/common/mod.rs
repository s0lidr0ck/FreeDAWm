//! A recorder on no device: a player driven by hand (`Player::stalled`), an input fed by hand
//! (`Recorder::synthetic`) and MIDI ports that exist only for the test (`FakeBackend`).
#![allow(dead_code)]

use soundcraft_playback::midi_in::{FakeBackend, MidiHub, MidiInputs};
use soundcraft_playback::record::{Recorder, SyntheticInput};
use soundcraft_playback::{Player, StalledAudio};
use soundcraft_recorder::rig::{InputChoice, Rig};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub const RATE: u32 = 48_000;
pub const BLOCK: usize = 256;

pub struct Offline {
    pub rig: Rig,
    pub audio: StalledAudio,
    pub input: Option<SyntheticInput>,
    pub keys: FakeBackend,
    /// Input frames fed so far (the next frame's index).
    pub fed: usize,
    /// When the next block is due (blocks run at real time, as a device's would).
    next: Instant,
}

/// A recorder with an input of `channels` (none: 0) and MIDI ports `ports`.
pub fn offline(channels: usize, ports: &[&str], choice: InputChoice) -> Offline {
    let mut e = soundcraft_engine::Engine::default();
    e.session_mut().sample_rate = soundcraft_time::SampleRate::new(RATE).unwrap();
    let (player, audio) = Player::stalled(e.session_arc(), RATE, BLOCK, 2);
    let (input, synth) = if channels > 0 {
        let (r, s): (Recorder, SyntheticInput) = Recorder::synthetic(&player, channels);
        (Some(r), Some(s))
    } else {
        (None, None)
    };
    let keys = FakeBackend::with_ports(ports);
    let midi = if ports.is_empty() {
        None
    } else {
        let mut m = MidiInputs::new(keys.boxed(), MidiHub::new(), BTreeMap::new());
        m.rescan();
        Some(m)
    };
    let rig = Rig::offline(player, input, midi, choice);
    Offline { rig, audio, input: synth, keys, fed: 0, next: Instant::now() }
}

impl Offline {
    /// One device callback: `BLOCK` input frames (`sample(frame, channel)`), then the output.
    /// Returns the output block (interleaved stereo).
    pub fn block(&mut self, sample: &dyn Fn(usize, usize) -> f32) -> Vec<f32> {
        let now = Instant::now();
        if self.next > now {
            std::thread::sleep(self.next - now);
        }
        // A sound card's clock does not slow down when the computer is busy: after a late block
        // the next ones run back to back until the device is on time again.
        self.next += Duration::from_secs_f64(BLOCK as f64 / f64::from(RATE));
        if let Some(i) = &self.input {
            let ch = self.rig.input_channels().max(1);
            let mut v = Vec::with_capacity(BLOCK * ch);
            for f in 0..BLOCK {
                for c in 0..ch {
                    v.push(sample(self.fed + f, c));
                }
            }
            i.push(&v);
        }
        self.fed += BLOCK;
        let mut out = vec![0.0f32; BLOCK * 2];
        self.audio.fill(&mut out, 2);
        out
    }

    /// Blocks for `secs` of real time, the recorder's frame run every few blocks.
    pub fn run(&mut self, secs: f64, sample: &dyn Fn(usize, usize) -> f32) {
        let n = (secs * f64::from(RATE) / BLOCK as f64).ceil() as usize;
        for k in 0..n {
            self.block(sample);
            if k % 4 == 0 {
                self.rig.step();
            }
        }
    }
}

/// The input every test feeds: channel 0 a ramp that never repeats within a take
/// (`(i mod 4000) / 8000`), channel 1 its negative.
pub fn ramp(i: usize, c: usize) -> f32 {
    let x = (i % 4000) as f32 / 8000.0;
    if c == 0 { x } else { -x }
}

pub fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("freedawm-recorder-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

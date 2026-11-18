//! An instrument's extra outputs on their own mixer channels: a taken output plays into its bus
//! and through the aux that reads it (its own fader), not through the instrument's track; an
//! output nobody takes stays on the main output; a muted source is silent on every output.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_dsp::{Plugin, PluginInfo};
use soundcraft_mix::{MixEngine, PreparedInstance, SlotKey};
use soundcraft_model::{ChannelFormat, Insert, InstrumentOut, Route, Session, TrackId, TrackKind};

const SR: usize = 48_000;
const BLOCK: usize = 256;
const ID: &str = "clap:test.two_outs";

/// Plays 0.25 on its main output, and 0.5 on "Out 2" while it is taken (else adds it to the
/// main output, as a real instrument does).
struct TwoOuts {
    inner: Box<dyn Plugin>,
    taken: u32,
}

impl Plugin for TwoOuts {
    fn info(&self) -> &'static PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _v: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let v = if self.taken & 1 != 0 { 0.25 } else { 0.75 };
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = v);
        }
    }
    fn extra_outputs(&self) -> Vec<String> {
        vec!["Out 2".into()]
    }
    fn set_outputs_taken(&mut self, mask: u32) {
        self.taken = mask;
    }
    fn read_output(&mut self, index: usize, out: &mut [Vec<f32>], frames: usize) -> bool {
        if index != 0 {
            return false;
        }
        for c in out.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.5);
        }
        true
    }
}

fn session(taken: bool) -> (Session, TrackId, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Kit"));
    s.track_mut(t).unwrap().instrument = Some(Insert::new(ID));
    s.track_mut(t).unwrap().mixer.output = Route::Main;
    let bus = s.add_bus("Kit Out 2", ChannelFormat::Stereo);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Kit Out 2"));
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
    s.track_mut(aux).unwrap().mixer.output = Route::Main;
    if taken {
        s.track_mut(t).unwrap().instrument_outs = vec![InstrumentOut { output: 0, bus }];
    }
    (s, t, aux)
}

/// The main mix's left channel after a few blocks.
fn main_left(s: &Session, t: TrackId) -> f32 {
    let plugin = TwoOuts { inner: soundcraft_dsp::create("gain").unwrap(), taken: 0 };
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: ID.into(), ch: 2, plugin: Box::new(plugin) }]);
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in 0..8 {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
    }
    out[0][BLOCK - 1]
}

#[test]
fn a_taken_output_plays_through_its_aux() {
    // Not taken: the instrument plays everything on its main output (0.75).
    let (s, t, _) = session(false);
    let none = main_left(&s, t);
    assert!(none > 0.1, "{none}");
    // Taken: 0.25 through the track and 0.5 through the aux; the same level in all.
    let (s, t, aux) = session(true);
    let both = main_left(&s, t);
    assert!((both - none).abs() < 1e-3, "the same sound, split over two channels: {none} vs {both}");
    // The aux's fader turns the output down alone.
    let (mut s2, t2, _) = session(true);
    s2.track_mut(aux).unwrap().mixer.mute = true;
    let main_only = main_left(&s2, t2);
    assert!((main_only - none / 3.0).abs() < 1e-3, "{main_only} vs {}", none / 3.0);
    // Muting the instrument's track silences its outputs too.
    let mut s3 = s.clone();
    s3.track_mut(t).unwrap().mixer.mute = true;
    assert!(main_left(&s3, t).abs() < 1e-4);
}

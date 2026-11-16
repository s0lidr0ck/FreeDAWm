//! A MIDI track playing an instrument on another track: the MIDI to Track instrument.
//!
//! An instrument or MIDI track whose instrument is MIDI to Track ([`ID`]; its state the target
//! track's id in decimal, its `channel` parameter 0 for "as played" or 1-16) makes no sound: what
//! it plays (its clips, its live input, chased controllers, the panic) is collected each block
//! and played by the target track's instrument after it, merged with the target's own notes and
//! with every other track routed there, each keeping its channel. Several MIDI tracks on
//! channels 1, 2, 3 … can so play one multi-timbral instrument.
//!
//! - The plugin runs where any instrument runs, so clip scheduling, MIDI Thru, chasing, the
//!   transport's stops and All MIDI Notes Off reach the target as they would reach a synth.
//! - **Order and loops.** The source renders in an earlier level than the target
//!   ([`crate::sidechain`] orders it as a module MIDI input), so the target hears this block's
//!   events in this block, at the same sample offsets as its own clips: what delay compensation
//!   does for the target's audio covers them. A route that would close a loop (the target
//!   already plays into the source, directly or through other routes, keys or buses) is refused
//!   and plays nothing.
//! - **Mute.** A muted source (by its mute, a solo elsewhere or its VCA) lets go of the notes it
//!   holds on the target and starts no new ones until it is heard again.
//!
//! On the audio thread nothing here allocates or panics: the events go into a fixed
//! [`MidiOut`] ring (full, an event is dropped and counted).

use soundcraft_dsp::{Category, MidiOut, ParamInfo, Plugin, PluginInfo, Taper, Unit};

/// The MIDI to Track instrument's plugin id.
pub const ID: &str = "track_midi";

static PARAMS: [ParamInfo; 1] = [ParamInfo {
    id: "channel",
    name: "Channel",
    min: 0.0,
    max: 16.0,
    default: 0.0,
    unit: Unit::None,
    taper: Taper::Linear,
    choices: &[],
    display: Some(channel_text),
    group: "",
}];

fn channel_text(v: f32) -> String {
    if v < 0.5 { "As played".into() } else { format!("{}", v.round() as u8) }
}

pub static INFO: PluginInfo = PluginInfo {
    id: ID,
    name: "MIDI to Track",
    short_name: "To Track",
    category: Category::Instrument,
    params: &PARAMS,
    is_instrument: true,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: true,
};

/// The plugin info of `id` when it is MIDI to Track.
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    (id == ID).then_some(&INFO)
}

/// A new MIDI to Track instance when `id` names it.
pub fn create(id: &str) -> Option<Box<dyn Plugin>> {
    (id == ID).then(|| Box::new(TrackMidi::new()) as Box<dyn Plugin>)
}

/// The target track id kept in a MIDI to Track instrument's state (`None`: another instrument,
/// or no target).
pub fn target_of(ins: &soundcraft_model::Insert) -> Option<soundcraft_model::TrackId> {
    if ins.plugin != ID {
        return None;
    }
    let bytes = ins.state_bytes()?;
    std::str::from_utf8(&bytes).ok()?.trim().parse::<u64>().ok().map(soundcraft_model::TrackId)
}

/// The MIDI to Track instrument (see the module docs).
pub struct TrackMidi {
    /// This block's events, handed to the host after `process` ([`Plugin::midi_out`]).
    out: MidiOut,
    /// 0: each message on its own channel; 1-16: every message on that one.
    channel: u8,
    /// The source is muted: new notes are not passed on.
    muted: bool,
    /// Notes it left on, by channel (bit per pitch), so a mute or a stop lets go of exactly those.
    held: [[u64; 2]; 16],
    /// Channels it sent on since its last stop (bit per channel).
    used: u16,
}

impl Default for TrackMidi {
    fn default() -> Self {
        TrackMidi::new()
    }
}

impl TrackMidi {
    pub fn new() -> TrackMidi {
        TrackMidi { out: MidiOut::new(), channel: 0, muted: false, held: [[0; 2]; 16], used: 0 }
    }

    fn emit(&mut self, offset: usize, msg: [u8; 3]) {
        let mut msg = msg;
        if !(0x80..0xF0).contains(&msg[0]) {
            return;
        }
        if self.channel > 0 {
            msg[0] = (msg[0] & 0xF0) | ((self.channel - 1) & 0x0F);
        }
        let ch = usize::from(msg[0] & 0x0F);
        let (kind, pitch) = (msg[0] & 0xF0, msg[1] & 0x7F);
        let bit = 1u64 << (pitch % 64);
        let on = kind == 0x90 && msg[2] > 0;
        let held = self.held.get(ch).and_then(|h| h.get(usize::from(pitch / 64))).is_some_and(|w| w & bit != 0);
        // Muted: nothing new starts; a note-off of a note it left on still goes.
        if self.muted && !(matches!(kind, 0x80 | 0x90) && !on && held) {
            return;
        }
        if let Some(w) = self.held.get_mut(ch).and_then(|h| h.get_mut(usize::from(pitch / 64))) {
            match kind {
                0x90 if on => *w |= bit,
                0x80 | 0x90 => *w &= !bit,
                _ => {}
            }
        }
        self.used |= 1 << ch;
        self.out.push(u32::try_from(offset).unwrap_or(u32::MAX), msg);
    }

    /// Note-offs for every note it left on (at the block's start).
    fn release_held(&mut self) {
        let held = std::mem::take(&mut self.held);
        for (ch, words) in held.iter().enumerate() {
            for (w, bits) in words.iter().enumerate() {
                for k in 0..64u8 {
                    if bits & (1u64 << k) != 0 {
                        self.out.push(0, [0x80 | (ch as u8 & 0x0F), (w as u8) * 64 + k, 0]);
                    }
                }
            }
        }
    }
}

impl Plugin for TrackMidi {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "channel" => {
                let v = PARAMS.first().map_or(0.0, |pi| pi.clamp(value)).round() as u8;
                if v != self.channel {
                    // Notes left on the old channel would never get their note-off.
                    self.release_held();
                    self.channel = v;
                }
                true
            }
            // The host's: whether the source track is heard this block.
            "muted" => {
                let m = value >= 0.5;
                if m && !self.muted {
                    self.release_held();
                }
                self.muted = m;
                true
            }
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "channel" => Some(f32::from(self.channel)),
            "muted" => Some(if self.muted { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.emit(offset, [0x90, note & 0x7F, velocity.clamp(1, 127)]);
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.emit(offset, [0x80, note & 0x7F, 0]);
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.emit(offset, msg);
    }

    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.emit(offset, msg);
    }

    /// Note-offs for every note it left on, then sustain up and All Notes Off on each channel it
    /// sent on since the last time.
    fn all_notes_off(&mut self) {
        let used = std::mem::take(&mut self.used);
        self.release_held();
        for ch in 0..16u8 {
            if used & (1 << ch) != 0 {
                self.out.push(0, [0xB0 | ch, 64, 0]);
                self.out.push(0, [0xB0 | ch, 123, 0]);
            }
        }
    }

    fn midi_out(&mut self, out: &mut MidiOut) {
        for e in self.out.events() {
            out.push(e.offset, e.msg);
        }
        self.out.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(p: &mut TrackMidi) -> Vec<(u32, [u8; 3])> {
        let mut out = MidiOut::new();
        p.midi_out(&mut out);
        out.events().iter().map(|e| (e.offset, e.msg)).collect()
    }

    #[test]
    fn it_passes_what_it_plays_on_its_channel_and_a_mute_lets_go() {
        let mut p = TrackMidi::new();
        p.set_param("channel", 3.0);
        p.note_on(10, 60, 100);
        p.midi(20, [0xB0, 1, 64]);
        assert_eq!(drain(&mut p), vec![(10, [0x92, 60, 100]), (20, [0xB2, 1, 64])]);
        assert!(drain(&mut p).is_empty(), "each block's events once");
        p.set_param("muted", 1.0);
        p.note_on(5, 62, 90);
        assert_eq!(drain(&mut p), vec![(0, [0x82, 60, 0])], "the held note lets go, a new one does not start");
        p.set_param("muted", 0.0);
        p.note_on(1, 64, 80);
        p.all_notes_off();
        let out = drain(&mut p);
        assert!(out.contains(&(1, [0x92, 64, 80])) && out.contains(&(0, [0x82, 64, 0])) && out.contains(&(0, [0xB2, 123, 0])), "{out:?}");
    }

    #[test]
    fn as_played_keeps_each_channel() {
        let mut p = TrackMidi::new();
        p.midi_raw(0, [0x95, 40, 70]);
        p.midi_raw(3, [0x85, 40, 0]);
        assert_eq!(drain(&mut p), vec![(0, [0x95, 40, 70]), (3, [0x85, 40, 0])]);
        let ins = {
            let mut i = soundcraft_model::Insert::new(ID);
            i.set_state_bytes(b"42");
            i
        };
        assert_eq!(target_of(&ins), Some(soundcraft_model::TrackId(42)));
        assert_eq!(target_of(&soundcraft_model::Insert::new("subtractive_synth")), None);
    }
}

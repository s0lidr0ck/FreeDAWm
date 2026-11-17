//! MIDI learn mappings: a controller message (a knob's CC, a pad's note) bound to a mixer or
//! plug-in parameter, or to a transport action.
//!
//! Parameter mappings are saved with the session ([`crate::Session::midi_maps`]): they name the
//! session's tracks and plug-ins, which mean nothing in another song. Transport mappings are the
//! same type, kept by the app per machine (the controller on this desk, whatever the song).

use crate::{AutoParam, TrackId};

/// What a controller sends that a mapping listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiSourceKind {
    /// A control change; `number` is the controller.
    Cc,
    /// A note (its velocity is the value; a note-off is 0); `number` is the pitch.
    Note,
}

/// A controller message to listen to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MidiSource {
    /// The input port's name; `None`: any port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    pub kind: MidiSourceKind,
    /// 0-15.
    pub channel: u8,
    /// The controller or pitch, 0-127.
    pub number: u8,
}

impl MidiSource {
    /// Whether `msg` (from port `port`) is this source.
    pub fn matches(&self, port: &str, msg: [u8; 3]) -> bool {
        if self.port.as_deref().is_some_and(|p| p != port) || msg[0] & 0x0F != self.channel {
            return false;
        }
        match self.kind {
            MidiSourceKind::Cc => msg[0] & 0xF0 == 0xB0 && msg[1] == self.number,
            MidiSourceKind::Note => matches!(msg[0] & 0xF0, 0x80 | 0x90) && msg[1] == self.number,
        }
    }

    /// The source a learnt message is (`None` for anything but a CC or a note).
    pub fn of(port: &str, msg: [u8; 3]) -> Option<MidiSource> {
        let kind = match msg[0] & 0xF0 {
            0xB0 => MidiSourceKind::Cc,
            0x80 | 0x90 => MidiSourceKind::Note,
            _ => return None,
        };
        Some(MidiSource { port: (!port.is_empty()).then(|| port.to_string()), kind, channel: msg[0] & 0x0F, number: msg[1] & 0x7F })
    }

    /// How a list names it: "CC 7 ch 1 (Keys)".
    pub fn label(&self) -> String {
        let what = match self.kind {
            MidiSourceKind::Cc => format!("CC {}", self.number),
            MidiSourceKind::Note => format!("Note {}", self.number),
        };
        match &self.port {
            Some(p) => format!("{what} ch {} ({p})", self.channel + 1),
            None => format!("{what} ch {}", self.channel + 1),
        }
    }
}

/// A transport action a controller key can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportAction {
    Play,
    Stop,
    PlayStop,
    Record,
    ReturnToZero,
    Loop,
    /// Punch in and out while playing (a footswitch): Record pressed again.
    Punch,
}

impl TransportAction {
    pub const ALL: [TransportAction; 7] = [
        TransportAction::Play,
        TransportAction::Stop,
        TransportAction::PlayStop,
        TransportAction::Record,
        TransportAction::ReturnToZero,
        TransportAction::Loop,
        TransportAction::Punch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TransportAction::Play => "Play",
            TransportAction::Stop => "Stop",
            TransportAction::PlayStop => "Play/Stop",
            TransportAction::Record => "Record",
            TransportAction::ReturnToZero => "Return to Zero",
            TransportAction::Loop => "Loop Playback",
            TransportAction::Punch => "Punch In/Out (footswitch)",
        }
    }
}

/// What a mapping moves.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiTarget {
    /// A track's automatable parameter: volume, pan, a send's level, a plug-in parameter.
    Param {
        track: TrackId,
        param: AutoParam,
    },
    Transport(TransportAction),
}

/// One mapping (see the module docs).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MidiMap {
    pub source: MidiSource,
    pub target: MidiTarget,
    /// The part of the parameter's range the controller's travel covers, 0..1 each (min may be
    /// above max).
    #[serde(default)]
    pub min: f32,
    #[serde(default = "one")]
    pub max: f32,
    /// The controller's travel runs the other way.
    #[serde(default)]
    pub invert: bool,
}

fn one() -> f32 {
    1.0
}

impl MidiMap {
    pub fn new(source: MidiSource, target: MidiTarget) -> MidiMap {
        MidiMap { source, target, min: 0.0, max: 1.0, invert: false }
    }

    /// Where a controller value (0-127) puts the parameter, 0..1 of its range.
    pub fn position(&self, value: u8) -> f32 {
        let x = f32::from(value.min(127)) / 127.0;
        let x = if self.invert { 1.0 - x } else { x };
        let (lo, hi) = (self.min.clamp(0.0, 1.0), self.max.clamp(0.0, 1.0));
        lo + (hi - lo) * x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mapping_matches_its_source_and_scales_its_range() {
        let src = MidiSource::of("Keys", [0xB2, 7, 100]).unwrap();
        assert_eq!(src, MidiSource { port: Some("Keys".into()), kind: MidiSourceKind::Cc, channel: 2, number: 7 });
        assert!(src.matches("Keys", [0xB2, 7, 0]));
        assert!(!src.matches("Other", [0xB2, 7, 0]) && !src.matches("Keys", [0xB3, 7, 0]) && !src.matches("Keys", [0x92, 7, 1]));
        assert_eq!(src.label(), "CC 7 ch 3 (Keys)");
        let mut m = MidiMap::new(src, MidiTarget::Transport(TransportAction::Play));
        assert_eq!((m.position(0), m.position(127)), (0.0, 1.0));
        (m.min, m.max, m.invert) = (0.25, 0.75, true);
        assert_eq!((m.position(0), m.position(127)), (0.75, 0.25));
        assert!(MidiSource::of("", [0xE0, 0, 0]).is_none());
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<MidiMap>(&json).unwrap(), m);
    }
}

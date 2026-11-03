//! What crosses the network: the status feed, the service remote's actions, the booth's
//! transport as followers see it, and a follower's report. See `docs/control-protocol.md`
//! ("LAN remote") for the wire format around them.

use serde::{Deserialize, Serialize};

use crate::take::BoothAnchor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransportState {
    #[default]
    Stopped,
    Playing,
    Recording,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TrackStatus {
    pub id: u64,
    pub name: String,
    pub armed: bool,
    /// Peak level 0..1 (meter scale), for the page's bar.
    pub level: f32,
    /// The remote PC feeding it (`from Piano PC`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MarkerStatus {
    pub id: u64,
    pub number: u32,
    pub name: String,
    /// Seconds on the timeline.
    pub at: f64,
}

/// A remote recorder as the booth sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RemoteStatus {
    pub name: String,
    /// `connected`, `recording`, `transferring`, `waiting to transfer`, `take received`,
    /// `not reachable`.
    pub state: String,
    /// `MIDI · 1,284 notes`.
    #[serde(default)]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_free: Option<u64>,
    /// Seconds since it was last heard from, while it is not connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<f64>,
    #[serde(default)]
    pub warning: bool,
}

/// The status feed: everything the service remote shows. Sent to each subscriber whenever it
/// changes, at most ~15 times a second.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RigStatus {
    /// This PC's name and the session's.
    pub pc: String,
    pub session: String,
    pub transport: TransportState,
    /// Playhead, seconds, and its bar.
    pub position: f64,
    #[serde(default)]
    pub bar: i64,
    /// Seconds since recording started (0 when not recording).
    pub record_elapsed: f64,
    pub tracks: Vec<TrackStatus>,
    pub markers: Vec<MarkerStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_free: Option<u64>,
    /// Hours of recording the free space holds at the armed tracks' rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_hours_left: Option<f64>,
    /// Filled in by the hub: the remote recorders and the browsers connected.
    #[serde(default)]
    pub remotes: Vec<RemoteStatus>,
    #[serde(default)]
    pub clients: Vec<String>,
    /// A banner for the page (a remote recorder unreachable at Record).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// What the service remote's buttons send (`POST /api/action`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RemoteAction {
    Record,
    Play,
    /// While recording, Stop needs the button held (`held: true`): a tap does nothing.
    Stop {
        #[serde(default)]
        held: bool,
    },
    ArmAll {
        value: bool,
    },
    Arm {
        track: u64,
        value: bool,
    },
    /// A marker at the playhead, named with the time unless a name is given.
    Marker {
        #[serde(default)]
        name: Option<String>,
    },
    RenameMarker {
        id: u64,
        name: String,
    },
}

/// The booth's transport as remote recorders follow it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BoothTransport {
    pub state: TransportState,
    /// The booth's take (0 when not recording); a new number per Record.
    pub take: u64,
    /// Where the booth's take sits (while recording; `end` set at Stop).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<BoothAnchor>,
    /// The booth's playhead (seconds) when this was sent.
    pub position: f64,
    /// Loop playback range (seconds), so a follower loops the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub looped: Option<[f64; 2]>,
    /// The booth's session name (a follower names its own session after it).
    #[serde(default)]
    pub session: String,
    /// The booth project's sample rate and bit depth (16, 24, 32 = float): a remote audio take is
    /// sent at that depth and lands at that rate.
    #[serde(default)]
    pub rate: u32,
    #[serde(default)]
    pub bits: u16,
}

/// A remote recorder's report to the booth (about once a second).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FollowerReport {
    /// `following`, `recording`, `transferring`, `waiting to transfer`.
    pub state: String,
    /// Notes in the take being recorded.
    pub notes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_free: Option<u64>,
    /// Takes waiting to be sent.
    pub queued: usize,
    /// Its MIDI input ports (the booth offers `Remote: <PC> › <port>` for each).
    pub ports: Vec<String>,
    /// Its armed audio inputs (the booth offers `Remote: <PC> › <input>` for each).
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Clips in the take being recorded.
    #[serde(default)]
    pub clips: usize,
    /// How far the take on its way has gone (0..1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sending: Option<f64>,
    /// The performer's last level check (`Level check: good, peak -8 dB`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
}

/// One note of the live sketch (`feed`): booth timeline seconds, best effort.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SketchNote {
    pub at: f64,
    pub pitch: u8,
    /// 0 for a note-off.
    pub vel: u8,
}

/// Formats a byte count as the page does ("412 GB").
pub fn human_bytes(b: u64) -> String {
    let gb = b as f64 / 1e9;
    if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", b as f64 / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_parse_from_the_page() {
        let a: RemoteAction = serde_json::from_str(r#"{"action":"stop"}"#).unwrap();
        assert_eq!(a, RemoteAction::Stop { held: false });
        let a: RemoteAction = serde_json::from_str(r#"{"action":"arm","track":3,"value":true}"#).unwrap();
        assert_eq!(a, RemoteAction::Arm { track: 3, value: true });
        assert!(serde_json::from_str::<RemoteAction>(r#"{"action":"delete_everything"}"#).is_err(), "nothing that deletes");
        assert_eq!(human_bytes(412_000_000_000), "412 GB");
        assert_eq!(human_bytes(1_500_000_000), "1.5 GB");
    }
}

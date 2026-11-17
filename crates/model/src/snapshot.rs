//! Mixer snapshots (Studio One's and Cubase's mix scenes): the strips' settings stored under a
//! name in the session, recalled whole or by section (`mix.snapshot_*` in the engine).

use crate::{Mixer, Track, TrackId};

/// Most snapshots a session keeps.
pub const MAX_SNAPSHOTS: usize = 64;

/// What a recall can bring back, by section: (id, label).
pub const SNAPSHOT_SECTIONS: [(&str, &str); 7] = [
    ("faders", "Faders and trim"),
    ("pan", "Pan, width and balance"),
    ("mutes", "Mute and solo"),
    ("sends", "Sends"),
    ("inserts", "Inserts"),
    ("routing", "Input, output and VCA"),
    ("delay", "Track delay"),
];

/// One stored mix: every strip's mixer settings as they were.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MixSnapshot {
    pub name: String,
    pub strips: Vec<SnapshotStrip>,
}

/// One track's mixer in a [`MixSnapshot`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotStrip {
    pub track: TrackId,
    pub mixer: Mixer,
}

impl MixSnapshot {
    /// The mixers of `tracks` now.
    pub fn take<'a>(name: &str, tracks: impl IntoIterator<Item = &'a Track>) -> MixSnapshot {
        MixSnapshot { name: name.to_string(), strips: tracks.into_iter().map(|t| SnapshotStrip { track: t.id, mixer: t.mixer.clone() }).collect() }
    }

    /// Puts the `sections` (ids of [`SNAPSHOT_SECTIONS`]) of the stored strip onto `t`, if the
    /// snapshot holds it; returns whether it did. A section whose shape no longer fits (a pan of
    /// another width after a format change) is left as it is.
    pub fn recall_onto(&self, t: &mut Track, sections: &[&str]) -> bool {
        let Some(st) = self.strips.iter().find(|s| s.track == t.id) else { return false };
        let (from, to) = (&st.mixer, &mut t.mixer);
        let on = |s: &str| sections.contains(&s);
        if on("faders") {
            to.volume_db = from.volume_db;
            to.trim_db = from.trim_db;
        }
        if on("pan") {
            if from.pan.len() == to.pan.len() {
                to.pan.clone_from(&from.pan);
            }
            to.surround = from.surround;
            to.width = from.width;
            to.balance = from.balance;
        }
        if on("mutes") {
            to.mute = from.mute;
            to.solo = from.solo;
        }
        if on("sends") {
            to.sends.clone_from(&from.sends);
        }
        if on("inserts") {
            to.inserts.clone_from(&from.inserts);
            to.next_module = to.next_module.max(from.next_module);
        }
        if on("routing") {
            to.input = from.input.clone();
            to.output = from.output.clone();
            to.vca = from.vca;
        }
        if on("delay") {
            to.delay = from.delay;
        }
        true
    }
}

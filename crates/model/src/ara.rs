//! What a session keeps for its ARA plug-ins: each plug-in's saved ARA document
//! ([`AraArchive`], saved with the session) and the clips' offline renders through the plug-in
//! ([`AraRenders`], derived, never saved).

use crate::{Clip, ClipId};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One ARA plug-in's document as the plug-in stored it (its ARA archive, opaque to FreeDAWm),
/// with what restoring it needs. Kept as it is while the plug-in is not installed, so a session
/// opened and saved without it loses nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AraArchive {
    /// The plug-in's id (`vst3:<class id>`).
    pub plugin: String,
    /// Its name, version and maker when it was saved (for the note shown without it).
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// The document archive id its factory gave (`ARAFactory::documentArchiveID`): a plug-in
    /// restores it only when its own id or one of its compatible ids is this.
    pub archive_id: String,
    /// The archive, standard base64.
    pub data: String,
    /// The state of the plug-in instance showing its editor (the VST3 component and controller
    /// state), standard base64; empty when none was open.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub view_state: String,
}

impl AraArchive {
    /// The archive's bytes (`None` when the base64 is broken).
    pub fn bytes(&self) -> Option<Vec<u8>> {
        crate::b64::decode(&self.data)
    }
}

/// A clip's audio rendered through its ARA plug-in, offline, from the clip's start for its
/// length (before clip gain and fades), one buffer per channel of its track.
#[derive(Debug, Clone, PartialEq)]
pub struct AraRender {
    /// [`ara_fingerprint`] of the clip it was rendered for.
    pub fingerprint: u64,
    pub audio: Arc<Vec<Vec<f32>>>,
}

impl AraRender {
    /// Whether this render plays `clip` as it is now (same plug-in, place, length and audio).
    pub fn serves(&self, clip: &Clip) -> bool {
        ara_fingerprint(clip) == Some(self.fingerprint)
    }
}

/// What a clip's ARA render depends on: its plug-in (not bypassed), timeline place and length,
/// file, offset and stretch. `None` for a clip without an ARA plug-in, or a bypassed one.
pub fn ara_fingerprint(clip: &Clip) -> Option<u64> {
    let a = clip.ara.as_ref().filter(|a| !a.bypass)?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(a.plugin.as_bytes());
    eat(&clip.id.0.to_le_bytes());
    eat(&clip.start.to_le_bytes());
    eat(&clip.length.to_le_bytes());
    eat(&clip.stretch.to_bits().to_le_bytes());
    if let crate::ClipContent::Audio { source, offset } = clip.content {
        eat(&source.0.to_le_bytes());
        eat(&offset.to_le_bytes());
    }
    Some(h)
}

/// The clips' ARA renders by clip ([`AraRender`]). Derived data behind an `Arc`, so session
/// snapshots share it; two sessions are equal whatever it holds.
#[derive(Debug, Clone, Default)]
pub struct AraRenders(pub Arc<BTreeMap<ClipId, Arc<AraRender>>>);

impl PartialEq for AraRenders {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

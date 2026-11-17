//! Per-track mixer state: inserts, sends, routing, fader, pan, mute/solo.

use crate::{AutomationMode, BusId, SourceId, TrackId};
use std::collections::BTreeMap;

pub const SEND_SLOTS: usize = 10;

/// Channel formats (track widths).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum ChannelFormat {
    #[default]
    Mono,
    Stereo,
    Lcr,
    Quad,
    Lcrs,
    Surround50,
    Surround51,
    Surround60,
    Surround61,
    Sdds70,
    Sdds71,
    Surround70,
    Surround71,
    Atmos702,
    Atmos712,
    Atmos502,
    Atmos512,
    Atmos504,
    Atmos514,
    Atmos704,
    Atmos714,
    Atmos706,
    Atmos716,
    Atmos904,
    Atmos914,
    Atmos906,
    Atmos916,
    /// Ambisonics of order 1..=7 ((n+1)² channels).
    Ambisonics(u8),
}

impl ChannelFormat {
    /// Most channels one track, bus or output carries through the mixer.
    pub const MAX_MIX_CHANNELS: usize = 16;
    pub const ALL: [ChannelFormat; 34] = [
        ChannelFormat::Mono,
        ChannelFormat::Stereo,
        ChannelFormat::Lcr,
        ChannelFormat::Quad,
        ChannelFormat::Lcrs,
        ChannelFormat::Surround50,
        ChannelFormat::Surround51,
        ChannelFormat::Surround60,
        ChannelFormat::Surround61,
        ChannelFormat::Sdds70,
        ChannelFormat::Sdds71,
        ChannelFormat::Surround70,
        ChannelFormat::Surround71,
        ChannelFormat::Atmos702,
        ChannelFormat::Atmos712,
        ChannelFormat::Ambisonics(1),
        ChannelFormat::Ambisonics(2),
        ChannelFormat::Ambisonics(3),
        ChannelFormat::Ambisonics(4),
        ChannelFormat::Ambisonics(5),
        ChannelFormat::Ambisonics(6),
        ChannelFormat::Ambisonics(7),
        ChannelFormat::Atmos502,
        ChannelFormat::Atmos512,
        ChannelFormat::Atmos504,
        ChannelFormat::Atmos514,
        ChannelFormat::Atmos704,
        ChannelFormat::Atmos714,
        ChannelFormat::Atmos706,
        ChannelFormat::Atmos716,
        ChannelFormat::Atmos904,
        ChannelFormat::Atmos914,
        ChannelFormat::Atmos906,
        ChannelFormat::Atmos916,
    ];
    pub fn channels(self) -> usize {
        match self {
            ChannelFormat::Mono => 1,
            ChannelFormat::Stereo => 2,
            ChannelFormat::Lcr => 3,
            ChannelFormat::Quad | ChannelFormat::Lcrs => 4,
            ChannelFormat::Surround50 => 5,
            ChannelFormat::Surround51 | ChannelFormat::Surround60 => 6,
            ChannelFormat::Surround61 | ChannelFormat::Sdds70 | ChannelFormat::Surround70 | ChannelFormat::Atmos502 => 7,
            ChannelFormat::Sdds71 | ChannelFormat::Surround71 | ChannelFormat::Atmos512 => 8,
            ChannelFormat::Atmos702 | ChannelFormat::Atmos504 => 9,
            ChannelFormat::Atmos712 | ChannelFormat::Atmos514 => 10,
            ChannelFormat::Atmos704 => 11,
            ChannelFormat::Atmos714 => 12,
            ChannelFormat::Atmos706 | ChannelFormat::Atmos904 => 13,
            ChannelFormat::Atmos716 | ChannelFormat::Atmos914 => 14,
            ChannelFormat::Atmos906 => 15,
            ChannelFormat::Atmos916 => 16,
            ChannelFormat::Ambisonics(n) => {
                let n = usize::from(n.clamp(1, 7)) + 1;
                n * n
            }
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ChannelFormat::Mono => "Mono",
            ChannelFormat::Stereo => "Stereo",
            ChannelFormat::Lcr => "LCR",
            ChannelFormat::Quad => "Quad",
            ChannelFormat::Lcrs => "LCRS",
            ChannelFormat::Surround50 => "5.0",
            ChannelFormat::Surround51 => "5.1",
            ChannelFormat::Surround60 => "6.0",
            ChannelFormat::Surround61 => "6.1",
            ChannelFormat::Sdds70 => "7.0 SDDS",
            ChannelFormat::Sdds71 => "7.1 SDDS",
            ChannelFormat::Surround70 => "7.0",
            ChannelFormat::Surround71 => "7.1",
            ChannelFormat::Atmos702 => "7.0.2",
            ChannelFormat::Atmos712 => "7.1.2",
            ChannelFormat::Atmos502 => "5.0.2",
            ChannelFormat::Atmos512 => "5.1.2",
            ChannelFormat::Atmos504 => "5.0.4",
            ChannelFormat::Atmos514 => "5.1.4",
            ChannelFormat::Atmos704 => "7.0.4",
            ChannelFormat::Atmos714 => "7.1.4",
            ChannelFormat::Atmos706 => "7.0.6",
            ChannelFormat::Atmos716 => "7.1.6",
            ChannelFormat::Atmos904 => "9.0.4",
            ChannelFormat::Atmos914 => "9.1.4",
            ChannelFormat::Atmos906 => "9.0.6",
            ChannelFormat::Atmos916 => "9.1.6",
            ChannelFormat::Ambisonics(1) => "1st Order Ambisonics",
            ChannelFormat::Ambisonics(2) => "2nd Order Ambisonics",
            ChannelFormat::Ambisonics(3) => "3rd Order Ambisonics",
            ChannelFormat::Ambisonics(4) => "4th Order Ambisonics",
            ChannelFormat::Ambisonics(5) => "5th Order Ambisonics",
            ChannelFormat::Ambisonics(6) => "6th Order Ambisonics",
            ChannelFormat::Ambisonics(_) => "7th Order Ambisonics",
        }
    }
    pub fn from_id(s: &str) -> Option<ChannelFormat> {
        ChannelFormat::ALL.into_iter().find(|f| f.label().eq_ignore_ascii_case(s.trim()))
    }
    /// Speakers in channel order, or empty for Ambisonics (ACN order, SN3D).
    ///
    /// Channel order is SMPTE / WAV order: speakers with a WAVE_FORMAT_EXTENSIBLE mask bit come in
    /// ascending bit order (L R C LFE Ls Rs Lc Rc Cs Lss Rss Ltf Rtf Ltr Rtr; 7.x puts the rear
    /// surrounds Lrs/Rrs on the "back" bits before the sides Lss/Rss), followed by speakers WAV
    /// cannot name (the top-middle pair of .6 layouts). E.g. 5.1 = L R C LFE Ls Rs,
    /// 7.1 = L R C LFE Lrs Rrs Lss Rss, 7.1.4 = L R C LFE Lrs Rrs Lss Rss Ltf Rtf Ltr Rtr.
    pub fn speakers(self) -> &'static [Speaker] {
        use Speaker::*;
        match self {
            ChannelFormat::Mono => &[C],
            ChannelFormat::Stereo => &[L, R],
            ChannelFormat::Lcr => &[L, R, C],
            ChannelFormat::Quad => &[L, R, Ls, Rs],
            ChannelFormat::Lcrs => &[L, R, C, Cs],
            ChannelFormat::Surround50 => &[L, R, C, Ls, Rs],
            ChannelFormat::Surround51 => &[L, R, C, Lfe, Ls, Rs],
            ChannelFormat::Surround60 => &[L, R, C, Ls, Rs, Cs],
            ChannelFormat::Surround61 => &[L, R, C, Lfe, Ls, Rs, Cs],
            ChannelFormat::Sdds70 => &[L, R, C, Ls, Rs, Lc, Rc],
            ChannelFormat::Sdds71 => &[L, R, C, Lfe, Ls, Rs, Lc, Rc],
            ChannelFormat::Surround70 => &[L, R, C, Lrs, Rrs, Lss, Rss],
            ChannelFormat::Surround71 => &[L, R, C, Lfe, Lrs, Rrs, Lss, Rss],
            ChannelFormat::Atmos702 => &[L, R, C, Lrs, Rrs, Lss, Rss, Ltm, Rtm],
            ChannelFormat::Atmos712 => &[L, R, C, Lfe, Lrs, Rrs, Lss, Rss, Ltm, Rtm],
            ChannelFormat::Atmos502 => &[L, R, C, Ls, Rs, Ltm, Rtm],
            ChannelFormat::Atmos512 => &[L, R, C, Lfe, Ls, Rs, Ltm, Rtm],
            ChannelFormat::Atmos504 => &[L, R, C, Ls, Rs, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos514 => &[L, R, C, Lfe, Ls, Rs, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos704 => &[L, R, C, Lrs, Rrs, Lss, Rss, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos714 => &[L, R, C, Lfe, Lrs, Rrs, Lss, Rss, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos706 => &[L, R, C, Lrs, Rrs, Lss, Rss, Ltf, Rtf, Ltr, Rtr, Ltm, Rtm],
            ChannelFormat::Atmos716 => &[L, R, C, Lfe, Lrs, Rrs, Lss, Rss, Ltf, Rtf, Ltr, Rtr, Ltm, Rtm],
            ChannelFormat::Atmos904 => &[L, R, C, Lrs, Rrs, Lw, Rw, Lss, Rss, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos914 => &[L, R, C, Lfe, Lrs, Rrs, Lw, Rw, Lss, Rss, Ltf, Rtf, Ltr, Rtr],
            ChannelFormat::Atmos906 => &[L, R, C, Lrs, Rrs, Lw, Rw, Lss, Rss, Ltf, Rtf, Ltr, Rtr, Ltm, Rtm],
            ChannelFormat::Atmos916 => &[L, R, C, Lfe, Lrs, Rrs, Lw, Rw, Lss, Rss, Ltf, Rtf, Ltr, Rtr, Ltm, Rtm],
            ChannelFormat::Ambisonics(_) => &[],
        }
    }
    pub fn is_ambisonic(self) -> bool {
        matches!(self, ChannelFormat::Ambisonics(_))
    }
    /// Whether the mixer can carry this format: at most [`ChannelFormat::MAX_MIX_CHANNELS`]
    /// channels (Ambisonics up to 3rd order; 4th to 7th order are stored but not offered).
    pub fn mixable(self) -> bool {
        self.channels() <= Self::MAX_MIX_CHANNELS
    }
    pub fn has_lfe(self) -> bool {
        self.speakers().contains(&Speaker::Lfe)
    }
    /// Each channel's loudness weight ([`Speaker::loudness_weight`]); 1.0 for every channel of
    /// an Ambisonics format (it has no speakers).
    pub fn loudness_weights(self) -> Vec<f32> {
        let sp = self.speakers();
        if sp.is_empty() { vec![1.0; self.channels()] } else { sp.iter().map(|s| s.loudness_weight()).collect() }
    }
    pub fn has_height(self) -> bool {
        self.speakers().iter().any(|s| s.is_height())
    }
    /// Short per-channel labels for meters (`W Y Z X 5…` for Ambisonics).
    pub fn channel_label(self, ch: usize) -> String {
        match self.speakers().get(ch) {
            Some(s) => s.label().to_string(),
            None if self.is_ambisonic() => ["W", "Y", "Z", "X"].get(ch).map_or_else(|| format!("{}", ch + 1), |s| s.to_string()),
            None => format!("{}", ch + 1),
        }
    }
    /// WAVE_FORMAT_EXTENSIBLE channel mask (0 = no positions, e.g. Ambisonics). Speakers that WAV
    /// cannot name (top middle) borrow the top-front bits when the layout has no top fronts.
    pub fn channel_mask(self) -> u32 {
        let sp = self.speakers();
        let has_tf = sp.contains(&Speaker::Ltf);
        sp.iter().fold(0u32, |m, s| match s {
            Speaker::Ltm if !has_tf => m | 0x1000,
            Speaker::Rtm if !has_tf => m | 0x4000,
            s => m | s.wave_bit(),
        })
    }
    /// Best format for a channel count.
    pub fn for_channels(n: usize) -> ChannelFormat {
        [
            ChannelFormat::Mono,
            ChannelFormat::Stereo,
            ChannelFormat::Lcr,
            ChannelFormat::Quad,
            ChannelFormat::Surround50,
            ChannelFormat::Surround51,
            ChannelFormat::Surround70,
            ChannelFormat::Surround71,
        ]
        .into_iter()
        .find(|f| f.channels() == n)
        .unwrap_or(if n <= 1 { ChannelFormat::Mono } else { ChannelFormat::Stereo })
    }
}

/// A loudspeaker position in a channel format.
///
/// Angles follow ITU-R BS.775 / Dolby conventions: azimuth in degrees, 0 = front centre, positive
/// = to the right; elevation in degrees above the listener plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Speaker {
    L,
    R,
    C,
    Lfe,
    /// 5.x surrounds (±110°).
    Ls,
    Rs,
    /// SDDS inner fronts (±15°).
    Lc,
    Rc,
    /// Centre surround (180°).
    Cs,
    /// 7.x side surrounds (±90°).
    Lss,
    Rss,
    /// 7.x rear surrounds (±150°).
    Lrs,
    Rrs,
    /// 9.x front wides (±60°).
    Lw,
    Rw,
    /// Top front (±45°, elevation 45°).
    Ltf,
    Rtf,
    /// Top middle (±90°, elevation 45°).
    Ltm,
    Rtm,
    /// Top rear (±135°, elevation 45°).
    Ltr,
    Rtr,
}

impl Speaker {
    pub fn azimuth(self) -> f32 {
        match self {
            Speaker::L => -30.0,
            Speaker::R => 30.0,
            Speaker::C | Speaker::Lfe => 0.0,
            Speaker::Ls => -110.0,
            Speaker::Rs => 110.0,
            Speaker::Lc => -15.0,
            Speaker::Rc => 15.0,
            Speaker::Cs => 180.0,
            Speaker::Lss => -90.0,
            Speaker::Rss => 90.0,
            Speaker::Lrs => -150.0,
            Speaker::Rrs => 150.0,
            Speaker::Lw => -60.0,
            Speaker::Rw => 60.0,
            Speaker::Ltf => -45.0,
            Speaker::Rtf => 45.0,
            Speaker::Ltm => -90.0,
            Speaker::Rtm => 90.0,
            Speaker::Ltr => -135.0,
            Speaker::Rtr => 135.0,
        }
    }
    pub fn elevation(self) -> f32 {
        if self.is_height() { 45.0 } else { 0.0 }
    }
    pub fn is_height(self) -> bool {
        matches!(self, Speaker::Ltf | Speaker::Rtf | Speaker::Ltm | Speaker::Rtm | Speaker::Ltr | Speaker::Rtr)
    }
    pub fn is_lfe(self) -> bool {
        self == Speaker::Lfe
    }
    /// The channel weight loudness measurement gives this speaker (ITU-R BS.1770-4, table 3):
    /// 0 for the LFE, 1.41 (+1.5 dB) for a speaker at ear height between 60° and 120° from the
    /// front, 1.0 for every other.
    pub fn loudness_weight(self) -> f32 {
        match self {
            Speaker::Lfe => 0.0,
            Speaker::Ls | Speaker::Rs | Speaker::Lss | Speaker::Rss | Speaker::Lw | Speaker::Rw => 1.41,
            _ => 1.0,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Speaker::L => "L",
            Speaker::R => "R",
            Speaker::C => "C",
            Speaker::Lfe => "LFE",
            Speaker::Ls => "Ls",
            Speaker::Rs => "Rs",
            Speaker::Lc => "Lc",
            Speaker::Rc => "Rc",
            Speaker::Cs => "Cs",
            Speaker::Lss => "Lss",
            Speaker::Rss => "Rss",
            Speaker::Lrs => "Lrs",
            Speaker::Rrs => "Rrs",
            Speaker::Lw => "Lw",
            Speaker::Rw => "Rw",
            Speaker::Ltf => "Ltf",
            Speaker::Rtf => "Rtf",
            Speaker::Ltm => "Ltm",
            Speaker::Rtm => "Rtm",
            Speaker::Ltr => "Ltr",
            Speaker::Rtr => "Rtr",
        }
    }
    /// WAVE_FORMAT_EXTENSIBLE `dwChannelMask` bit (0 for speakers WAV cannot name).
    pub fn wave_bit(self) -> u32 {
        match self {
            Speaker::L => 0x1,
            Speaker::R => 0x2,
            Speaker::C => 0x4,
            Speaker::Lfe => 0x8,
            Speaker::Ls | Speaker::Lrs => 0x10,
            Speaker::Rs | Speaker::Rrs => 0x20,
            Speaker::Lc | Speaker::Lw => 0x40,
            Speaker::Rc | Speaker::Rw => 0x80,
            Speaker::Cs => 0x100,
            Speaker::Lss => 0x200,
            Speaker::Rss => 0x400,
            Speaker::Ltf => 0x1000,
            Speaker::Rtf => 0x4000,
            Speaker::Ltr => 0x8000,
            Speaker::Rtr => 0x2_0000,
            Speaker::Ltm | Speaker::Rtm => 0,
        }
    }
}

/// Surround panner position (Pro Tools-style X/Y puck). `None` on a mixer means "use the stereo
/// pan", which keeps sessions made before surround mixing sounding exactly as they did.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SurroundPan {
    /// -1 (left) ..= 1 (right).
    pub x: f32,
    /// -1 (back) ..= 1 (front).
    pub y: f32,
    /// Elevation 0 (ear level) ..= 1 (overhead), used by height formats.
    pub z: f32,
    /// 0 = point source ..= 1 = spread over every speaker.
    pub divergence: f32,
    /// Centre speaker share of front-centre images, 0..=100 % (0 = phantom centre on L/R).
    pub center: f32,
    /// LFE send level, dB (-144 = off).
    pub lfe_db: f32,
}

impl Default for SurroundPan {
    fn default() -> Self {
        SurroundPan { x: 0.0, y: 1.0, z: 0.0, divergence: 0.0, center: 100.0, lfe_db: -144.0 }
    }
}

impl SurroundPan {
    /// The axis an [`AutoParam::Surround`](crate::AutoParam::Surround) lane moves: 0 x, 1 y,
    /// 2 z, 3 divergence (anything else reads x).
    pub fn axis(&self, i: u8) -> f32 {
        match i {
            1 => self.y,
            2 => self.z,
            3 => self.divergence,
            _ => self.x,
        }
    }

    /// Sets [`SurroundPan::axis`] `i` to `v`, kept in its range (a non-finite value is ignored).
    pub fn set_axis(&mut self, i: u8, v: f32) {
        if !v.is_finite() {
            return;
        }
        match i {
            1 => self.y = v.clamp(-1.0, 1.0),
            2 => self.z = v.clamp(0.0, 1.0),
            3 => self.divergence = v.clamp(0.0, 1.0),
            _ => self.x = v.clamp(-1.0, 1.0),
        }
    }

    /// Puck at the stereo pan position (front edge).
    pub fn from_stereo(pan: f32) -> Self {
        SurroundPan { x: finite_or(pan, 0.0).clamp(-1.0, 1.0), ..SurroundPan::default() }
    }
    /// Clamp into range (NaN → defaults).
    pub fn sanitize(&mut self) {
        let d = SurroundPan::default();
        self.x = finite_or(self.x, d.x).clamp(-1.0, 1.0);
        self.y = finite_or(self.y, d.y).clamp(-1.0, 1.0);
        self.z = finite_or(self.z, d.z).clamp(0.0, 1.0);
        self.divergence = finite_or(self.divergence, d.divergence).clamp(0.0, 1.0);
        self.center = finite_or(self.center, d.center).clamp(0.0, 100.0);
        self.lfe_db = finite_or(self.lfe_db, d.lfe_db).clamp(-144.0, 12.0);
    }
}

/// An internal mix bus (Pro Tools "bus" paths).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bus {
    pub id: BusId,
    pub name: String,
    pub format: ChannelFormat,
}

/// Physical output paths (from the I/O setup).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutputPath {
    pub name: String,
    /// First hardware channel (0-based).
    pub first_channel: u16,
    pub format: ChannelFormat,
}

/// Physical input paths (from the I/O setup): named device input channels, mono, stereo or a
/// multichannel (surround) group of up to [`ChannelFormat::MAX_MIX_CHANNELS`] channels. A channel
/// the open device lacks is kept and shown "(not available)".
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InputPath {
    pub name: String,
    /// First hardware channel (0-based).
    pub first_channel: u16,
    /// Mono, stereo or a multichannel format (up to 16 channels).
    pub format: ChannelFormat,
}

impl InputPath {
    /// How many device channels the input takes (1 to [`ChannelFormat::MAX_MIX_CHANNELS`]).
    pub fn width(&self) -> usize {
        self.format.channels().clamp(1, ChannelFormat::MAX_MIX_CHANNELS)
    }
}

/// The device input channel that track channel `k` reads from an input of `width` channels
/// starting at `first`: a mono input feeds every side of the track; a wider input feeds the
/// track's channels in order, and a track wider than it hears silence on the channels past the
/// input's (`None`), never a repeat of the input's last channel (a stereo input on a 5.1 track
/// is heard and recorded on L and R alone).
pub fn track_input_channel(k: usize, first: usize, width: usize) -> Option<usize> {
    match width {
        0 | 1 => Some(first),
        _ => (k < width).then(|| first.saturating_add(k)),
    }
}

/// The device input channel that track channel `k` of a `track_channels`-wide track reads when no
/// hardware input is chosen, on a device with `device_inputs` channels: the device's first
/// channels in order (a stereo track hears and records In 1-2); a mono device feeds every side,
/// and a device with fewer channels than the track leaves the rest silent (`None`). Monitoring
/// and recording both use this, so what is heard is what is recorded.
pub fn default_input_channel(k: usize, track_channels: usize, device_inputs: usize) -> Option<usize> {
    track_input_channel(k, 0, track_channels.min(device_inputs).max(1))
}

/// Per-track input monitoring (the strip's "I" button).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMonitor {
    /// An armed track hears its input while stopped or recording.
    #[default]
    Auto,
    /// Always hears its input.
    On,
    /// Never hears its input, even armed (direct monitoring on the interface).
    Off,
}

impl InputMonitor {
    pub fn id(self) -> &'static str {
        match self {
            InputMonitor::Auto => "auto",
            InputMonitor::On => "on",
            InputMonitor::Off => "off",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(InputMonitor::Auto),
            "on" => Some(InputMonitor::On),
            "off" => Some(InputMonitor::Off),
            _ => None,
        }
    }

    /// Auto → On → Off → Auto.
    pub fn next(self) -> Self {
        match self {
            InputMonitor::Auto => InputMonitor::On,
            InputMonitor::On => InputMonitor::Off,
            InputMonitor::Off => InputMonitor::Auto,
        }
    }
}

/// Old sessions saved a bool (`true` → On, `false` → Auto). Anything unrecognised loads as Auto
/// rather than failing the whole session.
impl<'de> serde::Deserialize<'de> for InputMonitor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bool(bool),
            Id(String),
            Other(serde::de::IgnoredAny),
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Bool(true) => InputMonitor::On,
            Raw::Bool(false) => InputMonitor::Auto,
            Raw::Id(s) => InputMonitor::from_id(&s).unwrap_or_default(),
            Raw::Other(_) => InputMonitor::Auto,
        })
    }
}

/// Where a signal goes or comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum Route {
    #[default]
    None,
    /// The main output (hardware outputs 1-2, or the bounce source).
    Main,
    Bus(BusId),
    /// Hardware input/output by path name.
    Hardware(String),
    /// A Band musician (0-based slot) as a track input; never valid as an output.
    Musician(u8),
    /// Chain modules' MIDI output (`Plugin::midi_out`: Shale's note per hit) as the input of an
    /// instrument or MIDI track: one module or several (a kit's Shales into one drum instrument),
    /// merged per block in sample order, each keeping its channel and note. Never empty (that is
    /// `None`), never valid as an output.
    Modules(Vec<ModuleRef>),
    /// Hardware MIDI input (a USB keyboard, an interface's MIDI In, Musical Typing) as the input
    /// of an instrument or MIDI track: one port or all of them, one channel or Omni. Never valid
    /// as an output.
    Midi(MidiIn),
}

/// Which live MIDI a track takes ([`Route::Midi`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct MidiIn {
    /// The port's name; `None`: all inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// 0: Omni (every channel), else 1-16.
    #[serde(default)]
    pub channel: u8,
    /// What the track does to its input on the way in, before its instrument hears it and
    /// before a take keeps it.
    #[serde(default, skip_serializing_if = "MidiTransform::is_identity")]
    pub transform: MidiTransform,
}

/// A track's MIDI input transform (Real-Time Properties for the input): applied to what the
/// instrument hears live (thru) and to what a take records, never to the clips. Note-ons and
/// note-offs go through the same mapping, so a note always ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MidiTransform {
    /// Semitones added to every note (and poly pressure key); a note moved past 0..127 is left out.
    pub transpose: i8,
    /// Velocity scale in percent (100: as played).
    pub velocity: u8,
    /// Velocity curve, -100..100: 0 straight, above 0 harder (soft playing comes out softer),
    /// below 0 softer (soft playing comes out louder).
    pub curve: i8,
    /// Every note-on at this velocity (0: off). Comes after scale and curve.
    pub fixed_velocity: u8,
    /// Only the keys from `key_low` to `key_high` (as played, before transposing) pass.
    pub key_low: u8,
    pub key_high: u8,
    /// Every channel message onto this channel, 1-16 (0: as played).
    pub channel: u8,
}

impl Default for MidiTransform {
    fn default() -> Self {
        MidiTransform { transpose: 0, velocity: 100, curve: 0, fixed_velocity: 0, key_low: 0, key_high: 127, channel: 0 }
    }
}

impl MidiTransform {
    /// Nothing changes.
    pub fn is_identity(&self) -> bool {
        *self == MidiTransform::default()
    }

    /// `msg` as the transform makes it; `None` when it is left out (a key outside the range, or
    /// moved past 0..127). No allocation.
    pub fn apply(&self, msg: [u8; 3]) -> Option<[u8; 3]> {
        if self.is_identity() {
            return Some(msg);
        }
        let [mut status, mut d1, mut d2] = msg;
        let kind = status & 0xF0;
        if matches!(kind, 0x80 | 0x90 | 0xA0) {
            if d1 < self.key_low || d1 > self.key_high {
                return None;
            }
            d1 = u8::try_from(i16::from(d1) + i16::from(self.transpose)).ok().filter(|p| *p <= 127)?;
        }
        if kind == 0x90 && d2 > 0 {
            let mut v = f32::from(d2) / 127.0;
            if self.curve != 0 {
                v = v.powf(2f32.powf(f32::from(self.curve.clamp(-100, 100)) / 50.0));
            }
            v *= f32::from(self.velocity) / 100.0;
            d2 = (v * 127.0).round().clamp(1.0, 127.0) as u8;
            if self.fixed_velocity > 0 {
                d2 = self.fixed_velocity.min(127);
            }
        }
        if (0x80..0xF0).contains(&status) && (1..=16).contains(&self.channel) {
            status = kind | (self.channel - 1);
        }
        Some([status, d1, d2])
    }
}

impl MidiIn {
    /// All inputs, Omni: what a new instrument track takes.
    pub fn all() -> MidiIn {
        MidiIn::default()
    }

    /// How a track input menu names it: "MIDI: All Inputs" or "MIDI: <port>", with " (ch N)"
    /// for one channel.
    pub fn label(&self) -> String {
        let dev = self.device.as_deref().unwrap_or("All Inputs");
        if self.channel == 0 { format!("MIDI: {dev}") } else { format!("MIDI: {dev} (ch {})", self.channel) }
    }
}

/// A chain module named across tracks: `module` on `track`'s chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct ModuleRef {
    pub track: TrackId,
    pub module: ModuleId,
}

impl Route {
    /// The modules a `Modules` input reads, in source order (empty for any other route).
    pub fn module_sources(&self) -> &[ModuleRef] {
        match self {
            Route::Modules(m) => m,
            _ => &[],
        }
    }

    /// `Modules` of `refs` without duplicates, `None` when that leaves nothing.
    pub fn modules(refs: impl IntoIterator<Item = ModuleRef>) -> Route {
        let mut out: Vec<ModuleRef> = Vec::new();
        for r in refs {
            if !out.contains(&r) {
                out.push(r);
            }
        }
        if out.is_empty() { Route::None } else { Route::Modules(out) }
    }

    /// The route a reserved name always means, whatever paths and busses the session has:
    /// "none", "no input", "no output" (or nothing) → `None`; "main", "master", "out 1-2" →
    /// `Main`. Case-insensitive, surrounding spaces ignored. Named inputs and outputs can't take
    /// these names: a route menu or command could never reach them.
    pub fn reserved(name: &str) -> Option<Route> {
        const NONE: [&str; 4] = ["", "none", "no input", "no output"];
        const MAIN: [&str; 3] = ["main", "master", "out 1-2"];
        let n = name.trim();
        if NONE.iter().any(|r| r.eq_ignore_ascii_case(n)) {
            Some(Route::None)
        } else if MAIN.iter().any(|r| r.eq_ignore_ascii_case(n)) {
            Some(Route::Main)
        } else {
            None
        }
    }
}

/// Stable identity of a module in a channel chain (0 = unassigned). Automation, plugin instances
/// and meters are keyed by this, never by position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ModuleId(pub u32);

/// Longest allowed channel chain.
pub const MAX_MODULES: usize = 32;

/// Most knobs a module can pin to its strip face.
const MAX_KNOBS: usize = 4;

/// How wide the channel strips are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum StripSize {
    #[default]
    Auto,
    Wide,
    Slim,
}

/// A plugin instance on an insert slot.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Insert {
    /// Stable chain identity (0 until `Mixer::sanitize` assigns one).
    #[serde(default)]
    pub id: ModuleId,
    /// Parameter names pinned as knobs on the strip (at most 4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knobs: Vec<String>,
    /// Registry id from `soundcraft-dsp` (e.g. `eq7`, `comp`).
    pub plugin: String,
    #[serde(default, deserialize_with = "finite_params")]
    pub params: BTreeMap<String, f32>,
    /// Host-side memory of the plugin's derived-parameter logic (`soundcraft_dsp::derive`),
    /// e.g. FireStation Dist's per-circuit Drive/Tone/Level. Keys are the plugin's own.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", deserialize_with = "finite_params")]
    pub memory: BTreeMap<String, f32>,
    #[serde(default)]
    pub bypass: bool,
    #[serde(default = "yes")]
    pub active: bool,
    /// Preset name shown in the plugin window header.
    #[serde(default)]
    pub preset: String,
    /// Opaque plugin state (third-party plugins: CLAP state / VST3 component + controller
    /// state), standard base64. Restored into a new instance before it first processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// The session audio source a sample-playing instrument (the built-in Sampler) plays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<SourceId>,
    /// The track whose signal keys this module (its sidechain input), for plugins that declare
    /// `PluginInfo::sidechain`. `None`: no key, the plugin keys from its own input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidechain: Option<SidechainSource>,
    /// What a built-in plugin's `state` works out to (FireStation CleanQ's ceiling on its grid),
    /// made by the engine off the audio thread whenever `state` is set or a session opens, and
    /// copied into the running plugin between blocks. Not saved: it is derived from `state`.
    #[serde(skip)]
    pub prepared: Option<PreparedState>,
    /// Files the plugin works from, by the plugin's own key (FireStation Amp's cabinet IR per amp:
    /// `irPathTwin` …): `bundled:<name>` for one shipped with SoundCraft, else a file path. The
    /// engine makes each ready (`Session::assets`) and the mix engine hands it over.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, String>,
    /// Session audio sources a sample-playing instrument plays, by the plugin's own key (Juke's
    /// pad samples, `p3.v2.r1`: pad 3, velocity layer 2, round-robin 1). Like `sample`, they live
    /// in the session's pool and `Audio Files/`, so a kit travels with its project; the mix
    /// engine hands each over with `Plugin::set_asset`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub samples: BTreeMap<String, SourceId>,
    /// A drum kit's names for its pads and their families (Juke), shown on the pads and in the
    /// step grid; the kit's sound itself is the parameters and `samples`. `None` for every other
    /// module, and for a kit still on its defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kit: Option<KitLayout>,
    /// The effects a kit instrument hosts on its buses (Juke's bus A, bus B and output slots),
    /// each a module in its own right. Empty for every other module.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kit_fx: Vec<KitFx>,
    /// The session-level instance link group the module is in (FireStation LevelUp's IN/OUT
    /// pairs, Shale's Kit Link), for plugins that declare `PluginInfo::link`: every module in a
    /// group, on any track, shares one value bus. `None`: not linked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_group: Option<u32>,
    /// The module runs after the fader and mute instead of before them (Cubase's post-fader
    /// slots; on the master, Pro Tools' place for its inserts): it is not in the pre-fader copy
    /// (pre-fader sends, keys, meters), is never frozen, and a commit keeps it live on the new
    /// track.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub post_fader: bool,
    /// Multi-mono: on a track of more than one channel the module runs one mono instance per
    /// channel, all taking its parameters and automation (linked), as Pro Tools' multi-mono
    /// plug-ins. Its stored state is the first instance's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multi_mono: bool,
    /// The insert's wet/dry mix, 0 (its input only) to 1 (its output only, the default): the
    /// mixer blends the module's output with its input, delayed by its latency, so any effect
    /// can be used in parallel. Clamped by the mixer.
    #[serde(default = "unity", skip_serializing_if = "is_unity")]
    pub mix: f32,
    /// Which loading of `state` the running plugin instance should have: a hosted preset loaded
    /// into the module (`mix.module_preset`) changes it, so the instance is created again from
    /// `state`, and undo (which puts the previous value back) does the same with the previous
    /// state. 0 for a module whose state was never loaded that way. Not saved.
    #[serde(skip)]
    pub state_load: u64,
    /// The device as another DAW wrote it, for a module that came in from a DAWproject file
    /// ([`ForeignDevice`]): a placeholder for a plugin or built-in device that is not here keeps
    /// it, so exporting the project again writes it back unchanged. `None` for every other module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign: Option<ForeignDevice>,
}

/// A device read from a DAWproject file, kept as written ([`Insert::foreign`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForeignDevice {
    /// The device element exactly as the file had it (`<Vst3Plugin …>…</Vst3Plugin>`,
    /// `<BuiltinDevice …>`), its `State` path included.
    pub xml: String,
    /// The display name (`deviceName`).
    pub name: String,
    /// The state file's bytes as found in the archive (standard base64), if it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// A fingerprint of [`Insert::state`] as the import set it: when the module's state still
    /// matches, the export writes the original file back byte for byte.
    #[serde(default)]
    pub state_print: u64,
}

/// `Insert::assets` keys of reference audio slots (`PluginInfo::references`): a whole audio file
/// at the session rate, not a prepared asset. `soundcraft_dsp::REFERENCE_KEYS` names the same.
pub const REFERENCE_KEYS: [&str; 4] = ["ref0", "ref1", "ref2", "ref3"];

/// Whether an `Insert::assets` key names a reference audio slot.
pub fn is_reference_key(key: &str) -> bool {
    REFERENCE_KEYS.contains(&key)
}

/// The `Insert::assets` key of Vault's impulse response: a reverb response made ready for its
/// convolution. `soundcraft_dsp::plugins::vault::IR_KEY` names the same.
pub const SPACE_IR_KEY: &str = "vaultIr";

/// Numbers made ready for a plugin from its stored state (see [`Insert::prepared`]), shared by
/// every snapshot of the document. `rev` tells one preparation from another without comparing
/// the numbers (the mix engine hands a new one over once); equality compares only the numbers.
#[derive(Debug, Clone)]
pub struct PreparedState {
    pub rev: u64,
    pub data: std::sync::Arc<[f64]>,
}

static PREPARED_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl PreparedState {
    /// A new preparation (a fresh `rev`, never 0).
    pub fn new(data: Vec<f64>) -> Self {
        let rev = PREPARED_REV.fetch_add(1, std::sync::atomic::Ordering::Relaxed).max(1);
        PreparedState { rev, data: data.into() }
    }
}

impl PartialEq for PreparedState {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

/// Where a module's sidechain key comes from: another track's signal, after its own chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SidechainSource {
    pub track: TrackId,
    /// Tap the source before its fader (after its inserts) instead of after fader and mute.
    #[serde(default)]
    pub pre_fader: bool,
}

fn yes() -> bool {
    true
}

fn unity() -> f32 {
    1.0
}

fn is_unity(v: &f32) -> bool {
    *v == 1.0
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_zero_u16(v: &u16) -> bool {
    *v == 0
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// The longest per-track delay, samples (a second at 192 kHz).
pub const MAX_TRACK_DELAY: u32 = 192_000;

/// A drum family: a pad's colour in the kit view and the step grid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrumFamily {
    Kick,
    Snare,
    Clap,
    Hat,
    Tom,
    Cymbal,
    #[default]
    Percussion,
}

impl DrumFamily {
    pub const ALL: [DrumFamily; 7] =
        [DrumFamily::Kick, DrumFamily::Snare, DrumFamily::Clap, DrumFamily::Hat, DrumFamily::Tom, DrumFamily::Cymbal, DrumFamily::Percussion];

    pub fn id(self) -> &'static str {
        match self {
            DrumFamily::Kick => "kick",
            DrumFamily::Snare => "snare",
            DrumFamily::Clap => "clap",
            DrumFamily::Hat => "hat",
            DrumFamily::Tom => "tom",
            DrumFamily::Cymbal => "cymbal",
            DrumFamily::Percussion => "percussion",
        }
    }

    pub fn from_id(id: &str) -> Option<DrumFamily> {
        DrumFamily::ALL.into_iter().find(|f| f.id() == id)
    }
}

/// One pad's name and family in a [`KitLayout`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KitPad {
    pub name: String,
    #[serde(default)]
    pub family: DrumFamily,
}

/// A drum kit's name and its pads' names and families, in pad order.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KitLayout {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub pads: Vec<KitPad>,
}

/// One of a kit instrument's effect buses (Juke): the two send buses and the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KitBus {
    A,
    B,
    Out,
}

impl KitBus {
    pub const ALL: [KitBus; 3] = [KitBus::A, KitBus::B, KitBus::Out];

    pub fn id(self) -> &'static str {
        match self {
            KitBus::A => "a",
            KitBus::B => "b",
            KitBus::Out => "out",
        }
    }

    pub fn from_id(id: &str) -> Option<KitBus> {
        KitBus::ALL.into_iter().find(|b| b.id() == id)
    }
}

/// An effect in one of a kit instrument's bus slots: a module like any other (its id is
/// [`KitFx::module_id`] of its place, outside the range chain modules use), hosted by the
/// instrument in its own signal path.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KitFx {
    pub bus: KitBus,
    /// The slot on its bus, from 0 (below [`KitFx::SLOTS`]).
    pub slot: u8,
    pub insert: Insert,
}

/// The first module id kit effects use; chain modules stay below it.
pub const KIT_FX_BASE: u32 = u32::MAX - 63;

/// The id automation lanes and mappings give a track's instrument (`Track::instrument`), which
/// is not in the chain: just below the kit effects' ids, never handed to a chain module.
pub const INSTRUMENT_MODULE: ModuleId = ModuleId(KIT_FX_BASE - 1);

impl KitFx {
    /// Effect slots per bus.
    pub const SLOTS: usize = 4;

    /// The place's index over every bus (`bus × SLOTS + slot`).
    pub fn index(bus: KitBus, slot: usize) -> usize {
        let b = match bus {
            KitBus::A => 0,
            KitBus::B => 1,
            KitBus::Out => 2,
        };
        b * Self::SLOTS + slot.min(Self::SLOTS - 1)
    }

    /// The module id of the effect at `index` ([`KitFx::index`]).
    pub fn module_id(index: usize) -> ModuleId {
        ModuleId(KIT_FX_BASE.saturating_add(u32::try_from(index).unwrap_or(63).min(63)))
    }

    /// The place a kit effect's module id names: (bus, slot).
    pub fn place(id: ModuleId) -> Option<(KitBus, usize)> {
        let i = usize::try_from(id.0.checked_sub(KIT_FX_BASE)?).ok()?;
        let bus = *KitBus::ALL.get(i / Self::SLOTS)?;
        Some((bus, i % Self::SLOTS))
    }
}

impl KitLayout {
    /// Most pads a layout names (Juke has 16; room for a bigger grid later).
    pub const MAX_PADS: usize = 32;

    pub fn sanitize(&mut self) {
        self.name = self.name.chars().take(64).collect();
        self.pads.truncate(Self::MAX_PADS);
        for p in &mut self.pads {
            p.name = p.name.chars().take(32).collect();
        }
    }
}

impl Insert {
    pub fn new(plugin: impl Into<String>) -> Self {
        Insert {
            id: ModuleId::default(),
            knobs: Vec::new(),
            plugin: plugin.into(),
            params: BTreeMap::new(),
            memory: BTreeMap::new(),
            bypass: false,
            active: true,
            preset: String::from("<factory default>"),
            state: None,
            sample: None,
            sidechain: None,
            prepared: None,
            assets: BTreeMap::new(),
            samples: BTreeMap::new(),
            kit: None,
            kit_fx: Vec::new(),
            link_group: None,
            post_fader: false,
            multi_mono: false,
            mix: 1.0,
            state_load: 0,
            foreign: None,
        }
    }

    /// Every session source the module plays: its `sample` and its keyed `samples`.
    /// The kit effect at (bus, slot), if the slot holds one.
    pub fn kit_effect(&self, bus: KitBus, slot: usize) -> Option<&KitFx> {
        self.kit_fx.iter().find(|f| f.bus == bus && usize::from(f.slot) == slot)
    }

    /// Keeps the kit effects valid: real slots, one effect each, in place order, each with its
    /// place's module id and nothing nested.
    pub fn sanitize_kit_fx(&mut self) {
        self.kit_fx.retain(|f| usize::from(f.slot) < KitFx::SLOTS);
        self.kit_fx.sort_by_key(|f| KitFx::index(f.bus, usize::from(f.slot)));
        self.kit_fx.dedup_by_key(|f| KitFx::index(f.bus, usize::from(f.slot)));
        for f in &mut self.kit_fx {
            f.insert.id = KitFx::module_id(KitFx::index(f.bus, usize::from(f.slot)));
            f.insert.kit_fx.clear();
            f.insert.kit = None;
        }
    }

    pub fn sources(&self) -> impl Iterator<Item = SourceId> + '_ {
        self.sample.into_iter().chain(self.samples.values().copied())
    }

    /// The decoded plugin state (`None` when absent or malformed).
    pub fn state_bytes(&self) -> Option<Vec<u8>> {
        self.state.as_deref().and_then(crate::b64::decode)
    }

    /// Stores a plugin state blob (base64-encoded); an empty blob clears it.
    pub fn set_state_bytes(&mut self, data: &[u8]) {
        self.state = (!data.is_empty()).then(|| crate::b64::encode(data));
    }
}

/// A send to a bus.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SendSlot {
    pub target: Route,
    pub level_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub pre_fader: bool,
    #[serde(default = "yes")]
    pub follow_main_pan: bool,
    /// Surround position into a multichannel bus (None = follow the track / stereo pan).
    #[serde(default)]
    pub surround: Option<SurroundPan>,
    /// The send's pan follows the track's main pan (and its automation) instead of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pan_follows_main: bool,
    /// An additional output (Pro Tools'): a post-fader copy of the main output into `target`, at
    /// the fader's level and with the main pan, whatever the send's own level, pan, mute and
    /// lanes say. It takes a send slot.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub output: bool,
}

impl SendSlot {
    pub fn new(target: Route) -> Self {
        SendSlot {
            target,
            level_db: -144.0,
            pan: 0.0,
            mute: false,
            pre_fader: false,
            follow_main_pan: true,
            surround: None,
            pan_follows_main: false,
            output: false,
        }
    }
}

/// The channel strip.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Mixer {
    pub volume_db: f32,
    /// One panner per source channel for mono/stereo (stereo tracks: two panners, default hard L/R).
    pub pan: Vec<f32>,
    pub mute: bool,
    pub solo: bool,
    #[serde(default)]
    pub solo_safe: bool,
    #[serde(default)]
    pub record_arm: bool,
    /// Record safe: the track cannot be record-armed until this is turned off (Pro Tools).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub record_safe: bool,
    #[serde(default)]
    pub input_monitor: InputMonitor,
    /// Record with inserts (print): an armed audio track's take is what its trim, polarity and
    /// inserts make of the input (after them, before the fader), not the input as it came in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub record_inserts: bool,
    /// Every channel's polarity inverted.
    #[serde(default)]
    pub phase_invert: bool,
    /// Single channels' polarity inverted, bit `c` for channel `c` (with `phase_invert` off;
    /// see [`Mixer::inverts`]).
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub phase_invert_channels: u16,
    pub input: Route,
    pub output: Route,
    /// The channel chain, in signal order (dense, at most [`MAX_MODULES`]). `null` entries
    /// (hand-edited or legacy files) are dropped on load.
    #[serde(deserialize_with = "dense_chain")]
    pub inserts: Vec<Insert>,
    pub sends: Vec<Option<SendSlot>>,
    pub automation_mode: AutomationMode,
    /// Input gain / trim, dB.
    #[serde(default)]
    pub trim_db: f32,
    /// VCA master that controls this track, if any (track id of the VCA).
    #[serde(default)]
    pub vca: Option<u64>,
    /// Surround panner, used when the output is multichannel (None = the stereo pan to L/R).
    #[serde(default)]
    pub surround: Option<SurroundPan>,
    /// Next module id to hand out (always above every id in the chain once sanitised).
    #[serde(default)]
    pub next_module: u32,
    #[serde(default)]
    pub strip_size: StripSize,
    /// Stereo width of a stereo strip, applied before its pans: 1 as it comes, 0 mono, −1 the
    /// sides swapped (−1..=1). Other formats ignore it.
    #[serde(default = "unity", skip_serializing_if = "is_unity")]
    pub width: f32,
    /// Balance of a stereo strip's output, after its pans: −1 only the left side, +1 only the
    /// right, 0 both at full level. Other formats ignore it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub balance: f32,
    /// Delay of the strip's signal, samples (0..=[`MAX_TRACK_DELAY`]), before its inserts. Delay
    /// compensation leaves it in: it moves the track against the others on purpose.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub delay: u32,
    /// How a stereo track's two pans move together when one is moved.
    #[serde(default, skip_serializing_if = "PanLink::is_off")]
    pub pan_link: PanLink,
}

/// How a stereo track's left and right pans follow each other (Pro Tools' link and inverse link).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanLink {
    /// Each pan moves alone.
    #[default]
    Off,
    /// Moving one moves the other by as much, the same way: the pair keeps its spread.
    Link,
    /// Moving one moves the other by as much the other way: the image widens or narrows about
    /// its centre.
    Mirror,
}

impl PanLink {
    pub fn is_off(&self) -> bool {
        *self == PanLink::Off
    }

    pub fn id(self) -> &'static str {
        match self {
            PanLink::Off => "off",
            PanLink::Link => "link",
            PanLink::Mirror => "mirror",
        }
    }

    pub fn parse(s: &str) -> Option<PanLink> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "independent" => Some(PanLink::Off),
            "link" | "linked" => Some(PanLink::Link),
            "mirror" | "mirrored" | "inverse" => Some(PanLink::Mirror),
            _ => None,
        }
    }

    /// Where the other pan goes when one moves by `d` from where it was.
    pub fn follow(self, other: f32, d: f32) -> f32 {
        match self {
            PanLink::Off => other,
            PanLink::Link => (other + d).clamp(-1.0, 1.0),
            PanLink::Mirror => (other - d).clamp(-1.0, 1.0),
        }
    }
}

impl Mixer {
    pub fn new(format: ChannelFormat) -> Self {
        let pan = match format {
            ChannelFormat::Mono => vec![0.0],
            ChannelFormat::Stereo => vec![-1.0, 1.0],
            _ => Vec::new(),
        };
        Mixer {
            volume_db: 0.0,
            pan,
            mute: false,
            solo: false,
            solo_safe: false,
            record_safe: false,
            record_arm: false,
            input_monitor: InputMonitor::Auto,
            record_inserts: false,
            phase_invert: false,
            phase_invert_channels: 0,
            input: Route::None,
            output: Route::Main,
            inserts: Vec::new(),
            sends: vec![None; SEND_SLOTS],
            automation_mode: AutomationMode::Read,
            trim_db: 0.0,
            vca: None,
            surround: None,
            next_module: 1,
            strip_size: StripSize::Auto,
            width: 1.0,
            balance: 0.0,
            delay: 0,
            pan_link: PanLink::Off,
        }
    }

    /// Whether channel `ch`'s polarity is inverted: every channel's when `phase_invert`, else
    /// the channels `phase_invert_channels` names.
    pub fn inverts(&self, ch: usize) -> bool {
        self.phase_invert || (ch < 16 && self.phase_invert_channels & (1 << ch) != 0)
    }

    /// Whether any channel's polarity is inverted.
    pub fn inverts_any(&self) -> bool {
        self.phase_invert || self.phase_invert_channels != 0
    }

    /// Whether the strip hears its hardware input now: On always, Off never, Auto when armed
    /// and the transport is stopped or recording.
    pub fn hears_input(&self, stopped: bool, recording: bool) -> bool {
        match self.input_monitor {
            InputMonitor::On => true,
            InputMonitor::Off => false,
            InputMonitor::Auto => self.record_arm && (stopped || recording),
        }
    }

    /// Whether the strip could hear its input in some transport state (the input stream is
    /// needed).
    pub fn may_hear_input(&self) -> bool {
        self.input_monitor == InputMonitor::On || (self.record_arm && self.input_monitor != InputMonitor::Off)
    }

    /// The module with this id.
    pub fn module(&self, id: ModuleId) -> Option<&Insert> {
        self.inserts.iter().find(|i| i.id == id)
    }

    pub fn module_mut(&mut self, id: ModuleId) -> Option<&mut Insert> {
        self.inserts.iter_mut().find(|i| i.id == id)
    }

    /// Position of the module with this id in the chain.
    pub fn module_index(&self, id: ModuleId) -> Option<usize> {
        self.inserts.iter().position(|i| i.id == id)
    }

    /// Inserts `ins` at `at` (clamped to the chain length; `None` = the end) under a fresh id,
    /// which it returns (`ins.id` is overwritten).
    pub fn add_module(&mut self, mut ins: Insert, at: Option<usize>) -> Result<ModuleId, ChainError> {
        if self.inserts.len() >= MAX_MODULES {
            return Err(ChainError::Full);
        }
        // Keep the counter above every id in the chain even if the caller never sanitised.
        let max_id = self.inserts.iter().map(|i| i.id.0).max().unwrap_or(0);
        if max_id == u32::MAX {
            return Err(ChainError::OutOfIds);
        }
        self.next_module = self.next_module.max(max_id.saturating_add(1));
        let id = self.fresh_module_id().ok_or(ChainError::OutOfIds)?;
        ins.id = id;
        let at = at.unwrap_or(self.inserts.len()).min(self.inserts.len());
        self.inserts.insert(at, ins);
        Ok(id)
    }

    /// Takes the module with this id out of the chain: its former position and the module.
    pub fn remove_module(&mut self, id: ModuleId) -> Option<(usize, Insert)> {
        let at = self.module_index(id)?;
        Some((at, self.inserts.remove(at)))
    }

    /// Moves the module with this id to position `to` (clamped to the chain). False when there
    /// is no such module.
    pub fn move_module(&mut self, id: ModuleId, to: usize) -> bool {
        let Some(from) = self.module_index(id) else { return false };
        let ins = self.inserts.remove(from);
        let to = to.min(self.inserts.len());
        self.inserts.insert(to, ins);
        true
    }

    /// Hands out the next unused id; `None` once the counter is exhausted.
    fn fresh_module_id(&mut self) -> Option<ModuleId> {
        // Ids from `KIT_FX_BASE` up are the kit effects', the one below it the instrument's.
        if self.next_module >= INSTRUMENT_MODULE.0 {
            return None;
        }
        let id = self.next_module.max(1);
        self.next_module = id.saturating_add(1);
        Some(ModuleId(id))
    }

    /// Gives every module a unique non-zero id and keeps `next_module` above all of them.
    fn assign_module_ids(&mut self) {
        let mut seen = std::collections::HashSet::new();
        let mut max_id = 0u32;
        let mut need = Vec::new();
        for (idx, ins) in self.inserts.iter().enumerate() {
            if ins.id.0 == 0 || ins.id.0 >= INSTRUMENT_MODULE.0 || !seen.insert(ins.id) {
                need.push(idx);
            } else {
                max_id = max_id.max(ins.id.0);
            }
        }
        self.next_module = self.next_module.max(max_id.saturating_add(1));
        let mut exhausted = max_id >= INSTRUMENT_MODULE.0 - 1;
        if !exhausted {
            for idx in need {
                match self.fresh_module_id() {
                    Some(id) => {
                        if let Some(ins) = self.inserts.get_mut(idx) {
                            ins.id = id;
                        }
                    }
                    None => {
                        exhausted = true;
                        break;
                    }
                }
            }
        }
        if exhausted {
            self.next_module = 1;
            for idx in 0..self.inserts.len() {
                let Some(id) = self.fresh_module_id() else { break };
                if let Some(ins) = self.inserts.get_mut(idx) {
                    ins.id = id;
                }
            }
        }
    }

    /// Clamp everything into range (after deserialising untrusted files).
    pub fn sanitize(&mut self) {
        self.volume_db = finite_or(self.volume_db, 0.0).clamp(-144.0, 12.0);
        for p in &mut self.pan {
            *p = finite_or(*p, 0.0).clamp(-1.0, 1.0);
        }
        self.inserts.truncate(MAX_MODULES);
        for ins in &mut self.inserts {
            ins.knobs.truncate(MAX_KNOBS);
        }
        self.assign_module_ids();
        self.sends.resize(SEND_SLOTS, None);
        for s in self.sends.iter_mut().flatten() {
            s.level_db = finite_or(s.level_db, -144.0).clamp(-144.0, 12.0);
            s.pan = finite_or(s.pan, 0.0).clamp(-1.0, 1.0);
            if let Some(sp) = &mut s.surround {
                sp.sanitize();
            }
        }
        if let Some(sp) = &mut self.surround {
            sp.sanitize();
        }
        self.trim_db = finite_or(self.trim_db, 0.0).clamp(-144.0, 24.0);
        // A musician or a module is a source only, and the band has six.
        if matches!(self.output, Route::Musician(_) | Route::Modules(_) | Route::Midi(_)) {
            self.output = Route::None;
        }
        if let Route::Midi(m) = &mut self.input
            && m.channel > 16
        {
            m.channel = 0;
        }
        if matches!(self.input, Route::Musician(n) if n >= 6) {
            self.input = Route::None;
        }
    }
}

impl crate::Session {
    /// The main mix format: the format of the first (main) output path, at least stereo and at
    /// most 16 channels (wider formats fall back to stereo).
    pub fn main_format(&self) -> ChannelFormat {
        match self.outputs.first().map(|o| o.format) {
            Some(f) if (2..=16).contains(&f.channels()) => f,
            _ => ChannelFormat::Stereo,
        }
    }
}

pub(crate) fn finite_or(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

/// Why a module could not be added to a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainError {
    /// The chain already holds [`MAX_MODULES`] modules.
    Full,
    /// The track's module id counter is exhausted.
    OutOfIds,
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainError::Full => write!(f, "the chain is full ({MAX_MODULES} modules)"),
            ChainError::OutOfIds => write!(f, "this track has run out of module ids"),
        }
    }
}

impl std::error::Error for ChainError {}

/// A chain saved with `null` holes (hand-edited, or not yet migrated) loads dense; anything past
/// [`MAX_MODULES`] is dropped by `Mixer::sanitize`.
fn dense_chain<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Insert>, D::Error> {
    let chain: Vec<Option<Insert>> = serde::Deserialize::deserialize(d)?;
    Ok(chain.into_iter().flatten().collect())
}

/// Parameters saved as `null` (how JSON writes NaN and infinity) or beyond `f32` are dropped, so
/// they keep their defaults instead of failing the whole session.
fn finite_params<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, f32>, D::Error> {
    let params: BTreeMap<String, Option<f64>> = serde::Deserialize::deserialize(d)?;
    Ok(params.into_iter().filter_map(|(k, v)| Some((k, v.map(|v| v as f32).filter(|v| v.is_finite())?))).collect())
}

/// Fader taper used by Pro Tools-style faders: position 0..1 ↔ dB (-inf..+12).
pub fn fader_pos_to_db(pos: f32) -> f32 {
    let p = finite_or(pos, 0.0).clamp(0.0, 1.0);
    if p <= 0.0 {
        return -144.0;
    }
    // Piecewise: top 75 % covers -24..+12 linearly-ish, the bottom compresses to -inf.
    if p >= 0.25 { -24.0 + (p - 0.25) / 0.75 * 36.0 } else { -24.0 - (1.0 - p / 0.25).powf(1.5) * 120.0 }
}

pub fn fader_db_to_pos(db: f32) -> f32 {
    let db = finite_or(db, -144.0);
    if db <= -144.0 {
        return 0.0;
    }
    if db >= -24.0 {
        (0.25 + (db.min(12.0) + 24.0) / 36.0 * 0.75).min(1.0)
    } else {
        let x = ((-24.0 - db) / 120.0).clamp(0.0, 1.0).powf(1.0 / 1.5);
        ((1.0 - x) * 0.25).max(0.0)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn kit_effect_ids_name_their_place_and_never_meet_chain_ids() {
        for bus in KitBus::ALL {
            for slot in 0..KitFx::SLOTS {
                let id = KitFx::module_id(KitFx::index(bus, slot));
                assert_eq!(KitFx::place(id), Some((bus, slot)));
            }
        }
        assert_eq!(KitFx::place(ModuleId(1)), None);
        assert_eq!(KitFx::place(ModuleId(KIT_FX_BASE + 12)), None);
        // A chain module claiming a kit id gets a chain id.
        let mut m = Mixer::new(ChannelFormat::Stereo);
        let mut ins = Insert::new("eq_7band");
        ins.id = ModuleId(KIT_FX_BASE + 1);
        m.inserts.push(ins);
        m.sanitize();
        assert!(m.inserts[0].id.0 < KIT_FX_BASE);
    }

    #[test]
    fn kit_effects_sanitize_to_one_per_slot() {
        let mut ins = Insert::new("juke");
        let fx = |bus, slot: u8| KitFx { bus, slot, insert: Insert::new("reverb") };
        ins.kit_fx = vec![fx(KitBus::Out, 1), fx(KitBus::A, 0), fx(KitBus::A, 0), fx(KitBus::B, 9)];
        ins.kit_fx[1].insert.kit_fx.push(fx(KitBus::A, 2));
        ins.sanitize_kit_fx();
        let places: Vec<_> = ins.kit_fx.iter().map(|f| (f.bus, f.slot)).collect();
        assert_eq!(places, vec![(KitBus::A, 0), (KitBus::Out, 1)]);
        assert!(ins.kit_fx.iter().all(|f| f.insert.kit_fx.is_empty() && KitFx::place(f.insert.id) == Some((f.bus, usize::from(f.slot)))));
        assert!(ins.kit_effect(KitBus::Out, 1).is_some());
    }
    use super::*;

    #[test]
    fn chain_ops() {
        let mut m = Mixer::new(ChannelFormat::Stereo);
        let a = m.add_module(Insert::new("gain"), None).unwrap();
        let b = m.add_module(Insert::new("compressor"), None).unwrap();
        let c = m.add_module(Insert::new("eq_7band"), Some(0)).unwrap();
        assert_eq!(m.inserts.iter().map(|i| i.id).collect::<Vec<_>>(), [c, a, b]);
        assert!(m.move_module(c, 99));
        assert_eq!(m.inserts.iter().map(|i| i.id).collect::<Vec<_>>(), [a, b, c]);
        assert_eq!(m.remove_module(b).map(|(i, _)| i), Some(1));
        for _ in 0..40 {
            let _ = m.add_module(Insert::new("gain"), None);
        }
        assert_eq!(m.inserts.len(), MAX_MODULES);
        assert!(matches!(m.add_module(Insert::new("gain"), None), Err(ChainError::Full)));
    }

    #[test]
    fn sanitize_assigns_unique_module_ids() {
        let mut m = Mixer::new(ChannelFormat::Stereo);
        m.inserts.push(Insert::new("gain"));
        m.inserts.push(Insert { id: ModuleId(7), ..Insert::new("gain") });
        m.inserts.push(Insert { id: ModuleId(7), ..Insert::new("trim") });
        m.sanitize();
        let ids: Vec<ModuleId> = m.inserts.iter().map(|i| i.id).collect();
        assert!(ids.iter().all(|i| i.0 != 0));
        let mut d = ids.clone();
        d.sort();
        d.dedup();
        assert_eq!(d.len(), 3);
        assert!(m.next_module > ids.iter().map(|i| i.0).max().unwrap());
        assert_eq!(m.module_index(ids[1]), Some(1));
    }

    #[test]
    fn sanitize_repairs_hostile_chains() {
        let mut m = Mixer::new(ChannelFormat::Mono);
        m.next_module = u32::MAX;
        m.inserts.push(Insert { id: ModuleId(u32::MAX), knobs: vec!["a".into(); 9], ..Insert::new("gain") });
        m.inserts.push(Insert::new("gain"));
        m.sanitize();
        let ids: Vec<u32> = m.inserts.iter().map(|i| i.id.0).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(m.next_module, 3);
        assert_eq!(m.inserts[0].knobs.len(), 4);
        // Adding behind an exhausted id space fails cleanly instead of reusing an id.
        let mut full = Mixer::new(ChannelFormat::Mono);
        full.inserts.push(Insert { id: ModuleId(u32::MAX), ..Insert::new("gain") });
        assert_eq!(full.add_module(Insert::new("gain"), None), Err(ChainError::OutOfIds));
    }

    #[test]
    fn insert_sidechain_round_trips_and_old_inserts_have_none() {
        let old: Insert = serde_json::from_str(r#"{"plugin":"comp"}"#).unwrap();
        assert_eq!(old.sidechain, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("sidechain"));
        let mut i = Insert::new("comp");
        i.sidechain = Some(SidechainSource { track: TrackId(7), pre_fader: true });
        let json = serde_json::to_string(&i).unwrap();
        let back: Insert = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sidechain, i.sidechain);
        // `pre_fader` is optional in hand-written files.
        let hand: Insert = serde_json::from_str(r#"{"plugin":"comp","sidechain":{"track":3}}"#).unwrap();
        assert_eq!(hand.sidechain, Some(SidechainSource { track: TrackId(3), pre_fader: false }));
    }

    #[test]
    fn old_inserts_without_ids_still_load() {
        let i: Insert = serde_json::from_str(r#"{"plugin":"eq_7band"}"#).unwrap();
        assert_eq!(i.id, ModuleId(0));
        assert!(i.knobs.is_empty());
        assert!(!serde_json::to_string(&i).unwrap().contains("knobs"));
    }

    #[test]
    fn insert_state_round_trips_through_json() {
        let mut i = Insert::new("vst3:00");
        assert_eq!(i.state_bytes(), None);
        let blob: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        i.set_state_bytes(&blob);
        let json = serde_json::to_string(&i).unwrap();
        let back: Insert = serde_json::from_str(&json).unwrap();
        assert_eq!(back.state_bytes().unwrap(), blob);
        i.set_state_bytes(&[]);
        assert_eq!(i.state, None);
        // Old sessions have no `state`; stateless inserts don't write one.
        let old: Insert = serde_json::from_str(r#"{"plugin":"eq_7band"}"#).unwrap();
        assert_eq!(old.state, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("state"));
        // A hostile state string decodes to nothing rather than failing the load.
        let bad: Insert = serde_json::from_str(r#"{"plugin":"x","state":"!!!"}"#).unwrap();
        assert_eq!(bad.state_bytes(), None);
    }

    #[test]
    fn params_saved_as_null_or_out_of_range_are_dropped() {
        let mut i = Insert::new("eq_7band");
        i.params.extend([("gain".to_string(), f32::NAN), ("q".to_string(), 0.7)]);
        let back: Insert = serde_json::from_str(&serde_json::to_string(&i).unwrap()).unwrap();
        assert_eq!(back.params, BTreeMap::from([("q".to_string(), 0.7)]));
        let big: Insert = serde_json::from_str(r#"{"plugin":"x","params":{"gain":1e39,"q":0.5}}"#).unwrap();
        assert_eq!(big.params, BTreeMap::from([("q".to_string(), 0.5)]));
    }

    #[test]
    fn fader_taper_round_trips() {
        for db in [-100.0f32, -60.0, -24.0, -10.0, 0.0, 6.0, 12.0] {
            let p = fader_db_to_pos(db);
            assert!((fader_pos_to_db(p) - db).abs() < 0.05, "{db} → {p}");
        }
        assert_eq!(fader_pos_to_db(0.0), -144.0);
        assert_eq!(fader_db_to_pos(f32::NAN), 0.0);
    }

    #[test]
    fn mixer_defaults_and_sanitize() {
        let mut m = Mixer::new(ChannelFormat::Stereo);
        assert_eq!(m.pan, vec![-1.0, 1.0]);
        m.volume_db = f32::INFINITY;
        m.inserts = vec![Insert::new("gain"); MAX_MODULES + 5];
        m.sanitize();
        assert_eq!(m.volume_db, 0.0);
        assert_eq!(m.inserts.len(), MAX_MODULES);
        // `null` holes in a saved chain load dense.
        let mut v = serde_json::to_value(Mixer::new(ChannelFormat::Mono)).unwrap();
        v["inserts"] = serde_json::json!([null, {"plugin": "gain"}, null, {"plugin": "trim"}]);
        let back: Mixer = serde_json::from_value(v).unwrap();
        assert_eq!(back.inserts.iter().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), ["gain", "trim"]);
    }

    /// Mix bug 9: loudness weighted only a 6-channel layout, as 5.1, whatever it was.
    #[test]
    fn loudness_weights_follow_the_speakers() {
        assert_eq!(ChannelFormat::Surround51.loudness_weights(), [1.0, 1.0, 1.0, 0.0, 1.41, 1.41]);
        // 7.1: L R C LFE Lrs Rrs Lss Rss: the rears at ±150° are not boosted, the sides are.
        assert_eq!(ChannelFormat::Surround71.loudness_weights(), [1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.41, 1.41]);
        // 6.0 has six channels but no LFE.
        assert_eq!(ChannelFormat::Surround60.loudness_weights(), [1.0, 1.0, 1.0, 1.41, 1.41, 1.0]);
        let w = ChannelFormat::Atmos714.loudness_weights();
        assert_eq!(w.len(), 12);
        assert_eq!(&w[8..], [1.0; 4], "heights count once");
        assert_eq!(ChannelFormat::Ambisonics(1).loudness_weights(), [1.0; 4]);
    }

    #[test]
    fn speaker_layouts_match_channel_counts_and_masks() {
        for f in ChannelFormat::ALL {
            if !f.is_ambisonic() {
                assert_eq!(f.speakers().len(), f.channels(), "{f:?}");
                // Mask bits are unique and in ascending channel order.
                let bits: Vec<u32> = f.speakers().iter().map(|s| s.wave_bit()).filter(|b| *b != 0).collect();
                assert!(bits.windows(2).all(|w| w[0] < w[1]), "{f:?} not in WAV order");
            }
        }
        assert_eq!(ChannelFormat::Surround51.channel_mask(), 0x3F);
        assert_eq!(ChannelFormat::Surround71.channel_mask(), 0x63F);
        assert_eq!(ChannelFormat::Atmos714.channel_mask(), 0x2_D63F);
        assert_eq!(ChannelFormat::Ambisonics(1).channel_mask(), 0);
    }

    #[test]
    fn surround_pan_defaults_and_old_sessions() {
        let m = Mixer::new(ChannelFormat::Mono);
        assert_eq!(m.surround, None);
        // A mixer saved before surround existed still loads.
        let mut v = serde_json::to_value(&m).unwrap();
        v.as_object_mut().unwrap().remove("surround");
        let back: Mixer = serde_json::from_value(v).unwrap();
        assert_eq!(back.surround, None);
        let mut sp: SurroundPan = serde_json::from_str(r#"{"x": 5.0}"#).unwrap();
        sp.sanitize();
        assert_eq!(sp.x, 1.0);
        assert_eq!(sp.center, 100.0);
    }

    #[test]
    fn formats() {
        assert_eq!(ChannelFormat::for_channels(6), ChannelFormat::Surround51);
        assert_eq!(ChannelFormat::from_id("stereo"), Some(ChannelFormat::Stereo));
    }

    #[test]
    fn old_input_monitor_bools_load_as_on_and_auto() {
        let load = |v: serde_json::Value| -> InputMonitor { serde_json::from_value(v).unwrap() };
        assert_eq!(load(serde_json::json!(true)), InputMonitor::On);
        assert_eq!(load(serde_json::json!(false)), InputMonitor::Auto);
        assert_eq!(load(serde_json::json!("off")), InputMonitor::Off);
        assert_eq!(load(serde_json::json!("on")), InputMonitor::On);
        assert_eq!(load(serde_json::json!("auto")), InputMonitor::Auto);
        // Hostile values fall back to the default instead of failing the session.
        assert_eq!(load(serde_json::json!("loud")), InputMonitor::Auto);
        assert_eq!(load(serde_json::json!(3)), InputMonitor::Auto);
        assert_eq!(load(serde_json::json!(null)), InputMonitor::Auto);
        assert_eq!(load(serde_json::json!({"x": [1]})), InputMonitor::Auto);
        assert_eq!(serde_json::to_value(InputMonitor::On).unwrap(), serde_json::json!("on"));
        // A whole old mixer loads; a new one round-trips.
        let mut v = serde_json::to_value(Mixer::new(ChannelFormat::Mono)).unwrap();
        v["input_monitor"] = serde_json::json!(true);
        let m: Mixer = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(m.input_monitor, InputMonitor::On);
        v.as_object_mut().unwrap().remove("input_monitor");
        let m: Mixer = serde_json::from_value(v).unwrap();
        assert_eq!(m.input_monitor, InputMonitor::Auto);
        let m = Mixer { input_monitor: InputMonitor::Off, ..Mixer::new(ChannelFormat::Stereo) };
        let back: Mixer = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn input_monitor_ids_and_cycle() {
        for m in [InputMonitor::Auto, InputMonitor::On, InputMonitor::Off] {
            assert_eq!(InputMonitor::from_id(m.id()), Some(m));
        }
        assert_eq!(InputMonitor::from_id("loud"), None);
        assert_eq!(InputMonitor::Auto.next(), InputMonitor::On);
        assert_eq!(InputMonitor::On.next(), InputMonitor::Off);
        assert_eq!(InputMonitor::Off.next(), InputMonitor::Auto);
    }

    #[test]
    fn hears_input_follows_the_three_states() {
        for state in [InputMonitor::Auto, InputMonitor::On, InputMonitor::Off] {
            for armed in [false, true] {
                let m = Mixer { input_monitor: state, record_arm: armed, ..Mixer::new(ChannelFormat::Mono) };
                for stopped in [false, true] {
                    for recording in [false, true] {
                        let want = match state {
                            InputMonitor::On => true,
                            InputMonitor::Off => false,
                            InputMonitor::Auto => armed && (stopped || recording),
                        };
                        assert_eq!(m.hears_input(stopped, recording), want, "{state:?} armed={armed} stopped={stopped} rec={recording}");
                    }
                }
                let may = state == InputMonitor::On || (armed && state != InputMonitor::Off);
                assert_eq!(m.may_hear_input(), may, "{state:?} armed={armed}");
            }
        }
    }

    #[test]
    fn reserved_route_names_resolve_case_insensitively() {
        for n in ["", " ", "none", "No Input", "NO OUTPUT"] {
            assert_eq!(Route::reserved(n), Some(Route::None), "{n:?}");
        }
        for n in ["main", "Master", "OUT 1-2", " out 1-2 "] {
            assert_eq!(Route::reserved(n), Some(Route::Main), "{n:?}");
        }
        for n in ["Mains", "Out 3-4", "In 1", "Vox", "nones"] {
            assert_eq!(Route::reserved(n), None, "{n:?}");
        }
    }

    /// A mono input feeds every side; a wider one feeds the track in order, and a track wider
    /// than its input is silent past it (bug 10 of the recording section).
    #[test]
    fn a_mono_input_feeds_both_sides() {
        assert_eq!(track_input_channel(0, 4, 1), Some(4));
        assert_eq!(track_input_channel(1, 4, 1), Some(4));
        assert_eq!(track_input_channel(5, 4, 1), Some(4));
        assert_eq!(track_input_channel(0, 6, 2), Some(6));
        assert_eq!(track_input_channel(1, 6, 2), Some(7));
        assert_eq!(track_input_channel(2, 6, 2), None, "a stereo input on a quad track: the rears are silent");
        assert_eq!(track_input_channel(5, 6, 2), None);
        assert_eq!(track_input_channel(1, 3, 0), Some(3));
    }

    #[test]
    fn no_input_is_the_devices_first_channels() {
        let chans = |track, device| (0..track).map(|k| default_input_channel(k, track, device)).collect::<Vec<_>>();
        assert_eq!(chans(2, 8), [Some(0), Some(1)]);
        assert_eq!(chans(2, 1), [Some(0), Some(0)]);
        assert_eq!(chans(1, 8), [Some(0)]);
        assert_eq!(chans(4, 2), [Some(0), Some(1), None, None], "a quad track on a stereo device");
        assert_eq!(chans(2, 0), [Some(0), Some(0)], "no device channels: nothing to read, never a panic");
    }
}

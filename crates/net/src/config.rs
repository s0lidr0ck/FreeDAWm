//! The network settings (Setup › Network), kept in their own file next to the app's settings
//! (`network.json`), never in a session: whether this PC listens, its port and name, the
//! devices paired with it, and the booth it follows.

use serde::{Deserialize, Serialize};

/// The default port: the page (`http://<pc>:48100/`) and the JSON-lines control channel share it.
pub const DEFAULT_PORT: u16 = 48100;

/// A browser or PC paired with this one (Setup › Network lists them, with Forget).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDevice {
    pub name: String,
    /// `browser` or `pc`.
    pub kind: String,
    pub token: String,
    /// Has connected as a remote recorder (it is expected at Record).
    #[serde(default)]
    pub follows: bool,
    /// Paired over the encrypted local link: its key (hex), the only one it may connect with,
    /// and only over that link. Empty: paired over the plain link or by invite.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
}

/// The booth this PC follows as a remote recorder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowConfig {
    /// Following now (Setup › Network › Follow a booth).
    #[serde(default)]
    pub enabled: bool,
    /// Start following when the app starts (nobody touches the piano PC on a Sunday).
    #[serde(default = "yes")]
    pub at_launch: bool,
    /// The booth's name as it announces itself, and its last address (`host:port`).
    #[serde(default)]
    pub booth_name: String,
    #[serde(default)]
    pub booth_addr: String,
    /// The token the booth gave when it was paired.
    #[serde(default)]
    pub token: String,
    /// Where a session never saved is saved, as its own project folder, when it records a take
    /// for the booth (empty: `Documents/FreeDAWm/Remote Takes`).
    #[serde(default)]
    pub takes_folder: String,
    /// Hear the booth's mix through the network (a remote performer at home).
    #[serde(default)]
    pub hear_mix: bool,
    /// The mix's volume in the performer's ears (linear).
    #[serde(default = "one")]
    pub mix_gain: f32,
    /// Following over the internet: the booth's address from its invite (JSON); empty on the LAN.
    #[serde(default)]
    pub internet: String,
    /// The booth's encrypted local link: its key (hex), kept from the first pairing over it (a
    /// booth answering with another key is refused), and its address there (`ip:udp port`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub booth_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub booth_lan: String,
}

fn one() -> f32 {
    1.0
}

impl Default for FollowConfig {
    fn default() -> Self {
        FollowConfig {
            enabled: false,
            at_launch: true,
            booth_name: String::new(),
            booth_addr: String::new(),
            token: String::new(),
            takes_folder: String::new(),
            hear_mix: false,
            mix_gain: 1.0,
            internet: String::new(),
            booth_key: String::new(),
            booth_lan: String::new(),
        }
    }
}

fn yes() -> bool {
    true
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn default_contact() -> u64 {
    2 * 3600
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetConfig {
    /// Allow control from this network (off by default).
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_port")]
    pub port: u16,
    /// This PC's name on the network (empty: the computer's name).
    #[serde(default)]
    pub pc_name: String,
    #[serde(default)]
    pub paired: Vec<PairedDevice>,
    #[serde(default)]
    pub follow: FollowConfig,
    /// A remote recorder that lost the booth ends its take after this long without contact.
    #[serde(default = "default_contact")]
    pub contact_timeout_s: u64,
    /// Remote takes this booth has landed (their ids), so a resent one lands once.
    #[serde(default)]
    pub landed: Vec<String>,
    /// The remote recorders' ports seen (`Remote: <PC> › <port>`), offered in the booth's track
    /// input menus even while that PC is away.
    #[serde(default)]
    pub remote_ports: Vec<String>,
    /// The remote recorders' audio inputs seen (`Remote: <PC> › <input>`), offered in the booth's
    /// audio track input menus even while that PC is away.
    #[serde(default)]
    pub remote_inputs: Vec<String>,
    /// Over the internet: this PC's key (hex; made once), whether a performer may join this PC
    /// with an invite, and the relay (empty: n0's public relays).
    #[serde(default)]
    pub internet_key: String,
    #[serde(default)]
    pub internet_host: bool,
    #[serde(default)]
    pub relay_url: String,
    /// Offer the encrypted local link (QUIC on UDP at `port`, beside the TCP listener) while
    /// listening; on by default.
    #[serde(default = "yes")]
    pub encrypted_link: bool,
    /// Take PCs over the encrypted local link only: the plain TCP link then serves the phone's
    /// remote page and nothing else. On by default; a value already saved is kept.
    #[serde(default = "yes")]
    pub encrypted_only: bool,
    /// The booth PIN (Setup › Network; empty: none, a random code per request as before). Kept
    /// here as the pairing tokens are, never in a session, never announced or sent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub booth_pin: String,
    /// Ask before each device joins with the booth PIN (Allow / Deny on the booth's screen).
    #[serde(default, skip_serializing_if = "is_false")]
    pub pin_ask: bool,
    /// Wrong booth PINs since the last PIN pairing (at [`crate::pairing::PIN_OFF_AFTER`] PIN
    /// pairing is off until the owner turns it back on), and where the last came from.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pin_wrong: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pin_last_from: String,
    /// Reusable internet invites still open (one-time ones live only while the app runs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invites: Vec<SavedInvite>,
    /// Studios this PC mixes remotely (File › Connect to a Studio…), apart from the booth it
    /// follows (the same studio may be both: each keeps its own pairing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub studios: Vec<KnownStudio>,
    /// Lock remote mixing (Setup › Network): paired PCs may watch the mixer and hear the mix,
    /// but every command that would change something is refused. For the whole session, all
    /// remotes alike.
    #[serde(default, skip_serializing_if = "is_false")]
    pub lock_remote_mixing: bool,
}

/// A studio this PC has paired with to mix remotely: enough to reconnect with one click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KnownStudio {
    /// Its name as it announces itself.
    pub name: String,
    /// Its plain address (`host:port`), the last one it was found at.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub addr: String,
    /// The token it gave this PC.
    pub token: String,
    /// Its encrypted local link: its key (hex, pinned at pairing) and address (`ip:udp port`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lan: String,
    /// Over the internet: its address from the invite (JSON).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub internet: String,
    /// When it was last connected (seconds since 1970).
    #[serde(default)]
    pub last_used: u64,
}

impl KnownStudio {
    /// The link it is reached by, in words.
    pub fn link(&self) -> &'static str {
        if !self.internet.is_empty() {
            "over the internet"
        } else if !self.key.is_empty() && !self.lan.is_empty() {
            "encrypted local link"
        } else {
            "local network (not encrypted)"
        }
    }
}

/// A reusable internet invite as the booth keeps it: the SHA-256 of its secret (hex), never the
/// secret itself, so reading this file does not hand out a live invite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedInvite {
    /// Names it in Setup › Network (Revoke).
    pub id: String,
    pub hash: String,
    /// Good until (seconds since 1970).
    pub until: u64,
    /// Joins allowed, and made so far.
    pub max_joins: u32,
    #[serde(default)]
    pub joins: u32,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig {
            enabled: false,
            port: DEFAULT_PORT,
            pc_name: String::new(),
            paired: Vec::new(),
            follow: FollowConfig::default(),
            contact_timeout_s: default_contact(),
            landed: Vec::new(),
            remote_ports: Vec::new(),
            remote_inputs: Vec::new(),
            internet_key: String::new(),
            internet_host: false,
            relay_url: String::new(),
            encrypted_link: true,
            encrypted_only: true,
            booth_pin: String::new(),
            pin_ask: false,
            pin_wrong: 0,
            pin_last_from: String::new(),
            invites: Vec::new(),
            studios: Vec::new(),
            lock_remote_mixing: false,
        }
    }
}

/// The most landed take ids remembered.
const MAX_LANDED: usize = 1000;

impl NetConfig {
    /// The paired device holding `token`.
    pub fn device(&self, token: &str) -> Option<&PairedDevice> {
        if token.is_empty() {
            return None;
        }
        self.paired.iter().find(|d| d.token == token)
    }

    pub fn note_landed(&mut self, id: &str) {
        if !self.landed.iter().any(|l| l == id) {
            self.landed.push(id.to_string());
        }
        if self.landed.len() > MAX_LANDED {
            let extra = self.landed.len() - MAX_LANDED;
            self.landed.drain(..extra);
        }
    }

    /// Reads the settings file; a missing or unreadable one gives the defaults.
    pub fn load(path: &std::path::Path) -> NetConfig {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Writes the settings file (through a temporary file, so a crash never leaves half of it).
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let text = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_default_off() {
        let c = NetConfig::default();
        assert!(!c.enabled, "the network is opt-in");
        assert_eq!(c.port, DEFAULT_PORT);
        let back: NetConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(back, c);
        let dir = std::env::temp_dir().join(format!("freedawm-net-cfg-{}", std::process::id()));
        let path = dir.join("network.json");
        let mut c = c;
        c.paired.push(PairedDevice { name: "Phone".into(), kind: "browser".into(), token: "abc".into(), follows: false, key: String::new() });
        c.save(&path).unwrap();
        assert_eq!(NetConfig::load(&path), c);
        assert_eq!(c.device("abc").map(|d| d.name.as_str()), Some("Phone"));
        assert!(c.device("").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pcs_take_the_encrypted_link_only_by_default_and_a_saved_choice_stays() {
        assert!(NetConfig::default().encrypted_only);
        let unset: NetConfig = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert!(unset.encrypted_only, "settings saved before the option existed take the default");
        let saved: NetConfig = serde_json::from_str(r#"{"encrypted_only": false}"#).unwrap();
        assert!(!saved.encrypted_only, "a choice already saved is kept");
    }
}

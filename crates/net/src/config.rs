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
    /// A Behringer X32 as a control surface (Setup › X32 Control).
    #[serde(default)]
    pub x32: crate::x32::X32Config,
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
            x32: crate::x32::X32Config::default(),
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
        c.paired.push(PairedDevice { name: "Phone".into(), kind: "browser".into(), token: "abc".into(), follows: false });
        c.save(&path).unwrap();
        assert_eq!(NetConfig::load(&path), c);
        assert_eq!(c.device("abc").map(|d| d.name.as_str()), Some("Phone"));
        assert!(c.device("").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}

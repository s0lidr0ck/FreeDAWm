//! Hosted plug-ins in their own process (first step: CLAP, opt-in).
//!
//! A [`BridgedPlugin`] implements [`soundcraft_dsp::Plugin`], so the mixer, automation and
//! presets see no difference; the plug-in itself runs in a helper process,
//! `freedawm-plugin-host` (next to the app, or `FREEDAWM_PLUGIN_HOST`), which loads it with
//! `soundcraft-clap-host` as the app would.
//!
//! - **Audio** goes over shared memory ([`exchange`], one file per instance in the temp folder):
//!   the audio thread never waits. It adds one block of latency (reported, so delay compensation
//!   lines it up); a block the helper has not finished in time is passed through (an effect) or
//!   silent (an instrument) and counted. Offline renders wait for each block, with no added
//!   latency.
//! - **Control** goes over a pipe (the helper's stdin and stdout, one JSON line each way):
//!   opening, preparing, the plug-in's parameters, and its state.
//! - **A crash** (the helper exits, or falls behind for [`HANG_SECONDS`] straight) silences that
//!   plug-in, an effect passing its input through, and is reported once
//!   ([`take_crash_reports`], and the log); the session and the app carry on.
//! - **Opt-in:** [`enabled`] (the app's setting, [`set_enabled`]; `FREEDAWM_PLUGIN_PROCESS=1` or
//!   `0` overrides it) is off by default; [`create`] answers only then, and only for `clap:` ids.
//!
//! Not in this step: the editor window, presets of the plug-in's own, latency or parameter
//! changes the plug-in makes itself, the transport (tempo, position) for the plug-in, a restart,
//! VST3 and AU.
//!
//! This is an isolated `unsafe` helper crate (craftrules never-crash): `unsafe` is denied
//! crate-wide and allowed only in the private `shm` module (mapping the shared file and viewing it
//! as atomic words, each with a `// SAFETY:` comment). The public API is safe and never panics.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod exchange;
#[cfg(not(target_arch = "wasm32"))]
mod helper;
#[cfg(not(target_arch = "wasm32"))]
mod host;
#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
mod shm;

#[cfg(not(target_arch = "wasm32"))]
pub use helper::serve;
#[cfg(not(target_arch = "wasm32"))]
pub use host::BridgedPlugin;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The helper's program name (without the platform's extension).
pub const HELPER_NAME: &str = "freedawm-plugin-host";

/// A helper behind for this long without a break counts as hung: its plug-in is given up.
pub const HANG_SECONDS: u64 = 5;

/// The app's setting (Setup › Preferences › Mixing › Run CLAP plug-ins in their own process).
static SETTING: AtomicBool = AtomicBool::new(false);

/// Sets this computer's choice (the app's Preferences); [`enabled`] follows it unless
/// `FREEDAWM_PLUGIN_PROCESS` overrides it. Plug-ins already loaded stay where they are.
pub fn set_enabled(on: bool) {
    SETTING.store(on, Ordering::Relaxed);
}

/// What `FREEDAWM_PLUGIN_PROCESS` forces, if it is set: `1` on, `0` off (anything else is
/// ignored, and the setting decides).
pub fn env_override() -> Option<bool> {
    override_of(std::env::var("FREEDAWM_PLUGIN_PROCESS").ok().as_deref())
}

fn override_of(v: Option<&str>) -> Option<bool> {
    match v.map(str::trim) {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

/// Whether hosted CLAP plug-ins loaded from now on run in their own process: the app's setting
/// ([`set_enabled`], off by default, so nothing changes for existing sessions), unless
/// `FREEDAWM_PLUGIN_PROCESS` overrides it ([`env_override`]).
pub fn enabled() -> bool {
    env_override().unwrap_or_else(|| SETTING.load(Ordering::Relaxed))
}

/// The helper program the app would start, or why there is none (none in the browser).
pub fn helper_path() -> Result<std::path::PathBuf, String> {
    #[cfg(not(target_arch = "wasm32"))]
    return host::helper_exe();
    #[cfg(target_arch = "wasm32")]
    Err("plug-ins in their own process need the desktop app".into())
}

/// A `clap:` plug-in in its own process, when that is [`enabled`] and the helper starts; `None`
/// otherwise (the caller then creates it as before).
pub fn create(id: &str) -> Option<Box<dyn soundcraft_dsp::Plugin>> {
    if !enabled() || soundcraft_clap_host::parse_id(id).is_none() {
        return None;
    }
    #[cfg(not(target_arch = "wasm32"))]
    match BridgedPlugin::spawn(id) {
        Ok(p) => return Some(Box::new(p)),
        Err(e) => log::warn!("{id}: not run in its own process: {e}"),
    }
    None
}

static REPORTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Says (once) that a bridged plug-in crashed or hung and was silenced.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn report(what: String) {
    log::error!("{what}");
    REPORTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(what);
}

/// The crashes reported since the last call, oldest first, for the app to say.
pub fn take_crash_reports() -> Vec<String> {
    std::mem::take(&mut *REPORTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// The control pipe's messages.
pub mod proto {
    use serde::{Deserialize, Serialize};

    /// Host to helper.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum Request {
        /// Load plug-in `id` (a `clap:` id, or for tests a built-in one), the CLAP bundles in
        /// `dir` made known first, and prepare it.
        Open {
            id: String,
            dir: Option<String>,
            prepare: Prepare,
        },
        Prepare {
            prepare: Prepare,
        },
        SaveState,
        LoadState {
            state: String,
        },
        Quit,
    }

    /// How the plug-in is prepared, and the shared file it is served through.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Prepare {
        pub rate: f32,
        pub max: usize,
        pub channels: usize,
        pub offline: bool,
        pub shm: String,
    }

    /// Helper to host: one per request.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    pub struct Reply {
        pub ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub info: Option<Info>,
        #[serde(default)]
        pub latency: usize,
        #[serde(default)]
        pub tail: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub state: Option<String>,
    }

    /// The plug-in as the helper loaded it.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Info {
        pub name: String,
        pub short_name: String,
        pub instrument: bool,
        pub params: Vec<Param>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Param {
        pub id: String,
        pub name: String,
        pub min: f32,
        pub max: f32,
        pub default: f32,
        pub unit: String,
        pub log: bool,
        pub choices: Vec<String>,
        pub group: String,
    }

    /// Bytes as hex (state blobs on the pipe).
    pub fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Hex back to bytes; `None` when it is not.
    pub fn unhex(s: &str) -> Option<Vec<u8>> {
        if !s.len().is_multiple_of(2) {
            return None;
        }
        (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_and_only_for_clap() {
        // The test environment does not set it.
        assert!(!enabled());
        assert!(create("clap:org.soundcraft.test.gain").is_none());
        assert!(create("eq_7band").is_none());
    }

    #[test]
    fn the_variable_overrides_the_setting_with_1_or_0_only() {
        assert_eq!(override_of(None), None);
        assert_eq!(override_of(Some("1")), Some(true));
        assert_eq!(override_of(Some(" 0 ")), Some(false));
        assert_eq!(override_of(Some("yes")), None, "only 1 and 0 override");
        assert_eq!(override_of(Some("")), None);
    }

    #[test]
    fn state_bytes_survive_the_pipe() {
        let b = vec![0u8, 1, 127, 128, 255];
        assert_eq!(proto::unhex(&proto::hex(&b)), Some(b));
        assert_eq!(proto::unhex("abc"), None);
        assert_eq!(proto::unhex("zz"), None);
        let r = proto::Request::Open {
            id: "clap:x".into(),
            dir: None,
            prepare: proto::Prepare { rate: 48_000.0, max: 256, channels: 2, offline: false, shm: "/tmp/x".into() },
        };
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.contains("\"op\":\"open\""), "{line}");
        assert_eq!(serde_json::from_str::<proto::Request>(&line).unwrap(), r);
    }
}

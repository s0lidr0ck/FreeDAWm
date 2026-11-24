//! Window › Layout › Close Plug-in Windows (`window.close_plugin_windows`, Cmd+Alt+W) and the
//! button beside the Mix window's Channel width bar: every open plug-in or effect editor closes at
//! once, each through its own close path.
//!
//! What closes: built-in modules' plugin windows (docked or detached into a window of their own)
//! and instrument windows, full editor windows, the editor drawers open beside Mix strips, hosted
//! CLAP/VST3/AU editors (in this process or the plug-in process), hosted Clip Effects editors
//! (their state kept on the clip first, as their own Close Editor does), ARA editors, and the
//! insert editor opened from the Remote Mix window. Pinned windows close too: Pin keeps a window
//! on top, not open. What stays: the Mix, Edit, MIDI and Vocal Editors, dialogs and every other
//! window. A plug-in window's Detach, Pin and Follow choices are kept for when it is opened again.

use crate::SoundApp;
use serde_json::{Value, json};

/// The command id.
pub const COMMAND: &str = "window.close_plugin_windows";

/// The open plug-in windows, by kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenPluginWindows {
    /// Plugin windows of built-in modules and instruments (`ui.plugin_windows`), docked or detached.
    pub plugin_windows: usize,
    /// Full editor windows (a tile's ⤢).
    pub full_editors: usize,
    /// Editor drawers open beside Mix strips.
    pub drawers: usize,
    /// Hosted plug-ins' own editor windows (inserts and instruments).
    pub hosted: usize,
    /// Hosted Clip Effects modules' editor windows.
    pub clip_hosted: usize,
    /// ARA plug-ins' editor windows.
    pub ara: usize,
    /// The Remote Mix window's insert editor.
    pub remote: usize,
}

impl OpenPluginWindows {
    pub fn total(&self) -> usize {
        self.plugin_windows + self.full_editors + self.drawers + self.hosted + self.clip_hosted + self.ara + self.remote
    }

    fn json(&self) -> Value {
        json!({
            "closed": self.total(),
            "plugin_windows": self.plugin_windows,
            "full_editors": self.full_editors,
            "drawers": self.drawers,
            "hosted": self.hosted,
            "clip_hosted": self.clip_hosted,
            "ara": self.ara,
            "remote": self.remote,
        })
    }
}

/// What is open now.
pub fn open(app: &SoundApp) -> OpenPluginWindows {
    let (hosted, clip_hosted) = app.player.as_ref().map_or((0, 0), |p| (p.open_editors().len(), p.open_clip_editors().len()));
    OpenPluginWindows {
        plugin_windows: app.ui.plugin_windows.len(),
        full_editors: app.ui.full_editors.len(),
        drawers: app.ui.drawers.len(),
        hosted,
        clip_hosted,
        ara: app.ara.open_editor_count(),
        remote: usize::from(app.net.as_ref().is_some_and(|n| n.studio.editor.is_some())),
    }
}

/// How many plug-in windows are open now.
pub fn count(app: &SoundApp) -> usize {
    open(app).total()
}

/// The button's and the menu item's hover text.
pub fn tooltip(n: usize) -> String {
    match n {
        0 => "No plug-in windows are open".to_string(),
        1 => "Close 1 plug-in window (pinned ones too)".to_string(),
        n => format!("Close {n} plug-in windows (pinned ones too)"),
    }
}

/// Closes every open plug-in window; returns what was open.
pub fn close_all(app: &mut SoundApp) -> OpenPluginWindows {
    let was = open(app);
    // Built-in windows and drawers: dropped from the lists that draw them, as their own ✕ does
    // (a detached one's native window closes when it is no longer drawn).
    app.ui.plugin_windows.clear();
    app.ui.full_editors.clear();
    app.ui.drawers.clear();
    crate::module_drawer::prune(app);
    // Hosted editors: each closed by its plug-in's own editor teardown, on this (the main) thread.
    // A plug-in slow to close holds this frame only; its instance, state and audio carry on.
    if let Some(p) = app.player.as_ref() {
        for (track, slot) in p.open_editors() {
            p.close_editor(track, slot);
        }
    }
    let clips = app.player.as_ref().map(soundcraft_playback::Player::open_clip_editors).unwrap_or_default();
    for slot in clips {
        // As the Clip Effects window's Close Editor: what was set in it is kept on the clip.
        app.capture_clip_slot(slot);
        if let Some(p) = app.player.as_ref() {
            p.close_clip_editor(slot);
        }
    }
    app.ara.close_editors();
    if let Some(net) = app.net.as_mut() {
        // As its window's ✕: changes already sent stay on the studio.
        net.studio.editor = None;
    }
    was
}

/// `window.close_plugin_windows`: `{}` → `{closed, plugin_windows, full_editors, drawers, hosted,
/// clip_hosted, ara, remote}` (what was open, by kind).
pub fn command(app: &mut SoundApp) -> Result<Value, String> {
    let was = close_all(app);
    if was.total() > 0 {
        app.ui.status = if was.total() == 1 { "Closed 1 plug-in window".to_string() } else { format!("Closed {} plug-in windows", was.total()) };
    }
    Ok(was.json())
}

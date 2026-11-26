//! Window › Full Screen (`window.fullscreen`): the main window fills its screen with no title bar
//! or taskbar, as F11 does in a web browser. The app's own menu bar and toolbars stay; nothing is
//! hidden. The state is never saved: FreeDAWm always opens windowed.
//!
//! The command only records what is wanted ([`crate::UiState::fullscreen`]); [`step`] tells the
//! main window (eframe's standard `ViewportCommand::Fullscreen`, native full screen on macOS) and
//! follows the window when the system changes it (macOS's green button, its own View menu item).
//! Esc does not leave full screen: it already cancels drags and closes dialogs. Only the shortcut
//! and the menu item do, and a hint saying so shows for a moment on entering.

use crate::SoundApp;
use serde_json::{Value, json};

/// The registry's default shortcut. F11 is held for talkback in a session with a control room
/// (`control_room::TALK_KEY`), so pressing it there would open the talkback mic instead: Shift+F11
/// is the nearest key that never does. On macOS it is the system's own Ctrl+Cmd+F.
pub const SHORTCUT: &str = if cfg!(target_os = "macos") { "Cmd+Ctrl+F" } else { "Shift+F11" };

/// How long the "Press … to leave full screen" hint shows, and the last part of it that fades.
const HINT_SECS: f64 = 2.5;
const FADE_SECS: f64 = 0.6;

/// What the main window was last told and last reported, and when the hint started.
#[derive(Debug, Default)]
pub struct Window {
    /// The last `Fullscreen` command sent to the main window (None: none yet, it is windowed).
    sent: Option<bool>,
    /// The full-screen state the window last reported (None: not reported).
    seen: Option<bool>,
    /// The hint's start, in egui time; set when entering, shown on the next drawn frame.
    hint_since: Option<f64>,
    /// Entering asked for the hint; [`step`] starts it at that frame's time.
    hint_pending: bool,
}

/// `window.fullscreen {value?: bool}`: sets (or without `value` toggles) full screen.
pub fn command(app: &mut SoundApp, p: &Value) -> Value {
    let on = p.get("value").and_then(Value::as_bool).unwrap_or(!app.ui.fullscreen);
    set(app, on);
    if let Some(ctx) = &app.docking_context {
        ctx.request_repaint();
    }
    json!({"value": on})
}

fn set(app: &mut SoundApp, on: bool) {
    if on && !app.ui.fullscreen {
        app.fullscreen.hint_pending = true;
        app.ui.status = hint_text(app, false);
    }
    if !on {
        app.fullscreen.hint_pending = false;
        app.fullscreen.hint_since = None;
    }
    app.ui.fullscreen = on;
}

/// "Press Shift+F11 to leave full screen" with the shortcut in effect (the user may have changed
/// it), or the menu item when the command has no shortcut.
pub fn hint_text(app: &SoundApp, mac: bool) -> String {
    let mac = mac || cfg!(target_os = "macos");
    match crate::shortcuts::effective_shortcut(&app.ui.shortcuts, "window.fullscreen") {
        Some(s) => format!("Press {} to leave full screen", crate::shortcuts::shortcut_label(&s, mac)),
        None => "Window › Full Screen leaves full screen".into(),
    }
}

/// Every frame (in `logic`, main window): sends the wanted state to the main window when it
/// changed, and takes the window's own state when the system changed it.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    let reported = ctx.input(|i| i.viewport().fullscreen);
    if let Some(now) = reported {
        // Only a change the window reports counts: right after a command it still reports the old state.
        if app.fullscreen.seen.is_some_and(|was| was != now) && now != app.ui.fullscreen {
            app.ui.fullscreen = now;
            app.fullscreen.sent = Some(now);
            app.fullscreen.hint_pending = false;
            app.fullscreen.hint_since = None;
        }
        app.fullscreen.seen = Some(now);
    }
    let want = app.ui.fullscreen;
    if app.fullscreen.sent.unwrap_or(false) != want {
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Fullscreen(want));
        app.fullscreen.sent = Some(want);
    }
    if app.fullscreen.hint_pending {
        app.fullscreen.hint_pending = false;
        app.fullscreen.hint_since = Some(ctx.input(|i| i.time));
    }
}

/// The hint's opacity at egui time `now` (0 once it has gone).
fn hint_alpha(since: f64, now: f64) -> f32 {
    let age = now - since;
    if !(0.0..HINT_SECS).contains(&age) {
        return 0.0;
    }
    ((HINT_SECS - age) / FADE_SECS).clamp(0.0, 1.0) as f32
}

/// Whether the hint is on screen (`ui.inspect` reports it).
pub fn hint_showing(app: &SoundApp, ctx: &egui::Context) -> bool {
    app.fullscreen.hint_since.is_some_and(|t| hint_alpha(t, ctx.input(|i| i.time)) > 0.0)
}

/// Draws the hint near the top of the main window, over everything, fading out.
pub fn hint(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(since) = app.fullscreen.hint_since else { return };
    let now = ctx.input(|i| i.time);
    let alpha = hint_alpha(since, now);
    if alpha <= 0.0 {
        app.fullscreen.hint_since = None;
        return;
    }
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let text = hint_text(app, mac);
    let rect = ctx.content_rect();
    egui::Area::new(egui::Id::new("fullscreen_hint"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .pivot(egui::Align2::CENTER_TOP)
        .fixed_pos(egui::pos2(rect.center().x, rect.top() + 64.0))
        .show(ctx, |ui| {
            ui.set_opacity(alpha);
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(egui::RichText::new(text).size(15.0));
            });
        });
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hint_shows_then_fades_and_goes() {
        assert_eq!(hint_alpha(10.0, 10.0), 1.0);
        assert_eq!(hint_alpha(10.0, 11.5), 1.0);
        assert!(hint_alpha(10.0, 12.2) > 0.0 && hint_alpha(10.0, 12.2) < 1.0);
        assert_eq!(hint_alpha(10.0, 12.6), 0.0);
        assert_eq!(hint_alpha(10.0, 9.0), 0.0);
    }
}

//! "Do to all" and "do to selected": the modifier rule shared by the Mix strip and the Edit track
//! header, as in Pro Tools. Alt = every track, Alt+Shift = the selected tracks (when the clicked
//! one is among them), no modifier = the clicked track. On the S key, Ctrl-click solos exclusively
//! and Solo Safe is Cmd-click on macOS, Ctrl+Shift-click elsewhere (and the key's right-click
//! menu everywhere).

use crate::{MainWindow, SoundApp};
use egui::Modifiers;
use serde_json::json;
use soundcraft_model::{Track, TrackId, TrackKind};

/// Which tracks a strip or header control acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The clicked track.
    One,
    /// Every track the control applies to (Alt).
    All,
    /// The selected tracks the control applies to (Alt+Shift), when the clicked track is selected.
    Selected,
}

impl Scope {
    /// The scope the held modifiers ask for.
    pub fn of(m: Modifiers) -> Scope {
        match (m.alt, m.shift) {
            (true, true) => Scope::Selected,
            (true, false) => Scope::All,
            _ => Scope::One,
        }
    }
}

/// What a click on a track's S key does, given the held modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoloClick {
    /// Plain solo (toggled), over the scope's tracks.
    Solo(Scope),
    /// Solo this track alone, clearing every other solo.
    Exclusive,
    /// Toggle Solo Safe (over the scope's tracks): a solo-safe track is never muted by others' solos.
    Safe(Scope),
}

impl SoloClick {
    /// `mac`: Cmd-click is Solo Safe there (Pro Tools); elsewhere it is Ctrl+Shift-click.
    pub fn of(m: Modifiers, mac: bool) -> SoloClick {
        let safe = if mac { m.mac_cmd } else { m.ctrl && m.shift && !m.alt };
        if safe {
            // Alt+Shift with Cmd: the selected tracks; Alt: every track.
            SoloClick::Safe(Scope::of(m))
        } else if m.ctrl {
            SoloClick::Exclusive
        } else {
            SoloClick::Solo(Scope::of(m))
        }
    }
}

/// The tracks `scope` names from a click on track `id` in `window`, keeping only those `fits`
/// (the control exists on them). The clicked track is first.
pub fn tracks(app: &SoundApp, window: MainWindow, id: TrackId, scope: Scope, fits: impl Fn(&Track) -> bool) -> Vec<u64> {
    let s = app.engine.session();
    let mut out = vec![id.0];
    let others: Vec<TrackId> = match scope {
        Scope::One => Vec::new(),
        Scope::All => s.tracks.iter().map(|t| t.id).collect(),
        Scope::Selected => {
            let sel = crate::window_sync::selection_of(app, window);
            if sel.contains(&id) { sel } else { Vec::new() }
        }
    };
    for t in others {
        if t != id && s.track(t).is_some_and(&fits) {
            out.push(t.0);
        }
    }
    out
}

/// Tracks with an S and an M key.
pub fn mixes(t: &Track) -> bool {
    t.kind != TrackKind::Master
}

/// Tracks with R and I keys.
pub fn records(t: &Track) -> bool {
    t.kind.has_playlist()
}

/// A click on track `id`'s toggle key (`mix.mute`, `mix.record_arm`) with modifiers `m`: the
/// clicked track's new state goes to every track of the scope.
pub fn toggle(app: &mut SoundApp, window: MainWindow, id: TrackId, cmd: &str, m: Modifiers) {
    let Some(track) = app.engine.session().track(id).cloned() else { return };
    let (now, fits): (bool, fn(&Track) -> bool) = match cmd {
        "mix.mute" => (track.mixer.mute, mixes),
        "mix.record_arm" => (track.mixer.record_arm, records),
        "mix.solo" => (track.mixer.solo, mixes),
        _ => return,
    };
    let tracks = tracks(app, window, id, Scope::of(m), fits);
    run(app, cmd, json!({"tracks": tracks, "value": !now}));
}

/// A click on track `id`'s I key: the clicked track's next monitoring state goes to the scope.
pub fn input_monitor(app: &mut SoundApp, window: MainWindow, id: TrackId, m: Modifiers) {
    let Some(next) = app.engine.session().track(id).map(|t| t.mixer.input_monitor.next()) else { return };
    let tracks = tracks(app, window, id, Scope::of(m), records);
    run(app, "mix.input_monitor", json!({"tracks": tracks, "value": next.id()}));
}

/// A click on track `id`'s S key with modifiers `m` (`mac`: the platform's Solo Safe key).
pub fn solo(app: &mut SoundApp, window: MainWindow, id: TrackId, m: Modifiers, mac: bool) {
    let Some(track) = app.engine.session().track(id).cloned() else { return };
    let r = match SoloClick::of(m, mac) {
        SoloClick::Solo(scope) => {
            let tracks = tracks(app, window, id, scope, mixes);
            app.run("mix.solo", json!({"tracks": tracks, "value": !track.mixer.solo, "exclusive": false}))
        }
        SoloClick::Exclusive => {
            // Already the only solo: the click clears it.
            let only = track.mixer.solo && app.engine.session().tracks.iter().filter(|t| t.mixer.solo).count() == 1;
            app.run("mix.solo", json!({"tracks": [id.0], "value": !only, "exclusive": true}))
        }
        SoloClick::Safe(scope) => {
            let tracks = tracks(app, window, id, scope, mixes);
            app.run("mix.solo_safe", json!({"tracks": tracks, "value": !track.mixer.solo_safe}))
        }
    };
    report(app, r);
}

/// The S key's right-click menu: Solo Safe and Exclusive Solo, reachable without modifiers.
pub fn solo_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId) {
    let Some(track) = app.engine.session().track(id).cloned() else { return };
    let mac = ui.ctx().os().is_mac();
    let safe_key = if mac { "⌘-click" } else { "Ctrl+Shift-click" };
    if ui.add(egui::Button::selectable(track.mixer.solo_safe, "Solo Safe").shortcut_text(safe_key)).clicked() {
        run(app, "mix.solo_safe", json!({"tracks": [id.0], "value": !track.mixer.solo_safe}));
        ui.close();
    }
    if ui.add(egui::Button::new("Exclusive Solo").shortcut_text("Ctrl-click")).clicked() {
        run(app, "mix.solo", json!({"tracks": [id.0], "value": true, "exclusive": true}));
        ui.close();
    }
    if ui.button("Clear All Solos").clicked() {
        run(app, "mix.clear_solos", json!({}));
        ui.close();
    }
}

/// The S key's tooltip line about the modifiers.
pub fn solo_hint(mac: bool) -> &'static str {
    if mac {
        "Alt-click: all tracks · Alt+Shift-click: selected · Ctrl-click: exclusive · ⌘-click: Solo Safe"
    } else {
        "Alt-click: all tracks · Alt+Shift-click: selected · Ctrl-click: exclusive · Ctrl+Shift-click: Solo Safe"
    }
}

/// Track `id`'s volume moved to `db` from `before` by a drag, or reset by a click, with
/// modifiers `m`: Alt+Shift moves the selected tracks by the same amount (a reset sets them
/// all), Alt every track; the clicked track takes `db`.
pub fn volume(app: &mut SoundApp, window: MainWindow, id: TrackId, before: f32, db: f32, reset: bool, m: Modifiers) {
    let tracks = tracks(app, window, id, Scope::of(m), |t| t.kind != TrackKind::Folder);
    let merge = format!("fader:{}", id.0);
    if tracks.len() <= 1 {
        let _ = app.engine.execute_merged("mix.volume", &json!({"track": id.0, "db": db}), &merge);
    } else if reset {
        run(app, "mix.volume", json!({"tracks": tracks, "db": db}));
    } else {
        let _ = app.engine.execute_merged("mix.volume", &json!({"tracks": tracks, "delta_db": db - before}), &merge);
    }
}

/// Track `id`'s pan `index` moved to `v` from `before`: with Alt or Alt+Shift, the scope's
/// tracks take the same value (a centring double-click centres them all).
pub fn pan(app: &mut SoundApp, window: MainWindow, id: TrackId, index: usize, v: f32, m: Modifiers) {
    let tracks = tracks(app, window, id, Scope::of(m), |t| !t.mixer.pan.is_empty() && t.kind != TrackKind::Vca);
    let _ = app.engine.execute_merged("mix.pan", &json!({"tracks": tracks, "pan": v, "index": index}), &format!("pan:{}:{index}", id.0));
}

/// Bypass track `id`'s module `module` (at chain position `slot`), with modifiers `m`: Alt
/// toggles the module at the same position on every track, Alt+Shift on the selected tracks, to
/// the clicked module's new state.
pub fn bypass(app: &mut SoundApp, window: MainWindow, id: TrackId, module: soundcraft_model::ModuleId, m: Modifiers) {
    let scope = Scope::of(m);
    let s = app.engine.session();
    let Some(track) = s.track(id) else { return };
    let Some(slot) = track.mixer.inserts.iter().position(|i| i.id == module) else { return };
    let Some(now) = track.mixer.inserts.get(slot).map(|i| i.bypass) else { return };
    let targets: Vec<(u64, u32)> = tracks(app, window, id, scope, |t| t.mixer.inserts.len() > slot && t.frozen_audio().is_none())
        .into_iter()
        .filter_map(|t| s.track(TrackId(t)).and_then(|tr| tr.mixer.inserts.get(slot)).map(|ins| (t, ins.id.0)))
        .collect();
    if let [(t, m)] = targets.as_slice() {
        run(app, "mix.module_bypass", json!({"track": t, "module": m, "value": !now}));
        return;
    }
    // One undo step for the lot.
    app.engine.end_merge();
    let key = format!("bypass-scope:{}:{slot}", id.0);
    for (t, m) in targets {
        if let Err(e) = app.engine.execute_merged("mix.module_bypass", &json!({"track": t, "module": m, "value": !now}), &key) {
            app.ui.status = e.to_string();
        }
    }
    app.engine.end_merge();
}

fn report(app: &mut SoundApp, r: Result<serde_json::Value, String>) {
    if let Err(e) = r {
        app.ui.status = e;
    }
}

/// Runs a command, putting a refusal in the status line.
fn run(app: &mut SoundApp, cmd: &str, params: serde_json::Value) {
    let r = app.run(cmd, params);
    report(app, r);
}

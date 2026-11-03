//! The drawer header's "Sidechain" source picker, for modules whose plugin takes a sidechain key
//! (`PluginInfo::sidechain`). It chooses the track whose signal keys the module, and whether it is
//! tapped before that track's fader, through `mix.module_sidechain` (one undo step; a choice that
//! would form a routing cycle is refused and the reason shows in the status line).
//!
//! The field shows whether the key is live ("Kick · silent (cycle)" otherwise); the menu greys the
//! tracks the engine would refuse, with the reason on hover. Both are worked out once per document
//! revision (the menu's only while it is open), not every frame.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use egui::{Align2, Rect, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::PluginInfo;
use soundcraft_mix::{SidechainProblem, short_problem, sidechain_refusal, sidechain_routes};
use soundcraft_model::{Insert, ModuleId, Session, SidechainSource, Track, TrackId, TrackKind};
use std::sync::Arc;

/// Whether the drawer shows the picker: the plugin takes a key, or the module still has a source
/// (so it can be cleared).
pub fn shows_picker(info: &PluginInfo, ins: &Insert) -> bool {
    info.sidechain || ins.sidechain.is_some()
}

/// What the picker field reads, given how the engine resolved the module's key (`problem`:
/// why it is silent).
pub fn source_label(s: &Session, ins: &Insert, problem: Option<SidechainProblem>) -> String {
    let Some(sc) = ins.sidechain else { return "None (own input)".into() };
    let name = s.track(sc.track).map_or("Missing track", |t| t.name.as_str());
    let pre = if sc.pre_fader { " · pre-fader" } else { "" };
    match problem {
        None => format!("{name}{pre}"),
        Some(p) => format!("{name}{pre} · silent ({})", short_problem(p)),
    }
}

/// Why the module's key is silent (`None`: live, or no source).
pub fn key_problem(s: &Session, track: TrackId, module: ModuleId) -> Option<SidechainProblem> {
    sidechain_routes(s).into_iter().find(|r| r.track == track && r.module == module)?.result.err()
}

/// The sources the menu offers, each with the reason the engine would refuse it (greyed).
pub fn source_choices(s: &Session, track: TrackId, module: ModuleId, pre_fader: bool) -> Vec<(TrackId, String, Option<String>)> {
    source_tracks(s, track)
        .into_iter()
        .map(|(id, name)| {
            let why = sidechain_refusal(s, track, module, Some(SidechainSource { track: id, pre_fader }));
            (id, name, why)
        })
        .collect()
}

/// A value worked out once per document revision, kept in egui's memory.
pub(crate) fn cached<T: Clone + Send + Sync + 'static>(ui: &Ui, id: egui::Id, rev: u64, make: impl FnOnce() -> T) -> T {
    if let Some((r, v)) = ui.ctx().data(|d| d.get_temp::<(u64, T)>(id))
        && r == rev
    {
        return v;
    }
    let v = make();
    ui.ctx().data_mut(|d| d.insert_temp(id, (rev, v.clone())));
    v
}

/// Tracks offered as sources for a module on `own`: every other track with a signal of its own
/// (not master or VCA faders), in session order.
pub fn source_tracks(s: &Session, own: TrackId) -> Vec<(TrackId, String)> {
    s.tracks.iter().filter(|t| t.id != own && !matches!(t.kind, TrackKind::Master | TrackKind::Vca)).map(|t| (t.id, t.name.clone())).collect()
}

/// The status-line note for a command result that reports silent or cleared sidechains
/// (`sidechain_warnings`, `sidechain_cleared`), if any.
pub fn result_note(v: &Value) -> Option<String> {
    let mut notes: Vec<String> = v
        .get("routing_warnings")
        .or_else(|| v.get("sidechain_warnings"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    notes.extend(v.get("sidechain_cleared").and_then(Value::as_str).map(str::to_string));
    (!notes.is_empty()).then(|| notes.join("; "))
}

/// Height of the picker row.
pub const ROW_H: f32 = 22.0;

/// Draws the picker row at `y` across `inner`; returns the y under it.
pub(crate) fn row(app: &mut SoundApp, ui: &mut Ui, track: &Track, ins: &Insert, inner: Rect, y: f32) -> f32 {
    let t = Tokens::current();
    let label_w = 66.0;
    ui.painter().text(pos2(inner.min.x, y + ROW_H / 2.0), Align2::LEFT_CENTER, "SIDECHAIN", bold(9.0), t.text_dim);
    let r = Rect::from_min_size(pos2(inner.min.x + label_w, y), vec2(inner.width() - label_w, ROW_H));
    let rev = app.engine.revision;
    let problem = cached(ui, egui::Id::new(("sidechain_status", track.id.0, ins.id.0)), rev, || key_problem(app.engine.session(), track.id, ins.id));
    let text = source_label(app.engine.session(), ins, problem);
    let resp = crate::module_drawer::menu_field(ui, r, &text, "sidechain", true).on_hover_text("Key this module from another track's signal");
    egui::Popup::menu(&resp).show(|ui| menu(app, ui, track.id, ins));
    y + ROW_H
}

fn menu(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert) {
    let cur = ins.sidechain;
    let mut pre = cur.is_some_and(|c| c.pre_fader);
    let mut source: Option<Value> = None;
    ui.label(egui::RichText::new("Key from").small().weak());
    if ui.add(egui::Button::selectable(cur.is_none(), "None (own input)")).clicked() {
        source = Some(Value::Null);
    }
    let rev = app.engine.revision;
    let choices = cached(ui, egui::Id::new(("sidechain_choices", track.0, ins.id.0, pre)), rev, || {
        Arc::new(source_choices(app.engine.session(), track, ins.id, pre))
    });
    for (id, name, why) in choices.iter() {
        let on = cur.is_some_and(|c| c.track == *id);
        let resp = ui.add_enabled(on || why.is_none(), egui::Button::selectable(on, name.as_str()));
        let resp = match why {
            Some(w) => resp.on_disabled_hover_text(w.as_str()),
            None => resp,
        };
        if resp.clicked() {
            source = Some(json!(id.0));
        }
    }
    ui.separator();
    let resp = ui.add_enabled(cur.is_some(), egui::Checkbox::new(&mut pre, "Pre-fader"));
    if resp.changed()
        && let Some(c) = cur
    {
        source = Some(json!(c.track.0));
    }
    if let Some(src) = source {
        // A refusal (a cycle) shows in the status line (`SoundApp::run`).
        let _ = app.run("mix.module_sidechain", json!({"track": track.0, "module": ins.id.0, "source": src, "pre_fader": pre}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, SidechainSource};

    #[test]
    fn the_picker_shows_for_sidechain_plugins_and_lists_other_audio_tracks() {
        let mut s = Session::default();
        let kick = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Kick"));
        let bass = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Bass"));
        s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master 1"));
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
        assert_eq!(source_tracks(&s, bass), [(kick, "Kick".to_string()), (aux, "Verb".to_string())]);
        let mut ins = Insert::new("comp");
        let comp = soundcraft_dsp::plugin_info("compressor").unwrap();
        assert!(!comp.sidechain && !shows_picker(comp, &ins));
        assert_eq!(source_label(&s, &ins, None), "None (own input)");
        ins.sidechain = Some(SidechainSource { track: kick, pre_fader: true });
        assert!(shows_picker(comp, &ins), "a stale source can still be cleared");
        assert_eq!(source_label(&s, &ins, None), "Kick · pre-fader");
        assert_eq!(source_label(&s, &ins, Some(SidechainProblem::Cycle)), "Kick · pre-fader · silent (cycle)");
        ins.sidechain = Some(SidechainSource { track: TrackId(999_999), pre_fader: false });
        assert_eq!(source_label(&s, &ins, Some(SidechainProblem::Missing)), "Missing track · silent (missing)");
    }

    #[test]
    fn the_menu_greys_what_the_engine_would_refuse_and_the_field_shows_why_a_key_is_silent() {
        let mut s = Session::default();
        let kick = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Kick"));
        let bass = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Bass"));
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
        let mk = s.track_mut(kick).unwrap().mixer.add_module(Insert::new("comp"), None).unwrap();
        let mut keyed = Insert::new("comp");
        keyed.sidechain = Some(SidechainSource { track: kick, pre_fader: false });
        let mb = s.track_mut(bass).unwrap().mixer.add_module(keyed, None).unwrap();
        assert_eq!(key_problem(&s, bass, mb), None);
        // Kick keyed from Bass would silence Bass's key; from the aux it can't render.
        let c = source_choices(&s, kick, mk, false);
        assert_eq!(c.iter().map(|x| x.0).collect::<Vec<_>>(), [bass, aux]);
        assert!(c.iter().all(|x| x.2.is_some()), "{c:?}");
        // Bass may take Kick or the aux? Not the aux (an audio track can't wait for an aux).
        let c = source_choices(&s, bass, mb, false);
        assert!(c[0].2.is_none() && c[1].2.is_some(), "{c:?}");
        s.track_mut(kick).unwrap().inactive = true;
        assert_eq!(key_problem(&s, bass, mb), Some(SidechainProblem::Inactive));
    }
}

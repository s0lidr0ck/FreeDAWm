//! Find and spill in the Mix window: a field that keeps only the strips whose name holds the
//! text, and Spill (a VCA's, folder's or aux's strip menu) that shows only what it controls or
//! sums, with it. Both are this window's view, not the session's: nothing is saved or undone.

use crate::SoundApp;
use crate::theme::Tokens;
use egui::Ui;
use soundcraft_model::{Route, Session, Track, TrackId, TrackKind};

/// What the Mix window is narrowed to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MixFind {
    /// Strips whose name holds this (any case); empty keeps every strip.
    pub text: String,
    /// The strip spilled: it and what it controls or sums.
    pub spill: Option<TrackId>,
}

fn id() -> egui::Id {
    egui::Id::new("mix_find")
}

/// The Mix window's find and spill now.
pub fn get(ctx: &egui::Context) -> MixFind {
    ctx.data(|d| d.get_temp(id())).unwrap_or_default()
}

fn set(ctx: &egui::Context, f: MixFind) {
    ctx.data_mut(|d| d.insert_temp(id(), f));
}

/// Shows only `track` and what it controls or sums (see [`spill_members`]).
pub fn spill(ctx: &egui::Context, track: TrackId) {
    let mut f = get(ctx);
    f.spill = Some(track);
    set(ctx, f);
}

/// Whether `t` spills: a VCA, a folder or an aux has members to show.
pub fn spills(t: &Track) -> bool {
    matches!(t.kind, TrackKind::Vca | TrackKind::Folder | TrackKind::Aux)
}

/// The tracks a spilled `of` shows besides itself: a VCA's members (and those of the VCAs under
/// it), a folder's tracks at every depth, or what feeds an aux's input bus (by output or send).
pub fn spill_members(s: &Session, of: TrackId) -> Vec<TrackId> {
    let Some(t) = s.track(of) else { return Vec::new() };
    let mut out: Vec<TrackId> = Vec::new();
    match t.kind {
        TrackKind::Vca => {
            let mut masters = vec![of];
            let mut i = 0;
            while let Some(m) = masters.get(i).copied() {
                i += 1;
                let under: Vec<&Track> = s.tracks.iter().filter(|x| x.mixer.vca == Some(m.0) && !out.contains(&x.id) && x.id != of).collect();
                for x in under {
                    out.push(x.id);
                    if x.kind == TrackKind::Vca && masters.len() < 64 {
                        masters.push(x.id);
                    }
                }
            }
        }
        TrackKind::Folder => {
            for x in &s.tracks {
                let mut up = x.folder;
                let mut depth = 0;
                while let Some(f) = up
                    && depth < 64
                {
                    if f == of {
                        out.push(x.id);
                        break;
                    }
                    up = s.track(f).and_then(|y| y.folder);
                    depth += 1;
                }
            }
        }
        _ => {
            if let Route::Bus(b) = t.mixer.input {
                for x in s.tracks.iter().filter(|x| x.id != of) {
                    let feeds = x.mixer.output == Route::Bus(b) || x.mixer.sends.iter().flatten().any(|sd| sd.target == Route::Bus(b));
                    if feeds {
                        out.push(x.id);
                    }
                }
            }
        }
    }
    out
}

/// Whether the Mix window shows `t` under `f`.
pub fn keeps(s: &Session, f: &MixFind, t: &Track) -> bool {
    let text = f.text.trim().to_lowercase();
    if !text.is_empty() && !t.name.to_lowercase().contains(&text) {
        return false;
    }
    match f.spill {
        Some(sp) if s.track(sp).is_some() => t.id == sp || spill_members(s, sp).contains(&t.id),
        _ => true,
    }
}

/// The find field and, while a strip is spilled, its name with Unspill: above the strips.
pub fn bar(app: &SoundApp, ui: &mut Ui) {
    let mut f = get(ui.ctx());
    let before = f.clone();
    ui.horizontal(|ui| {
        ui.label("Find");
        ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(140.0).hint_text("track name"))
            .on_hover_text("Shows only the strips whose name holds this");
        if !f.text.is_empty() && ui.small_button("✕").on_hover_text("Show every strip").clicked() {
            f.text.clear();
        }
        let s = app.engine.session();
        match f.spill.and_then(|sp| s.track(sp)) {
            Some(t) => {
                ui.separator();
                ui.label(egui::RichText::new(format!("Spilled: {}", t.name)).color(Tokens::current().text_dim));
                if ui.small_button("Unspill").clicked() {
                    f.spill = None;
                }
            }
            None => f.spill = None,
        }
    });
    if f != before {
        set(ui.ctx(), f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, SendSlot};

    #[test]
    fn find_and_spill_narrow_the_strips() {
        let mut s = Session::default();
        let kick = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Kick"));
        let snare = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Snare"));
        let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
        let vca = s.add_track(TrackKind::Vca, ChannelFormat::Mono, Some("Drums VCA"));
        let sub = s.add_track(TrackKind::Vca, ChannelFormat::Mono, Some("Sub VCA"));
        let bus = s.add_bus("Verb", ChannelFormat::Stereo);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        s.track_mut(kick).unwrap().mixer.vca = Some(vca.0);
        s.track_mut(sub).unwrap().mixer.vca = Some(vca.0);
        s.track_mut(snare).unwrap().mixer.vca = Some(sub.0);
        s.track_mut(vox).unwrap().mixer.sends[0] = Some(SendSlot::new(Route::Bus(bus)));
        let shown = |f: &MixFind| s.tracks.iter().filter(|t| keeps(&s, f, t)).map(|t| t.name.clone()).collect::<Vec<_>>();
        assert_eq!(shown(&MixFind { text: "vox".into(), spill: None }), ["Lead Vox"]);
        assert_eq!(shown(&MixFind { text: String::new(), spill: Some(vca) }), ["Kick", "Snare", "Drums VCA", "Sub VCA"], "nested VCAs too");
        assert_eq!(shown(&MixFind { text: String::new(), spill: Some(aux) }), ["Lead Vox", "Verb"], "what sends to the aux");
        assert_eq!(shown(&MixFind { text: "kick".into(), spill: Some(vca) }), ["Kick"]);
        assert!(spills(s.track(vca).unwrap()) && !spills(s.track(kick).unwrap()));
        // A spilled track that is gone spills nothing away.
        assert_eq!(shown(&MixFind { text: String::new(), spill: Some(TrackId(9_999)) }).len(), s.tracks.len());
    }
}

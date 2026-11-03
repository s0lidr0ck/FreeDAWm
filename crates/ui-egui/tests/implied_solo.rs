//! The S and M keys show what a solo does, in the Mix strips and the Edit headers alike: soloing
//! the Kick Bus lights its S, shows the tracks on its path (Kick In, Kick Out upstream, the Drum
//! Bus downstream) as implied solos naming it, and shows the Snare as muted by the solo.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Route, SendSlot, Session, TrackKind};
use soundcraft_ui_egui::{Services, SoundApp};

/// Kick In, Kick Out → Kick Bus → Drum Bus → Main; Snare → Drum Bus and sends to Verb.
fn kit() -> Session {
    let mut s = Session::default();
    let audio = |s: &mut Session, name: &str| s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
    let kick_in = audio(&mut s, "Kick In");
    let kick_out = audio(&mut s, "Kick Out");
    let snare = audio(&mut s, "Snare");
    let (kb, db, vb) =
        (s.add_bus("Kick Bus", ChannelFormat::Stereo), s.add_bus("Drum Bus", ChannelFormat::Stereo), s.add_bus("Verb", ChannelFormat::Stereo));
    for (name, input, output) in [("Kick Bus", kb, Route::Bus(db)), ("Drum Bus", db, Route::Main), ("Verb", vb, Route::Main)] {
        let t = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some(name));
        let m = &mut s.track_mut(t).unwrap().mixer;
        m.input = Route::Bus(input);
        m.output = output;
    }
    s.track_mut(kick_in).unwrap().mixer.output = Route::Bus(kb);
    s.track_mut(kick_out).unwrap().mixer.output = Route::Bus(kb);
    let sn = &mut s.track_mut(snare).unwrap().mixer;
    sn.output = Route::Bus(db);
    sn.sends[0] = Some(SendSlot::new(Route::Bus(vb)));
    s
}

fn harness(window: &str) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(Engine::new(kit()), None, Services::default());
    app.run(window, json!({})).unwrap();
    app.run("mix.solo", json!({"track": "Kick Bus"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        h.step();
    }
    h
}

fn check(h: &Harness<'_, SoundApp>, window: &str) {
    let soloed = h.get_by_label("Kick Bus solo");
    assert_eq!(soloed.accesskit_node().toggled(), Some(egui::accesskit::Toggled::True), "{window}: the Kick Bus is soloed");
    for name in ["Kick In", "Kick Out", "Drum Bus"] {
        let label = format!("{name} solo, implied by Kick Bus");
        let key = h.query_by_label(&label);
        assert!(key.is_some(), "{window}: {name} shows an implied solo");
        assert_eq!(key.unwrap().accesskit_node().toggled(), Some(egui::accesskit::Toggled::False), "{window}: {name} is not soloed itself");
    }
    assert!(h.query_by_label("Snare mute, muted by solo").is_some(), "{window}: the Snare is muted by the solo");
    assert!(h.query_by_label("Snare solo").is_some(), "{window}: the Snare's S is plain");
    assert!(h.query_by_label("Verb mute, muted by solo").is_some(), "{window}: a send's return off the path is muted");
    assert!(h.query_by_label("Kick In mute").is_some(), "{window}: an implied solo is not muted");
}

#[test]
fn the_mix_window_shows_implied_solos() {
    check(&harness("window.mix"), "Mix");
}

#[test]
fn the_edit_window_shows_implied_solos() {
    check(&harness("window.edit"), "Edit");
}

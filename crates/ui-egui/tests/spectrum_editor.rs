//! The Spectrum editor: a tab of the lower dock beside the Vocal and MIDI editors (Window ›
//! Editors › Spectrum), one curve per shown track in its colour, the list's switches, Show all /
//! Hide all / Only selected tracks, hover, and the view kept with the session.

use egui::{Pos2, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_engine::cmd::spectrum::{self as cmd, Curve};
use soundcraft_mix::spectrum_tap::{SpectrumTaps, track_key};
use soundcraft_model::{ChannelFormat, Session, TrackId, TrackKind};
use soundcraft_ui_egui::docking::Panel;
use soundcraft_ui_egui::spectrum_view::{self, FFTS_PER_FRAME};
use soundcraft_ui_egui::{Services, SoundApp};

const RATE: f32 = 48_000.0;

fn taps() -> &'static SpectrumTaps {
    Box::leak(Box::new(SpectrumTaps::new()))
}

/// Kick, Bass, Keys and Vox (mono audio) and a Verb aux.
fn session() -> (Session, Vec<TrackId>) {
    let mut s = Session::default();
    let mut ids = Vec::new();
    for (i, name) in ["Kick", "Bass", "Keys", "Vox"].iter().enumerate() {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
        s.track_mut(t).unwrap().color = soundcraft_model::TRACK_COLORS[(i * 3 + 1) % 16];
        ids.push(t);
    }
    ids.push(s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb")));
    (s, ids)
}

fn app_for(s: Session) -> SoundApp {
    let mut app = SoundApp::new(soundcraft_engine::Engine::new(s), None, Services::default());
    app.spectrum.taps = taps();
    app.ui.show_spectrum = true;
    app
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut b = Harness::builder().with_size(vec2(1500.0, 1000.0)).with_step_dt(1.0 / 30.0);
    if proof_dir().is_some() {
        b = b.wgpu();
    }
    let mut h = b.build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

/// Pushes 2048 samples of a sine into `t`'s tap, as the audio thread would while it is shown.
fn feed(h: &Harness<SoundApp>, t: TrackId, hz: f32, amp: f32) {
    let taps = h.state().spectrum.taps;
    let Some(i) = taps.slot_of(track_key(t)).filter(|i| taps.is_on(*i) && taps.is_active()) else { return };
    let ring = taps.ring(i).unwrap();
    let w = ring.written();
    let x: Vec<f32> = (0..2048).map(|k| amp * (std::f32::consts::TAU * hz * (w + k) as f32 / RATE).sin()).collect();
    ring.push_slice(&x);
    // The stereo scope's taps, while it asks: a negative frequency puts the right out of phase.
    if taps.is_stereo(i)
        && let Some((l, r)) = taps.stereo_rings(i)
    {
        let sign = if hz < 0.0 { -1.0 } else { 1.0 };
        l.push_slice(&x);
        r.push_slice(&x.iter().map(|v| v * sign).collect::<Vec<f32>>());
    }
}

/// `frames` frames, feeding each `(track, hz)` before each.
fn play(h: &mut Harness<SoundApp>, tones: &[(TrackId, f32)], frames: usize) {
    for _ in 0..frames {
        for (t, hz) in tones {
            feed(h, *t, *hz, 0.5);
        }
        h.step();
    }
}

/// What the graph shows for `c` at `hz` (dBFS).
fn level(h: &Harness<SoundApp>, c: Curve, hz: f32) -> Option<f32> {
    h.state().spectrum.level(c, hz)
}

/// The list's switch for `name`.
macro_rules! checkbox {
    ($h:expr, $name:expr) => {
        $h.get_by_role_and_label(egui::accesskit::Role::CheckBox, $name)
    };
}

#[test]
fn it_opens_from_the_window_menu_as_a_tab_beside_the_midi_editor() {
    let (s, _) = session();
    let mut app = app_for(s);
    app.ui.show_spectrum = false;
    app.ui.show_midi_editor = true;
    let mut h = harness(app);
    assert!(!h.state().ui.docking.layout.contains(&Panel::Spectrum));
    assert!(!h.state().spectrum.taps.is_active(), "closed: the taps do not run");
    // The menu leaf Window › Editors › Spectrum, run as the menu bar runs it.
    let menus = soundcraft_ui_egui::menus::tree();
    let window = menus.iter().find(|n| n.label == "Window").unwrap();
    let leaf = window.children.iter().find(|n| n.label == "Editors").unwrap().children.iter().find(|n| n.label == "Spectrum").unwrap();
    let id = soundcraft_ui_egui::menus::command_for_path(&leaf.path, &soundcraft_ui_egui::menus::ui_aliases()).unwrap();
    assert_eq!(id, "window.spectrum");
    h.state_mut().run(&id, json!({})).unwrap();
    h.run_steps(3);
    let layout = &h.state().ui.docking.layout;
    let mut nodes: Vec<_> = layout.root.iter().collect();
    let mut tabbed = false;
    while let Some(n) = nodes.pop() {
        match n {
            craft_ui::docking::Node::Split { first, second, .. } => {
                nodes.push(first);
                nodes.push(second);
            }
            craft_ui::docking::Node::Tabs { panels, .. } => {
                tabbed |= panels.iter().position(|p| *p == Panel::Midi) < panels.iter().position(|p| *p == Panel::Spectrum)
                    && panels.contains(&Panel::Midi);
            }
            _ => {}
        }
    }
    assert!(tabbed, "Spectrum is a tab after the MIDI Editor: {layout:?}");
    assert!(h.state().spectrum.taps.is_active(), "drawn: the taps run");
    let strip = |h: &Harness<SoundApp>, label: &str| h.ctx.data(|d| d.get_temp::<Rect>(egui::Id::new(("soundcraft-dock-tab", label))));
    assert!(strip(&h, "MIDI EDITOR").unwrap().right() < strip(&h, "SPECTRUM").unwrap().left(), "the lower dock strip has SPECTRUM after MIDI EDITOR");
    // The list: every track that sounds, and the master.
    for name in ["Kick", "Bass", "Keys", "Vox", "Verb", "Master"] {
        checkbox!(h, name);
    }
    // Again: it closes, and the taps stop.
    h.state_mut().run("window.spectrum", json!({})).unwrap();
    h.run_steps(3);
    assert!(!h.state().ui.docking.layout.contains(&Panel::Spectrum));
    assert!(!h.state().spectrum.taps.is_active());
    assert_eq!(h.state().spectrum.taps.claimed().count(), 0, "closed: every tap is given back");
}

#[test]
fn hiding_a_track_removes_its_curve_and_its_tap() {
    let (s, ids) = session();
    let (kick, bass, keys, vox, verb) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    let mut h = harness(app_for(s));
    let tones = [(kick, 60.0), (bass, 110.0), (keys, 1000.0), (vox, 3000.0)];
    play(&mut h, &tones, 12);
    // Tracks show by default; the aux and the master do not.
    let want: Vec<Curve> = [kick, bass, keys, vox].map(Curve::Track).to_vec();
    assert_eq!(h.state().spectrum.drawn, want);
    for (t, hz) in tones {
        let peak = level(&h, Curve::Track(t), hz).unwrap();
        let away = level(&h, Curve::Track(t), hz * 4.0).unwrap();
        assert!(peak > -30.0 && peak > away + 30.0, "{hz} Hz peaks where it plays: {peak} vs {away}");
    }
    assert!(h.state().spectrum.taps.slot_of(track_key(verb)).is_none(), "a hidden curve has no tap");
    checkbox!(h, "Bass").click();
    play(&mut h, &tones, 3);
    assert!(!h.state().spectrum.drawn.contains(&Curve::Track(bass)));
    assert!(h.state().spectrum.level(Curve::Track(bass), 110.0).is_none(), "no curve kept for it");
    assert!(h.state().spectrum.taps.slot_of(track_key(bass)).is_none(), "its tap is given back: no audio work, no analysis");
    assert!(!cmd::visible(h.state().engine.session(), Curve::Track(bass)));
    // And back.
    checkbox!(h, "Bass").click();
    play(&mut h, &tones, 12);
    assert!(level(&h, Curve::Track(bass), 110.0).unwrap() > -30.0);
    // Hide all, then Show all (the aux and the master too).
    h.get_by_label("Hide all").click();
    h.run_steps(2);
    assert!(h.state().spectrum.drawn.is_empty());
    assert_eq!(h.state().spectrum.taps.claimed().count(), 0);
    h.get_by_label("Show all").click();
    h.run_steps(2);
    assert_eq!(h.state().spectrum.drawn.len(), 6);
    assert!(h.state().spectrum.drawn.contains(&Curve::Master));
}

#[test]
fn only_selected_follows_the_edit_selection() {
    let (s, ids) = session();
    let (bass, keys) = (ids[1], ids[2]);
    let mut h = harness(app_for(s));
    h.state_mut().run("edit.select", json!({"tracks": [keys.0]})).unwrap();
    h.get_by_label("Only selected tracks").click();
    h.run_steps(2);
    assert_eq!(h.state().spectrum.drawn, vec![Curve::Track(keys)]);
    h.state_mut().run("edit.select", json!({"tracks": [bass.0, keys.0]})).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().spectrum.drawn, vec![Curve::Track(bass), Curve::Track(keys)]);
    // The switches are greyed out while it is on.
    assert!(checkbox!(h, "Vox").accesskit_node().is_disabled());
    h.get_by_label("Only selected tracks").click();
    h.run_steps(2);
    assert_eq!(h.state().spectrum.drawn.len(), 4, "off: the switches decide again");
}

#[test]
fn what_is_shown_survives_save_and_load() {
    let (s, ids) = session();
    let (bass, verb) = (ids[1], ids[4]);
    let mut h = harness(app_for(s));
    checkbox!(h, "Bass").click();
    h.run_steps(1);
    checkbox!(h, "Verb").click();
    h.run_steps(1);
    checkbox!(h, "Master").click();
    h.run_steps(2);
    let before = h.state().spectrum.drawn.clone();
    assert!(!before.contains(&Curve::Track(bass)) && before.contains(&Curve::Track(verb)) && before.contains(&Curve::Master));
    // The session (the curves) and the preferences (the tab) as they are written and read.
    let text = serde_json::to_string(h.state().engine.session()).unwrap();
    let ui = serde_json::to_string(&h.state().ui).unwrap();
    let back: Session = serde_json::from_str(&text).unwrap();
    let mut app = app_for(back);
    app.ui = serde_json::from_str(&ui).unwrap();
    assert!(app.ui.show_spectrum);
    let h2 = harness(app);
    assert_eq!(h2.state().spectrum.drawn, before);
    // Preferences written before the editor existed load with it closed.
    let mut old: serde_json::Value = serde_json::from_str(&ui).unwrap();
    old.as_object_mut().unwrap().remove("show_spectrum");
    let old: soundcraft_ui_egui::UiState = serde_json::from_value(old).unwrap();
    assert!(!old.show_spectrum);
}

#[test]
fn hovering_a_curve_names_it_and_reads_frequency_and_level() {
    let (s, ids) = session();
    let keys = ids[2];
    let mut h = harness(app_for(s));
    let tones = [(ids[0], 60.0), (keys, 1000.0)];
    play(&mut h, &tones, 12);
    let plot = h.state().spectrum.plot.unwrap();
    let l = level(&h, Curve::Track(keys), 1000.0).unwrap();
    let at = Pos2::new(spectrum_view::x_of(plot, 1000.0), spectrum_view::y_of(plot, l));
    h.event(egui::Event::PointerMoved(at));
    play(&mut h, &tones, 2);
    assert_eq!(h.state().spectrum.hovered, Some(Curve::Track(keys)));
    let (hz, db) = h.state().spectrum.readout.unwrap();
    assert!((hz / 1000.0).ln().abs() < 0.03, "{hz}");
    assert!((db - l).abs() < 2.0, "{db} vs {l}");
    assert_eq!(spectrum_view::hz_text(hz.round()), "1.00 kHz");
    // Over a list row: that track's curve.
    let row = checkbox!(h, "Kick").rect();
    h.event(egui::Event::PointerMoved(row.center()));
    play(&mut h, &tones, 2);
    assert_eq!(h.state().spectrum.hovered, Some(Curve::Track(ids[0])));
}

#[test]
fn forty_four_tracks_stay_within_the_analysis_budget() {
    let mut s = Session::default();
    let ids: Vec<TrackId> = (0..44).map(|i| s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&format!("T{i}")))).collect();
    let mut h = harness(app_for(s));
    let tones: Vec<(TrackId, f32)> = ids.iter().enumerate().map(|(i, t)| (*t, 100.0 * (1.0 + i as f32 * 0.25))).collect();
    let mut most = 0;
    for _ in 0..20 {
        play(&mut h, &tones, 1);
        most = most.max(h.state().spectrum.analysed_last_frame);
    }
    assert_eq!(most, FFTS_PER_FRAME, "a frame analyses at most {FFTS_PER_FRAME} curves");
    // All 44 want a curve (tracks show by default): the first MAX_VISIBLE are drawn, and the list
    // says so.
    let drawn = h.state().spectrum.drawn.clone();
    assert_eq!(drawn, ids.iter().take(cmd::MAX_VISIBLE).map(|t| Curve::Track(*t)).collect::<Vec<_>>());
    assert_eq!(h.state().spectrum.taps.claimed().count(), cmd::MAX_VISIBLE, "taps only for the drawn curves");
    for (t, hz) in tones.iter().take(cmd::MAX_VISIBLE) {
        assert!(level(&h, Curve::Track(*t), *hz).unwrap() > -40.0, "every drawn curve gets its turn: {hz} Hz");
    }
    h.get_by_label(&format!("Showing the first {} of 44: at most {} curves at once.", cmd::MAX_VISIBLE, cmd::MAX_VISIBLE));
    // Show all switches on the first MAX_VISIBLE; one more is refused, its switch greyed with the reason.
    h.get_by_label("Show all").click();
    h.run_steps(2);
    assert!(checkbox!(h, "T30").accesskit_node().is_disabled());
    assert!(h.state_mut().run("spectrum.show", json!({"track": ids[30].0, "shown": true})).unwrap_err().contains("at most"));
    checkbox!(h, "T0").click();
    h.run_steps(2);
    assert!(!checkbox!(h, "T30").accesskit_node().is_disabled(), "room again once one is hidden");
}

#[test]
fn same_coloured_tracks_are_drawn_in_shades_of_their_colour() {
    let mut s = Session::default();
    let ids: Vec<TrackId> =
        ["Gtr L", "Gtr R", "Gtr Lead", "Bass"].iter().map(|n| s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(n))).collect();
    let red = [200, 60, 60];
    for t in &ids[..3] {
        s.track_mut(*t).unwrap().color = red;
    }
    // Near-identical counts as the same.
    s.track_mut(ids[2]).unwrap().color = [204, 58, 63];
    s.track_mut(ids[3]).unwrap().color = [90, 150, 240];
    let mut h = harness(app_for(s));
    let tones: Vec<(TrackId, f32)> = ids.iter().map(|t| (*t, 500.0)).collect();
    play(&mut h, &tones, 10);
    let looks = h.state().spectrum.looks.clone();
    let look = |i: usize| looks[&Curve::Track(ids[i])];
    use spectrum_view::{LineStyle::Solid, MIN_CONTRAST, PLOT_BG_LIGHTEST, contrast};
    // Shades, not dashes: three solid lines in three clearly different reds.
    assert_eq!([look(0).style, look(1).style, look(2).style, look(3).style], [Solid; 4]);
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        assert!(contrast(look(i).color, look(j).color) >= 1.45, "{i} and {j} apart: {:?} {:?}", look(i).color, look(j).color);
    }
    for i in 0..4 {
        assert!(contrast(look(i).color, PLOT_BG_LIGHTEST) >= MIN_CONTRAST - 0.05, "{i} stands out from the plot");
        let c = look(i).color;
        assert!(if i < 3 { c.r() > c.g() && c.r() > c.b() } else { c.b() > c.r() }, "{i} keeps its hue: {c:?}");
    }
    // The blue track has a colour to itself: drawn as it is.
    assert_eq!(look(3).color, egui::Color32::from_rgb(90, 150, 240));
    // The user's colours stay as they were.
    assert_eq!(h.state().engine.session().track(ids[1]).unwrap().color, red);
    // Hover still names the one under the pointer.
    let plot = h.state().spectrum.plot.unwrap();
    let lv = level(&h, Curve::Track(ids[3]), 500.0).unwrap();
    h.event(egui::Event::PointerMoved(Pos2::new(spectrum_view::x_of(plot, 500.0), spectrum_view::y_of(plot, lv))));
    play(&mut h, &tones, 2);
    assert!(h.state().spectrum.hovered.is_some());
}

#[test]
fn buses_and_the_master_have_their_own_sections() {
    let (s, ids) = session();
    let verb = ids[4];
    let mut h = harness(app_for(s));
    play(&mut h, &[], 2);
    // Each section has a header with its switch; buses and the master start hidden.
    h.get_by_label("Hide all tracks");
    h.get_by_label("Show all buses");
    h.get_by_label("Show all master");
    assert!(!h.state().spectrum.drawn.contains(&Curve::Track(verb)));
    // One click shows the buses.
    h.get_by_label("Show all buses").click();
    h.run_steps(2);
    assert!(h.state().spectrum.drawn.contains(&Curve::Track(verb)));
    assert!(cmd::visible(h.state().engine.session(), Curve::Track(verb)));
    h.get_by_label("Hide all buses");
    // And the tracks go in one click too.
    h.get_by_label("Hide all tracks").click();
    h.run_steps(2);
    assert_eq!(h.state().spectrum.drawn, vec![Curve::Track(verb)]);
    h.get_by_label("Show all master").click();
    h.run_steps(2);
    assert_eq!(h.state().spectrum.drawn, vec![Curve::Track(verb), Curve::Master]);
}

#[test]
fn names_on_the_graph_never_overlap() {
    let mut s = Session::default();
    // Twelve tracks all peaking at nearly the same place.
    let ids: Vec<TrackId> = (0..12).map(|i| s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&format!("Track {i}")))).collect();
    let mut h = harness(app_for(s));
    let tones: Vec<(TrackId, f32)> = ids.iter().enumerate().map(|(i, t)| (*t, 400.0 + i as f32 * 10.0)).collect();
    play(&mut h, &tones, 12);
    let labels = h.state().spectrum.labels.clone();
    assert!(!labels.is_empty() && labels.len() <= 6, "the loudest few are named: {}", labels.len());
    for (i, (_, a)) in labels.iter().enumerate() {
        for (_, b) in labels.iter().skip(i + 1) {
            assert!(!a.intersects(*b), "{a:?} overlaps {b:?}");
        }
        assert!(h.state().spectrum.plot.unwrap().expand(1.0).contains_rect(*a), "inside the plot");
    }
}

fn proof_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("SOUNDCRAFT_SPECTRUM_PROOF_DIR").map(Into::into)
}

/// With `SOUNDCRAFT_SPECTRUM_PROOF_DIR` set: pictures of the editor over the demo session as the
/// mix engine plays it into the taps. Otherwise only checks that it draws.
#[test]
fn proof_pictures_of_the_demo() {
    // The dock at its usual height in a large window, and a short dock in a small window.
    proof_at(vec2(1500.0, 1000.0), 400.0, "large");
    proof_at(vec2(1050.0, 700.0), 250.0, "small");
}

/// The proof pictures at window `size` with the lower dock `dock` high, named
/// `<SOUNDCRAFT_SPECTRUM_PROOF_TAG>-<name>-…` (tag "spectrum" when unset).
fn proof_at(size: egui::Vec2, dock: f32, name: &str) {
    let engine = soundcraft_engine::demo::demo_engine();
    let s = engine.session().clone();
    let mut app = SoundApp::new(engine, None, Services::default());
    let taps = taps();
    app.spectrum.taps = taps;
    app.ui.show_spectrum = true;
    app.run("spectrum.show", json!({"master": true, "shown": true})).unwrap();
    let mut b = Harness::builder().with_size(size).with_step_dt(1.0 / 30.0);
    if proof_dir().is_some() {
        b = b.wgpu();
    }
    let mut h = b.build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    let _ = h.state_mut().run("ui.dock", json!({"operation": "resizeSplit", "path": [], "size": {"FixedSecond": dock}}));
    let mut mix = soundcraft_mix::MixEngine::new(RATE, 512);
    mix.set_spectrum_taps(Some(taps));
    let mut out = vec![vec![0.0f32; 512]; 2];
    // From bar 9 (everything playing), 1/30 s of audio a frame.
    let mut pos = (32.0 * 60.0 / 104.0 * f64::from(RATE)) as i64;
    let mut run = |h: &mut Harness<SoundApp>, frames: usize| {
        for _ in 0..frames {
            for _ in 0..3 {
                mix.render(&s, pos, 512, &mut out);
                pos += 512;
            }
            h.step();
        }
    };
    run(&mut h, 40);
    assert!(!h.state().spectrum.drawn.is_empty());
    let tag = std::env::var("SOUNDCRAFT_SPECTRUM_PROOF_TAG").unwrap_or_else(|_| "spectrum".into());
    let shot = |h: &mut Harness<SoundApp>, what: &str| {
        if let Some(dir) = proof_dir() {
            std::fs::create_dir_all(&dir).unwrap();
            h.render().unwrap().save(dir.join(format!("{tag}-{name}-{what}.png"))).unwrap();
        }
    };
    shot(&mut h, "all");
    // Pointing at Bass in the list: its curve comes forward, named.
    let row = checkbox!(h, "Bass").rect();
    h.event(egui::Event::PointerMoved(row.center()));
    run(&mut h, 3);
    let bass = h.state().engine.session().tracks.iter().find(|t| t.name == "Bass").map(|t| t.id).unwrap();
    assert_eq!(h.state().spectrum.hovered, Some(Curve::Track(bass)));
    shot(&mut h, "hover-bass");
    h.event(egui::Event::PointerGone);
    // Everything, with the stereo scope.
    h.state_mut().run("spectrum.show_all", json!({})).unwrap();
    h.state_mut().run("spectrum.scope", json!({"on": true})).unwrap();
    run(&mut h, 30);
    // In a small dock the scope waits for room (its button says so); the graph keeps the space.
    let fits = name == "large";
    assert_eq!(h.state().spectrum.scope_rect.is_some(), fits);
    assert_eq!(!h.state().spectrum.scope.is_empty(), fits, "no room: no stereo work either");
    shot(&mut h, "scope");
    // With Tilt.
    h.state_mut().run("spectrum.tilt", json!({"on": true})).unwrap();
    run(&mut h, 20);
    shot(&mut h, "tilt");
}

/// Not a check: prints what the editor costs a UI frame with 8, 12, 24 and 40 visible tracks
/// (run by hand: `cargo test -p soundcraft-ui-egui --test spectrum_editor measure -- --ignored --nocapture`).
#[test]
#[ignore = "a timing measurement, run by hand"]
fn measure_the_frame_cost() {
    let mut a = soundcraft_dsp::spectrum::Analyzer::default();
    let x: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.37).sin()).collect();
    let mut out = Vec::new();
    let t0 = std::time::Instant::now();
    for _ in 0..1000 {
        a.magnitude_db_into(&x, 4096, &mut out);
    }
    println!("one 4096-point analysis: {:.1} µs", t0.elapsed().as_secs_f64() * 1e3);
    for n in [8usize, 12, 24, 40] {
        let mut s = Session::default();
        let ids: Vec<TrackId> = (0..n).map(|i| s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&format!("T{i}")))).collect();
        let tones: Vec<(TrackId, f32)> = ids.iter().enumerate().map(|(i, t)| (*t, 80.0 * (1.0 + i as f32 * 0.3))).collect();
        let time = |open: bool, scope: bool| {
            let mut app = app_for(s.clone());
            app.ui.show_spectrum = open;
            app.run("spectrum.scope", json!({"on": scope})).unwrap();
            let mut h = harness(app);
            play(&mut h, &tones, 20);
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t0 = std::time::Instant::now();
                play(&mut h, &tones, 30);
                best = best.min(t0.elapsed().as_secs_f64() / 30.0 * 1e3);
            }
            best
        };
        let (closed, open, scope) = (time(false, false), time(true, false), time(true, true));
        println!(
            "{n} visible tracks: UI frame {closed:.2} ms closed, {open:.2} ms open (+{:.2} ms), with the scope {scope:.2} ms (+{:.2} ms)",
            open - closed,
            scope - closed
        );
    }
}

#[test]
fn the_stereo_scope_draws_each_shown_track_and_reads_its_correlation() {
    let (s, ids) = session();
    let (kick, bass, keys) = (ids[0], ids[1], ids[2]);
    let mut h = harness(app_for(s));
    // Hidden by default: no scope, no stereo taps.
    play(&mut h, &[(kick, 60.0)], 3);
    assert!(h.state().spectrum.scope_rect.is_none());
    let slot = |h: &Harness<SoundApp>, t: TrackId| h.state().spectrum.taps.slot_of(track_key(t)).unwrap();
    assert!(!h.state().spectrum.taps.is_stereo(slot(&h, kick)));
    h.get_by_label("Stereo scope").click();
    // Kick in phase (mono), Keys out of phase; Bass hidden.
    checkbox!(h, "Bass").click();
    let tones = [(kick, 60.0), (keys, -750.0)];
    play(&mut h, &tones, 6);
    assert!(h.state().engine.session().edit.flag(cmd::SCOPE), "kept with the session");
    assert!(h.state().spectrum.scope_rect.is_some());
    assert!(h.state().spectrum.taps.is_stereo(slot(&h, kick)));
    let sc = h.state().spectrum.scope.clone();
    assert!(!sc.contains_key(&Curve::Track(bass)), "a hidden track has no dots");
    let k = &sc[&Curve::Track(kick)];
    assert!(k.correlation.unwrap() > 0.99 && k.points.iter().all(|(_, side)| side.abs() < 1e-6), "mono: a vertical line");
    let o = &sc[&Curve::Track(keys)];
    assert!(o.correlation.unwrap() < -0.99 && o.points.iter().all(|(mid, _)| mid.abs() < 1e-6), "out of phase: a horizontal line");
    assert_eq!(spectrum_view::correlation_text(-1.0), "-1.00");
    // Hovering a dot picks its track, and the graph highlights it too.
    let r = h.state().spectrum.scope_rect.unwrap();
    let field = spectrum_view::scope_field(r);
    // The out-of-phase line lies across the centre, its loudest dots at 0.9 of the radius (the
    // zoom), the left channel's positive peaks to the left.
    let radius = field.width().min(field.height()) * 0.5 - 2.0;
    h.event(egui::Event::PointerMoved(field.center() - vec2(0.9 * radius, 0.0)));
    play(&mut h, &tones, 2);
    assert_eq!(h.state().spectrum.hovered, Some(Curve::Track(keys)));
    // Off again: the taps stop carrying left and right.
    h.get_by_label("Stereo scope").click();
    play(&mut h, &tones, 2);
    assert!(h.state().spectrum.scope_rect.is_none() && h.state().spectrum.scope.is_empty());
    assert!(!h.state().spectrum.taps.is_stereo(slot(&h, kick)));
}

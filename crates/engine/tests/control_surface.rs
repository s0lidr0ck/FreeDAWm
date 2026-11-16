//! Control surfaces against simulated devices: the documented MIDI bytes go in as the hardware
//! would send them, and the bytes the host sends back are checked, so none of this depends on a
//! device being plugged in.

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_engine::surface::{self, SurfaceGroup, fader_14};
use soundcraft_model::{AutoParam, TrackId, fader_db_to_pos};

fn session() -> (Engine, Vec<TrackId>) {
    let mut e = Engine::default();
    let mut ids = Vec::new();
    for name in ["Kick", "Snare", "Bass Guitar", "Keys"] {
        let id = e.execute("track.new", &json!({"kind": "audio", "name": name})).unwrap()["tracks"][0].as_u64().unwrap();
        ids.push(TrackId(id));
    }
    (e, ids)
}

fn mackie() -> SurfaceGroup {
    SurfaceGroup::new(vec![surface::protocol("mackie").unwrap()])
}

/// Pitch bend on `strip` putting the fader at `db`.
fn bend(strip: u8, db: f32) -> [u8; 3] {
    let v = fader_14(fader_db_to_pos(db));
    [0xE0 | strip, (v & 0x7F) as u8, (v >> 7) as u8]
}

fn sent(g: &mut SurfaceGroup, e: &Engine) -> Vec<Vec<u8>> {
    g.output(e, 0, &|_| 0.0).into_iter().map(|(_, m)| m).collect()
}

fn mixer(e: &Engine, t: TrackId) -> soundcraft_model::Mixer {
    e.session().track(t).unwrap().mixer.clone()
}

fn vol(e: &Engine, t: TrackId) -> f32 {
    e.session().track(t).unwrap().mixer.volume_db
}

#[test]
fn mackie_moves_the_mixer_and_a_fader_move_is_one_undo_step() {
    let (mut e, t) = session();
    let mut g = mackie();
    let first = sent(&mut g, &e);
    // The first frame shows everything: names on the LCD, faders at 0 dB, LEDs off.
    let lcd = first.iter().find(|m| m.starts_with(&[0xF0, 0, 0, 0x66, 0x14, 0x12, 0x00])).expect("the LCD");
    assert_eq!(&lcd[7..7 + 21], b"Kick   Snare  Bass G ");
    assert!(first.contains(&bend(0, 0.0).to_vec()));
    assert!(first.contains(&vec![0x90, 0x10, 0x00]), "mute LED off");
    assert!(sent(&mut g, &e).is_empty(), "nothing changed, nothing sent");

    // Touch, move, let go: strip 2 (Snare) to -10 dB in three steps.
    g.input(&mut e, 0, &[0x90, 0x69, 0x7F]);
    for db in [-3.0, -6.0, -10.0] {
        g.input(&mut e, 0, &bend(1, db));
    }
    g.input(&mut e, 0, &[0x90, 0x69, 0x00]);
    assert!((vol(&e, t[1]) + 10.0).abs() < 0.05, "{}", vol(&e, t[1]));
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(vol(&e, t[1]), 0.0, "the whole move is one undo step");

    // Mute, solo, rec and select buttons run the commands (undoable as their on-screen buttons).
    g.input(&mut e, 0, &[0x90, 0x12, 0x7F]);
    g.input(&mut e, 0, &[0x90, 0x12, 0x00]);
    g.input(&mut e, 0, &[0x90, 0x08, 0x7F]);
    g.input(&mut e, 0, &[0x90, 0x19, 0x7F]);
    assert!(mixer(&e, t[2]).mute && mixer(&e, t[0]).solo);
    assert_eq!(e.session().edit.selected_tracks, vec![t[1]]);
    let back = sent(&mut g, &e);
    for led in [[0x90, 0x12, 0x7F], [0x90, 0x08, 0x7F], [0x90, 0x19, 0x7F], [0x90, 0x1B, 0x00]] {
        assert!(back.contains(&led.to_vec()), "{led:?} lit: {back:?}");
    }
    e.execute("edit.undo", &json!({})).unwrap();
    assert!(!mixer(&e, t[0]).solo, "a surface's solo undoes like the strip's");

    // The V-Pot pans: three clicks clockwise, the ring and the LCD follow.
    g.input(&mut e, 0, &[0xB0, 0x10, 0x03]);
    let pan = mixer(&e, t[0]).pan[0];
    assert!((pan - 0.06).abs() < 1e-4, "{pan}");
    let back = sent(&mut g, &e);
    assert!(back.iter().any(|m| m.starts_with(&[0xF0, 0, 0, 0x66, 0x14, 0x12])), "the pan shown on the LCD");
    // The V-Pot pushed centres it.
    g.input(&mut e, 0, &[0x90, 0x20, 0x7F]);
    assert_eq!(mixer(&e, t[0]).pan[0], 0.0);

    // The mixer moved elsewhere (the Mix window, an agent): the motor fader follows.
    e.execute("mix.volume", &json!({"track": t[3].0, "db": -20.0})).unwrap();
    assert!(sent(&mut g, &e).contains(&bend(3, -20.0).to_vec()));
}

#[test]
fn touching_a_fader_is_touching_it_on_screen_for_automation() {
    let (mut e, t) = session();
    e.execute("mix.automation_mode", &json!({"tracks": [t[0].0], "mode": "touch"})).unwrap();
    let mut g = mackie();
    let q = 4_800;
    let play = |e: &mut Engine, at: i64| {
        e.transport.playing = true;
        e.transport.position = at;
        e.automation_pass(true)
    };
    play(&mut e, 0);
    g.input(&mut e, 0, &[0x90, 0x68, 0x7F]);
    g.input(&mut e, 0, &bend(0, -6.0));
    // Held still: it keeps writing, as the on-screen fader held by the pointer does.
    for i in 1..=5 {
        assert_eq!(play(&mut e, i * q), 1, "pass {i} writes the held fader");
    }
    g.input(&mut e, 0, &[0x90, 0x68, 0x00]);
    assert_eq!(play(&mut e, 6 * q), 0, "let go, Touch stops writing at once");
    let lane = e.session().track(t[0]).unwrap().lane(&AutoParam::Volume).cloned().unwrap();
    assert!(lane.points.len() >= 5);
    assert!(lane.points.iter().all(|p| (p.value + 6.0).abs() < 0.05));
    // While held, the motor stays where the hand is: nothing is sent for that fader.
    g.input(&mut e, 0, &[0x90, 0x68, 0x7F]);
    e.execute("mix.volume", &json!({"track": t[0].0, "db": -30.0})).unwrap();
    assert!(!sent(&mut g, &e).iter().any(|m| m.first() == Some(&0xE0)));
}

#[test]
fn banks_and_channels_shift_and_the_transport_runs() {
    let (mut e, t) = session();
    for i in 0..10 {
        e.execute("track.new", &json!({"kind": "audio", "name": format!("T{i}")})).unwrap();
    }
    let mut g = mackie();
    sent(&mut g, &e);
    g.input(&mut e, 0, &[0x90, 0x2F, 0x7F]);
    assert_eq!(g.bank(), 6, "bank right stops where the last eight tracks fill the strips");
    g.input(&mut e, 0, &[0x90, 0x30, 0x7F]);
    assert_eq!(g.bank(), 5);
    g.input(&mut e, 0, &[0x90, 0x2E, 0x7F]);
    assert_eq!(g.bank(), 0);
    g.input(&mut e, 0, &[0x90, 0x31, 0x7F]);
    assert_eq!(g.track_on(e.session(), 0, 0), Some(t[1]));
    let lcd = sent(&mut g, &e).into_iter().find(|m| m.starts_with(&[0xF0, 0, 0, 0x66, 0x14, 0x12])).unwrap();
    assert_eq!(&lcd[7..14], b"Snare  ", "the strips show the new bank");
    // Transport: Play asks the host to play; the Play LED follows the transport.
    g.input(&mut e, 0, &[0x90, 0x5E, 0x7F]);
    assert!(!e.transport_requests.is_empty());
    e.transport.playing = true;
    let back = sent(&mut g, &e);
    assert!(back.contains(&vec![0x90, 0x5E, 0x7F]) && back.contains(&vec![0x90, 0x5D, 0x00]));
    // The jog wheel moves the playhead a tenth of a second a click.
    e.transport.playing = false;
    e.transport.position = 0;
    g.input(&mut e, 0, &[0xB0, 0x3C, 0x05]);
    assert_eq!(e.session().edit.playhead, 24_000);
}

#[test]
fn an_extender_shows_the_next_eight_tracks() {
    let mut e = Engine::default();
    let mut ids = Vec::new();
    for i in 0..12 {
        ids.push(TrackId(e.execute("track.new", &json!({"kind": "audio", "name": format!("T{i}")})).unwrap()["tracks"][0].as_u64().unwrap()));
    }
    let mut g = SurfaceGroup::new(vec![surface::protocol("mackie").unwrap(), surface::protocol("mackie_xt").unwrap()]);
    let out = g.output(&e, 0, &|_| 0.0);
    let xt_lcd = out.iter().find(|(u, m)| *u == 1 && m.starts_with(&[0xF0, 0, 0, 0x66, 0x15, 0x12])).unwrap();
    assert_eq!(&xt_lcd.1[7..14], b"T8     ");
    g.input(&mut e, 1, &[0x90, 0x10, 0x7F]);
    assert!(e.session().track(ids[8]).unwrap().mixer.mute, "the extender's first strip is the ninth track");
}

#[test]
fn hui_moves_the_mixer_and_shows_it_back() {
    let (mut e, t) = session();
    let mut g = SurfaceGroup::new(vec![surface::protocol("hui").unwrap()]);
    let first = g.output(&e, 0, &|id| if id == t[0] { 1.0 } else { 0.0 });
    let first: Vec<Vec<u8>> = first.into_iter().map(|(_, m)| m).collect();
    assert_eq!(first.first(), Some(&vec![0x90, 0x00, 0x00]), "the ping");
    assert!(first.contains(&vec![0xF0, 0, 0, 0x66, 0x05, 0x00, 0x10, 0x00, b'K', b'i', b'c', b'k', 0xF7]));
    assert!(first.contains(&vec![0xA0, 0x00, 12]), "a full-scale meter, left side");
    // Touch and move strip 3's fader, mute strip 2.
    for m in [[0xB0, 0x0F, 0x02], [0xB0, 0x2F, 0x40]] {
        g.input(&mut e, 0, &m);
    }
    let v = fader_14(fader_db_to_pos(-12.0));
    g.input(&mut e, 0, &[0xB0, 0x02, (v >> 7) as u8]);
    g.input(&mut e, 0, &[0xB0, 0x22, (v & 0x7F) as u8]);
    for m in [[0xB0, 0x2F, 0x00], [0xB0, 0x0F, 0x01], [0xB0, 0x2F, 0x42]] {
        g.input(&mut e, 0, &m);
    }
    assert!((vol(&e, t[2]) + 12.0).abs() < 0.05);
    assert!(e.session().track(t[1]).unwrap().mixer.mute);
    let back: Vec<Vec<u8>> = g.output(&e, 100, &|_| 0.0).into_iter().map(|(_, m)| m).collect();
    let i = back.iter().position(|m| *m == vec![0xB0, 0x0C, 0x01]).expect("zone 1 selected");
    assert_eq!(back.get(i + 1), Some(&vec![0xB0, 0x2C, 0x42]), "its mute LED on");
    // Transport from zone 0x0E.
    g.input(&mut e, 0, &[0xB0, 0x0F, 0x0E]);
    g.input(&mut e, 0, &[0xB0, 0x2F, 0x44]);
    assert!(!e.transport_requests.is_empty());
}

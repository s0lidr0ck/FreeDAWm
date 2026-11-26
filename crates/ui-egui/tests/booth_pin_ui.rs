//! Setup › Network › Booth PIN: a short PIN is refused in plain words, Suggest one makes a 6-digit
//! one shown in the clear, Set PIN keeps it (masked again unless Show), the ask-first switch,
//! the paused and turned-off states with their way back on, Forget all, and the booth's notice.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_net::hub::PinState;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(vec2(1600.0, 1600.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    )
}

fn type_into_pin(h: &mut Harness<'static, SoundApp>, text: &str) {
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::new("net-pin")));
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::Text(text.to_string()));
    h.run_steps(2);
}

fn network_window() -> Option<Harness<'static, SoundApp>> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.ui.show_network = true;
    let mut h = harness(app);
    h.run_steps(3);
    if h.state().net.is_none() {
        eprintln!("SKIPPED: no network state");
        return None;
    }
    Some(h)
}

#[test]
fn a_short_pin_is_refused_and_a_suggested_one_is_set() {
    let Some(mut h) = network_window() else { return };
    assert!(h.query_by_label_contains("Booth PIN (none)").is_some(), "off by default");
    assert!(h.query_by_label_contains("never works over the internet").is_some());
    assert!(h.query_by_label_contains("does not unpair devices already paired").is_some());
    type_into_pin(&mut h, "12345");
    h.get_by_label("Set PIN").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("at least 6 characters").is_some());
    assert_eq!(h.state().net.as_ref().unwrap().hub.pin_state(), PinState::Unset);
    h.get_by_label("Suggest one").click();
    h.run_steps(3);
    // Shown in the clear once suggested.
    assert!(h.query_by_label_contains("Suggested").is_some());
    h.get_by_label("Set PIN").click();
    h.run_steps(3);
    let pin = h.state().net.as_ref().unwrap().hub.config().booth_pin;
    assert!(pin.len() == 6 && pin.chars().all(|c| c.is_ascii_digit()), "{pin}");
    assert!(h.query_by_label_contains("Booth PIN set").is_some());
    assert!(h.query_by_label_contains("Ask me before each device joins").is_some());
    h.get_by_label("Ask me before each device joins").click();
    h.run_steps(3);
    assert!(h.state().net.as_ref().unwrap().hub.config().pin_ask);
}

#[test]
fn paused_and_turned_off_say_so_with_the_way_back_on() {
    let Some(mut h) = network_window() else { return };
    let hub = h.state().net.as_ref().unwrap().hub.clone();
    hub.set_booth_pin("sunday-choir").unwrap();
    hub.listen(0).unwrap();
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], hub.listening_port().unwrap()));
    let wrong = || {
        let (_, r) = soundcraft_net::hub::http_request(addr, "POST", "/api/pair/request", None, Some(&serde_json::json!({"name": "Guest"}))).unwrap();
        let _ = soundcraft_net::hub::http_request(
            addr,
            "POST",
            "/api/pair/confirm",
            None,
            Some(&serde_json::json!({"request": r["request"], "code": "nope-nope"})),
        );
    };
    for _ in 0..5 {
        wrong();
    }
    h.run_steps(3);
    assert!(h.query_by_label_contains("PIN pairing paused: 5 wrong tries from Guest").is_some());
    // The booth's notice, wherever the user is.
    assert!(h.query_by_label_contains("PIN pairing paused for 5 minutes").is_some());
    assert!(h.query_by_label("Resume PIN pairing now").is_some());
    // 15 more (the pairing requests' own limit is 20 in 10 minutes, so this goes on from here).
    hub.age_pin_pause(3600.0);
    for _ in 0..3 {
        for _ in 0..5 {
            wrong();
        }
        hub.age_pin_pause(3600.0);
    }
    h.run_steps(3);
    // In the window, and as the booth's notice.
    assert!(h.query_by_label_contains("wrong tries (the last from Guest). Consider a new PIN").is_some());
    assert!(h.query_by_label_contains("turn it back on in Setup › Network").is_some());
    h.get_by_label("Turn PIN pairing back on").click();
    h.run_steps(3);
    assert_eq!(hub.pin_state(), PinState::On);
    hub.stop();
}

#[test]
fn a_pin_pairing_shows_a_notice_and_forget_all_asks_twice() {
    let Some(mut h) = network_window() else { return };
    let hub = h.state().net.as_ref().unwrap().hub.clone();
    hub.set_booth_pin("sunday-choir").unwrap();
    hub.listen(0).unwrap();
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], hub.listening_port().unwrap()));
    let (_, r) =
        soundcraft_net::hub::http_request(addr, "POST", "/api/pair/request", None, Some(&serde_json::json!({"name": "Alto's phone"}))).unwrap();
    let (code, _) = soundcraft_net::hub::http_request(
        addr,
        "POST",
        "/api/pair/confirm",
        None,
        Some(&serde_json::json!({"request": r["request"], "code": "sunday-choir"})),
    )
    .unwrap();
    assert_eq!(code, 200);
    h.run_steps(3);
    assert!(h.query_by_label_contains("Alto's phone paired with the booth PIN").is_some());
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(hub.notices().is_empty());
    h.get_by_label("Forget all…").click();
    h.run_steps(3);
    assert_eq!(hub.config().paired.len(), 1, "not yet");
    h.get_by_label("Really forget all 1?").click();
    h.run_steps(3);
    assert!(hub.config().paired.is_empty());
    hub.stop();
}

//! Scout Send with a fake `ask_claude`: the answer streams into a turn and is stored with its
//! guides; without a key the settings open with the hint; errors stay in the conversation.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Guide, Mixer, ScoutCapture, ScoutMetrics, ScoutStrip, ScoutTurn, Session, TrackKind};
use soundcraft_time::Range;
use soundcraft_ui_egui::scout::ask::{ClaudeEvent, ClaudeRequest, Role, SYSTEM};
use soundcraft_ui_egui::scout::panel::NO_KEY;
use soundcraft_ui_egui::{Services, SoundApp};
use std::sync::{Arc, Mutex};

fn session() -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let m = ScoutMetrics { valid: true, duration_sec: 2.0, sample_rate: 48_000.0, ..ScoutMetrics::default() };
    s.scout.captures.push(ScoutCapture {
        id: 1,
        name: "Chorus".into(),
        created: "2026-10-20T12:00:00Z".into(),
        sample_rate: 48_000.0,
        tempo: 120.0,
        played: vec![Range::new(0, 96_000)],
        loop_range: None,
        played_truncated: false,
        strips: vec![ScoutStrip {
            track: t,
            name: "Vox".into(),
            kind: TrackKind::Audio,
            channel_type: "channel".into(),
            role: "Channel".into(),
            channels: 1,
            mixer: Mixer::new(ChannelFormat::Mono),
            automated: Vec::new(),
            base: m.clone(),
            post: m,
            note: None,
            target: None,
        }],
        bands_version: 2,
    });
    s.scout.next_id = 2;
    s
}

type Sent = Arc<Mutex<Vec<ClaudeRequest>>>;
type Callback = Arc<Mutex<Option<Box<dyn FnMut(ClaudeEvent) + Send>>>>;

/// Services with a fake Claude: requests are recorded and the callback kept, so the test plays
/// the stream event by event.
fn fake(key: Option<&str>) -> (Services, Sent, Callback) {
    let sent: Sent = Arc::default();
    let cb: Callback = Arc::default();
    let (s2, c2) = (sent.clone(), cb.clone());
    let key = key.map(str::to_string);
    let services = Services {
        ask_claude: Some(Arc::new(move |req, on_event| {
            s2.lock().unwrap().push(req);
            *c2.lock().unwrap() = Some(on_event);
        })),
        api_key_status: Some(Arc::new(move || key.as_ref().map(|_| "sk-ant-…a1b2".to_string()))),
        ..Services::default()
    };
    (services, sent, cb)
}

fn emit(cb: &Callback, ev: ClaudeEvent) {
    (cb.lock().unwrap().as_mut().unwrap())(ev);
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    )
}

const ANSWER_END: &str = "\n\n```json\n{\"format\": \"scout-guides\", \"version\": 1, \"guides\": [{\"kind\": \"eq\", \"track\": \"Vox\", \"lo_hz\": 2500, \"hi_hz\": 4000, \"gain_lo_db\": -4, \"gain_hi_db\": -2, \"dynamic\": true, \"label\": \"harsh\"}, {\"kind\": \"eq\", \"track\": \"Ghost\", \"lo_hz\": 100, \"hi_hz\": 200, \"gain_lo_db\": -1, \"gain_hi_db\": 0, \"dynamic\": false, \"label\": \"x\"}]}\n```";

#[test]
fn send_streams_into_a_turn_and_stores_it() {
    let (services, sent, cb) = fake(Some("key"));
    let mut h = harness(SoundApp::new(Engine::new(session()), None, services));
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.run_steps(2);
    h.state_mut().scout.question = "Is the vocal harsh?".into();
    h.run_steps(1);
    h.get_by_label("Send").click();
    h.run_steps(2);
    let req = sent.lock().unwrap()[0].clone();
    assert_eq!(req.model, "claude-opus-5-5");
    assert_eq!(req.system, SYSTEM);
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].0, Role::User);
    assert!(req.messages[0].1.starts_with("Is the vocal harsh?"), "the first message is prompt_text");
    assert!(req.messages[0].1.contains("\"scout_payload_version\": \"0.6.0-daw\""));
    assert_eq!(req.body()["stream"], true);
    assert!(h.state().scout.question.is_empty(), "the question moved into the conversation");
    emit(&cb, ClaudeEvent::Text("**Summary** Harsh at 3 kHz.".into()));
    h.run_steps(2);
    assert!(h.query_by_label_contains("Harsh at 3 kHz").is_some(), "streamed into the panel");
    assert!(h.query_by_label_contains("is answering").is_some());
    emit(&cb, ClaudeEvent::Text(ANSWER_END.into()));
    emit(&cb, ClaudeEvent::Done);
    h.run_steps(3);
    let s = h.state().engine.session();
    assert_eq!(s.scout.turns.len(), 1, "stored with scout.answer");
    let turn = &s.scout.turns[0];
    assert_eq!(turn.question, "Is the vocal harsh?");
    assert_eq!(turn.captures, vec![1]);
    assert_eq!(turn.model, "claude-opus-5-5");
    assert!(matches!(&turn.guides[..], [Guide::Eq { track, .. }] if track == "Vox"), "{:?}", turn.guides);
    assert_eq!(turn.notes.len(), 1, "the guide for an unknown track is dropped with a note");
    assert!(h.state().scout.asking.is_none());
    assert!(h.query_by_label_contains("Vox · Clean Q").is_some(), "a suggestion row");
    assert!(h.query_by_label_contains("scout-guides").is_none(), "the block itself is not shown");
    // A follow-up carries the conversation, then the new question with a fresh package.
    h.state_mut().scout.question = "And the low end?".into();
    h.state_mut().run("ui.scout.send", json!({})).unwrap();
    let req = sent.lock().unwrap()[1].clone();
    let roles: Vec<Role> = req.messages.iter().map(|m| m.0).collect();
    assert_eq!(roles, vec![Role::User, Role::Assistant, Role::User]);
    assert_eq!(req.messages[0].1, "Is the vocal harsh?");
    assert!(req.messages[2].1.starts_with("And the low end?") && req.messages[2].1.contains("scout_payload_version"));
    // The follow-up fails: nothing is stored, and the error and the question stay.
    emit(&cb, ClaudeEvent::Error("Rate limited; try again shortly".into()));
    h.run_steps(2);
    assert_eq!(h.state().engine.session().scout.turns.len(), 1, "a failed question is not stored");
    assert!(h.query_by_label_contains("Rate limited").is_some(), "the error is in the conversation");
    assert_eq!(h.state().scout.question, "And the low end?", "the question is kept");
}

#[test]
fn send_without_key_shows_the_settings_hint() {
    let (services, sent, _) = fake(None);
    let mut h = harness(SoundApp::new(Engine::new(session()), None, services));
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.run_steps(2);
    assert!(h.query_by_label_contains(NO_KEY).is_some(), "the empty conversation says so");
    h.state_mut().scout.question = "Too bright?".into();
    let e = h.state_mut().run("ui.scout.send", json!({})).unwrap_err();
    assert_eq!(e.to_string(), NO_KEY);
    h.run_steps(2);
    assert!(sent.lock().unwrap().is_empty(), "nothing sent");
    assert!(h.state().scout.settings_open);
    assert!(h.query_by_label_contains("No Anthropic API key").is_some(), "Scout ▸ Settings is open");
    assert_eq!(h.state().scout.question, "Too bright?", "the question is kept");
    // Without a host network call (the web build) Send is refused too.
    let mut h = harness(SoundApp::new(Engine::new(session()), None, Services::default()));
    h.state_mut().scout.question = "Too bright?".into();
    assert!(h.state_mut().run("ui.scout.send", json!({})).is_err());
    h.run_steps(1);
}

#[test]
fn a_host_that_goes_quiet_ends_the_wait() {
    let (services, _, cb) = fake(Some("key"));
    let mut h = harness(SoundApp::new(Engine::new(session()), None, services));
    h.state_mut().scout.question = "Q".into();
    h.state_mut().run("ui.scout.send", json!({})).unwrap();
    emit(&cb, ClaudeEvent::Text("partial".into()));
    // The host drops its callback without Done or Error.
    *cb.lock().unwrap() = None;
    h.run_steps(2);
    let a = h.state().scout.asking.as_ref().unwrap();
    assert!(!a.busy() && a.error.is_some());
    assert!(h.state().engine.session().scout.turns.is_empty());
    // An empty answer is an error too, not an empty turn.
    h.state_mut().scout.question = "Q".into();
    h.state_mut().run("ui.scout.send", json!({})).unwrap();
    emit(&cb, ClaudeEvent::Done);
    h.run_steps(2);
    assert!(h.state().engine.session().scout.turns.is_empty());
    assert!(h.state().scout.asking.as_ref().unwrap().error.is_some());
}

#[test]
fn copy_and_package_refuse_an_empty_capture_list() {
    let mut h = harness(SoundApp::new(Engine::new(session()), None, Services::default()));
    for id in ["ui.scout.copy", "ui.scout.package"] {
        let e = h.state_mut().run(id, json!({"captures": []})).unwrap_err();
        assert_eq!(e.to_string(), "Choose at least one capture", "{id}");
        assert!(h.state_mut().run(id, json!({"captures": [99]})).is_err(), "{id}: a capture that doesn't exist");
    }
    assert!(h.state_mut().run("ui.scout.copy", json!({"captures": [1]})).is_ok());
}

#[test]
fn each_answer_copies_its_own_question_and_captures() {
    let mut s = session();
    s.scout.turns.push(ScoutTurn { id: 2, question: "Old question".into(), captures: vec![1], answer: "Fine.".into(), ..ScoutTurn::default() });
    let mut h = harness(SoundApp::new(Engine::new(s), None, Services::default()));
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.state_mut().scout.question = "A new question".into();
    h.run_steps(2);
    h.get_by_label("Copy prompt + package").click();
    h.run_steps(2);
    let copied: Vec<String> = h
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(copied.len(), 1);
    assert!(copied[0].starts_with("Old question"), "that turn's question, not the one typed");
}

#[test]
fn an_answer_never_lands_in_a_replaced_session() {
    let (services, _, cb) = fake(Some("key"));
    let mut h = harness(SoundApp::new(Engine::new(session()), None, services));
    h.state_mut().scout.question = "Q".into();
    h.state_mut().run("ui.scout.send", json!({})).unwrap();
    emit(&cb, ClaudeEvent::Text("half an answer".into()));
    h.run_steps(1);
    // File > New (or Open) mid-stream; the answer finishes afterwards.
    h.state_mut().engine.replace_session(session());
    emit(&cb, ClaudeEvent::Text(" and the rest".into()));
    emit(&cb, ClaudeEvent::Done);
    h.run_steps(3);
    assert!(h.state().engine.session().scout.turns.is_empty(), "nothing stored in the new session");
    assert!(h.state().scout.asking.is_none(), "the question went with the old session");
    // Even if the answer is taken before the reset is handled, the session check drops it.
    h.state_mut().scout.question = "Q2".into();
    h.state_mut().run("ui.scout.send", json!({})).unwrap();
    emit(&cb, ClaudeEvent::Text("answer".into()));
    emit(&cb, ClaudeEvent::Done);
    h.state_mut().engine.replace_session(session());
    h.state_mut().engine.transport_requests.clear();
    h.run_steps(2);
    assert!(h.state().engine.session().scout.turns.is_empty());
}

#[test]
fn cancel_stops_waiting_and_keeps_the_question() {
    let (services, _, cb) = fake(Some("key"));
    let mut h = harness(SoundApp::new(Engine::new(session()), None, services));
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.state_mut().scout.question = "Too dark?".into();
    h.state_mut().run("ui.scout.send", json!({})).unwrap();
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().scout.asking.is_none());
    assert_eq!(h.state().scout.question, "Too dark?");
    // What the host still sends goes nowhere.
    emit(&cb, ClaudeEvent::Text("late".into()));
    emit(&cb, ClaudeEvent::Done);
    h.run_steps(2);
    assert!(h.state().engine.session().scout.turns.is_empty());
    assert!(h.query_by_label("Send").is_some());
}

#[test]
fn the_key_status_is_read_once_not_every_frame() {
    let reads = Arc::new(Mutex::new(0u32));
    let r2 = reads.clone();
    let services = Services {
        ask_claude: Some(Arc::new(|_, _| {})),
        api_key_status: Some(Arc::new(move || {
            *r2.lock().unwrap() += 1;
            None
        })),
        set_api_key: Some(Arc::new(|_| Ok(()))),
        ..Services::default()
    };
    let mut h = harness(SoundApp::new(Engine::new(session()), None, services));
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.state_mut().scout.settings_open = true;
    h.run_steps(10);
    assert_eq!(*reads.lock().unwrap(), 1);
    h.state_mut().scout.key_draft = "sk-ant-new".into();
    h.run_steps(1);
    h.get_by_label("Save key").click();
    h.run_steps(3);
    assert_eq!(*reads.lock().unwrap(), 2, "re-read once after a save");
}

//! A gesture (`execute_merged`) discarded with `undo_discard` brings back the Redo stack its first
//! step cleared: a fader or note drag that ends where it began costs no Redo.

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, TrackKind};

fn engine_with_redo() -> (Engine, u64) {
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None).0;
    e.execute("mix.pan", &json!({"track": t, "pan": 0.5})).unwrap();
    assert!(e.undo());
    assert_eq!(e.redo_label(), Some("Set Pan"));
    (e, t)
}

#[test]
fn a_discarded_gesture_keeps_the_redo_stack() {
    let (mut e, t) = engine_with_redo();
    let undo = e.undo_history().len();
    for db in [-1.0, -2.0, -0.0] {
        e.execute_merged("mix.volume", &json!({"track": t, "db": db}), "fader").unwrap();
    }
    assert!(!e.can_redo(), "a step clears Redo while the gesture lasts");
    assert!(e.undo_discard());
    e.end_merge();
    assert_eq!(e.undo_history().len(), undo, "the gesture left no step");
    assert_eq!(e.redo_label(), Some("Set Pan"), "and what there was to redo is back");
    assert!(e.redo());
}

#[test]
fn the_kept_redo_lasts_while_the_gesture_step_is_the_newest() {
    // The gesture ended (the app's per-frame end_merge runs before a release's discard): the
    // step is still the newest, so discarding it still brings Redo back.
    let (mut e, t) = engine_with_redo();
    e.execute_merged("mix.volume", &json!({"track": t, "db": -3.0}), "fader").unwrap();
    e.end_merge();
    assert!(e.undo_discard());
    assert_eq!(e.redo_label(), Some("Set Pan"));
    // A later step on top: discarding that one is a plain discard, Redo stays gone.
    let (mut e, t) = engine_with_redo();
    e.execute_merged("mix.volume", &json!({"track": t, "db": -3.0}), "fader").unwrap();
    e.end_merge();
    e.execute("mix.pan", &json!({"track": t, "pan": -0.5})).unwrap();
    assert!(e.undo_discard());
    assert!(!e.can_redo());
    // Undo in between drops it too.
    let (mut e, t) = engine_with_redo();
    e.execute_merged("mix.volume", &json!({"track": t, "db": -3.0}), "fader").unwrap();
    e.end_merge();
    assert!(e.undo());
    assert_eq!(e.redo_label(), Some("Set Volume"));
    // A plain step keeps nothing.
    let (mut e, t) = engine_with_redo();
    e.execute("mix.volume", &json!({"track": t, "db": -3.0})).unwrap();
    assert!(e.undo_discard());
    assert!(!e.can_redo());
}

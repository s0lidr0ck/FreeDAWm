use super::*;
use crate::Session;

fn filled() -> ScoutState {
    let mut m = ScoutMetrics { valid: true, rms_dbfs: -18.5, ..Default::default() };
    m.spectrum31[3] = -40.0;
    let strip = ScoutStrip {
        track: TrackId(7),
        name: "Kick".into(),
        kind: TrackKind::Audio,
        channel_type: "channel".into(),
        role: "Channel".into(),
        channels: 2,
        mixer: Mixer::new(crate::ChannelFormat::Stereo),
        automated: vec![AutomatedParam { module: ModuleId(2), param: "gain".into(), start: 0.0, end: 1.0 }],
        base: m.clone(),
        post: m,
        note: Some("n".into()),
        target: None,
    };
    ScoutState {
        goal: "Punchier".into(),
        captures: vec![ScoutCapture {
            id: 1,
            name: "Take".into(),
            created: "2026-10-20T00:00:00Z".into(),
            sample_rate: 48000.0,
            tempo: 120.0,
            played: vec![Range { start: 0, end: 480000 }],
            loop_range: Some(Range { start: 10, end: 20 }),
            played_truncated: true,
            strips: vec![strip],
            bands_version: 2,
        }],
        turns: vec![ScoutTurn {
            id: 2,
            question: "Why muddy?".into(),
            captures: vec![1],
            answer: "Cut 200 Hz".into(),
            notes: vec!["Guide 3 dropped: unknown track \"Bass\"".into()],
            guides: vec![
                Guide::Eq {
                    track: "Kick".into(),
                    lo_hz: 150.0,
                    hi_hz: 300.0,
                    gain_lo_db: -3.0,
                    gain_hi_db: -3.0,
                    dynamic: false,
                    q: Some(1.2),
                    label: "mud".into(),
                },
                Guide::Module {
                    track: "Kick".into(),
                    module: "Comp".into(),
                    param: "ratio".into(),
                    current: "2".into(),
                    range: "3-4".into(),
                    reason: "r".into(),
                },
            ],
            error: None,
            model: "m".into(),
        }],
        guides_shown: Some(2),
        next_id: 3,
    }
}

#[test]
fn old_sessions_load_with_an_empty_scout() {
    let mut v = serde_json::to_value(Session::default()).unwrap();
    v.as_object_mut().unwrap().remove("scout");
    let s: Session = serde_json::from_value(v).unwrap();
    assert_eq!(s.scout, ScoutState::default());
}

#[test]
fn scout_round_trips() {
    let s = Session { scout: filled(), ..Default::default() };
    let back: Session = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back.scout, s.scout);
    assert!(!back.scout.is_empty());
}

#[test]
fn empty_scout_is_not_written() {
    let v = serde_json::to_value(Session::default()).unwrap();
    assert!(v.get("scout").is_none());
}

#[test]
fn metrics_defaults_and_partial_json() {
    let m: ScoutMetrics = serde_json::from_str("{}").unwrap();
    assert_eq!(m.rms_dbfs, -100.0);
    assert_eq!(m.stereo_correlation, 1.0);
    assert_eq!(m.low_freq_correlation, 1.0);
    assert_eq!(m.mid_side_ratio_db, -100.0);
    let g: Guide = serde_json::from_str(r#"{"kind":"eq"}"#).unwrap();
    assert!(matches!(g, Guide::Eq { dynamic: false, q: None, .. }));
}

fn with_scout(v: serde_json::Value) -> Session {
    let mut s = serde_json::to_value(Session::default()).unwrap();
    s.as_object_mut().unwrap().insert("scout".into(), v);
    serde_json::from_value(s).unwrap()
}

#[test]
fn a_malformed_scout_block_loads_as_empty() {
    assert_eq!(with_scout(serde_json::json!({"goal": 1})).scout, ScoutState::default());
    assert_eq!(with_scout(serde_json::json!("junk")).scout, ScoutState::default());
}

#[test]
fn a_null_metric_takes_its_default_and_the_rest_survives() {
    let mut st = serde_json::to_value(filled()).unwrap();
    let post = &mut st["captures"][0]["strips"][0]["post"];
    post["rms_dbfs"] = serde_json::Value::Null;
    post["stereo_correlation"] = serde_json::Value::Null;
    st["captures"][0]["tempo"] = serde_json::Value::Null;
    st["turns"][0]["guides"][0]["lo_hz"] = serde_json::Value::Null;
    let s = with_scout(st);
    let strip = &s.scout.captures[0].strips[0];
    assert_eq!(strip.post.rms_dbfs, -100.0);
    assert_eq!(strip.post.stereo_correlation, 1.0);
    assert!(strip.post.valid);
    assert_eq!(strip.post.spectrum31[3], -40.0);
    assert_eq!(strip.base.rms_dbfs, -18.5);
    assert_eq!(s.scout.captures[0].tempo, 0.0);
    assert_eq!(s.scout.captures[0].sample_rate, 48000.0);
    assert_eq!(s.scout.goal, "Punchier");
}

#[test]
fn an_unknown_guide_kind_is_skipped() {
    let mut st = serde_json::to_value(filled()).unwrap();
    let g = st["turns"][0]["guides"].as_array_mut().unwrap();
    g.insert(0, serde_json::json!({"kind": "teleport"}));
    g.push(serde_json::json!(5));
    let s = with_scout(st);
    assert_eq!(s.scout.turns[0].guides, filled().turns[0].guides);
}

#[test]
fn captures_stored_before_v2_bands_load_as_v1() {
    let mut st = serde_json::to_value(filled()).unwrap();
    assert_eq!(st["captures"][0]["bands_version"], 2);
    st["captures"][0].as_object_mut().unwrap().remove("bands_version");
    let s = with_scout(st.clone());
    assert_eq!(s.scout.captures[0].bands_version, 1);
    // The stored results come back untouched: a capture holds results, never filter state.
    assert_eq!(s.scout.captures[0].strips, filled().captures[0].strips);
    st["captures"][0]["bands_version"] = serde_json::json!("junk");
    let s = with_scout(st);
    assert_eq!(s.scout.captures[0].bands_version, 1);
    assert_eq!(s.scout.goal, "Punchier");
}

#[test]
fn a_profile_round_trips_and_loads_leniently() {
    let mut m = ScoutMetrics { valid: true, lufs_integrated: -14.2, stereo_width: 0.4, ..Default::default() };
    m.spectrum31[10] = -22.5;
    let mut p = ScoutProfile::new("Big Chorus · vocals", m);
    p.kind = ProfileKind::Track;
    p.role = "vocal".into();
    p.tags = vec!["bright".into(), "pop".into()];
    p.source = ProfileSource {
        file: "song.wav".into(),
        start_s: 12.0,
        end_s: 42.0,
        stem: Some("vocals".into()),
        model: Some("demucs".into()),
        position: None,
    };
    p.created = "2026-10-30T00:00:00Z".into();
    assert_eq!(ScoutProfile::from_json(&p.to_json()).unwrap(), p);
    // Bad numbers keep their defaults; a bad kind and non-text tags are dropped; never a panic.
    let bad = r#"{"name":"  X ","kind":"stem","tags":["a",3,null],"bands_version":-1,"metrics":{"lufs_integrated":null,"rms_dbfs":"loud","spectrum31":[1,null,"x"]}}"#;
    let q = ScoutProfile::from_json(bad).unwrap();
    assert_eq!(q.name, "X");
    assert_eq!(q.kind, ProfileKind::Track);
    assert_eq!(q.tags, vec!["a".to_string()]);
    assert_eq!(q.bands_version, 2);
    assert_eq!(q.metrics.lufs_integrated, -100.0);
    assert!(ScoutProfile::from_json("[1,2]").is_err());
    assert!(ScoutProfile::from_json(r#"{"name":""}"#).is_err());
    assert!(ScoutProfile::from_json(&" ".repeat(MAX_PROFILE_BYTES + 1)).is_err());
}

#[test]
fn profiles_filter_by_role_and_words() {
    let mut p = ScoutProfile::new("Warm Bass DI", ScoutMetrics::default());
    p.role = "bass".into();
    p.tags = vec!["motown".into()];
    assert!(p.matches("", ""));
    assert!(p.matches("warm motown", "Bass"));
    assert!(!p.matches("bright", ""));
    assert!(!p.matches("", "vocal"));
    assert_eq!(profile_file_stem(" a/b:c? "), "a_b_c_");
    assert!(!profile_file_stem("../..").contains(['/', '.']));
    assert_eq!(profile_file_stem("  "), "profile");
    assert_eq!(p.source_text(), "");
}

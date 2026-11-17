//! The instrument's parameters as automation lanes: the lane picker lists them under the
//! instrument's name, a lane shown stays under the track, and its label names the instrument.

use serde_json::json;
use soundcraft_model::{AutoParam, INSTRUMENT_MODULE};
use soundcraft_ui_egui::{auto_lanes, edit_window};

#[test]
fn the_instruments_parameters_are_offered_as_lanes() {
    let mut e = soundcraft_engine::Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap();
    let keys = e.session().track_by_name("Keys").unwrap().clone();
    let synth = keys.instrument.as_ref().map(|i| i.plugin.clone()).expect("an instrument");
    let level = AutoParam::Plugin { module: INSTRUMENT_MODULE, param: "level".into() };
    let all = auto_lanes::choices(e.session(), &keys);
    let c = all.iter().find(|c| c.param == level).unwrap_or_else(|| panic!("the instrument's level is offered ({synth})"));
    let group = c.group.clone().expect("grouped under the instrument");
    e.execute("track.auto_lane", &json!({"track": keys.id.0, "param": level.id(), "show": true})).unwrap();
    e.execute("automation.set_point", &json!({"track": keys.id.0, "param": level.id(), "at": 0, "value": -12.0})).unwrap();
    let keys = e.session().track(keys.id).unwrap();
    assert_eq!(keys.lane(&level).unwrap().points[0].value, -12.0);
    assert_eq!(auto_lanes::shown_lanes(keys), std::slice::from_ref(&level));
    assert!(edit_window::view_label(keys, &level.id()).starts_with(&group), "{}", edit_window::view_label(keys, &level.id()));
}

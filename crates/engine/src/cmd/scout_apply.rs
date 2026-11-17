//! Scout builds a starting chain from its answer, one suggestion at a time and only when asked:
//! `scout.plan` says what each suggestion of an answer would do, `scout.apply` does one of them
//! as one undo step. An EQ guide becomes a band on the track's Clean Q (a Clean Q is added at the
//! end of the chain when the track has none); a module suggestion sets that parameter to the
//! middle of the suggested range (the module is added when the track has none). Nothing is sent
//! anywhere: only the answer already received is read.

use super::module::{build_insert, chain_add, do_params, find_mut};
use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_dsp::Unit;
use soundcraft_dsp::plugins::fs as firestation;
use soundcraft_model::{Guide, ModuleId, Session, TrackId};

use firestation::cleanq::clean_q_dsp as q;

const CLEANQ: &str = "fs_cleanq";
/// Clean Q's bands.
const BANDS: usize = 24;
/// Clean Q's band types, by index (`TYPE_CHOICES`).
const BELL: f32 = 0.0;
const LOW_SHELF: f32 = 1.0;
const HIGH_SHELF: f32 = 2.0;
/// A band's `on` value when active.
const ACTIVE: f32 = 2.0;
/// Where a dynamic band starts acting, until set by ear.
const DYN_THRESHOLD_DB: f32 = -20.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "scout.plan",
            "Scout Plan",
            [],
            None,
            "{turn: answer id} — what Apply would do with each suggestion of that answer, without doing it → {steps: [{guide, track, module, action, applicable, reason?}]}",
            always,
            plan
        ),
        cmd!(
            "scout.apply",
            "Apply Scout Suggestion",
            [],
            None,
            "{turn: answer id, guide: index (from 0)} — applies one suggestion of that answer, as one undo step: an EQ guide becomes a Clean Q band on its track (a Clean Q is added at the end of the chain if it has none); a module suggestion sets the parameter to the middle of its suggested range (the module is added if missing). Uses only the answer already received → {track, module, added, params, action, note?}",
            always,
            apply
        ),
    ]
}

/// One suggestion, worked out: where it goes and the values it sets.
#[derive(Debug, Clone, PartialEq)]
struct Step {
    track: TrackId,
    plugin: &'static str,
    /// The module it changes, or `None` to add one.
    module: Option<ModuleId>,
    params: Vec<(String, f32)>,
    action: String,
    note: Option<String>,
}

fn turn_guides<'a>(s: &'a Session, cmd: &str, p: &Value) -> Result<&'a [Guide]> {
    let id = p.get("turn").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`turn` (an answer id) required"))?;
    s.scout
        .turns
        .iter()
        .find(|t| u64::from(t.id) == id)
        .map(|t| t.guides.as_slice())
        .ok_or_else(|| crate::EngineError::NotFound(format!("{cmd}: no Scout answer {id}")))
}

fn track_named(s: &Session, name: &str) -> Option<TrackId> {
    s.tracks.iter().find(|t| t.name.eq_ignore_ascii_case(name.trim())).map(|t| t.id)
}

/// The first module of `plugin` on the track's chain.
fn first_module(s: &Session, t: TrackId, plugin: &str) -> Option<ModuleId> {
    s.track(t)?.mixer.inserts.iter().find(|m| m.plugin == plugin).map(|m| m.id)
}

fn step(s: &Session, g: &Guide) -> std::result::Result<Step, String> {
    match g {
        Guide::Eq { track, lo_hz, hi_hz, gain_lo_db, gain_hi_db, dynamic, q: q_hint, label } => {
            let t = track_named(s, track).ok_or_else(|| format!("no track named \"{track}\""))?;
            let (lo, hi) = (*lo_hz, *hi_hz);
            if !(lo.is_finite() && hi.is_finite() && lo >= 20.0 && hi <= 20_000.0 && lo < hi) {
                return Err(format!("{lo}–{hi} Hz is not a usable range"));
            }
            let gain = (gain_lo_db + gain_hi_db) / 2.0;
            if !gain.is_finite() || gain.abs() > 30.0 {
                return Err("its gain is not usable".into());
            }
            let module = first_module(s, t, CLEANQ);
            // The first band that is off, on the Clean Q there is (band 1 on a new one).
            let band = match module.and_then(|m| s.track(t)?.mixer.module(m)) {
                Some(ins) => (1..=BANDS)
                    .find(|b| ins.params.get(&format!("b{b:02}_on")).copied().unwrap_or(0.0) < 0.5)
                    .ok_or("its Clean Q has no free band")?,
                None => 1,
            };
            let center = (lo * hi).sqrt();
            let (kind, freq, kind_name) = if lo <= 25.0 && hi < 1_000.0 {
                (LOW_SHELF, hi, "low shelf")
            } else if hi >= 19_000.0 && lo > 1_000.0 {
                (HIGH_SHELF, lo, "high shelf")
            } else {
                (BELL, center, "bell")
            };
            let qv = q_hint.filter(|v| v.is_finite()).unwrap_or(center / (hi - lo)).clamp(0.1, 20.0);
            let k = format!("b{band:02}_");
            let mut params = vec![
                (format!("{k}on"), ACTIVE),
                (format!("{k}type"), kind),
                (format!("{k}freq"), q::freq_to_knob(f64::from(freq)).clamp(0.0, 1.0)),
                (format!("{k}q"), q::q_to_knob(f64::from(qv)).clamp(0.0, 1.0)),
            ];
            let mut note = None;
            let what = if *dynamic {
                params.push((format!("{k}dyn"), 1.0));
                params.push((format!("{k}gain"), q::gain_to_knob(0.0)));
                params.push((format!("{k}range"), gain));
                params.push((format!("{k}thr"), DYN_THRESHOLD_DB));
                note = Some(format!("The threshold is left at {DYN_THRESHOLD_DB} dB: set it so the band acts only when the problem is there"));
                format!("dynamic {kind_name}, up to {gain:+.1} dB")
            } else {
                params.push((format!("{k}dyn"), 0.0));
                params.push((format!("{k}gain"), q::gain_to_knob(f64::from(gain)).clamp(0.0, 1.0)));
                format!("{kind_name} {gain:+.1} dB")
            };
            let at = if freq >= 1000.0 { format!("{:.1} kHz", freq / 1000.0) } else { format!("{freq:.0} Hz") };
            let mut action = format!("Clean Q band {band}: {what} at {at}");
            if !label.is_empty() {
                action.push_str(&format!(" ({label})"));
            }
            Ok(Step { track: t, plugin: CLEANQ, module, params, action, note })
        }
        Guide::Module { track, module, param, range, .. } => {
            let t = track_named(s, track).ok_or_else(|| format!("no track named \"{track}\""))?;
            let info = soundcraft_dsp::plugin_info(module.trim())
                .or_else(|| soundcraft_dsp::plugins().iter().copied().find(|i| i.name.eq_ignore_ascii_case(module.trim())))
                .filter(|i| !i.is_instrument)
                .ok_or_else(|| format!("\"{module}\" is not a built-in effect"))?;
            let pi = info
                .params
                .iter()
                .find(|x| x.id.eq_ignore_ascii_case(param.trim()) || x.name.eq_ignore_ascii_case(param.trim()))
                .ok_or_else(|| format!("{} has no parameter \"{param}\"", info.name))?;
            // A value on a knob scale with its own display, or a choice, cannot be read from
            // the suggestion's numbers: those are set by hand.
            if pi.display.is_some() || matches!(pi.unit, Unit::Choice | Unit::Toggle) {
                return Err(format!("{} {} is set by hand: its suggested values are not on the parameter's own scale", info.name, pi.name));
            }
            let nums = numbers(range);
            let target = match nums.as_slice() {
                [a] => *a,
                [a, b, ..] => (a + b) / 2.0,
                [] => return Err(format!("no number in the suggested range \"{range}\"")),
            };
            let v = pi.clamp(target);
            let module_id = first_module(s, t, info.id);
            let mut action = format!("{} {}: {}", info.name, pi.name, pi.format(v));
            if (v - target).abs() > 1e-3 {
                action.push_str(" (the nearest it goes)");
            }
            Ok(Step { track: t, plugin: info.id, module: module_id, params: vec![(pi.id.to_string(), v)], action, note: None })
        }
    }
}

/// The numbers in a free-text range: "-18 to -12 dB" is -18 and -12, "30-50 ms" is 30 and 50
/// (a dash after a digit is a range), "−3" with a typographic minus is -3, and of a ratio
/// "2:1–4:1" only the first numbers count (2 and 4).
fn numbers(text: &str) -> Vec<f32> {
    let c: Vec<char> = text.chars().map(|c| if c == '\u{2212}' { '-' } else { c }).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let prev = i.checked_sub(1).and_then(|k| c.get(k)).copied();
        let digit_next = c.get(i + 1).is_some_and(char::is_ascii_digit);
        let negative = c.get(i) == Some(&'-') && digit_next && !prev.is_some_and(|p| p.is_ascii_digit());
        if c.get(i).is_some_and(char::is_ascii_digit) || negative {
            let start = i;
            i += 1;
            while c.get(i).is_some_and(|d| d.is_ascii_digit() || *d == '.') {
                i += 1;
            }
            let after_colon = prev == Some(':') && !negative;
            if let Ok(v) = c.get(start..i).unwrap_or_default().iter().collect::<String>().parse::<f32>()
                && v.is_finite()
                && !after_colon
            {
                out.push(v);
            }
        } else {
            i += 1;
        }
    }
    out
}

fn plan(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.plan";
    let s = e.session();
    let guides = turn_guides(s, C, p)?;
    let steps: Vec<Value> = guides
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let track = match g {
                Guide::Eq { track, .. } | Guide::Module { track, .. } => track.clone(),
            };
            match step(s, g) {
                Ok(st) => json!({"guide": i, "track": track, "module": st.plugin, "action": st.action, "applicable": true, "note": st.note}),
                Err(why) => json!({"guide": i, "track": track, "applicable": false, "reason": why}),
            }
        })
        .collect();
    Ok(json!({"steps": steps}))
}

fn apply(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.apply";
    let i = p.get("guide").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(C, "`guide` (an index) required"))?;
    let st = {
        let s = e.session();
        let g = turn_guides(s, C, p)?.get(i).ok_or_else(|| bad(C, format!("that answer has no suggestion {i}")))?;
        step(s, g).map_err(|why| bad(C, why))?
    };
    let added = st.module.is_none();
    let module = match st.module {
        Some(m) => m,
        None => {
            let ins = build_insert(C, st.plugin, None)?;
            let tr = e.session_mut().track_mut(st.track).ok_or_else(|| bad(C, "no track"))?;
            chain_add(tr, C, ins, None)?.0
        }
    };
    let values: serde_json::Map<String, Value> = st.params.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    do_params(e, C, st.track, module, &json!({"params": values}))?;
    // The module is where the user will look next.
    let _ = find_mut(e, C, st.track, module)?;
    Ok(json!({"track": st.track, "module": module, "plugin": st.plugin, "added": added, "params": values, "action": st.action, "note": st.note}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    fn engine(guides: Vec<Guide>) -> (Engine, TrackId) {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        e.execute("scout.answer", &json!({"turn": {"question": "q", "captures": [], "answer": "a"}})).unwrap();
        let id = e.session().scout.turns[0].id;
        e.session_mut().scout.turns[0].guides = guides;
        assert_eq!(id, e.session().scout.turns[0].id);
        (e, t)
    }

    fn eq(lo: f32, hi: f32, a: f32, b: f32, dynamic: bool) -> Guide {
        Guide::Eq { track: "vox".into(), lo_hz: lo, hi_hz: hi, gain_lo_db: a, gain_hi_db: b, dynamic, q: None, label: "harsh".into() }
    }

    #[test]
    fn each_suggestion_is_applied_alone_as_one_undo_step() {
        let (mut e, t) = engine(vec![eq(2500.0, 4000.0, -4.0, -2.0, false), eq(200.0, 400.0, -3.0, -3.0, true)]);
        let turn = e.session().scout.turns[0].id;
        let plan = e.execute("scout.plan", &json!({"turn": turn})).unwrap();
        assert_eq!(plan["steps"][0]["applicable"], json!(true), "{plan}");
        assert!(plan["steps"][0]["action"].as_str().unwrap().contains("band 1: bell -3.0 dB at 3.2 kHz"), "{plan}");
        // Planning changes nothing.
        assert_eq!(e.session().track(t).unwrap().mixer.inserts.len(), 0);

        let r = e.execute("scout.apply", &json!({"turn": turn, "guide": 0})).unwrap();
        assert_eq!(r["added"], json!(true));
        let m = ModuleId(r["module"].as_u64().unwrap() as _);
        let band = |e: &Engine, k: &str| e.session().track(t).unwrap().mixer.module(m).unwrap().params[k];
        assert_eq!(band(&e, "b01_on"), ACTIVE);
        assert!((q::knob_to_freq(band(&e, "b01_freq")) - (2500.0f64 * 4000.0).sqrt()).abs() < 1.0);
        assert!((q::knob_to_gain_db(band(&e, "b01_gain")) + 3.0).abs() < 1e-3);
        // The second goes on the same Clean Q, in the next free band, as a dynamic band.
        let r = e.execute("scout.apply", &json!({"turn": turn, "guide": 1})).unwrap();
        assert_eq!((r["added"].as_bool(), r["module"].as_u64()), (Some(false), Some(u64::from(m.0))));
        assert_eq!(band(&e, "b02_dyn"), 1.0);
        assert_eq!(band(&e, "b02_range"), -3.0);
        assert!(r["note"].as_str().unwrap().contains("threshold"));
        // Each was its own undo step: one undo takes back band 2 only.
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(band(&e, "b02_on"), 0.0);
        assert_eq!(band(&e, "b01_on"), ACTIVE);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(t).unwrap().mixer.inserts.len(), 0, "the Clean Q it added goes too");
    }

    #[test]
    fn module_suggestions_set_real_units_and_refuse_the_rest() {
        let module = |m: &str, param: &str, range: &str| Guide::Module {
            track: "Vox".into(),
            module: m.into(),
            param: param.into(),
            current: String::new(),
            range: range.into(),
            reason: String::new(),
        };
        // A parameter in real units (dB) of a built-in effect, found by id or name.
        let real = soundcraft_dsp::plugins()
            .iter()
            .filter(|i| !i.is_instrument)
            .find_map(|i| i.params.iter().find(|p| p.unit == Unit::Db && p.display.is_none() && p.min < -10.0 && p.max > -5.0).map(|p| (i, p)))
            .expect("some built-in effect has a plain dB parameter");
        let (info, pi) = real;
        let (mut e, t) = engine(vec![
            module(info.name, pi.name, "-10 to −6 dB"),
            module("NoSuchThing", "x", "1"),
            module(info.id, pi.id, "no numbers here"),
            module(info.id, "nope", "1"),
        ]);
        let turn = e.session().scout.turns[0].id;
        let r = e.execute("scout.apply", &json!({"turn": turn, "guide": 0})).unwrap();
        let m = ModuleId(r["module"].as_u64().unwrap() as _);
        let v = e.session().track(t).unwrap().mixer.module(m).unwrap().params[pi.id];
        assert!((v - pi.clamp(-8.0)).abs() < 1e-4, "{v}");
        let plan = e.execute("scout.plan", &json!({"turn": turn})).unwrap();
        for i in 1..4 {
            assert_eq!(plan["steps"][i]["applicable"], json!(false), "{plan}");
            assert!(e.execute("scout.apply", &json!({"turn": turn, "guide": i})).is_err());
        }
        assert!(e.execute("scout.apply", &json!({"turn": turn, "guide": 9})).is_err());
        assert!(e.execute("scout.apply", &json!({"turn": 999, "guide": 0})).is_err());
    }

    #[test]
    fn ranges_are_read_as_numbers() {
        assert_eq!(numbers("-18 to -12 dB"), vec![-18.0, -12.0]);
        assert_eq!(numbers("−3 dB"), vec![-3.0]);
        assert_eq!(numbers("2:1–4:1"), vec![2.0, 4.0]);
        assert_eq!(numbers("30-50 ms"), vec![30.0, 50.0]);
        assert!(numbers("more").is_empty());
    }
}

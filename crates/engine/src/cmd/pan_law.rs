//! Setup › Preferences… › Mixing › Pan Law: how loud a mono source panned to the centre of a stereo (or wider)
//! output plays, as each of the four reference DAWs lets you choose. The mix reads it every block
//! (`soundcraft_mix::session_pan_law`).

use super::*;
use crate::cmd;
use serde_json::{Value, json};

/// The laws offered, by their centre level in dB.
pub const LAWS: [f64; 4] = [0.0, -3.0, -4.5, -6.0];

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "mix.pan_law",
        "Pan Law",
        ["Options", "More", "Pan Law"],
        None,
        "{law: 0|-3|-4.5|-6 — the centre level in dB of a mono source panned to the middle} → {law}",
        always,
        set
    )]
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    let law = match p.get("law") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().trim_end_matches("dB").trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|db| LAWS.contains(db))
    .ok_or_else(|| bad("mix.pan_law", "`law` is 0, -3, -4.5 or -6"))?;
    e.session_mut().edit.values.insert(soundcraft_mix::PAN_LAW_KEY.into(), law);
    Ok(json!({"law": law}))
}

/// The session's pan law, its centre level in dB.
pub fn current(s: &soundcraft_model::Session) -> f64 {
    f64::from(soundcraft_mix::session_pan_law(s).center_db())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pan_law_is_one_of_four_and_undoes() {
        let mut e = Engine::default();
        assert_eq!(current(e.session()), -3.0, "the default");
        e.execute("mix.pan_law", &json!({"law": -6})).unwrap();
        assert_eq!(current(e.session()), -6.0);
        e.execute("mix.pan_law", &json!({"law": "-4.5 dB"})).unwrap();
        assert_eq!(current(e.session()), -4.5);
        assert!(e.execute("mix.pan_law", &json!({"law": -2})).is_err());
        assert!(e.execute("mix.pan_law", &json!({})).is_err());
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(current(e.session()), -6.0);
    }
}
